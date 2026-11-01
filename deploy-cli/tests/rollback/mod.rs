//! Explicit manual rollback through the CLI (simulated only): release listing, exact-identity
//! selection, refusals that change nothing, confirmation, the serving release and trigger pause
//! afterwards, rollback after a restoration, and an interrupted rollback.

use super::*;
use harpoon_deploy::ids::ReleaseId;
use harpoon_deploy::release::ReleaseConfig;
use harpoon_deploy::state::Step;

const APP: &str = "app_00000000000000000000000001";
const OTHER: &str = "app_00000000000000000000000002";
const OLD: &str = "dep_00000000000000000000000010";
const NEW: &str = "dep_00000000000000000000000011";
const OTHER_DEP: &str = "dep_00000000000000000000000020";
const FAILED: &str = "dep_00000000000000000000000012";

fn rel(dep: &str) -> String {
    format!("rel_{}", &dep[4..])
}

/// A succeeded release as a real deployment leaves it: pinned configuration, retained image,
/// container. `at` orders releases; `image` says whether the image is still recorded.
fn release(t: &T, app: &str, dep: &str, at: u64, pin: bool, image: bool) {
    t.docker_runtime_sim();
    let (app, dep): (ApplicationId, DeploymentId) = (app.parse().unwrap(), dep.parse().unwrap());
    let mut d = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, at);
    d.transition(DeploymentStatus::Queued, at + 1).unwrap();
    d.transition(DeploymentStatus::Running, at + 2).unwrap();
    d.release = Some(ReleaseId::from_deployment(&dep));
    if pin {
        let mut c = ReleaseConfig::simulated(38123);
        c.image_id = format!("sha256:{}", &format!("{at:x}").repeat(64)[..64]);
        c.image_tag = format!("harpoon/app:{dep}");
        d.pinned = Some(c);
    }
    d.transition(DeploymentStatus::Succeeded, at + 3).unwrap();
    let name = format!("hp-{dep}");
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&d).unwrap();
    if image {
        let tag = format!("harpoon/app:{dep}");
        j.record(&intent(
            &app,
            &dep,
            ResourceKind::Image,
            &tag,
            Disposition::Retain,
        ))
        .unwrap();
        j.record(&receipt(&dep, ResourceKind::Image, &tag, "img"))
            .unwrap();
    }
    j.record(&intent(
        &app,
        &dep,
        ResourceKind::Container,
        &name,
        Disposition::RemoveOnFailure,
    ))
    .unwrap();
    j.record(&receipt(
        &dep,
        ResourceKind::Container,
        &name,
        &format!("cid-{dep}"),
    ))
    .unwrap();
}

fn crafted() -> T {
    let t = T::new();
    release(&t, APP, OLD, 100, true, true);
    release(&t, APP, NEW, 200, true, true);
    release(&t, OTHER, OTHER_DEP, 150, true, true);
    t
}

fn rollback(t: &T, app: &str, to: &str, extra: &[&str]) -> Outcome {
    let mut a = vec!["rollback", "--simulate", "--app", app, "--to", to];
    a.extend(extra);
    t.cli(&a)
}

fn lines_of(status: &str, app: &str) -> Vec<String> {
    status
        .lines()
        .filter(|l| l.contains(&format!("app={app}")))
        .map(String::from)
        .collect()
}

#[test]
fn releases_lists_the_serving_release_the_targets_and_why_others_are_not() {
    let t = crafted();
    release(&t, APP, "dep_00000000000000000000000009", 50, false, true);
    release(&t, APP, "dep_00000000000000000000000008", 40, true, false);
    let before = t.journal_bytes();
    let o = t.cli(&["releases", "--app", APP]);
    assert_exit(&o, 0);
    let s = &o.stdout;
    assert!(
        s.contains(&format!("serves release {} (deployment {NEW})", rel(NEW))),
        "{s}"
    );
    assert!(
        s.contains(&format!("{} deployment {NEW}", rel(NEW))) && s.contains("SERVING"),
        "{s}"
    );
    assert!(
        s.contains(&format!("{} deployment {OLD}", rel(OLD))) && s.contains("rollback-target"),
        "{s}"
    );
    assert!(s.contains("no pinned configuration was stored"), "{s}");
    assert!(s.contains("retained image is not recorded"), "{s}");
    assert!(
        !s.contains(OTHER_DEP),
        "another application's release is never listed: {s}"
    );
    assert_eq!(t.journal_bytes(), before, "listing changes nothing");
}

