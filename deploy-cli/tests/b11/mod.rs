//! Bounded cutover recovery through the CLI (simulated only): pause visibility, the explicit
//! `restore` command for an interrupted cutover, one attempt only, and honest outcomes.

use super::*;
use harpoon_deploy::ids::ReleaseId;
use harpoon_deploy::state::Step;

fn app_of(o: &Outcome) -> String {
    let after = o
        .stdout
        .split("(application ")
        .nth(1)
        .expect("an application id");
    after.split(')').next().unwrap().to_string()
}

/// Shared with the hardening tests.
pub fn earlier_release_for_tests(t: &T, app: &str, dep: &str, pin: bool) {
    earlier_release(t, app, dep, pin)
}

/// A succeeded release of `app` as an earlier deployment left it: record and serving container.
/// `pin` says whether its configuration was pinned (a release from before pinning has none).
fn earlier_release(t: &T, app: &str, dep: &str, pin: bool) {
    t.docker_runtime_sim();
    let (app, dep): (ApplicationId, DeploymentId) = (app.parse().unwrap(), dep.parse().unwrap());
    let mut d = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, 100);
    d.transition(DeploymentStatus::Queued, 101).unwrap();
    d.transition(DeploymentStatus::Running, 102).unwrap();
    d.release = Some(ReleaseId::from_deployment(&dep));
    if pin {
        d.pinned = Some(harpoon_deploy::release::ReleaseConfig::simulated(38123));
    }
    d.transition(DeploymentStatus::Succeeded, 103).unwrap();
    let name = format!("hp-{dep}");
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&d).unwrap();
    j.record(&LedgerRecord::Intent {
        key: ResourceKey::new(ResourceKind::Container, &name),
        app,
        deployment: dep.clone(),
        disposition: Disposition::RemoveOnFailure,
        at_ms: 100,
    })
    .unwrap();
    j.record(&LedgerRecord::Receipt {
        key: ResourceKey::new(ResourceKind::Container, &name),
        deployment: dep,
        id: "cid-1".into(),
        at_ms: 101,
    })
    .unwrap();
}

/// A replacement that died after stopping the release: cutover begun, never confirmed.
fn dead_cutover(t: &T, app: &str, dep: &str, replaces: &str) {
    let (app, dep): (ApplicationId, DeploymentId) = (app.parse().unwrap(), dep.parse().unwrap());
    let mut d = Deployment::new(dep, app, Trigger::Cli, 200);
    d.transition(DeploymentStatus::Queued, 201).unwrap();
    d.transition(DeploymentStatus::Running, 202).unwrap();
    d.start_step(Step::Cutover, 203).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&replaces.parse().unwrap()));
    d.begin_cutover(204).unwrap();
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&d).unwrap();
}

const APP: &str = "app_00000000000000000000000001";
const OTHER: &str = "app_00000000000000000000000002";
const PREV: &str = "dep_00000000000000000000000010";
const OTHER_PREV: &str = "dep_00000000000000000000000020";
const DEAD: &str = "dep_00000000000000000000000011";

fn restore(t: &T, dep: &str, extra: &[&str]) -> Outcome {
    let mut a = vec!["restore", "--simulate", "--deployment", dep];
    a.extend(extra);
    t.cli(&a)
}

fn crafted() -> T {
    let t = T::new();
    earlier_release(&t, APP, PREV, true);
    earlier_release(&t, OTHER, OTHER_PREV, true);
    dead_cutover(&t, APP, DEAD, PREV);
    assert_exit(&t.cli(&["recover"]), 0);
    t
}

fn lines_of(status: &str, app: &str) -> Vec<String> {
    status
        .lines()
        .filter(|l| l.contains(&format!("app={app}")))
        .map(String::from)
        .collect()
}

#[test]
fn builds_and_checks_that_fail_never_pause_and_a_dead_cutover_does() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let s = t.cli(&["status"]).stdout;
    assert!(!s.contains("automatic triggers paused"), "{s}");

    let t = crafted();
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!("automatic triggers paused for application {APP}: deployment {DEAD} began a cutover that never finished")),
        "{s}"
    );
    assert!(
        !s.contains(&format!("paused for application {OTHER}")),
        "the other application is not paused: {s}"
    );
    assert!(s.contains("the application may be down"), "{s}");
    assert!(
        s.contains(&format!("restore --deployment {DEAD} --simulate")),
        "the explicit recovery is offered: {s}"
    );
}

#[test]
fn live_restoration_is_refused_before_anything_is_read() {
    let t = T::new();
    let o = t.cli(&["restore", "--deployment", DEAD]);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("live restoration is not enabled"),
        "{}",
        o.stderr
    );
    assert!(!t.rt.exists());
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn the_dry_run_shows_the_attempt_and_changes_nothing() {
    let t = crafted();
    let before = t.journal_bytes();
    let o = restore(&t, DEAD, &[]);
    assert_exit(&o, 0);
    for want in [
        "cutover began and was never confirmed",
        &format!("previous release: rel_{} (deployment {PREV})", &PREV[4..]),
        "pinned configuration: image sha256:",
        "one attempt, budget 60000 ms, never repeated",
        "dry run: nothing was changed",
    ] {
        assert!(o.stdout.contains(want), "missing {want:?}:\n{}", o.stdout);
    }
    assert_eq!(t.journal_bytes(), before);
}

