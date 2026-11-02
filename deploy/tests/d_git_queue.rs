//! Milestone D at the engine level: events queued for the source executor, newest wins, execution
//! records, idempotency on the delivery id, and replay (including a crash between the records).

use harpoon_deploy::control::*;
use harpoon_deploy::engine::MemoryJournal;
use harpoon_deploy::ids::{ApplicationId, DeploymentId, ReleaseId};
use harpoon_deploy::state::*;

const A: &str = "app_00000000000000000000000001";
const B: &str = "app_00000000000000000000000002";

fn app(s: &str) -> ApplicationId {
    s.parse().unwrap()
}
fn dep(n: u32) -> DeploymentId {
    format!("dep_0000000000000000000000{n:04}").parse().unwrap()
}

fn succeeded(a: &str, n: u32, at: u64) -> Deployment {
    let mut d = Deployment::new(dep(n), app(a), Trigger::Poll, at);
    d.transition(DeploymentStatus::Queued, at + 1).unwrap();
    d.transition(DeploymentStatus::Running, at + 2).unwrap();
    d.release = Some(ReleaseId::from_deployment(&dep(n)));
    d.transition(DeploymentStatus::Succeeded, at + 3).unwrap();
    d
}

fn failed_cutover(a: &str, n: u32, at: u64) -> Deployment {
    let mut d = Deployment::new(dep(n), app(a), Trigger::Cli, at);
    d.transition(DeploymentStatus::Queued, at + 1).unwrap();
    d.transition(DeploymentStatus::Running, at + 2).unwrap();
    d.start_step(Step::Cutover, at + 3).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&dep(1)));
    d.begin_cutover(at + 4).unwrap();
    d.fail_cutover(at + 5).unwrap();
    d.start_recovery(60_000, at + 6).unwrap();
    d.finish_recovery(RecoveryOutcome::Restored, at + 7).unwrap();
    d.transition(DeploymentStatus::Failed, at + 8).unwrap();
    d
}

fn ev<'a>(a: &'a ApplicationId, id: &'a str, rev: &'a str, at: u64) -> Incoming<'a> {
    Incoming {
        app: a,
        delivery_id: id,
        source: "poll",
        revision: rev,
        received_ms: at,
    }
}

fn exec(a: &str, delivery: &str, n: Option<u32>, sup: &[&str], at: u64) -> ExecRec {
    ExecRec {
        app: app(a),
        delivery: delivery.into(),
        outcome: if n.is_some() {
            ExecOutcome::Deployed
        } else {
            ExecOutcome::Failed
        },
        deployment: n.map(dep),
        reason: if n.is_some() {
            String::new()
        } else {
            "the repository is unavailable".into()
        },
        superseded: sup.iter().map(|s| s.to_string()).collect(),
        at_ms: at,
    }
}

fn replayed(j: &MemoryJournal) -> ControlState {
    let mut s = ControlState::default();
    for r in &j.control_records {
        s.apply(r).unwrap();
    }
    s
}

#[test]
fn queueing_is_idempotent_on_the_delivery_id_and_refuses_a_changed_revision() {
    let a = app(A);
    let mut j = MemoryJournal::new();
    let mut c = ControlState::default();
    assert_eq!(
        queue_event(&mut j, &mut c, ev(&a, "poll:shop:r1", "r1", 100)).unwrap(),
        Ingress::Held
    );
    // a retry arrives later, same content: nothing is written
    assert_eq!(
        queue_event(&mut j, &mut c, ev(&a, "poll:shop:r1", "r1", 999)).unwrap(),
        Ingress::Duplicate
    );
    assert_eq!(j.control_records.len(), 1);
    // the same id with another revision is refused (a tampered or confused delivery)
    assert!(matches!(
        queue_event(&mut j, &mut c, ev(&a, "poll:shop:r1", "r2", 101)),
        Err(ControlError::Conflict(_))
    ));
    let mut other_source = ev(&a, "poll:shop:r1", "r1", 101);
    other_source.source = "github";
    assert!(matches!(
        queue_event(&mut j, &mut c, other_source),
        Err(ControlError::Conflict(_))
    ));
    for bad in ["", "has space", "x\ny"] {
        assert!(
            matches!(
                queue_event(&mut j, &mut c, ev(&a, bad, "r", 1)),
                Err(ControlError::Invalid(_))
            ),
            "{bad:?}"
        );
    }
    assert_eq!(j.control_records.len(), 1, "refusals write nothing");
    // applications are isolated
    let b = app(B);
    assert_eq!(
        queue_event(&mut j, &mut c, ev(&b, "poll:shop:r1", "r1", 100)).unwrap(),
        Ingress::Held
    );
    assert_eq!(replayed(&j), c);
}

