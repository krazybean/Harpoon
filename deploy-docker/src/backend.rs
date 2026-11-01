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
use crate::buildx_run::{run_build, BuildRequest, BuildRunError, SecretFiles, StdFileProbe};
use crate::confine::{confine, Source};
use crate::dockerfile::required_images;
use crate::forwarder::HostListenerCheck;
use crate::guest::{check_guest_free, GuestDiskProbe};
use crate::ignore::{always_sent, IgnoreRules};
use crate::preflight::{context_size_with, DfProbe, DiskProbe};
use crate::probe::{
    BindProbe, ContainerProbe, HttpTransport, PortProbe, ResponseMarker, SleepWaiter, TcpHttp,
};
use crate::snapshot;
use harpoon_deploy::backend::{Backend, PreviousRelease, ResourceOps, RestoreResult, StepError};
use harpoon_deploy::buildx::{self, BuildxBinary, BuildxError};
use harpoon_deploy::engine::StepIo;
use harpoon_deploy::health::{run_health, CheckEvidence, HealthSpec, Verdict};
use harpoon_deploy::ids::{labels, DeploymentId};
use harpoon_deploy::ledger::{
    CleanupOutcome, Disposition, Observed, RemoveRequest, RemoveResult, ResourceKey, ResourceKind,
};
use harpoon_deploy::ports::{self, Occupancy, PortLease, PortRange, Purpose};
use harpoon_deploy::release::ReleaseConfig;
use harpoon_deploy::secrets::{SecretBundle, SecretRef};
use harpoon_deploy::state::Reason;
use harpoon_deploy::time::{Entropy, OsEntropy, SystemClock};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How a health response is tied to the container that gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribution {
    /// No marker (only `DockerConfig::health_marker`, if set). Not acceptable for a live deployment.
    Off,
    /// Each container gets its own random marker in `HARPOON_DEPLOYMENT_MARKER`; a response counts
    /// only if it carries the same value in the `X-Harpoon-Deployment-Marker` header.
    PerContainer,
}

pub const MARKER_ENV: &str = "HARPOON_DEPLOYMENT_MARKER";
pub const MARKER_HEADER: &str = "X-Harpoon-Deployment-Marker";
/// Extra label on a container recreated by a restoration: the deployment whose release it restores.
pub const RESTORES_LABEL: &str = "io.harpoon.restores";

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
    /// Whether health responses must prove which container answered (see [`Attribution`]).
    pub attribution: Attribution,
    /// Live-execution requirements: when set, the backend refuses to start unless attribution is on,
    /// a host-listener check and a guest free-space probe are configured. The CLI sets it for every
    /// non-simulated deployment; unit tests of the library leave it off.
    pub live_gates: bool,
    /// When set, a health response only counts if it carries this marker (see [`ResponseMarker`]).
    pub health_marker: Option<ResponseMarker>,
    pub env: Vec<String>,
    /// `--target`: the Dockerfile stage to build.
    pub build_target: Option<String>,
    /// Non-secret build arguments (`--build-arg`), sorted by name.
    pub build_args: Vec<(String, String)>,
    /// Build secrets (BuildKit `--secret`): the id the Dockerfile asks for, and the exact version.
    /// References only; the values come from the backend's [`SecretBundle`].
    pub build_secrets: Vec<(String, SecretRef)>,
    /// Runtime secrets: environment name and exact version, injected into the container.
    pub runtime_secrets: Vec<(String, SecretRef)>,
    /// The configuration snapshot (a deployment id) and the canonical build description this
    /// release is pinned with. Identity only.
    pub snapshot_id: Option<String>,
    pub build_identity: Option<String>,
    pub limits: Limits,
    pub max_context_bytes: u64,
    pub min_free_bytes: u64,
    pub api_timeout: Duration,
    /// Time allowed for one removal outside a step (recovery, post-deployment cleanup).
    pub cleanup_timeout: Duration,
    pub log_tail_lines: u32,
    pub log_max_bytes: usize,
    /// Allow this backend to replace a running release (stop it, start the new one, restore the old
    /// one on failure). **Off, and never switched on by the CLI**: live replacement stays disabled
    /// until the evidence listed in `DEPLOY_B11_RECOVERY.md` exists. Tests turn it on against a
    /// simulated Docker only.
    pub enable_replacement: bool,
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
            attribution: Attribution::Off,
            live_gates: false,
            health_marker: None,
            env: Vec::new(),
            build_target: None,
            build_args: Vec::new(),
            build_secrets: Vec::new(),
            runtime_secrets: Vec::new(),
            snapshot_id: None,
            build_identity: None,
            limits: Limits::FIRST_TEST,
            max_context_bytes: 256 * 1000 * 1000,
            min_free_bytes: 2 * 1024 * 1024 * 1024,
            api_timeout: Duration::from_secs(30),
            cleanup_timeout: Duration::from_secs(60),
            log_tail_lines: 200,
            log_max_bytes: 256 * 1024,
            enable_replacement: false,
        }
    }
}

#[derive(Debug, Default)]
struct State {
    image_tag: Option<String>,
    /// The verified ID of the image this attempt built; containers are created from it, never from the tag.
    image_id: Option<String>,
    /// Base images the preflight checked, to detect a Dockerfile that changed afterwards.
    required_images: Option<BTreeSet<String>>,
    context_bytes: u64,
    /// Per-container response markers (container name → value).
    markers: std::collections::BTreeMap<String, String>,
    pre: Option<(String, u16)>, // container name, test port
    app: Option<(String, u16)>, // container name, application port
    replaced_from: Option<u16>,
    /// Set only while the one restoration attempt runs: creation and the health check use the
    /// previous release's pinned configuration instead of this attempt's.
    restoring: Option<PreviousRelease>,
    /// The application port reserved by `plan_release`, used by the next `start_new`.
    planned_lease: Option<PortLease>,
    /// A developer-initiated rollback: the earlier release's deployment and exact pinned
    /// configuration. The run builds nothing; it re-serves that retained image.
    rollback: Option<(DeploymentId, ReleaseConfig)>,
}

type CopyHook = Box<dyn FnMut(&Path)>;

