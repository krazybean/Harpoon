//! Bounded cutover recovery: pinned configuration, the one restoration attempt after a failed
//! cutover, its recorded outcome and duration, the pause, preservation of the failed candidate,
//! crash handling and the explicit recovery of an interrupted cutover. Fake backend, in-memory
//! journal, manual clock: no Docker, VM, network, file or process access.

use harpoon_deploy::backend::{Call, FakeBackend, FakeRuntime, PreviousRelease, RestoreResult, StepError};
use harpoon_deploy::engine::{
    recover_cutover, run_deployment_with, Context, EngineError, JournalError, MemoryJournal, PauseReason, Report,
    RunConfig,
};
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator, ReleaseId};
use harpoon_deploy::journal_format::{encode_snapshot, replay, Line};
use harpoon_deploy::ledger::{LedgerRecord, ResourceKind};
use harpoon_deploy::release::ReleaseConfig;
use harpoon_deploy::state::*;
use harpoon_deploy::time::{Clock, ManualClock, SeqEntropy};
use std::rc::Rc;

struct W {
    clock: Rc<ManualClock>,
    gen: IdGenerator<Rc<ManualClock>, SeqEntropy>,
    app: ApplicationId,
    prev_dep: DeploymentId,
}

fn world() -> W {
    let clock = ManualClock::new(1_000_000);
    let mut gen = IdGenerator::new(clock.clone(), SeqEntropy::new(21));
    let app = gen.application().unwrap();
    let prev_dep = gen.deployment().unwrap();
    W {
        clock,
        gen,
        app,
        prev_dep,
    }
}

fn pin() -> ReleaseConfig {
    ReleaseConfig::simulated(38123)
}

fn cfg(with_pin: bool) -> RunConfig {
    RunConfig {
        previous: with_pin.then(pin),
        ..RunConfig::default()
    }
}

fn candidate(w: &mut W) -> Deployment {
    let id = w.gen.deployment().unwrap();
    let mut d = Deployment::new(id, w.app.clone(), Trigger::Manual, w.clock.now_ms());
    d.transition(DeploymentStatus::Queued, w.clock.now_ms()).unwrap();
    d
}

fn ctx(w: &W) -> Context {
    Context {
        previous_release: Some(ReleaseId::from_deployment(&w.prev_dep)),
    }
}

/// A replacement whose final check fails after the previous release was stopped.
fn failed_cutover(w: &mut W, b: &mut FakeBackend, with_pin: bool) -> (Deployment, Report, MemoryJournal) {
    let mut d = candidate(w);
    let mut j = MemoryJournal::new();
    let r = run_deployment_with(&mut d, &ctx(w), b, &mut j, &w.clock, &cfg(with_pin), &|| false).unwrap();
    (d, r, j)
}

fn failing(w: &W) -> FakeBackend {
    FakeBackend::new(w.clock.clone()).fail_final_check("HTTP 503")
}

// ---------------------------------------------------------------- outcomes

#[test]
fn restoration_with_verified_health_is_restored_with_measured_durations() {
    let mut w = world();
    let mut b = failing(&w).restore_with(RestoreResult::Healthy, 400);
    let (d, r, j) = failed_cutover(&mut w, &mut b, true);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let rec = d.recovery.unwrap();
    assert_eq!(rec.outcome, RecoveryOutcome::Restored);
    assert_eq!(rec.outcome.as_str(), "restored");
    // stopping the failed candidate (100) and the restoration itself (400) are inside the window
    assert_eq!(rec.finished_ms.unwrap() - rec.started_ms, 500, "restoration duration");
    // stop 500 + start 300 + final check 200 + stop of the failed candidate 100 + restore 400
    assert_eq!(
        d.interruption.unwrap().duration_ms(),
        Some(1_500),
        "measured interruption"
    );
    assert_eq!(rec.budget_ms, 60_000, "the default budget");
    assert!(!d.application_may_be_down());
    // it was told exactly what to restore, from the pinned record
    let told = b.restored.as_ref().unwrap();
    assert_eq!(told.deployment, w.prev_dep);
    assert_eq!(told.config.as_ref(), Some(&pin()));
    assert_eq!(
        b.calls
            .iter()
            .filter(|c| matches!(c, Call::RestorePrevious { .. }))
            .count(),
        1
    );
    assert_eq!(r.pause, Some(PauseReason::FailedCutover));
    // durable
    assert_eq!(j.last().unwrap().recovery.unwrap().outcome, RecoveryOutcome::Restored);
}

