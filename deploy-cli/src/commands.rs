use crate::args::{Parsed, AUTH_FLAG, AUTH_PHRASE};
use crate::store::{self, Kind, Runtime};
use crate::{exit, Fail, Gate};
use harpoon_deploy::backend::{FakeBackend, FakeRuntime, ResourceOps};
use harpoon_deploy::buildx;
use harpoon_deploy::engine::{
    end_state, execute_actions, reconcile, recover_cutover, run_deployment_confirmed,
    run_deployment_with, Context, CutoverPlan, NotCutOver, Report, RunConfig,
};
use harpoon_deploy::ids::{ApplicationId, IdGenerator};
use harpoon_deploy::ledger::{
    plan as ledger_plan, Action, Disposition, EndState, EntryState, Ledger, Observed, ResourceKind,
};
use harpoon_deploy::ports::{self, Occupancy, Purpose};
use harpoon_deploy::state::{
    trigger_pauses, CutoverState, Deployment, DeploymentStatus, RecoveryOutcome, Trigger,
};
use harpoon_deploy::time::{Clock, ManualClock, OsEntropy, SystemClock};
use harpoon_deploy_docker::api::DockerApi;
use harpoon_deploy_docker::backend::{Attribution, DockerBackend, DockerConfig};
use harpoon_deploy_docker::buildx_run::StdFileProbe;
use harpoon_deploy_docker::confine::{confine, Source};
use harpoon_deploy_docker::dockerfile::required_images;
use harpoon_deploy_docker::forwarder::{listeners_in_range, LsofCheck};
use harpoon_deploy_docker::guest::MgmtDfProbe;
use harpoon_deploy_docker::ignore::{always_sent, IgnoreRules};
use harpoon_deploy_docker::preflight::context_size_with;
use harpoon_deploy_journal::{writer_lock_status, LockStatus, Restored};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

type R = Result<i32, Fail>;

pub fn dispatch(p: &Parsed, gate: &Gate, out: &mut String) -> R {
    match p.command.as_str() {
        "status" => status(p, out),
        "doctor" => doctor(p, out),
        "plan" => plan(p, out),
        "recover" => recover(p, out),
        "reconcile" => reconcile_cmd(p, out),
        "deploy" => deploy(p, gate, out),
        "replace" => replace(p, out),
        "restore" => restore(p, out),
        "releases" => releases(p, out),
        "rollback" => rollback(p, out),
        "export-journal" => export_journal(p, out),
        "inventory" => inventory(p, out),
        "teardown" => teardown(p, out),
        "cleanup-image" => cleanup_image(p, out),
        other => Err(Fail::usage(format!("unknown command {other:?}"))),
    }
}

fn now_ms() -> u64 {
    SystemClock.now_ms()
}

// ------------------------------------------------------------------ shared pieces

fn socket_flag(p: &Parsed) -> Result<Option<PathBuf>, Fail> {
    match p.value("docker-socket") {
        None => Ok(None),
        Some("") => Err(Fail::usage("--docker-socket must not be empty")),
        Some(s) if !s.starts_with('/') => Err(Fail::usage(
            "--docker-socket must be an absolute path (unix:// and tcp:// are not accepted)",
        )),
        Some(s) => Ok(Some(PathBuf::from(s))),
    }
}

fn api(socket: &Path) -> DockerApi {
    DockerApi::new(socket, Duration::from_secs(10))
}

fn authorized(p: &Parsed) -> Result<(), Fail> {
    match p.value(AUTH_FLAG) {
        Some(v) if v == AUTH_PHRASE => Ok(()),
        Some(_) => Err(Fail::refused(format!(
            "--{AUTH_FLAG} must be given exactly as --{AUTH_FLAG}={AUTH_PHRASE}"
        ))),
        None => Err(Fail::refused(format!(
            "this changes Docker state and needs the explicit opt-in --{AUTH_FLAG}={AUTH_PHRASE}"
        ))),
    }
}

struct SourceCfg {
    cfg: DockerConfig,
    src: Source,
    images: BTreeSet<String>,
    size: u64,
    notes: Vec<String>,
}

fn deployment_config(health_deadline_ms: u64, prior_ledger: Ledger) -> RunConfig {
    let mut cfg = RunConfig {
        prior_ledger,
        ..RunConfig::default()
    };
    cfg.budgets.preliminary_ms = health_deadline_ms;
    cfg
}

fn source_cfg(p: &Parsed, rt: &Runtime, socket: &Path, need_port: bool) -> Result<SourceCfg, Fail> {
    let project = p
        .value("project-dir")
        .ok_or_else(|| Fail::config("--project-dir DIR is required"))?;
    let project = PathBuf::from(project);
    let context = match p.value("context") {
        None => project.clone(),
        Some(c) if Path::new(c).is_absolute() => PathBuf::from(c),
        Some(c) => project.join(c),
    };
    let port: u16 =
        match (p.value("container-port"), need_port) {
            (Some(v), _) => v.parse().ok().filter(|n| *n != 0).ok_or_else(|| {
                Fail::config(format!("--container-port {v:?} is not a port number"))
            })?,
            (None, true) => {
                return Err(Fail::config(
                    "--container-port N is required (the port the application listens on)",
                ))
            }
            (None, false) => 8080,
        };
    let mut cfg = DockerConfig::first_test(socket, &context, rt.scratch_dir(), port);
    cfg.project_root = Some(project.clone());
    if let Some(d) = p.value("dockerfile") {
        cfg.dockerfile = d.to_string();
    }
    let path = p.value("health-path").unwrap_or("/");
    if !path.starts_with('/') {
        return Err(Fail::config("--health-path must start with '/'"));
    }
    // The path becomes the request target: CR, LF or any other control character (or a space)
    // could split or alter the request.
    if let Some(c) = path.chars().find(|c| c.is_control() || *c == ' ') {
        return Err(Fail::config(format!(
            "--health-path contains a control or space character (U+{:04X}); use a plain absolute path such as /health",
            c as u32
        )));
    }
    cfg.health = harpoon_deploy::health::HealthSpec::first_test(path);
    if let Some(t) = p.value("health-timeout-secs") {
        let secs: u64 = t
            .parse()
            .ok()
            .filter(|n| (1..=600).contains(n))
            .ok_or_else(|| Fail::config("--health-timeout-secs must be 1..=600"))?;
        cfg.health.startup_deadline_ms = secs * 1000;
    }
    let src =
        confine(&project, &context, &cfg.dockerfile).map_err(|e| Fail::config(e.to_string()))?;
    let images = required_images(&src.dockerfile_text).map_err(|e| Fail::config(e.to_string()))?;
    let rules = src
        .ignore
        .as_ref()
        .map(|(_, t)| IgnoreRules::parse(t))
        .unwrap_or_default();
    let size = context_size_with(
        &src.context,
        cfg.max_context_bytes,
        &rules,
        &always_sent(&src.dockerfile_rel()),
    )
    .map_err(|e| {
        Fail::config(format!(
            "cannot measure the build context {}: {e}",
            src.context.display()
        ))
    })?;
    if size > cfg.max_context_bytes {
        return Err(Fail::config(format!(
            "the build context is larger than the {} MB limit (counted after .dockerignore where Harpoon can interpret it)",
            cfg.max_context_bytes / 1_000_000
        )));
    }
    let notes = rules.notes.clone();
    Ok(SourceCfg {
        cfg,
        src,
        images,
        size,
        notes,
    })
}

fn find_dep<'a>(
    deps: &'a [Deployment],
    id: &harpoon_deploy::ids::DeploymentId,
) -> Option<&'a Deployment> {
    deps.iter().find(|d| &d.id == id)
}

/// Everything in the ledger that still needs a decision. Entries a finished deployment deliberately
/// keeps (a retained image, the serving container of a successful deployment) are not unresolved.
fn unresolved(ledger: &Ledger, deps: &[Deployment]) -> Vec<String> {
    let mut v = Vec::new();
    for e in ledger.entries() {
        let what = format!(
            "{} {} (deployment {})",
            e.key.kind.as_str(),
            e.key.name,
            e.deployment
        );
        if e.needs_attention() {
            // The one narrow exception (see `Ledger::deferred_image_conflict`): a retained image of a
            // deployment that did not succeed, whose deletion was refused as a conflict, with
            // everything else of that deployment released. It is reported as a notice, not a blocker.
            let end = find_dep(deps, &e.deployment).and_then(|d| end_state(d.status));
            if ledger.deferred_image_conflict(e, end) {
                continue;
            }
            let why = match &e.state {
                EntryState::CleanupFailed(m) => format!("cleanup failed: {m}"),
                EntryState::CleanupConflict(c, m) => format!(
                    "cleanup refused as a conflict ({}): {m} (it blocks because this deployment has other unresolved work or did not end unsuccessfully)",
                    c.as_str()
                ),
                EntryState::Refused(m) => format!("not touched, ownership not proven: {m}"),
                _ => "a removal was attempted and its result was never recorded".to_string(),
            };
            v.push(format!("{what}: {why}"));
            continue;
        }
        if e.is_released() {
            continue;
        }
        if matches!(e.state, EntryState::Intended) {
            v.push(format!(
                "{what}: creation was intended but no receipt exists (it may or may not exist)"
            ));
            continue;
        }
        match find_dep(deps, &e.deployment).and_then(|d| end_state(d.status)) {
            None => v.push(format!("{what}: its deployment is not finished")),
            Some(end) => {
                let keep = e.disposition == Disposition::Retain
                    || (e.disposition == Disposition::RemoveOnFailure
                        && end == EndState::Succeeded);
                if !keep {
                    v.push(format!("{what}: should have been cleaned and was not"));
                }
            }
        }
    }
    v
}

/// Retained images of deployments that did not succeed. They never block anything by themselves;
/// this lists them so that `cleanup-image` can be found. (A retained image whose removal failed
/// for any reason other than a conflict is reported by `unresolved`, because that is ambiguous.)
fn retained_image_notices(ledger: &Ledger, deps: &[Deployment]) -> Vec<String> {
    let mut v = Vec::new();
    for e in ledger.entries() {
        if e.key.kind != ResourceKind::Image
            || e.disposition != Disposition::Retain
            || e.is_released()
            || e.cleanup_outstanding
        {
            continue;
        }
        let Some(id) = &e.id else { continue };
        let Some(d) = find_dep(deps, &e.deployment) else {
            continue;
        };
        if end_state(d.status) != Some(EndState::NotSucceeded) {
            continue;
        }
        let note = match &e.state {
            EntryState::Created => "retained".to_string(),
            EntryState::CleanupConflict(_, m)
                if ledger.deferred_image_conflict(e, Some(EndState::NotSucceeded)) =>
            {
                format!("cleanup refused as a conflict: {m}")
            }
            _ => continue,
        };
        v.push(format!(
            "image {} (id {id}) of deployment {} ({}): {note}",
            e.key.name, e.deployment, d.status
        ));
    }
    v
}

fn unfinished(deps: &[Deployment]) -> Vec<String> {
    deps.iter()
        .filter(|d| !d.status.is_terminal())
        .map(|d| {
            format!(
                "deployment {} is {} (run `recover` to mark it interrupted)",
                d.id, d.status
            )
        })
        .collect()
}

fn ended(deps: &[Deployment]) -> Vec<(harpoon_deploy::ids::DeploymentId, EndState)> {
    deps.iter()
        .filter_map(|d| end_state(d.status).map(|e| (d.id.clone(), e)))
        .collect()
}

fn print_lease_and_problems(out: &mut String, r: &Restored) {
    let leases = r.outstanding_leases();
    if leases.is_empty() {
        let _ = writeln!(out, "outstanding leases: none");
    } else {
        let _ = writeln!(out, "outstanding leases:");
        for e in r.ledger.entries().iter().filter(|e| {
            e.key.kind == harpoon_deploy::ledger::ResourceKind::PortLease && !e.is_released()
        }) {
            let _ = writeln!(
                out,
                "  port {} held by deployment {} ({:?})",
                e.key.name, e.deployment, e.state
            );
        }
    }
    let u = unresolved(&r.ledger, &r.deployments);
    if u.is_empty() {
        let _ = writeln!(out, "unresolved cleanup: none");
    } else {
        let _ = writeln!(out, "unresolved cleanup ({}):", u.len());
        for l in u {
            let _ = writeln!(out, "  {l}");
        }
    }
    let n = retained_image_notices(&r.ledger, &r.deployments);
    if !n.is_empty() {
        let _ = writeln!(
            out,
            "retained images of unsuccessful deployments (they do not block deployments; remove one with `cleanup-image`):"
        );
        for l in n {
            let _ = writeln!(out, "  {l}");
        }
    }
}

