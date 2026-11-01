//! B14: durable pauses, held events and explicit resume through the real CLI. Events enter only
//! through the engine's ingress function (there is no event source in this milestone), written by
//! the test as a future integration would. Offline; the fake Docker is only used to prove it is
//! never touched.

use super::*;
use harpoon_deploy::control::{self, ControlState, Incoming, Ingress};
use harpoon_deploy::ids::ReleaseId;
use harpoon_deploy::state::Step;

const APP: &str = "app_00000000000000000000000001";
const OTHER: &str = "app_00000000000000000000000002";

fn dep_id(n: u32) -> DeploymentId {
    format!("dep_0000000000000000000000{n:04}").parse().unwrap()
}

/// A deployment whose cutover began, failed and was restored (the record a failed replacement leaves).
fn failed_cutover(t: &T, app: &str, n: u32, at: u64) {
    t.docker_runtime_sim();
    let mut d = Deployment::new(dep_id(n), app.parse().unwrap(), Trigger::Cli, at);
    d.transition(DeploymentStatus::Queued, at + 1).unwrap();
    d.transition(DeploymentStatus::Running, at + 2).unwrap();
    d.start_step(Step::Cutover, at + 3).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&dep_id(1)));
    d.begin_cutover(at + 4).unwrap();
    d.fail_cutover(at + 5).unwrap();
    d.start_recovery(60_000, at + 6).unwrap();
    d.finish_recovery(harpoon_deploy::state::RecoveryOutcome::Restored, at + 7)
        .unwrap();
    d.transition(DeploymentStatus::Failed, at + 8).unwrap();
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&d).unwrap();
}

/// What a future trigger integration does: hand each event to the engine's ingress.
fn ingest(t: &T, app: &str, events: &[(&str, &str, u64)]) -> Vec<Ingress> {
    let (mut j, r) = open_j(&t.journal_dir()).unwrap();
    let mut ctl: ControlState = r.control.clone();
    let app: ApplicationId = app.parse().unwrap();
    events
        .iter()
        .map(|(d, rev, ms)| {
            control::ingest(
                &mut *j,
                &r.deployments,
                &mut ctl,
                Incoming {
                    app: &app,
                    delivery_id: d,
                    source: "webhook",
                    revision: rev,
                    received_ms: *ms,
                },
            )
            .unwrap()
        })
        .collect()
}

fn resume(t: &T, app: &str, extra: &[&str]) -> Outcome {
    let mut a = vec!["resume", "--app", app];
    a.extend(extra);
    t.cli(&a)
}

fn control_of(t: &T) -> ControlState {
    harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir())
        .unwrap()
        .control
}

#[test]
fn a_failed_cutover_creates_a_durable_pause_that_any_writer_records_once() {
    let t = T::new();
    failed_cutover(&t, APP, 10, 100);
    // before any writer has run, the pause is in force but only derived
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains("derived from the deployment record, not yet written"),
        "{s}"
    );
    let o = t.cli(&["recover"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("recorded 1 pause record(s)"),
        "{}",
        o.stdout
    );
    let again = t.cli(&["recover"]);
    assert!(
        !again.stdout.contains("recorded 1 pause"),
        "{}",
        again.stdout
    );
    let c = control_of(&t);
    assert_eq!(c.pauses.len(), 1);
    assert_eq!(
        c.pauses[0].at_ms, 105,
        "the time the cutover failed, from the deployment record"
    );
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!("automatic triggers paused for application {APP}: deployment {} failed its cutover (recovery restored); since 105 ms (epoch), recorded", dep_id(10))),
        "{s}"
    );
    assert!(
        s.contains(&format!("application {APP}: PAUSED — held events: 0")),
        "{s}"
    );
    assert!(s.contains("harpoon-deploy resume --app"), "{s}");
}

