//! The desktop's typed window onto the real deployment engine (Milestone E).
//!
//! No daemon, no server, no second database. The engine's own journal (SQLite, one writer at a time) is
//! read directly through `harpoon-deploy-journal`'s read-only replay and turned into plain facts
//! ([`EngineSnapshot`]); the UI derives everything it shows from those facts. Mutations run the same
//! `harpoon_deploy_cli` library the CLI runs, with `Gate::shipping()` and the real Keychain, and answer with
//! the engine's own refusal text. Nothing here parses human CLI output to learn *state*: after an operation
//! the UI asks for a fresh snapshot.
//!
//! Integrity: the shipping gate is closed, so live deploy, replace, rollback and restore are refused by the
//! engine with its real reason; this module has no other execution path and never starts the
//! acceptance-only binary. A missing, damaged or locked store is reported as such, never replaced by
//! invented data. Secret values travel only as the `value` of a secret operation, into the engine's
//! standard-input path and the Keychain; they are never part of a snapshot or a result.

use harpoon_deploy::config::{self, Setting, Value};
use harpoon_deploy::control::{self};
use harpoon_deploy::ids::{ApplicationId, DeploymentId};
use harpoon_deploy::secrets::SecretStore;
use harpoon_deploy::state::{CutoverState, Deployment};
use harpoon_deploy_cli::{run_with, Gate, Services};
use harpoon_deploy_journal::{writer_lock_status, LockStatus, Restored, SqliteJournal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const SCHEMA: u32 = 1;
const GATE_REASON: &str = "Live deployment, replacement, rollback and restoration are closed in this build of Harpoon (the shipping gate). Nothing was started, stopped or changed.";

/// `<app data>/deploy`: the runtime directory of the desktop's own deployment engine store.
pub fn runtime_dir(app_data: &Path) -> PathBuf {
    app_data.join("deploy")
}

/// The runtime directory and journal directory exist and are private (0700), as the engine requires.
fn prepare(rt: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    for d in [rt.to_path_buf(), journal_dir(rt)] {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&d)
            .map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot secure {}: {e}", d.display()))?;
    }
    Ok(())
}

fn journal_dir(rt: &Path) -> PathBuf {
    rt.join("deploy-journal")
}

fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn name_of(d: &impl std::fmt::Debug) -> String {
    let t = format!("{d:?}");
    snake(t.split(['{', '(', ' ']).next().unwrap_or(&t))
}