#[test]
fn the_explicit_recovery_runs_once_records_its_outcome_and_leaves_other_applications_alone() {
    let t = crafted();
    let s0 = t.cli(&["status"]).stdout;
    let o = restore(&t, DEAD, &["--apply"]);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("recovery: restored: the previous release started and passed its own health check in 400 ms (budget 60000 ms)"), "{}", o.stdout);
    assert!(o.stdout.contains("measured interruption:"), "{}", o.stdout);
    let s = t.cli(&["status"]).stdout;
    assert!(s.contains("recovery: restored in 400 ms"), "{s}");
    assert!(
        !s.contains("restore --deployment"),
        "no second attempt is offered: {s}"
    );
    assert!(
        s.contains("recovery restored"),
        "the pause line carries the outcome: {s}"
    );
    // the deployment is still the interrupted one it was; nothing else about the application moved
    assert!(
        lines_of(&s, APP)
            .iter()
            .any(|l| l.contains(DEAD) && l.contains("interrupted")),
        "{s}"
    );
    assert_eq!(
        lines_of(&s0, OTHER),
        lines_of(&s, OTHER),
        "the other application is untouched"
    );

    // never twice
    let before = t.journal_bytes();
    let o = restore(&t, DEAD, &["--apply"]);
    assert_exit(&o, 6);
    assert!(
        o.stderr
            .contains("already has a recorded restoration attempt"),
        "{}",
        o.stderr
    );
    assert_eq!(t.journal_bytes(), before);
    assert_exit(&t.cli(&["doctor"]), 0);
}

#[test]
fn a_release_without_a_pinned_configuration_is_not_attempted_and_says_why() {
    let t = T::new();
    earlier_release(&t, APP, PREV, false);
    dead_cutover(&t, APP, DEAD, PREV);
    assert_exit(&t.cli(&["recover"]), 0);
    let dry = restore(&t, DEAD, &[]);
    assert_exit(&dry, 0);
    assert!(dry.stdout.contains("none stored"), "{}", dry.stdout);
    let o = restore(&t, DEAD, &["--apply"]);
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains(
            "recovery: not_attempted: the previous release's pinned configuration was never stored"
        ),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("the application may be down"),
        "{}",
        o.stdout
    );
    let s = t.cli(&["status"]).stdout;
    assert!(s.contains("the application may be down"), "{s}");
    assert!(
        !s.contains("restore --deployment"),
        "not offered again: {s}"
    );
}

#[test]
fn only_an_interrupted_unrecovered_cutover_can_be_restored() {
    let t = crafted();
    // a succeeded release, a deployment that does not exist, and a first release with no earlier one
    for (dep, why) in [
        (PREV, "only an interrupted deployment"),
        ("dep_0000000000000000000000ffff", "not a deployment id"),
    ] {
        let o = restore(&t, dep, &["--apply"]);
        assert!(o.code == 6 || o.code == 2, "{}", o.stderr);
        assert!(
            o.stderr.contains(why) || o.stderr.contains("not in the journal"),
            "{}",
            o.stderr
        );
    }
    let solo = T::new();
    solo.docker_runtime_sim();
    dead_cutover(&solo, OTHER, DEAD, OTHER_PREV);
    assert_exit(&solo.cli(&["recover"]), 0);
    let o = restore(&solo, DEAD, &["--apply"]);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("is not in the journal"), "{}", o.stderr);
}

#[test]
fn a_restoration_that_died_part_way_is_closed_as_interrupted_and_never_repeated() {
    let t = T::new();
    earlier_release(&t, APP, PREV, true);
    dead_cutover(&t, APP, DEAD, PREV);
    assert_exit(&t.cli(&["recover"]), 0);
    // the explicit recovery recorded its attempt, then the process died before the result
    {
        let (mut j, r) = open_j(&t.journal_dir()).unwrap();
        let mut d = r
            .deployments
            .iter()
            .find(|d| d.id.as_str() == DEAD)
            .unwrap()
            .clone();
        d.begin_explicit_recovery(60_000, 300).unwrap();
        j.persist(&d).unwrap();
    }
    let o = t.cli(&["recover"]);
    assert_exit(&o, 0);
    let s = t.cli(&["status"]).stdout;
    assert!(s.contains("recovery: restore_failed"), "{s}");
    assert!(s.contains("the application may be down"), "{s}");
    let before = t.journal_bytes();
    let o = restore(&t, DEAD, &["--apply"]);
    assert_exit(&o, 6);
    assert!(
        o.stderr
            .contains("already has a recorded restoration attempt"),
        "{}",
        o.stderr
    );
    assert_eq!(t.journal_bytes(), before);
    // and a replacement stays blocked while the application may be down
    let o = {
        let src = t.src();
        let mut a: Vec<&str> = vec!["replace", "--simulate", "--app", APP];
        a.extend(src.iter().map(String::as_str));
        t.cli(&a)
    };
    assert_exit(&o, 6);
    assert!(o.stderr.contains("may be down"), "{}", o.stderr);
}

#[test]
fn a_simulated_replacement_pins_each_release_so_it_could_be_restored_later() {
    let t = T::new();
    let o = t.sim_deploy();
    assert_exit(&o, 0);
    let app = app_of(&o);
    let src = t.src();
    let mut a: Vec<&str> = vec!["replace", "--simulate", "--app", &app, "--confirm-cutover"];
    a.extend(src.iter().map(String::as_str));
    let o = t.cli(&a);
    assert_exit(&o, 0);
    let bytes = String::from_utf8_lossy(&t.journal_bytes()).to_string();
    assert!(
        bytes.matches("v1;img=").count() >= 2,
        "both releases carry a pin"
    );
    assert!(!bytes.contains("password") && !bytes.contains("secret"));
}
