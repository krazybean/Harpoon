//! Which earlier releases an application could be rolled back to, decided from the durable
//! records only (spec D5). A release is a succeeded deployment; it is a target only when everything
//! needed to re-serve it exactly is provable: a pinned configuration naming an immutable image id,
//! and a retained image of that deployment in the resource ledger. Nothing is guessed from
//! container names or tags, and an unprovable release is listed with the reasons, never offered.

use crate::ids::{ApplicationId, DeploymentId, ReleaseId};
use crate::ledger::{Ledger, ResourceKind};
use crate::release::ReleaseConfig;
use crate::serving::serving_release;
use crate::state::{Deployment, DeploymentStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedRelease {
    pub release: ReleaseId,
    pub deployment: DeploymentId,
    pub finished_ms: u64,
    pub config: Option<ReleaseConfig>,
    pub serving: bool,
    /// Why this release cannot be rolled back to. Empty means it can.
    pub problems: Vec<String>,
}

impl RetainedRelease {
    pub fn eligible(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Every release of `app` that was ever deployed and succeeded, newest first, each with what
/// stands in the way of rolling back to it. A rollback deployment is not itself a release: it
/// re-serves one of these.
pub fn retained_releases(ledger: &Ledger, deployments: &[Deployment], app: &ApplicationId) -> Vec<RetainedRelease> {
    let serving = serving_release(ledger, deployments, app);
    let serving_finished = serving.as_ref().and_then(|s| {
        deployments
            .iter()
            .find(|d| d.id == s.deployment)
            .map(|d| d.finished_ms.unwrap_or(d.created_ms))
    });
    let mut out: Vec<RetainedRelease> = deployments
        .iter()
        .filter(|d| {
            d.application_id == *app
                && d.status == DeploymentStatus::Succeeded
                && d.release.is_some()
                && d.rollback_to.is_none()
        })
        .map(|d| {
            let release = d.release.clone().expect("filtered");
            let finished_ms = d.finished_ms.unwrap_or(d.created_ms);
            let is_serving = serving.as_ref().is_some_and(|s| s.release == release);
            let mut problems = Vec::new();
            match (&serving, serving_finished) {
                (None, _) => problems.push("the application has no serving release recorded".to_string()),
                (Some(_), None) => problems.push("the serving release's deployment is not in the journal".to_string()),
                (Some(_), Some(sf)) => {
                    if is_serving {
                        problems.push("this is the release serving now".to_string());
                    } else if finished_ms >= sf {
                        problems.push(
                            "this release is not earlier than the serving one (moving forward is a replacement, not a rollback)".to_string(),
                        );
                    }
                }
            }
            match &d.pinned {
                None => problems.push("no pinned configuration was stored for it (it predates pinning)".to_string()),
                Some(c) if !c.image_id.starts_with("sha256:") => problems.push(format!(
                    "its pinned image {:?} is not an immutable image id",
                    c.image_id
                )),
                Some(_) => {}
            }
            let retained = ledger.entries().iter().any(|e| {
                e.deployment == d.id
                    && e.app == *app
                    && e.key.kind == ResourceKind::Image
                    && e.id.is_some()
                    && !e.is_released()
            });
            if !retained {
                problems.push("its retained image is not recorded as present (removed, or never recorded)".to_string());
            }
            RetainedRelease {
                release,
                deployment: d.id.clone(),
                finished_ms,
                config: d.pinned.clone(),
                serving: is_serving,
                problems,
            }
        })
        .collect();
    out.sort_by(|a, b| (b.finished_ms, b.deployment.as_str()).cmp(&(a.finished_ms, a.deployment.as_str())));
    out
}

/// The one release `target` names, or every reason it cannot be a rollback target. The exact
/// release identity is required: there is no "previous" and no matching by name or tag.
pub fn select_target(
    ledger: &Ledger,
    deployments: &[Deployment],
    app: &ApplicationId,
    target: &ReleaseId,
) -> Result<RetainedRelease, Vec<String>> {
    match retained_releases(ledger, deployments, app)
        .into_iter()
        .find(|r| r.release == *target)
    {
        None => Err(vec![format!(
            "release {target} is not a succeeded release of application {app} in the journal"
        )]),
        Some(r) if !r.eligible() => Err(r.problems),
        Some(r) => Ok(r),
    }
}
