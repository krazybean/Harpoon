//! B1: the resource ledger through the lifecycle — intent before create, receipt after,
//! cleanup accounting that never disappears, deadlines, evidence, and crash recovery.
//! Deterministic: fake backend, in-memory journal, manual clock.

mod common;

use common::*;
use harpoon_deploy::backend::{Call, FakeBackend, Op, OpTrace, ResourceCall};
use harpoon_deploy::engine::{run_deployment_with, Budgets, EngineError, JournalError, MemoryJournal, RunConfig};
use harpoon_deploy::health::{Attempt, CheckEvidence, Observation, StatusClass, Verdict, SCOPE_NOTE};
use harpoon_deploy::ledger::{Disposition, EndState, EntryState, Ledger, LedgerRecord, Released, ResourceKind};
use harpoon_deploy::logs::Redactor;
use harpoon_deploy::state::*;
use std::cell::RefCell;
use std::rc::Rc;

fn ops() -> OpTrace {
    Rc::new(RefCell::new(Vec::new()))
}

// ---------------------------------------------------------------- intent / receipt

#[test]
fn intent_is_durable_before_the_object_exists_and_the_receipt_carries_the_real_id() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let t = ops();
    let mut b = fx.backend().with_ops_trace(t.clone());
    let mut j = MemoryJournal::new().with_ops_trace(t.clone());
    run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();

    let trace = t.borrow();
    let mut created = 0;
    for (i, op) in trace.iter().enumerate() {
        if let Op::Created { kind, name, id } = op {
            created += 1;
            let intent_at = trace[..i]
                .iter()
                .position(|o| matches!(o, Op::Write(LedgerRecord::Intent { key, .. }) if key.kind == *kind && &key.name == name))
                .unwrap_or_else(|| panic!("{name} was created without a prior durable intent"));
            let receipt_at = trace[i..]
                .iter()
                .position(|o| matches!(o, Op::Write(LedgerRecord::Receipt { key, id: rid, .. }) if key.kind == *kind && &key.name == name && rid == id))
                .unwrap_or_else(|| panic!("{name} has no receipt with the id the runtime returned"));
            assert!(intent_at < i && receipt_at > 0);
        }
    }
    assert!(
        created >= 3,
        "image, preliminary container and serving container were created"
    );
}

#[test]
fn a_successful_first_deployment_keeps_what_serves_and_removes_what_was_temporary() {
    let mut fx = Fx::new();
    let bystanders = fx.add_bystanders();
    let mut d = fx.deployment();
    let mut b = fx.backend();
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    assert!(r.is_clean(), "nothing needs attention: {:?}", r.cleanup_problems());

    let mine = labelled_for(&fx, &d);
    let names: Vec<&str> = mine.iter().map(|o| o.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.starts_with("harpoon/")),
        "the built image is retained"
    );
    assert!(
        names.contains(&format!("hp-{}", d.id).as_str()),
        "the serving container stays"
    );
    assert!(
        !names.contains(&format!("hp-{}-pre", d.id).as_str()),
        "the preliminary container is gone"
    );
    // The temporary container's cleanup is on the books; the serving container's is not attempted.
    let pre = r
        .ledger
        .entries()
        .iter()
        .find(|e| e.key.name.ends_with("-pre"))
        .unwrap();
    assert_eq!(pre.state, EntryState::Released(Released::Removed));
    let serving = r
        .ledger
        .entries()
        .iter()
        .find(|e| e.key.name == format!("hp-{}", d.id))
        .unwrap();
    assert_eq!(serving.cleanup_attempts, 0);
    // Bystanders are exactly as they were.
    for by in &bystanders {
        assert!(fx.world.borrow().objects.contains(by), "{} was touched", by.name);
    }
}

#[test]
fn a_failed_first_deployment_removes_its_container_and_lease_but_keeps_the_image() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend().fail_final_check("HTTP 500");
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let mine = labelled_for(&fx, &d);
    assert_eq!(mine.len(), 1, "only the retained, never-served image remains: {mine:?}");
    assert_eq!(mine[0].kind, ResourceKind::Image);
    assert!(r.ledger.leased_ports().is_empty(), "the port lease was released");
    assert!(r.is_clean());
}

// ---------------------------------------------------------------- cleanup never disappears

