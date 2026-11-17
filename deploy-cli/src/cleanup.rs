//! Release-aware cleanup of Harpoon-owned Docker artifacts.
//!
//! What a successful replacement leaves behind (the stopped containers, port leases and images of releases that no
//! longer serve) and what a failed one leaves (its retained image) is removed here, by the ids the journal recorded.
//! Everything goes through the engine's ledger: an object is removed only when Docker still shows the recorded id with
//! all three Harpoon ownership labels, and the result is journaled. Protected, never touched:
//!
//! * the release each application serves now, and the deployment that owns its container;
//! * the image id the serving release pins, even when an older deployment recorded the same id;
//! * a deployment that has not finished (a writer holding the journal proves nothing else is deploying);
//! * a prepared-but-unconfirmed candidate (cancelled), which the person may still confirm;
//! * volumes, networks and anything Harpoon did not create: this module has no code path that names them.
//!
//! Removing a superseded release's image makes it ineligible for the old image-based rollback (the ledger says
//! so); restoring it is a rebuild of its recorded commit.

use crate::args::Parsed;
use crate::commands::{describe, docker_backend, unfinished};
use crate::store::{self, Kind, Runtime};
use crate::{exit, Fail};
use harpoon_deploy::backend::ResourceOps;
use harpoon_deploy::engine::{end_state, execute_actions};
use harpoon_deploy::ids::{ApplicationId, DeploymentId};
use harpoon_deploy::ledger::ResourceKey;
use harpoon_deploy::ledger::{
    plan as ledger_plan, Action, Disposition, EndState, EntryState, Observed, ResourceKind,
};
use harpoon_deploy::serving::serving_release;
use harpoon_deploy::state::DeploymentStatus;
use harpoon_deploy::time::SystemClock;
use harpoon_deploy_docker::backend::ImageVerdict;
use harpoon_deploy_docker::guest::MgmtDfProbe;
use harpoon_deploy_docker::json::{n, obj, s as js, Json};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Tidied {
    pub containers: usize,
    pub leases: usize,
    pub images: usize,
}