#[test]
fn an_unpaused_application_executes_the_newest_queued_event_and_supersedes_the_older_ones() {
    let a = app(A);
    let deps = vec![succeeded(A, 1, 10)];
    let mut j = MemoryJournal::new();
    let mut c = ControlState::default();
    assert_eq!(next_execution(&deps, &c, &a), Next::Nothing);
    for (id, rev, at) in [("d1", "r1", 100), ("d3", "r3", 300), ("d2", "r2", 200)] {
        queue_event(&mut j, &mut c, ev(&a, id, rev, at)).unwrap();
    }
    let Next::Newest { chosen, superseded } = next_execution(&deps, &c, &a) else {
        panic!("expected newest")
    };
    assert_eq!(chosen.rec.delivery, "d3", "newest received wins, not newest recorded");
    assert_eq!(superseded.len(), 2);
    let sup: Vec<&str> = superseded.iter().map(String::as_str).collect();
    let deps2 = vec![succeeded(A, 1, 10), succeeded(A, 2, 400)];
    record_exec(&mut j, &mut c, exec(A, "d3", Some(2), &sup, 410)).unwrap();
    let _ = deps2;
    assert_eq!(c.event(&a, "d3").unwrap().status, EventStatus::Executed);
    assert_eq!(
        c.event(&a, "d3").unwrap().related.as_deref(),
        Some(dep(2).to_string().as_str())
    );
    for d in ["d1", "d2"] {
        let e = c.event(&a, d).unwrap();
        assert_eq!(e.status, EventStatus::Superseded(SupersededWhy::NewerSelected));
        assert_eq!(e.related.as_deref(), Some("d3"));
    }
    assert_eq!(next_execution(&deps, &c, &a), Next::Nothing);
    // an executed event is not executed twice, and a late duplicate delivery changes nothing
    assert!(record_exec(&mut j, &mut c, exec(A, "d3", Some(2), &[], 500)).is_err());
    assert_eq!(
        queue_event(&mut j, &mut c, ev(&a, "d3", "r3", 600)).unwrap(),
        Ingress::Duplicate
    );
    assert_eq!(replayed(&j), c, "replay reaches the same state");
}

#[test]
fn a_paused_application_keeps_events_held_until_a_decision_selects_one() {
    let a = app(A);
    let deps = vec![succeeded(A, 1, 10), failed_cutover(A, 2, 100)];
    let mut j = MemoryJournal::new();
    let mut c = ControlState::default();
    queue_event(&mut j, &mut c, ev(&a, "d1", "r1", 300)).unwrap();
    queue_event(&mut j, &mut c, ev(&a, "d2", "r2", 310)).unwrap();
    assert_eq!(
        next_execution(&deps, &c, &a),
        Next::Nothing,
        "paused: nothing runs by itself"
    );
    resume(&mut j, &deps, &mut c, &a, Choice::Revision("r1".into()), 400).unwrap();
    let Next::Selected(e) = next_execution(&deps, &c, &a) else {
        panic!("the selection is executed")
    };
    assert_eq!(e.rec.delivery, "d1");
    let deps2 = vec![succeeded(A, 1, 10), failed_cutover(A, 2, 100), succeeded(A, 3, 500)];
    record_exec(&mut j, &mut c, exec(A, "d1", Some(3), &[], 510)).unwrap();
    assert_eq!(c.event(&a, "d1").unwrap().status, EventStatus::Executed);
    assert_eq!(
        next_execution(&deps2, &c, &a),
        Next::Nothing,
        "the pause stays until it is dismissed"
    );
    assert!(is_paused(&a, &deps2, &c));
    assert_eq!(replayed(&j), c);
}

#[test]
fn a_failed_execution_is_recorded_with_its_reason_and_other_applications_are_untouched() {
    let (a, b) = (app(A), app(B));
    let deps = vec![succeeded(A, 1, 10), succeeded(B, 2, 10)];
    let mut j = MemoryJournal::new();
    let mut c = ControlState::default();
    queue_event(&mut j, &mut c, ev(&a, "d1", "r1", 100)).unwrap();
    queue_event(&mut j, &mut c, ev(&b, "d1", "r9", 100)).unwrap();
    record_exec(&mut j, &mut c, exec(A, "d1", None, &[], 110)).unwrap();
    let e = c.event(&a, "d1").unwrap();
    assert_eq!(e.status, EventStatus::Failed);
    assert_eq!(e.related.as_deref(), Some("the repository is unavailable"));
    assert_eq!(c.event(&b, "d1").unwrap().status, EventStatus::Held);
    assert!(matches!(next_execution(&deps, &c, &b), Next::Newest { .. }));
    // contradictions are refused and write nothing
    let n = j.control_records.len();
    assert!(record_exec(&mut j, &mut c, exec(A, "nope", Some(5), &[], 1)).is_err());
    assert!(record_exec(
        &mut j,
        &mut c,
        ExecRec {
            deployment: Some(dep(5)),
            ..exec(A, "d1", None, &[], 1)
        }
    )
    .is_err());
    assert!(
        record_exec(&mut j, &mut c, exec(B, "d1", Some(2), &["d1"], 1)).is_err(),
        "cannot supersede itself"
    );
    assert_eq!(j.control_records.len(), n);
}

#[test]
fn execution_records_round_trip_and_replay_checks_the_deployment() {
    let a = app(A);
    let mut j = MemoryJournal::new();
    let mut c = ControlState::default();
    queue_event(&mut j, &mut c, ev(&a, "d1", "r1", 100)).unwrap();
    queue_event(&mut j, &mut c, ev(&a, "d2", "r2", 200)).unwrap();
    record_exec(&mut j, &mut c, exec(A, "d2", Some(4), &["d1"], 300)).unwrap();
    for r in &j.control_records {
        let line = harpoon_deploy::control::encode(r);
        let body = line.split_once('\t').unwrap().1;
        let f: Vec<&str> = body.split('\t').collect();
        assert_eq!(&harpoon_deploy::control::decode(&f).unwrap(), r);
    }
    // replay requires the deployment to exist and belong to the application
    assert!(c.check_against(&[succeeded(A, 4, 250)]).is_ok());
    assert!(c.check_against(&[]).is_err());
    assert!(c.check_against(&[succeeded(B, 4, 250)]).is_err());
    // a cut after every write replays to a prefix and completing it reaches the same state
    let all = j.control_records.clone();
    for n in 0..=all.len() {
        let mut s = ControlState::default();
        for r in &all[..n] {
            s.apply(r).unwrap();
        }
        for r in &all[n..] {
            s.apply(r).unwrap();
        }
        assert_eq!(s, c, "cut after {n}");
    }
}