#[test]
fn a_failed_cleanup_stays_in_the_outcome_and_in_the_journal() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let serving = format!("hp-{}", d.id);
    let mut b = fx.backend().fail_final_check("HTTP 500").fail_remove(&serving);
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();

    assert_eq!(
        r.status,
        DeploymentStatus::Failed,
        "the status stays truthful about the deployment"
    );
    let problems = r.cleanup_problems();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].key.name, serving);
    assert!(matches!(&problems[0].state, EntryState::CleanupFailed(m) if m.contains("could not remove")));
    assert!(!r.is_clean());
    assert!(fx.world.borrow().has(&serving), "and the object really is still there");
    // The journal alone reproduces the same accounting.
    let replayed = Ledger::replay(&j.ledger_records).unwrap();
    assert_eq!(replayed, r.ledger);
    assert_eq!(replayed.needing_attention().len(), 1);
}

/// B3 changed this contract (it closes the B1 gap "preliminary container removed after the
/// deployment ends"): the preliminary container is removed *before* cutover, so a removal that
/// fails stops the deployment while the old release is untouched — and is still reported.
#[test]
fn a_failed_removal_of_the_preliminary_container_stops_the_deployment_before_cutover() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let pre = format!("hp-{}-pre", d.id);
    let mut b = fx.backend().fail_remove(&pre);
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = d.failure.as_ref().unwrap();
    assert_eq!(f.step, Step::PreliminaryCheck);
    assert!(f.message.contains("before cutover"), "{}", f.message);
    assert!(!b.called(&Call::StartNew), "nothing was cut over");
    assert_eq!(r.cleanup_problems().len(), 1);
    assert_eq!(r.cleanup_problems()[0].key.name, pre);
    assert!(fx.world.borrow().has(&pre), "and the object really is still there");
}

#[test]
fn a_removal_refused_because_labels_changed_is_recorded_not_forced() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let serving = format!("hp-{}", d.id);
    let mut b = fx.backend().fail_final_check("x").refuse_remove(&serving);
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    let e = r.ledger.entries().iter().find(|e| e.key.name == serving).unwrap();
    assert!(matches!(&e.state, EntryState::Refused(m) if m.contains("labels")));
    assert!(fx.world.borrow().has(&serving));
    assert!(r.cleanup_problems().iter().any(|p| p.key.name == serving));
}

#[test]
fn an_unreachable_runtime_makes_cleanup_fail_visibly_instead_of_being_skipped() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend().fail_final_check("x").fail_list();
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let problems = r.cleanup_problems();
    assert!(!problems.is_empty(), "container cleanup could not happen and says so");
    assert!(problems
        .iter()
        .all(|p| matches!(&p.state, EntryState::CleanupFailed(m) if m.contains("could not list"))));
    assert!(
        b.resource_calls.iter().all(|c| matches!(c, ResourceCall::List)),
        "nothing was removed blind"
    );
    assert!(
        r.ledger.leased_ports().is_empty(),
        "the lease is a ledger-only reservation and is released"
    );
}

#[test]
fn a_create_that_fails_after_the_intent_is_resolved_as_never_created() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend().fail_create(ResourceKind::Container);
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let pre = r
        .ledger
        .entries()
        .iter()
        .find(|e| e.key.name.ends_with("-pre"))
        .unwrap();
    assert_eq!(pre.id, None, "no receipt: nothing was created");
    assert_eq!(pre.state, EntryState::Released(Released::AlreadyAbsent));
    assert!(r.is_clean());
}

#[test]
fn a_name_taken_by_a_foreign_object_is_refused_and_left_alone() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let name = format!("hp-{}-pre", d.id);
    let foreign = fx
        .world
        .borrow_mut()
        .add_foreign(ResourceKind::Container, &name, vec![]);
    let mut b = fx.backend();
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let e = r.ledger.entries().iter().find(|e| e.key.name == name).unwrap();
    assert!(matches!(e.state, EntryState::Refused(_)), "got {:?}", e.state);
    assert!(
        fx.world
            .borrow()
            .objects
            .iter()
            .any(|o| o.id == foreign && o.labels.is_empty()),
        "the foreign object is untouched"
    );
    assert!(!r.is_clean(), "the collision is surfaced");
    assert!(
        !b.resource_calls
            .iter()
            .any(|c| matches!(c, ResourceCall::Remove { id, .. } if *id == foreign)),
        "no removal was even attempted on it"
    );
}

#[test]
fn objects_labelled_for_the_deployment_but_unknown_to_the_ledger_are_reported_not_removed() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let stray = harpoon_deploy::backend::harpoon_labels(&d.application_id, &d.id);
    let id = fx
        .world
        .borrow_mut()
        .add_foreign(ResourceKind::Container, "stray", stray);
    let mut b = fx.backend().fail_final_check("x");
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert!(r.unaccounted.iter().any(|o| o.id == id));
    assert!(fx.world.borrow().objects.iter().any(|o| o.id == id));
    assert!(!r.is_clean());
}