/// Plan (`apply == false`: nothing is changed, the journal is not written) or carry out the cleanup. A refusal
/// leaves everything as it was.
pub(crate) fn run(
    p: &Parsed,
    rt: &Runtime,
    socket: &Path,
    apply: bool,
    out: &mut String,
) -> Result<Tidied, Fail> {
    let mut backend = docker_backend(p, rt, socket)?;
    let (mut writer, restored) = if apply {
        let (j, r) = store::open_writer(rt, Kind::Docker)?;
        (Some(j), r)
    } else {
        (
            None,
            store::read_restored(rt)?.ok_or_else(|| {
                Fail::refused("there is no journal; nothing is recorded to clean up")
            })?,
        )
    };
    let deps = &restored.deployments;
    if let Some(first) = unfinished(deps).first() {
        return Err(Fail::refused(format!(
            "{first}\ncleanup never runs while a deployment is unfinished; nothing was changed"
        )));
    }

    // protected: what each application serves (and the deployment that owns its container), or, when nothing is
    // proven to serve, every deployment of that application
    let apps: BTreeSet<ApplicationId> = deps.iter().map(|d| d.application_id.clone()).collect();
    let mut protected: BTreeSet<DeploymentId> = BTreeSet::new();
    let mut protected_images: BTreeSet<String> = BTreeSet::new();
    for app in &apps {
        match serving_release(&restored.ledger, deps, app) {
            Some(s) => {
                protected.insert(s.deployment.clone());
                protected.insert(s.container_owner.clone());
                if let Some(c) = &s.config {
                    protected_images.insert(c.image_id.clone());
                }
            }
            None => protected.extend(
                deps.iter()
                    .filter(|d| d.application_id == *app)
                    .map(|d| d.id.clone()),
            ),
        }
    }
    // in scope: succeeded releases that no longer serve (treated as ended without success for the planner), and
    // failed or interrupted deployments. Cancelled (prepared) candidates are left for the person to confirm or remove.
    let scope: Vec<(DeploymentId, EndState)> = deps
        .iter()
        .filter(|d| !protected.contains(&d.id))
        .filter_map(|d| match d.status {
            DeploymentStatus::Succeeded => Some((d.id.clone(), EndState::NotSucceeded)),
            // one whose cutover began is left for `status`/`restore`: the application may be mid-recovery
            DeploymentStatus::Failed | DeploymentStatus::Interrupted
                if d.replaces.is_none() && d.recovery.is_none() =>
            {
                end_state(d.status).map(|e| (d.id.clone(), e))
            }
            _ => None,
        })
        .collect();
    let in_scope: BTreeSet<&DeploymentId> = scope.iter().map(|(d, _)| d).collect();

    // Every decision is also written as one `item: {json}` line that names the application, deployment and release it
    // belongs to, so a reader never has to guess ownership from an id. Ownership comes only from the journal: the
    // registered repository of the application, else the removal record that disconnected it, else nothing (unassociated).
    let describe_owner = |app: &ApplicationId| -> (Option<String>, Option<String>) {
        if let Some(r) = restored.git.repo_of_app(app) {
            (
                Some(r.name.clone()),
                Some(r.environment.clone().unwrap_or_else(|| "production".into())),
            )
        } else if let Some(g) = restored.git.disconnected(app) {
            (Some(g.name.clone()), None)
        } else {
            (None, None)
        }
    };
    let item = |out: &mut String,
                app: &ApplicationId,
                dep: &DeploymentId,
                kind: &str,
                name: &str,
                status: &str,
                reason: &str,
                detail: &str,
                physical: Option<bool>| {
        let (app_name, env) = describe_owner(app);
        let d = deps.iter().find(|d| d.id == *dep);
        let j = obj(vec![
            ("app", js(app.to_string())),
            ("appName", app_name.map_or(Json::Null, js)),
            ("environment", env.map_or(Json::Null, js)),
            ("deployment", js(dep.to_string())),
            (
                "createdMs",
                d.map_or(Json::Null, |d| n(d.created_ms as i64)),
            ),
            (
                "status",
                js(d.map_or("unknown".to_string(), |d| format!("{}", d.status))),
            ),
            (
                "commit",
                restored.git.source_of(dep).map_or(Json::Null, |sr| {
                    js(sr.commit.chars().take(7).collect::<String>())
                }),
            ),
            ("kind", js(kind)),
            ("name", js(name)),
            ("verdict", js(status)),
            ("reason", js(reason)),
            ("detail", js(detail)),
            ("physical", physical.map_or(Json::Null, Json::Bool)),
        ]);
        let _ = writeln!(out, "item: {}", j.to_json());
    };
    let _ = writeln!(
        out,
        "cleanup of Harpoon-owned artifacts — {}",
        if apply {
            "APPLYING"
        } else {
            "dry run: nothing will be changed"
        }
    );
    for id in &protected {
        let _ = writeln!(out, "protected: deployment {id} (serves now, owns the serving container, or nothing is proven to serve)");
    }
    for id in &protected {
        for e in restored.ledger.entries().iter().filter(|e| {
            e.deployment == *id
                && !e.is_released()
                && matches!(
                    e.key.kind,
                    ResourceKind::Container | ResourceKind::Image | ResourceKind::PortLease
                )
        }) {
            let proven = serving_release(&restored.ledger, deps, &e.app).is_some();
            let (reason, detail) = if proven {
                (
                    "serving-release",
                    "the release serving this application now, or the container that serves it",
                )
            } else {
                ("nothing-proven-serving", "no release of this application is recorded as serving, so nothing of it is touched")
            };
            item(
                out,
                &e.app,
                id,
                e.key.kind.as_str(),
                &e.key.name,
                "protected",
                reason,
                detail,
                None,
            );
        }
    }
    let observed = backend.list_resources().map_err(|e| {
        Fail::runtime(format!(
            "cannot list the runtime: {}\nNothing was assumed absent and nothing was changed.",
            e.message
        ))
    })?;
    let mut ledger = restored.ledger.clone();
    let mut tidied = Tidied::default();
    // Docker's image sizes share layers and are not additive, so what was freed is what the guest filesystem reports.
    let guest_free = || guest_free(p, socket);
    let free_before = guest_free();

    // 1. containers and port leases of those deployments
    let actions: Vec<Action> = ledger_plan(&ledger, &observed, &scope)
        .into_iter()
        .filter(|a| !matches!(a, Action::ReportOnly { .. } | Action::Refuse { .. }))
        .collect();
    for a in &actions {
        let _ = writeln!(out, "{}{}", if apply { "" } else { "would: " }, describe(a));
    }
    for a in &actions {
        match a {
            Action::Remove {
                entry: (dep, key),
                observed,
                ..
            } => {
                let app = ledger.find(dep, key).map(|e| e.app.clone());
                let failed = deps
                    .iter()
                    .any(|d| d.id == *dep && d.status != DeploymentStatus::Succeeded);
                if let Some(app) = app {
                    item(out, &app, dep, observed.kind.as_str(), &key.name, "ready", if failed { "failed-attempt" } else { "superseded" }, "Docker still holds this object with the recorded id and all three Harpoon labels", Some(true));
                }
            }
            Action::MarkAbsent { entry: (dep, key) } => {
                if let Some(app) = ledger.find(dep, key).map(|e| e.app.clone()) {
                    item(
                        out,
                        &app,
                        dep,
                        key.kind.as_str(),
                        &key.name,
                        "reconcile",
                        "already-absent",
                        "Docker no longer has this object; only the journal still lists it",
                        Some(false),
                    );
                }
            }
            Action::ReleaseLease { entry: (dep, key) } => {
                if let Some(app) = ledger.find(dep, key).map(|e| e.app.clone()) {
                    item(out, &app, dep, "port-lease", &key.name, "reconcile", "stale-port-lease", "a host port reserved for a release that no longer serves; a journal entry only, no Docker object", None);
                }
            }
            _ => {}
        }
    }
    // what the container and lease pass will have released, so a dry run judges the images as the apply will (after it)
    let will_release: Vec<(DeploymentId, ResourceKey)> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Remove { entry, .. }
            | Action::MarkAbsent { entry }
            | Action::ReleaseLease { entry } => Some(entry.clone()),
            _ => None,
        })
        .collect();
    tidied.containers = actions.iter().filter(|a| matches!(a, Action::Remove { observed, .. } if observed.kind == ResourceKind::Container)).count();
    tidied.leases = actions
        .iter()
        .filter(|a| matches!(a, Action::ReleaseLease { .. }))
        .count();
    if let Some(j) = writer.as_mut() {
        execute_actions(&actions, &mut ledger, j, &mut backend, &SystemClock).map_err(|e| {
            Fail::failed(format!(
                "journal write failed during cleanup: {e}; run `recover`, then `status`, and retry"
            ))
        })?;
    }

    // 2. retained images, one at a time, each re-verified against Docker
    let candidates: Vec<(
        DeploymentId,
        harpoon_deploy::ledger::ResourceKey,
        String,
        ApplicationId,
    )> = ledger
        .entries()
        .iter()
        .filter(|e| {
            e.key.kind == ResourceKind::Image
                && e.disposition == Disposition::Retain
                && !e.is_released()
                && in_scope.contains(&e.deployment)
        })
        .filter(|e| !matches!(e.state, EntryState::Refused(_)))
        .filter_map(|e| {
            e.id.clone()
                .map(|id| (e.deployment.clone(), e.key.clone(), id, e.app.clone()))
        })
        .collect();
    for (dep, key, id, app) in candidates {
        let skip_as = |out: &mut String, verdict: &str, reason: &str, why: &str| {
            let _ = writeln!(out, "kept: image {id} of {dep}: {why}");
            item(
                out,
                &app,
                &dep,
                "image",
                &key.name,
                verdict,
                reason,
                why,
                Some(true),
            );
        };
        let skip = |out: &mut String, why: &str| skip_as(out, "review", "needs-review", why);
        if protected_images.contains(&id) {
            skip_as(
                out,
                "protected",
                "serving-pins-image",
                "the serving release pins this image",
            );
            continue;
        }
        let open: Vec<String> = ledger
            .entries()
            .iter()
            .filter(|o| {
                o.deployment == dep
                    && o.key != key
                    && !o.is_released()
                    && !will_release.contains(&(o.deployment.clone(), o.key.clone()))
            })
            .map(|o| format!("{} {}", o.key.kind.as_str(), o.key.name))
            .collect();
        if !open.is_empty() {
            skip(
                out,
                &format!(
                    "the deployment still has unreleased resources ({})",
                    open.join(", ")
                ),
            );
            continue;
        }
        if ledger.entries().iter().any(|o| {
            o.deployment != dep
                && o.key.kind == ResourceKind::Image
                && !o.is_released()
                && o.id.as_deref() == Some(&id)
                && protected.contains(&o.deployment)
        }) {
            skip_as(
                out,
                "protected",
                "shared-with-protected",
                "a protected deployment records the same image",
            );
            continue;
        }
        let action = match backend.assess_retained_image(&id, &key.name, &app, &dep) {
            Err(e) => {
                skip(out, &format!("Docker could not be asked ({})", e.message));
                continue;
            }
            Ok(ImageVerdict::NotExclusive(why)) | Ok(ImageVerdict::Ownership(why)) => {
                skip(out, &why);
                continue;
            }
            Ok(ImageVerdict::Absent) => {
                item(
                    out,
                    &app,
                    &dep,
                    "image",
                    &key.name,
                    "reconcile",
                    "already-absent",
                    "Docker no longer has this image; only the journal still lists it",
                    Some(false),
                );
                Action::MarkAbsent {
                    entry: (dep.clone(), key.clone()),
                }
            }
            Ok(ImageVerdict::Eligible) => {
                let failed = deps
                    .iter()
                    .any(|d| d.id == dep && d.status != DeploymentStatus::Succeeded);
                item(out, &app, &dep, "image", &key.name, "ready", if failed { "failed-attempt" } else { "superseded" }, "Docker holds exactly this image id with all three Harpoon labels, one tag, and no container uses it", Some(true));
                Action::Remove {
                    entry: (dep.clone(), key.clone()),
                    observed: Observed {
                        kind: ResourceKind::Image,
                        name: key.name.clone(),
                        id: id.clone(),
                        labels: Vec::new(),
                    },
                    adopt: false,
                }
            }
        };
        let _ = writeln!(
            out,
            "{}{}",
            if apply { "" } else { "would: " },
            describe(&action)
        );
        if let Some(j) = writer.as_mut() {
            let mut ops = backend.retained_image_ops();
            if let Err(e) = execute_actions(
                std::slice::from_ref(&action),
                &mut ledger,
                j,
                &mut ops,
                &SystemClock,
            ) {
                return Err(Fail::failed(format!("journal write failed during cleanup: {e}; run `recover`, then `status`, and retry")));
            }
            match ledger.find(&dep, &key).map(|e| &e.state) {
                Some(EntryState::Released(_)) => tidied.images += 1,
                _ => skip(out, "Docker did not remove it; it stays recorded and is retried by the next cleanup"),
            }
        } else if matches!(action, Action::Remove { .. }) {
            tidied.images += 1;
        }
    }
    let verb = if apply { "removed" } else { "would remove" };
    let _ = writeln!(out, "cleanup: {verb} {} container(s), release {} port lease(s), {verb} {} image(s). Volumes, networks, build cache and other applications' artifacts are never touched.", tidied.containers, tidied.leases, tidied.images);
    let _ = writeln!(
        out,
        "reclaimed-containers: {}\nreclaimed-images: {}",
        tidied.containers, tidied.images
    );
    if apply {
        match (free_before, guest_free()) {
            (Some(b), Some(a)) => {
                let _ = writeln!(
                    out,
                    "guest-free-before-bytes: {b}\nguest-free-after-bytes: {a}"
                );
            }
            _ => {
                let _ = writeln!(
                    out,
                    "guest-free: not measurable (the management channel did not answer)"
                );
            }
        }
    }
    Ok(tidied)
}