// ------------------------------------------------------------------ status

/// Missing deployment store: list the containers that carry Harpoon's ownership labels. Read-only
/// (one `GET /containers/json`); never adopts, records, stops, removes or modifies anything, and
/// never goes by container names. What it prints is Docker's current state, not deployment history.
fn discover_workloads(sock: &Path, out: &mut String) -> i32 {
    use harpoon_deploy::ids::{labels, DeploymentId};
    let _ = writeln!(
        out,
        "WARNING: deployment history is unavailable: the deployment store is missing. Nothing below is authoritative history, \
         and nothing was adopted, recorded or recreated."
    );
    let list = match api(sock).list_containers(Duration::from_secs(10)) {
        Ok(l) => l,
        Err(e) => {
            let _ = writeln!(
                out,
                "docker inspection FAILED ({}): {e}\n\
                 The Docker inventory is UNKNOWN, not empty; running Harpoon workloads may exist.",
                sock.display()
            );
            return exit::RUNTIME;
        }
    };
    let label = |c: &harpoon_deploy_docker::api::ContainerSummary, k: &str| {
        c.labels
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
    };
    let (mut found, mut foreign) = (Vec::new(), 0usize);
    for c in &list {
        if label(c, labels::MANAGED).as_deref() == Some("true") {
            found.push(c);
        } else {
            foreign += 1;
        }
    }
    let _ = writeln!(
        out,
        "docker discovery (read-only, {}): {} Harpoon-labelled container(s) (UNVERIFIED against any deployment record); \
         {foreign} other container(s) ignored (ownership is read from labels only, never from names)",
        sock.display(),
        found.len()
    );
    for c in found {
        let app = label(c, labels::APP).and_then(|a| a.parse::<ApplicationId>().ok().map(|_| a));
        let dep =
            label(c, labels::DEPLOYMENT).and_then(|d| d.parse::<DeploymentId>().ok().map(|_| d));
        let _ = writeln!(
            out,
            "  discovered container {} name={} state={}",
            c.id,
            c.name,
            if c.state.is_empty() {
                "unknown"
            } else {
                &c.state
            }
        );
        let _ = writeln!(
            out,
            "    application={} deployment={} image={}",
            app.as_deref()
                .unwrap_or("unknown (label missing or malformed)"),
            dep.as_deref()
                .unwrap_or("unknown (label missing or malformed)"),
            if c.image_id.is_empty() {
                "unknown"
            } else {
                &c.image_id
            }
        );
    }
    let _ = writeln!(
        out,
        "These resources keep running and were not touched. To manage them, an operator must decide; Harpoon does not adopt them."
    );
    exit::OK
}

fn status(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let _ = writeln!(out, "runtime: {}", rt.dir.display());
    // Corrupt or unusable state is an error here and never reaches the Docker fallback.
    let Some(r) = store::read_restored(&rt)? else {
        let _ = writeln!(
            out,
            "journal: not initialised (nothing has been deployed; nothing was created)"
        );
        return match socket_flag(p)? {
            Some(sock) => Ok(discover_workloads(&sock, out)),
            None => {
                let _ = writeln!(
                    out,
                    "note: if workloads were deployed and this store was lost, they may still be running; \
                     add --docker-socket PATH to list Harpoon-labelled containers (read-only)"
                );
                Ok(exit::OK)
            }
        };
    };
    if socket_flag(p)?.is_some() {
        let _ = writeln!(
            out,
            "note: deployment history is available, so Docker was not consulted"
        );
    }
    let kind = rt
        .kind()?
        .map(Kind::as_str)
        .unwrap_or("unmarked (treated as docker)");
    let lock = match writer_lock_status(&rt.journal_dir()) {
        Ok(LockStatus::Held) => "held by a writer",
        Ok(LockStatus::Free) => "free",
        Ok(LockStatus::NotInitialized) => "absent",
        Err(_) => "unknown",
    };
    let _ = writeln!(out, "kind: {kind}");
    let _ = writeln!(
        out,
        "journal: {} (lock: {lock})",
        if rt.journal_file().exists() {
            rt.journal_file()
        } else {
            rt.journal_dir().join("journal.wal")
        }
        .display()
    );
    if r.discarded_tail_bytes > 0 {
        let _ = writeln!(
            out,
            "note: {} trailing bytes are an incomplete write; they are ignored here and removed by the next writer",
            r.discarded_tail_bytes
        );
    }
    let mut deps = r.deployments.clone();
    deps.sort_by(|a, b| {
        b.created_ms
            .cmp(&a.created_ms)
            .then(b.id.as_str().cmp(a.id.as_str()))
    });
    let _ = writeln!(out, "deployments: {}", deps.len());
    if let Some(d) = deps.first() {
        let _ = writeln!(out, "latest: {} {}", d.id, d.status);
    }
    for d in &deps {
        let _ = writeln!(
            out,
            "  {} {} app={} created_ms={}",
            d.id, d.status, d.application_id, d.created_ms
        );
        if let Some(t) = &d.rollback_to {
            let _ = writeln!(
                out,
                "    rollback to release {t}{}",
                d.replaces
                    .as_ref()
                    .map(|r| format!(", replacing {r}"))
                    .unwrap_or_default()
            );
        }
        if let Some(f) = &d.failure {
            let _ = writeln!(
                out,
                "    failed at {}: {}",
                f.step.as_str(),
                f.message.lines().next().unwrap_or("")
            );
        }
        if let Some(rec) = &d.recovery {
            let took = rec
                .finished_ms
                .map(|f| format!(" in {} ms", f.saturating_sub(rec.started_ms)))
                .unwrap_or_default();
            let _ = writeln!(out, "    recovery: {}{took}", rec.outcome.as_str());
        }
        if let Some(ms) = d.interruption.and_then(|i| i.duration_ms()) {
            let _ = writeln!(out, "    interruption: {ms} ms between stopping the previous release and the confirmed new one");
        }
        if d.status == DeploymentStatus::Interrupted && d.application_may_be_down() {
            let _ = writeln!(
                out,
                "    cutover began and was never confirmed: the application may be down"
            );
            if d.recovery.is_none() {
                let _ = writeln!(
                    out,
                    "    explicit bounded recovery (one attempt): harpoon-deploy restore --deployment {} --simulate [--apply]",
                    d.id
                );
            }
        }
        for e in r.ledger.entries().iter().filter(|e| e.deployment == d.id) {
            let _ = writeln!(
                out,
                "    {} {} id={} state={} keep={}",
                e.key.kind.as_str(),
                e.key.name,
                e.id.as_deref().unwrap_or("-"),
                state_word(&e.state),
                disposition_word(e.disposition)
            );
        }
    }
    print_lease_and_problems(out, &r);
    let mut apps: Vec<&ApplicationId> = r.deployments.iter().map(|d| &d.application_id).collect();
    apps.sort();
    apps.dedup();
    for app in apps {
        if let Some(sv) = harpoon_deploy::serving::serving_release(&r.ledger, &r.deployments, app) {
            let _ = writeln!(
                out,
                "serving: application {app} runs release {} (deployment {}) in container {} of deployment {}{}",
                sv.release,
                sv.deployment,
                sv.container_name,
                sv.container_owner,
                match &sv.via {
                    harpoon_deploy::serving::ServedBy::Original => String::new(),
                    harpoon_deploy::serving::ServedBy::Restored { by } => format!(" (restored by {by})"),
                    harpoon_deploy::serving::ServedBy::RolledBack { by } => format!(" (rolled back by {by})"),
                }
            );
        }
    }
    for pz in trigger_pauses(&r.deployments) {
        if let Some(t) = &pz.rollback {
            let _ = writeln!(
                out,
                "automatic triggers paused for application {}: deployment {} rolled the application back to release {t}; automatic deployment does not resume by itself and there is no `resume` yet",
                pz.application, pz.deployment,
            );
            continue;
        }
        let _ = writeln!(
            out,
            "automatic triggers paused for application {}: deployment {} {} ({}); there is no `resume` yet",
            pz.application,
            pz.deployment,
            if pz.incomplete { "began a cutover that never finished" } else { "failed its cutover" },
            pz.recovery.map(|r| format!("recovery {}", r.as_str())).unwrap_or_else(|| "no recovery recorded".into()),
        );
    }
    let nf = unfinished(&r.deployments);
    for l in nf {
        let _ = writeln!(out, "attention: {l}");
    }
    Ok(exit::OK)
}

fn state_word(s: &EntryState) -> &'static str {
    match s {
        EntryState::Intended => "intended",
        EntryState::Created => "created",
        EntryState::Released(_) => "released",
        EntryState::CleanupFailed(_) => "cleanup-failed",
        EntryState::CleanupConflict(..) => "cleanup-conflict",
        EntryState::Refused(_) => "refused",
    }
}

fn disposition_word(d: Disposition) -> &'static str {
    match d {
        Disposition::Retain => "retain",
        Disposition::RemoveOnFailure => "remove-on-failure",
        Disposition::RemoveAlways => "remove-always",
    }
}

// ------------------------------------------------------------------ doctor

struct Checks {
    lines: Vec<String>,
    ok: usize,
    warn: usize,
    fail: usize,
    code: i32,
}

impl Checks {
    fn ok(&mut self, name: &str, d: impl AsRef<str>) {
        self.ok += 1;
        self.lines.push(format!("[ok]   {name}: {}", d.as_ref()));
    }
    fn warn(&mut self, name: &str, d: impl AsRef<str>) {
        self.warn += 1;
        self.lines.push(format!("[warn] {name}: {}", d.as_ref()));
    }
    fn fail(&mut self, code: i32, name: &str, d: impl AsRef<str>) {
        self.fail += 1;
        if self.code == exit::OK {
            self.code = code;
        }
        self.lines.push(format!("[FAIL] {name}: {}", d.as_ref()));
    }
}

fn doctor(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let socket = socket_flag(p)?;
    let mut c = Checks {
        lines: vec![],
        ok: 0,
        warn: 0,
        fail: 0,
        code: exit::OK,
    };

    let mut source: Option<SourceCfg> = None;
    if p.value("project-dir").is_some() {
        match source_cfg(
            p,
            &rt,
            socket.as_deref().unwrap_or(Path::new("/nonexistent")),
            false,
        ) {
            Ok(s) => {
                c.ok(
                    "source",
                    format!(
                        "{} (Dockerfile {}, {} bytes counted of {} allowed, base images: {})",
                        s.src.context.display(),
                        s.src.dockerfile_rel().display(),
                        s.size,
                        s.cfg.max_context_bytes,
                        if s.images.is_empty() {
                            "none".to_string()
                        } else {
                            s.images.iter().cloned().collect::<Vec<_>>().join(", ")
                        }
                    ),
                );
                for n in &s.notes {
                    c.warn("source", format!(".dockerignore: {n}"));
                }
                source = Some(s);
            }
            Err(f) => c.fail(f.code, "source", f.message),
        }
    } else {
        c.warn("source", "no --project-dir given; source not checked");
    }

    match p.value("resources-dir") {
        Some(d) => match buildx::resolve(Some(Path::new(d)), &StdFileProbe) {
            Ok(b) => c.ok(
                "bundled Buildx",
                format!("{} (the only Buildx Harpoon will run)", b.path.display()),
            ),
            Err(e) => c.fail(exit::CONFIG, "bundled Buildx", e.to_string()),
        },
        None => c.warn("bundled Buildx", "no --resources-dir given; not checked"),
    }

    match store::private_dir(&rt.dir, false).and_then(|exists| {
        if exists {
            store::private_dir(&rt.journal_dir(), false)
        } else {
            Ok(false)
        }
    }) {
        Err(f) => c.fail(f.code, "journal location", f.message),
        Ok(false) => c.ok(
            "journal location",
            format!(
                "{} does not exist yet; the first writing command creates it privately (0700)",
                rt.dir.display()
            ),
        ),
        Ok(true) => match store::read_restored(&rt) {
            Err(f) => c.fail(f.code, "journal", f.message),
            Ok(None) => c.ok("journal", "directory exists, no journal yet"),
            Ok(Some(r)) => {
                let lock = match writer_lock_status(&rt.journal_dir()) {
                    Ok(LockStatus::Held) => "a writer holds it right now",
                    Ok(_) => "no writer holds it",
                    Err(_) => "lock state unknown",
                };
                c.ok(
                    "journal",
                    format!(
                        "readable: {} deployments, {} ledger entries; {lock}",
                        r.deployments.len(),
                        r.ledger.entries().len()
                    ),
                );
                let u =
                    unresolved(&r.ledger, &r.deployments).len() + unfinished(&r.deployments).len();
                if u > 0 {
                    c.warn("journal", format!("{u} item(s) need recovery or reconciliation before a deploy is allowed (see `status`)"));
                }
            }
        },
    }

    match &socket {
        None => c.warn("runtime", "no --docker-socket given; Docker was not contacted (the environment and Docker contexts are never consulted)"),
        Some(s) => {
            let a = api(s);
            match a.ping(Duration::from_secs(5)) {
                Err(e) => c.fail(exit::RUNTIME, "runtime", format!("{} did not answer: {e}", s.display())),
                Ok(()) => {
                    c.ok("runtime", format!("{} answered a ping (read-only; nothing was changed)", s.display()));
                    if let Some(src) = &source {
                        for img in &src.images {
                            match a.image_inspect(img, Duration::from_secs(5)) {
                                Ok(Some(_)) => c.ok("base image", format!("{img} is present locally")),
                                Ok(None) => c.fail(exit::CONFIG, "base image", format!("{img} is not present locally and Harpoon never pulls")),
                                Err(e) => c.fail(exit::RUNTIME, "base image", format!("cannot inspect {img}: {e}")),
                            }
                        }
                    }
                }
            }
        }
    }

    if let (Some(sock), Some(m)) = (&socket, p.value("mgmt-socket")) {
        if !m.starts_with('/') {
            return Err(Fail::usage("--mgmt-socket must be an absolute path"));
        }
        let required = source
            .as_ref()
            .map(|s| s.cfg.min_free_bytes.saturating_add(s.size))
            .unwrap_or(2 * 1024 * 1024 * 1024);
        let probe = MgmtDfProbe::new(sock, m, Duration::from_secs(10));
        match probe.measure() {
            Err(e) => c.fail(exit::RUNTIME, "guest free space", e),
            Ok((free, what)) if free >= required => c.ok(
                "guest free space",
                format!(
                    "{} MB free on {what}; {} MB required (read-only: hostname, uname -r, df)",
                    free / 1_000_000,
                    required / 1_000_000
                ),
            ),
            Ok((free, what)) => c.fail(
                exit::RUNTIME,
                "guest free space",
                format!(
                    "only {} MB free on {what}; {} MB required",
                    free / 1_000_000,
                    required / 1_000_000
                ),
            ),
        }
    } else if p.value("mgmt-socket").is_some() {
        c.warn(
            "guest free space",
            "--mgmt-socket given without --docker-socket; not measured",
        );
    } else {
        c.warn(
            "guest free space",
            "no --mgmt-socket given; not measured (a live deployment requires it)",
        );
    }

    for l in &c.lines {
        let _ = writeln!(out, "{l}");
    }
    let _ = writeln!(
        out,
        "doctor: {} ok, {} warnings, {} failures",
        c.ok, c.warn, c.fail
    );
    Ok(c.code)
}

