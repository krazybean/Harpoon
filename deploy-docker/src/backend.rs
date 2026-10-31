//! `DockerBackend`: the first real implementation of `harpoon_deploy::backend::Backend`, for the
//! **first disposable deployment only**. Release replacement (`stop_current`, `restore_previous`)
//! is deliberately not implemented here.
//!
//! Rules this file enforces:
//! * every create is preceded by a durable ledger intent and followed by a receipt with the id
//!   Docker returned (via `StepIo`);
//! * nothing is removed or stopped without re-reading the object by id and re-checking its
//!   identity (id, name) and all three Harpoon labels;
//! * the only builder is the bundled Buildx; the only engine is the configured socket; the
//!   environment (`DOCKER_HOST`, contexts, config) is never consulted; nothing is ever pruned;
//! * health evidence comes only from real observations of the published port.

use crate::api::{ApiError, ContainerInfo, CreateOutcome, CreateSpec, DockerApi, Limits};
use crate::buildx_run::{run_build, BuildRequest, BuildRunError, StdFileProbe};
use crate::confine::{confine, Source};
use crate::dockerfile::required_images;
use crate::ignore::{always_sent, IgnoreRules};
use crate::preflight::{context_size_with, DfProbe, DiskProbe};
use crate::probe::{
    BindProbe, ContainerProbe, HttpTransport, PortProbe, ResponseMarker, SleepWaiter, TcpHttp,
};
use harpoon_deploy::backend::{Backend, ResourceOps, RestoreResult, StepError};
use harpoon_deploy::buildx::{self, BuildxBinary, BuildxError};
use harpoon_deploy::engine::StepIo;
use harpoon_deploy::health::{run_health, CheckEvidence, HealthSpec, Verdict};
use harpoon_deploy::ids::{labels, DeploymentId};
use harpoon_deploy::ledger::{
    CleanupOutcome, Disposition, Observed, RemoveRequest, RemoveResult, ResourceKey, ResourceKind,
};
use harpoon_deploy::ports::{self, Occupancy, PortLease, PortRange, Purpose};
use harpoon_deploy::state::Reason;
use harpoon_deploy::time::SystemClock;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct DockerConfig {
    /// The one Docker socket this backend talks to.
    pub socket: PathBuf,
    /// Directory holding the source tree to build.
    pub context_dir: PathBuf,
    /// The project folder the user selected. The build context and the Dockerfile must resolve
    /// inside it. `None` means the context folder itself is the selected project.
    pub project_root: Option<PathBuf>,
    /// Dockerfile name inside `context_dir`.
    pub dockerfile: String,
    /// Empty, Harpoon-owned directory used as HOME/DOCKER_CONFIG for Buildx.
    pub scratch_dir: PathBuf,
    pub container_port: u16,
    pub health: HealthSpec,
    /// When set, a health response only counts if it carries this marker (see [`ResponseMarker`]).
    pub health_marker: Option<ResponseMarker>,
    pub env: Vec<String>,
    pub limits: Limits,
    pub max_context_bytes: u64,
    pub min_free_bytes: u64,
    pub api_timeout: Duration,
    /// Time allowed for one removal outside a step (recovery, post-deployment cleanup).
    pub cleanup_timeout: Duration,
    pub log_tail_lines: u32,
    pub log_max_bytes: usize,
}

impl DockerConfig {
    /// The approved first-test limits: 256 MiB / 0.5 CPU / 64 PIDs, 256 MB context, 2 GiB free,
    /// health on `/` with 2 consecutive successes inside 60 s.
    pub fn first_test(
        socket: impl Into<PathBuf>,
        context_dir: impl Into<PathBuf>,
        scratch_dir: impl Into<PathBuf>,
        container_port: u16,
    ) -> DockerConfig {
        DockerConfig {
            socket: socket.into(),
            context_dir: context_dir.into(),
            project_root: None,
            dockerfile: "Dockerfile".into(),
            scratch_dir: scratch_dir.into(),
            container_port,
            health: HealthSpec::first_test("/"),
            health_marker: None,
            env: Vec::new(),
            limits: Limits::FIRST_TEST,
            max_context_bytes: 256 * 1000 * 1000,
            min_free_bytes: 2 * 1024 * 1024 * 1024,
            api_timeout: Duration::from_secs(30),
            cleanup_timeout: Duration::from_secs(60),
            log_tail_lines: 200,
            log_max_bytes: 256 * 1024,
        }
    }
}