// ---------------------------------------------------------------- crash injection

type Script = fn(FakeBackend) -> FakeBackend;

fn scenarios() -> Vec<(&'static str, Script, bool)> {
    vec![
        ("success, first deployment", |b| b, false),
        ("success, with a previous release", |b| b, true),
        ("preliminary check fails", |b| b.fail_preliminary("x"), true),
        ("final check fails, restored", |b| b.fail_final_check("x"), true),
        (
            "first deployment, final check fails",
            |b| b.fail_final_check("x"),
            false,
        ),
    ]
}

#[test]
fn a_crash_at_every_write_including_ledger_writes_is_recoverable_by_label() {
    let mut adopted_somewhere = false;
    let mut crashed_mid_cleanup = false;
    for (name, script, has_prev) in scenarios() {
        let total = {
            let mut fx = Fx::new();
            let mut d = fx.deployment();
            let ctx = if has_prev { fx.previous() } else { Fx::first() };
            let mut b = script(fx.backend());
            let mut j = MemoryJournal::new();
            run(&fx, &mut d, &ctx, &mut b, &mut j).unwrap();
            assert!(
                !j.ledger_records.is_empty(),
                "{name}: the scenario writes ledger records"
            );
            j.writes()
        };
        for crash_at in 0..total {
            let mut fx = Fx::new();
            let bystanders = fx.add_bystanders();
            let mut d = fx.deployment();
            let ctx = if has_prev { fx.previous() } else { Fx::first() };
            let mut b = script(fx.backend());
            let mut old = MemoryJournal::crash_at(crash_at);
            let res = run(&fx, &mut d, &ctx, &mut b, &mut old);
            assert!(
                matches!(res, Err(EngineError::Journal(JournalError::Crash))),
                "{name}@{crash_at}: {res:?}"
            );

            let mut new = MemoryJournal::new();
            let (ledger, end, _) = restart_and_reconcile(&fx, &old, &mut new).unwrap();
            if old
                .ledger_records
                .iter()
                .any(|r| matches!(r, LedgerRecord::CleanupAttempt { .. }))
            {
                crashed_mid_cleanup = true;
            }
            adopted_somewhere |= new
                .ledger_records
                .iter()
                .any(|r| matches!(r, LedgerRecord::Receipt { .. }));

            // 1. nothing is left half-known: every entry is settled or deliberately retained
            for e in ledger.entries() {
                assert!(
                    !matches!(e.state, EntryState::Intended),
                    "{name}@{crash_at}: {} is still an unresolved intent",
                    e.key.name
                );
                if e.disposition != Disposition::Retain
                    && !(e.disposition == Disposition::RemoveOnFailure && end == EndState::Succeeded)
                {
                    assert!(
                        e.is_released(),
                        "{name}@{crash_at}: {} should have been released, is {:?}",
                        e.key.name,
                        e.state
                    );
                }
            }
            // 2. whatever of this deployment's remains in the runtime is exactly what was meant to stay
            for o in labelled_for(&fx, &d) {
                let e = ledger
                    .entries()
                    .iter()
                    .find(|e| e.id.as_deref() == Some(o.id.as_str()))
                    .unwrap_or_else(|| panic!("{name}@{crash_at}: {} survived without a ledger entry", o.name));
                assert!(
                    e.disposition == Disposition::Retain
                        || (e.disposition == Disposition::RemoveOnFailure && end == EndState::Succeeded),
                    "{name}@{crash_at}: {} should have been removed",
                    o.name
                );
            }
            // 3. bystanders are untouched
            for by in &bystanders {
                assert!(
                    fx.world.borrow().objects.contains(by),
                    "{name}@{crash_at}: touched {}",
                    by.name
                );
            }
        }
    }
    assert!(
        adopted_somewhere,
        "at least one crash point fell between create and receipt and was adopted by label"
    );
    assert!(
        crashed_mid_cleanup,
        "at least one crash point fell inside the cleanup phase"
    );
}

/// The first write index at which a crash leaves a container in the runtime with no receipt for it.
fn crash_point_between_create_and_receipt() -> usize {
    let total = {
        let mut fx = Fx::new();
        let mut d = fx.deployment();
        let mut j = MemoryJournal::new();
        let mut b = fx.backend();
        run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
        j.writes()
    };
    (0..total)
        .find(|n| {
            let mut fx = Fx::new();
            let mut d = fx.deployment();
            let mut b = fx.backend();
            let mut j = MemoryJournal::crash_at(*n);
            let _ = run(&fx, &mut d, &Fx::first(), &mut b, &mut j);
            let exists = fx
                .world
                .borrow()
                .objects
                .iter()
                .any(|o| o.kind == ResourceKind::Container);
            let receipted = j
                .ledger_records
                .iter()
                .any(|r| matches!(r, LedgerRecord::Receipt { key, .. } if key.kind == ResourceKind::Container));
            exists && !receipted
        })
        .expect("a crash point between create and receipt exists")
}