// ------------------------------------------------------------------ plan

fn plan(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let socket = socket_flag(p)?;
    let s = source_cfg(
        p,
        &rt,
        socket.as_deref().unwrap_or(Path::new("/nonexistent")),
        true,
    )?;
    let restored = store::read_restored(&rt)?;
    let (ledger, deps) = match &restored {
        Some(r) => (r.ledger.clone(), r.deployments.clone()),
        None => (Ledger::new(), vec![]),
    };

    let _ = writeln!(
        out,
        "plan — nothing is built, created or started by this command"
    );
    let _ = writeln!(
        out,
        "source: {} (Dockerfile {})",
        s.src.context.display(),
        s.src.dockerfile_rel().display()
    );
    let _ = writeln!(
        out,
        "context: {} bytes counted of {} allowed",
        s.size, s.cfg.max_context_bytes
    );
    for n in &s.notes {
        let _ = writeln!(out, "  .dockerignore: {n}");
    }

    let mut docker_ports = BTreeSet::new();
    let mut socket_note = "not contacted (no --docker-socket)".to_string();
    let mut missing: Vec<String> = Vec::new();
    if let Some(sock) = &socket {
        let a = api(sock);
        match a.ping(Duration::from_secs(5)) {
            Err(e) => {
                return Err(Fail::runtime(format!(
                    "{} did not answer: {e}",
                    sock.display()
                )))
            }
            Ok(()) => {
                docker_ports = a.published_ports(Duration::from_secs(10)).map_err(|e| {
                    Fail::runtime(format!("cannot read Docker's port mappings: {e}"))
                })?;
                for img in &s.images {
                    match a.image_inspect(img, Duration::from_secs(5)) {
                        Ok(Some(_)) => {}
                        Ok(None) => missing.push(img.clone()),
                        Err(e) => return Err(Fail::runtime(format!("cannot inspect {img}: {e}"))),
                    }
                }
                socket_note = format!("{} (read-only queries)", sock.display());
            }
        }
    }
    let _ = writeln!(
        out,
        "base images (local only, never pulled): {}{}",
        if s.images.is_empty() {
            "none".to_string()
        } else {
            s.images.iter().cloned().collect::<Vec<_>>().join(", ")
        },
        if socket.is_none() {
            " — presence not checked"
        } else {
            ""
        }
    );
    let _ = writeln!(out, "runtime: {socket_note}");

    let occ = |purpose| {
        let _ = purpose;
        Occupancy {
            docker_mappings: docker_ports.clone(),
            ..Occupancy::default()
        }
        .with_ledger(&ledger)
    };
    let test_port = ports::allocate(
        Purpose::Test,
        s.cfg.container_port,
        &occ(Purpose::Test),
        None,
        &[],
    );
    let app_port = ports::allocate(
        Purpose::Application,
        s.cfg.container_port,
        &occ(Purpose::Application),
        None,
        &[],
    );
    let app = p.value("app").unwrap_or("<new application id>");
    let _ = writeln!(
        out,
        "proposed operations, in order (first deployment only):"
    );
    let _ = writeln!(
        out,
        "  1 source        confine and measure the build context (done above)"
    );
    let _ = writeln!(out, "  2 build         bundled Buildx → image harpoon/{}:<deployment id>, labelled with the three ownership labels; fails if that tag exists", app.to_ascii_lowercase());
    let _ = writeln!(out, "  3 preliminary   test container on host port {} → container port {}; removed before cutover", port_word(&test_port), s.cfg.container_port);
    let _ = writeln!(
        out,
        "  4 start         serving container on host port {} → container port {}",
        port_word(&app_port),
        s.cfg.container_port
    );
    let _ = writeln!(
        out,
        "  5 final check   GET {} expecting 2xx, {} consecutive within {} s (no deployment-specific marker yet: B7 gate)",
        s.cfg.health.path,
        s.cfg.health.consecutive_ok,
        s.cfg.health.startup_deadline_ms / 1000
    );
    let _ = writeln!(
        out,
        "limits: 256 MiB memory, 0.5 CPU, 64 PIDs, no swap, no restart policy, no volumes"
    );
    let _ = writeln!(
        out,
        "host listeners on candidate ports are checked at deploy time"
    );
    let _ = writeln!(out, "live execution: disabled in this build until B7");

    let mut blockers: Vec<String> = Vec::new();
    for m in &missing {
        blockers.push(format!(
            "base image {m} is not present locally (Harpoon never pulls)"
        ));
    }
    if let Some(Kind::Simulated) = rt.kind()? {
        blockers.push(
            "this runtime directory is simulated; a real deployment needs its own".to_string(),
        );
    }
    blockers.extend(unfinished(&deps));
    blockers.extend(unresolved(&ledger, &deps));
    if test_port.is_err() || app_port.is_err() {
        blockers.push("no free port in a range".to_string());
    }
    if let Some(a) = p.value("app") {
        blockers.extend(existing_release_blocker(&ledger, a));
    }
    if blockers.is_empty() {
        let _ = writeln!(out, "blockers: none");
        Ok(exit::OK)
    } else {
        let _ = writeln!(out, "blockers — `deploy` would refuse:");
        for b in blockers {
            let _ = writeln!(out, "  {b}");
        }
        Ok(exit::REFUSED)
    }
}

fn port_word(r: &Result<ports::PortLease, ports::PortError>) -> String {
    match r {
        Ok(l) => l.host_port.to_string(),
        Err(e) => format!("(none: {e})"),
    }
}

fn existing_release_blocker(ledger: &Ledger, app: &str) -> Option<String> {
    ledger
        .entries()
        .iter()
        .find(|e| {
            e.app.as_str() == app && e.key.kind == harpoon_deploy::ledger::ResourceKind::Container && !e.is_released()
        })
        .map(|e| {
            format!(
                "container {} of application {app} is already recorded; replacing a release is not implemented in the first slice",
                e.key.name
            )
        })
}

// ------------------------------------------------------------------ recover

fn recover(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    if !rt.initialised() {
        let _ = writeln!(
            out,
            "nothing to recover: no journal exists at {}",
            rt.journal_dir().display()
        );
        return Ok(exit::OK);
    }
    let kind = rt.kind()?.unwrap_or(Kind::Docker);
    let (_j, r) = store::open_writer_recovered(&rt, kind, now_ms())?;
    if r.discarded_tail_bytes > 0 {
        let _ = writeln!(
            out,
            "removed {} bytes of an incomplete trailing write",
            r.discarded_tail_bytes
        );
    }
    if r.startup.interrupted.is_empty() {
        let _ = writeln!(out, "no deployment needed to be marked interrupted");
    } else {
        for id in &r.startup.interrupted {
            let _ = writeln!(out, "marked interrupted: {id}");
        }
    }
    for id in &r.startup.incomplete_cutovers {
        let _ = writeln!(
            out,
            "WARNING: {id} began a cutover that was never confirmed; the application may be down"
        );
    }
    print_lease_and_problems(out, &r);
    let _ = writeln!(out, "recover changed deployment status only. Docker was not contacted; nothing was removed, adopted, released or redeployed.");
    if !unresolved(&r.ledger, &r.deployments).is_empty() {
        let _ = writeln!(out, "next: harpoon-deploy reconcile --runtime-dir {} --docker-socket <path> --resources-dir <dir>", rt.dir.display());
    }
    Ok(exit::OK)
}

// ------------------------------------------------------------------ reconcile

fn describe(a: &Action) -> String {
    match a {
        Action::Remove {
            entry,
            observed,
            adopt,
        } => format!(
            "remove {} {} (id {}) of {}{}",
            observed.kind.as_str(),
            observed.name,
            observed.id,
            entry.0,
            if *adopt {
                ", recording its id first"
            } else {
                ""
            }
        ),
        Action::Adopt { entry, observed } => format!(
            "record the id {} of {} {} for {} and keep it",
            observed.id,
            observed.kind.as_str(),
            observed.name,
            entry.0
        ),
        Action::ReleaseLease { entry } => format!(
            "release port lease {} of {} (ledger only)",
            entry.1.name, entry.0
        ),
        Action::MarkAbsent { entry } => format!(
            "{} {} of {} is not in the runtime: record it as absent",
            entry.1.kind.as_str(),
            entry.1.name,
            entry.0
        ),
        Action::Refuse { entry, reason } => format!(
            "REFUSE {} {} of {}: {reason}",
            entry.1.kind.as_str(),
            entry.1.name,
            entry.0
        ),
        Action::ReportOnly { observed, reason } => format!(
            "report only: {} {} (id {}): {reason}",
            observed.kind.as_str(),
            observed.name,
            observed.id
        ),
    }
}

fn docker_backend(p: &Parsed, rt: &Runtime, socket: &Path) -> Result<DockerBackend, Fail> {
    let resources = p.value("resources-dir").ok_or_else(|| {
        Fail::config(
            "--resources-dir DIR is required (the bundled-resources directory holding Buildx)",
        )
    })?;
    let cfg = DockerConfig::first_test(socket, rt.dir.clone(), rt.scratch_dir(), 8080);
    DockerBackend::new(cfg, Some(Path::new(resources))).map_err(|e| Fail::config(e.to_string()))
}

