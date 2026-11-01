//! Durable pauses, held events and explicit resume decisions at the engine level: lifecycle,
//! idempotency, isolation, supersession, codec, and a crash at every write.

use harpoon_deploy::control::*;
use harpoon_deploy::engine::{Journal, MemoryJournal};
use harpoon_deploy::ids::{ApplicationId, DeploymentId, ReleaseId};
use harpoon_deploy::journal_format::{decode_line, Line};
use harpoon_deploy::state::*;

const A: &str = "app_00000000000000000000000001";
const B: &str = "app_00000000000000000000000002";

fn app(s: &str) -> ApplicationId {
    s.parse().unwrap()
}
fn dep(n: u32) -> DeploymentId {
    format!("dep_0000000000000000000000{n:04}").parse().unwrap()
}

fn base(a: &str, id: DeploymentId, at: u64) -> Deployment {
    let mut d = Deployment::new(id, app(a), Trigger::Cli, at);
    d.transition(DeploymentStatus::Queued, at + 1).unwrap();
    d.transition(DeploymentStatus::Running, at + 2).unwrap();
    d
}

/// A cutover that began and failed, with a bounded recovery that restored the previous release.
fn failed_cutover(a: &str, n: u32, at: u64) -> Deployment {
    let mut d = base(a, dep(n), at);
    d.start_step(Step::Cutover, at + 3).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&dep(1)));
    d.begin_cutover(at + 4).unwrap();
    d.fail_cutover(at + 5).unwrap();
    d.start_recovery(60_000, at + 6).unwrap();
    d.finish_recovery(RecoveryOutcome::Restored, at + 7).unwrap();
    d.transition(DeploymentStatus::Failed, at + 8).unwrap();
    d
}

fn open_cutover(a: &str, n: u32, at: u64) -> Deployment {
    let mut d = base(a, dep(n), at);
    d.start_step(Step::Cutover, at + 3).unwrap();
    d.begin_cutover(at + 4).unwrap();
    d
}

fn rolled_back(a: &str, n: u32, at: u64) -> Deployment {
    let mut d = base(a, dep(n), at);
    d.rollback_to = Some(ReleaseId::from_deployment(&dep(1)));
    d.transition(DeploymentStatus::Succeeded, at + 3).unwrap();
    d
}

fn failed_before_cutover(a: &str, n: u32, at: u64, kind: FailureKind, step: Step) -> Deployment {
    let mut d = base(a, dep(n), at);
    d.failure = Some(Failure {
        step,
        kind,
        message: "x".into(),
        timed_out: false,
    });
    d.transition(DeploymentStatus::Failed, at + 3).unwrap();
    d
}

fn ev<'a>(a: &'a ApplicationId, delivery: &'a str, rev: &'a str, at: u64) -> Incoming<'a> {
    Incoming {
        app: a,
        delivery_id: delivery,
        source: "webhook",
        revision: rev,
        received_ms: at,
    }
}

/// What a restart sees: the control records the journal kept, replayed in order.
fn replayed(j: &MemoryJournal) -> ControlState {
    let mut s = ControlState::default();
    for r in &j.control_records {
        s.apply(r).unwrap();
    }
    s
}

#[test]
fn only_a_cutover_that_began_or_a_successful_rollback_pauses() {
    let cases = [
        (failed_cutover(A, 10, 100), Some(PauseKind::FailedCutover)),
        (open_cutover(A, 11, 100), Some(PauseKind::InterruptedCutover)),
        (rolled_back(A, 12, 100), Some(PauseKind::Rollback)),
        (failed_before_cutover(A, 13, 100, FailureKind::Build, Step::Build), None),
        (
            failed_before_cutover(A, 14, 100, FailureKind::PreliminaryCheck, Step::PreliminaryCheck),
            None,
        ),
        (
            failed_before_cutover(A, 15, 100, FailureKind::Source, Step::Source),
            None,
        ),
        // a prepared-but-unconfirmed candidate: nothing began, nothing is paused
        (base(A, dep(16), 100), None),
    ];
    for (d, want) in cases {
        let c = pause_candidates(std::slice::from_ref(&d));
        assert_eq!(c.first().map(|c| c.kind), want, "{:?} {:?}", d.status, d.cutover);
    }
    // a cancelled run that never began a cutover
    let mut d = base(A, dep(17), 100);
    d.transition(DeploymentStatus::Cancelled, 110).unwrap();
    assert!(pause_candidates(&[d]).is_empty());
}