#[test]
fn live_rollback_is_refused_before_anything_is_read() {
    let t = T::new();
    let o = t.cli(&["rollback", "--app", APP, "--to", &rel(OLD)]);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("live rollback is not enabled"),
        "{}",
        o.stderr
    );
    assert!(!t.rt.exists());
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn unprovable_targets_are_refused_and_nothing_changes() {
    let t = crafted();
    release(&t, APP, "dep_00000000000000000000000009", 50, false, true);
    release(&t, APP, "dep_00000000000000000000000008", 40, true, false);
    let before = t.journal_bytes();
    for (to, why) in [
        (
            rel("dep_00000000000000000000000077"),
            "is not a succeeded release of application",
        ),
        (rel(OTHER_DEP), "is not a succeeded release of application"), // another application's release
        (rel(NEW), "serving now"),
        (
            rel("dep_00000000000000000000000009"),
            "no pinned configuration",
        ),
        (rel("dep_00000000000000000000000008"), "retained image"),
    ] {
        let o = rollback(&t, APP, &to, &["--confirm-rollback"]);
        assert_exit(&o, 6);
        assert!(o.stderr.contains(why), "{to}: {}", o.stderr);
        assert!(
            o.stderr
                .contains("Nothing was prepared, stopped or changed"),
            "{}",
            o.stderr
        );
        assert_eq!(t.journal_bytes(), before, "{to}");
    }
    // a name or tag is not an identity
    let o = rollback(&t, APP, "previous", &[]);
    assert_exit(&o, 2);
    assert_eq!(t.journal_bytes(), before);
}

#[test]
fn without_confirmation_the_target_is_prepared_and_the_serving_release_is_untouched() {
    let t = crafted();
    let o = rollback(&t, APP, &rel(OLD), &[]);
    assert_exit(&o, 0);
    for want in [
        "Application rollback only. Data, volumes and database schema are NOT rolled back.",
        &format!("serving now: release {} (deployment {NEW})", rel(NEW)),
        &format!("rolling back to: release {} (deployment {OLD})", rel(OLD)),
        "anticipated interruption",
        "rollback not confirmed",
        "serving release untouched",
    ] {
        assert!(o.stdout.contains(want), "missing {want:?}:\n{}", o.stdout);
    }
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!("runs release {} (deployment {NEW})", rel(NEW))),
        "{s}"
    );
    assert!(!s.contains("automatic triggers paused"), "{s}");
    assert_exit(&t.cli(&["doctor"]), 0);
}

#[test]
fn a_confirmed_rollback_serves_the_earlier_release_pauses_triggers_and_leaves_others_alone() {
    let t = crafted();
    let s0 = t.cli(&["status"]).stdout;
    let o = rollback(&t, APP, &rel(OLD), &["--confirm-rollback"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains(
            "Application rollback only. Data, volumes and database schema are NOT rolled back."
        ),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("measured interruption:"), "{}", o.stdout);
    assert!(
        o.stdout.contains(&format!(
            "rolled back: application {APP} serves release {}",
            rel(OLD)
        )),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("automatic triggers are paused"),
        "{}",
        o.stdout
    );

    // everything below is read back from SQLite in a fresh process
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!(
            "serving: application {APP} runs release {} (deployment {OLD})",
            rel(OLD)
        )),
        "{s}"
    );
    assert!(s.contains("(rolled back by dep_"), "{s}");
    assert!(
        s.contains(&format!("automatic triggers paused for application {APP}"))
            && s.contains(&format!(
                "rolled the application back to release {}",
                rel(OLD)
            )),
        "{s}"
    );
    assert!(
        s.contains(&format!(
            "rollback to release {}, replacing {}",
            rel(OLD),
            rel(NEW)
        )),
        "{s}"
    );
    assert!(
        !s.contains(&format!("paused for application {OTHER}")),
        "{s}"
    );
    assert_eq!(
        lines_of(&s0, OTHER),
        lines_of(&s, OTHER),
        "the other application is untouched"
    );
    assert_exit(&t.cli(&["doctor"]), 0);

    // moving forward again is a replacement, not a rollback
    let o = rollback(&t, APP, &rel(NEW), &["--confirm-rollback"]);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("not earlier"), "{}", o.stderr);
    // and the releases listing now names the rolled-back-to release as serving
    let l = t.cli(&["releases", "--app", APP]).stdout;
    assert!(
        l.contains(&format!("serves release {} (deployment {OLD})", rel(OLD))),
        "{l}"
    );
}