fn reconcile_cmd(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let apply = p.switch("apply");
    if apply {
        authorized(p)?;
    }
    let socket = socket_flag(p)?
        .ok_or_else(|| Fail::runtime("reconcile needs an explicit --docker-socket PATH; nothing is assumed about the runtime"))?;
    if !rt.initialised() {
        let _ = writeln!(
            out,
            "nothing to reconcile: no journal exists at {}",
            rt.journal_dir().display()
        );
        return Ok(exit::OK);
    }
    if rt.kind()? == Some(Kind::Simulated) {
        return Err(Fail::refused(
            "this is a simulated runtime; there is nothing real to reconcile",
        ));
    }
    let mut backend = docker_backend(p, &rt, &socket)?;

    // Dry run reads the journal without locking or writing; apply takes the writer lock first.
    let (mut writer, restored) = if apply {
        let (j, r) = store::open_writer(&rt, Kind::Docker)?;
        (Some(j), r)
    } else {
        let r = store::read_restored(&rt)?.expect("initialised above");
        (None, r)
    };
    let deps = &restored.deployments;
    let nf = unfinished(deps);
    if apply && !nf.is_empty() {
        return Err(Fail::refused(format!(
            "{}\nreconcile --apply only acts on finished deployments",
            nf.join("\n")
        )));
    }
    for l in &nf {
        let _ = writeln!(out, "note: {l}");
    }
    let observed = match backend.list_resources() {
        Ok(o) => o,
        Err(e) => {
            return Err(Fail::runtime(format!(
            "cannot list the runtime ({}): {}\nNothing was assumed absent and nothing was changed.",
            socket.display(),
            e.message
        )))
        }
    };
    let ended_list = ended(deps);
    let actions = ledger_plan(&restored.ledger, &observed, &ended_list);
    if actions.is_empty() {
        let _ = writeln!(out, "reconcile: nothing to do");
    }
    for a in &actions {
        let _ = writeln!(out, "{}{}", if apply { "" } else { "would: " }, describe(a));
    }
    if !apply {
        let _ = writeln!(out, "dry run: nothing was changed. Add --apply and --{AUTH_FLAG}=… to carry these out; every removal is re-verified (id, name and all three ownership labels) at the moment it happens.");
        return Ok(exit::OK);
    }
    let mut ledger = restored.ledger.clone();
    let j = writer.as_mut().expect("apply holds the writer");
    let unaccounted =
        reconcile(&mut ledger, &ended_list, j, &mut backend, &SystemClock).map_err(|e| {
            Fail::failed(format!(
                "journal write failed during reconcile: {e}; run `recover` and retry"
            ))
        })?;
    for o in &unaccounted {
        let _ = writeln!(
            out,
            "reported, not touched: {} {} (id {})",
            o.kind.as_str(),
            o.name,
            o.id
        );
    }
    let left = unresolved(&ledger, deps);
    if left.is_empty() {
        let _ = writeln!(out, "reconcile: complete; nothing unresolved");
        Ok(exit::OK)
    } else {
        let _ = writeln!(out, "reconcile: {} item(s) still unresolved:", left.len());
        for l in left {
            let _ = writeln!(out, "  {l}");
        }
        Ok(exit::FAILED)
    }
}

// ------------------------------------------------------------------ deploy

fn deploy(p: &Parsed, gate: &Gate, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let simulate = p.switch("simulate");
    let socket = socket_flag(p)?;

    // ---- gates, before anything is read or written
    if !simulate {
        if !gate.live_deploy_enabled {
            return Err(Fail::refused(
                "live deployment is disabled in this build (Milestone B7 gate). Nothing was read, written or contacted.\n\
                 Use `plan` to validate a deployment, or `deploy --simulate` against a separate runtime directory for an offline run.",
            ));
        }
        authorized(p)?;
        if socket.is_none() {
            return Err(Fail::runtime(
                "deploy needs an explicit --docker-socket PATH",
            ));
        }
        forwarder_flags(p)?;
        mgmt_flag(p)?;
    }
    let sock = socket
        .clone()
        .unwrap_or_else(|| PathBuf::from("/nonexistent"));
    let s = source_cfg(p, &rt, &sock, true)?;

    let kind = if simulate {
        Kind::Simulated
    } else {
        Kind::Docker
    };
    let (mut journal, restored) = store::open_writer(&rt, kind)?;
    let deps = restored.deployments.clone();

    let mut blockers = unfinished(&deps);
    blockers.extend(unresolved(&restored.ledger, &deps));
    let mut gen = IdGenerator::new(SystemClock, OsEntropy::new());
    let app: ApplicationId = match p.value("app") {
        Some(a) => a
            .parse()
            .map_err(|e| Fail::usage(format!("--app {a:?} is not an application id: {e}")))?,
        None => gen
            .application()
            .map_err(|e| Fail::failed(format!("cannot generate an id: {e}")))?,
    };
    blockers.extend(existing_release_blocker(&restored.ledger, app.as_str()));
    if !blockers.is_empty() {
        let mut m = String::from(
            "deploy refused: unresolved ownership or journal state makes execution unsafe:\n",
        );
        for b in &blockers {
            let _ = writeln!(m, "  {b}");
        }
        m.push_str("Nothing was built or started. Use `status`, `recover` and `reconcile` to resolve this.");
        return Err(Fail::refused(m));
    }

    let dep_id = gen
        .deployment()
        .map_err(|e| Fail::failed(format!("cannot generate an id: {e}")))?;
    let mut d = Deployment::new(dep_id, app, Trigger::Cli, now_ms());
    d.transition(DeploymentStatus::Queued, now_ms())
        .map_err(|e| Fail::failed(e.to_string()))?;
    let _ = writeln!(
        out,
        "deployment {} (application {}) — {}",
        d.id,
        d.application_id,
        if simulate {
            "SIMULATED, no Docker"
        } else {
            "Docker"
        }
    );
    if !simulate {
        let _ = writeln!(
            out,
            "image tag: harpoon/{}:{}-<128 random bits, chosen at build>",
            d.application_id.as_str().to_ascii_lowercase(),
            d.id
        );
    }
    // Keep the engine's preliminary-step budget aligned with the user-visible health deadline.
    let cfg = deployment_config(s.cfg.health.startup_deadline_ms, restored.ledger.clone());
    let ctx = Context {
        previous_release: None,
    };

    let report = if simulate {
        let clock = ManualClock::new(now_ms());
        let mut b = FakeBackend::new(clock.clone()).with_runtime(FakeRuntime::world());
        run_deployment_with(&mut d, &ctx, &mut b, &mut journal, &clock, &cfg, &|| false)
    } else {
        let resources = p
            .value("resources-dir")
            .ok_or_else(|| Fail::config("--resources-dir DIR is required"))?;
        store::private_dir(&rt.scratch_dir(), true)?;
        // Every live requirement is switched on here; the backend refuses to start without them.
        let mut live_cfg = s.cfg.clone();
        live_cfg.live_gates = true;
        live_cfg.attribution = Attribution::PerContainer;
        let (lsof, pid, command) = forwarder_flags(p)?;
        let mgmt = mgmt_flag(p)?;
        let mut b = DockerBackend::new(live_cfg, Some(Path::new(resources)))
            .map_err(|e| Fail::config(e.to_string()))?
            .with_forwarder_check(LsofCheck { lsof, pid, command })
            .with_guest_disk(MgmtDfProbe::new(&sock, mgmt, Duration::from_secs(10)));
        run_deployment_with(
            &mut d,
            &ctx,
            &mut b,
            &mut journal,
            &SystemClock,
            &cfg,
            &|| false,
        )
    };
    let report = report.map_err(|e| {
        Fail::failed(format!(
            "the run stopped on a journal or engine error: {e}\nRun `recover`, then `status`."
        ))
    })?;
    print_report(out, &d, &report);
    Ok(
        if report.status == DeploymentStatus::Succeeded && report.is_clean() {
            exit::OK
        } else {
            exit::FAILED
        },
    )
}

// ------------------------------------------------------------------ replace

/// The release currently serving `app` and the container that really serves it (after a
/// restoration that is the restored container, recorded under the deployment that restored it).
fn current_release(
    ledger: &Ledger,
    deps: &[Deployment],
    app: &ApplicationId,
) -> Option<harpoon_deploy::serving::Serving> {
    harpoon_deploy::serving::serving_release(ledger, deps, app)
}

/// Health-gated replacement of a running release, offline only. The candidate is built and checked
/// while the current release keeps running; the cutover plan and its anticipated interruption are
/// printed before anything is stopped; the cutover itself needs `--confirm-cutover`.
///
/// Live replacement is deliberately not available: the bounded restoration that must follow a
/// failed cutover is the next milestone, so a live run is refused before anything is read.
fn replace(p: &Parsed, out: &mut String) -> R {
    if !p.switch("simulate") {
        return Err(Fail::refused(
            "live replacement is not enabled in this build: the bounded cutover recovery that must follow a failed cutover is not implemented yet. \
             Nothing was read, written or contacted. Use `replace --simulate` against a separate runtime directory for an offline run.",
        ));
    }
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let app: ApplicationId = p
        .value("app")
        .ok_or_else(|| Fail::usage("replace needs --app APPLICATION_ID"))?
        .parse()
        .map_err(|e| Fail::usage(format!("--app is not an application id: {e}")))?;
    let sock = PathBuf::from("/nonexistent");
    let s = source_cfg(p, &rt, &sock, true)?;
    // Look before opening the writer: a refusal must not create a journal.
    let Some(existing) = store::read_restored(&rt)? else {
        return Err(replace_refused(&[format!(
            "application {app} has no running release recorded; use `deploy` for a first deployment"
        )]));
    };
    if current_release(&existing.ledger, &existing.deployments, &app).is_none() {
        return Err(replace_refused(&[format!(
            "application {app} has no running release recorded; use `deploy` for a first deployment"
        )]));
    }
    let (mut journal, restored) = store::open_writer(&rt, Kind::Simulated)?;
    let deps = restored.deployments.clone();

    let mut blockers = unfinished(&deps);
    blockers.extend(unresolved(&restored.ledger, &deps));
    blockers.extend(
        deps.iter()
            .filter(|d| d.application_may_be_down())
            .map(|d| {
                format!(
                    "deployment {} began a cutover that was never confirmed: the application may be down. Replacing is blocked until the bounded cutover recovery exists",
                    d.id
                )
            }),
    );
    let Some(serving) = current_release(&restored.ledger, &deps, &app) else {
        blockers.push(format!("application {app} has no running release recorded"));
        return Err(replace_refused(&blockers));
    };
    let (cur_dep, cur_release) = (serving.deployment.clone(), serving.release.clone());
    if !blockers.is_empty() {
        return Err(replace_refused(&blockers));
    }

    let mut gen = IdGenerator::new(SystemClock, OsEntropy::new());
    let dep_id = gen
        .deployment()
        .map_err(|e| Fail::failed(format!("cannot generate an id: {e}")))?;
    let mut d = Deployment::new(dep_id, app, Trigger::Cli, now_ms());
    d.transition(DeploymentStatus::Queued, now_ms())
        .map_err(|e| Fail::failed(e.to_string()))?;
    let mut cfg = deployment_config(s.cfg.health.startup_deadline_ms, restored.ledger.clone());
    cfg.previous = serving.config.clone();
    cfg.previous_container = Some(harpoon_deploy::backend::ContainerRef {
        owner: serving.container_owner.clone(),
        name: serving.container_name.clone(),
    });
    let plan = CutoverPlan::new(&d.application_id, &cur_release, &d.id, &cfg);
    let _ = writeln!(
        out,
        "deployment {} (application {}) — SIMULATED, no Docker; replaces the release of deployment {cur_dep}, served by container {} of deployment {}{}",
        d.id,
        d.application_id,
        serving.container_name,
        serving.container_owner,
        match &serving.via {
            harpoon_deploy::serving::ServedBy::Original => String::new(),
            harpoon_deploy::serving::ServedBy::Restored { by } => format!(" (restored by {by})"),
            harpoon_deploy::serving::ServedBy::RolledBack { by } => format!(" (rolled back by {by})"),
        }
    );
    for l in plan.lines() {
        let _ = writeln!(out, "{l}");
    }
    let confirmed = p.switch("confirm-cutover");
    if !confirmed {
        let _ = writeln!(
            out,
            "cutover not confirmed: the candidate will be built and checked, then the run stops with the current release untouched"
        );
    }

    let ctx = Context {
        previous_release: Some(cur_release),
    };
    let clock = ManualClock::new(now_ms());
    let mut b = FakeBackend::new(clock.clone()).with_runtime(FakeRuntime::world());
    let report = run_deployment_confirmed(
        &mut d,
        &ctx,
        &mut b,
        &mut journal,
        &clock,
        &cfg,
        &|| false,
        &|_: &CutoverPlan| confirmed,
    )
    .map_err(|e| {
        Fail::failed(format!(
            "the run stopped on a journal or engine error: {e}\nRun `recover`, then `status`."
        ))
    })?;
    print_report(out, &d, &report);
    if let Some(i) = &d.interruption {
        let _ = match i.duration_ms() {
            Some(ms) => writeln!(
                out,
                "measured interruption: {ms} ms (simulated clock; a real cutover has not been measured)"
            ),
            None => writeln!(
                out,
                "interruption still open: the application may be down; run `recover`"
            ),
        };
    }
    if let Some(pr) = &report.prepared {
        let _ = match pr.why {
            NotCutOver::NotConfirmed => writeln!(
                out,
                "candidate prepared, current release untouched. Re-run with --confirm-cutover to perform the (simulated) cutover; the candidate image is kept (see `cleanup-image`)."
            ),
            NotCutOver::Unsupported => writeln!(
                out,
                "candidate prepared, current release untouched: this backend cannot replace a running release."
            ),
        };
        return Ok(exit::OK);
    }
    Ok(
        if report.status == DeploymentStatus::Succeeded && report.is_clean() {
            exit::OK
        } else {
            exit::FAILED
        },
    )
}