#[test]
fn a_pause_is_recorded_once_with_its_origin_time_and_context_and_replays_exactly() {
    let deps = vec![failed_cutover(A, 10, 100), rolled_back(B, 20, 200)];
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    assert_eq!(sync_pauses(&mut j, &deps, &mut ctl).unwrap(), 2);
    assert_eq!(sync_pauses(&mut j, &deps, &mut ctl).unwrap(), 0, "idempotent");
    assert_eq!(j.control_records.len(), 2);
    let p = ctl.pause_of(&dep(10)).unwrap();
    assert_eq!(
        (p.kind, p.at_ms, p.context.as_str()),
        (PauseKind::FailedCutover, 105, "recovery restored")
    );
    let r = ctl.pause_of(&dep(20)).unwrap();
    assert_eq!((r.kind, r.at_ms), (PauseKind::Rollback, 203));
    assert!(r.context.starts_with("rolled back to rel_"));
    assert_eq!(replayed(&j), ctl);
    let act = active_pauses(&deps, &ctl);
    assert_eq!(act.len(), 2);
    assert!(act.iter().all(|p| p.recorded));
}

#[test]
fn events_are_held_only_while_paused_deduplicated_isolated_and_never_dropped() {
    let (a, b) = (app(A), app(B));
    let deps = vec![failed_cutover(A, 10, 100)];
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    // B is not paused: nothing is recorded, the caller decides
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&b, "d1", "r1", 300)).unwrap(),
        Ingress::NotPaused
    );
    assert!(ctl.events.is_empty());
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&a, "d1", "r1", 300)).unwrap(),
        Ingress::Held
    );
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&a, "d2", "r2", 310)).unwrap(),
        Ingress::Held
    );
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&a, "d3", "r3", 320)).unwrap(),
        Ingress::Held
    );
    // duplicate delivery: no new event, no write
    let writes = j.writes();
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&a, "d2", "r2", 310)).unwrap(),
        Ingress::Duplicate
    );
    assert_eq!(j.writes(), writes);
    // the same delivery id with different content is refused and changes nothing
    for bad in [
        ev(&a, "d2", "other", 310),
        Incoming {
            source: "poll",
            ..ev(&a, "d2", "r2", 310)
        },
        ev(&a, "d2", "r2", 999),
    ] {
        assert!(matches!(
            ingest(&mut j, &deps, &mut ctl, bad),
            Err(ControlError::Conflict(_))
        ));
    }
    assert_eq!(j.writes(), writes);
    // the same delivery id for another application is a different event
    let deps2 = vec![failed_cutover(A, 10, 100), failed_cutover(B, 30, 100)];
    assert_eq!(
        ingest(&mut j, &deps2, &mut ctl, ev(&b, "d1", "rb", 330)).unwrap(),
        Ingress::Held
    );
    assert_eq!(ctl.held(&a).len(), 3);
    assert_eq!(ctl.held(&b).len(), 1);
    assert_eq!(replayed(&j), ctl);
}