/// Free bytes on the guest filesystem through the management channel, when the caller gave one and it answers.
fn guest_free(p: &Parsed, socket: &Path) -> Option<u64> {
    let mgmt = p.value("mgmt-socket")?;
    MgmtDfProbe::new(socket, mgmt, Duration::from_secs(10))
        .measure()
        .ok()
        .map(|(free, _)| free)
}

/// Why a build-cache prune must not be applied now, from the journal alone (read-only: the same detection and reader as every
/// other command, so a legacy `journal.wal` counts as much as the SQLite store). `Ok(())` only when a journal is genuinely absent
/// (nothing is recorded as deploying) or is readable, unlocked and has no unfinished deployment. Anything that cannot be
/// established fails closed. This is Harpoon's own record: it says nothing about builds started by other Docker clients.
/// The preview uses this alone; the apply path uses [`prune_hold`], which adds the lock.
fn prune_guard(rt: &Runtime) -> Result<(), String> {
    if !rt.initialised() {
        return Ok(());
    }
    match harpoon_deploy_journal::writer_lock_status(&rt.journal_dir()) {
        Ok(harpoon_deploy_journal::LockStatus::Held) => return Err(
            "another Harpoon process holds the deployment journal, so a deployment may be running"
                .into(),
        ),
        Ok(_) => {}
        Err(e) => {
            return Err(format!(
                "the journal's lock state could not be read ({})",
                e.0
            ))
        }
    }
    unfinished_in_journal(rt)
}