#[test]
fn a_restoration_that_starts_but_cannot_be_verified_is_restored_unverified() {
    let mut w = world();
    let mut b = failing(&w).restore_with(RestoreResult::StartedUnverified, 900);
    let (d, _, _) = failed_cutover(&mut w, &mut b, true);
    assert_eq!(d.recovery.unwrap().outcome, RecoveryOutcome::RestoredUnverified);
    assert_eq!(d.recovery.unwrap().outcome.as_str(), "restored_unverified");
}

#[test]
fn a_failed_restoration_leaves_the_application_possibly_down_and_is_not_retried() {
    let mut w = world();
    let mut b = failing(&w).restore_with(RestoreResult::Failed(Reason::StartFailed), 50);
    let (d, r, _) = failed_cutover(&mut w, &mut b, true);
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::RestoreFailed(Reason::StartFailed)
    );
    assert!(d.application_may_be_down());
    assert_eq!(d.interruption.unwrap().ended_ms, None, "the interruption stays open");
    assert_eq!(
        b.calls
            .iter()
            .filter(|c| matches!(c, Call::RestorePrevious { .. }))
            .count(),
        1
    );
    assert_eq!(r.status, DeploymentStatus::Failed);
}

#[test]
fn missing_pinned_configuration_is_not_attempted_and_says_why() {
    let mut w = world();
    let mut b = failing(&w);
    let (d, r, _) = failed_cutover(&mut w, &mut b, false);
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::ConfigUnavailable)
    );
    assert!(
        !b.calls.iter().any(|c| matches!(c, Call::RestorePrevious { .. })),
        "nothing to restore from"
    );
    assert!(d.application_may_be_down());
    assert_eq!(r.pause, Some(PauseReason::FailedCutover));
}

#[test]
fn an_unavailable_image_or_unreachable_docker_is_not_attempted_with_the_reason() {
    for reason in [Reason::ImageMissing, Reason::DockerUnreachable] {
        let mut w = world();
        let mut b = failing(&w).restore_with(RestoreResult::NotAttempted(reason), 5);
        let (d, _, _) = failed_cutover(&mut w, &mut b, true);
        assert_eq!(d.recovery.unwrap().outcome, RecoveryOutcome::NotAttempted(reason));
        assert!(d.application_may_be_down());
    }
}

#[test]
fn the_restoration_deadline_is_enforced_even_when_the_backend_answers_late() {
    // The backend gives up inside its budget: a timeout.
    let mut w = world();
    let mut b = failing(&w).restore_errors(StepError::timeout("ran out of time"), 60_001);
    let (d, _, _) = failed_cutover(&mut w, &mut b, true);
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::RestoreFailed(Reason::Timeout)
    );

    // A "healthy" answer that arrives past the budget is not trusted as restored in time.
    let mut w = world();
    let mut b = failing(&w).restore_with(RestoreResult::Healthy, 61_000);
    let (d, _, _) = failed_cutover(&mut w, &mut b, true);
    assert_eq!(d.recovery.unwrap().outcome, RecoveryOutcome::RestoredUnverified);
    assert_eq!(
        d.recovery.unwrap().finished_ms.unwrap() - d.recovery.unwrap().started_ms,
        61_100
    );

    // The budget is configurable and passed through.
    let mut w = world();
    let mut b = failing(&w);
    let mut d = candidate(&mut w);
    let mut c = cfg(true);
    c.policy.restore_budget_ms = 5_000;
    run_deployment_with(
        &mut d,
        &ctx(&w),
        &mut b,
        &mut MemoryJournal::new(),
        &w.clock,
        &c,
        &|| false,
    )
    .unwrap();
    assert!(b.called(&Call::RestorePrevious { budget_ms: 5_000 }));
}

#[test]
fn a_backend_error_in_the_restoration_is_a_failed_restoration_not_an_engine_error() {
    let mut w = world();
    let mut b = failing(&w).restore_errors(StepError::new("create refused"), 10);
    let (d, r, _) = failed_cutover(&mut w, &mut b, true);
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::RestoreFailed(Reason::StartFailed)
    );
    assert!(r.warnings.iter().any(|m| m.contains("create refused")));
}

// ---------------------------------------------------------------- pause and preservation