#[test]
fn malformed_events_are_refused_before_anything_is_written() {
    let a = app(A);
    let deps = vec![failed_cutover(A, 10, 100)];
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    for (d, s, r) in [
        ("", "webhook", "r"),
        ("d 1", "webhook", "r"),
        ("d\t1", "webhook", "r"),
        ("d1", "web hook", "r"),
        ("d1", "", "r"),
        ("d1", "webhook", ""),
        ("d1", "webhook", "r\nx"),
        ("d1", "webhook", "r,s"),
    ] {
        let e = Incoming {
            app: &a,
            delivery_id: d,
            source: s,
            revision: r,
            received_ms: 1,
        };
        assert!(
            matches!(ingest(&mut j, &deps, &mut ctl, e), Err(ControlError::Invalid(_))),
            "{d:?} {s:?} {r:?}"
        );
    }
    assert_eq!(j.writes(), 0);
}

fn paused_with_three() -> (Vec<Deployment>, MemoryJournal, ControlState) {
    let a = app(A);
    let deps = vec![failed_cutover(A, 10, 100)];
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    for (d, r, t) in [("d1", "aaa", 300), ("d2", "bbb", 310), ("d3", "ccc", 320)] {
        assert_eq!(ingest(&mut j, &deps, &mut ctl, ev(&a, d, r, t)).unwrap(), Ingress::Held);
    }
    (deps, j, ctl)
}

fn status_of(ctl: &ControlState, d: &str) -> EventStatus {
    ctl.event(&app(A), d).unwrap().status
}

#[test]
fn newest_wins_selects_the_latest_supersedes_the_rest_and_keeps_the_pause() {
    let (deps, mut j, mut ctl) = paused_with_three();
    let a = app(A);
    let r = resume(&mut j, &deps, &mut ctl, &a, Choice::Newest, 400).unwrap();
    assert_eq!(
        r,
        Resumed::Selected {
            delivery: "d3".into(),
            revision: "ccc".into(),
            superseded: 2
        }
    );
    assert_eq!(status_of(&ctl, "d3"), EventStatus::Selected);
    assert_eq!(
        status_of(&ctl, "d1"),
        EventStatus::Superseded(SupersededWhy::NewerSelected)
    );
    assert_eq!(ctl.event(&a, "d1").unwrap().related.as_deref(), Some("d3"));
    // history preserved, nothing deployed, still paused
    assert_eq!(ctl.events.len(), 3);
    assert!(is_paused(&a, &deps, &ctl), "a selection does not clear the pause");
    assert!(ctl.pending_selection(&a).is_some());
    // a repeat of the same request is refused (pending), and a different one too; nothing written
    let w = j.writes();
    assert!(matches!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Newest, 401),
        Err(ControlError::Conflict(_))
    ));
    assert!(matches!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("aaa".into()), 401),
        Err(ControlError::Conflict(_))
    ));
    assert_eq!(j.writes(), w);
    // the same revision again is idempotent
    assert_eq!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("ccc".into()), 402).unwrap(),
        Resumed::AlreadySelected { delivery: "d3".into() }
    );
    assert_eq!(j.writes(), w);
    // events arriving now are still held: the application is still blocked
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&a, "d4", "ddd", 500)).unwrap(),
        Ingress::Held
    );
}

#[test]
fn a_specific_revision_is_selected_without_guessing_and_the_others_are_recorded() {
    let (deps, mut j, mut ctl) = paused_with_three();
    let a = app(A);
    let w = j.writes();
    // an unknown revision is refused with the held ones named, and nothing changes
    let e = resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("zzz".into()), 400).unwrap_err();
    assert!(e.to_string().contains("held revisions: aaa, bbb, ccc"), "{e}");
    assert!(matches!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("bad rev".into()), 400),
        Err(ControlError::Invalid(_))
    ));
    assert_eq!(j.writes(), w);
    let r = resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("bbb".into()), 400).unwrap();
    assert!(matches!(r, Resumed::Selected { ref delivery, .. } if delivery == "d2"));
    assert_eq!(status_of(&ctl, "d2"), EventStatus::Selected);
    for d in ["d1", "d3"] {
        assert_eq!(
            status_of(&ctl, d),
            EventStatus::Superseded(SupersededWhy::NotChosen),
            "{d}"
        );
    }
    assert_eq!(ctl.events.len(), 3, "nothing is dropped");
}