fn unfinished_in_journal(rt: &Runtime) -> Result<(), String> {
    match store::read_restored(rt) {
        Ok(Some(r)) => match unfinished(&r.deployments).first() {
            Some(first) => Err(first.to_string()),
            None => Ok(()),
        },
        Ok(None) => Ok(()),
        Err(e) => Err(format!(
            "the deployment journal could not be read ({})",
            e.message.lines().next().unwrap_or("")
        )),
    }
}

/// The apply path's guard: the check above made under the journal's own writer lock, which the returned hold keeps until the prune is
/// over. Every Harpoon deployment writer takes that lock before it starts, so none can begin between the check and the prune (the
/// desktop's own operations are also serialised by its operations mutex; this closes the same window for a terminal process).
/// A genuinely absent journal needs no lock. The lock is a flag on a lock file: no journal data is read for writing, migrated or changed.
fn prune_hold(rt: &Runtime) -> Result<Option<harpoon_deploy_journal::WriterHold>, String> {
    if !rt.initialised() {
        return Ok(None);
    }
    let hold = harpoon_deploy_journal::hold_writer_lock(&rt.journal_dir()).map_err(|e| {
        if e.0.contains("another Harpoon process holds") {
            "another Harpoon process holds the deployment journal, so a deployment may be running"
                .to_string()
        } else {
            format!("the journal's writer lock could not be taken ({})", e.0)
        }
    })?;
    unfinished_in_journal(rt)?;
    Ok(Some(hold))
}