#[test]
fn a_rollback_after_a_restored_deployment_replaces_the_restored_container() {
    let t = T::new();
    release(&t, APP, "dep_00000000000000000000000009", 50, true, true);
    release(&t, APP, OLD, 100, true, true);
    // NEW replaced OLD and failed its cutover; OLD was restored into a container recorded under NEW
    let (app, failed, old): (ApplicationId, DeploymentId, DeploymentId) = (
        APP.parse().unwrap(),
        FAILED.parse().unwrap(),
        OLD.parse().unwrap(),
    );
    let mut d = Deployment::new(failed.clone(), app.clone(), Trigger::Cli, 500);
    d.transition(DeploymentStatus::Queued, 501).unwrap();
    d.transition(DeploymentStatus::Running, 502).unwrap();
    d.start_step(Step::Cutover, 503).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&old));
    d.begin_cutover(504).unwrap();
    d.fail_cutover(505).unwrap();
    d.start_step(Step::Recovery, 506).unwrap();
    d.start_recovery(60_000, 506).unwrap();
    d.finish_recovery(harpoon_deploy::state::RecoveryOutcome::Restored, 900)
        .unwrap();
    d.transition(DeploymentStatus::Failed, 901).unwrap();
    let name = harpoon_deploy::serving::restored_container_name(&failed);
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.persist(&d).unwrap();
        j.record(&intent(
            &app,
            &failed,
            ResourceKind::Container,
            &name,
            Disposition::Retain,
        ))
        .unwrap();
        j.record(&receipt(
            &failed,
            ResourceKind::Container,
            &name,
            "cid-restored",
        ))
        .unwrap();
    }
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!(
            "runs release {} (deployment {OLD}) in container {name}",
            rel(OLD)
        )),
        "{s}"
    );
    let o = rollback(
        &t,
        APP,
        &rel("dep_00000000000000000000000009"),
        &["--confirm-rollback"],
    );
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains(&format!(
            "serving now: release {} (deployment {OLD}), container {name} of deployment {FAILED}",
            rel(OLD)
        )),
        "{}",
        o.stdout
    );
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!(
            "runs release {} (deployment dep_00000000000000000000000009)",
            rel("dep_00000000000000000000000009")
        )),
        "{s}"
    );
    assert_exit(&t.cli(&["doctor"]), 0);
}

#[test]
fn an_interrupted_rollback_is_never_resumed_or_repeated_and_blocks_further_changes() {
    let t = crafted();
    // a rollback that died after it stopped the serving release
    let (app, dep): (ApplicationId, DeploymentId) = (
        APP.parse().unwrap(),
        "dep_00000000000000000000000030".parse().unwrap(),
    );
    let mut d = Deployment::new(dep, app, Trigger::Cli, 300);
    d.rollback_to = Some(ReleaseId::from_deployment(&OLD.parse().unwrap()));
    d.transition(DeploymentStatus::Queued, 301).unwrap();
    d.transition(DeploymentStatus::Running, 302).unwrap();
    d.start_step(Step::Cutover, 303).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&NEW.parse().unwrap()));
    d.begin_cutover(304).unwrap();
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.persist(&d).unwrap();
    }
    assert_exit(&t.cli(&["recover"]), 0);
    let s = t.cli(&["status"]).stdout;
    assert!(s.contains("MAY BE DOWN"), "{s}");
    assert!(
        s.contains(&format!("rollback to release {}", rel(OLD))),
        "{s}"
    );
    assert!(
        s.contains("restore --deployment dep_00000000000000000000000030"),
        "{s}"
    );
    // no new rollback (or replacement) while the application may be down
    let before = t.journal_bytes();
    let o = rollback(&t, APP, &rel(OLD), &["--confirm-rollback"]);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("may be down"), "{}", o.stderr);
    assert_eq!(t.journal_bytes(), before);
    // the existing explicit bounded recovery restores the release that was serving, once
    let o = t.cli(&[
        "restore",
        "--simulate",
        "--deployment",
        "dep_00000000000000000000000030",
        "--apply",
    ]);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("recovery: restored"), "{}", o.stdout);
    let o = t.cli(&[
        "restore",
        "--simulate",
        "--deployment",
        "dep_00000000000000000000000030",
        "--apply",
    ]);
    assert_exit(&o, 6);
    assert!(
        o.stderr
            .contains("already has a recorded restoration attempt"),
        "{}",
        o.stderr
    );
}