// ------------------------------------------------------------------ the facts

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    pub schema: u32,
    pub generated_ms: u64,
    pub runtime_dir: String,
    /// Whose store this is: Harpoon's own, or an external runtime attached read-only.
    pub source: RuntimeSourceDto,
    /// `missing` (nothing was ever recorded), `ok`, or `error` (the store is damaged or unreadable).
    pub store: String,
    pub store_error: Option<String>,
    /// A writer holds the journal right now (a deployment or another command is running).
    pub writer_held: bool,
    pub gate: GateDto,
    /// The loopback router behind the friendly application URLs.
    pub router: RouterDto,
    pub repos: Vec<RepoDto>,
    pub apps: Vec<AppDto>,
    pub definitions: Vec<DefinitionDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterDto {
    pub domain: &'static str,
    /// The loopback port the router listens on, when it is running.
    pub port: Option<u16>,
    /// Why it is not running (for example the port is taken); the direct-port URLs keep working.
    pub error: Option<String>,
    /// Whether this Mac resolves `*.harpoon.localhost` to loopback: `null` until checked, `false` with the reason in
    /// `resolveError`. When it does not, the friendly address would not open and the UI says so.
    pub resolves: Option<bool>,
    pub resolve_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSourceDto {
    /// `own` (the desktop's deployment engine store) or `external` (attached read-only by path).
    pub kind: &'static str,
    pub path: String,
    /// `sqlite`, `legacy` (the pre-SQLite `journal.wal`, read without migrating it) or `none`.
    pub journal_format: &'static str,
    /// An external source is never written: not by this module, and not by any operation.
    pub read_only: bool,
}

/// One ledger entry: an image, container or port lease the engine recorded for a deployment.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDto {
    pub kind: String,
    /// Image tag, container name, or the decimal port number.
    pub name: String,
    /// The id the runtime returned (absent until a receipt was recorded).
    pub id: Option<String>,
    pub state: String,
    pub keep: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateDto {
    pub live_deploy: bool,
    pub live_replacement: bool,
    /// Executing queued Git events (Gate U, unattended execution). Closed in every shipping build.
    pub live_triggered: bool,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDto {
    pub name: String,
    pub url: String,
    pub app: String,
    pub project: Option<String>,
    pub environment: Option<String>,
    pub branch: String,
    pub filters: Vec<String>,
    pub credential: String,
    pub webhook: Option<String>,
    pub container_port: u16,
    pub dockerfile: Option<String>,
    pub context: Option<String>,
    pub target: Option<String>,
    pub health_path: Option<String>,
    /// The version of this registration (when it was last recorded). An edit names the one it was based on.
    pub registered_ms: u64,
    pub seen: Vec<SeenDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenDto {
    pub ref_name: String,
    pub commit: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDto {
    pub id: String,
    /// The stable `<label>.harpoon.localhost` name of this application (its identity, not a release's).
    pub hostname: String,
    /// `serving`, `idle` (nothing recorded as serving), `conflict` (another application derives the same name) or
    /// `external` (read-only attached runtime: not routed).
    pub route: &'static str,
    pub repo: Option<String>,
    /// Set while the application has no registered repository because its registration was removed: the name it
    /// had, and the address it last had. The application, its history and its serving release are unaffected.
    pub disconnected: Option<DisconnectedDto>,
    pub deployments: Vec<DeploymentDto>,
    pub serving: Option<ServingDto>,
    pub pauses: Vec<PauseDto>,
    pub events: Vec<EventDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectedDto {
    pub name: String,
    pub url: String,
    pub registered_ms: u64,
    pub removed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServingDto {
    pub release: String,
    pub deployment: String,
    pub container: String,
    /// `original`, `restored` or `rolled_back`
    pub via: String,
    pub since_ms: u64,
    pub host_port: Option<u16>,
    pub health_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseDto {
    pub origin: String,
    pub kind: String,
    pub since_ms: u64,
    pub context: String,
    pub recorded: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDto {
    pub delivery: String,
    pub source: String,
    pub revision: String,
    pub received_ms: u64,
    pub status: String,
    pub decided_ms: Option<u64>,
    pub related: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepDto {
    pub name: String,
    pub status: String,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub note: String,
    pub log: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDto {
    pub repo: String,
    pub url: String,
    pub selector: String,
    pub requested: String,
    pub commit: String,
    pub trigger: String,
    pub delivery: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinDto {
    pub image_tag: String,
    pub image_id: String,
    pub host_port: u16,
    pub container_port: u16,
    pub health_path: String,
    pub env_names: Vec<String>,
    pub secrets: Vec<PinSecretDto>,
    pub snapshot: Option<String>,
    pub build: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinSecretDto {
    pub name: String,
    pub id: String,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDto {
    pub name: String,
    /// `config` or `secret`
    pub kind: String,
    pub build: bool,
    pub runtime: bool,
    /// The scope that supplied it, or `command line`.
    pub scope: String,
    /// Config values only. A secret is never a value.
    pub value: Option<String>,
    pub secret_id: Option<String>,
    pub version: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDto {
    pub id: String,
    /// What the engine recorded creating for this deployment (images, containers, port leases), with ids.
    pub resources: Vec<ResourceDto>,
    pub status: String,
    pub trigger: String,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub steps: Vec<StepDto>,
    pub cutover: String,
    pub cutover_at_ms: Option<u64>,
    pub failure: Option<FailureDto>,
    pub recovery: Option<RecoveryDto>,
    pub interruption: Option<InterruptionDto>,
    pub release: Option<String>,
    pub replaces: Option<String>,
    pub rollback_to: Option<String>,
    /// A cutover began and was never confirmed: the application may be down.
    pub may_be_down: bool,
    /// The record is not terminal. Whether anything is still working on it is `writerHeld`.
    pub unfinished: bool,
    pub source: Option<SourceDto>,
    pub pin: Option<PinDto>,
    pub config: Option<Vec<EntryDto>>,
    /// Changes from the previous snapshot of the same application: names, scopes, flags, versions.
    pub config_diff: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureDto {
    pub step: String,
    pub kind: String,
    pub message: String,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryDto {
    pub outcome: String,
    /// Why a restoration failed or was not attempted: a code and the engine's plain words.
    pub reason: Option<String>,
    pub reason_text: Option<String>,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    pub budget_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptionDto {
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionDto {
    pub scope: String,
    pub name: String,
    /// `config`, `secret` or `unset`
    pub kind: String,
    pub build: bool,
    pub runtime: bool,
    pub value: Option<String>,
    pub secret_id: Option<String>,
    pub version: Option<u32>,
    pub at_ms: u64,
}

fn entry_dto(e: &config::Entry) -> EntryDto {
    let (kind, value, secret_id, version) = match &e.value {
        Value::Config(v) => ("config", Some(v.clone()), None, None),
        Value::Secret(r) => ("secret", None, Some(r.id.clone()), Some(r.version)),
    };
    EntryDto {
        name: e.name.clone(),
        kind: kind.into(),
        build: e.flags.build,
        runtime: e.flags.runtime,
        scope: e
            .source
            .as_ref()
            .map(|s| s.slug())
            .unwrap_or_else(|| "command line".into()),
        value,
        secret_id,
        version,
    }
}

fn reason_of(o: harpoon_deploy::state::RecoveryOutcome) -> Option<harpoon_deploy::state::Reason> {
    use harpoon_deploy::state::RecoveryOutcome::{NotAttempted, RestoreFailed};
    match o {
        RestoreFailed(r) | NotAttempted(r) => Some(r),
        _ => None,
    }
}

fn entry_state(s: &harpoon_deploy::ledger::EntryState) -> &'static str {
    use harpoon_deploy::ledger::{EntryState::*, Released};
    match s {
        Intended => "intended",
        Created => "created",
        Released(Released::Removed) => "released",
        Released(Released::AlreadyAbsent) => "released_absent",
        CleanupFailed(_) => "cleanup_failed",
        CleanupConflict(..) => "cleanup_conflict",
        Refused(_) => "refused",
    }
}

fn deployment_dto(d: &Deployment, r: &Restored, prev_snapshot: Option<&config::Snapshot>) -> DeploymentDto {
    let logged: BTreeSet<&str> = r.logs.of(&d.id).iter().map(|l| l.step.as_str()).collect();
    let snap = r.config.snapshot_of(&d.id);
    let (cutover, cutover_at_ms) = match d.cutover {
        CutoverState::NotStarted => ("not_started", None),
        CutoverState::NotRequired => ("not_required", None),
        CutoverState::Begun { at_ms } => ("begun", Some(at_ms)),
        CutoverState::Confirmed { at_ms } => ("confirmed", Some(at_ms)),
        CutoverState::Failed { at_ms } => ("failed", Some(at_ms)),
    };
    DeploymentDto {
        id: d.id.to_string(),
        resources: r
            .ledger
            .entries()
            .iter()
            .filter(|e| e.deployment == d.id)
            .map(|e| ResourceDto {
                kind: e.key.kind.as_str().to_string(),
                name: e.key.name.clone(),
                id: e.id.clone(),
                state: entry_state(&e.state).to_string(),
                keep: match e.disposition {
                    harpoon_deploy::ledger::Disposition::Retain => "retain",
                    harpoon_deploy::ledger::Disposition::RemoveOnFailure => "remove_on_failure",
                    harpoon_deploy::ledger::Disposition::RemoveAlways => "remove_always",
                }
                .to_string(),
            })
            .collect(),
        status: d.status.to_string(),
        trigger: name_of(&d.trigger),
        created_ms: d.created_ms,
        started_ms: d.started_ms,
        finished_ms: d.finished_ms,
        steps: d
            .steps
            .iter()
            .map(|s| StepDto {
                name: s.step.as_str().into(),
                status: name_of(&s.status),
                started_ms: s.started_ms,
                finished_ms: s.finished_ms,
                note: s.note.clone(),
                log: logged.contains(s.step.as_str()),
            })
            .collect(),
        cutover: cutover.into(),
        cutover_at_ms,
        failure: d.failure.as_ref().map(|f| FailureDto {
            step: f.step.as_str().into(),
            kind: name_of(&f.kind),
            message: f.message.clone(),
            timed_out: f.timed_out,
        }),
        recovery: d.recovery.map(|x| RecoveryDto {
            outcome: x.outcome.as_str().into(),
            reason: reason_of(x.outcome).map(|r| name_of(&r)),
            reason_text: reason_of(x.outcome).map(|r| r.as_str_for_humans().to_string()),
            started_ms: x.started_ms,
            finished_ms: x.finished_ms,
            budget_ms: x.budget_ms,
        }),
        interruption: d.interruption.map(|i| InterruptionDto {
            started_ms: i.started_ms,
            ended_ms: i.ended_ms,
        }),
        release: d.release.as_ref().map(|x| x.to_string()),
        replaces: d.replaces.as_ref().map(|x| x.to_string()),
        rollback_to: d.rollback_to.as_ref().map(|x| x.to_string()),
        may_be_down: d.application_may_be_down(),
        unfinished: !d.status.is_terminal(),
        source: r.git.source_of(&d.id).map(|s| SourceDto {
            repo: s.repo.clone(),
            url: s.url.clone(),
            selector: s.selector.as_str().into(),
            requested: s.requested.clone(),
            commit: s.commit.clone(),
            trigger: s.trigger.clone(),
            delivery: s.delivery.clone(),
        }),
        pin: d.pinned.as_ref().map(|p| PinDto {
            image_tag: p.image_tag.clone(),
            image_id: p.image_id.clone(),
            host_port: p.host_port,
            container_port: p.container_port,
            health_path: p.health.path.clone(),
            env_names: p.env.iter().map(|(k, _)| k.clone()).collect(),
            secrets: p
                .secrets
                .iter()
                .map(|(n, s)| PinSecretDto {
                    name: n.clone(),
                    id: s.id.clone(),
                    version: s.version,
                })
                .collect(),
            snapshot: p.snapshot.clone(),
            build: p.build.clone(),
        }),
        config: snap.map(|s| s.entries.iter().map(entry_dto).collect()),
        config_diff: match (snap, prev_snapshot) {
            (Some(now), Some(before)) => config::diff(&before.entries, &now.entries),
            _ => Vec::new(),
        },
    }
}

/// Everything the Deployments views need, from the engine's own records. Read-only.
pub fn snapshot(rt: &Path, now_ms: u64) -> EngineSnapshot {
    build_snapshot(rt, now_ms, "own")
}

fn app_ids_of(r: &harpoon_deploy_journal::Restored) -> Vec<ApplicationId> {
    let mut ids: Vec<ApplicationId> = r.deployments.iter().map(|d| d.application_id.clone()).collect();
    ids.extend(r.git.repos.values().map(|x| x.app.clone()));
    ids.sort_by_key(|a| a.to_string());
    ids.dedup();
    ids
}

/// The application router's table, from the durable records alone (read-only: no writer is opened).
/// A store that is not there yet is an empty table; an unreadable one is an error, so the router keeps its last table.
pub fn route_table(rt: &Path) -> Result<harpoon_deploy::route::RouteTable, String> {
    let jd = journal_dir(rt);
    if !jd.join(harpoon_deploy_journal::SQLITE_FILE).exists() {
        return Ok(Default::default());
    }
    let r = SqliteJournal::read_only(&jd).map_err(|e| e.0)?;
    Ok(harpoon_deploy::route::table_of(&r.ledger, &r.deployments, &app_ids_of(&r)))
}

fn journal_format(jd: &Path) -> &'static str {
    if jd.join(harpoon_deploy_journal::SQLITE_FILE).exists() {
        "sqlite"
    } else if jd.join(harpoon_deploy_journal::JOURNAL_FILE).exists() {
        "legacy"
    } else {
        "none"
    }
}

/// The one reader behind both the desktop's own store and an attached external runtime. It only ever calls
/// `SqliteJournal::read_only` and `writer_lock_status`: no writer is opened, nothing is locked, created,
/// repaired or migrated.
fn build_snapshot(rt: &Path, now_ms: u64, kind: &'static str) -> EngineSnapshot {
    let jd = journal_dir(rt);
    let mut snap = EngineSnapshot {
        schema: SCHEMA,
        generated_ms: now_ms,
        runtime_dir: rt.display().to_string(),
        source: RuntimeSourceDto {
            kind,
            path: rt.display().to_string(),
            journal_format: journal_format(&jd),
            read_only: kind == "external",
        },
        store: "missing".into(),
        store_error: None,
        writer_held: false,
        gate: GateDto {
            live_deploy: Gate::shipping().live_deploy_enabled,
            live_replacement: Gate::shipping().live_replacement_enabled,
            live_triggered: Gate::shipping().live_triggered_enabled,
            reason: GATE_REASON,
        },
        router: {
            let (port, error) = crate::app_router::status();
            let res = crate::app_router::resolution();
            RouterDto {
                domain: harpoon_deploy::route::DOMAIN,
                port,
                error,
                resolves: res.as_ref().map(|r| r.is_ok()),
                resolve_error: res.and_then(|r| r.err()),
            }
        },
        repos: vec![],
        apps: vec![],
        definitions: vec![],
    };
    // An empty journal directory (a first operation that failed before writing anything) is an
    // uninitialised store, not a damaged one. A directory with either journal file in it is read, and
    // fails closed below if that file is unreadable.
    if !jd.join(harpoon_deploy_journal::SQLITE_FILE).exists() && !jd.join(harpoon_deploy_journal::JOURNAL_FILE).exists()
    {
        return snap;
    }
    snap.writer_held = matches!(writer_lock_status(&jd), Ok(LockStatus::Held));
    let r = match SqliteJournal::read_only(&jd) {
        Ok(r) => r,
        Err(e) => {
            snap.store = "error".into();
            snap.store_error = Some(e.0);
            return snap;
        }
    };
    snap.store = "ok".into();
    for repo in r.git.repos.values() {
        snap.repos.push(RepoDto {
            name: repo.name.clone(),
            url: repo.url.clone(),
            app: repo.app.to_string(),
            project: repo.project.clone(),
            environment: repo.environment.clone(),
            branch: repo.branch.clone(),
            filters: repo.filters.clone(),
            credential: repo.cred.as_str().into(),
            webhook: repo.webhook.map(|w| w.as_str().into()),
            container_port: repo.container_port,
            dockerfile: repo.dockerfile.clone(),
            context: repo.context.clone(),
            target: repo.target.clone(),
            health_path: repo.health_path.clone(),
            registered_ms: repo.at_ms,
            seen: r
                .git
                .seen
                .iter()
                .filter(|((name, _), _)| name == &repo.name)
                .map(|((_, rf), s)| SeenDto {
                    ref_name: rf.clone(),
                    commit: s.commit.clone(),
                    at_ms: s.at_ms,
                })
                .collect(),
        });
    }
    let app_ids = app_ids_of(&r);
    let routes = harpoon_deploy::route::table_of(&r.ledger, &r.deployments, &app_ids);
    let pauses = control::active_pauses(&r.deployments, &r.control);
    for app in app_ids {
        let mut deps: Vec<&Deployment> = r.deployments.iter().filter(|d| d.application_id == app).collect();
        deps.sort_by_key(|d| (d.created_ms, d.id.to_string()));
        let mut prev: Option<&config::Snapshot> = None;
        let mut dtos = Vec::new();
        for d in deps {
            dtos.push(deployment_dto(d, &r, prev));
            if let Some(s) = r.config.snapshot_of(&d.id) {
                prev = Some(s);
            }
        }
        let serving = harpoon_deploy::serving::serving_release(&r.ledger, &r.deployments, &app).map(|s| ServingDto {
            release: s.release.to_string(),
            deployment: s.deployment.to_string(),
            container: s.container_name.clone(),
            via: match s.via {
                harpoon_deploy::serving::ServedBy::Original => "original",
                harpoon_deploy::serving::ServedBy::Restored { .. } => "restored",
                harpoon_deploy::serving::ServedBy::RolledBack { .. } => "rolled_back",
            }
            .into(),
            since_ms: s.since_ms,
            host_port: s.config.as_ref().map(|c| c.host_port),
            health_path: s.config.as_ref().map(|c| c.health.path.clone()),
        });
        snap.apps.push(AppDto {
            id: app.to_string(),
            hostname: harpoon_deploy::route::hostname(&app),
            // The router reads Harpoon's own journal only: an attached external runtime is never routed.
            route: match routes.route_of(&app) {
                _ if kind == "external" => "external",
                Some(harpoon_deploy::route::Route::Serving { .. }) => "serving",
                Some(harpoon_deploy::route::Route::Conflict { .. }) => "conflict",
                _ => "idle",
            },
            repo: r.git.repo_of_app(&app).map(|x| x.name.clone()),
            disconnected: r.git.disconnected(&app).map(|x| DisconnectedDto {
                name: x.name.clone(),
                url: x.url.clone(),
                registered_ms: x.registration,
                removed_ms: x.at_ms,
            }),
            deployments: dtos,
            serving,
            pauses: pauses
                .iter()
                .filter(|p| p.app == app)
                .map(|p| PauseDto {
                    origin: p.origin.to_string(),
                    kind: p.kind.as_str().into(),
                    since_ms: p.since_ms,
                    context: p.context.clone(),
                    recorded: p.recorded,
                })
                .collect(),
            events: r
                .control
                .events_of(&app)
                .map(|e| EventDto {
                    delivery: e.rec.delivery.clone(),
                    source: e.rec.source.clone(),
                    revision: e.rec.revision.clone(),
                    received_ms: e.rec.received_ms,
                    status: e.status.as_str().into(),
                    decided_ms: e.decided_ms,
                    related: e.related.clone(),
                })
                .collect(),
        });
    }
    for ((scope, name), def) in r.config.current() {
        let (kind, build, runtime, value, secret_id, version) = match &def.setting {
            Setting::Unset => ("unset", false, false, None, None, None),
            Setting::Set {
                flags,
                value: Value::Config(v),
            } => ("config", flags.build, flags.runtime, Some(v.clone()), None, None),
            Setting::Set {
                flags,
                value: Value::Secret(s),
            } => (
                "secret",
                flags.build,
                flags.runtime,
                None,
                Some(s.id.clone()),
                Some(s.version),
            ),
        };
        snap.definitions.push(DefinitionDto {
            scope: scope.slug(),
            name: name.to_string(),
            kind: kind.into(),
            build,
            runtime,
            value,
            secret_id,
            version,
            at_ms: def.at_ms,
        });
    }
    snap
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepLogDto {
    pub step: String,
    pub text: String,
    pub withheld: bool,
    pub truncated: bool,
    pub at_ms: u64,
}

/// The recorded, redacted step logs of one deployment (evidence of the run; not the application's logs).
pub fn step_logs(rt: &Path, deployment: &str) -> Result<Vec<StepLogDto>, String> {
    let id: DeploymentId = deployment.parse().map_err(|_| "not a deployment id".to_string())?;
    let r = SqliteJournal::read_only(&journal_dir(rt)).map_err(|e| e.0)?;
    Ok(r.logs
        .of(&id)
        .into_iter()
        .map(|l| StepLogDto {
            step: l.step.as_str().into(),
            text: l.text.clone(),
            withheld: l.withheld,
            truncated: l.truncated,
            at_ms: l.at_ms,
        })
        .collect())
}

// ------------------------------------------------------------------ external runtimes (read-only)

/// Canonical roots of the external runtimes attached in this process. Anything that would write to one of
/// them is refused in [`run_op_with`], whatever the caller says; the frontend is not what keeps them safe.
static ATTACHED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Is `rt` an attached external runtime, or inside or above one?
fn is_attached(rt: &Path) -> bool {
    let c = canonical(rt);
    ATTACHED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|a| {
        let a = canonical(a); // a remembered reference may not have existed when it was recorded
        c.starts_with(&a) || a.starts_with(&c)
    })
}

fn register_attached(p: &Path) {
    let mut attached = ATTACHED.lock().unwrap_or_else(|e| e.into_inner());
    if !attached.iter().any(|a| a == p) {
        attached.push(p.to_path_buf());
    }
}

fn unregister_attached(p: &Path) {
    ATTACHED.lock().unwrap_or_else(|e| e.into_inner()).retain(|a| a != p);
}

/// A deployment runtime somewhere else on this Mac (another build's, a preserved acceptance session), shown
/// read-only. The only way to get one is [`ExternalRuntime::attach`], which validates the path, and the only
/// thing it can do is [`ExternalRuntime::snapshot`], which only reads.
pub struct ExternalRuntime {
    root: PathBuf,
}

impl ExternalRuntime {
    /// `path` must be the runtime folder (the one that contains `deploy-journal/`), absolute, free of `..`,
    /// not Harpoon's own store, with a real `deploy-journal` directory holding a `deployments.sqlite3` or a
    /// legacy `journal.wal` that is a regular file (symbolic links are refused, not followed into).
    pub fn attach(path: &str, own: &Path) -> Result<ExternalRuntime, String> {
        use std::path::Component;
        let p = Path::new(path);
        if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err("An external runtime is given as an absolute path without '..'.".into());
        }
        if path.chars().any(|c| c.is_control()) {
            return Err("The path contains a control character.".into());
        }
        let root = std::fs::canonicalize(p).map_err(|e| format!("Cannot read {path}: {e}"))?;
        if !root.is_dir() {
            return Err(format!("{} is not a folder.", root.display()));
        }
        if root == canonical(own) {
            return Err("That is Harpoon's own deployment store; it is already shown.".into());
        }
        let jd = journal_dir(&root);
        match std::fs::symlink_metadata(&jd) {
            Ok(m) if m.file_type().is_dir() => {}
            Ok(m) if m.file_type().is_symlink() => {
                return Err("deploy-journal is a symbolic link; it is not followed.".into())
            }
            _ if root.file_name().is_some_and(|n| n == "deploy-journal") => {
                return Err("Choose the runtime folder that contains deploy-journal, not deploy-journal itself.".into())
            }
            _ => return Err(format!("No deploy-journal folder in {}.", root.display())),
        }
        let mut any = false;
        for f in [
            harpoon_deploy_journal::SQLITE_FILE,
            harpoon_deploy_journal::JOURNAL_FILE,
        ] {
            match std::fs::symlink_metadata(jd.join(f)) {
                Ok(m) if m.file_type().is_file() => any = true,
                Ok(_) => return Err(format!("{f} is not a regular file; it is not read.")),
                Err(_) => {}
            }
        }
        if !any {
            return Err(format!(
                "No deployment journal (deployments.sqlite3 or journal.wal) in {}.",
                jd.display()
            ));
        }
        register_attached(&root);
        Ok(ExternalRuntime { root })
    }

    /// Read-only by construction: this and [`build_snapshot`] use `SqliteJournal::read_only` and
    /// `writer_lock_status` only. A legacy journal is decoded where it is, never converted.
    pub fn snapshot(&self, now_ms: u64) -> EngineSnapshot {
        build_snapshot(&self.root, now_ms, "external")
    }
}

// ---- remembering which external runtimes are attached (a reference list, nothing else)

const EXTERNAL_FILE: &str = "external-runtimes.json";
const MAX_ATTACHED: usize = 32;
static EXTERNAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// What is written: the validated folder of each attached runtime, and nothing from inside it (no journal content, no ids).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRefs {
    version: u32,
    runtimes: Vec<StoredRef>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRef {
    path: String,
}

/// An attached runtime as the desktop shows it: read just now, or unavailable with the reason (its reference is kept either way).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEntry {
    pub path: String,
    pub snapshot: Option<EngineSnapshot>,
    pub error: Option<String>,
}

/// The list of attached runtimes, kept in the desktop's own configuration directory (`<app data>/external-runtimes.json`),
/// beside and never inside the deployment store or any external runtime. Restoring reads each runtime read-only through
/// [`ExternalRuntime`]; nothing here can write to, lock, repair or migrate an external journal.
pub struct ExternalStore {
    file: PathBuf,
}

impl ExternalStore {
    pub fn new(app_data: &Path) -> ExternalStore {
        ExternalStore {
            file: app_data.join(EXTERNAL_FILE),
        }
    }

    /// The remembered folders. A missing file is an empty list; a file that cannot be read is an error and is left exactly as it
    /// is (it is never replaced, so a damaged list cannot silently lose the references in it).
    fn read(&self) -> Result<Vec<String>, String> {
        let bytes = match std::fs::read(&self.file) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => {
                return Err(format!(
                    "The list of attached runtimes ({}) cannot be read: {e}. It was left as it is.",
                    self.file.display()
                ))
            }
        };
        let parsed: StoredRefs = serde_json::from_slice(&bytes).map_err(|e| {
            format!(
                "The list of attached runtimes ({}) is not valid: {e}. It was left as it is.",
                self.file.display()
            )
        })?;
        let bad = parsed.version != 1
            || parsed.runtimes.len() > MAX_ATTACHED
            || parsed.runtimes.iter().any(|r| {
                !Path::new(&r.path).is_absolute() || r.path.len() > 4096 || r.path.chars().any(|c| c.is_control())
            });
        if bad {
            return Err(format!(
                "The list of attached runtimes ({}) has an unsupported or invalid entry. It was left as it is.",
                self.file.display()
            ));
        }
        Ok(parsed.runtimes.into_iter().map(|r| r.path).collect())
    }

    /// Atomically: a private temporary file in the same directory, flushed, then renamed over the list.
    fn write(&self, paths: &[String]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let dir = self.file.parent().ok_or("no configuration directory")?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let body = serde_json::to_vec_pretty(&StoredRefs {
            version: 1,
            runtimes: paths.iter().map(|p| StoredRef { path: p.clone() }).collect(),
        })
        .map_err(|e| e.to_string())?;
        let tmp = dir.join(format!("{EXTERNAL_FILE}.tmp-{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
        f.write_all(&body)
            .and_then(|_| f.write_all(b"\n"))
            .and_then(|_| f.sync_all())
            .map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.file).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Cannot update {}: {e}", self.file.display())
        })
    }

    fn entry(&self, path: &str, own: &Path, now_ms: u64) -> ExternalEntry {
        match ExternalRuntime::attach(path, own) {
            Ok(rt) => ExternalEntry {
                path: path.to_string(),
                snapshot: Some(rt.snapshot(now_ms)),
                error: None,
            },
            Err(e) => ExternalEntry {
                path: path.to_string(),
                snapshot: None,
                error: Some(e),
            },
        }
    }

    /// Every remembered runtime, revalidated and read read-only now. One that is missing, invalid or unreadable is returned
    /// as unavailable with the reason; its reference is kept, and it is read again whenever the folder is back. Every
    /// remembered path is also registered as attached, so the mutation guard covers it even while it is unavailable.
    pub fn restore(&self, own: &Path, now_ms: u64) -> Result<Vec<ExternalEntry>, String> {
        let _one = EXTERNAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let paths = self.read()?;
        for p in &paths {
            register_attached(Path::new(p));
        }
        Ok(paths.iter().map(|p| self.entry(p, own, now_ms)).collect())
    }

    /// Validate `path` (read-only) and remember it. Refused, with nothing written, when it is not a runtime folder or the list is full.
    pub fn add(&self, path: &str, own: &Path, now_ms: u64) -> Result<ExternalEntry, String> {
        let _one = EXTERNAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut paths = self.read()?;
        let rt = ExternalRuntime::attach(path, own)?;
        let stored = rt.root.display().to_string();
        if !paths.contains(&stored) {
            if paths.len() >= MAX_ATTACHED {
                return Err(format!(
                    "At most {MAX_ATTACHED} runtimes can be attached; detach one first."
                ));
            }
            paths.push(stored.clone());
            self.write(&paths)?;
        }
        Ok(ExternalEntry {
            path: stored,
            snapshot: Some(rt.snapshot(now_ms)),
            error: None,
        })
    }

    /// Detach: forget the reference. Only the entry in the list is removed; the folder and everything in it are not touched,
    /// and the folder does not need to exist. Returns whether there was such a reference.
    pub fn remove(&self, path: &str) -> Result<bool, String> {
        let _one = EXTERNAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut paths = self.read()?;
        let canon = canonical(Path::new(path)).display().to_string();
        let before = paths.len();
        paths.retain(|p| p != path && *p != canon);
        let removed = before - paths.len();
        if removed > 0 {
            self.write(&paths)?;
            unregister_attached(Path::new(path));
            unregister_attached(Path::new(&canon));
        }
        Ok(removed > 0)
    }
}

// ------------------------------------------------------------------ operations

// An operation names no runtime: it always acts on the desktop's own store. A field that tries to name one
// (or anything else unknown) is an error, not something quietly ignored.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
#[allow(clippy::large_enum_variant)] // short-lived command argument; boxing would only obscure the JSON contract
pub enum DeployOp {
    #[serde(rename_all = "camelCase")]
    AddRepo {
        name: String,
        url: String,
        app: Option<String>,
        project: Option<String>,
        environment: Option<String>,
        branch: Option<String>,
        filters: Vec<String>,
        container_port: u16,
        dockerfile: Option<String>,
        context: Option<String>,
        target: Option<String>,
        health_path: Option<String>,
        username: Option<String>,
        known_host: Option<String>,
        webhook: Option<String>,
    },
    /// Change an existing registration. Only the fields given are changed; the name, application and credential
    /// settings never are. `expect_registered_ms` is the version the person was looking at.
    #[serde(rename_all = "camelCase")]
    UpdateRepo {
        name: String,
        expect_registered_ms: u64,
        url: Option<String>,
        branch: Option<String>,
        dockerfile: Option<String>,
        container_port: Option<u16>,
        health_path: Option<String>,
        #[serde(default)]
        confirm_change: bool,
    },
    #[serde(rename_all = "camelCase")]
    RepoSecret {
        name: String,
        kind: String,
        value: String,
    },
    #[serde(rename_all = "camelCase")]
    SetConfig {
        scope: String,
        name: String,
        value: String,
        build: bool,
        runtime: bool,
    },
    #[serde(rename_all = "camelCase")]
    SetSecret {
        scope: String,
        name: String,
        value: String,
        build: bool,
        runtime: bool,
    },
    #[serde(rename_all = "camelCase")]
    Unset {
        scope: String,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    Deploy {
        repo: String,
        branch: Option<String>,
        tag: Option<String>,
        commit: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    RunQueue {
        repo: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Poll {
        repo: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Rollback {
        app: String,
        release: String,
    },
    #[serde(rename_all = "camelCase")]
    Restore {
        deployment: String,
    },
    // a struct variant (not a unit variant) so that deny_unknown_fields applies to it too
    Recover {},
    #[serde(rename_all = "camelCase")]
    Resume {
        app: String,
        mode: String,
        revision: Option<String>,
    },
}

/// What the person confirmed in the dialog for ONE operation. It is a separate argument from the operation, so an
/// operation payload can never carry it and an operation that changes no Docker state ignores it. Absent means
/// "not confirmed": the engine then refuses a Docker-changing command at its own explicit opt-in.
///
/// * `live_changes`: the person confirmed that Harpoon may change real Docker state for this operation. Only this
///   adds the engine's `--authorize-live-docker-changes` argument.
/// * `cutover`: for a replacement only, the person separately confirmed that the running release is stopped to
///   switch (the application is down until the new release passes its check). Only this adds `--confirm-cutover`;
///   without it an existing application is only prepared (built and checked), never switched. It is refused unless
///   `live_changes` is also true.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Confirmation {
    #[serde(default)]
    pub live_changes: bool,
    #[serde(default)]
    pub cutover: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpResult {
    pub ok: bool,
    pub exit_code: i32,
    /// The engine's own first line of explanation (its refusal reason, or what it did).
    pub summary: String,
    /// The engine's full output. Secret values never appear in it.
    pub detail: String,
    /// The shipping gate refused it (the exact reason is in `summary`).
    pub refused_by_gate: bool,
    /// What the person can do about a failure, in plain words (never an exit code). `None` on success.
    pub hint: Option<String>,
}

fn opt(a: &mut Vec<String>, flag: &str, v: &Option<String>) {
    if let Some(v) = v.as_ref().filter(|v| !v.is_empty()) {
        a.push(format!("--{flag}"));
        a.push(v.clone());
    }
}

/// The arguments and the standard input for an operation. The secret value, if any, is the input.
fn plan(op: &DeployOp) -> (Vec<String>, Vec<u8>) {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match op {
        DeployOp::AddRepo {
            name,
            url,
            app,
            project,
            environment,
            branch,
            filters,
            container_port,
            dockerfile,
            context,
            target,
            health_path,
            username,
            known_host,
            webhook,
        } => {
            let mut a = s(&[
                "repo-add",
                "--name",
                name,
                "--url",
                url,
                "--container-port",
                &container_port.to_string(),
            ]);
            opt(&mut a, "app", app);
            opt(&mut a, "project", project);
            opt(&mut a, "environment", environment);
            opt(&mut a, "branch", branch);
            for f in filters {
                a.push("--filter".into());
                a.push(f.clone());
            }
            opt(&mut a, "dockerfile", dockerfile);
            opt(&mut a, "context", context);
            opt(&mut a, "target", target);
            opt(&mut a, "health-path", health_path);
            opt(&mut a, "username", username);
            opt(&mut a, "known-host", known_host);
            opt(&mut a, "webhook", webhook);
            // the desktop only ever creates: an existing name is refused, never silently replaced
            a.push("--new".into());
            (a, vec![])
        }
        DeployOp::UpdateRepo {
            name,
            expect_registered_ms,
            url,
            branch,
            dockerfile,
            container_port,
            health_path,
            confirm_change,
        } => {
            let mut a = s(&[
                "repo-update",
                "--name",
                name,
                "--expect-registered-ms",
                &expect_registered_ms.to_string(),
            ]);
            opt(&mut a, "url", url);
            opt(&mut a, "branch", branch);
            opt(&mut a, "dockerfile", dockerfile);
            opt(&mut a, "health-path", health_path);
            if let Some(p) = container_port {
                a.push("--container-port".into());
                a.push(p.to_string());
            }
            if *confirm_change {
                a.push("--confirm-change".into());
            }
            (a, vec![])
        }
        DeployOp::RepoSecret { name, kind, value } => (
            s(&["repo-secret", "--name", name, "--kind", kind]),
            value.clone().into_bytes(),
        ),
        DeployOp::SetConfig {
            scope,
            name,
            value,
            build,
            runtime,
        } => {
            let mut a = s(&["config-set", "--scope", scope, "--name", name, "--value", value]);
            if *build {
                a.push("--build".into());
            }
            if *runtime {
                a.push("--runtime".into());
            }
            (a, vec![])
        }
        DeployOp::SetSecret {
            scope,
            name,
            value,
            build,
            runtime,
        } => {
            let mut a = s(&["secret-set", "--scope", scope, "--name", name]);
            if *build {
                a.push("--build".into());
            }
            if *runtime {
                a.push("--runtime".into());
            }
            (a, value.clone().into_bytes())
        }
        DeployOp::Unset { scope, name } => (s(&["config-unset", "--scope", scope, "--name", name]), vec![]),
        DeployOp::Deploy {
            repo,
            branch,
            tag,
            commit,
        } => {
            let mut a = s(&["git-deploy", "--repo", repo]);
            opt(&mut a, "branch", branch);
            opt(&mut a, "tag", tag);
            opt(&mut a, "commit", commit);
            (a, vec![])
        }
        DeployOp::RunQueue { repo } => {
            let mut a = s(&["git-run"]);
            opt(&mut a, "repo", repo);
            (a, vec![])
        }
        // Poll and queue only: finds new commits and queues events, never deploys.
        DeployOp::Poll { repo } => {
            let mut a = s(&["watch", "--once", "--queue-only"]);
            opt(&mut a, "repo", repo);
            (a, vec![])
        }
        DeployOp::Rollback { app, release } => (
            s(&["rollback", "--app", app, "--to", release, "--confirm-rollback"]),
            vec![],
        ),
        DeployOp::Restore { deployment } => (s(&["restore", "--deployment", deployment, "--apply"]), vec![]),
        DeployOp::Recover {} => (s(&["recover"]), vec![]),
        DeployOp::Resume { app, mode, revision } => {
            let mut a = s(&["resume", "--app", app]);
            match mode.as_str() {
                "dismiss" => a.push("--dismiss".into()),
                "newest" => a.push("--newest".into()),
                _ => {
                    a.push("--revision".into());
                    a.push(revision.clone().unwrap_or_default());
                }
            }
            (a, vec![])
        }
    }
}

// ------------------------------------------------------------------ runtime configuration (E.1)

/// What a live engine command needs to reach the Harpoon runtime. Nothing here is inferred from the
/// environment, a Docker context or a default: every value comes from the runtime's own state file and
/// is checked to still be true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub docker_socket: PathBuf,
    pub mgmt_socket: PathBuf,
    /// The directory in which `harpoon/bin/docker-buildx` is looked up (the engine probes exactly that path).
    pub resources_dir: PathBuf,
    /// The Harpoon runtime process that owns the published-port listeners.
    pub forwarder_pid: u32,
    pub forwarder_command: String,
    /// The guest address the engine discovered and published (`forwardTarget`); `None` for an engine that does not
    /// publish one. Only an address with `source: "discovered"` is carried; the CLI proves it is the VM.
    pub forward_address: Option<String>,
}

/// The management socket is a fixed path of the Harpoon runtime (`HarpoonPaths.mgmtSocketPath`); it is
/// not recorded in `runtime.json`, so it is a constant here and must still be a live socket.
pub const MGMT_SOCKET: &str = "/tmp/harpoon-mgmt.sock";
/// The program name the engine's listener check expects for the runtime process.
const RUNTIME_COMMAND: &str = "harpoon";

fn is_socket(p: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::symlink_metadata(p)
        .map(|m| m.file_type().is_socket())
        .unwrap_or(false)
}

/// Read and verify the runtime's state. `support` is Harpoon's Application Support directory;
/// `command_of(pid)` is the running program name of a process, or `None` when it is not running.
pub fn discover_runtime(
    support: &Path,
    mgmt_socket: &Path,
    resources: &Path,
    command_of: &dyn Fn(u32) -> Option<String>,
) -> Result<RuntimeInfo, String> {
    let file = support.join("runtime.json");
    let text = std::fs::read_to_string(&file).map_err(|_| {
        format!("Harpoon's runtime state ({}) is missing, so the runtime is not running or not known. Start Harpoon and try again.", file.display())
    })?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Harpoon's runtime state is unreadable ({e}); nothing was assumed."))?;
    let socket = v
        .get("socketPath")
        .and_then(|x| x.as_str())
        .filter(|s| s.starts_with('/'))
        .ok_or("Harpoon's runtime state has no absolute Docker socket path.")?;
    let pid = v
        .get("pid")
        .and_then(|x| x.as_u64())
        .filter(|p| *p > 1 && *p <= u32::MAX as u64)
        .ok_or("Harpoon's runtime state has no valid process id.")? as u32;
    // the pid file is written by the same process; a disagreement means one of them is stale
    let pidfile = std::fs::read_to_string(support.join("runtime.pid"))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok());
    if pidfile != Some(pid) {
        return Err("Harpoon's runtime state is stale: its two process records disagree.".into());
    }
    match command_of(pid) {
        Some(c) if c == RUNTIME_COMMAND => {}
        Some(c) => {
            return Err(format!(
                "Harpoon's runtime state is stale: process {pid} is `{c}`, not the Harpoon runtime."
            ))
        }
        None => {
            return Err(format!(
                "Harpoon's runtime state is stale: process {pid} is not running."
            ))
        }
    }
    let docker_socket = PathBuf::from(socket);
    if !is_socket(&docker_socket) {
        return Err(format!("Harpoon's Docker socket {socket} is not available."));
    }
    let mgmt_socket = mgmt_socket.to_path_buf();
    if !is_socket(&mgmt_socket) {
        return Err(format!(
            "Harpoon's management socket {} is not available, so guest free space cannot be measured.",
            mgmt_socket.display()
        ));
    }
    let buildx = resources.join(harpoon_deploy::buildx::BUNDLED_RELATIVE);
    let meta = std::fs::symlink_metadata(&buildx)
        .map_err(|_| format!("the bundled Buildx is missing at {}.", buildx.display()))?;
    if !meta.file_type().is_file() {
        return Err(format!(
            "the bundled Buildx at {} is not a regular file.",
            buildx.display()
        ));
    }
    Ok(RuntimeInfo {
        docker_socket,
        mgmt_socket,
        resources_dir: resources.to_path_buf(),
        forwarder_pid: pid,
        forwarder_command: RUNTIME_COMMAND.into(),
        forward_address: forward_address(&v),
    })
}

/// The engine's published forwarding address: a discovered, private, dotted IPv4 address, or nothing.
/// A fallback or malformed value is dropped here, so the strict marker-only behavior applies.
fn forward_address(v: &serde_json::Value) -> Option<String> {
    let t = v.get("forwardTarget")?;
    if t.get("source")?.as_str()? != "discovered" {
        return None;
    }
    let a = t.get("address")?.as_str()?;
    let ip: std::net::Ipv4Addr = a.parse().ok()?;
    (ip.is_private() && ip.to_string() == a).then(|| a.to_string())
}

/// The running program name of a process (`ps`), for the stale-state check.
pub fn process_command(pid: u32) -> Option<String> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&out.stdout);
    let name = t.trim().rsplit('/').next().unwrap_or("").to_string();
    (out.status.success() && !name.is_empty()).then_some(name)
}

/// Which operations reach Docker (and so need the runtime's coordinates). Polling only queues; the other
/// operations touch the journal and the Keychain only.
fn needs_runtime(op: &DeployOp) -> bool {
    matches!(
        op,
        DeployOp::Deploy { .. } | DeployOp::RunQueue { .. } | DeployOp::Rollback { .. } | DeployOp::Restore { .. }
    )
}

/// Whether the gate lets this operation through to Docker at all.
fn gate_open_for(op: &DeployOp, gate: &Gate) -> bool {
    match op {
        DeployOp::Deploy { .. } | DeployOp::RunQueue { .. } => gate.live_deploy_enabled,
        DeployOp::Rollback { .. } | DeployOp::Restore { .. } => gate.live_replacement_enabled,
        _ => true,
    }
}

/// Arguments that hand the runtime's coordinates to a Docker-touching command. The live-change
/// authorization is deliberately NOT among them: that opt-in is the user's explicit act and is never
/// supplied implicitly.
pub fn runtime_args(r: &RuntimeInfo) -> Vec<String> {
    let mut args = vec![
        "--docker-socket".into(),
        r.docker_socket.display().to_string(),
        "--mgmt-socket".into(),
        r.mgmt_socket.display().to_string(),
        "--resources-dir".into(),
        r.resources_dir.display().to_string(),
        "--forwarder-pid".into(),
        r.forwarder_pid.to_string(),
        "--forwarder-command".into(),
        r.forwarder_command.clone(),
    ];
    if let Some(a) = &r.forward_address {
        args.extend(["--forward-address".into(), a.clone()]);
    }
    args
}

/// A refusal produced here, before the engine is reached.
fn refusal(msg: String, hint: String) -> OpResult {
    OpResult {
        ok: false,
        exit_code: harpoon_deploy_cli::exit::REFUSED,
        summary: msg.clone(),
        detail: msg,
        refused_by_gate: false,
        hint: Some(hint),
    }
}

/// The engine arguments and standard input for an operation, or the refusal that stops it here.
///
/// The live-change authorization is added in exactly one place: here, and only when the person confirmed it
/// (`confirmation.live_changes`) for an operation that changes Docker state and whose runtime coordinates were
/// found. `--confirm-cutover` is added only for a Deploy the person confirmed as a cutover, and never without
/// the authorization.
pub fn build_args(
    op: &DeployOp,
    confirmation: &Confirmation,
    gate: &Gate,
    runtime: &dyn Fn() -> Result<RuntimeInfo, String>,
) -> Result<(Vec<String>, Vec<u8>), OpResult> {
    let (mut args, stdin) = plan(op);
    if confirmation.cutover && !confirmation.live_changes {
        return Err(refusal(
            "Refused: a cutover confirmation arrived without the confirmation to change Docker. Nothing was changed.".into(),
            "Confirm the deployment in the dialog again. Nothing was changed.".into(),
        ));
    }
    if confirmation.cutover && !matches!(op, DeployOp::Deploy { .. }) {
        return Err(refusal(
            "Refused: a cutover confirmation applies only to a deployment. Nothing was changed."
                .into(),
            "Nothing was changed.".into(),
        ));
    }
    if needs_runtime(op) {
        match runtime() {
            Ok(info) => {
                args.extend(runtime_args(&info));
                if confirmation.live_changes {
                    args.push(format!(
                        "--{}={}",
                        harpoon_deploy_cli::AUTH_FLAG,
                        harpoon_deploy_cli::AUTH_PHRASE
                    ));
                }
                if confirmation.cutover {
                    args.push("--confirm-cutover".into());
                }
            }
            Err(e) if gate_open_for(op, gate) => {
                let msg = format!("Refused before contacting Docker: {e}");
                return Err(refusal(
                    msg,
                    format!("Harpoon's runtime is not ready ({e}). Start Harpoon and wait until Docker shows as ready, then try again. Nothing was changed."),
                ));
            }
            Err(_) => {}
        }
    }
    Ok((args, stdin))
}

/// Plain words for a failed operation: what happened and what to do next, from the engine's exit code and its
/// own text. `None` for success. The engine's full output stays in `detail`.
pub fn actionable(op: &DeployOp, code: i32, stdout: &str, stderr: &str) -> Option<String> {
    use harpoon_deploy_cli::exit;
    if code == exit::OK {
        return None;
    }
    let first = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let nothing = "Nothing was changed.";
    Some(match code {
        c if c == exit::REFUSED && stderr.contains(harpoon_deploy_cli::AUTH_FLAG) => {
            format!("Harpoon needs your confirmation before it changes Docker, and none was given. Open the operation again and confirm it. {nothing}")
        }
        c if c == exit::REFUSED && stderr.contains("Gate U") => {
            "Running queued Git events is closed in this build because unattended execution has not been approved. Use Deploy to build a revision you choose; checking for new commits only queues them. Nothing was changed.".to_string()
        }
        c if c == exit::REFUSED && (stderr.contains("not enabled in this build") || stderr.contains("disabled in this build")) => {
            format!("This build of Harpoon does not allow that operation. {nothing}")
        }
        c if c == exit::REFUSED && stderr.contains("another Harpoon process holds") => {
            "Another Harpoon deployment command is still running and holds the deployment history. Wait for it to finish, then try again. Nothing was changed.".to_string()
        }
        c if c == exit::REFUSED => {
            format!("Harpoon refused this before changing anything: {first}")
        }
        c if c == exit::USAGE => {
            format!("Harpoon sent the engine a request it could not read (an internal error, not something you did): {first} Please report it. {nothing}")
        }
        c if c == exit::CONFIG => {
            format!("The configuration was rejected before anything was built or started: {first} Fix it in the repository or variable settings and try again.")
        }
        c if c == exit::JOURNAL => {
            format!("The deployment history could not be used: {first} Do not delete it; it is the record of what Harpoon created. {nothing}")
        }
        c if c == exit::RUNTIME => {
            format!("Harpoon could not reach Docker or its management channel: {first} Check that Harpoon shows Docker as ready and try again. {nothing}")
        }
        c if c == exit::FAILED => {
            let step = stdout
                .lines()
                .map(str::trim)
                .find(|l| l.starts_with("failure at ") || l.starts_with("nothing was deployed"))
                .unwrap_or(first);
            let verb = match op {
                DeployOp::Rollback { .. } => "The rollback",
                DeployOp::Restore { .. } => "The recovery",
                _ => "The deployment",
            };
            format!("{verb} did not succeed: {step} Open the step logs for the details. Harpoon retried nothing and removed nothing it did not create; the engine's output lists the next steps.")
        }
        _ => format!("The operation did not complete: {first}"),
    })
}

/// Test seam with no runtime: the shipping gate and a runtime that cannot be found.
#[cfg(test)]
pub fn run_op(rt: &Path, op: &DeployOp, keychain: &dyn SecretStore) -> OpResult {
    // a lock refusal caused only by a stray copy of the lock (see `tests::LOCK_WAIT`) is waited out; every other answer is returned as is
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let r = run_op_with(rt, op, &Confirmation::default(), keychain, &Gate::shipping(), &|| {
            Err("no runtime in this test".into())
        });
        if !r.ok && r.detail.contains("another Harpoon process holds") && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }
        return r;
    }
}

/// Run one operation through the engine's CLI library. The app passes `Gate::shipping()`. The store is
/// created (private) on first use. `keychain` is the real Keychain in the app and a fake in tests.
/// `runtime` reads and verifies the runtime's coordinates; it is consulted only for Docker-touching
/// operations. While the gate is closed for an operation, a runtime that cannot be found is not a
/// refusal of its own: the engine refuses at the gate first, with the gate's reason. Once the gate would
/// let the operation through, a missing or stale runtime refuses it here, before the engine is reached.
pub fn run_op_with(
    rt: &Path,
    op: &DeployOp,
    confirmation: &Confirmation,
    keychain: &dyn SecretStore,
    gate: &Gate,
    runtime: &dyn Fn() -> Result<RuntimeInfo, String>,
) -> OpResult {
    if is_attached(rt) {
        let msg = "Refused: that runtime is attached read-only. Operations act only on Harpoon's own deployment store."
            .to_string();
        return OpResult {
            ok: false,
            exit_code: harpoon_deploy_cli::exit::REFUSED,
            summary: msg.clone(),
            detail: msg,
            refused_by_gate: false,
            hint: Some(
                "Operations act only on Harpoon's own deployment store. Nothing was changed."
                    .into(),
            ),
        };
    }
    if let Err(e) = prepare(rt) {
        return OpResult {
            ok: false,
            exit_code: 1,
            summary: e.clone(),
            detail: e.clone(),
            refused_by_gate: false,
            hint: Some(format!("Harpoon could not prepare its deployment store: {e} Check the disk and the folder's permissions, then try again.")),
        };
    }
    let (mut args, stdin) = match build_args(op, confirmation, gate, runtime) {
        Ok(v) => v,
        Err(refusal) => return refusal,
    };
    args.push("--runtime-dir".into());
    args.push(rt.display().to_string());
    let mut input: &[u8] = &stdin;
    let mut svc = Services {
        secrets: keychain,
        stdin: &mut input,
        stdin_is_terminal: false,
        next: None,
    };
    let out = run_with(&args, gate, &mut svc);
    let text = if out.stderr.trim().is_empty() {
        out.stdout.clone()
    } else {
        format!("{}{}", out.stdout, out.stderr)
    };
    let first = |t: &str| {
        t.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string()
    };
    let summary = if out.code == 0 {
        first(&out.stdout)
    } else {
        first(&out.stderr).chars().take(400).collect::<String>()
    };
    OpResult {
        ok: out.code == 0,
        exit_code: out.code,
        refused_by_gate: out.code == harpoon_deploy_cli::exit::REFUSED
            && (out.stderr.contains("not enabled in this build") || out.stderr.contains("disabled in this build")),
        hint: actionable(op, out.code, &out.stdout, &out.stderr),
        summary,
        detail: text,
    }
}

// ------------------------------------------------------------------ Tauri commands

/// The desktop's data directory: the real app data directory, or the test namespace's own one (debug builds only), so a
/// namespaced desktop never opens, creates or routes from the real deployment store.
pub fn data_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    if let Some(ns) = crate::namespace::active()? {
        return Ok(ns.data_dir());
    }
    app.path().app_data_dir().map_err(|e| format!("no app data directory: {e}"))
}

fn app_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    data_root(app).map(|d| runtime_dir(&d))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The directory that contains `harpoon/bin/docker-buildx`: the app's Resources (the bundle maps
/// `bundle-resources/harpoon` to `harpoon`), or the source tree's `bundle-resources` in a debug build.
fn bundled_resources(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    if cfg!(debug_assertions) {
        return Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bundle-resources"));
    }
    app.path()
        .resource_dir()
        .map_err(|e| format!("no resource directory: {e}"))
}

static OPS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tauri::command]
pub async fn deploy_snapshot(app: tauri::AppHandle) -> Result<EngineSnapshot, String> {
    let dir = app_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || snapshot(&dir, now_ms()))
        .await
        .map_err(|e| format!("background task failed: {e}"))
}

fn app_data(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    data_root(app)
}

/// Every attached runtime (remembered across restarts), read read-only now. Takes nothing from the caller.
#[tauri::command]
pub async fn external_runtimes(app: tauri::AppHandle) -> Result<Vec<ExternalEntry>, String> {
    let (data, own) = (app_data(&app)?, app_dir(&app)?);
    tauri::async_runtime::spawn_blocking(move || ExternalStore::new(&data).restore(&own, now_ms()))
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

/// Validate a folder, read it read-only, and remember the reference.
#[tauri::command]
pub async fn external_runtime_add(app: tauri::AppHandle, path: String) -> Result<ExternalEntry, String> {
    let (data, own) = (app_data(&app)?, app_dir(&app)?);
    tauri::async_runtime::spawn_blocking(move || ExternalStore::new(&data).add(&path, &own, now_ms()))
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

/// Detach: forget the reference. The folder and its files are not touched.
#[tauri::command]
pub async fn external_runtime_remove(app: tauri::AppHandle, path: String) -> Result<bool, String> {
    let data = app_data(&app)?;
    tauri::async_runtime::spawn_blocking(move || ExternalStore::new(&data).remove(&path))
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

#[tauri::command]
pub async fn deploy_step_logs(app: tauri::AppHandle, deployment: String) -> Result<Vec<StepLogDto>, String> {
    let dir = app_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || step_logs(&dir, &deployment))
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

/// Where the engine's state, management socket and secrets live for this process: the production locations, or the
/// test namespace's (debug builds only). In a namespace the Keychain is replaced by a store that dies with the process,
/// so an isolated desktop cannot read or write the user's Keychain entries.
pub struct Environment {
    pub support: PathBuf,
    pub mgmt_socket: PathBuf,
    pub namespace: Option<crate::namespace::Namespace>,
}

pub fn environment(home: &Path) -> Result<Environment, String> {
    Ok(environment_for(crate::namespace::active()?, home))
}

/// The same, for a given namespace (kept apart so it can be tested without the process environment).
pub fn environment_for(namespace: Option<crate::namespace::Namespace>, home: &Path) -> Environment {
    match namespace {
        Some(ns) => Environment { support: ns.support_dir(), mgmt_socket: ns.mgmt_socket(), namespace: Some(ns) },
        None => Environment { support: home.join("Library/Application Support/Harpoon"), mgmt_socket: PathBuf::from(MGMT_SOCKET), namespace: None },
    }
}

/// In a namespace the runtime that `runtime.json` names must be the namespace's own: its Docker socket is the one under the
/// root. A state file that points anywhere else (the serving engine's socket, say) is refused, never followed.
pub fn confine_to_namespace(info: RuntimeInfo, ns: &crate::namespace::Namespace) -> Result<RuntimeInfo, String> {
    if info.docker_socket != ns.docker_socket() {
        return Err(format!(
            "the runtime state names the Docker socket {}, which is not the test namespace's ({}); nothing was assumed",
            info.docker_socket.display(),
            ns.docker_socket().display()
        ));
    }
    if info.mgmt_socket != ns.mgmt_socket() {
        return Err(format!("the management socket {} is not the test namespace's", info.mgmt_socket.display()));
    }
    Ok(info)
}

#[tauri::command]
pub async fn deploy_op(
    app: tauri::AppHandle,
    op: DeployOp,
    confirmation: Option<Confirmation>,
) -> Result<OpResult, String> {
    let dir = app_dir(&app)?;
    use tauri::Manager;
    let home = app.path().home_dir().map_err(|e| format!("no home directory: {e}"))?;
    let env = environment(&home)?;
    let resources = bundled_resources(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        // one engine command at a time from the desktop; the journal's own lock covers other processes
        let _one = OPS.lock().unwrap_or_else(|e| e.into_inner());
        let real_keychain;
        let namespace_secrets;
        let kc: &dyn SecretStore = if env.namespace.is_some() {
            namespace_secrets = harpoon_deploy::secrets::MemorySecretStore::new();
            &namespace_secrets
        } else {
            real_keychain = harpoon_deploy_keychain::Keychain::new();
            &real_keychain
        };
        run_op_with(&dir, &op, &confirmation.unwrap_or_default(), kc, &Gate::shipping(), &|| {
            let info = discover_runtime(&env.support, &env.mgmt_socket, &resources, &process_command)?;
            match &env.namespace {
                Some(ns) => confine_to_namespace(info, ns),
                None => Ok(info),
            }
        })
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))
}

// ------------------------------------------------------------------ host listeners (effective exposure)

/// One TCP listener on this Mac, as `lsof` reports it. This is what is actually bound on the host, as opposed to
/// what Docker reports for a container's published ports (a mapping inside Harpoon's VM).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostListenerDto {
    pub port: u16,
    /// As `lsof` prints it: `127.0.0.1:38000`, `*:38000`, `[::1]:38000`.
    pub address: String,
    /// `loopback` (reachable from this Mac only), `wildcard` (every interface) or `interface` (one non-loopback address).
    pub scope: &'static str,
    pub pid: u32,
    pub command: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostListenersDto {
    /// The check ran and `listeners` is complete. When `false`, `listeners` is empty and nothing may be concluded from it.
    pub verified: bool,
    pub error: Option<String>,
    pub checked_ms: u64,
    pub listeners: Vec<HostListenerDto>,
}

pub fn listener_scope(host: &str) -> &'static str {
    match host {
        "*" | "0.0.0.0" | "[::]" => "wildcard",
        "[::1]" | "localhost" => "loopback",
        h if h.starts_with("127.") => "loopback",
        _ => "interface",
    }
}

/// Every TCP listener on this Mac. Read-only: the engine's own `lsof` runner (fixed arguments, cleared
/// environment, 5 s limit). A failure is reported as unverified, never as "nothing is listening".
pub fn host_listeners(lsof: &Path, now_ms: u64) -> HostListenersDto {
    host_listeners_from(
        harpoon_deploy_docker::forwarder::listeners_in_range(lsof, 1, u16::MAX),
        now_ms,
    )
}

/// The same, from what `lsof` answered (kept apart so it can be tested without starting a process).
pub fn host_listeners_from(
    found: Result<Vec<harpoon_deploy_docker::forwarder::Listener>, String>,
    now_ms: u64,
) -> HostListenersDto {
    match found {
        Ok(found) => HostListenersDto {
            verified: true,
            error: None,
            checked_ms: now_ms,
            listeners: found
                .into_iter()
                .filter_map(|l| {
                    Some(HostListenerDto {
                        port: l.port()?,
                        scope: listener_scope(l.host()),
                        address: l.address.clone(),
                        pid: l.pid,
                        command: l.command.clone(),
                    })
                })
                .collect(),
        },
        Err(e) => HostListenersDto {
            verified: false,
            error: Some(e),
            checked_ms: now_ms,
            listeners: vec![],
        },
    }
}

#[tauri::command]
pub async fn host_tcp_listeners() -> Result<HostListenersDto, String> {
    tauri::async_runtime::spawn_blocking(|| {
        host_listeners(
            Path::new(harpoon_deploy_docker::forwarder::LsofCheck::DEFAULT_PATH),
            now_ms(),
        )
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))
}

/// The path in the macOS folder picker's answer (`POSIX path of (choose folder …)`): one absolute path,
/// a trailing slash dropped. Anything else (empty, relative, control characters) is no answer.
pub fn parse_picked_folder(out: &str) -> Option<String> {
    let p = out.trim_end_matches(['\n', '\r']);
    let p = if p.len() > 1 { p.trim_end_matches('/') } else { p };
    (p.starts_with('/') && !p.chars().any(|c| c.is_control())).then(|| p.to_string())
}

/// The standard macOS folder picker with a fixed AppleScript (nothing from the app is interpolated into
/// it), the same mechanism the Compose import uses. `None` when the person cancels. The path is only
/// filled into the form; the engine validates it when the repository is registered.
#[tauri::command]
pub async fn deploy_pick_folder() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let out = std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "POSIX path of (choose folder with prompt \"Select the Git repository folder\")",
            ])
            .output()
            .map_err(|_| "Could not open the macOS folder picker".to_string())?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("-128") || err.contains("User canceled") {
                return Ok(None);
            }
            return Err("The folder picker did not complete".into());
        }
        parse_picked_folder(&String::from_utf8_lossy(&out.stdout))
            .map(Some)
            .ok_or_else(|| "The folder picker returned a path Harpoon cannot use".into())
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))?
}

#[cfg(test)]
mod tests;