/// `prune-build-cache`: removes BuildKit cache nothing is using, through Docker's own prune call (what `docker builder
/// prune` does without `--all`). The cache is not something Harpoon records, so there is nothing to journal; what is
/// reported is Docker's figure and the guest filesystem's measured free space before and after. Later builds are slower
/// until the cache is rebuilt. Dry run by default; `--apply` needs the live-changes authorization.
pub(crate) fn prune_cache_cmd(p: &Parsed, out: &mut String) -> Result<i32, Fail> {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let apply = p.switch("apply");
    if apply {
        crate::commands::authorized(p)?;
    }
    let socket = crate::commands::socket_flag(p)?
        .ok_or_else(|| Fail::runtime("prune-build-cache needs an explicit --docker-socket PATH; nothing is assumed about the runtime"))?;
    if rt.kind()? == Some(Kind::Simulated) {
        return Err(Fail::refused(
            "this is a simulated runtime; there is nothing real to prune",
        ));
    }
    // Checked here, on the apply path itself, so no caller (desktop, terminal, script) can skip it by skipping a preview. The hold
    // lives until this function returns, that is, through the prune.
    let guard = if apply { Ok(()) } else { prune_guard(&rt) };
    let _hold = if apply {
        match prune_hold(&rt) {
            Ok(h) => h,
            Err(why) => {
                return Err(Fail::refused(format!(
                    "the build cache was not pruned: {why}. Prune only when no deployment is running or unfinished (run `recover` for an interrupted one). Nothing was changed."
                )))
            }
        }
    } else {
        None
    };
    let api = harpoon_deploy_docker::api::DockerApi::new(&socket, Duration::from_secs(30));
    let runtime_err = |e: harpoon_deploy_docker::api::ApiError| {
        Fail::runtime(format!(
            "cannot reach Docker at {}: {e}\nNothing was changed.",
            socket.display()
        ))
    };
    let (total, unused) = api
        .build_cache_usage(Duration::from_secs(30))
        .map_err(runtime_err)?;
    let _ = writeln!(out, "build cache: {total} bytes in all, {unused} bytes not in use. A prune removes only the unused part; images, containers and volumes are not touched, and later builds are slower until the cache fills again.");
    let _ = writeln!(out, "build-cache-reclaimable-bytes: {unused}");
    if !apply {
        match &guard {
            Ok(()) => {
                let _ = writeln!(out, "prune-guard: clear");
            }
            Err(why) => {
                let _ = writeln!(out, "prune-guard: blocked: {why}");
            }
        }
        let _ = writeln!(
            out,
            "dry run: nothing was changed. Add --apply and --{}=… to carry this out.",
            crate::AUTH_FLAG
        );
        return Ok(exit::OK);
    }
    let before = guest_free(p, &socket);
    let freed = api.prune_build_cache(Duration::from_secs(300)).map_err(|e| {
        use harpoon_deploy_docker::api::ApiError;
        use harpoon_deploy_docker::http::HttpError;
        match &e {
            // Docker never received the request: nothing happened
            ApiError::Transport(HttpError::Connect(_)) => Fail::failed(format!("Docker did not complete the prune: {e}. Nothing was changed.")),
            // The request may have reached Docker and been carried out; the answer is what is missing. Never reported as a plain failure.
            ApiError::Transport(_) | ApiError::Malformed { .. } => Fail::failed(format!(
                "result unknown: Docker did not give a usable answer to the prune ({e}). The prune may have completed fully, partly or not at all; nothing is assumed."
            )),
            // Docker answered that it failed; some cache may already have gone before the error
            ApiError::Status { .. } => Fail::failed(format!("Docker did not complete the prune: {e}. Cache removed before the error is not restored.")),
        }
    })?;
    let _ = writeln!(out, "build-cache-reclaimed-bytes: {freed}");
    match (before, guest_free(p, &socket)) {
        (Some(b), Some(a)) => {
            let _ = writeln!(
                out,
                "guest-free-before-bytes: {b}\nguest-free-after-bytes: {a}"
            );
        }
        _ => {
            let _ = writeln!(
                out,
                "guest-free: not measurable (the management channel did not answer)"
            );
        }
    }
    Ok(exit::OK)
}

/// `cleanup-releases`: dry run by default; `--apply` needs the live-changes authorization.
pub(crate) fn cleanup_cmd(p: &Parsed, out: &mut String) -> Result<i32, Fail> {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let apply = p.switch("apply");
    if apply {
        crate::commands::authorized(p)?;
    }
    let socket = crate::commands::socket_flag(p)?
        .ok_or_else(|| Fail::runtime("cleanup-releases needs an explicit --docker-socket PATH; nothing is assumed about the runtime"))?;
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
    run(p, &rt, &socket, apply, out)?;
    if !apply {
        let _ = writeln!(
            out,
            "dry run: nothing was changed. Add --apply and --{}=… to carry this out.",
            crate::AUTH_FLAG
        );
    }
    Ok(exit::OK)
}