#[test]
fn dismissal_clears_the_pause_dismisses_everything_and_a_new_failure_pauses_again() {
    let (mut deps, mut j, mut ctl) = paused_with_three();
    let a = app(A);
    resume(&mut j, &deps, &mut ctl, &a, Choice::Newest, 400).unwrap();
    // abandoning a pending selection is a dismissal
    let r = resume(&mut j, &deps, &mut ctl, &a, Choice::Dismiss, 410).unwrap();
    assert_eq!(r, Resumed::Dismissed { events: 1, pauses: 1 });
    assert_eq!(status_of(&ctl, "d3"), EventStatus::Dismissed);
    assert_eq!(
        status_of(&ctl, "d1"),
        EventStatus::Superseded(SupersededWhy::NewerSelected),
        "history kept"
    );
    assert!(!is_paused(&a, &deps, &ctl));
    assert_eq!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Dismiss, 420).unwrap(),
        Resumed::NothingToDo
    );
    // the old deployment is still in history, but it is not re-derived into a pause
    assert!(active_pauses(&deps, &ctl).is_empty());
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&a, "d9", "x", 430)).unwrap(),
        Ingress::NotPaused
    );
    // a NEW failed cutover pauses again
    deps.push(failed_cutover(A, 40, 500));
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&a, "d10", "y", 600)).unwrap(),
        Ingress::Held
    );
    let act = active_pauses(&deps, &ctl);
    assert_eq!(act.len(), 1);
    assert_eq!(act[0].origin, dep(40));
    assert_eq!(replayed(&j), ctl);
}

#[test]
fn resuming_one_application_never_touches_another() {
    let (a, b) = (app(A), app(B));
    let deps = vec![failed_cutover(A, 10, 100), rolled_back(B, 20, 100)];
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    ingest(&mut j, &deps, &mut ctl, ev(&a, "d1", "ra", 300)).unwrap();
    ingest(&mut j, &deps, &mut ctl, ev(&b, "d1", "rb", 300)).unwrap();
    resume(&mut j, &deps, &mut ctl, &a, Choice::Dismiss, 400).unwrap();
    assert!(!is_paused(&a, &deps, &ctl) && is_paused(&b, &deps, &ctl));
    assert_eq!(ctl.held(&b).len(), 1);
    assert_eq!(ctl.event(&b, "d1").unwrap().status, EventStatus::Held);
}

#[test]
fn a_decision_with_nothing_to_choose_is_refused_without_a_write() {
    let a = app(A);
    let deps = vec![failed_cutover(A, 10, 100)];
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    sync_pauses(&mut j, &deps, &mut ctl).unwrap();
    let w = j.writes();
    assert!(matches!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Newest, 1),
        Err(ControlError::Conflict(_))
    ));
    assert!(matches!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("r".into()), 1),
        Err(ControlError::Conflict(_))
    ));
    assert_eq!(j.writes(), w);
    // a dismissal with only a pause clears it, with no events
    assert_eq!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Dismiss, 2).unwrap(),
        Resumed::Dismissed { events: 0, pauses: 1 }
    );
}

#[test]
fn every_record_round_trips_through_its_framed_line_and_damage_is_refused() {
    let (deps, mut j, mut ctl) = paused_with_three();
    let a = app(A);
    resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("bbb".into()), 400).unwrap();
    resume(&mut j, &deps, &mut ctl, &a, Choice::Dismiss, 410).unwrap();
    assert!(j.control_records.len() >= 6);
    for r in &j.control_records {
        let line = encode(r);
        match decode_line(&line).unwrap() {
            Line::Control(back) => assert_eq!(&back, r),
            other => panic!("{other:?}"),
        }
        // any single-byte change is caught by the checksum (or the decoder)
        let mut bytes = line.clone().into_bytes();
        let at = bytes.len() / 2;
        bytes[at] = if bytes[at] == b'x' { b'y' } else { b'x' };
        assert!(decode_line(&String::from_utf8(bytes).unwrap()).is_err());
    }
}