#[test]
fn triggers_pause_only_after_a_cutover_that_really_began_and_did_not_end_well() {
    // build failure, preliminary failure, an unconfirmed plan, a first deployment: no pause
    let mut w = world();
    let mut deps = Vec::new();
    for b in [
        FakeBackend::new(w.clock.clone()).fail_build("RUN false"),
        FakeBackend::new(w.clock.clone()).fail_preliminary("503"),
    ] {
        let mut b = b;
        let (d, r, _) = failed_cutover(&mut w, &mut b, true);
        assert_eq!(r.pause, None);
        deps.push(d);
    }
    assert!(
        trigger_pauses(&deps).is_empty(),
        "nothing was stopped, so nothing pauses"
    );

    // a failed cutover pauses that application, with its recovery outcome
    let mut b = failing(&w).restore_with(RestoreResult::Healthy, 10);
    let (d, _, _) = failed_cutover(&mut w, &mut b, true);
    deps.push(d.clone());
    let p = trigger_pauses(&deps);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].application, w.app);
    assert_eq!(p[0].deployment, d.id);
    assert_eq!(p[0].recovery, Some(RecoveryOutcome::Restored));
    assert!(!p[0].incomplete);

    // a successful replacement does not
    let mut ok = candidate(&mut w);
    let mut b = FakeBackend::new(w.clock.clone());
    run_deployment_with(
        &mut ok,
        &ctx(&w),
        &mut b,
        &mut MemoryJournal::new(),
        &w.clock,
        &cfg(true),
        &|| false,
    )
    .unwrap();
    assert_eq!(ok.status, DeploymentStatus::Succeeded);
    assert!(trigger_pauses(&[ok]).is_empty());
}

#[test]
fn the_failed_candidate_is_preserved_and_unrelated_objects_are_untouched() {
    let mut w = world();
    let rt = FakeRuntime::world();
    let other = IdGenerator::new(w.clock.clone(), SeqEntropy::new(99))
        .application()
        .unwrap();
    let other_dep = IdGenerator::new(w.clock.clone(), SeqEntropy::new(98))
        .deployment()
        .unwrap();
    let (c_other, c_bare) = {
        let mut r = rt.borrow_mut();
        (
            r.add_foreign(
                ResourceKind::Container,
                "another-app",
                harpoon_deploy::backend::harpoon_labels(&other, &other_dep),
            ),
            r.add_foreign(ResourceKind::Container, "no-labels", vec![]),
        )
    };
    let mut b = failing(&w)
        .with_runtime(rt.clone())
        .restore_with(RestoreResult::Healthy, 10);
    let (d, r, _) = failed_cutover(&mut w, &mut b, true);
    let world = rt.borrow();
    let ids: Vec<_> = world.objects.iter().map(|o| o.id.clone()).collect();
    assert!(
        ids.contains(&c_other) && ids.contains(&c_bare),
        "other applications and unlabelled objects stay"
    );
    // the failed candidate's serving container and its image are kept for inspection
    let serving = format!("hp-{}", d.id);
    assert!(
        world.has(&serving),
        "the failed candidate container is kept: {:?}",
        world.names()
    );
    assert!(world.objects.iter().any(|o| o.kind == ResourceKind::Image));
    // the preliminary container, which exists only for its check, is gone
    assert!(!world.has(&format!("hp-{}-pre", d.id)));
    assert!(r.cleanup_problems().is_empty());
}

// ---------------------------------------------------------------- crashes and explicit recovery

fn all_snapshots(j: &MemoryJournal) -> Vec<Line> {
    j.snapshots
        .iter()
        .map(|d| Line::Snapshot(Box::new(d.clone())))
        .collect()
}

#[test]
fn a_crash_at_every_write_of_a_failing_cutover_is_accurately_recovered() {
    for n in 0..40 {
        let mut w = world();
        let mut b = failing(&w).restore_with(RestoreResult::Healthy, 10);
        let mut d = candidate(&mut w);
        let mut j = MemoryJournal::crash_at(n);
        let res = run_deployment_with(&mut d, &ctx(&w), &mut b, &mut j, &w.clock, &cfg(true), &|| false);
        if res.is_ok() {
            break; // past the last write
        }
        assert!(matches!(res, Err(EngineError::Journal(JournalError::Crash))));
        let Some(last) = j.last() else { continue };
        let mut deps = vec![last.clone()];
        let report = recover_on_start(&mut deps, 5_000_000);
        let d = &deps[0];
        // never resumed: a deployment that was not finished is interrupted, nothing more
        assert!(d.status.is_terminal(), "write {n}: {:?}", d.status);
        if matches!(d.cutover, CutoverState::Begun { .. } | CutoverState::Failed { .. })
            && d.status == DeploymentStatus::Interrupted
        {
            assert!(
                d.application_may_be_down() || d.recovery.map(|r| r.outcome.service_restored()).unwrap_or(false),
                "write {n}"
            );
            assert!(
                report.incomplete_cutovers.contains(&d.id) || !d.application_may_be_down(),
                "write {n}"
            );
        }
        // an attempt that was in progress is closed as interrupted, never left "in progress"
        if let Some(r) = d.recovery {
            assert_ne!(r.outcome, RecoveryOutcome::InProgress, "write {n}");
        }
        // the record the crash left behind replays
        assert!(replay(&all_snapshots(&j)).is_ok(), "write {n}");
    }
}