fn replace_refused(blockers: &[String]) -> Fail {
    let mut m = String::from("replace refused:\n");
    for b in blockers {
        let _ = writeln!(m, "  {b}");
    }
    m.push_str(
        "Nothing was built or stopped. Use `status`, `recover` and `reconcile` to resolve this.",
    );
    Fail::refused(m)
}

/// What the one bounded restoration did, in words that say what happened and promise nothing.
fn print_recovery(out: &mut String, d: &Deployment) {
    let Some(r) = &d.recovery else { return };
    let took = r
        .finished_ms
        .map(|f| {
            format!(
                " in {} ms (budget {} ms)",
                f.saturating_sub(r.started_ms),
                r.budget_ms
            )
        })
        .unwrap_or_default();
    let line = match r.outcome {
        RecoveryOutcome::InProgress => "restoration started and has no recorded result".to_string(),
        RecoveryOutcome::Restored => "restored: the previous release started and passed its own health check".to_string(),
        RecoveryOutcome::RestoredUnverified => {
            "restored_unverified: the previous release started, but its health check did not pass or was inconclusive".to_string()
        }
        RecoveryOutcome::RestoreFailed(why) => format!("restore_failed: {}", why.as_str_for_humans()),
        RecoveryOutcome::NotAttempted(why) => format!("not_attempted: {}", why.as_str_for_humans()),
    };
    let _ = writeln!(out, "recovery: {line}{took}");
    if d.application_may_be_down() {
        let _ = writeln!(
            out,
            "the application may be down; the failed candidate was kept for inspection"
        );
    }
}

// ------------------------------------------------------------------ releases and rollback

/// Why an application cannot be changed right now, whatever the change: an unfinished deployment,
/// an unresolved resource, or a cutover that was never confirmed.
fn operation_blockers(ledger: &Ledger, deps: &[Deployment], what: &str) -> Vec<String> {
    let mut blockers = unfinished(deps);
    blockers.extend(unresolved(ledger, deps));
    blockers.extend(deps.iter().filter(|d| d.application_may_be_down()).map(|d| {
        format!(
            "deployment {} began a cutover that was never confirmed: the application may be down. {what} is blocked until it is resolved (see `status`)",
            d.id
        )
    }));
    blockers
}

/// The retained releases of one application, read from the journal. Reads only: no Docker, no
/// writer lock, nothing created.
fn releases(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let app: ApplicationId = p
        .value("app")
        .ok_or_else(|| Fail::usage("releases needs --app APPLICATION_ID"))?
        .parse()
        .map_err(|e| Fail::usage(format!("--app is not an application id: {e}")))?;
    let Some(r) = store::read_restored(&rt)? else {
        return Err(Fail::refused(
            "there is no journal; no releases are recorded",
        ));
    };
    let list = harpoon_deploy::rollback::retained_releases(&r.ledger, &r.deployments, &app);
    let serving = harpoon_deploy::serving::serving_release(&r.ledger, &r.deployments, &app);
    match &serving {
        Some(s) => {
            let _ = writeln!(
                out,
                "application {app} serves release {} (deployment {}) in container {}",
                s.release, s.deployment, s.container_name
            );
        }
        None => {
            let _ = writeln!(out, "application {app} has no serving release recorded");
        }
    }
    if list.is_empty() {
        let _ = writeln!(out, "no succeeded releases are recorded");
    }
    for rel in &list {
        let state = if rel.serving {
            "SERVING"
        } else if rel.eligible() {
            "rollback-target"
        } else {
            "not a rollback target"
        };
        let _ = writeln!(
            out,
            "  {} deployment {} finished_ms={} {state}",
            rel.release, rel.deployment, rel.finished_ms
        );
        if let Some(c) = &rel.config {
            let _ = writeln!(out, "    pinned image {} (tag {})", c.image_id, c.image_tag);
        }
        if !rel.eligible() && !rel.serving {
            for pr in &rel.problems {
                let _ = writeln!(out, "    - {pr}");
            }
        }
    }
    let _ = writeln!(
        out,
        "roll back with: harpoon-deploy rollback --app {app} --to RELEASE_ID --simulate [--confirm-rollback] (the exact release id, nothing is guessed)"
    );
    Ok(exit::OK)
}

/// Developer-initiated rollback to one named earlier release, offline only. The target's retained
/// image and pinned configuration are used as they are (nothing is rebuilt); the serving release
/// keeps running until the target is prepared and has passed its preliminary check; the plan, the
/// interruption and the data warning are printed before anything is stopped; the cutover needs
/// `--confirm-rollback`. A failed cutover gets the same one bounded restoration as a replacement.
fn rollback(p: &Parsed, out: &mut String) -> R {
    if !p.switch("simulate") {
        return Err(Fail::refused(
            "live rollback is not enabled in this build: live replacement, which a rollback is, is not enabled. \
             Nothing was read, written or contacted. Use `rollback --simulate` against a separate runtime directory for an offline run.",
        ));
    }
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let app: ApplicationId = p
        .value("app")
        .ok_or_else(|| Fail::usage("rollback needs --app APPLICATION_ID"))?
        .parse()
        .map_err(|e| Fail::usage(format!("--app is not an application id: {e}")))?;
    let target: harpoon_deploy::ids::ReleaseId = p
        .value("to")
        .ok_or_else(|| Fail::usage("rollback needs --to RELEASE_ID (see `releases`)"))?
        .parse()
        .map_err(|e| Fail::usage(format!("--to is not a release id: {e}")))?;
    // Look before opening the writer: a refusal must not create a journal.
    let Some(existing) = store::read_restored(&rt)? else {
        return Err(rollback_refused(&[format!(
            "application {app} has no releases recorded"
        )]));
    };
    harpoon_deploy::rollback::select_target(&existing.ledger, &existing.deployments, &app, &target)
        .map_err(|why| rollback_refused(&why))?;
    let (mut journal, restored) = store::open_writer(&rt, Kind::Simulated)?;
    let deps = restored.deployments.clone();
    let blockers = operation_blockers(&restored.ledger, &deps, "A rollback");
    if !blockers.is_empty() {
        return Err(rollback_refused(&blockers));
    }
    // Decided again from the writer's own view, which is what the run will use.
    let chosen = harpoon_deploy::rollback::select_target(&restored.ledger, &deps, &app, &target)
        .map_err(|why| rollback_refused(&why))?;
    let target_config = chosen.config.expect("an eligible target has a pin");
    let Some(serving) = current_release(&restored.ledger, &deps, &app) else {
        return Err(rollback_refused(&[format!(
            "application {app} has no serving release recorded"
        )]));
    };

    let mut gen = IdGenerator::new(SystemClock, OsEntropy::new());
    let dep_id = gen
        .deployment()
        .map_err(|e| Fail::failed(format!("cannot generate an id: {e}")))?;
    let mut d = Deployment::new(dep_id, app, Trigger::Cli, now_ms());
    d.rollback_to = Some(target.clone());
    d.transition(DeploymentStatus::Queued, now_ms())
        .map_err(|e| Fail::failed(e.to_string()))?;
    let mut cfg = deployment_config(
        target_config.health.startup_deadline_ms,
        restored.ledger.clone(),
    );
    cfg.previous = serving.config.clone();
    cfg.previous_container = Some(harpoon_deploy::backend::ContainerRef {
        owner: serving.container_owner.clone(),
        name: serving.container_name.clone(),
    });
    let mut plan = CutoverPlan::new(&d.application_id, &serving.release, &d.id, &cfg);
    plan.candidate = target.clone();
    plan.rollback = true;
    let _ = writeln!(
        out,
        "rollback deployment {} (application {}) — SIMULATED, no Docker",
        d.id, d.application_id
    );
    let _ = writeln!(
        out,
        "serving now: release {} (deployment {}), container {} of deployment {}",
        serving.release, serving.deployment, serving.container_name, serving.container_owner
    );
    let _ = writeln!(
        out,
        "rolling back to: release {target} (deployment {}), pinned image {}",
        target.deployment(),
        target_config.image_id
    );
    for l in plan.lines() {
        let _ = writeln!(out, "{l}");
    }
    let confirmed = p.switch("confirm-rollback");
    if !confirmed {
        let _ = writeln!(
            out,
            "rollback not confirmed: the target will be prepared and checked, then the run stops with the serving release untouched"
        );
    }

    let ctx = Context {
        previous_release: Some(serving.release.clone()),
    };
    let clock = ManualClock::new(now_ms());
    let mut b = FakeBackend::new(clock.clone())
        .with_runtime(FakeRuntime::world())
        .with_rollback(target_config);
    let report = run_deployment_confirmed(
        &mut d,
        &ctx,
        &mut b,
        &mut journal,
        &clock,
        &cfg,
        &|| false,
        &|_: &CutoverPlan| confirmed,
    )
    .map_err(|e| {
        Fail::failed(format!(
            "the run stopped on a journal or engine error: {e}\nRun `recover`, then `status`. A rollback is never repeated automatically."
        ))
    })?;
    print_report(out, &d, &report);
    if let Some(i) = &d.interruption {
        let _ = match i.duration_ms() {
            Some(ms) => writeln!(
                out,
                "measured interruption: {ms} ms (simulated clock; a real cutover has not been measured)"
            ),
            None => writeln!(
                out,
                "interruption still open: the application may be down; run `recover`"
            ),
        };
    }
    if report.prepared.is_some() {
        let _ = writeln!(
            out,
            "target prepared, serving release untouched. Re-run with --confirm-rollback to perform the (simulated) rollback."
        );
        return Ok(exit::OK);
    }
    if d.status == DeploymentStatus::Succeeded && report.is_clean() {
        let _ = writeln!(
            out,
            "rolled back: application {} serves release {target}, verified by its final health check. Application rollback only; data, volumes and database schema were NOT rolled back.",
            d.application_id
        );
        let _ = writeln!(
            out,
            "automatic triggers are paused for application {}; they do not resume by themselves",
            d.application_id
        );
        return Ok(exit::OK);
    }
    Ok(exit::FAILED)
}

fn rollback_refused(blockers: &[String]) -> Fail {
    let mut m = String::from("rollback refused:\n");
    for b in blockers {
        let _ = writeln!(m, "  {b}");
    }
    m.push_str("Nothing was prepared, stopped or changed. Use `releases`, `status`, `recover` and `reconcile`.");
    Fail::refused(m)
}

// ------------------------------------------------------------------ restore