#[test]
fn replay_refuses_inconsistent_control_records() {
    let a = app(A);
    let ev1 = ControlRecord::Event(EventRec {
        app: a.clone(),
        delivery: "d1".into(),
        source: "webhook".into(),
        revision: "r".into(),
        received_ms: 1,
    });
    // a duplicate delivery recorded twice
    let mut s = ControlState::default();
    s.apply(&ev1).unwrap();
    assert!(s.apply(&ev1).is_err());
    // a decision naming an unknown delivery
    let bad = ControlRecord::Resume(ResumeRec {
        app: a.clone(),
        at_ms: 2,
        choice: Choice::Newest,
        selected: Some("nope".into()),
        dismissed: vec![],
        superseded: vec![],
        cleared: vec![],
    });
    assert!(ControlState::default().apply(&bad).is_err());
    // a dismissal that leaves a held event unaccounted for
    let leaves = ControlRecord::Resume(ResumeRec {
        app: a.clone(),
        at_ms: 2,
        choice: Choice::Dismiss,
        selected: None,
        dismissed: vec![],
        superseded: vec![],
        cleared: vec![],
    });
    assert!(s.apply(&leaves).is_err());
    // a decision clearing a deployment that never paused anything
    let dep_ok = failed_before_cutover(A, 13, 100, FailureKind::Build, Step::Build);
    let mut s2 = ControlState::default();
    s2.apply(&ControlRecord::Resume(ResumeRec {
        app: a.clone(),
        at_ms: 2,
        choice: Choice::Dismiss,
        selected: None,
        dismissed: vec![],
        superseded: vec![],
        cleared: vec![dep(13)],
    }))
    .unwrap();
    assert!(s2.check_against(&[dep_ok]).is_err());
    // a pause naming a deployment of another application
    let mut s3 = ControlState::default();
    s3.apply(&ControlRecord::Pause(PauseRec {
        app: app(B),
        origin: dep(10),
        kind: PauseKind::FailedCutover,
        at_ms: 1,
        context: String::new(),
    }))
    .unwrap();
    assert!(s3.check_against(&[failed_cutover(A, 10, 100)]).is_err());
}

#[test]
fn a_crash_at_every_write_of_the_whole_sequence_recovers_to_exactly_the_same_state() {
    let (a, b) = (app(A), app(B));
    let deps = vec![failed_cutover(A, 10, 100), rolled_back(B, 20, 100)];

    // the sequence a developer's tooling performs; every step is safe to run again after a restart
    fn script<J: Journal>(
        j: &mut J,
        deps: &[Deployment],
        ctl: &mut ControlState,
        (a, b): (&ApplicationId, &ApplicationId),
        tolerant: bool,
    ) -> Result<(), ControlError> {
        fn ok<T>(r: Result<T, ControlError>, tolerant: bool) -> Result<(), ControlError> {
            match r {
                Err(ControlError::Conflict(_)) if tolerant => Ok(()),
                Err(e) => Err(e),
                Ok(_) => Ok(()),
            }
        }
        for (app, d, r, t) in [
            (a, "d1", "r1", 300),
            (a, "d2", "r2", 310),
            (b, "e1", "q1", 320),
            (a, "d3", "r3", 330),
            (a, "d2", "r2", 310),
        ] {
            ok(
                ingest(
                    j,
                    deps,
                    ctl,
                    Incoming {
                        app,
                        delivery_id: d,
                        source: "webhook",
                        revision: r,
                        received_ms: t,
                    },
                ),
                tolerant,
            )?;
        }
        ok(resume(j, deps, ctl, a, Choice::Newest, 400), tolerant)?;
        ok(resume(j, deps, ctl, a, Choice::Dismiss, 410), tolerant)?;
        Ok(())
    }

    // the reference run
    let mut j = MemoryJournal::new();
    let mut want = ControlState::default();
    script(&mut j, &deps, &mut want, (&a, &b), false).unwrap();
    let total = j.writes();
    assert_eq!(total, 8, "two pauses, four events, two decisions");
    assert!(!is_paused(&a, &deps, &want) && is_paused(&b, &deps, &want));
    assert_eq!(want.events.len(), 4, "history of three events for A and one for B");

    let mut crashes = 0;
    for n in 0..=total {
        let mut j = MemoryJournal::crash_at(n);
        let mut ctl = ControlState::default();
        let first = script(&mut j, &deps, &mut ctl, (&a, &b), false);
        if first.is_ok() {
            assert_eq!(ctl, want, "no crash, same state");
            continue;
        }
        crashes += 1;
        assert_eq!(
            first,
            Err(ControlError::Journal(harpoon_deploy::engine::JournalError::Crash))
        );
        // restart: what the journal kept is exactly a valid prefix and replays
        let mut after = MemoryJournal::new();
        after.control_records = j.control_records.clone();
        let mut ctl = replayed(&after);
        ctl.check_against(&deps).unwrap();
        // the interrupted request is simply made again
        script(&mut after, &deps, &mut ctl, (&a, &b), true).unwrap();
        assert_eq!(ctl, want, "crash at write {n}");
        assert_eq!(replayed(&after), want, "and a further restart reconstructs it");
    }
    assert_eq!(crashes, total);
}