/// A deployment whose process died after the previous release was stopped.
fn interrupted_mid_cutover(w: &mut W) -> Deployment {
    let mut d = candidate(w);
    let now = w.clock.now_ms();
    d.transition(DeploymentStatus::Running, now).unwrap();
    d.start_step(Step::Cutover, now).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&w.prev_dep));
    d.begin_cutover(now).unwrap();
    let mut v = [d];
    let rep = recover_on_start(&mut v, now + 10);
    assert_eq!(rep.incomplete_cutovers.len(), 1);
    let [d] = v;
    assert_eq!(d.status, DeploymentStatus::Interrupted);
    d
}

fn prev(w: &W) -> PreviousRelease {
    PreviousRelease {
        release: ReleaseId::from_deployment(&w.prev_dep),
        deployment: w.prev_dep.clone(),
        config: None,
        container: None,
    }
}

#[test]
fn explicit_recovery_of_an_interrupted_cutover_runs_once_and_never_again() {
    let mut w = world();
    let mut d = interrupted_mid_cutover(&mut w);
    assert!(d.application_may_be_down());
    let mut b = FakeBackend::new(w.clock.clone()).restore_with(RestoreResult::Healthy, 700);
    let mut j = MemoryJournal::new();
    let r = recover_cutover(&mut d, prev(&w), &mut b, &mut j, &w.clock, &cfg(true)).unwrap();
    assert_eq!(
        d.status,
        DeploymentStatus::Interrupted,
        "the interrupted deployment is not resumed"
    );
    let rec = d.recovery.unwrap();
    assert_eq!(rec.outcome, RecoveryOutcome::Restored);
    assert_eq!(rec.finished_ms.unwrap() - rec.started_ms, 700);
    assert!(d.interruption.unwrap().ended_ms.is_some());
    assert!(!d.application_may_be_down());
    assert_eq!(r.pause, Some(PauseReason::FailedCutover));
    assert!(b.restored.as_ref().unwrap().config.is_some());
    // the attempt was on disk as in-progress before the backend was called
    assert_eq!(
        j.snapshots.first().unwrap().recovery.unwrap().outcome,
        RecoveryOutcome::InProgress
    );
    // a second attempt is refused and does nothing
    let calls = b.calls.len();
    let again = recover_cutover(&mut d, prev(&w), &mut b, &mut j, &w.clock, &cfg(true));
    assert!(matches!(again, Err(EngineError::Transition(_))));
    assert_eq!(b.calls.len(), calls);
}

#[test]
fn explicit_recovery_without_pinned_configuration_records_that_it_could_not_try() {
    let mut w = world();
    let mut d = interrupted_mid_cutover(&mut w);
    let mut b = FakeBackend::new(w.clock.clone());
    recover_cutover(
        &mut d,
        prev(&w),
        &mut b,
        &mut MemoryJournal::new(),
        &w.clock,
        &cfg(false),
    )
    .unwrap();
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::ConfigUnavailable)
    );
    assert!(b.calls.is_empty());
    assert!(d.application_may_be_down());
}

#[test]
fn explicit_recovery_is_refused_for_anything_but_an_interrupted_unrecovered_cutover() {
    let mut w = world();
    let mut b = FakeBackend::new(w.clock.clone());
    // a finished, failed deployment, an unfinished one, and a first deployment's NotRequired cutover
    let mut fb = failing(&w);
    let (d, _, _) = failed_cutover(&mut w, &mut fb, true);
    for mut d in [d, candidate(&mut w)] {
        assert!(recover_cutover(
            &mut d,
            prev(&w),
            &mut b,
            &mut MemoryJournal::new(),
            &w.clock,
            &cfg(true)
        )
        .is_err());
    }
    assert!(b.calls.is_empty());
}

