//! Which release an application is serving, and in which container. The answer is derived from the
//! durable records (deployments and the resource ledger), so it survives restarts.
//!
//! Usually the newest succeeded deployment serves, from its own container. After a bounded
//! restoration the previous release serves again, but from a **new** container recorded under the
//! failed deployment that restored it (`hp-<failed>-restored`); the stopped original stays recorded
//! under the release's own deployment. Replacement, status, reconcile and teardown must all use this
//! answer rather than assume that a release's container belongs to the release's deployment.

use crate::ids::{ApplicationId, DeploymentId, ReleaseId};
use crate::ledger::{Ledger, ResourceKind};
use crate::release::ReleaseConfig;
use crate::state::{Deployment, DeploymentStatus, RecoveryOutcome};

/// The name of the container a restoration creates for the deployment `failed`.
pub fn restored_container_name(failed: &DeploymentId) -> String {
    format!("hp-{failed}-restored")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServedBy {
    /// The release's own container, created when it was deployed.
    Original,
    /// A container recreated by the bounded restoration of this failed deployment.
    Restored { by: DeploymentId },
    /// A container a developer-initiated rollback created from the release's retained image.
    RolledBack { by: DeploymentId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Serving {
    pub application: ApplicationId,
    pub release: ReleaseId,
    /// The deployment that built the release (its image and pinned configuration live there).
    pub deployment: DeploymentId,
    /// The deployment whose ledger holds the serving container.
    pub container_owner: DeploymentId,
    pub container_name: String,
    pub via: ServedBy,
    pub since_ms: u64,
    /// The configuration this container actually serves with (what a later replacement or rollback
    /// would have to restore), when one was pinned.
    pub config: Option<ReleaseConfig>,
}

/// The release `app` serves now, or `None` when nothing is recorded as serving.
pub fn serving_release(ledger: &Ledger, deployments: &[Deployment], app: &ApplicationId) -> Option<Serving> {
    let live_container = |owner: &DeploymentId, only: Option<&str>| {
        ledger
            .entries()
            .iter()
            .filter(|e| {
                e.deployment == *owner
                    && e.app == *app
                    && e.key.kind == ResourceKind::Container
                    && !e.is_released()
                    && e.id.is_some()
                    && only.is_none_or(|n| e.key.name == n)
            })
            .map(|e| e.key.name.clone())
            .next()
    };
    let mut best: Option<Serving> = None;
    let mut consider = |c: Serving| {
        if best
            .as_ref()
            .is_none_or(|b| (c.since_ms, c.container_owner.as_str()) > (b.since_ms, b.container_owner.as_str()))
        {
            best = Some(c);
        }
    };
    for d in deployments.iter().filter(|d| d.application_id == *app) {
        if d.status == DeploymentStatus::Succeeded {
            if let (Some(rel), Some(name)) = (d.release.clone(), live_container(&d.id, None)) {
                // A rollback serves the *earlier* release, from a container of its own.
                let (release, deployment, via) = match &d.rollback_to {
                    Some(target) => (
                        target.clone(),
                        target.deployment(),
                        ServedBy::RolledBack { by: d.id.clone() },
                    ),
                    None => (rel, d.id.clone(), ServedBy::Original),
                };
                consider(Serving {
                    application: app.clone(),
                    release,
                    deployment,
                    container_owner: d.id.clone(),
                    container_name: name,
                    via,
                    since_ms: d.finished_ms.unwrap_or(d.created_ms),
                    config: d.pinned.clone(),
                });
            }
        }
        let restored = d.recovery.filter(|r| {
            matches!(
                r.outcome,
                RecoveryOutcome::Restored | RecoveryOutcome::RestoredUnverified
            )
        });
        if let (Some(rec), Some(rel)) = (restored, d.replaces.clone()) {
            if let Some(name) = live_container(&d.id, Some(&restored_container_name(&d.id))) {
                consider(Serving {
                    config: deployments
                        .iter()
                        .find(|x| x.id == rel.deployment())
                        .and_then(|x| x.pinned.clone()),
                    application: app.clone(),
                    deployment: rel.deployment(),
                    release: rel,
                    container_owner: d.id.clone(),
                    container_name: name,
                    via: ServedBy::Restored { by: d.id.clone() },
                    since_ms: rec.finished_ms.unwrap_or(rec.started_ms),
                });
            }
        }
    }
    best
}

/// Every container entry that belongs to the release built by `release_dep`: its own, and any that
/// a restoration created for it. These are what dismantling that release must handle.
pub fn containers_of_release<'a>(
    ledger: &'a Ledger,
    deployments: &[Deployment],
    release_dep: &DeploymentId,
) -> Vec<&'a crate::ledger::Entry> {
    let release = ReleaseId::from_deployment(release_dep);
    ledger
        .entries()
        .iter()
        .filter(|e| e.key.kind == ResourceKind::Container)
        .filter(|e| {
            e.deployment == *release_dep
                || deployments.iter().any(|f| {
                    f.id == e.deployment
                        && f.rollback_to.as_ref() == Some(&release)
                        && f.status == DeploymentStatus::Succeeded
                })
                || deployments.iter().any(|f| {
                    f.id == e.deployment
                        && f.replaces.as_ref() == Some(&release)
                        && e.key.name == restored_container_name(&f.id)
                        && f.recovery.is_some_and(|r| {
                            matches!(
                                r.outcome,
                                RecoveryOutcome::Restored | RecoveryOutcome::RestoredUnverified
                            )
                        })
                })
        })
        .collect()
}
