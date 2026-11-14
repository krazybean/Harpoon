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
use harpoon_deploy::ledger::{plan as ledger_plan, Action, Disposition, EndState, EntryState, Observed, ResourceKind};
use harpoon_deploy::serving::serving_release;
use harpoon_deploy::state::DeploymentStatus;
use harpoon_deploy::time::SystemClock;
use harpoon_deploy_docker::backend::ImageVerdict;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Tidied {
    pub containers: usize,
    pub leases: usize,
    pub images: usize,
}

/// Plan (`apply == false`: nothing is changed, the journal is not written) or carry out the cleanup. A refusal
/// leaves everything as it was.
pub(crate) fn run(p: &Parsed, rt: &Runtime, socket: &Path, apply: bool, out: &mut String) -> Result<Tidied, Fail> {
    let mut backend = docker_backend(p, rt, socket)?;
    let (mut writer, restored) = if apply {
        let (j, r) = store::open_writer(rt, Kind::Docker)?;
        (Some(j), r)
    } else {
        (None, store::read_restored(rt)?.ok_or_else(|| Fail::refused("there is no journal; nothing is recorded to clean up"))?)
    };
    let deps = &restored.deployments;
    if let Some(first) = unfinished(deps).first() {
        return Err(Fail::refused(format!("{first}\ncleanup never runs while a deployment is unfinished; nothing was changed")));
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
            None => protected.extend(deps.iter().filter(|d| d.application_id == *app).map(|d| d.id.clone())),
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
            DeploymentStatus::Failed | DeploymentStatus::Interrupted if d.replaces.is_none() && d.recovery.is_none() => {
                end_state(d.status).map(|e| (d.id.clone(), e))
            }
            _ => None,
        })
        .collect();
    let in_scope: BTreeSet<&DeploymentId> = scope.iter().map(|(d, _)| d).collect();

    let _ = writeln!(out, "cleanup of Harpoon-owned artifacts — {}", if apply { "APPLYING" } else { "dry run: nothing will be changed" });
    for id in &protected {
        let _ = writeln!(out, "protected: deployment {id} (serves now, owns the serving container, or nothing is proven to serve)");
    }
    let observed = backend.list_resources().map_err(|e| {
        Fail::runtime(format!("cannot list the runtime: {}\nNothing was assumed absent and nothing was changed.", e.message))
    })?;
    let mut ledger = restored.ledger.clone();
    let mut tidied = Tidied::default();

    // 1. containers and port leases of those deployments
    let actions: Vec<Action> = ledger_plan(&ledger, &observed, &scope)
        .into_iter()
        .filter(|a| !matches!(a, Action::ReportOnly { .. } | Action::Refuse { .. }))
        .collect();
    for a in &actions {
        let _ = writeln!(out, "{}{}", if apply { "" } else { "would: " }, describe(a));
    }
    tidied.containers = actions.iter().filter(|a| matches!(a, Action::Remove { observed, .. } if observed.kind == ResourceKind::Container)).count();
    tidied.leases = actions.iter().filter(|a| matches!(a, Action::ReleaseLease { .. })).count();
    if let Some(j) = writer.as_mut() {
        execute_actions(&actions, &mut ledger, j, &mut backend, &SystemClock)
            .map_err(|e| Fail::failed(format!("journal write failed during cleanup: {e}; run `recover`, then `status`, and retry")))?;
    }

    // 2. retained images, one at a time, each re-verified against Docker
    let candidates: Vec<(DeploymentId, harpoon_deploy::ledger::ResourceKey, String, ApplicationId)> = ledger
        .entries()
        .iter()
        .filter(|e| e.key.kind == ResourceKind::Image && e.disposition == Disposition::Retain && !e.is_released() && in_scope.contains(&e.deployment))
        .filter(|e| !matches!(e.state, EntryState::Refused(_)))
        .filter_map(|e| e.id.clone().map(|id| (e.deployment.clone(), e.key.clone(), id, e.app.clone())))
        .collect();
    for (dep, key, id, app) in candidates {
        let skip = |out: &mut String, why: &str| {
            let _ = writeln!(out, "kept: image {id} of {dep}: {why}");
        };
        if protected_images.contains(&id) {
            skip(out, "the serving release pins this image");
            continue;
        }
        let open: Vec<String> = ledger
            .entries()
            .iter()
            .filter(|o| o.deployment == dep && o.key != key && !o.is_released())
            .map(|o| format!("{} {}", o.key.kind.as_str(), o.key.name))
            .collect();
        if !open.is_empty() {
            skip(out, &format!("the deployment still has unreleased resources ({})", open.join(", ")));
            continue;
        }
        if ledger.entries().iter().any(|o| o.deployment != dep && o.key.kind == ResourceKind::Image && !o.is_released() && o.id.as_deref() == Some(&id) && protected.contains(&o.deployment)) {
            skip(out, "a protected deployment records the same image");
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
            Ok(ImageVerdict::Absent) => Action::MarkAbsent { entry: (dep.clone(), key.clone()) },
            Ok(ImageVerdict::Eligible) => Action::Remove {
                entry: (dep.clone(), key.clone()),
                observed: Observed { kind: ResourceKind::Image, name: key.name.clone(), id: id.clone(), labels: Vec::new() },
                adopt: false,
            },
        };
        let _ = writeln!(out, "{}{}", if apply { "" } else { "would: " }, describe(&action));
        if let Some(j) = writer.as_mut() {
            let mut ops = backend.retained_image_ops();
            if let Err(e) = execute_actions(std::slice::from_ref(&action), &mut ledger, j, &mut ops, &SystemClock) {
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
    let _ = writeln!(out, "reclaimed-containers: {}\nreclaimed-images: {}", tidied.containers, tidied.images);
    Ok(tidied)
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
        return Err(Fail::refused(format!("there is no journal at {}; nothing is recorded to clean up", rt.journal_dir().display())));
    }
    if rt.kind()? == Some(Kind::Simulated) {
        return Err(Fail::refused("this is a simulated runtime; there is nothing real to clean up"));
    }
    run(p, &rt, &socket, apply, out)?;
    if !apply {
        let _ = writeln!(out, "dry run: nothing was changed. Add --apply and --{}=… to carry this out.", crate::AUTH_FLAG);
    }
    Ok(exit::OK)
}
