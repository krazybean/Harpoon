//! `status` tells the truth about an unfinished cutover: a journal status of `running` is a record,
//! not a live process or a healthy application; recovery is explicit; a restored service is not
//! reported as down; nothing here changes a record or contacts Docker.

use super::*;

fn open_cutover() -> T {
    let t = T::new();
    earlier_release(&t, APP, PREV, true);
    dead_cutover(&t, APP, DEAD, PREV);
    t
}

#[test]
fn an_open_cutover_is_unverified_and_possibly_down_before_recover_is_ever_run() {
    let t = open_cutover();
    let before = t.journal_bytes();
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!("{DEAD} running")),
        "the record still says running: {s}"
    );
    assert!(
        s.contains("completion is UNVERIFIED and the application MAY BE DOWN"),
        "{s}"
    );
    assert!(
        s.contains("not proof of a live process") && s.contains("no writer holds the journal"),
        "{s}"
    );
    assert!(
        s.contains("per the records only") && s.contains("application DOWN"),
        "serving identity is record-derived: {s}"
    );
    assert!(
        s.contains("`recover` marks it interrupted (a change to the record only)"),
        "{s}"
    );
    assert!(
        !s.contains("restore --deployment"),
        "restore is offered only after recover: {s}"
    );
    assert_eq!(t.journal_bytes(), before, "status changed nothing");
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn while_a_writer_holds_the_journal_status_does_not_call_it_dead_and_warns_against_recover() {
    let t = open_cutover();
    let (_writer, _) = open_j(&t.journal_dir()).unwrap();
    let s = t.cli(&["status"]).stdout;
    assert!(s.contains("MAY BE DOWN"), "{s}");
    assert!(
        s.contains("a writer holds the journal now, so it may still be working"),
        "{s}"
    );
    assert!(s.contains("do not run `recover` while it is active"), "{s}");
    assert!(!s.contains("probably gone"), "{s}");
}

#[test]
fn recover_is_the_explicit_transition_and_the_hint_matches_the_runtime() {
    let t = open_cutover();
    let o = t.cli(&["recover"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains(&format!("marked interrupted: {DEAD}")),
        "{}",
        o.stdout
    );
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!("{DEAD} interrupted")) && s.contains("MAY BE DOWN"),
        "{s}"
    );
    assert!(
        s.contains(&format!("restore --deployment {DEAD} --simulate")),
        "a simulated runtime is offered the simulated command: {s}"
    );
    // a Docker runtime must not be told to use --simulate, nor that a shipping build can restore it
    std::fs::write(t.rt.join("runtime-kind"), "docker\n").unwrap();
    let before = t.journal_bytes();
    let s = t.cli(&["status"]).stdout;
    assert!(!s.contains("--simulate"), "{s}");
    assert!(
        s.contains("this build cannot restore a live runtime")
            && s.contains("nothing restores it automatically"),
        "{s}"
    );
    assert_eq!(t.journal_bytes(), before);
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn after_a_successful_restoration_the_history_is_kept_and_the_service_is_not_called_down() {
    let t = crafted();
    let o = restore(&t, DEAD, &["--apply"]);
    assert_exit(&o, 0);
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!("{DEAD} interrupted")),
        "history kept: {s}"
    );
    assert!(s.contains("recovery: restored"), "{s}");
    assert!(
        s.contains("its candidate was never confirmed as the successful serving release")
            && s.contains("was restored by an explicit restoration")
            && !s.contains("never served"),
        "{s}"
    );
    assert!(
        !s.contains("MAY BE DOWN") && !s.contains("per the records only"),
        "{s}"
    );
    assert!(
        s.contains(&format!("(restored by {DEAD})")),
        "serving identity: {s}"
    );
    assert!(
        s.contains(&format!(
            "automatic triggers paused for application {APP}: deployment {DEAD}"
        )) && s.contains("recovery restored"),
        "the pause is kept: {s}"
    );
    assert!(!s.contains(&format!("{DEAD} succeeded")), "{s}");
    // a second restoration is refused and changes nothing
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
    // and status itself is read-only
    let _ = t.cli(&["status"]);
    assert_eq!(t.journal_bytes(), before);
}

#[test]
fn a_healthy_deployment_reads_exactly_as_before() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let s = t.cli(&["status"]).stdout;
    for bad in [
        "MAY BE DOWN",
        "UNVERIFIED",
        "attention:",
        "per the records only",
        "not proof",
    ] {
        assert!(!s.contains(bad), "{bad}: {s}");
    }
    assert!(s.contains("serving: application"), "{s}");
}

#[test]
fn a_missing_store_does_not_claim_that_nothing_was_deployed() {
    let t = T::new();
    let o = t.cli(&["status"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains("not initialised: no deployment store exists at"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("does not show that nothing was deployed"),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains("nothing has been deployed"),
        "{}",
        o.stdout
    );
    assert!(!t.rt.exists(), "nothing was created");
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn an_automatic_recovery_is_not_called_explicit_and_the_header_and_notes_agree_on_the_lock() {
    use harpoon_deploy::state::{RecoveryOutcome, Step};
    let t = T::new();
    earlier_release(&t, APP, PREV, true);
    let (app, dep): (ApplicationId, DeploymentId) = (APP.parse().unwrap(), DEAD.parse().unwrap());
    let mut d = Deployment::new(dep, app, Trigger::Cli, 200);
    d.transition(DeploymentStatus::Queued, 201).unwrap();
    d.transition(DeploymentStatus::Running, 202).unwrap();
    d.start_step(Step::Cutover, 203).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&PREV.parse().unwrap()));
    d.begin_cutover(204).unwrap();
    d.fail_cutover(205).unwrap();
    d.start_recovery(60_000, 206).unwrap();
    d.finish_recovery(RecoveryOutcome::Restored, 207).unwrap();
    d.transition(DeploymentStatus::Failed, 208).unwrap();
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.persist(&d).unwrap();
    }
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains("restored by the automatic bounded recovery of that run"),
        "{s}"
    );
    assert!(
        !s.contains("explicit restoration")
            && !s.contains("never served")
            && !s.contains("MAY BE DOWN"),
        "{s}"
    );
    // with a writer holding the journal, the header and the notes tell the same story
    let (_w, _) = open_j(&t.journal_dir()).unwrap();
    let held = t.cli(&["status"]).stdout;
    assert!(held.contains("(lock: held by a writer)"), "{held}");
}

#[test]
fn a_live_runtime_is_not_offered_simulated_or_unavailable_commands_by_releases() {
    let t = open_cutover();
    std::fs::write(t.rt.join("runtime-kind"), "docker\n").unwrap();
    let o = t.cli(&["releases", "--app", APP]);
    assert!(!o.stdout.contains("--simulate"), "{}", o.stdout);
    assert!(
        o.stdout
            .contains("this build cannot roll back a live runtime"),
        "{}",
        o.stdout
    );
}