#[test]
fn a_crash_during_recovery_itself_is_recoverable_too() {
    let target = crash_point_between_create_and_receipt();
    let mut crashed_recoveries = 0;
    for second_crash in 0.. {
        let mut fx = Fx::new();
        let bystanders = fx.add_bystanders();
        let mut d = fx.deployment();
        let mut b = fx.backend();
        let mut first = MemoryJournal::crash_at(target);
        assert!(run(&fx, &mut d, &Fx::first(), &mut b, &mut first).is_err());
        assert!(
            labelled_for(&fx, &d).iter().any(|o| o.kind == ResourceKind::Container),
            "the container exists but the ledger holds only an intent"
        );

        // Recovery attempt 1 dies at its `second_crash`-th write.
        let mut attempt1 = MemoryJournal::crash_at(second_crash);
        let ledger = match restart_and_reconcile(&fx, &first, &mut attempt1) {
            Ok((ledger, _, _)) => ledger,
            Err(EngineError::Journal(JournalError::Crash)) => {
                crashed_recoveries += 1;
                // Attempt 2 starts from everything that was made durable, by either process.
                let mut durable = MemoryJournal::new();
                durable.snapshots = first.snapshots.clone();
                durable.ledger_records = first
                    .ledger_records
                    .iter()
                    .chain(attempt1.ledger_records.iter())
                    .cloned()
                    .collect();
                let mut attempt2 = MemoryJournal::new();
                restart_and_reconcile(&fx, &durable, &mut attempt2)
                    .expect("the second recovery completes")
                    .0
            }
            Err(e) => panic!("unexpected {e}"),
        };
        for e in ledger.entries() {
            assert!(
                !matches!(e.state, EntryState::Intended),
                "crash in recovery @{second_crash}: {e:?}"
            );
            if e.disposition != Disposition::Retain {
                assert!(e.is_released(), "crash in recovery @{second_crash}: {e:?}");
            }
        }
        assert!(
            labelled_for(&fx, &d).iter().all(|o| o.kind == ResourceKind::Image),
            "only the retained image may remain"
        );
        for by in &bystanders {
            assert!(
                fx.world.borrow().objects.contains(by),
                "crash in recovery @{second_crash}: touched {}",
                by.name
            );
        }
        if second_crash > 0 && attempt1.ledger_records.len() + 1 < second_crash {
            break; // the recovery finished before reaching this crash point: every earlier point has been covered
        }
    }
    assert!(
        crashed_recoveries >= 3,
        "recovery performs several durable writes, each of which was killed in turn"
    );
}

// ---------------------------------------------------------------- deadlines

fn small(build_ms: u64) -> RunConfig {
    RunConfig {
        budgets: Budgets {
            build_ms,
            ..Budgets::default()
        },
        ..RunConfig::default()
    }
}

#[test]
fn a_build_past_its_budget_fails_with_an_explicit_timeout_even_if_it_returned_success() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let ctx = fx.previous();
    let mut b = fx.backend().build_takes(2_000);
    let mut j = MemoryJournal::new();
    let r = run_deployment_with(&mut d, &ctx, &mut b, &mut j, &fx.clock, &small(1_500), &never).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = d.failure.as_ref().unwrap();
    assert_eq!((f.step, f.kind, f.timed_out), (Step::Build, FailureKind::Build, true));
    assert!(
        f.message.contains("timed out after 2000 ms (budget 1500 ms)"),
        "{}",
        f.message
    );
    assert_eq!(d.release, None, "a late build produces no release");
    assert!(
        !b.called(&Call::StopCurrent),
        "a timeout before the cutover stops nothing"
    );
    assert_eq!(r.pause, None);
}

#[test]
fn the_default_build_budget_is_ten_minutes_and_health_budget_sixty_seconds() {
    let b = Budgets::default();
    assert_eq!(b.build_ms, 600_000);
    assert_eq!(b.final_ms, 60_000);
    assert_eq!(b.preliminary_ms, 60_000);
}