// ---- final correctness review: refusals and NotPaused write nothing; replay checks the cause

/// The record count and content, standing in for the journal hash.
fn snapshot(j: &MemoryJournal) -> Vec<String> {
    j.control_records.iter().map(encode).collect()
}

#[test]
fn a_refused_resume_writes_nothing_even_when_a_legacy_pause_is_unmaterialized() {
    let a = app(A);
    // legacy: a failed cutover whose pause no pause record materialises yet
    let deps = vec![failed_cutover(A, 10, 100)];
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    let before = snapshot(&j);
    for choice in [
        Choice::Revision("nope".into()),
        Choice::Revision("bad revision!".into()),
        Choice::Newest,
    ] {
        assert!(resume(&mut j, &deps, &mut ctl, &a, choice, 900).is_err());
        assert_eq!(snapshot(&j), before);
        assert!(j.control_records.is_empty() && ctl == ControlState::default());
    }
    // a conflicting pending selection: held and selected, then another choice is refused
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&a, "d1", "aaa", 300)).unwrap(),
        Ingress::Held
    );
    ingest(&mut j, &deps, &mut ctl, ev(&a, "d2", "bbb", 310)).unwrap();
    resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("aaa".into()), 400).unwrap();
    let (before, state) = (snapshot(&j), ctl.clone());
    assert!(resume(&mut j, &deps, &mut ctl, &a, Choice::Newest, 500).is_err());
    assert!(resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("bbb".into()), 500).is_err());
    assert_eq!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Revision("aaa".into()), 500).unwrap(),
        Resumed::AlreadySelected { delivery: "d1".into() }
    );
    assert_eq!((snapshot(&j), &ctl), (before, &state));
}

#[test]
fn a_select_with_nothing_held_on_a_legacy_pause_writes_nothing_and_a_dismissal_records_only_that_app() {
    let (a, b) = (app(A), app(B));
    let deps = vec![failed_cutover(A, 10, 100), rolled_back(B, 20, 100)];
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    assert!(resume(&mut j, &deps, &mut ctl, &a, Choice::Newest, 1).is_err());
    assert!(j.control_records.is_empty());
    // dismissing A records A's pause and its decision; B's unmaterialized pause is untouched
    resume(&mut j, &deps, &mut ctl, &a, Choice::Dismiss, 2).unwrap();
    assert_eq!(j.control_records.len(), 2);
    assert!(ctl.pause_of(&dep(10)).is_some() && ctl.pause_of(&dep(20)).is_none());
    assert!(is_paused(&b, &deps, &ctl) && !is_paused(&a, &deps, &ctl));
    assert_eq!(replayed(&j), ctl);
    // nothing to do for an unpaused application writes nothing
    let n = j.control_records.len();
    assert_eq!(
        resume(&mut j, &deps, &mut ctl, &a, Choice::Dismiss, 3).unwrap(),
        Resumed::NothingToDo
    );
    assert_eq!(j.control_records.len(), n);
}