/// Explicit, bounded restoration after a cutover that was interrupted part-way (D10). One attempt
/// per deployment, ever: a recorded attempt, even one that died, forbids another. Dry run unless
/// `--apply`. Offline only; live restoration is not enabled.
fn restore(p: &Parsed, out: &mut String) -> R {
    if !p.switch("simulate") {
        return Err(Fail::refused(
            "live restoration is not enabled in this build: replacing a running release is not enabled live, so there is nothing to restore live. \
             Nothing was read, written or contacted. Use `restore --simulate` against a separate runtime directory for an offline run.",
        ));
    }
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let dep_id: harpoon_deploy::ids::DeploymentId = p
        .value("deployment")
        .ok_or_else(|| Fail::usage("restore needs --deployment ID"))?
        .parse()
        .map_err(|e| Fail::usage(format!("--deployment is not a deployment id: {e}")))?;
    let Some(r) = store::read_restored(&rt)? else {
        return Err(Fail::refused("there is no journal; nothing to restore"));
    };
    let Some(d) = find_dep(&r.deployments, &dep_id) else {
        return Err(Fail::refused(format!(
            "deployment {dep_id} is not in the journal"
        )));
    };
    if let Some(rec) = &d.recovery {
        return Err(Fail::refused(format!(
            "deployment {dep_id} already has a recorded restoration attempt ({}); it is never repeated. Use `status` and `reconcile`.",
            rec.outcome.as_str()
        )));
    }
    if d.status != DeploymentStatus::Interrupted || !matches!(d.cutover, CutoverState::Begun { .. })
    {
        return Err(Fail::refused(format!(
            "deployment {dep_id} is {} with cutover {:?}; only an interrupted deployment whose cutover began and was never confirmed can be restored",
            d.status, d.cutover
        )));
    }
    let Some(prev_release) = d.replaces.clone() else {
        return Err(Fail::refused(format!(
            "deployment {dep_id} does not record which release it replaced, so what to restore cannot be known; nothing was changed"
        )));
    };
    let prev_dep = prev_release.deployment();
    if find_dep(&r.deployments, &prev_dep).is_none() {
        return Err(Fail::refused(format!(
            "the replaced release's deployment {prev_dep} is not in the journal; nothing was changed"
        )));
    }
    let pinned = find_dep(&r.deployments, &prev_dep).and_then(|x| x.pinned.clone());
    let _ = writeln!(
        out,
        "restore deployment {dep_id} (application {}): cutover began and was never confirmed",
        d.application_id
    );
    let _ = writeln!(
        out,
        "previous release: {prev_release} (deployment {prev_dep})"
    );
    match &pinned {
        Some(c) => {
            let _ = writeln!(
                out,
                "pinned configuration: image {} port {}→{} memory {} B cpu {} pids {} health {}",
                c.image_id,
                c.host_port,
                c.container_port,
                c.memory_bytes,
                c.nano_cpus,
                c.pids,
                c.health.path
            );
        }
        None => {
            let _ = writeln!(out, "pinned configuration: none stored; the attempt will be recorded as not_attempted (config_unavailable)");
        }
    }
    let _ = writeln!(
        out,
        "one attempt, budget 60000 ms, never repeated. The failed candidate is not touched; if it still holds the port, run `reconcile` first (it is not undone by this command)."
    );
    if !p.switch("apply") {
        let _ = writeln!(
            out,
            "dry run: nothing was changed. Re-run with --apply to make the attempt."
        );
        return Ok(exit::OK);
    }

    let (mut journal, restored) = store::open_writer(&rt, Kind::Simulated)?;
    let mut d = find_dep(&restored.deployments, &dep_id)
        .cloned()
        .ok_or_else(|| Fail::failed("the deployment disappeared from the journal"))?;
    let mut cfg = RunConfig {
        prior_ledger: restored.ledger.clone(),
        previous: pinned,
        ..RunConfig::default()
    };
    cfg.policy.restore_budget_ms = 60_000;
    let prev = harpoon_deploy::backend::PreviousRelease {
        release: prev_release,
        deployment: prev_dep,
        config: None,
        container: None,
    };
    let clock = ManualClock::new(now_ms());
    let mut b = FakeBackend::new(clock.clone()).with_runtime(FakeRuntime::world());
    let report = recover_cutover(&mut d, prev, &mut b, &mut journal, &clock, &cfg).map_err(|e| {
        Fail::failed(format!(
            "the restoration stopped on a journal or engine error: {e}\nRun `recover`, then `status`."
        ))
    })?;
    if !report.blocked.is_empty() {
        let _ = writeln!(out, "restoration not started; nothing was recorded or changed, and the one attempt is not spent:");
        for b in &report.blocked {
            let _ = writeln!(out, "  {b}");
        }
        return Ok(exit::REFUSED);
    }
    print_recovery(out, &d);
    if let Some(i) = &d.interruption {
        if let Some(ms) = i.duration_ms() {
            let _ = writeln!(out, "measured interruption: {ms} ms (simulated clock)");
        }
    }
    for w in &report.warnings {
        let _ = writeln!(out, "warning: {w}");
    }
    Ok(match d.recovery.map(|r| r.outcome) {
        Some(RecoveryOutcome::Restored | RecoveryOutcome::RestoredUnverified) => exit::OK,
        _ => exit::FAILED,
    })
}

fn print_report(out: &mut String, d: &Deployment, r: &Report) {
    let _ = writeln!(out, "result: {}", r.status);
    for s in &d.steps {
        let _ = writeln!(out, "  {:<17} {:?}", s.step.as_str(), s.status);
    }
    if let Some(f) = &d.failure {
        let _ = writeln!(out, "failure at {}: {}", f.step.as_str(), f.message);
        // Failure evidence for inspection: the bounded, redacted output of the failing step.
        for (step, cap) in &r.logs {
            if *step == f.step {
                let lines: Vec<&str> = cap.text.lines().collect();
                let from = lines.len().saturating_sub(20);
                for l in &lines[from..] {
                    let _ = writeln!(out, "  | {l}");
                }
            }
        }
    }
    print_recovery(out, d);
    for (step, ev) in &r.evidence {
        let _ = writeln!(
            out,
            "evidence {}: {:?}{} — {}",
            step.as_str(),
            ev.verdict,
            if ev.simulated { " (simulated)" } else { "" },
            ev.summary()
        );
    }
    for w in &r.warnings {
        let _ = writeln!(out, "warning: {w}");
    }
    for e in r.cleanup_problems() {
        let _ = writeln!(
            out,
            "cleanup problem: {} {} — {}",
            e.key.kind.as_str(),
            e.key.name,
            state_word(&e.state)
        );
    }
    for o in &r.unaccounted {
        let _ = writeln!(
            out,
            "unaccounted object (not removed): {} {}",
            o.kind.as_str(),
            o.name
        );
    }
}

// ------------------------------------------------------------------ export-journal

fn export_journal(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let to = PathBuf::from(
        p.value("to")
            .ok_or_else(|| Fail::usage("--to FILE is required"))?,
    );
    let mut src = rt.journal_file();
    if !src.exists() {
        // A runtime no writer has upgraded yet still has only the legacy journal: export that.
        src = rt.journal_dir().join("journal.wal");
    }
    if !src.exists() {
        return Err(Fail::journal(format!(
            "there is no journal at {}",
            src.display()
        )));
    }
    // Compare real locations, so a symlinked or differently spelled path cannot land inside the journal.
    let parent = to
        .parent()
        .filter(|x| !x.as_os_str().is_empty())
        .ok_or_else(|| Fail::usage("--to must be a file path with an existing parent directory"))?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|e| Fail::usage(format!("the directory for --to does not exist: {e}")))?;
    let jdir = std::fs::canonicalize(rt.journal_dir()).unwrap_or_else(|_| rt.journal_dir());
    if parent.starts_with(&jdir) {
        return Err(Fail::usage("--to must be outside the journal directory"));
    }
    let bytes = std::fs::read(&src)
        .map_err(|e| Fail::journal(format!("cannot read {}: {e}", src.display())))?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode_0600()
        .open(&to)
        .map_err(|e| {
            Fail::journal(format!(
                "cannot create {} (it must not exist yet): {e}",
                to.display()
            ))
        })?;
    use std::io::Write as _;
    f.write_all(&bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| Fail::journal(format!("cannot write {}: {e}", to.display())))?;
    let _ = writeln!(
        out,
        "copied {} bytes of {} to {} (mode 0600). The journal itself was not modified.",
        bytes.len(),
        src.display(),
        to.display()
    );
    Ok(exit::OK)
}

trait Mode0600 {
    fn mode_0600(&mut self) -> &mut Self;
}
impl Mode0600 for std::fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }
}

/// `--mgmt-socket` is mandatory for a live deployment: free space in the guest is measured through
/// Harpoon's management channel and cross-checked against Docker.
fn mgmt_flag(p: &Parsed) -> Result<PathBuf, Fail> {
    match p.value("mgmt-socket") {
        None => Err(Fail::config(
            "--mgmt-socket PATH is required for a live deployment: guest free space is measured through the management channel",
        )),
        Some(m) if !m.starts_with('/') => Err(Fail::config("--mgmt-socket must be an absolute path")),
        Some(m) => Ok(PathBuf::from(m)),
    }
}

/// `--forwarder-pid` is mandatory for a live deployment: the host listener has to be proven to be
/// the Harpoon app's own process, so the operator names that process.
fn forwarder_flags(p: &Parsed) -> Result<(PathBuf, u32, String), Fail> {
    let pid: u32 = p
        .value("forwarder-pid")
        .ok_or_else(|| {
            Fail::config(
                "--forwarder-pid N is required for a live deployment: the host listener for the published port must be proven to belong to the Harpoon app (process N)",
            )
        })?
        .parse()
        .ok()
        .filter(|n| *n > 1)
        .ok_or_else(|| Fail::config("--forwarder-pid must be a process id greater than 1"))?;
    let command = p
        .value("forwarder-command")
        .unwrap_or("Harpoon")
        .to_string();
    if command.is_empty() || command.chars().any(|c| c.is_control()) {
        return Err(Fail::config(
            "--forwarder-command must be a plain program name",
        ));
    }
    let lsof = p.value("lsof-path").unwrap_or(LsofCheck::DEFAULT_PATH);
    if !lsof.starts_with('/') {
        return Err(Fail::config("--lsof-path must be an absolute path"));
    }
    Ok((PathBuf::from(lsof), pid, command))
}

// ------------------------------------------------------------------ inventory

/// Read-only snapshot for the before/after comparison of a live acceptance run. Sorted and free of
/// timestamps, so `diff before.txt after.txt` shows exactly what changed.
fn inventory(p: &Parsed, out: &mut String) -> R {
    let sock = socket_flag(p)?
        .ok_or_else(|| Fail::runtime("inventory needs an explicit --docker-socket PATH"))?;
    let a = api(&sock);
    let t = Duration::from_secs(10);
    a.ping(t)
        .map_err(|e| Fail::runtime(format!("{} did not answer: {e}", sock.display())))?;
    let mut lines: Vec<String> = Vec::new();
    let mut ctrs = a
        .list_containers(t)
        .map_err(|e| Fail::runtime(format!("cannot list containers: {e}")))?;
    ctrs.sort_by(|x, y| x.name.cmp(&y.name).then(x.id.cmp(&y.id)));
    for c in &ctrs {
        let harpoon = c
            .labels
            .iter()
            .any(|(k, v)| k == "io.harpoon.managed" && v == "true");
        let mut ports = c.host_ports.clone();
        ports.sort();
        lines.push(format!(
            "container {} name={} state={} harpoon={} published={:?}",
            c.id,
            c.name,
            c.state,
            if harpoon { "yes" } else { "no" },
            ports
        ));
    }
    let mut imgs = a
        .list_images(t)
        .map_err(|e| Fail::runtime(format!("cannot list images: {e}")))?;
    imgs.sort();
    for (id, tag, labels) in &imgs {
        let harpoon = labels
            .iter()
            .any(|(k, v)| k == "io.harpoon.managed" && v == "true");
        lines.push(format!(
            "image {id} tag={tag} harpoon={}",
            if harpoon { "yes" } else { "no" }
        ));
    }
    for (name, driver) in a
        .list_volumes(t)
        .map_err(|e| Fail::runtime(format!("cannot list volumes: {e}")))?
    {
        lines.push(format!("volume {name} driver={driver}"));
    }
    let ports = a
        .published_ports(t)
        .map_err(|e| Fail::runtime(format!("cannot read port mappings: {e}")))?;
    for port in ports {
        lines.push(format!("docker-port-mapping {port}"));
    }
    match p.value("lsof-path") {
        None => lines.push("host-listeners not captured (no --lsof-path given)".into()),
        Some(l) if !l.starts_with('/') => {
            return Err(Fail::usage("--lsof-path must be an absolute path"))
        }
        Some(l) => {
            for x in listeners_in_range(Path::new(l), 38000, 39099)
                .map_err(|e| Fail::runtime(format!("cannot capture host listeners: {e}")))?
            {
                lines.push(format!(
                    "host-listener {} pid={} command={}",
                    x.address, x.pid, x.command
                ));
            }
        }
    }
    lines.sort_by_key(|l| (l.split(' ').next().unwrap_or("").to_string(), l.clone()));
    let mut text = String::from("# harpoon-deploy inventory (read-only; sorted; no timestamps)\n");
    for l in &lines {
        text.push_str(l);
        text.push('\n');
    }
    match p.value("to") {
        None => out.push_str(&text),
        Some(to) => {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode_0600()
                .open(to)
                .map_err(|e| {
                    Fail::journal(format!("cannot create {to} (it must not exist yet): {e}"))
                })?;
            f.write_all(text.as_bytes())
                .and_then(|()| f.sync_all())
                .map_err(|e| Fail::journal(format!("cannot write {to}: {e}")))?;
            let _ = writeln!(
                out,
                "wrote {} inventory lines to {to} (mode 0600)",
                lines.len()
            );
        }
    }
    Ok(exit::OK)
}

// ------------------------------------------------------------------ teardown

/// One selected deployment's resources, from the journal.
struct Targets {
    containers: Vec<harpoon_deploy::ledger::Entry>,
    images: Vec<harpoon_deploy::ledger::Entry>,
    leases: Vec<harpoon_deploy::ledger::Entry>,
}