#[test]
fn a_final_check_that_runs_past_its_budget_triggers_the_one_bounded_restoration() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let ctx = fx.previous();
    let mut b = fx.backend().final_check_takes(61_000);
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(d.failure.as_ref().unwrap().timed_out);
    assert_eq!(d.failure.as_ref().unwrap().kind, FailureKind::FinalCheck);
    assert!(
        b.called(&Call::RestorePrevious { budget_ms: 60_000 }),
        "the cutover had begun, so one restoration is attempted"
    );
    assert_eq!(r.pause, Some(harpoon_deploy::engine::PauseReason::FailedCutover));
}

#[test]
fn a_stop_that_runs_past_the_cutover_budget_is_a_failed_cutover() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let ctx = fx.previous();
    let mut b = fx.backend();
    // cutover budget below the fake's stop_current time (500 ms)
    let cfg = RunConfig {
        budgets: Budgets {
            cutover_ms: 400,
            ..Budgets::default()
        },
        previous: Some(harpoon_deploy::release::ReleaseConfig::simulated(38000)),
        ..RunConfig::default()
    };
    let mut j = MemoryJournal::new();
    let r = run_deployment_with(&mut d, &ctx, &mut b, &mut j, &fx.clock, &cfg, &never).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = d.failure.as_ref().unwrap();
    assert_eq!((f.step, f.timed_out), (Step::Cutover, true));
    assert!(b.called(&Call::RestorePrevious { budget_ms: 60_000 }));
}

// ---------------------------------------------------------------- evidence

fn unhealthy_evidence() -> CheckEvidence {
    CheckEvidence {
        target: "http://127.0.0.1:39001/".into(),
        expected: "HTTP 2xx, 2 consecutive".into(),
        expect: StatusClass::Success2xx,
        simulated: false,
        attempts: vec![Attempt {
            at_ms: 1,
            observation: Observation::Status(503),
            container_running: true,
        }],
        consecutive_required: 2,
        consecutive_reached: 0,
        elapsed_ms: 60_000,
        verdict: Verdict::NotHealthy,
        scope_note: SCOPE_NOTE,
    }
}

#[test]
fn a_check_that_returns_without_confirming_health_is_not_a_success() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend().final_check_reports(unhealthy_evidence());
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(
        r.status,
        DeploymentStatus::Failed,
        "no success without evidence that says healthy"
    );
    assert_eq!(d.failure.as_ref().unwrap().kind, FailureKind::FinalCheck);
    assert!(d
        .failure
        .as_ref()
        .unwrap()
        .message
        .contains("0/2 consecutive successes"));
    let (step, ev) = r.evidence.last().unwrap();
    assert_eq!(*step, Step::FinalCheck);
    assert_eq!(ev.verdict, Verdict::NotHealthy);
    assert_eq!(ev.scope_note, SCOPE_NOTE);
}

#[test]
fn passing_checks_leave_evidence_with_the_scope_note() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend();
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    let steps: Vec<Step> = r.evidence.iter().map(|(s, _)| *s).collect();
    assert_eq!(steps, vec![Step::PreliminaryCheck, Step::FinalCheck]);
    assert!(r
        .evidence
        .iter()
        .all(|(_, e)| e.is_healthy() && e.scope_note == SCOPE_NOTE));
}

// ---------------------------------------------------------------- warnings and redaction

#[test]
fn a_failed_stop_of_the_new_release_is_a_warning_not_silence() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let ctx = fx.previous();
    let mut b = fx
        .backend()
        .fail_final_check("x")
        .fail_stop_new("docker stop timed out");
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(r
        .warnings
        .iter()
        .any(|w| w.contains("could not stop the failed new release") && w.contains("docker stop timed out")));
    assert!(!r.is_clean());
}

#[test]
fn secrets_are_redacted_from_failure_messages_and_step_logs() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend().fail_build("npm ERR! token=s3cr3t-value rejected");
    let mut j = MemoryJournal::new();
    let cfg = RunConfig {
        redactor: Redactor::new(&[("NPM_TOKEN", "s3cr3t-value")]),
        ..RunConfig::default()
    };
    let r = run_deployment_with(&mut d, &Fx::first(), &mut b, &mut j, &fx.clock, &cfg, &never).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let msg = &d.failure.as_ref().unwrap().message;
    assert!(
        msg.contains("[REDACTED:NPM_TOKEN]") && !msg.contains("s3cr3t-value"),
        "{msg}"
    );
    assert!(
        !format!("{:?}", j.snapshots).contains("s3cr3t-value"),
        "no snapshot carries the secret"
    );
    assert!(!format!("{:?}", r.logs).contains("s3cr3t-value"));
}
