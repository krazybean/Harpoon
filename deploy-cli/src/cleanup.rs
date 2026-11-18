//! Release-aware cleanup of Harpoon-owned Docker artifacts.
//!
//! What a successful replacement leaves behind (the stopped containers, port leases and images of releases that no
//! longer serve) and what a failed one leaves (its retained image) is removed here, by the ids the journal recorded.
//! Everything goes through the engine's ledger: an object is removed only when Docker still shows the recorded id with
//! all three Harpoon ownership labels, and the result is journaled. Protected, never touched:
//!
//! * the release each application serves now, and the deployment that owns its container;
//! * the most recent previous release proven restorable (pinned configuration, retained image in the journal, the image in
//!   Docker by its immutable id with this deployment's labels), and, when that cannot be shown, every candidate (fail closed);
//! * a deployment whose cutover began and whose restoration is not finished and verified, or whose pause no decision cleared;
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
use harpoon_deploy::control::active_pauses;
use harpoon_deploy::engine::{end_state, execute_actions};
use harpoon_deploy::ids::{ApplicationId, DeploymentId};
use harpoon_deploy::ledger::ResourceKey;
use harpoon_deploy::ledger::{
    plan as ledger_plan, Action, Disposition, EndState, EntryState, Observed, ResourceKind,
};
use harpoon_deploy::rollback::retained_releases;
use harpoon_deploy::serving::{restored_container_name, serving_release};
use harpoon_deploy::state::{Deployment, DeploymentStatus, RecoveryOutcome};
use harpoon_deploy::time::SystemClock;
use harpoon_deploy_docker::backend::{AbandonedContainer, ImageVerdict, RollbackImage};
use harpoon_deploy_docker::guest::MgmtDfProbe;
use harpoon_deploy_docker::json::{n, obj, s as js, Json};
use harpoon_deploy_docker::reclaim::estimate_reclaim;
use std::collections::{BTreeMap, BTreeSet};
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
    // Rollback targets. For each application that serves a release, the most recent PREVIOUS release that is proven
    // restorable is kept, so that cleanup never removes the image a rollback would start from. "Proven" is the journal's own
    // rollback eligibility (succeeded, earlier than the serving release, a pinned immutable image id, a retained image in the
    // ledger) AND Docker holding exactly that id with this deployment's labels. Order only ranks releases that are proven; it
    // never makes one a target. What cannot be shown is kept, not guessed: a journal image that is not the pinned one, a Docker
    // that cannot be asked, an image whose labels do not match, or two proven releases that cannot be ordered keep every
    // remaining candidate. An image Docker proves absent is not a rollback and protects nothing.
    let mut rollback_why: BTreeMap<DeploymentId, (&'static str, String)> = BTreeMap::new();
    let mut rollback_images: BTreeSet<String> = BTreeSet::new();
    let mut retention: Vec<(ApplicationId, DeploymentId, String)> = Vec::new();
    for app in &apps {
        let Some(serving) = serving_release(&restored.ledger, deps, app) else {
            continue;
        };
        let eligible: Vec<_> = retained_releases(&restored.ledger, deps, app)
            .into_iter()
            .filter(|r| r.eligible())
            .collect();
        let mut keep: Vec<(usize, &'static str, String)> = Vec::new();
        let mut state = "no previous release is provably restorable".to_string();
        for (i, r) in eligible.iter().enumerate() {
            let pinned = r
                .config
                .as_ref()
                .map(|c| c.image_id.clone())
                .unwrap_or_default();
            let recorded = restored.ledger.entries().iter().any(|e| {
                e.deployment == r.deployment
                    && e.key.kind == ResourceKind::Image
                    && !e.is_released()
                    && e.id.as_deref() == Some(pinned.as_str())
            });
            let doubt = if !recorded {
                Some("the journal's retained image is not the image the release pins".to_string())
            } else {
                match backend.verify_rollback_image(&pinned, app, &r.deployment) {
                    RollbackImage::Absent => continue,
                    RollbackImage::Unverifiable(why) => Some(why),
                    RollbackImage::Verified if eligible.get(i + 1).is_some_and(|n| n.finished_ms == r.finished_ms) => {
                        Some("two restorable releases finished at the same moment, so the more recent one cannot be told".to_string())
                    }
                    RollbackImage::Verified => None,
                }
            };
            match doubt {
                None => {
                    keep.push((i, "rollback-target", format!("the most recent previous release proven restorable: pinned image {pinned} present in Docker with this deployment's labels; kept so the application can be rolled back")));
                    state = format!("rollback target {}", r.deployment);
                }
                Some(why) => {
                    // fail closed: this and every older candidate stay
                    for j in i..eligible.len() {
                        keep.push((
                            j,
                            "rollback-unverified",
                            format!("rollback eligibility could not be verified ({why}); kept"),
                        ));
                    }
                    state = format!(
                        "rollback eligibility unverified ({why}); {} release(s) kept",
                        eligible.len() - i
                    );
                }
            }
            break;
        }
        for (i, reason, detail) in keep {
            let r = &eligible[i];
            protected.insert(r.deployment.clone());
            if let Some(c) = &r.config {
                rollback_images.insert(c.image_id.clone());
            }
            rollback_why.insert(r.deployment.clone(), (reason, detail));
        }
        retention.push((app.clone(), serving.deployment.clone(), state));
    }
    // An interrupted or failed cutover is settled when its bounded restoration finished and passed its health check
    // (`restored`: not unverified, failed, not attempted or still running) and no pause caused by it is still in force. Its
    // leftovers belong to no running release then: whatever now serves is protected above and the rollback target below.
    let paused: BTreeSet<DeploymentId> = active_pauses(deps, &restored.control)
        .into_iter()
        .map(|p| p.origin)
        .collect();
    let settled = |d: &Deployment| {
        d.recovery
            .is_some_and(|r| r.outcome == RecoveryOutcome::Restored)
            && !paused.contains(&d.id)
    };
    // in scope: succeeded releases that no longer serve (treated as ended without success for the planner), and
    // failed or interrupted deployments. Cancelled (prepared) candidates are left for the person to confirm or remove.
    let scope: Vec<(DeploymentId, EndState)> = deps
        .iter()
        .filter(|d| !protected.contains(&d.id))
        .filter_map(|d| match d.status {
            DeploymentStatus::Succeeded => Some((d.id.clone(), EndState::NotSucceeded)),
            // one whose cutover began is left for `status`/`restore`: the application may be mid-recovery. Only a deployment
            // whose restoration finished and was verified, and whose pause a decision cleared, is settled and in scope.
            DeploymentStatus::Failed | DeploymentStatus::Interrupted
                if (d.replaces.is_none() && d.recovery.is_none()) || settled(d) =>
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
        match rollback_why.get(id) {
            Some(("rollback-target", _)) => {
                let _ = writeln!(out, "protected: deployment {id} (rollback target: the most recent previous release proven restorable)");
            }
            Some(_) => {
                let _ = writeln!(out, "protected: deployment {id} (rollback eligibility could not be verified, so it is kept)");
            }
            None => {
                let _ = writeln!(out, "protected: deployment {id} (serves now, owns the serving container, or nothing is proven to serve)");
            }
        }
    }
    for (app, serving, state) in &retention {
        let eligible = scope
            .iter()
            .filter(|(d, _)| {
                deps.iter().any(|x| {
                    x.id == *d
                        && x.application_id == *app
                        && x.status == DeploymentStatus::Succeeded
                })
            })
            .count();
        let settled_left = scope
            .iter()
            .filter(|(d, _)| {
                deps.iter().any(|x| {
                    x.id == *d
                        && x.application_id == *app
                        && x.status != DeploymentStatus::Succeeded
                        && settled(x)
                })
            })
            .count();
        let tail = if settled_left > 0 {
            format!("; {settled_left} settled interrupted deployment(s) (restored, pause cleared) eligible")
        } else {
            String::new()
        };
        let _ = writeln!(out, "retention: application {app}: serving {serving}; {state}; {eligible} other release(s) eligible for cleanup{tail}");
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
            let (reason, detail) = if let Some((reason, detail)) = rollback_why.get(id) {
                (*reason, detail.as_str())
            } else if proven {
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
    // The container a restoration recreated is recorded as retained, so the planner never removes it. Once its deployment is
    // settled and out of scope of everything that serves, it is abandoned; it is removed only when Docker shows it is
    // Harpoon's by id and all three labels and that it is stopped. A running or unverifiable one is kept and says why.
    let mut abandoned: Vec<(
        ApplicationId,
        DeploymentId,
        String,
        &'static str,
        &'static str,
        String,
    )> = Vec::new();
    let mut actions = actions;
    for (dep, _) in &scope {
        let Some(d) = deps.iter().find(|d| d.id == *dep) else {
            continue;
        };
        if d.status == DeploymentStatus::Succeeded || !settled(d) {
            continue;
        }
        let name = restored_container_name(dep);
        for e in ledger.entries().iter().filter(|e| {
            e.deployment == *dep
                && e.key.kind == ResourceKind::Container
                && e.key.name == name
                && !e.is_released()
                && !matches!(e.state, EntryState::Refused(_))
        }) {
            let Some(id) = e.id.clone() else {
                abandoned.push((
                    e.app.clone(),
                    dep.clone(),
                    name.clone(),
                    "review",
                    "needs-review",
                    "the journal has no receipt for this container, so what it is cannot be shown"
                        .into(),
                ));
                continue;
            };
            let entry = (dep.clone(), e.key.clone());
            let Some(o) = observed
                .iter()
                .find(|o| o.kind == ResourceKind::Container && o.id == id)
            else {
                actions.push(Action::MarkAbsent { entry });
                continue;
            };
            if !o.labelled_for(&e.app, dep) {
                abandoned.push((e.app.clone(), dep.clone(), name.clone(), "review", "needs-review", "Docker's container does not carry this deployment's Harpoon labels; not touched".into()));
                continue;
            }
            match backend.verify_abandoned_container(&id, &e.app, dep) {
                AbandonedContainer::Stopped => actions.push(Action::Remove {
                    entry,
                    observed: o.clone(),
                    adopt: false,
                }),
                AbandonedContainer::Absent => actions.push(Action::MarkAbsent { entry }),
                AbandonedContainer::Running(why) | AbandonedContainer::Unverifiable(why) => {
                    abandoned.push((
                        e.app.clone(),
                        dep.clone(),
                        name.clone(),
                        "review",
                        "needs-review",
                        format!("{why}; kept"),
                    ));
                }
            }
        }
    }
    for (app, dep, name, verdict, reason, why) in &abandoned {
        let _ = writeln!(out, "kept: container {name} of {dep}: {why}");
        item(
            out,
            app,
            dep,
            "container",
            name,
            verdict,
            reason,
            why,
            Some(true),
        );
    }
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
                    let restoration = key.name == restored_container_name(dep);
                    item(
                        out,
                        &app,
                        dep,
                        observed.kind.as_str(),
                        &key.name,
                        "ready",
                        if restoration {
                            "abandoned-restoration"
                        } else if failed {
                            "failed-attempt"
                        } else {
                            "superseded"
                        },
                        if restoration {
                            "the container a restoration recreated for a release that no longer serves: stopped, and Docker shows it is Harpoon's by id and all three labels"
                        } else {
                            "Docker still holds this object with the recorded id and all three Harpoon labels"
                        },
                        Some(true),
                    );
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

    // What Docker says about every image's layers, taken before anything is removed. Removing an image removes a reference;
    // space is freed only for layers no image that stays keeps, which is what the estimate below is made of.
    let layers = backend.image_layers();
    let mut removed_images: BTreeSet<String> = BTreeSet::new();

    // containers this run removes: an image they were made from is judged as it will be once they are gone
    let going: BTreeSet<String> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Remove { observed, .. } if observed.kind == ResourceKind::Container => {
                Some(observed.id.clone())
            }
            _ => None,
        })
        .collect();

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
        if rollback_images.contains(&id) {
            skip_as(
                out,
                "protected",
                "rollback-pins-image",
                "the image a rollback of this application would start from",
            );
            continue;
        }
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
        let action = match backend.assess_retained_image_after(&id, &key.name, &app, &dep, &going) {
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
                Some(EntryState::Released(_)) => {
                    tidied.images += 1;
                    removed_images.insert(id.clone());
                }
                _ => skip(out, "Docker did not remove it; it stays recorded and is retried by the next cleanup"),
            }
        } else if matches!(action, Action::Remove { .. }) {
            tidied.images += 1;
            removed_images.insert(id.clone());
        }
    }
    // Estimated space, from layer sharing. Never inferred from how many image references go: two images with the same layers
    // free nothing while one stays. Said apart from the measured change in the guest's free space printed after an apply.
    let estimate = match &layers {
        Ok(all) => {
            let e = estimate_reclaim(all, &removed_images);
            let _ = writeln!(
                out,
                "estimated-reclaim-bytes: {}\nestimated-reclaim-exact: {}",
                e.bytes, e.exact
            );
            for gone in all.iter().filter(|i| removed_images.contains(&i.id)) {
                if let Some(held) = all.iter().find(|k| {
                    !removed_images.contains(&k.id)
                        && k.layers == gone.layers
                        && (protected_images.contains(&k.id) || rollback_images.contains(&k.id))
                }) {
                    let _ = writeln!(out, "shared-layers: image {} has the same layers as protected image {}; removing it frees nothing by itself, and its space is freed only if that image ever goes too", gone.id, held.id);
                }
            }
            if !removed_images.is_empty() && e.bytes == 0 {
                let _ = writeln!(out, "estimate: removing these image reference(s) is expected to free no space, because every layer is still used by an image that stays");
            }
            Some(e)
        }
        Err(why) => {
            let _ = writeln!(
                out,
                "estimated-reclaim-bytes: not estimated ({})",
                why.message
            );
            None
        }
    };
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
                let freed = a.saturating_sub(b);
                let _ = writeln!(out, "measured-reclaimed-bytes: {freed}");
                let check = match estimate {
                    _ if tidied.images == 0 => "no image was removed, so there is nothing to compare".to_string(),
                    Some(e) if e.bytes == 0 && e.exact => "image reference(s) were removed but no space was expected: their layers are used by images that stay. None is claimed".to_string(),
                    Some(e) if freed >= e.bytes / 10 * 9 => "the measured change matches the estimate".to_string(),
                    Some(_) => "the measured change is lower than estimated: Docker may keep the layers, or something else used space meanwhile. Only the measured figure is real".to_string(),
                    None => "no estimate was available; only the measured figure is real".to_string(),
                };
                let _ = writeln!(out, "reclaim-check: {check}");
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
/// The preview uses this alone; the apply path uses [`cache_hold`], which adds the lock.
fn cache_guard(rt: &Runtime) -> Result<(), String> {
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
/// over. Every Harpoon deployment writer takes that lock before it starts, so none can begin between the check and the cache removal (the
/// desktop's own operations are also serialised by its operations mutex; this closes the same window for a terminal process).
/// A genuinely absent journal needs no lock. The lock is a flag on a lock file: no journal data is read for writing, migrated or changed.
fn cache_hold(rt: &Runtime) -> Result<Option<harpoon_deploy_journal::WriterHold>, String> {
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
    let guard = if apply { Ok(()) } else { cache_guard(&rt) };
    let _hold = if apply {
        match cache_hold(&rt) {
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
    let usage = api
        .build_cache_usage(Duration::from_secs(30))
        .map_err(runtime_err)?;
    // A record's size is not space a prune frees: records overlap each other and, when Docker marks one shared, its layers
    // belong to an image and stay while the image does. Only an unused, unshared record is something a prune can remove.
    let _ = writeln!(
        out,
        "build cache: {} record(s) whose own sizes add up to {} bytes. That is not disk space: records overlap, and {} of them ({} bytes) share layers with an image, {} ({} bytes) are in use.",
        usage.records, usage.total_bytes, usage.shared_records, usage.shared_bytes, usage.in_use_records, usage.in_use_bytes
    );
    if usage.prunable_records == 0 {
        let _ = writeln!(out, "A prune is expected to free about 0 bytes: no record is both unused and unshared. The space is held by images, and only removing releases (cleanup-releases) can free it.");
    } else {
        let _ = writeln!(out, "A prune can remove {} record(s), at most {} bytes (an estimate; Docker's own figure after the prune is the result). Images, containers and volumes are not touched, and later builds are slower until the cache fills again.", usage.prunable_records, usage.prunable_bytes);
    }
    let _ = writeln!(
        out,
        "build-cache-records: {}\nbuild-cache-total-bytes: {}\nbuild-cache-in-use-bytes: {}\nbuild-cache-shared-bytes: {}\nbuild-cache-prunable-records: {}\nbuild-cache-reclaimable-bytes: {}",
        usage.records, usage.total_bytes, usage.in_use_bytes, usage.shared_bytes, usage.prunable_records, usage.prunable_bytes
    );
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
                "result unknown: Docker did not give a usable answer to the prune request ({e}). The prune may have completed fully, partly or not at all; nothing is assumed."
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