fn targets_of(
    ledger: &Ledger,
    deps: &[Deployment],
    dep: &harpoon_deploy::ids::DeploymentId,
) -> Targets {
    let mine = |k: ResourceKind| -> Vec<_> {
        ledger
            .entries()
            .iter()
            .filter(|e| &e.deployment == dep && e.key.kind == k)
            .cloned()
            .collect()
    };
    // A restoration recreates the release's container under the failed deployment that restored
    // it; dismantling the release must handle that container too, each by its own recorded ids.
    let containers = harpoon_deploy::serving::containers_of_release(ledger, deps, dep)
        .into_iter()
        .cloned()
        .collect();
    // A rollback serves the release from a container (and port lease) of its own deployment.
    let release = harpoon_deploy::ids::ReleaseId::from_deployment(dep);
    let mut leases = mine(ResourceKind::PortLease);
    for f in deps.iter().filter(|f| {
        f.rollback_to.as_ref() == Some(&release) && f.status == DeploymentStatus::Succeeded
    }) {
        leases.extend(
            ledger
                .entries()
                .iter()
                .filter(|e| e.deployment == f.id && e.key.kind == ResourceKind::PortLease)
                .cloned(),
        );
    }
    Targets {
        containers,
        images: mine(ResourceKind::Image),
        leases,
    }
}

/// How a recorded Docker object is found in a full, successful listing.
fn locate<'a>(e: &harpoon_deploy::ledger::Entry, observed: &'a [Observed]) -> Located<'a> {
    let Some(id) = &e.id else {
        return Located::NoReceipt;
    };
    match observed.iter().find(|o| o.kind == e.key.kind && &o.id == id) {
        Some(o) if o.labelled_for(&e.app, &e.deployment) && (e.key.kind == ResourceKind::Image || o.name == e.key.name) => {
            Located::Ours(o)
        }
        Some(o) => Located::Mismatch(format!(
            "the {} with id {} is named {:?} and carries labels that are not exactly this deployment's; it is not touched",
            e.key.kind.as_str(),
            id,
            o.name
        )),
        None => {
            let same_name = e.key.kind == ResourceKind::Container
                && observed.iter().any(|o| o.kind == e.key.kind && o.name == e.key.name);
            if same_name {
                Located::Mismatch(format!(
                    "a {} named {:?} exists but its id is not the recorded {id}; it is not touched",
                    e.key.kind.as_str(),
                    e.key.name
                ))
            } else {
                Located::Absent
            }
        }
    }
}

enum Located<'a> {
    Ours(&'a Observed),
    Mismatch(String),
    Absent,
    NoReceipt,
}

fn state_note(e: &harpoon_deploy::ledger::Entry) -> &'static str {
    match &e.state {
        EntryState::Created => "recorded",
        EntryState::CleanupFailed(_) => "an earlier removal failed",
        EntryState::CleanupConflict(..) => "an earlier removal was refused as a conflict",
        EntryState::Intended => "no receipt",
        EntryState::Refused(_) => "refused earlier",
        EntryState::Released(_) => "already released",
    }
}

fn teardown(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let apply = p.switch("apply");
    if apply {
        authorized(p)?;
    }
    let dep_id: harpoon_deploy::ids::DeploymentId = p
        .value("deployment")
        .ok_or_else(|| {
            Fail::usage(
                "--deployment dep_… is required (the one succeeded deployment to dismantle)",
            )
        })?
        .parse()
        .map_err(|e| Fail::usage(format!("--deployment is not a deployment id: {e}")))?;
    let socket = socket_flag(p)?.ok_or_else(|| {
        Fail::runtime(
            "teardown needs an explicit --docker-socket PATH; nothing is assumed about the runtime",
        )
    })?;
    if !rt.initialised() {
        return Err(Fail::refused(format!(
            "there is no journal at {}; nothing is recorded to tear down",
            rt.journal_dir().display()
        )));
    }
    if rt.kind()? == Some(Kind::Simulated) {
        return Err(Fail::refused(
            "this is a simulated runtime; there is nothing real to tear down",
        ));
    }
    let mut backend = docker_backend(p, &rt, &socket)?;
    let (mut writer, restored) = if apply {
        let (j, r) = store::open_writer(&rt, Kind::Docker)?;
        (Some(j), r)
    } else {
        (None, store::read_restored(&rt)?.expect("initialised above"))
    };
    let Some(d) = restored
        .deployments
        .iter()
        .find(|d| d.id == dep_id)
        .cloned()
    else {
        return Err(Fail::refused(format!(
            "deployment {dep_id} is not in the journal"
        )));
    };
    if d.status != DeploymentStatus::Succeeded {
        return Err(Fail::refused(format!(
            "deployment {dep_id} is {}, not succeeded. `teardown` dismantles a successful deployment only; use `recover` and `reconcile` for anything else.",
            d.status
        )));
    }
    let t = targets_of(&restored.ledger, &restored.deployments, &dep_id);
    let open: Vec<&harpoon_deploy::ledger::Entry> = t
        .containers
        .iter()
        .chain(&t.images)
        .chain(&t.leases)
        .filter(|e| !e.is_released())
        .collect();
    // An image that an earlier teardown deliberately kept is not "open" work; see below.
    if open.is_empty() {
        let _ = writeln!(
            out,
            "teardown: nothing to do — every recorded resource of {dep_id} is already released"
        );
        return Ok(exit::OK);
    }
    if let Some(e) = open
        .iter()
        .find(|e| matches!(e.state, EntryState::Refused(_)))
    {
        return Err(Fail::refused(format!(
            "{} {} of {dep_id} was refused earlier ({}); its ownership was not proven, so teardown will not touch it. Inspect it with `status`.",
            e.key.kind.as_str(),
            e.key.name,
            match &e.state {
                EntryState::Refused(m) => m.as_str(),
                _ => "",
            }
        )));
    }
    if let Some(e) = t
        .containers
        .iter()
        .chain(&t.images)
        .find(|e| !e.is_released() && e.id.is_none())
    {
        return Err(Fail::refused(format!(
            "{} {} of {dep_id} has no receipt, so what it was is unknown; use `reconcile`, not `teardown`",
            e.key.kind.as_str(),
            e.key.name
        )));
    }

    let list_err = |e: harpoon_deploy::backend::StepError| {
        Fail::runtime(format!(
            "cannot list the runtime ({}): {}\nNothing was assumed absent and nothing was changed.",
            socket.display(),
            e.message
        ))
    };
    let observed = backend.list_resources().map_err(list_err)?;
    let docker = api(&socket);
    let t10 = Duration::from_secs(10);
    let _ = writeln!(
        out,
        "teardown of {dep_id} (application {}) — {}",
        d.application_id,
        if apply {
            "APPLYING"
        } else {
            "dry run: nothing will be changed"
        }
    );

    // ---- phase 1: containers, each located by its recorded ID and ownership
    let mut c_actions: Vec<Action> = Vec::new();
    for e in t.containers.iter().filter(|e| !e.is_released()) {
        let entry = (e.deployment.clone(), e.key.clone());
        match locate(e, &observed) {
            Located::Ours(o) => c_actions.push(Action::Remove {
                entry,
                observed: o.clone(),
                adopt: false,
            }),
            Located::Mismatch(reason) => c_actions.push(Action::Refuse { entry, reason }),
            Located::Absent => c_actions.push(Action::MarkAbsent { entry }),
            Located::NoReceipt => unreachable!("checked above"),
        }
    }
    for a in &c_actions {
        let _ = writeln!(out, "{}{}", if apply { "" } else { "would: " }, describe(a));
    }
    let open_images: Vec<_> = t.images.iter().filter(|e| !e.is_released()).collect();
    let open_leases: Vec<_> = t.leases.iter().filter(|e| !e.is_released()).collect();

    if !apply {
        for e in &open_images {
            let _ = writeln!(
                out,
                "would: remove image {} (id {}) only once its containers are gone and no other container uses it ({})",
                e.key.name,
                e.id.as_deref().unwrap_or("-"),
                state_note(e)
            );
        }
        for e in &open_leases {
            let _ = writeln!(out, "would: release port lease {} only once the containers are verified absent and Docker no longer maps that port", e.key.name);
        }
        let _ = writeln!(out, "dry run: nothing was changed. Add --apply and --{AUTH_FLAG}=… to carry this out; each removal re-verifies id, name and all three ownership labels at that moment, stops before removing, and never uses force, prune or a name pattern.");
        return Ok(exit::OK);
    }

    let j = writer.as_mut().expect("apply holds the writer");
    let mut ledger = restored.ledger.clone();
    let journal_err = |e: harpoon_deploy::engine::EngineError| {
        Fail::failed(format!(
            "journal write failed during teardown: {e}; run `recover`, then `status`, and retry"
        ))
    };
    // One container at a time, and the first one that does not come out clean (a failed removal or a
    // refusal) stops the rest: nothing further is touched on top of an unexplained failure, and the
    // containers not reached are reported and retried by the next run.
    let mut not_reached: Vec<String> = Vec::new();
    for (i, a) in c_actions.iter().enumerate() {
        execute_actions(
            std::slice::from_ref(a),
            &mut ledger,
            j,
            &mut backend,
            &SystemClock,
        )
        .map_err(journal_err)?;
        let key = match a {
            Action::Remove { entry, .. }
            | Action::Refuse { entry, .. }
            | Action::MarkAbsent { entry } => entry,
            _ => continue,
        };
        let clean = ledger.find(&key.0, &key.1).is_some_and(|e| e.is_released());
        if !clean {
            not_reached = c_actions[i + 1..]
                .iter()
                .filter_map(|n| match n {
                    Action::Remove { entry, .. }
                    | Action::Refuse { entry, .. }
                    | Action::MarkAbsent { entry } => Some(entry.1.name.clone()),
                    _ => None,
                })
                .collect();
            break;
        }
    }
    for n in &not_reached {
        let _ = writeln!(out, "not attempted: container {n} — an earlier container did not come out clean, so nothing further was touched");
    }
    let released = |ledger: &Ledger, es: &[harpoon_deploy::ledger::Entry]| {
        es.iter().all(|e| {
            ledger
                .find(&e.deployment, &e.key)
                .is_some_and(|x| x.is_released())
        })
    };
    let containers_gone = released(&ledger, &t.containers);
    let mut problems: Vec<String> = Vec::new();
    if !containers_gone {
        problems.push("a container could not be removed or verified; the image and the port leases were left alone".into());
    }

    // ---- phase 2: images, only when the containers are gone and nothing else uses the image
    if containers_gone && !open_images.is_empty() {
        let fresh = backend.list_resources().map_err(list_err)?;
        let ctrs = docker
            .list_containers(t10)
            .map_err(|e| Fail::runtime(format!("cannot re-list containers: {e}")))?;
        let mut i_actions: Vec<Action> = Vec::new();
        for e in &open_images {
            let entry = (e.deployment.clone(), e.key.clone());
            match locate(e, &fresh) {
                Located::Ours(o) => {
                    let users: Vec<String> = ctrs
                        .iter()
                        .filter(|c| c.image_id.is_empty() || c.image_id == o.id)
                        .map(|c| {
                            if c.image_id.is_empty() {
                                format!("{} (image unknown)", c.name)
                            } else {
                                c.name.clone()
                            }
                        })
                        .collect();
                    if users.is_empty() {
                        i_actions.push(Action::Remove {
                            entry,
                            observed: o.clone(),
                            adopt: false,
                        });
                    } else {
                        let _ = writeln!(
                            out,
                            "kept: image {} is still referenced by {} — not removed",
                            e.key.name,
                            users.join(", ")
                        );
                    }
                }
                Located::Mismatch(reason) => i_actions.push(Action::Refuse { entry, reason }),
                Located::Absent => i_actions.push(Action::MarkAbsent { entry }),
                Located::NoReceipt => unreachable!("checked above"),
            }
        }
        for a in &i_actions {
            let _ = writeln!(out, "{}", describe(a));
        }
        execute_actions(&i_actions, &mut ledger, j, &mut backend, &SystemClock)
            .map_err(journal_err)?;
    }

    // ---- phase 3: leases, only after the containers are verified absent and the port is unmapped
    if containers_gone && !open_leases.is_empty() {
        let mapped = docker
            .published_ports(t10)
            .map_err(|e| Fail::runtime(format!("cannot read Docker's port mappings: {e}")))?;
        let mut l_actions: Vec<Action> = Vec::new();
        for e in &open_leases {
            let port: Option<u16> = e.key.name.parse().ok();
            match port {
                Some(pn) if !mapped.contains(&pn) => l_actions.push(Action::ReleaseLease {
                    entry: (e.deployment.clone(), e.key.clone()),
                }),
                Some(pn) => problems.push(format!(
                    "port {pn} is still mapped by a container; lease kept"
                )),
                None => problems.push(format!("lease {:?} is not a port number; kept", e.key.name)),
            }
        }
        for a in &l_actions {
            let _ = writeln!(out, "{}", describe(a));
        }
        execute_actions(&l_actions, &mut ledger, j, &mut backend, &SystemClock)
            .map_err(journal_err)?;
    }

    // ---- outcome, from the journal's own state
    let t_after = targets_of(&ledger, &restored.deployments, &dep_id);
    for e in t_after
        .containers
        .iter()
        .chain(&t_after.images)
        .chain(&t_after.leases)
    {
        match &e.state {
            EntryState::CleanupFailed(m) => problems.push(format!(
                "{} {}: removal failed and is recorded: {m}",
                e.key.kind.as_str(),
                e.key.name
            )),
            EntryState::CleanupConflict(_, m) => problems.push(format!(
                "{} {}: removal refused as a conflict and recorded: {m}",
                e.key.kind.as_str(),
                e.key.name
            )),
            EntryState::Refused(m) => problems.push(format!(
                "{} {}: refused, not touched: {m}",
                e.key.kind.as_str(),
                e.key.name
            )),
            _ => {}
        }
    }
    if problems.is_empty() {
        let kept = t_after.images.iter().any(|e| !e.is_released());
        let _ = writeln!(
            out,
            "teardown: complete{}",
            if kept {
                " (an image was kept because something still uses it)"
            } else {
                ""
            }
        );
        Ok(exit::OK)
    } else {
        problems.dedup();
        let _ = writeln!(out, "teardown: incomplete — nothing was forced; the state is recorded and a repeat run retries only what is left:");
        for pr in problems {
            let _ = writeln!(out, "  {pr}");
        }
        Ok(exit::FAILED)
    }
}