#[derive(Debug, Default)]
struct State {
    image_tag: Option<String>,
    pre: Option<(String, u16)>, // container name, test port
    app: Option<(String, u16)>, // container name, application port
    replaced_from: Option<u16>,
}

pub struct DockerBackend<T: HttpTransport = TcpHttp> {
    cfg: DockerConfig,
    api: DockerApi,
    buildx: BuildxBinary,
    transport: T,
    ports: Box<dyn PortProbe>,
    disk: Box<dyn DiskProbe>,
    st: State,
}

/// Step-scoped time budget on the real clock.
struct Budget {
    start: Instant,
    total: Duration,
}

impl Budget {
    fn new(ms: u64) -> Budget {
        Budget {
            start: Instant::now(),
            total: Duration::from_millis(ms),
        }
    }
    fn left(&self, what: &str) -> Result<Duration, StepError> {
        let left = self.total.saturating_sub(self.start.elapsed());
        if left.is_zero() {
            Err(StepError::timeout(format!(
                "the step's time budget ran out before {what}"
            )))
        } else {
            Ok(left)
        }
    }
}

fn api_err(op: &str, e: ApiError) -> StepError {
    let m = format!("{op}: {e}");
    if e.is_timeout() {
        StepError::timeout(m)
    } else {
        StepError::new(m)
    }
}