#[test]
fn a_successful_rollback_records_a_pause_and_a_preliminary_or_unconfirmed_run_records_none() {
    let t = T::new();
    let src = t.src();
    let mut a: Vec<&str> = vec!["deploy", "--simulate"];
    a.extend(src.iter().map(String::as_str));
    let o = t.cli(&a);
    assert_exit(&o, 0);
    let app = o
        .stdout
        .split("(application ")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap()
        .to_string();
    // an unconfirmed replacement never begins a cutover: no pause
    let mut r: Vec<&str> = vec!["replace", "--simulate", "--app", &app];
    r.extend(src.iter().map(String::as_str));
    assert_exit(&t.cli(&r), 0);
    assert!(control_of(&t).pauses.is_empty());
    assert!(!t
        .cli(&["status"])
        .stdout
        .contains("automatic triggers paused"));
    // a confirmed one and a rollback to the first release
    r.push("--confirm-cutover");
    assert_exit(&t.cli(&r), 0);
    assert!(
        control_of(&t).pauses.is_empty(),
        "a successful replacement does not pause"
    );
    let rel = {
        let rs = harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir()).unwrap();
        let mut d = rs.deployments;
        d.sort_by_key(|d| d.created_ms);
        d[0].release.clone().unwrap()
    };
    let o = t.cli(&[
        "rollback",
        "--simulate",
        "--app",
        &app,
        "--to",
        rel.as_str(),
        "--confirm-rollback",
    ]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("recorded 1 pause record(s)"),
        "{}",
        o.stdout
    );
    let c = control_of(&t);
    assert_eq!(c.pauses.len(), 1);
    assert_eq!(
        c.pauses[0].kind,
        harpoon_deploy::control::PauseKind::Rollback
    );
    let e = t.cli(&["events"]).stdout;
    assert!(
        e.contains("rollback since") && e.contains("in force"),
        "{e}"
    );
}

#[test]
fn a_preliminary_failure_records_no_pause() {
    let t = T::new();
    image_appears_after_build(&t);
    let a = live_args(&t);
    let o = t.go(&OPEN, &a.iter().map(String::as_str).collect::<Vec<_>>());
    assert_exit(&o, 7);
    assert!(control_of(&t).pauses.is_empty());
    let s = t.cli(&["status"]).stdout;
    assert!(
        !s.contains("automatic triggers paused") && !s.contains("PAUSED"),
        "{s}"
    );
    assert!(t.cli(&["events"]).stdout.contains("none recorded"));
}

#[test]
fn events_held_while_paused_are_listed_deduplicated_isolated_and_survive_a_restart() {
    let t = T::new();
    failed_cutover(&t, APP, 10, 100);
    failed_cutover(&t, OTHER, 20, 100);
    let got = ingest(
        &t,
        APP,
        &[
            ("d1", "aaa", 300),
            ("d2", "bbb", 310),
            ("d1", "aaa", 300),
            ("d3", "ccc", 320),
        ],
    );
    assert_eq!(
        got,
        [
            Ingress::Held,
            Ingress::Held,
            Ingress::Duplicate,
            Ingress::Held
        ]
    );
    assert_eq!(ingest(&t, OTHER, &[("d1", "zzz", 330)]), [Ingress::Held]);
    let c = control_of(&t);
    assert_eq!(c.events.len(), 4);
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!("application {APP}: PAUSED — held events: 3")),
        "{s}"
    );
    assert!(
        s.contains("held: revision bbb delivery d2 source webhook received 310 ms"),
        "{s}"
    );
    assert!(
        s.contains(&format!("application {OTHER}: PAUSED — held events: 1")),
        "{s}"
    );
    // a fresh process (every CLI call is one) sees the identical history
    assert_eq!(t.cli(&["events"]).stdout, t.cli(&["events"]).stdout);
    let one = t.cli(&["events", "--app", OTHER]).stdout;
    assert!(
        one.contains("delivery d1 source webhook revision zzz") && !one.contains("revision aaa"),
        "{one}"
    );
}

