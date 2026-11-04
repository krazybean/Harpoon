//! What to tell a developer when a candidate failed **before** any cutover (source, build or
//! preliminary check): what is and is not affected, and only commands this CLI really has. Nothing
//! here retries, restores or removes anything; it only describes.

use harpoon_deploy::ids::ApplicationId;
use harpoon_deploy::ledger::{Ledger, ResourceKind};
use harpoon_deploy::state::{Deployment, FailureKind};

pub struct Ctx<'a> {
    /// `deploy`, `replace` or `rollback`.
    pub command: &'a str,
    pub app: &'a ApplicationId,
    pub deployment: &'a Deployment,
    pub ledger: &'a Ledger,
    pub runtime_dir: &'a str,
    /// An offline run: no Docker image exists to remove.
    pub simulated: bool,
    /// The release that keeps serving (release id, container name), when one was serving.
    pub serving: Option<(String, String)>,
    /// Whether the engine reported a trigger pause (it must not, for this kind of failure).
    pub paused: bool,
}

/// `None` when the failure is not a before-cutover failure (a cutover or final-check failure has
/// its own recovery output).
pub fn lines(c: &Ctx<'_>) -> Option<Vec<String>> {
    let f = c.deployment.failure.as_ref()?;
    if !matches!(
        f.kind,
        FailureKind::Source | FailureKind::Build | FailureKind::PreliminaryCheck
    ) {
        return None;
    }
    let mut v = Vec::new();
    v.push(format!(
        "the candidate failed at {} before any cutover began",
        f.step.as_str()
    ));
    match &c.serving {
        Some((release, container)) => v.push(format!(
            "  - nothing was stopped or changed: release {release} (container {container}) keeps serving"
        )),
        None => v.push("  - nothing was stopped or changed (no release was serving)".into()),
    }
    v.push("  - no restoration was attempted: there was nothing to restore".into());
    if !c.paused {
        v.push("  - automatic triggers are not paused by this failure".into());
    }
    let image = c
        .ledger
        .entries()
        .iter()
        .find(|e| {
            e.deployment == c.deployment.id
                && e.key.kind == ResourceKind::Image
                && !e.is_released()
                && e.id.is_some()
        })
        .map(|e| (e.key.name.clone(), e.id.clone().unwrap_or_default()));
    match &image {
        Some((tag, id)) => v.push(format!(
            "  - the candidate's image is retained for inspection: {tag} ({id})"
        )),
        None if f.kind == FailureKind::Build => {
            v.push("  - the build produced no retained image".into())
        }
        None => {}
    }
    v.push("next steps (nothing is retried automatically):".into());
    v.push(format!(
        "  inspect:  harpoon-deploy status --runtime-dir {}   (shows the failing step, its message and any retained image; the bounded output above is what the step logged)",
        c.runtime_dir
    ));
    let sim = if c.simulated { " --simulate" } else { "" };
    v.push(match c.command {
        "replace" => format!(
            "  retry:    fix the cause, then run `harpoon-deploy replace --app {}{sim} …` again with the same options; it is a new deployment and builds a new image",
            c.app
        ),
        "rollback" => format!(
            "  retry:    run the same `harpoon-deploy rollback --app {}{sim} --to RELEASE --confirm-rollback` again once the cause is fixed",
            c.app
        ),
        _ => format!(
            "  retry:    fix the cause, then run `harpoon-deploy deploy{sim} --app {} …` again with the same options; it is a new deployment and builds a new image",
            c.app
        ),
    });
    if let (Some((_, id)), false) = (&image, c.simulated) {
        v.push(format!(
            "  remove:   harpoon-deploy cleanup-image --runtime-dir {} --deployment {} --image {id}   (a dry run; add --apply, --docker-socket, --resources-dir and the authorization flag to remove it)",
            c.runtime_dir, c.deployment.id
        ));
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use harpoon_deploy::ledger::{Disposition, LedgerRecord, ResourceKey};
    use harpoon_deploy::state::{DeploymentStatus, Failure, Step, Trigger};

    fn failed(kind: FailureKind, step: Step) -> (Deployment, ApplicationId) {
        let app: ApplicationId = "app_00000000000000000000000001".parse().unwrap();
        let mut d = Deployment::new(
            "dep_00000000000000000000000002".parse().unwrap(),
            app.clone(),
            Trigger::Cli,
            1,
        );
        d.transition(DeploymentStatus::Queued, 2).unwrap();
        d.transition(DeploymentStatus::Running, 3).unwrap();
        d.failure = Some(Failure {
            step,
            kind,
            message: "x".into(),
            timed_out: false,
        });
        d.transition(DeploymentStatus::Failed, 4).unwrap();
        (d, app)
    }

    fn with_image(d: &Deployment, app: &ApplicationId) -> Ledger {
        let mut l = Ledger::new();
        let key = ResourceKey::new(ResourceKind::Image, "harpoon/app:dep");
        l.apply(&LedgerRecord::Intent {
            key: key.clone(),
            app: app.clone(),
            deployment: d.id.clone(),
            disposition: Disposition::Retain,
            at_ms: 1,
        })
        .unwrap();
        l.apply(&LedgerRecord::Receipt {
            key,
            deployment: d.id.clone(),
            id: format!("sha256:{}", "a".repeat(64)),
            at_ms: 2,
        })
        .unwrap();
        l
    }

    #[test]
    fn a_preliminary_failure_with_a_serving_release_says_nothing_was_touched_and_offers_only_real_commands(
    ) {
        let (d, app) = failed(FailureKind::PreliminaryCheck, Step::PreliminaryCheck);
        let ledger = with_image(&d, &app);
        let v = lines(&Ctx {
            command: "replace",
            app: &app,
            deployment: &d,
            ledger: &ledger,
            runtime_dir: "/rt",
            simulated: false,
            serving: Some(("rel_x".into(), "hp-dep_x".into())),
            paused: false,
        })
        .unwrap();
        let t = v.join("\n");
        assert!(t.contains("keeps serving") && t.contains("rel_x"), "{t}");
        assert!(
            t.contains("no restoration was attempted") && t.contains("not paused"),
            "{t}"
        );
        assert!(
            t.contains("retained for inspection") && t.contains(&"a".repeat(64)),
            "{t}"
        );
        assert!(t.contains("status --runtime-dir /rt") && t.contains("cleanup-image --runtime-dir /rt --deployment dep_00000000000000000000000002 --image sha256:"), "{t}");
        assert!(
            t.contains("replace --app app_00000000000000000000000001"),
            "{t}"
        );
        // no command that does not exist
        for invented in [
            "retry ",
            "harpoon-deploy retry",
            "resume",
            "logs",
            "--force",
            "prune",
        ] {
            assert!(
                !t.contains(&format!("harpoon-deploy {invented}"))
                    && !t.contains("--force")
                    && !t.contains("prune"),
                "{invented}: {t}"
            );
        }
    }

    #[test]
    fn a_simulated_run_has_no_image_to_remove_and_a_first_deployment_says_nothing_served() {
        let (d, app) = failed(FailureKind::Build, Step::Build);
        let empty = Ledger::new();
        let t = lines(&Ctx {
            command: "deploy",
            app: &app,
            deployment: &d,
            ledger: &empty,
            runtime_dir: "/rt",
            simulated: true,
            serving: None,
            paused: false,
        })
        .unwrap()
        .join("\n");
        assert!(
            t.contains("no release was serving")
                && t.contains("the build produced no retained image"),
            "{t}"
        );
        assert!(
            !t.contains("cleanup-image") && t.contains("deploy --simulate --app"),
            "{t}"
        );
    }

    #[test]
    fn a_failure_after_the_cutover_began_gets_no_before_cutover_advice() {
        for (k, s) in [
            (FailureKind::Cutover, Step::Cutover),
            (FailureKind::FinalCheck, Step::FinalCheck),
        ] {
            let (d, app) = failed(k, s);
            let l = Ledger::new();
            assert!(lines(&Ctx {
                command: "replace",
                app: &app,
                deployment: &d,
                ledger: &l,
                runtime_dir: "/rt",
                simulated: false,
                serving: None,
                paused: false
            })
            .is_none());
        }
    }
}