fn all_labels(
    app: &harpoon_deploy::ids::ApplicationId,
    dep: &DeploymentId,
) -> Vec<(String, String)> {
    labels::for_deployment(app, dep)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

fn has_labels(
    have: &[(String, String)],
    app: &harpoon_deploy::ids::ApplicationId,
    dep: &DeploymentId,
) -> bool {
    all_labels(app, dep)
        .iter()
        .all(|(k, v)| have.iter().any(|(hk, hv)| hk == k && hv == v))
}

/// Docker repository names must be lower-case; the identifier alphabet is upper-case.
pub fn image_tag(app: &harpoon_deploy::ids::ApplicationId, dep: &DeploymentId) -> String {
    format!("harpoon/{}:{}", app.as_str().to_ascii_lowercase(), dep)
}

pub fn preliminary_name(dep: &DeploymentId) -> String {
    format!("hp-{dep}-pre")
}
pub fn serving_name(dep: &DeploymentId) -> String {
    format!("hp-{dep}")
}
pub fn replacement_name(dep: &DeploymentId) -> String {
    format!("hp-{dep}-r2")
}

impl DockerBackend<TcpHttp> {
    /// Resolve the bundled Buildx (`<Resources>/harpoon/bin/docker-buildx`) or fail with the
    /// explicit, no-fallback error.
    pub fn new(cfg: DockerConfig, resources_dir: Option<&Path>) -> Result<Self, BuildxError> {
        Self::with_transport(cfg, resources_dir, TcpHttp)
    }
}

impl<T: HttpTransport> DockerBackend<T> {
    /// Like [`DockerBackend::new`] with an injected HTTP transport. A transport that reports
    /// `is_simulated()` makes this backend's evidence simulated, which the engine rejects unless
    /// the backend accepts it — and a backend only accepts it if its transport is simulated.
    pub fn with_transport(
        cfg: DockerConfig,
        resources_dir: Option<&Path>,
        transport: T,
    ) -> Result<Self, BuildxError> {
        let buildx = buildx::resolve(resources_dir, &StdFileProbe)?;
        let api = DockerApi::new(&cfg.socket, cfg.api_timeout);
        Ok(DockerBackend {
            cfg,
            api,
            buildx,
            transport,
            ports: Box::new(BindProbe),
            disk: Box::new(DfProbe),
            st: State::default(),
        })
    }

    pub fn with_port_probe(mut self, p: impl PortProbe + 'static) -> Self {
        self.ports = Box::new(p);
        self
    }
    pub fn with_disk_probe(mut self, d: impl DiskProbe + 'static) -> Self {
        self.disk = Box::new(d);
        self
    }
    pub fn buildx_path(&self) -> &Path {
        &self.buildx.path
    }

    // ---------------------------------------------------------------- shared helpers

    fn confined_source(&self) -> Result<Source, StepError> {
        let root = self
            .cfg
            .project_root
            .as_deref()
            .unwrap_or(&self.cfg.context_dir);
        confine(root, &self.cfg.context_dir, &self.cfg.dockerfile)
            .map_err(|e| StepError::new(e.to_string()))
    }

    fn occupancy(
        &self,
        io: &StepIo<'_>,
        purpose: Purpose,
        b: &Budget,
    ) -> Result<Occupancy, StepError> {
        let docker_mappings = self
            .api
            .published_ports(b.left("reading Docker's port mappings")?)
            .map_err(|e| api_err("reading Docker's port mappings", e))?;
        let r = PortRange::for_purpose(purpose);
        let host_listeners: BTreeSet<u16> = (r.start..=r.end)
            .filter(|p| self.ports.is_listening(*p))
            .collect();
        Ok(Occupancy {
            host_listeners,
            docker_mappings,
            ..Occupancy::default()
        }
        .with_ledger(io.ledger()))
    }

    fn receipted_id(io: &StepIo<'_>, kind: ResourceKind, name: &str) -> Option<String> {
        io.ledger()
            .find(io.deployment(), &ResourceKey::new(kind, name))
            .and_then(|e| e.id.clone())
    }

    /// Create a labelled container: intent → create → receipt. A name that is already taken is
    /// never taken over: the intent stays unresolved and the engine's reconciliation decides
    /// (adopt if it carries this deployment's labels, refuse otherwise).
    fn create_container(
        &mut self,
        io: &mut StepIo<'_>,
        b: &Budget,
        name: &str,
        disposition: Disposition,
        lease: &PortLease,
    ) -> Result<String, StepError> {
        let tag = self
            .st
            .image_tag
            .clone()
            .ok_or_else(|| StepError::new("no image was built"))?;
        let lbl = all_labels(io.app(), io.deployment());
        // Docker creates an anonymous volume per declared image volume (inherited ones included)
        // even with no mounts in the request, and this ledger does not manage volumes. Refuse
        // before recording any container intent, so nothing is created.
        let img = self
            .api
            .image_inspect(&tag, b.left("checking the image's declared volumes")?)
            .map_err(|e| api_err("checking the image's declared volumes", e))?
            .ok_or_else(|| StepError::new(format!("image {tag} no longer exists")))?;
        if !img.volumes.is_empty() {
            return Err(StepError::new(format!(
                "image {tag} declares volumes ({}); Harpoon's first slice does not manage volumes, so no container was created. Remove the VOLUME instruction (including any inherited from the base image).",
                img.volumes.join(", ")
            )));
        }
        let intent = io.intend(ResourceKey::new(ResourceKind::Container, name), disposition)?;
        let spec = CreateSpec {
            image: &tag,
            labels: &lbl,
            env: &self.cfg.env,
            container_port: lease.container_port,
            host_port: lease.host_port,
            limits: self.cfg.limits,
        };
        match self
            .api
            .create_container(name, &spec, b.left("creating the container")?)
            .map_err(|e| api_err(&format!("creating container {name}"), e))?
        {
            CreateOutcome::Created(id) => {
                io.receipt(&intent, &id)?;
                Ok(id)
            }
            CreateOutcome::NameInUse(why) => Err(StepError::new(format!(
                "the container name {name} is already in use by something this deployment did not create; it was not touched ({why})"
            ))),
        }
    }

    /// Read the object back and check it is what the ledger says it is. Nothing is stopped or
    /// removed on the strength of a name or a stale listing.
    fn verify_container(&self, req: &RemoveRequest, left: Duration) -> Result<Verified, StepError> {
        let Some(info) = self
            .api
            .container_inspect(&req.id, left)
            .map_err(|e| api_err("re-checking the container before changing it", e))?
        else {
            return Ok(Verified::Absent);
        };
        if info.id != req.id {
            return Ok(Verified::Refused(format!(
                "the object behind id {} reports a different id; not touched",
                req.id
            )));
        }
        if info.name != req.name {
            return Ok(Verified::Refused(format!(
                "container {} is now named {}, not {}; not touched",
                short(&info.id),
                info.name,
                req.name
            )));
        }
        if !has_labels(&info.labels, &req.app, &req.deployment) {
            return Ok(Verified::Refused(format!(
                "container {} no longer carries this deployment's Harpoon labels (managed, app, deployment); not touched",
                req.name
            )));
        }
        Ok(Verified::Ok(info))
    }

    fn attach_logs(&self, io: &mut StepIo<'_>, b: &Budget, id: &str) {
        let Ok(left) = b.left("reading container logs") else {
            return;
        };
        match self
            .api
            .logs(id, self.cfg.log_tail_lines, self.cfg.log_max_bytes, left)
        {
            Ok(l) => {
                io.log.push_line("--- container output (last lines) ---");
                io.log.push_text(&l.text);
                if l.truncated {
                    io.log
                        .push_line("--- container output was cut at the size cap ---");
                }
            }
            Err(e) => io
                .log
                .push_line(&format!("(could not read container logs: {e})")),
        }
    }

    fn health(
        &mut self,
        io: &mut StepIo<'_>,
        b: &Budget,
        id: &str,
        port: u16,
        what: &str,
    ) -> Result<CheckEvidence, StepError> {
        let spec = HealthSpec {
            startup_deadline_ms: self
                .cfg
                .health
                .startup_deadline_ms
                .min(b.total.as_millis() as u64),
            ..self.cfg.health.clone()
        };
        let target = format!("http://127.0.0.1:{port}{}", spec.path);
        let mut probe = ContainerProbe {
            api: &self.api,
            container_id: id,
            port,
            path: &spec.path,
            transport: &self.transport,
            marker: self.cfg.health_marker.as_ref(),
            attempt_timeout: Duration::from_secs(2).min(self.cfg.api_timeout),
        };
        let mut ev = run_health(&spec, &target, &mut probe, &SystemClock, &SleepWaiter);
        ev.simulated = self.transport.is_simulated();
        if ev.verdict == Verdict::Healthy && ev.validate().is_ok() {
            return Ok(ev);
        }
        self.attach_logs(io, b, id);
        let msg = format!("{what} did not pass: {}", ev.summary());
        Err(if ev.verdict == Verdict::NotHealthy {
            StepError::timeout(msg)
        } else {
            StepError::new(msg)
        }
        .with_evidence(ev))
    }
}

enum Verified {
    Absent,
    Refused(String),
    Ok(ContainerInfo),
}

fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

// ------------------------------------------------------------------ ResourceOps

impl<T: HttpTransport> ResourceOps for DockerBackend<T> {
    fn list_resources(&mut self) -> Result<Vec<Observed>, StepError> {
        let left = self.cfg.cleanup_timeout;
        let mut out = Vec::new();
        for c in self
            .api
            .list_containers(left)
            .map_err(|e| api_err("listing containers", e))?
        {
            out.push(Observed {
                kind: ResourceKind::Container,
                name: c.name,
                id: c.id,
                labels: c.labels,
            });
        }
        for (id, name, labels) in self
            .api
            .list_images(left)
            .map_err(|e| api_err("listing images", e))?
        {
            out.push(Observed {
                kind: ResourceKind::Image,
                name,
                id,
                labels,
            });
        }
        Ok(out)
    }

    fn remove_resource(&mut self, req: &RemoveRequest) -> Result<RemoveResult, StepError> {
        let budget = Budget {
            start: Instant::now(),
            total: self.cfg.cleanup_timeout,
        };
        match req.kind {
            ResourceKind::PortLease => Ok(RemoveResult::Refused(
                "a port lease is a ledger reservation, not a Docker object".into(),
            )),
            ResourceKind::Container => {
                let info =
                    match self.verify_container(req, budget.left("re-checking the container")?)? {
                        Verified::Absent => return Ok(RemoveResult::AlreadyAbsent),
                        Verified::Refused(why) => return Ok(RemoveResult::Refused(why)),
                        Verified::Ok(i) => i,
                    };
                if info.running {
                    let stopped = self
                        .api
                        .stop_container(&req.id, 10, budget.left("stopping the container")?)
                        .map_err(|e| api_err(&format!("stopping {}", req.name), e))?;
                    if !stopped {
                        return Ok(RemoveResult::AlreadyAbsent);
                    }
                }
                match self
                    .api
                    .remove_container(&req.id, budget.left("removing the container")?)
                    .map_err(|e| api_err(&format!("removing {}", req.name), e))?
                {
                    true => Ok(RemoveResult::Removed),
                    false => Ok(RemoveResult::AlreadyAbsent),
                }
            }
            ResourceKind::Image => {
                let Some(img) = self
                    .api
                    .image_inspect(&req.id, budget.left("re-checking the image")?)
                    .map_err(|e| api_err("re-checking the image before removing it", e))?
                else {
                    return Ok(RemoveResult::AlreadyAbsent);
                };
                if img.id != req.id {
                    return Ok(RemoveResult::Refused(format!(
                        "image {} reports a different id; not touched",
                        req.name
                    )));
                }
                if !has_labels(&img.labels, &req.app, &req.deployment) {
                    return Ok(RemoveResult::Refused(format!(
                        "image {} does not carry this deployment's Harpoon labels; not touched",
                        req.name
                    )));
                }
                match self
                    .api
                    .remove_image(&req.id, budget.left("removing the image")?)
                    .map_err(|e| api_err(&format!("removing image {}", req.name), e))?
                {
                    true => Ok(RemoveResult::Removed),
                    false => Ok(RemoveResult::AlreadyAbsent),
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Backend

impl<T: HttpTransport> Backend for DockerBackend<T> {
    fn accepts_simulated_evidence(&self) -> bool {
        self.transport.is_simulated()
    }

    fn prepare_source(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        let b = Budget::new(io.budget_ms);
        let src = self.confined_source()?;
        let ctx = src.context.clone();
        let text = src.dockerfile_text.clone();
        io.log.push_line(&format!("source: {}", ctx.display()));

        let rules = src
            .ignore
            .as_ref()
            .map(|(_, t)| IgnoreRules::parse(t))
            .unwrap_or_default();
        for n in &rules.notes {
            io.log.push_line(&format!("dockerignore: {n}"));
        }
        let size = context_size_with(
            &ctx,
            self.cfg.max_context_bytes,
            &rules,
            &always_sent(&src.dockerfile_rel()),
        )
        .map_err(|e| {
            StepError::new(format!(
                "cannot measure the build context {}: {e}",
                ctx.display()
            ))
        })?;
        if size > self.cfg.max_context_bytes {
            return Err(StepError::new(format!(
                "the build context is larger than the {} MB limit (counted after applying .dockerignore where Harpoon can interpret it); nothing was built",
                self.cfg.max_context_bytes / 1_000_000
            )));
        }
        let free = self
            .disk
            .free_bytes(&self.cfg.scratch_dir)
            .map_err(|e| StepError::new(format!("disk-space preflight failed: {e}")))?;
        if free < self.cfg.min_free_bytes {
            return Err(StepError::new(format!(
                "only {} MB free on the build volume; at least {} MB are required. Harpoon never frees space by pruning.",
                free / 1_000_000,
                self.cfg.min_free_bytes / 1_000_000
            )));
        }
        let images = required_images(&text).map_err(|p| StepError::new(p.to_string()))?;

        self.api
            .ping(b.left("pinging Docker")?)
            .map_err(|e| api_err("Docker is not reachable on the configured socket", e))?;
        for img in &images {
            match self
                .api
                .image_inspect(img, b.left("checking base images")?)
                .map_err(|e| api_err(&format!("checking base image {img}"), e))?
            {
                Some(_) => io.log.push_line(&format!("base image {img}: present locally")),
                None => {
                    return Err(StepError::new(format!(
                        "base image {img} is not present locally; Harpoon does not pull images in this slice"
                    )))
                }
            }
        }
        std::fs::create_dir_all(&self.cfg.scratch_dir)
            .map_err(|e| StepError::new(format!("cannot create the scratch directory: {e}")))?;
        Ok(())
    }

    fn build(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        let b = Budget::new(io.budget_ms);
        let tag = image_tag(io.app(), io.deployment());
        let lbl = all_labels(io.app(), io.deployment());
        // Fail closed if the tag is taken. A build would move the tag to the new image and the old
        // image would silently lose it — even a Harpoon-labelled one; replacing an image needs a
        // separately approved recovery mechanism, so this slice never does it. Nothing is recorded
        // for a tag this deployment does not own.
        if let Some(existing) = self
            .api
            .image_inspect(&tag, b.left("checking the image tag is unused")?)
            .map_err(|e| api_err("checking the image tag is unused", e))?
        {
            let owner = if has_labels(&existing.labels, io.app(), io.deployment()) {
                "carrying this deployment's Harpoon labels"
            } else if existing.labels.iter().any(|(k, _)| k == labels::MANAGED) {
                "carrying Harpoon labels"
            } else {
                "that Harpoon did not create"
            };
            return Err(StepError::new(format!(
                "image tag {tag} already exists ({owner}); it was not overwritten and no build was run"
            )));
        }
        // Intent first: the build is what creates the image.
        let intent = io.intend(
            ResourceKey::new(ResourceKind::Image, &tag),
            Disposition::Retain,
        )?;
        self.st.image_tag = Some(tag.clone());
        // Confined again right before the build, and the canonical paths are what Buildx gets.
        let src = self.confined_source()?;
        let req = BuildRequest {
            binary: &self.buildx,
            context: &src.context,
            dockerfile: &src.dockerfile,
            tag: &tag,
            labels: &lbl,
            socket: &self.cfg.socket,
            scratch: &self.cfg.scratch_dir,
        };
        let log = &mut *io.log;
        match run_build(&req, b.left("starting the build")?, &mut |line| {
            log.push_line(line)
        }) {
            Ok(()) => {}
            Err(BuildRunError::Timeout) => {
                return Err(StepError::timeout(
                    "the build ran past its time budget and was killed",
                ))
            }
            Err(BuildRunError::Spawn(m)) => {
                return Err(StepError::new(format!(
                    "could not start the bundled Buildx: {m}"
                )))
            }
            Err(BuildRunError::Failed(c)) => {
                return Err(StepError::new(format!(
                    "the build failed (Buildx exit {})",
                    c.map(|c| c.to_string())
                        .unwrap_or_else(|| "by signal".into())
                )))
            }
        }
        let img = self
            .api
            .image_inspect(&tag, b.left("checking the built image")?)
            .map_err(|e| api_err("checking the built image", e))?
            .ok_or_else(|| {
                StepError::new(format!(
                    "the build reported success but image {tag} does not exist"
                ))
            })?;
        if !has_labels(&img.labels, io.app(), io.deployment()) {
            return Err(StepError::new(format!(
                "image {tag} exists but does not carry this deployment's Harpoon labels; it was not recorded as ours"
            )));
        }
        io.receipt(&intent, &img.id)?;
        io.log.push_line(&format!(
            "image {tag} = {}",
            short(img.id.trim_start_matches("sha256:"))
        ));
        Ok(())
    }

    fn preliminary_check(&mut self, io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError> {
        let b = Budget::new(io.budget_ms);
        let occ = self.occupancy(io, Purpose::Test, &b)?;
        let lease = ports::allocate(Purpose::Test, self.cfg.container_port, &occ, None, &[])
            .map_err(|e| StepError::new(e.to_string()))?;
        // The test port and the container are both step-scoped: the engine removes them before cutover.
        let lease_intent = io.intend(
            ResourceKey::new(ResourceKind::PortLease, lease.host_port.to_string()),
            Disposition::RemoveAlways,
        )?;
        io.receipt(&lease_intent, &lease.host_port.to_string())?;
        let name = preliminary_name(io.deployment());
        let id = self.create_container(io, &b, &name, Disposition::RemoveAlways, &lease)?;
        self.st.pre = Some((name, lease.host_port));
        self.api
            .start_container(&id, b.left("starting the preliminary container")?)
            .map_err(|e| api_err("starting the preliminary container", e))?;
        self.health(io, &b, &id, lease.host_port, "the preliminary check")
    }

    fn stop_current(&mut self, _io: &mut StepIo<'_>) -> Result<(), StepError> {
        Err(StepError::new(
            "replacing a running release is not implemented in this backend (B3 covers the first deployment only)",
        ))
    }

    fn start_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        let b = Budget::new(io.budget_ms);
        let occ = self.occupancy(io, Purpose::Application, &b)?;
        let mut lease = ports::allocate(
            Purpose::Application,
            self.cfg.container_port,
            &occ,
            None,
            &[],
        )
        .map_err(|e| StepError::new(e.to_string()))?;
        io.lease_port(lease)?;
        let mut name = serving_name(io.deployment());
        let mut id = self.create_container(io, &b, &name, Disposition::RemoveOnFailure, &lease)?;
        self.st.app = Some((name.clone(), lease.host_port));
        let started = self
            .api
            .start_container(&id, b.left("starting the application container")?);
        if let Err(e) = started {
            if !e.is_port_collision() {
                self.attach_logs(io, &b, &id);
                return Err(api_err("starting the application container", e));
            }
            // One replacement after a collision, then fail. Remove the unstarted container (with
            // the usual identity check), release the contested lease, take another port.
            let first = lease.host_port;
            self.discard_container(io, &name, &id)?;
            io.release_lease(first)?;
            let occ = self.occupancy(io, Purpose::Application, &b)?;
            lease = ports::reallocate_after_collision(&lease, self.st.replaced_from, &occ)
                .map_err(|e| StepError::new(e.to_string()))?;
            self.st.replaced_from = Some(first);
            io.lease_port(lease)?;
            name = replacement_name(io.deployment());
            id = self.create_container(io, &b, &name, Disposition::RemoveOnFailure, &lease)?;
            self.st.app = Some((name.clone(), lease.host_port));
            if let Err(e) = self
                .api
                .start_container(&id, b.left("starting the replacement container")?)
            {
                self.attach_logs(io, &b, &id);
                return Err(api_err(
                    &format!("starting the application container on replacement port {} (ports {first} and {} both failed)", lease.host_port, lease.host_port),
                    e,
                ));
            }
        }
        let info = self
            .api
            .container_inspect(&id, b.left("confirming the container started")?)
            .map_err(|e| api_err("confirming the container started", e))?;
        match info {
            Some(c) if c.running => Ok(()),
            _ => {
                self.attach_logs(io, &b, &id);
                Err(StepError::new(
                    "the application container is not running after start",
                ))
            }
        }
    }

    fn final_check(&mut self, io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError> {
        let b = Budget::new(io.budget_ms);
        let (name, port) = self
            .st
            .app
            .clone()
            .ok_or_else(|| StepError::new("no application container was started"))?;
        let id = Self::receipted_id(io, ResourceKind::Container, &name).ok_or_else(|| {
            StepError::new(format!("container {name} has no receipt in the ledger"))
        })?;
        self.health(io, &b, &id, port, "the final check")
    }

    fn stop_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        let b = Budget::new(io.budget_ms);
        let (name, _) = self
            .st
            .app
            .clone()
            .ok_or_else(|| StepError::new("no application container was started"))?;
        let id = Self::receipted_id(io, ResourceKind::Container, &name).ok_or_else(|| {
            StepError::new(format!("container {name} has no receipt in the ledger"))
        })?;
        let req = RemoveRequest {
            kind: ResourceKind::Container,
            id: id.clone(),
            name,
            app: io.app().clone(),
            deployment: io.deployment().clone(),
        };
        match self.verify_container(&req, b.left("re-checking the container")?)? {
            Verified::Absent => Ok(()),
            Verified::Refused(why) => Err(StepError::new(why)),
            Verified::Ok(_) => {
                self.api
                    .stop_container(&id, 10, b.left("stopping the container")?)
                    .map_err(|e| api_err("stopping the new release", e))?;
                Ok(())
            }
        }
    }

    fn restore_previous(&mut self, _budget_ms: u64) -> RestoreResult {
        RestoreResult::NotAttempted(Reason::NoPreviousRelease)
    }
}

impl<T: HttpTransport> DockerBackend<T> {
    /// Remove a container created in this step, with ledger attempt/result records around it.
    fn discard_container(
        &mut self,
        io: &mut StepIo<'_>,
        name: &str,
        id: &str,
    ) -> Result<(), StepError> {
        let key = ResourceKey::new(ResourceKind::Container, name);
        let req = RemoveRequest {
            kind: ResourceKind::Container,
            id: id.to_string(),
            name: name.to_string(),
            app: io.app().clone(),
            deployment: io.deployment().clone(),
        };
        io.cleanup_attempt(&key)?;
        let outcome = match self.remove_resource(&req) {
            Ok(RemoveResult::Removed) => CleanupOutcome::Removed,
            Ok(RemoveResult::AlreadyAbsent) => CleanupOutcome::AlreadyAbsent,
            Ok(RemoveResult::Refused(w)) => CleanupOutcome::Refused(w),
            Err(e) => CleanupOutcome::Failed(e.message),
        };
        let ok = matches!(
            outcome,
            CleanupOutcome::Removed | CleanupOutcome::AlreadyAbsent
        );
        let why = format!("{outcome:?}");
        io.cleanup_result(&key, outcome)?;
        if ok {
            Ok(())
        } else {
            Err(StepError::new(format!(
                "could not remove the collided container {name}: {why}"
            )))
        }
    }
}