#[test]
fn resume_newest_selects_without_deploying_and_keeps_the_application_blocked() {
    let t = T::new();
    failed_cutover(&t, APP, 10, 100);
    ingest(&t, APP, &[("d1", "aaa", 300), ("d2", "bbb", 310)]);
    let o = resume(&t, APP, &["--newest"]);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("selected revision bbb (delivery d2); 1 other held event(s) were recorded as superseded"), "{}", o.stdout);
    assert!(
        o.stdout.contains("NOTHING HAS BEEN DEPLOYED")
            && o.stdout.contains("pending attended execution"),
        "{}",
        o.stdout
    );
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains("PENDING ATTENDED EXECUTION: revision bbb (delivery d2)")
            && s.contains("NOTHING HAS BEEN DEPLOYED"),
        "{s}"
    );
    assert!(
        s.contains(&format!("application {APP}: PAUSED — held events: 0")),
        "{s}"
    );
    assert!(
        s.contains("history: revision aaa delivery d1 superseded by delivery d2 (newer_selected)"),
        "{s}"
    );
    assert!(
        s.contains(&format!("resume --app {APP} --dismiss")) && !s.contains("--newest |"),
        "only dismissal is offered: {s}"
    );
    assert!(
        !s.contains("serving: application"),
        "a recorded decision is never a serving claim: {s}"
    );
    // events arriving now are still held; choosing again is refused; the same revision is a no-op
    assert_eq!(ingest(&t, APP, &[("d3", "ccc", 400)]), [Ingress::Held]);
    let before = t.journal_bytes();
    assert_exit(&resume(&t, APP, &["--newest"]), 6);
    assert_exit(&resume(&t, APP, &["--revision", "ccc"]), 6);
    let same = resume(&t, APP, &["--revision", "bbb"]);
    assert_exit(&same, 0);
    assert!(same.stdout.contains("already selected"), "{}", same.stdout);
    assert_eq!(t.journal_bytes(), before);
    // dismissal abandons the selection, clears the pause and keeps every event in the history
    let o = resume(&t, APP, &["--dismiss"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("dismissed 2 event(s)") && o.stdout.contains("cleared 1 pause(s)"),
        "{}",
        o.stdout
    );
    let s = t.cli(&["status"]).stdout;
    assert!(
        !s.contains("automatic triggers paused")
            && s.contains(&format!("application {APP}: not paused")),
        "{s}"
    );
    assert!(
        s.contains("history: revision bbb delivery d2 dismissed")
            && s.contains("history: revision ccc delivery d3 dismissed"),
        "{s}"
    );
    assert_eq!(control_of(&t).events.len(), 3);
    // repeating the dismissal writes nothing
    let before = t.journal_bytes();
    let again = resume(&t, APP, &["--dismiss"]);
    assert_exit(&again, 0);
    assert!(
        again.stdout.contains("nothing to resume"),
        "{}",
        again.stdout
    );
    assert_eq!(t.journal_bytes(), before);
}

#[test]
fn resume_with_an_explicit_revision_refuses_a_revision_that_is_not_held_without_changing_state() {
    let t = T::new();
    failed_cutover(&t, APP, 10, 100);
    ingest(
        &t,
        APP,
        &[("d1", "aaa", 300), ("d2", "bbb", 310), ("d3", "ccc", 320)],
    );
    t.cli(&["recover"]);
    let before = t.journal_bytes();
    let o = resume(&t, APP, &["--revision", "nope"]);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("no held event has revision nope")
            && o.stderr.contains("held revisions: aaa, bbb, ccc"),
        "{}",
        o.stderr
    );
    assert_exit(&resume(&t, APP, &["--revision", "bad rev"]), 2);
    assert_eq!(t.journal_bytes(), before, "a refusal changes nothing");
    let o = resume(&t, APP, &["--revision", "bbb"]);
    assert_exit(&o, 0);
    let c = control_of(&t);
    use harpoon_deploy::control::{EventStatus, SupersededWhy};
    assert_eq!(
        c.event(&APP.parse().unwrap(), "d2").unwrap().status,
        EventStatus::Selected
    );
    assert_eq!(
        c.event(&APP.parse().unwrap(), "d1").unwrap().status,
        EventStatus::Superseded(SupersededWhy::NotChosen)
    );
    assert_eq!(
        c.event(&APP.parse().unwrap(), "d3").unwrap().status,
        EventStatus::Superseded(SupersededWhy::NotChosen)
    );
}

#[test]
fn a_decision_needs_exactly_one_explicit_choice_and_never_guesses() {
    let t = T::new();
    failed_cutover(&t, APP, 10, 100);
    ingest(&t, APP, &[("d1", "aaa", 300)]);
    let before = t.journal_bytes();
    assert_exit(&resume(&t, APP, &[]), 2);
    assert_exit(&resume(&t, APP, &["--dismiss", "--newest"]), 2);
    assert_exit(&resume(&t, APP, &["--newest", "--revision", "aaa"]), 2);
    assert_exit(&t.cli(&["resume", "--dismiss"]), 2);
    assert_exit(&t.cli(&["resume", "--app", "not-an-id", "--dismiss"]), 2);
    assert_eq!(t.journal_bytes(), before);
    // an application with no store at all
    let empty = T::new();
    assert_exit(&resume(&empty, APP, &["--dismiss"]), 6);
    assert!(!empty.rt.exists(), "a refusal creates nothing");
}