pub struct DockerBackend<T: HttpTransport = TcpHttp> {
    cfg: DockerConfig,
    api: DockerApi,
    buildx: BuildxBinary,
    transport: T,
    ports: Box<dyn PortProbe>,
    disk: Box<dyn DiskProbe>,
    entropy: Box<dyn Entropy>,
    forwarder: Option<Box<dyn HostListenerCheck>>,
    guest_disk: Option<Box<dyn GuestDiskProbe>>,
    copy_hook: Option<CopyHook>,
    /// Resolved secret values for this operation, in memory only (never formatted, never stored).
    secrets: SecretBundle,
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
/// The tag of one build attempt. `nonce` is 128 random bits per attempt, so a name that did not
/// exist when the intent was written cannot be created by anyone else in the meantime.
pub fn image_tag(
    app: &harpoon_deploy::ids::ApplicationId,
    dep: &DeploymentId,
    nonce: &str,
) -> String {
    format!(
        "harpoon/{}:{}-{}",
        app.as_str().to_ascii_lowercase(),
        dep,
        nonce
    )
}

/// 128 random bits as 32 lower-case hex digits.
pub fn random_hex(e: &mut dyn Entropy) -> Result<String, StepError> {
    let mut b = [0u8; 16];
    e.fill(&mut b).map_err(|err| {
        StepError::new(format!(
            "no randomness available ({err}); nothing was started"
        ))
    })?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
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
            entropy: Box::new(OsEntropy::new()),
            forwarder: None,
            guest_disk: None,
            copy_hook: None,
            secrets: SecretBundle::new(),
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
    /// Randomness for image tags and response markers. Tests inject a deterministic source.
    pub fn with_entropy(mut self, e: impl Entropy + 'static) -> Self {
        self.entropy = Box::new(e);
        self
    }
    /// The resolved secret values this operation needs (build secrets, runtime secrets, and the
    /// pinned secrets of a release that may be restored or rolled back to).
    pub fn with_secrets(mut self, bundle: SecretBundle) -> Self {
        self.secrets = bundle;
        self
    }
    /// Checked after a healthy response: the host listener for the port must be the forwarder's.
    pub fn with_forwarder_check(mut self, c: impl HostListenerCheck + 'static) -> Self {
        self.forwarder = Some(Box::new(c));
        self
    }
    /// Called with each source path just before the snapshot opens it. For tests that need to
    /// change a file at the worst moment.
    #[doc(hidden)]
    /// Make this a rollback to the release built by `deployment`: the run builds nothing and
    /// serves that release's retained image (by its pinned id) with its pinned container port,
    /// health check and limits. Only the host port is chosen afresh, through the ledger.
    pub fn with_rollback(mut self, deployment: DeploymentId, pinned: ReleaseConfig) -> Self {
        self.cfg.container_port = pinned.container_port;
        self.cfg.health = pinned.health.clone();
        self.cfg.limits = Limits {
            memory_bytes: pinned.memory_bytes,
            nano_cpus: pinned.nano_cpus,
            pids: pinned.pids,
        };
        self.st.rollback = Some((deployment, pinned));
        self
    }

    pub fn with_snapshot_hook(mut self, f: impl FnMut(&Path) + 'static) -> Self {
        self.copy_hook = Some(Box::new(f));
        self
    }
    pub fn with_guest_disk(mut self, p: impl GuestDiskProbe + 'static) -> Self {
        self.guest_disk = Some(Box::new(p));
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
        // Normally the image this attempt built. While restoring, the previous release's pinned
        // image, which carries that release's own deployment label.
        let (image_id, tag, image_owner, limits) = match self
            .st
            .restoring
            .as_ref()
            .and_then(|p| p.config.as_ref().map(|c| (p, c)))
        {
            Some((p, c)) => (
                c.image_id.clone(),
                c.image_tag.clone(),
                p.deployment.clone(),
                Limits {
                    memory_bytes: c.memory_bytes,
                    nano_cpus: c.nano_cpus,
                    pids: c.pids,
                },
            ),
            None => {
                let image_id = self
                    .st
                    .image_id
                    .clone()
                    .ok_or_else(|| StepError::new("no verified image was built"))?;
                let tag = self
                    .st
                    .image_tag
                    .clone()
                    .unwrap_or_else(|| image_id.clone());
                let owner = self
                    .st
                    .rollback
                    .as_ref()
                    .map_or_else(|| io.deployment().clone(), |(d, _)| d.clone());
                (image_id, tag, owner, self.cfg.limits)
            }
        };
        let mut lbl = all_labels(io.app(), io.deployment());
        if let Some(p) = &self.st.restoring {
            lbl.push((RESTORES_LABEL.to_string(), p.deployment.to_string()));
        }
        // Docker creates an anonymous volume per declared image volume (inherited ones included)
        // even with no mounts in the request, and this ledger does not manage volumes. Refuse
        // before recording any container intent, so nothing is created. The image is addressed by
        // its verified ID, so a tag that moves cannot change which image is checked or run.
        let img = self
            .api
            .image_inspect(&image_id, b.left("checking the image's declared volumes")?)
            .map_err(|e| api_err("checking the image's declared volumes", e))?
            .ok_or_else(|| {
                StepError::new(format!(
                    "image {tag} ({}) no longer exists",
                    short(&image_id)
                ))
            })?;
        if img.id != image_id || !has_labels(&img.labels, io.app(), &image_owner) {
            return Err(StepError::new(format!(
                "image {tag} is no longer the image that was built and verified; no container was created"
            )));
        }
        if !img.volumes.is_empty() {
            return Err(StepError::new(format!(
                "image {tag} declares volumes ({}); Harpoon's first slice does not manage volumes, so no container was created. Remove the VOLUME instruction (including any inherited from the base image).",
                img.volumes.join(", ")
            )));
        }
        // Validated again here, whatever the source (this run, a pin read from the journal): a
        // reserved or malformed variable never reaches Docker.
        let pairs = harpoon_deploy::envvars::validate(&self.effective_env())
            .map_err(|e| StepError::new(format!("environment: {e}; no container was created")))?;
        let mut env = harpoon_deploy::envvars::docker_form(&pairs);
        for (name, r) in self.effective_secrets() {
            let v = self.secrets.get(&r).ok_or_else(|| {
                StepError::new(format!(
                    "secret {name} ({r}) is not available; no container was created"
                ))
            })?;
            env.push(format!("{name}={}", v.expose_str()));
        }
        if self.cfg.attribution == Attribution::PerContainer {
            let value = random_hex(&mut *self.entropy)?;
            env.push(format!("{MARKER_ENV}={value}"));
            self.st.markers.insert(name.to_string(), value);
        }
        let intent = io.intend(ResourceKey::new(ResourceKind::Container, name), disposition)?;
        let spec = CreateSpec {
            image: &image_id,
            labels: &lbl,
            env: &env,
            container_port: lease.container_port,
            host_port: lease.host_port,
            limits,
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

    /// The container must be publishing exactly its one container port on exactly this host port
    /// (and nothing else), on an unspecified address as Docker reports it for a port published
    /// without a `HostIp`. Anything else — another host port, an extra published port, a specific
    /// non-loopback address — is a refusal.
    fn verify_mapping(
        info: &ContainerInfo,
        container_port: u16,
        host_port: u16,
    ) -> Result<(), String> {
        let tcp: Vec<_> = info.published.iter().filter(|p| p.proto == "tcp").collect();
        if tcp.is_empty() {
            return Err(format!(
                "Docker reports no published port for container port {container_port}"
            ));
        }
        for p in &tcp {
            if p.container_port != container_port {
                return Err(format!(
                    "Docker reports container port {} published; only {container_port} may be",
                    p.container_port
                ));
            }
            if p.host_port != host_port {
                return Err(format!(
                    "Docker reports container port {container_port} published on host port {}, not the leased {host_port}",
                    p.host_port
                ));
            }
            if !matches!(p.host_ip.as_str(), "" | "0.0.0.0" | "::" | "127.0.0.1") {
                return Err(format!("Docker reports the port bound to {}", p.host_ip));
            }
        }
        Ok(())
    }

    fn live_gate_problems(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.cfg.attribution != Attribution::PerContainer {
            v.push("per-container health attribution is off");
        }
        if self.forwarder.is_none() {
            v.push("no host-listener (forwarder) check is configured");
        }
        if self.guest_disk.is_none() {
            v.push("no guest-side free-space probe is configured");
        }
        v
    }

    fn health(
        &mut self,
        io: &mut StepIo<'_>,
        b: &Budget,
        id: &str,
        name: &str,
        port: u16,
        what: &str,
    ) -> Result<CheckEvidence, StepError> {
        let base = match self.st.restoring.as_ref().and_then(|p| p.config.as_ref()) {
            Some(c) => c.health.clone(),
            None => self.cfg.health.clone(),
        };
        let container_port = self
            .st
            .restoring
            .as_ref()
            .and_then(|p| p.config.as_ref())
            .map_or(self.cfg.container_port, |c| c.container_port);
        let spec = HealthSpec {
            startup_deadline_ms: base.startup_deadline_ms.min(b.total.as_millis() as u64),
            ..base
        };
        let target = format!("http://127.0.0.1:{port}{}", spec.path);
        // Which answer counts: this container's own marker, or (library use only) a fixed one.
        let marker = match self.cfg.attribution {
            Attribution::PerContainer => {
                let value = self.st.markers.get(name).ok_or_else(|| {
                    StepError::new(format!(
                        "container {name} has no response marker; its health cannot be attributed"
                    ))
                })?;
                Some(
                    ResponseMarker::header(MARKER_HEADER, value)
                        .map_err(|e| StepError::new(format!("bad response marker: {e}")))?,
                )
            }
            Attribution::Off => self.cfg.health_marker.clone(),
        };
        if self.cfg.live_gates && marker.is_none() {
            return Err(StepError::new(
                "a live health check needs a response marker; none is configured",
            ));
        }
        // Mapping, before: a running container must publish exactly the leased port. (A container
        // that is not running is left to the probe, which reports the exit.)
        match self
            .api
            .container_inspect(id, b.left("reading the port mapping")?)
            .map_err(|e| api_err("reading the container's port mapping", e))?
        {
            Some(info) if info.running => Self::verify_mapping(&info, container_port, port)
                .map_err(|m| {
                    StepError::new(format!(
                        "{what}: the port mapping is not what was leased: {m}"
                    ))
                })?,
            _ => {}
        }
        let mut probe = ContainerProbe {
            api: &self.api,
            container_id: id,
            port,
            path: &spec.path,
            transport: &self.transport,
            marker: marker.as_ref(),
            attempt_timeout: Duration::from_secs(2).min(self.cfg.api_timeout),
        };
        let mut ev = run_health(&spec, &target, &mut probe, &SystemClock, &SleepWaiter);
        ev.simulated = self.transport.is_simulated();
        if ev.verdict == Verdict::Healthy && ev.validate().is_ok() {
            // Mapping, after, and the host listener: the response was this container's, and the
            // path it came by is the one that was verified.
            let post = self
                .api
                .container_inspect(id, b.left("re-reading the port mapping")?)
                .map_err(|e| api_err("re-reading the container's port mapping", e))?;
            let problem = match post {
                Some(info) if info.running => {
                    Self::verify_mapping(&info, container_port, port).err()
                }
                _ => Some("the container is not running after the health probe".to_string()),
            }
            .or_else(|| {
                self.forwarder
                    .as_ref()
                    .and_then(|f| f.verify(port).err().map(|e| format!("host listener: {e}")))
            });
            match problem {
                None => return Ok(ev),
                Some(m) => {
                    self.attach_logs(io, b, id);
                    return Err(StepError::new(format!(
                        "{what}: a healthy response was seen but could not be trusted: {m}"
                    ))
                    .with_evidence(ev));
                }
            }
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

impl<T: HttpTransport> DockerBackend<T> {
    /// A rollback's source step: prove the earlier release's retained image is there, is exactly
    /// the pinned id, and carries that release's own labels. Nothing is created or changed, and the
    /// image is never addressed by a tag.
    fn prepare_rollback_image(
        &mut self,
        io: &mut StepIo<'_>,
        b: &Budget,
        dep: &DeploymentId,
        c: &ReleaseConfig,
    ) -> Result<(), StepError> {
        if !c.image_id.starts_with("sha256:") {
            return Err(StepError::new(format!(
                "the pinned image {:?} is not an immutable image id; nothing was changed",
                c.image_id
            )));
        }
        self.api
            .ping(b.left("pinging Docker")?)
            .map_err(|e| api_err("Docker is not reachable on the configured socket", e))?;
        let img = self
            .api
            .image_inspect(&c.image_id, b.left("checking the retained image")?)
            .map_err(|e| api_err("checking the retained image", e))?
            .ok_or_else(|| {
                StepError::new(format!(
                    "the retained image {} of the target release no longer exists; nothing was changed",
                    short(&c.image_id)
                ))
            })?;
        if img.id != c.image_id || !has_labels(&img.labels, io.app(), dep) {
            return Err(StepError::new(format!(
                "image {} is not the image pinned for the target release, or does not carry its Harpoon labels; nothing was changed",
                short(&c.image_id)
            )));
        }
        io.log.push_line(&format!(
            "rollback: retained image {} verified for deployment {dep}",
            short(&c.image_id)
        ));
        self.st.image_id = Some(c.image_id.clone());
        self.st.image_tag = Some(c.image_tag.clone());
        Ok(())
    }
}

impl<T: HttpTransport> DockerBackend<T> {
    #[allow(clippy::too_many_arguments)]
    fn build_from_snapshot(
        &mut self,
        io: &mut StepIo<'_>,
        b: &Budget,
        tag: &str,
        nonce: &str,
        lbl: &[(String, String)],
        intent: &harpoon_deploy::engine::Intent,
        snap: &snapshot::Snapshot,
        src: &Source,
    ) -> Result<(), StepError> {
        for s in &snap.skipped {
            io.log.push_line(&format!(
                "context: skipped {s} (not a regular file, folder or link)"
            ));
        }
        io.log.push_line(&format!(
            "context snapshot: {} files, {} bytes, private copy",
            snap.files, snap.bytes
        ));
        // The Dockerfile Buildx will read must be the one that was checked.
        let text = std::fs::read_to_string(&snap.dockerfile)
            .map_err(|e| StepError::new(format!("cannot read the snapshot's Dockerfile: {e}")))?;
        let images = required_images(&text).map_err(|p| StepError::new(p.to_string()))?;
        if text != src.dockerfile_text
            || self
                .st
                .required_images
                .as_ref()
                .is_some_and(|prev| *prev != images)
        {
            return Err(StepError::new(
                "the Dockerfile changed after the preflight checked it; nothing was built",
            ));
        }
        let iidfile = self.cfg.scratch_dir.join(format!("iid-{nonce}"));
        if iidfile.exists() {
            return Err(StepError::new(format!(
                "{} already exists; nothing was built",
                iidfile.display()
            )));
        }
        // Build secrets: every one must be available before Buildx starts; they go to BuildKit as
        // files in a private directory beside (never inside) the context snapshot, and the files are
        // gone again whatever happens (`SecretFiles` removes them on drop).
        SecretFiles::sweep(&self.cfg.scratch_dir);
        let mut wanted: Vec<(String, &[u8])> = Vec::new();
        for (id, r) in &self.cfg.build_secrets {
            let v = self.secrets.get(r).ok_or_else(|| {
                StepError::new(format!(
                    "build secret {id} ({r}) is not available; nothing was built"
                ))
            })?;
            wanted.push((id.clone(), v.expose()));
        }
        let files = SecretFiles::create(&self.cfg.scratch_dir, nonce, &wanted).map_err(|e| {
            StepError::new(format!(
                "cannot prepare the build secrets: {e}; nothing was built"
            ))
        })?;
        if files.files.iter().any(|(_, p)| p.starts_with(&snap.dir)) {
            return Err(StepError::new(
                "a build secret would sit inside the build context; nothing was built",
            ));
        }
        let req = BuildRequest {
            binary: &self.buildx,
            context: &snap.dir,
            dockerfile: &snap.dockerfile,
            tag,
            labels: lbl,
            iidfile: &iidfile,
            socket: &self.cfg.socket,
            scratch: &self.cfg.scratch_dir,
            target: self.cfg.build_target.as_deref(),
            build_args: &self.cfg.build_args,
            secrets: &files.files,
        };
        let log = &mut *io.log;
        let outcome = run_build(&req, b.left("starting the build")?, &mut |text| {
            log.push_chunk(text)
        });
        drop(files);
        let iid_text = std::fs::read_to_string(&iidfile);
        let _ = std::fs::remove_file(&iidfile);
        match outcome {
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
        // The image's identity is what Buildx reported through --iidfile, in a file only this
        // attempt knew the name of. Everything below addresses the image by that ID.
        let iid = iid_text.map_err(|e| {
            StepError::new(format!(
                "the build reported success but wrote no image ID file: {e}"
            ))
        })?;
        let iid = iid.trim().to_string();
        let well_formed = iid.strip_prefix("sha256:").is_some_and(|h| {
            h.len() == 64 && h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
        });
        if !well_formed {
            return Err(StepError::new(format!(
                "the image ID file does not hold a sha256 ID ({iid:?})"
            )));
        }
        let img = self
            .api
            .image_inspect(&iid, b.left("checking the built image")?)
            .map_err(|e| api_err("checking the built image", e))?
            .ok_or_else(|| {
                StepError::new(format!(
                    "the build reported image {} but Docker has no such image",
                    short(&iid)
                ))
            })?;
        if img.id != iid {
            return Err(StepError::new(format!(
                "Docker answered for a different image than the reported {}",
                short(&iid)
            )));
        }
        if !has_labels(&img.labels, io.app(), io.deployment()) {
            return Err(StepError::new(format!(
                "image {} does not carry this deployment's Harpoon labels; it was not recorded as ours",
                short(&iid)
            )));
        }
        // It is ours and its ID is known: record it now, so it is tracked whatever happens next.
        io.receipt(intent, &iid)?;
        self.st.image_id = Some(iid.clone());
        // The tag must still lead to that same image; if it was moved, nothing runs from it.
        let tagged = self
            .api
            .image_inspect(tag, b.left("checking the tag still names the built image")?)
            .map_err(|e| api_err("checking the tag", e))?;
        match tagged {
            Some(t) if t.id == iid => {}
            Some(_) => {
                return Err(StepError::new(format!(
                    "tag {tag} now names a different image than the one built ({}); the built image was recorded and nothing will run",
                    short(&iid)
                )))
            }
            None => {
                return Err(StepError::new(format!(
                    "tag {tag} no longer exists; the built image {} was recorded and nothing will run",
                    short(&iid)
                )))
            }
        }
        io.log.push_line(&format!(
            "image {tag} = {}",
            short(iid.trim_start_matches("sha256:"))
        ));
        Ok(())
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
        if self.cfg.live_gates {
            let missing = self.live_gate_problems();
            if !missing.is_empty() {
                return Err(StepError::new(format!(
                    "live execution requirements are not met: {}. Nothing was built.",
                    missing.join("; ")
                )));
            }
        }
        if let Some((dep, c)) = self.st.rollback.clone() {
            return self.prepare_rollback_image(io, &b, &dep, &c);
        }
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
        if let Some(g) = &self.guest_disk {
            let required = self.cfg.min_free_bytes.saturating_add(size);
            let free = check_guest_free(&**g, required).map_err(StepError::new)?;
            io.log.push_line(&format!(
                "guest free space: {} MB (required {} MB)",
                free / 1_000_000,
                required / 1_000_000
            ));
        }
        let images = required_images(&text).map_err(|p| StepError::new(p.to_string()))?;
        self.st.required_images = Some(images.clone());
        self.st.context_bytes = size;

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
        if self.st.rollback.is_some() {
            io.log
                .push_line("rollback: nothing is built; the retained image is used as it is");
            return Ok(());
        }
        let b = Budget::new(io.budget_ms);
        let nonce = random_hex(&mut *self.entropy)?;
        let tag = image_tag(io.app(), io.deployment(), &nonce);
        let lbl = all_labels(io.app(), io.deployment());
        // Fail closed if the tag is taken. With 128 random bits this only happens through a bug or
        // a broken randomness source, but the check costs one read and keeps the promise that a
        // build never moves someone else's tag.
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
        // Confined again right before the build; then copied once into a private directory so
        // nothing the user changes afterwards can reach the builder.
        let src = self.confined_source()?;
        let rules = src
            .ignore
            .as_ref()
            .map(|(_, t)| IgnoreRules::parse(t))
            .unwrap_or_default();
        let mut hook = self.copy_hook.take();
        let snap = snapshot::create_with(
            &src,
            &rules,
            self.cfg.max_context_bytes,
            &self.cfg.scratch_dir,
            &nonce,
            &mut |p| {
                if let Some(h) = hook.as_mut() {
                    h(p)
                }
            },
        );
        self.copy_hook = hook;
        let snap = snap.map_err(|e| StepError::new(e.to_string()))?;
        let result = self.build_from_snapshot(io, &b, &tag, &nonce, &lbl, &intent, &snap, &src);
        let _ = snapshot::remove(&snap, &self.cfg.scratch_dir);
        result
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
        self.st.pre = Some((name.clone(), lease.host_port));
        self.api
            .start_container(&id, b.left("starting the preliminary container")?)
            .map_err(|e| api_err("starting the preliminary container", e))?;
        self.health(io, &b, &id, &name, lease.host_port, "the preliminary check")
    }

    /// Replacing a running release is off unless a test turns it on (`enable_replacement`); the
    /// CLI never does. The engine then stops after the preliminary check, before anything is
    /// stopped, and reports the candidate as prepared.
    fn supports_cutover(&self) -> bool {
        self.cfg.enable_replacement
    }

    fn plan_release(&mut self, io: &mut StepIo<'_>) -> Result<Option<ReleaseConfig>, StepError> {
        let b = Budget::new(io.budget_ms);
        let (Some(image_id), Some(image_tag)) =
            (self.st.image_id.clone(), self.st.image_tag.clone())
        else {
            return Err(StepError::new(
                "no verified image was built; there is nothing to plan",
            ));
        };
        harpoon_deploy::envvars::validate(&self.effective_env())
            .map_err(|e| StepError::new(format!("environment: {e}; nothing was planned")))?;
        self.check_secrets_available("nothing was planned")?;
        let occ = self.occupancy(io, Purpose::Application, &b)?;
        let lease = ports::allocate(
            Purpose::Application,
            self.cfg.container_port,
            &occ,
            None,
            &[],
        )
        .map_err(|e| StepError::new(e.to_string()))?;
        io.lease_port(lease)?;
        self.st.planned_lease = Some(lease);
        Ok(Some(ReleaseConfig {
            image_id,
            image_tag,
            container_port: self.cfg.container_port,
            host_port: lease.host_port,
            health: self.cfg.health.clone(),
            memory_bytes: self.cfg.limits.memory_bytes,
            nano_cpus: self.cfg.limits.nano_cpus,
            pids: self.cfg.limits.pids,
            env: self.effective_env(),
            secrets: self.effective_secrets(),
            snapshot: self.effective_snapshot(),
            build: self.effective_build(),
        }))
    }

    /// Read-only: running containers that already publish the previous release's port, with who
    /// owns them. The previous release's own recorded container is not a blocker. Nothing is
    /// stopped, created or removed to answer this.
    fn restore_blockers(
        &mut self,
        io: &mut StepIo<'_>,
        previous: &PreviousRelease,
    ) -> Result<Vec<String>, StepError> {
        let Some(cfg) = previous.config.as_ref() else {
            return Ok(Vec::new());
        };
        // The restoration would inject the previous release's own pinned secrets: if one cannot be
        // resolved now, the cutover must not start.
        let missing: Vec<String> = cfg
            .secrets
            .iter()
            .filter(|(_, r)| self.secrets.get(r).is_none())
            .map(|(n, r)| format!("secret {n} ({r}) of the previous release is not available, so it could not be restored"))
            .collect();
        if !missing.is_empty() {
            return Ok(missing);
        }
        let b = Budget::new(io.budget_ms);
        let own: Vec<String> = io
            .ledger()
            .entries()
            .iter()
            .filter(|e| {
                e.key.kind == ResourceKind::Container
                    && (e.deployment == previous.deployment
                        || previous
                            .container
                            .as_ref()
                            .is_some_and(|c| c.owner == e.deployment && c.name == e.key.name))
            })
            .filter_map(|e| e.id.clone())
            .collect();
        let list = self
            .api
            .list_containers(b.left("listing containers for the previous release's port")?)
            .map_err(|e| api_err("listing containers for the previous release's port", e))?;
        let mut out = Vec::new();
        for c in list {
            if own.contains(&c.id) {
                continue;
            }
            let publishes = if c.state == "running" {
                c.host_ports.contains(&cfg.host_port)
            } else {
                self.api
                    .configured_host_ports(
                        &c.id,
                        b.left("reading a container's port configuration")?,
                    )
                    .map_err(|e| api_err("reading a container's port configuration", e))?
                    .contains(&cfg.host_port)
            };
            if !publishes {
                continue;
            }
            let owner = if c
                .labels
                .iter()
                .any(|(k, v)| k == labels::APP && *v == io.app().to_string())
            {
                let dep = c
                    .labels
                    .iter()
                    .find(|(k, _)| k == labels::DEPLOYMENT)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("?");
                format!("it carries this application's Harpoon labels (deployment {dep}); if it is the failed candidate, `reconcile` removes it after verifying that ownership")
            } else if c.labels.iter().any(|(k, _)| k == labels::MANAGED) {
                "it carries another application's Harpoon labels and is not touched".to_string()
            } else {
                "it carries no Harpoon labels and is not touched; free the port yourself"
                    .to_string()
            };
            if c.state == "running" {
                out.push(format!(
                    "port {} is published by the running container {} ({}): {owner}",
                    cfg.host_port,
                    c.name,
                    short(&c.id)
                ));
            } else {
                io.log.push_line(&format!(
                    "port {} is configured on the stopped container {} ({}), which does not bind it: {owner}",
                    cfg.host_port,
                    c.name,
                    short(&c.id)
                ));
            }
        }
        Ok(out)
    }

    fn pinned_config(&self) -> Option<ReleaseConfig> {
        let (_, host_port) = self.st.app.clone()?;
        Some(ReleaseConfig {
            image_id: self.st.image_id.clone()?,
            image_tag: self.st.image_tag.clone()?,
            container_port: self.cfg.container_port,
            host_port,
            health: self.cfg.health.clone(),
            memory_bytes: self.cfg.limits.memory_bytes,
            nano_cpus: self.cfg.limits.nano_cpus,
            pids: self.cfg.limits.pids,
            env: self.effective_env(),
            secrets: self.effective_secrets(),
            snapshot: self.effective_snapshot(),
            build: self.effective_build(),
        })
    }

    /// Stop the previous release's container: found in the ledger by the previous deployment,
    /// re-read by id and re-checked against its name and all three labels before it is touched.
    /// Nothing is removed; the stopped container stays as the previous release's record.
    fn stop_current(
        &mut self,
        io: &mut StepIo<'_>,
        previous: &PreviousRelease,
    ) -> Result<(), StepError> {
        let b = Budget::new(io.budget_ms);
        let entry = io
            .ledger()
            .entries()
            .iter()
            .find(|e| {
                // The container that actually serves it: after a restoration that is the restored
                // one, recorded under the deployment that restored it.
                let (owner, name) = match &previous.container {
                    Some(c) => (&c.owner, Some(&c.name)),
                    None => (&previous.deployment, None),
                };
                e.deployment == *owner
                    && name.is_none_or(|n| e.key.name == *n)
                    && e.app == *io.app()
                    && e.key.kind == ResourceKind::Container
                    && !e.is_released()
                    && e.id.is_some()
            })
            .cloned()
            .ok_or_else(|| {
                StepError::new(
                    "the previous release has no recorded container; nothing was stopped",
                )
            })?;
        let req = RemoveRequest {
            kind: ResourceKind::Container,
            id: entry.id.clone().unwrap_or_default(),
            name: entry.key.name.clone(),
            app: entry.app.clone(),
            deployment: entry.deployment.clone(),
        };
        match self.verify_container(&req, b.left("re-checking the current release")?)? {
            Verified::Absent => Err(StepError::new(
                "the current release's container is gone; nothing was stopped",
            )),
            Verified::Refused(why) => Err(StepError::new(why)),
            Verified::Ok(info) if !info.running => Err(StepError::new(
                "the current release is not running; nothing was stopped",
            )),
            Verified::Ok(_) => {
                self.api
                    .stop_container(&req.id, 10, b.left("stopping the current release")?)
                    .map_err(|e| api_err("stopping the current release", e))?;
                Ok(())
            }
        }
    }

    fn start_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        let b = Budget::new(io.budget_ms);
        // The port was reserved (and the configuration pinned) before this step by `plan_release`.
        let mut lease = match self.st.planned_lease.take() {
            Some(l) => l,
            None => {
                let occ = self.occupancy(io, Purpose::Application, &b)?;
                let l = ports::allocate(
                    Purpose::Application,
                    self.cfg.container_port,
                    &occ,
                    None,
                    &[],
                )
                .map_err(|e| StepError::new(e.to_string()))?;
                io.lease_port(l)?;
                l
            }
        };
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
        self.health(io, &b, &id, &name, port, "the final check")
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

    /// One bounded attempt, never retried: recreate the previous release from its pinned image id
    /// and configuration, start it, and run its own health check, all inside `budget_ms`. The
    /// failed candidate and every unrelated object are left alone; the recreated container is new,
    /// labelled for this deployment plus `io.harpoon.restores`, and recorded through the ledger.
    fn restore_previous(
        &mut self,
        io: &mut StepIo<'_>,
        previous: &PreviousRelease,
        budget_ms: u64,
    ) -> Result<RestoreResult, StepError> {
        let Some(cfg) = previous.config.clone() else {
            return Ok(RestoreResult::NotAttempted(Reason::ConfigUnavailable));
        };
        let b = Budget::new(budget_ms);
        self.st.restoring = Some(previous.clone());
        let out = self.restore_inner(io, &b, previous, &cfg);
        self.st.restoring = None;
        out
    }
}

impl<T: HttpTransport> DockerBackend<T> {
    /// The non-secret environment the container being created gets (the marker is added separately,
    /// per container). A restoration uses the **restored release's own pinned environment**, a
    /// rollback the target's; only a new build uses this run's configuration. Never the CLI's
    /// arguments of some other run, and never another release's values.
    fn effective_env(&self) -> Vec<(String, String)> {
        if let Some(c) = self.st.restoring.as_ref().and_then(|p| p.config.as_ref()) {
            return c.env.clone();
        }
        if let Some((_, c)) = &self.st.rollback {
            return c.env.clone();
        }
        self.cfg_env_pairs()
    }

    /// The runtime secrets of the container being created: a restoration uses the restored release's
    /// own pinned versions, a rollback the target's, a new build this run's resolved ones.
    fn effective_secrets(&self) -> Vec<(String, SecretRef)> {
        if let Some(c) = self.st.restoring.as_ref().and_then(|p| p.config.as_ref()) {
            return c.secrets.clone();
        }
        if let Some((_, c)) = &self.st.rollback {
            return c.secrets.clone();
        }
        self.cfg.runtime_secrets.clone()
    }

    /// Where the release's configuration came from, and how it was built: a rollback keeps its
    /// target's, a new build records this run's.
    fn effective_snapshot(&self) -> Option<String> {
        match &self.st.rollback {
            Some((_, c)) => c.snapshot.clone(),
            None => self.cfg.snapshot_id.clone(),
        }
    }
    fn effective_build(&self) -> Option<String> {
        match &self.st.rollback {
            Some((_, c)) => c.build.clone(),
            None => self.cfg.build_identity.clone(),
        }
    }

    /// Defense in depth for the CLI's preflight: every secret this operation can inject must already
    /// be in memory, so a missing one stops it before anything is created or stopped.
    fn check_secrets_available(&self, consequence: &str) -> Result<(), StepError> {
        for (name, r) in self.effective_secrets() {
            if self.secrets.get(&r).is_none() {
                return Err(StepError::new(format!(
                    "secret {name} ({r}) is not available; {consequence}"
                )));
            }
        }
        for (id, r) in &self.cfg.build_secrets {
            if self.st.rollback.is_none() && self.secrets.get(r).is_none() {
                return Err(StepError::new(format!(
                    "build secret {id} ({r}) is not available; {consequence}"
                )));
            }
        }
        Ok(())
    }

    /// `cfg.env` as pairs. A malformed entry (the CLI validates, a library user might not) is kept
    /// as a name with no value so that the pin never silently drops it; creation refuses it later.
    fn cfg_env_pairs(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .cfg
            .env
            .iter()
            .map(|e| match e.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (e.clone(), String::new()),
            })
            .collect();
        v.sort();
        v
    }

    fn restore_inner(
        &mut self,
        io: &mut StepIo<'_>,
        b: &Budget,
        previous: &PreviousRelease,
        cfg: &ReleaseConfig,
    ) -> Result<RestoreResult, StepError> {
        // A journal failure aborts the run; anything else is a failed or impossible restoration.
        fn judged(
            io: &mut StepIo<'_>,
            e: StepError,
            fallback: Reason,
        ) -> Result<RestoreResult, StepError> {
            if e.fatal.is_none() {
                io.log.push_line(&format!("restoration: {}", e.message));
            }
            if e.fatal.is_some() {
                Err(e)
            } else if e.timed_out {
                Ok(RestoreResult::Failed(Reason::Timeout))
            } else {
                Ok(RestoreResult::Failed(fallback))
            }
        }
        let img = match self.api.image_inspect(
            &cfg.image_id,
            b.left("checking the previous release's image")?,
        ) {
            Ok(img) => img,
            Err(e) if e.is_timeout() => return Ok(RestoreResult::Failed(Reason::Timeout)),
            Err(ApiError::Transport(_)) => {
                return Ok(RestoreResult::NotAttempted(Reason::DockerUnreachable))
            }
            Err(e) => {
                io.log.push_line(&format!("restoration: {e}"));
                return Ok(RestoreResult::Failed(Reason::StartFailed));
            }
        };
        match img {
            Some(i)
                if i.id == cfg.image_id
                    && has_labels(&i.labels, io.app(), &previous.deployment) => {}
            Some(_) => {
                io.log.push_line("restoration: the pinned image id no longer carries the previous release's Harpoon labels");
                return Ok(RestoreResult::NotAttempted(Reason::ImageMissing));
            }
            None => {
                io.log
                    .push_line("restoration: the previous release's image no longer exists");
                return Ok(RestoreResult::NotAttempted(Reason::ImageMissing));
            }
        }
        let lease = PortLease {
            host_port: cfg.host_port,
            container_port: cfg.container_port,
            purpose: Purpose::Application,
        };
        // The previous release still holds its port lease in the ledger, which is what kept the port
        // reserved while it was stopped; the restored container serves on that same lease. A lease
        // held by anyone else is not ours to use; a free one is taken the usual way.
        let leased_by_previous: Vec<String> = io
            .ledger()
            .entries()
            .iter()
            .filter(|e| {
                e.key.kind == ResourceKind::PortLease
                    && e.deployment == previous.deployment
                    && !e.is_released()
            })
            .map(|e| e.key.name.clone())
            .collect();
        if !leased_by_previous.is_empty()
            && !leased_by_previous.contains(&cfg.host_port.to_string())
        {
            io.log.push_line(&format!(
                "restoration: the pinned port {} is not the port the previous release leased ({}); the pin cannot be trusted, so nothing was created",
                cfg.host_port,
                leased_by_previous.join(", ")
            ));
            return Ok(RestoreResult::NotAttempted(Reason::ConfigUnavailable));
        }
        let holder = io
            .ledger()
            .entries()
            .iter()
            .find(|e| {
                e.key.kind == ResourceKind::PortLease
                    && e.key.name == cfg.host_port.to_string()
                    && !e.is_released()
            })
            .map(|e| e.deployment.clone());
        match holder {
            Some(d) if d == previous.deployment => {}
            Some(d) => {
                io.log.push_line(&format!(
                    "restoration: port {} is leased by deployment {d}; it was not used",
                    cfg.host_port
                ));
                return Ok(RestoreResult::Failed(Reason::StartFailed));
            }
            None => {
                if let Err(e) = io.lease_port(lease) {
                    return judged(io, e, Reason::StartFailed);
                }
            }
        }
        let name = harpoon_deploy::serving::restored_container_name(io.deployment());
        let id = match self.create_container(io, b, &name, Disposition::Retain, &lease) {
            Ok(id) => id,
            Err(e) => return judged(io, e, Reason::StartFailed),
        };
        if let Err(e) = self
            .api
            .start_container(&id, b.left("starting the restored container")?)
        {
            self.attach_logs(io, b, &id);
            return Ok(RestoreResult::Failed(if e.is_timeout() {
                Reason::Timeout
            } else {
                Reason::StartFailed
            }));
        }
        match self.health(io, b, &id, &name, cfg.host_port, "the restoration check") {
            Ok(ev) => {
                io.log
                    .push_line(&format!("restoration check: {}", ev.summary()));
                Ok(RestoreResult::Healthy)
            }
            Err(e) if e.fatal.is_some() => Err(e),
            Err(e) => {
                io.log
                    .push_line(&format!("restoration check: {}", e.message));
                Ok(RestoreResult::StartedUnverified)
            }
        }
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
            // Containers are never removed through the retained-image path; a conflict here would be
            // a defect, and it is recorded as an ordinary (blocking) failure.
            Ok(RemoveResult::Conflict(c, w)) => {
                CleanupOutcome::Failed(format!("conflict ({}): {w}", c.as_str()))
            }
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

// ------------------------------------------------------------------ B8.2: retained-image cleanup

/// What is known about a retained image right now. Ownership and exclusivity are different
/// questions on purpose: an ownership failure is the terminal F1 class (the object is not
/// provably Harpoon's), while "not exclusively removable" is an ordinary, retryable conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageVerdict {
    /// Harpoon's image by id and labels, carrying no other tag or digest, used by no container.
    Eligible,
    /// Docker has no image with this id.
    Absent,
    /// The id or the ownership labels do not match: not provably Harpoon's. Never removed.
    Ownership(String),
    /// Ownership is shown, but removal would not be exclusive (another tag or digest, a container,
    /// or a container whose image Docker does not report). Nothing is removed.
    NotExclusive(String),
}

impl<T: HttpTransport> DockerBackend<T> {
    /// Read-only: inspect the image by its immutable id and list every container (running and
    /// stopped). An unreachable or malformed answer is an error, never "absent" and never
    /// "eligible".
    pub fn assess_retained_image(
        &self,
        id: &str,
        tag: &str,
        app: &harpoon_deploy::ids::ApplicationId,
        dep: &DeploymentId,
    ) -> Result<ImageVerdict, StepError> {
        let budget = Budget {
            start: Instant::now(),
            total: self.cfg.cleanup_timeout,
        };
        let Some(img) = self
            .api
            .image_inspect(id, budget.left("inspecting the retained image")?)
            .map_err(|e| api_err("inspecting the retained image", e))?
        else {
            return Ok(ImageVerdict::Absent);
        };
        if img.id != id {
            return Ok(ImageVerdict::Ownership(format!(
                "Docker answered for image {} when asked about {}; not touched",
                short(&img.id),
                short(id)
            )));
        }
        if !has_labels(&img.labels, app, dep) {
            return Ok(ImageVerdict::Ownership(format!(
                "image {} does not carry this deployment's Harpoon labels (managed, app, deployment); not touched",
                short(id)
            )));
        }
        let other_tags: Vec<&str> = img
            .tags
            .iter()
            .map(String::as_str)
            .filter(|t| *t != tag)
            .collect();
        if !other_tags.is_empty() {
            return Ok(ImageVerdict::NotExclusive(format!(
                "image {} is also tagged {}; Harpoon removes only an image it exclusively owns",
                short(id),
                other_tags.join(", ")
            )));
        }
        if !img.repo_digests.is_empty() {
            return Ok(ImageVerdict::NotExclusive(format!(
                "image {} is known by registry digest {}; it may be referenced elsewhere",
                short(id),
                img.repo_digests.join(", ")
            )));
        }
        let containers = self
            .api
            .list_containers(budget.left("listing containers")?)
            .map_err(|e| api_err("listing containers", e))?;
        let mut users = Vec::new();
        let mut unknown = Vec::new();
        for c in &containers {
            if c.image_id.is_empty() {
                unknown.push(c.name.clone());
            } else if c.image_id == id {
                users.push(format!("{} ({})", c.name, c.state));
            }
        }
        if !users.is_empty() {
            return Ok(ImageVerdict::NotExclusive(format!(
                "container {} was created from this image",
                users.join(", ")
            )));
        }
        if !unknown.is_empty() {
            return Ok(ImageVerdict::NotExclusive(format!(
                "Docker does not report which image container {} uses, so it cannot be shown not to use this one",
                unknown.join(", ")
            )));
        }
        Ok(ImageVerdict::Eligible)
    }

    /// The only image-removal path of `cleanup-image`: re-assesses immediately before deleting and
    /// then deletes **by id** with `force=false&noprune=true`, leaving the final say to Docker.
    pub fn retained_image_ops(&mut self) -> RetainedImageOps<'_, T> {
        RetainedImageOps { backend: self }
    }
}

/// [`ResourceOps`] for removing one retained image. Outcomes map onto the journal like this:
///
/// - removed / 404 → `Removed` / `AlreadyAbsent`;
/// - id or labels no longer match → `Refused` (terminal, F1, unchanged);
/// - not exclusive any more → `Conflict(NotExclusive)`, from Harpoon's own structured check;
/// - Docker's HTTP status 409 → `Conflict(DockerRefused)`, decided from the status code alone;
///   both are the typed `CleanupConflict` outcome (retryable, and non-blocking only under the
///   narrow rule of `Ledger::deferred_image_conflict`);
/// - any other error (timeout, disconnect, 5xx, malformed) → `Err`, an ordinary failed cleanup: the
///   outcome is ambiguous and stays blocking, whatever its text says.
pub struct RetainedImageOps<'a, T: HttpTransport> {
    backend: &'a mut DockerBackend<T>,
}

impl<T: HttpTransport> ResourceOps for RetainedImageOps<'_, T> {
    fn list_resources(&mut self) -> Result<Vec<Observed>, StepError> {
        self.backend.list_resources()
    }

    fn remove_resource(&mut self, req: &RemoveRequest) -> Result<RemoveResult, StepError> {
        if req.kind != ResourceKind::Image {
            return Ok(RemoveResult::Refused(
                "this operation removes a retained image only".into(),
            ));
        }
        match self
            .backend
            .assess_retained_image(&req.id, &req.name, &req.app, &req.deployment)?
        {
            ImageVerdict::Absent => return Ok(RemoveResult::AlreadyAbsent),
            ImageVerdict::Ownership(why) => return Ok(RemoveResult::Refused(why)),
            ImageVerdict::NotExclusive(why) => {
                return Ok(RemoveResult::Conflict(
                    harpoon_deploy::ledger::ConflictCause::NotExclusive,
                    format!("{why}; nothing was deleted"),
                ))
            }
            ImageVerdict::Eligible => {}
        }
        let left = self.backend.cfg.cleanup_timeout;
        match self.backend.api.remove_image(&req.id, left) {
            Ok(true) => Ok(RemoveResult::Removed),
            Ok(false) => Ok(RemoveResult::AlreadyAbsent),
            Err(e) if e.is_conflict() => Ok(RemoveResult::Conflict(
                harpoon_deploy::ledger::ConflictCause::DockerRefused,
                format!(
                    "Docker refused to delete image {} (HTTP 409: it is still referenced); nothing was deleted",
                    short(&req.id)
                ),
            )),
            Err(e) => Err(api_err(&format!("removing image {}", req.name), e)),
        }
    }
}