#[test]
fn not_paused_ingress_writes_nothing_for_any_application() {
    let (a, b) = (app(A), app(B));
    // A has no pause; B has a failed cutover that no record materialises yet
    let deps = vec![failed_cutover(B, 20, 100)];
    let mut j = MemoryJournal::new();
    let mut ctl = ControlState::default();
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&a, "d1", "r1", 300)).unwrap(),
        Ingress::NotPaused
    );
    assert!(j.control_records.is_empty(), "no write, not even for B");
    assert_eq!(ctl, ControlState::default());
    // B's own event records B's pause and the event, and only those
    assert_eq!(
        ingest(&mut j, &deps, &mut ctl, ev(&b, "d1", "r1", 300)).unwrap(),
        Ingress::Held
    );
    assert_eq!(j.control_records.len(), 2);
    assert_eq!(replayed(&j), ctl);
}

#[test]
fn replay_checks_that_a_pause_names_a_pause_causing_deployment_of_a_compatible_kind_and_time() {
    let pause = |origin, kind, at_ms| {
        let mut s = ControlState::default();
        s.apply(&ControlRecord::Pause(PauseRec {
            app: app(A),
            origin,
            kind,
            at_ms,
            context: String::new(),
        }))
        .unwrap();
        s
    };
    let failed = failed_cutover(A, 10, 100); // failed at 105
                                             // a deployment of the application that caused no pause: contradiction
    let plain = failed_before_cutover(A, 13, 100, FailureKind::Build, Step::Build);
    assert!(pause(dep(13), PauseKind::FailedCutover, 105)
        .check_against(&[plain])
        .is_err());
    // the real thing, and a kind or time that contradicts the record
    assert!(pause(dep(10), PauseKind::FailedCutover, 105)
        .check_against(std::slice::from_ref(&failed))
        .is_ok());
    assert!(pause(dep(10), PauseKind::Rollback, 105)
        .check_against(std::slice::from_ref(&failed))
        .is_err());
    assert!(pause(dep(10), PauseKind::FailedCutover, 999)
        .check_against(std::slice::from_ref(&failed))
        .is_err());
    assert!(pause(dep(10), PauseKind::InterruptedCutover, 99)
        .check_against(std::slice::from_ref(&failed))
        .is_err());
    // legitimate evolution: recorded while the cutover was open, failed since
    assert!(pause(dep(10), PauseKind::InterruptedCutover, 104)
        .check_against(std::slice::from_ref(&failed))
        .is_ok());
    // the other direction (a failure recorded, then "interrupted") never happens
    let open = open_cutover(A, 11, 100);
    assert!(pause(dep(11), PauseKind::InterruptedCutover, 104)
        .check_against(std::slice::from_ref(&open))
        .is_ok());
    assert!(pause(dep(11), PauseKind::FailedCutover, 104)
        .check_against(&[open])
        .is_err());
    // a rollback pause matches its rollback only
    let rb = rolled_back(A, 12, 100);
    assert!(pause(dep(12), PauseKind::Rollback, 103)
        .check_against(std::slice::from_ref(&rb))
        .is_ok());
    assert!(pause(dep(12), PauseKind::Rollback, 104).check_against(&[rb]).is_err());
    // a restoration after the pause was recorded does not disturb it
    let mut restored = open_cutover(A, 14, 100);
    let s = pause(dep(14), PauseKind::InterruptedCutover, 104);
    restored.start_recovery(60_000, 110).unwrap_or(());
    assert!(s.check_against(&[restored]).is_ok());
}