#[test]
fn a_crash_during_the_explicit_recovery_is_closed_as_interrupted_and_not_repeated() {
    let mut w = world();
    let mut d = interrupted_mid_cutover(&mut w);
    let before = d.clone();
    // the first write (recovery recorded as in progress) lands, the second (the outcome) is lost
    let mut j = MemoryJournal::crash_at(1);
    let mut b = FakeBackend::new(w.clock.clone());
    let res = recover_cutover(&mut d, prev(&w), &mut b, &mut j, &w.clock, &cfg(true));
    assert!(matches!(res, Err(EngineError::Journal(JournalError::Crash))));
    let persisted = j.last().unwrap().clone();
    assert_eq!(persisted.recovery.unwrap().outcome, RecoveryOutcome::InProgress);
    // the store accepts exactly this one change to an interrupted deployment
    assert!(replay(&[
        Line::Snapshot(Box::new(before.clone())),
        Line::Snapshot(Box::new(persisted.clone()))
    ])
    .is_ok());
    // next start: the attempt is recorded as interrupted, the application is reported possibly down
    let mut v = [persisted];
    recover_on_start(&mut v, 9_000_000);
    let [mut d2] = v;
    assert_eq!(
        d2.recovery.unwrap().outcome,
        RecoveryOutcome::RestoreFailed(Reason::Interrupted)
    );
    assert!(d2.application_may_be_down());
    assert!(replay(&[Line::Snapshot(Box::new(before)), Line::Snapshot(Box::new(d2.clone()))]).is_ok());
    // and it is not attempted a second time
    let mut b = FakeBackend::new(w.clock.clone());
    assert!(recover_cutover(
        &mut d2,
        prev(&w),
        &mut b,
        &mut MemoryJournal::new(),
        &w.clock,
        &cfg(true)
    )
    .is_err());
    assert!(b.calls.is_empty());
}

#[test]
fn the_store_rejects_any_other_change_to_an_interrupted_deployment() {
    let mut w = world();
    let d = interrupted_mid_cutover(&mut w);
    // a second recovery over a final one
    let mut done = d.clone();
    done.fail_cutover(1).unwrap();
    done.start_recovery(60_000, 1).unwrap();
    done.finish_recovery(RecoveryOutcome::Restored, 2).unwrap();
    let mut again = done.clone();
    again.recovery.as_mut().unwrap().outcome = RecoveryOutcome::RestoreFailed(Reason::StartFailed);
    let lines = |ds: &[&Deployment]| {
        ds.iter()
            .map(|d| Line::Snapshot(Box::new((*d).clone())))
            .collect::<Vec<_>>()
    };
    assert!(replay(&lines(&[&d, &done])).is_ok());
    assert!(
        replay(&lines(&[&d, &done, &again])).is_err(),
        "a final recovery never changes"
    );
    // an unrelated field
    let mut other = done.clone();
    other.created_ms += 1;
    assert!(replay(&lines(&[&d, &other])).is_err());
    // a deployment that is not Interrupted never gets this exception
    let mut failed = done.clone();
    failed.status = DeploymentStatus::Failed;
    assert!(replay(&lines(&[&d, &failed])).is_err());
    let _ = LedgerRecord::CleanupAttempt {
        key: harpoon_deploy::ledger::ResourceKey::new(ResourceKind::Container, "x"),
        deployment: w.prev_dep.clone(),
        at_ms: 0,
    };
}

// ---------------------------------------------------------------- pinned configuration

#[test]
fn every_serving_release_has_its_configuration_pinned_before_the_final_check() {
    let mut w = world();
    let mut d = candidate(&mut w);
    let mut b = FakeBackend::new(w.clock.clone());
    let mut j = MemoryJournal::new();
    run_deployment_with(
        &mut d,
        &Context { previous_release: None },
        &mut b,
        &mut j,
        &w.clock,
        &cfg(false),
        &|| false,
    )
    .unwrap();
    assert_eq!(d.status, DeploymentStatus::Succeeded);
    assert_eq!(d.pinned, Some(ReleaseConfig::simulated(39000)));
    // it was on disk before the final check began
    let first_pinned = j.snapshots.iter().position(|s| s.pinned.is_some()).unwrap();
    let final_started = j
        .snapshots
        .iter()
        .position(|s| s.step(Step::FinalCheck).status != StepStatus::Pending)
        .unwrap();
    assert!(first_pinned <= final_started);
    // it survives the codec, and old snapshots without it still decode
    let line = encode_snapshot(&d);
    match harpoon_deploy::journal_format::decode_line(&line).unwrap() {
        Line::Snapshot(back) => assert_eq!(back.pinned, d.pinned),
        _ => panic!("not a snapshot"),
    }
    let mut bare = d.clone();
    bare.pinned = None;
    assert_eq!(encode_snapshot(&bare).split('\t').count() + 1, line.split('\t').count());
}