// ------------------------------------------------------------------ cleanup-image (B8.2)

/// Remove the retained image of ONE deployment that did not succeed. Offline-tested only.
///
/// Journal rules (nothing is read from Docker until they all hold), then Docker rules (image id,
/// all three labels, only the recorded tag, no digest, no container in any state), then the
/// deletion by id without force (parent layers are left alone). Problems that make removal uncertain are
/// refused **without writing the journal**; only an ownership mismatch is recorded, as the same
/// terminal refusal `reconcile` and `teardown` record. The attempt is journaled before Docker is
/// asked, and the result after.
fn cleanup_image(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let apply = p.switch("apply");
    if apply {
        authorized(p)?;
    }
    let dep_id: harpoon_deploy::ids::DeploymentId = p
        .value("deployment")
        .ok_or_else(|| Fail::usage("--deployment dep_… is required (the one deployment whose retained image to remove)"))?
        .parse()
        .map_err(|e| Fail::usage(format!("--deployment is not a deployment id: {e}")))?;
    let image = p.value("image").ok_or_else(|| {
        Fail::usage("--image sha256:… is required (the image id recorded for that deployment)")
    })?;
    let well_formed = image.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64 && h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
    });
    if !well_formed {
        return Err(Fail::usage(
            "--image must be a full image id, sha256: followed by 64 lower-case hex digits; a tag or a prefix is not accepted",
        ));
    }
    let socket = socket_flag(p)?.ok_or_else(|| {
        Fail::runtime("cleanup-image needs an explicit --docker-socket PATH; nothing is assumed about the runtime")
    })?;
    if !rt.initialised() {
        return Err(Fail::refused(format!(
            "there is no journal at {}; nothing is recorded to clean up",
            rt.journal_dir().display()
        )));
    }
    if rt.kind()? == Some(Kind::Simulated) {
        return Err(Fail::refused(
            "this is a simulated runtime; there is nothing real to clean up",
        ));
    }
    let mut backend = docker_backend(p, &rt, &socket)?;
    let (mut writer, restored) = if apply {
        let (j, r) = store::open_writer(&rt, Kind::Docker)?;
        (Some(j), r)
    } else {
        (None, store::read_restored(&rt)?.expect("initialised above"))
    };
    let ledger = &restored.ledger;

    // ---- journal rules
    let Some(d) = restored.deployments.iter().find(|d| d.id == dep_id) else {
        return Err(Fail::refused(format!(
            "deployment {dep_id} is not in the journal"
        )));
    };
    let images: Vec<&harpoon_deploy::ledger::Entry> = ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == dep_id && e.key.kind == ResourceKind::Image)
        .collect();
    let Some(e) = images.iter().find(|e| e.id.as_deref() == Some(image)) else {
        let recorded: Vec<String> = images
            .iter()
            .map(|e| {
                e.id.clone()
                    .unwrap_or_else(|| format!("{} (no receipt)", e.key.name))
            })
            .collect();
        return Err(Fail::refused(format!(
            "deployment {dep_id} has no recorded image with id {image}. Recorded: {}.{}",
            if recorded.is_empty() {
                "none".to_string()
            } else {
                recorded.join(", ")
            },
            if images.iter().any(|e| e.id.is_none()) {
                " An image with no receipt is resolved by `reconcile`, not here."
            } else {
                ""
            }
        )));
    };
    if e.is_released() {
        let _ = writeln!(
            out,
            "cleanup-image: nothing to do — image {image} of {dep_id} is already released"
        );
        return Ok(exit::OK);
    }
    if let EntryState::Refused(m) = &e.state {
        return Err(Fail::refused(format!(
            "image {image} of {dep_id} was refused earlier ({m}); its ownership was not proven, so it will not be touched. Inspect it with `status`."
        )));
    }
    match end_state(d.status) {
        None => {
            return Err(Fail::refused(format!(
                "deployment {dep_id} is {}, not finished; run `recover` first",
                d.status
            )))
        }
        Some(EndState::Succeeded) => {
            return Err(Fail::refused(format!(
                "deployment {dep_id} succeeded. Its image is part of the running release; use `teardown` to dismantle it."
            )))
        }
        Some(EndState::NotSucceeded | EndState::FailedCutover) => {}
    }
    if e.disposition != Disposition::Retain {
        return Err(Fail::refused(format!(
            "image {image} is not a retained image ({:?}); `reconcile` manages it",
            e.disposition
        )));
    }
    let open: Vec<String> = ledger
        .entries()
        .iter()
        .filter(|o| o.deployment == dep_id && o.key != e.key && !o.is_released())
        .map(|o| format!("{} {}", o.key.kind.as_str(), o.key.name))
        .collect();
    if !open.is_empty() {
        return Err(Fail::refused(format!(
            "deployment {dep_id} still has unreleased resources ({}); run `reconcile` first. Nothing was changed.",
            open.join(", ")
        )));
    }
    if let Some(o) = ledger.entries().iter().find(|o| {
        o.deployment != dep_id
            && o.key.kind == ResourceKind::Image
            && !o.is_released()
            && o.id.as_deref() == Some(image)
    }) {
        return Err(Fail::refused(format!(
            "image {image} is also recorded for deployment {}; it is shared, so it will not be removed",
            o.deployment
        )));
    }

    // ---- Docker rules (read-only)
    let runtime_err = |what: &str, e: harpoon_deploy::backend::StepError| {
        Fail::runtime(format!(
            "{what} ({}): {}\nNothing was assumed absent or eligible and nothing was changed.",
            socket.display(),
            e.message
        ))
    };
    let verdict = backend
        .assess_retained_image(image, &e.key.name, &e.app, &e.deployment)
        .map_err(|er| runtime_err("cannot inspect the image or list containers", er))?;
    let _ = writeln!(
        out,
        "cleanup-image of {dep_id} (application {}) — {}",
        d.application_id,
        if apply {
            "APPLYING"
        } else {
            "dry run: nothing will be changed"
        }
    );
    let _ = writeln!(
        out,
        "recorded: image {image} (tag {}), retained after a {} deployment",
        e.key.name, d.status
    );
    let entry = (e.deployment.clone(), e.key.clone());
    let mut action = None;
    match &verdict {
        harpoon_deploy_docker::backend::ImageVerdict::NotExclusive(why) => {
            return Err(Fail::refused(format!(
                "refused: {why}. Nothing was changed and the journal was not written; remove the cause and run this command again."
            )));
        }
        harpoon_deploy_docker::backend::ImageVerdict::Ownership(why) => {
            if !apply {
                let _ = writeln!(
                    out,
                    "would: REFUSE image {image}: {why} — --apply would record this as a terminal refusal, exactly as `reconcile` does"
                );
                return Ok(exit::REFUSED);
            }
            action = Some(Action::Refuse {
                entry: entry.clone(),
                reason: why.clone(),
            });
        }
        harpoon_deploy_docker::backend::ImageVerdict::Absent => {
            let _ = writeln!(
                out,
                "verified: Docker has no image {image}; --apply records it as already absent"
            );
        }
        harpoon_deploy_docker::backend::ImageVerdict::Eligible => {
            let _ = writeln!(
                out,
                "verified: deployment is {}; its other resources are released; Docker holds exactly this id with all three Harpoon labels; only the recorded tag; no digest; no container, running or stopped, uses it",
                d.status
            );
            let _ = writeln!(
                out,
                "{}remove image {image} by id (force=false, parent layers left alone); Docker itself re-checks references at that moment",
                if apply { "" } else { "would: " }
            );
        }
    }
    if !apply {
        let _ = writeln!(
            out,
            "dry run: nothing was changed. Add --apply and --{AUTH_FLAG}=… to carry this out."
        );
        return Ok(exit::OK);
    }

    // ---- apply: the attempt is journaled before Docker is asked, the result after
    let mut ledger = restored.ledger.clone();
    let j = writer.as_mut().expect("apply holds the writer");
    let journal_err = |e: harpoon_deploy::engine::EngineError| {
        Fail::failed(format!(
            "journal write failed during cleanup-image: {e}; run `recover`, then `status`, and retry"
        ))
    };
    let action = action.unwrap_or_else(|| Action::Remove {
        entry: entry.clone(),
        observed: Observed {
            kind: ResourceKind::Image,
            name: e.key.name.clone(),
            id: image.to_string(),
            labels: Vec::new(),
        },
        adopt: false,
    });
    let mut ops = backend.retained_image_ops();
    execute_actions(
        std::slice::from_ref(&action),
        &mut ledger,
        j,
        &mut ops,
        &SystemClock,
    )
    .map_err(journal_err)?;

    let after = ledger.find(&entry.0, &entry.1).expect("the entry exists");
    match &after.state {
        EntryState::Released(harpoon_deploy::ledger::Released::Removed) => {
            let _ = writeln!(out, "removed image {image} by id");
            let _ = writeln!(out, "cleanup-image: complete");
            Ok(exit::OK)
        }
        EntryState::Released(harpoon_deploy::ledger::Released::AlreadyAbsent) => {
            let _ = writeln!(
                out,
                "image {image} is not in Docker; recorded as already absent"
            );
            let _ = writeln!(out, "cleanup-image: complete");
            Ok(exit::OK)
        }
        EntryState::CleanupConflict(_, m)
            if ledger.deferred_image_conflict(after, Some(EndState::NotSucceeded)) =>
        {
            let _ = writeln!(out, "kept: {m}");
            let _ = writeln!(
                out,
                "cleanup-image: incomplete — the conflict is recorded and does not block other deployments; remove its cause and run this command again to retry"
            );
            Ok(exit::FAILED)
        }
        EntryState::CleanupConflict(_, m) => {
            let _ = writeln!(out, "kept: {m}");
            let _ = writeln!(
                out,
                "cleanup-image: incomplete — the conflict is recorded, but this runtime stays blocked until the deployment's other unresolved work is settled"
            );
            Ok(exit::FAILED)
        }
        EntryState::CleanupFailed(m) => {
            let _ = writeln!(
                out,
                "cleanup-image: incomplete — the outcome is not certain ({m}); it is recorded, nothing was forced, and the runtime stays blocked until a repeat of this command settles it"
            );
            Ok(exit::FAILED)
        }
        EntryState::Refused(m) => {
            let _ = writeln!(
                out,
                "cleanup-image: refused — {m}; recorded as a terminal refusal; nothing was touched"
            );
            Ok(exit::FAILED)
        }
        _ => {
            let _ = writeln!(out, "cleanup-image: incomplete — the state was not settled");
            Ok(exit::FAILED)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::deployment_config;
    use harpoon_deploy::ledger::Ledger;

    #[test]
    fn health_timeout_also_extends_the_engine_preliminary_budget() {
        let cfg = deployment_config(125_000, Ledger::new());
        assert_eq!(cfg.budgets.preliminary_ms, 125_000);
    }
}