#[test]
fn a_new_failed_cutover_after_a_resume_pauses_again_and_the_old_one_is_not_re_derived() {
    let t = T::new();
    failed_cutover(&t, APP, 10, 100);
    ingest(&t, APP, &[("d1", "aaa", 300)]);
    assert_exit(&resume(&t, APP, &["--dismiss"]), 0);
    // restart: the old deployment is still in the history, and the pause does not come back
    for _ in 0..2 {
        let s = t.cli(&["status"]).stdout;
        assert!(!s.contains("automatic triggers paused"), "{s}");
        assert!(s.contains(&format!("{} failed", dep_id(10))), "{s}");
    }
    assert_eq!(ingest(&t, APP, &[("d2", "bbb", 500)]), [Ingress::NotPaused]);
    failed_cutover(&t, APP, 40, 600);
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!("deployment {} failed its cutover", dep_id(40))),
        "{s}"
    );
    assert!(
        !s.contains(&format!("deployment {} failed its cutover", dep_id(10))),
        "{s}"
    );
    assert_eq!(ingest(&t, APP, &[("d3", "ccc", 700)]), [Ingress::Held]);
    let e = t.cli(&["events"]).stdout;
    assert!(
        e.contains(&format!(
            "deployment {} failed_cutover since 105 ms (as recorded: recovery restored) — cleared by a decision",
            dep_id(10)
        )),
        "{e}"
    );
    assert!(
        e.contains(&format!(
            "deployment {} failed_cutover since 605 ms",
            dep_id(40)
        )) && e.contains("in force"),
        "{e}"
    );
}

#[test]
fn event_management_and_resume_never_touch_docker() {
    let t = T::new();
    failed_cutover(&t, APP, 10, 100);
    t.h.docker.with(|s| {
        s.add_container("bystander", vec![], true);
    });
    ingest(&t, APP, &[("d1", "aaa", 300), ("d2", "bbb", 310)]);
    for a in [
        vec!["events"],
        vec!["status"],
        vec!["resume", "--app", APP, "--newest"],
        vec!["resume", "--app", APP, "--dismiss"],
    ] {
        assert_exit(&t.cli(&a), 0);
    }
    assert_eq!(t.docker_requests(), 0, "no request ever reached Docker");
    // resume does not even accept a Docker socket
    let sock = t.socket();
    let o = t.cli(&[
        "resume",
        "--app",
        APP,
        "--dismiss",
        "--docker-socket",
        &sock,
    ]);
    assert_exit(&o, 2);
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn a_writer_holding_the_journal_blocks_decisions_and_corrupt_event_state_fails_closed() {
    let t = T::new();
    failed_cutover(&t, APP, 10, 100);
    ingest(&t, APP, &[("d1", "aaa", 300)]);
    {
        let (_w, _) = open_j(&t.journal_dir()).unwrap();
        let before = t.journal_bytes();
        let o = resume(&t, APP, &["--dismiss"]);
        assert_exit(&o, 4);
        assert!(
            o.stderr
                .contains("another Harpoon process holds the journal"),
            "{}",
            o.stderr
        );
        assert_eq!(t.journal_bytes(), before);
    }
    // damage one stored event record
    let mut bytes = t.journal_bytes();
    let at = bytes.windows(9).position(|w| w == b"\tevent\t1\t").unwrap() + 20;
    bytes[at] = if bytes[at] == b'a' { b'b' } else { b'a' };
    std::fs::write(t.journal_dir().join(SQLITE_FILE), &bytes).unwrap();
    let damaged = t.journal_bytes();
    for cmd in [vec!["status"], vec!["events"], vec!["recover"]] {
        assert_exit(&t.cli(&cmd), 4);
    }
    assert_exit(&resume(&t, APP, &["--dismiss"]), 4);
    assert_eq!(
        t.journal_bytes(),
        damaged,
        "nothing was repaired or rewritten"
    );
}

#[test]
fn a_store_without_any_pause_or_event_reads_exactly_as_before() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let s = t.cli(&["status"]).stdout;
    for absent in [
        "PAUSED",
        "held events",
        "PENDING",
        "automatic triggers paused",
    ] {
        assert!(!s.contains(absent), "{absent}: {s}");
    }
    assert!(t.cli(&["events"]).stdout.contains("none recorded"));
    assert_exit(
        &resume(&t, "app_00000000000000000000000009", &["--dismiss"]),
        0,
    );
}
