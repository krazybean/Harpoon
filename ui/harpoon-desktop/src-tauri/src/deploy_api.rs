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
    /// `missing` (nothing was ever recorded), `ok`, or `error` (the store is damaged or unreadable).
    pub store: String,
    pub store_error: Option<String>,
    /// A writer holds the journal right now (a deployment or another command is running).
    pub writer_held: bool,
    pub gate: GateDto,
    pub repos: Vec<RepoDto>,
    pub apps: Vec<AppDto>,
    pub definitions: Vec<DefinitionDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateDto {
    pub live_deploy: bool,
    pub live_replacement: bool,
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
    pub repo: Option<String>,
    pub deployments: Vec<DeploymentDto>,
    pub serving: Option<ServingDto>,
    pub pauses: Vec<PauseDto>,
    pub events: Vec<EventDto>,
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
    let jd = journal_dir(rt);
    let mut snap = EngineSnapshot {
        schema: SCHEMA,
        generated_ms: now_ms,
        runtime_dir: rt.display().to_string(),
        store: "missing".into(),
        store_error: None,
        writer_held: false,
        gate: GateDto {
            live_deploy: Gate::shipping().live_deploy_enabled,
            live_replacement: Gate::shipping().live_replacement_enabled,
            reason: GATE_REASON,
        },
        repos: vec![],
        apps: vec![],
        definitions: vec![],
    };
    if !jd.exists() {
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
    let mut app_ids: Vec<ApplicationId> = r.deployments.iter().map(|d| d.application_id.clone()).collect();
    app_ids.extend(r.git.repos.values().map(|x| x.app.clone()));
    app_ids.sort_by_key(|a| a.to_string());
    app_ids.dedup();
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
            repo: r.git.repo_of_app(&app).map(|x| x.name.clone()),
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

// ------------------------------------------------------------------ operations

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
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
    Recover,
    #[serde(rename_all = "camelCase")]
    Resume {
        app: String,
        mode: String,
        revision: Option<String>,
    },
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
        DeployOp::Recover => (s(&["recover"]), vec![]),
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
    })
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
    vec![
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
    ]
}

/// The engine arguments and standard input for an operation, or the refusal that stops it here.
pub fn build_args(
    op: &DeployOp,
    gate: &Gate,
    runtime: &dyn Fn() -> Result<RuntimeInfo, String>,
) -> Result<(Vec<String>, Vec<u8>), OpResult> {
    let (mut args, stdin) = plan(op);
    if needs_runtime(op) {
        match runtime() {
            Ok(info) => args.extend(runtime_args(&info)),
            Err(e) if gate_open_for(op, gate) => {
                let msg = format!("Refused before contacting Docker: {e}");
                return Err(OpResult {
                    ok: false,
                    exit_code: harpoon_deploy_cli::exit::REFUSED,
                    summary: msg.clone(),
                    detail: msg,
                    refused_by_gate: false,
                });
            }
            Err(_) => {}
        }
    }
    Ok((args, stdin))
}

/// Test seam with no runtime: the shipping gate and a runtime that cannot be found.
#[cfg(test)]
pub fn run_op(rt: &Path, op: &DeployOp, keychain: &dyn SecretStore) -> OpResult {
    run_op_with(rt, op, keychain, &Gate::shipping(), &|| {
        Err("no runtime in this test".into())
    })
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
    keychain: &dyn SecretStore,
    gate: &Gate,
    runtime: &dyn Fn() -> Result<RuntimeInfo, String>,
) -> OpResult {
    if let Err(e) = prepare(rt) {
        return OpResult {
            ok: false,
            exit_code: 1,
            summary: e.clone(),
            detail: e,
            refused_by_gate: false,
        };
    }
    let (mut args, stdin) = match build_args(op, gate, runtime) {
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
        summary,
        detail: text,
    }
}

// ------------------------------------------------------------------ Tauri commands

fn app_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .map(|d| runtime_dir(&d))
        .map_err(|e| format!("no app data directory: {e}"))
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

#[tauri::command]
pub async fn deploy_step_logs(app: tauri::AppHandle, deployment: String) -> Result<Vec<StepLogDto>, String> {
    let dir = app_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || step_logs(&dir, &deployment))
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

#[tauri::command]
pub async fn deploy_op(app: tauri::AppHandle, op: DeployOp) -> Result<OpResult, String> {
    let dir = app_dir(&app)?;
    use tauri::Manager;
    let support = app
        .path()
        .home_dir()
        .map(|h| h.join("Library/Application Support/Harpoon"))
        .map_err(|e| format!("no home directory: {e}"))?;
    let resources = bundled_resources(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        // one engine command at a time from the desktop; the journal's own lock covers other processes
        let _one = OPS.lock().unwrap_or_else(|e| e.into_inner());
        let kc = harpoon_deploy_keychain::Keychain::new();
        run_op_with(&dir, &op, &kc, &Gate::shipping(), &|| {
            discover_runtime(&support, Path::new(MGMT_SOCKET), &resources, &process_command)
        })
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))
}

#[cfg(test)]
mod tests;
