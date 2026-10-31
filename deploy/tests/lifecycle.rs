//! Deterministic lifecycle tests: scripted fake backend, in-memory journal, manual clock.
//! No Docker, VM, network, file or process access.

use harpoon_deploy::backend::{Call, Event, FakeBackend, RestoreResult, Trace};
use harpoon_deploy::engine::{
    run_deployment, run_deployment_with, Context, EngineError, JournalError, MemoryJournal, PauseReason, Policy,
    RunConfig,
};
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator, ReleaseId};
use harpoon_deploy::state::*;
use harpoon_deploy::time::{ManualClock, SeqEntropy};
use std::rc::Rc;

struct World {
    clock: Rc<ManualClock>,
    gen: IdGenerator<Rc<ManualClock>, SeqEntropy>,
    app: ApplicationId,
}

impl World {
    fn new() -> World {
        let clock = ManualClock::new(1_000_000);
        let mut gen = IdGenerator::new(clock.clone(), SeqEntropy::new(7));
        let app = gen.application().unwrap();
        World { clock, gen, app }
    }
    fn deployment(&mut self) -> Deployment {
        let id = self.gen.deployment().unwrap();
        let mut d = Deployment::new(id, self.app.clone(), Trigger::Manual, self.clock.now());
        d.transition(DeploymentStatus::Queued, self.clock.now()).unwrap();
        d
    }
    fn previous(&mut self) -> Context {
        let prev: DeploymentId = self.gen.deployment().unwrap();
        Context {
            previous_release: Some(ReleaseId::from_deployment(&prev)),
        }
    }
}

trait Now {
    fn now(&self) -> u64;
}
impl Now for Rc<ManualClock> {
    fn now(&self) -> u64 {
        use harpoon_deploy::time::Clock;
        self.now_ms()
    }
}

fn never() -> bool {
    false
}

fn run(
    w: &World,
    d: &mut Deployment,
    ctx: &Context,
    b: &mut FakeBackend,
    j: &mut MemoryJournal,
) -> Result<harpoon_deploy::engine::Report, EngineError> {
    run_deployment_with(d, ctx, b, j, &w.clock, &pinned(Policy::default()), &never)
}

/// A previous release whose pinned configuration was stored, so that a failed cutover can restore it.
fn pinned(policy: Policy) -> RunConfig {
    RunConfig {
        policy,
        previous: Some(harpoon_deploy::release::ReleaseConfig::simulated(38000)),
        ..RunConfig::default()
    }
}

// ---------------------------------------------------------------- success paths

#[test]
fn success_with_a_previous_release_records_steps_cutover_and_interruption() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (FakeBackend::new(w.clock.clone()), MemoryJournal::new());
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    assert_eq!(r.pause, None);
    assert!(matches!(d.cutover, CutoverState::Confirmed { .. }));
    for s in [
        Step::Source,
        Step::Build,
        Step::PreliminaryCheck,
        Step::Cutover,
        Step::FinalCheck,
    ] {
        assert_eq!(d.step(s).status, StepStatus::Succeeded, "{s:?}");
    }
    assert_eq!(d.step(Step::Recovery).status, StepStatus::Skipped);
    assert_eq!(d.release, Some(ReleaseId::from_deployment(&d.id)));
    // interruption = stop_current (500) + start_new (300) + final_check (200)
    assert_eq!(d.interruption.unwrap().duration_ms(), Some(1000));
    assert_eq!(
        b.calls,
        vec![
            Call::PrepareSource,
            Call::Build,
            Call::PreliminaryCheck,
            Call::StopCurrent,
            Call::StartNew,
            Call::FinalCheck
        ]
    );
}

#[test]
fn first_deployment_has_no_cutover_no_interruption_and_never_stops_anything() {
    let mut w = World::new();
    let mut d = w.deployment();
    let ctx = Context { previous_release: None };
    let (mut b, mut j) = (FakeBackend::new(w.clock.clone()), MemoryJournal::new());
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    assert_eq!(d.cutover, CutoverState::NotRequired);
    assert!(d.interruption.is_none());
    assert!(!b.called(&Call::StopCurrent));
    assert!(!d.application_may_be_down());
}

// ---------------------------------------------------------------- failures before the cutover

fn assert_nothing_stopped(b: &FakeBackend, d: &Deployment, r: &harpoon_deploy::engine::Report) {
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(r.pause, None, "an ordinary failure must not pause triggers");
    assert!(!b.called(&Call::StopCurrent), "nothing may be stopped");
    assert!(
        !b.calls.iter().any(|c| matches!(c, Call::RestorePrevious { .. })),
        "no restoration without a cutover"
    );
    assert_eq!(d.cutover, CutoverState::NotStarted);
    assert!(d.recovery.is_none() && d.interruption.is_none());
    assert!(!d.application_may_be_down());
}

#[test]
fn source_failure_stops_nothing_and_does_not_pause() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone()).fail_source("clone failed"),
        MemoryJournal::new(),
    );
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_nothing_stopped(&b, &d, &r);
    assert_eq!(d.failure.as_ref().unwrap().kind, FailureKind::Source);
    assert!(!b.called(&Call::Build));
}

#[test]
fn build_failure_stops_nothing_and_leaves_no_release() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone()).fail_build("npm run build exited 2"),
        MemoryJournal::new(),
    );
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_nothing_stopped(&b, &d, &r);
    assert_eq!(d.release, None, "a failed build produces no release");
    assert_eq!(d.failure.as_ref().unwrap().step, Step::Build);
    assert_eq!(d.step(Step::PreliminaryCheck).status, StepStatus::Skipped);
}

#[test]
fn preliminary_check_failure_stops_nothing_and_does_not_pause() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone()).fail_preliminary("503"),
        MemoryJournal::new(),
    );
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_nothing_stopped(&b, &d, &r);
    assert_eq!(d.failure.as_ref().unwrap().kind, FailureKind::PreliminaryCheck);
    assert!(
        d.release.is_some(),
        "the image was built; the release record exists but never served"
    );
}

// ---------------------------------------------------------------- failed cutover and recovery

fn assert_failed_cutover(d: &Deployment, r: &harpoon_deploy::engine::Report, b: &FakeBackend) {
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(
        r.pause,
        Some(PauseReason::FailedCutover),
        "an actual failed cutover pauses triggers"
    );
    assert!(matches!(d.cutover, CutoverState::Failed { .. }));
    assert!(
        b.calls
            .iter()
            .any(|c| matches!(c, Call::RestorePrevious { budget_ms: 60_000 })),
        "one bounded attempt with the policy budget"
    );
    assert_eq!(
        b.calls
            .iter()
            .filter(|c| matches!(c, Call::RestorePrevious { .. }))
            .count(),
        1,
        "never more than one attempt"
    );
}

#[test]
fn final_check_failure_restores_the_previous_release() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone()).fail_final_check("HTTP 503"),
        MemoryJournal::new(),
    );
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_failed_cutover(&d, &r, &b);
    assert_eq!(d.recovery.unwrap().outcome, RecoveryOutcome::Restored);
    assert_eq!(d.failure.as_ref().unwrap().kind, FailureKind::FinalCheck);
    assert!(
        b.called(&Call::StopNew),
        "the failed new release is stopped to free its address first"
    );
    let i = d.interruption.unwrap();
    assert!(i.ended_ms.is_some());
    // stop 500 + start 300 + final 200 + stop_new 100 + restore 400
    assert_eq!(i.duration_ms(), Some(1500));
    assert!(!d.application_may_be_down());
    assert_eq!(d.step(Step::Recovery).status, StepStatus::Succeeded);
}

#[test]
fn start_failure_after_stopping_the_old_release_also_restores_it() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone()).fail_start_new("port in use"),
        MemoryJournal::new(),
    );
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_failed_cutover(&d, &r, &b);
    assert_eq!(d.recovery.unwrap().outcome, RecoveryOutcome::Restored);
    assert_eq!(d.failure.as_ref().unwrap().step, Step::Cutover);
    assert!(!b.called(&Call::StopNew), "a release that never started needs no stop");
    assert!(!b.called(&Call::FinalCheck));
}

#[test]
fn failure_to_stop_the_old_release_is_treated_as_a_failed_cutover() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone()).fail_stop_current("docker timeout"),
        MemoryJournal::new(),
    );
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_failed_cutover(&d, &r, &b);
    assert!(!b.called(&Call::StartNew));
}

#[test]
fn restored_but_unhealthy_is_reported_as_unverified_not_as_restored() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone())
            .fail_final_check("503")
            .restore_with(RestoreResult::StartedUnverified, 400),
        MemoryJournal::new(),
    );
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_failed_cutover(&d, &r, &b);
    assert_eq!(d.recovery.unwrap().outcome, RecoveryOutcome::RestoredUnverified);
    assert!(d.interruption.unwrap().ended_ms.is_some(), "a release is running again");
}

#[test]
fn failed_restoration_leaves_the_application_possibly_down_and_the_interruption_open() {
    let mut w = World::new();
    for reason in [
        Reason::SecretUnavailable,
        Reason::Timeout,
        Reason::StartFailed,
        Reason::DockerUnreachable,
    ] {
        let (mut d, ctx) = (w.deployment(), w.previous());
        let (mut b, mut j) = (
            FakeBackend::new(w.clock.clone())
                .fail_final_check("503")
                .restore_with(RestoreResult::Failed(reason), 100),
            MemoryJournal::new(),
        );
        let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
        assert_failed_cutover(&d, &r, &b);
        assert_eq!(d.recovery.unwrap().outcome, RecoveryOutcome::RestoreFailed(reason));
        assert_eq!(d.interruption.unwrap().ended_ms, None, "still interrupted");
        assert!(d.application_may_be_down());
        assert_eq!(d.step(Step::Recovery).status, StepStatus::Failed);
    }
}

#[test]
fn restoration_that_cannot_be_attempted_is_recorded_with_its_reason() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone())
            .fail_final_check("503")
            .restore_with(RestoreResult::NotAttempted(Reason::ImageMissing), 0),
        MemoryJournal::new(),
    );
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_failed_cutover(&d, &r, &b);
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::ImageMissing)
    );
    assert!(d.application_may_be_down());
}

#[test]
fn custom_restore_budget_is_passed_through_unchanged() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone()).fail_final_check("x"),
        MemoryJournal::new(),
    );
    run_deployment_with(
        &mut d,
        &ctx,
        &mut b,
        &mut j,
        &w.clock,
        &pinned(Policy {
            restore_budget_ms: 5_000,
        }),
        &never,
    )
    .unwrap();
    assert!(b.called(&Call::RestorePrevious { budget_ms: 5_000 }));
    assert_eq!(d.recovery.unwrap().budget_ms, 5_000);
}

#[test]
fn a_first_deployment_that_fails_after_starting_never_attempts_a_restore() {
    let mut w = World::new();
    let mut d = w.deployment();
    let ctx = Context { previous_release: None };
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone()).fail_final_check("503"),
        MemoryJournal::new(),
    );
    let r = run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(r.pause, None, "no cutover happened, so no pause");
    assert!(b.called(&Call::StopNew));
    assert!(!b.calls.iter().any(|c| matches!(c, Call::RestorePrevious { .. })));
    assert!(d.recovery.is_none());
}

// ---------------------------------------------------------------- cancellation

#[test]
fn cancel_before_the_cutover_ends_cleanly_and_stops_nothing() {
    for cancel_after_polls in 0..4usize {
        let mut w = World::new();
        let (mut d, ctx) = (w.deployment(), w.previous());
        let (mut b, mut j) = (FakeBackend::new(w.clock.clone()), MemoryJournal::new());
        let polls = std::cell::Cell::new(0usize);
        let cancel = || {
            let n = polls.get();
            polls.set(n + 1);
            n >= cancel_after_polls
        };
        let r = run_deployment(&mut d, &ctx, &mut b, &mut j, &w.clock, &Policy::default(), &cancel).unwrap();
        assert_eq!(r.status, DeploymentStatus::Cancelled, "poll {cancel_after_polls}");
        assert_eq!(r.pause, None);
        assert!(!b.called(&Call::StopCurrent));
        assert_eq!(d.cutover, CutoverState::NotStarted);
    }
}

#[test]
fn cancel_requested_after_the_cutover_began_is_ignored() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (FakeBackend::new(w.clock.clone()), MemoryJournal::new());
    // Cancels only once the engine has already stopped the old release (4 pre-cutover polls).
    let polls = std::cell::Cell::new(0usize);
    let cancel = || {
        polls.set(polls.get() + 1);
        polls.get() > 4
    };
    let r = run_deployment(&mut d, &ctx, &mut b, &mut j, &w.clock, &Policy::default(), &cancel).unwrap();
    assert_eq!(
        r.status,
        DeploymentStatus::Succeeded,
        "the switch completes; it is never left half-applied"
    );
    assert_eq!(polls.get(), 4, "cancel is not polled once the cutover has begun");
}

// ---------------------------------------------------------------- journal ordering and crashes

#[test]
fn the_cutover_journal_is_written_before_the_old_release_is_stopped() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (FakeBackend::new(w.clock.clone()), MemoryJournal::new());
    run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    let first_begun = j
        .snapshots
        .iter()
        .position(|s| matches!(s.cutover, CutoverState::Begun { .. }))
        .unwrap();
    let snap = &j.snapshots[first_begun];
    assert_eq!(snap.step(Step::Cutover).status, StepStatus::Running);
    assert!(snap.interruption.unwrap().ended_ms.is_none());
    // Every earlier snapshot predates any stop: they all have cutover NotStarted.
    assert!(j.snapshots[..first_begun]
        .iter()
        .all(|s| s.cutover == CutoverState::NotStarted));
}

#[test]
fn recovery_is_persisted_as_in_progress_before_the_restore_runs() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (
        FakeBackend::new(w.clock.clone()).fail_final_check("503"),
        MemoryJournal::new(),
    );
    run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    assert!(j
        .snapshots
        .iter()
        .any(|s| s.recovery.map(|r| r.outcome) == Some(RecoveryOutcome::InProgress)));
    assert_eq!(j.last().unwrap().recovery.unwrap().outcome, RecoveryOutcome::Restored);
}

/// Kill the engine at every persistence point of every scenario, then run startup
/// recovery on whatever was last written. The result must always be consistent.
#[test]
fn a_crash_at_any_write_recovers_to_a_consistent_state() {
    type Scenario = fn(FakeBackend) -> FakeBackend;
    let scenarios: Vec<(&str, Scenario, bool)> = vec![
        ("success", |b| b, true),
        ("build fails", |b| b.fail_build("x"), true),
        ("final check fails, restored", |b| b.fail_final_check("x"), true),
        (
            "restore fails",
            |b| {
                b.fail_final_check("x")
                    .restore_with(RestoreResult::Failed(Reason::Timeout), 10)
            },
            true,
        ),
        ("first deployment", |b| b, false),
    ];
    for (name, script, has_previous) in scenarios {
        // First learn how many writes a full run makes.
        let total = {
            let mut w = World::new();
            let mut d = w.deployment();
            let ctx = if has_previous {
                w.previous()
            } else {
                Context { previous_release: None }
            };
            let (mut b, mut j) = (script(FakeBackend::new(w.clock.clone())), MemoryJournal::new());
            run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
            j.snapshots.len()
        };
        assert!(total >= 3, "{name}: expected several writes, got {total}");
        for crash_at in 0..total {
            let mut w = World::new();
            let mut d = w.deployment();
            let ctx = if has_previous {
                w.previous()
            } else {
                Context { previous_release: None }
            };
            let (mut b, mut j) = (
                script(FakeBackend::new(w.clock.clone())),
                MemoryJournal::crash_at(crash_at),
            );
            let res = run(&w, &mut d, &ctx, &mut b, &mut j);
            assert!(
                matches!(res, Err(EngineError::Journal(JournalError::Crash))),
                "{name}@{crash_at}"
            );

            // What is on disk is the last successful snapshot (or nothing, if the first write died).
            let mut on_disk: Vec<Deployment> = match j.last() {
                Some(s) => vec![s.clone()],
                None => vec![{
                    let mut q = w.deployment();
                    q.status = DeploymentStatus::Queued;
                    q
                }],
            };
            let before = on_disk[0].clone();
            let report = harpoon_deploy::state::recover_on_start(&mut on_disk, w.clock_now());
            let after = &on_disk[0];

            if before.status.is_terminal() {
                assert!(
                    report.interrupted.is_empty(),
                    "{name}@{crash_at}: terminal records are left alone"
                );
                assert_eq!(after, &before);
            } else {
                assert_eq!(after.status, DeploymentStatus::Interrupted, "{name}@{crash_at}");
                assert!(
                    after.steps.iter().all(|s| s.status != StepStatus::Running),
                    "{name}@{crash_at}: no step stays running"
                );
                let began_unconfirmed =
                    matches!(before.cutover, CutoverState::Begun { .. } | CutoverState::Failed { .. })
                        && !before.recovery.map(|r| r.outcome.service_restored()).unwrap_or(false);
                assert_eq!(
                    report.incomplete_cutovers.contains(&after.id),
                    began_unconfirmed,
                    "{name}@{crash_at}: incomplete-cutover flag must match the journal"
                );
            }
            // Startup recovery is idempotent.
            assert!(harpoon_deploy::state::recover_on_start(&mut on_disk, 1)
                .interrupted
                .is_empty());
        }
    }
}

trait ClockNow {
    fn clock_now(&self) -> u64;
}
impl ClockNow for World {
    fn clock_now(&self) -> u64 {
        use harpoon_deploy::time::Clock;
        self.clock.now_ms()
    }
}

// ---------------------------------------------------------------- misuse is rejected, not guessed

#[test]
fn running_a_deployment_that_is_not_queued_is_an_error() {
    let mut w = World::new();
    let ctx = w.previous();
    let id = w.gen.deployment().unwrap();
    let mut d = Deployment::new(id, w.app.clone(), Trigger::Cli, 1); // still `Requested`
    let (mut b, mut j) = (FakeBackend::new(w.clock.clone()), MemoryJournal::new());
    assert!(matches!(
        run(&w, &mut d, &ctx, &mut b, &mut j),
        Err(EngineError::Transition(_))
    ));
    assert!(b.calls.is_empty(), "no side effect on a rejected run");
}

#[test]
fn a_terminal_deployment_cannot_be_run_again() {
    let mut w = World::new();
    let (mut d, ctx) = (w.deployment(), w.previous());
    let (mut b, mut j) = (FakeBackend::new(w.clock.clone()), MemoryJournal::new());
    run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();
    let calls = b.calls.len();
    assert!(run(&w, &mut d, &ctx, &mut b, &mut j).is_err());
    assert_eq!(b.calls.len(), calls);
}

// ---------------------------------------------------------------- write-ahead ordering

/// For every scenario, every backend side effect must be immediately preceded (in the
/// shared trace) by a persisted record that already shows the step it belongs to as
/// `Running` — and, for stopping the previous release, the cutover journal as `Begun`.
/// This is what makes a crash at any point recoverable.
#[test]
fn every_side_effect_is_preceded_by_a_write_that_describes_it() {
    type Scenario = fn(FakeBackend) -> FakeBackend;
    let scenarios: Vec<(&str, Scenario, bool)> = vec![
        ("success", |b| b, true),
        ("final check fails", |b| b.fail_final_check("x"), true),
        ("start fails", |b| b.fail_start_new("x"), true),
        ("stop fails", |b| b.fail_stop_current("x"), true),
        ("first deployment", |b| b, false),
        ("first deployment final fails", |b| b.fail_final_check("x"), false),
    ];
    for (name, script, has_previous) in scenarios {
        let mut w = World::new();
        let mut d = w.deployment();
        let ctx = if has_previous {
            w.previous()
        } else {
            Context { previous_release: None }
        };
        let trace: Trace = Default::default();
        let mut b = script(FakeBackend::new(w.clock.clone())).with_trace(trace.clone());
        let mut j = MemoryJournal::new().with_trace(trace.clone());
        run(&w, &mut d, &ctx, &mut b, &mut j).unwrap();

        let events = trace.borrow();
        let mut last_persist: Option<(Option<Step>, CutoverState)> = None;
        let mut calls = 0;
        for e in events.iter() {
            match e {
                Event::Persist { running, cutover, .. } => last_persist = Some((*running, *cutover)),
                Event::Call(c) => {
                    calls += 1;
                    let (running, cutover) =
                        last_persist.unwrap_or_else(|| panic!("{name}: {c:?} ran before any write"));
                    let expected_step = match c {
                        Call::PrepareSource => Step::Source,
                        Call::Build => Step::Build,
                        Call::PreliminaryCheck => Step::PreliminaryCheck,
                        Call::StopCurrent | Call::StartNew => Step::Cutover,
                        Call::FinalCheck => Step::FinalCheck,
                        Call::StopNew | Call::RestorePrevious { .. } => Step::Recovery,
                    };
                    // `StopNew` after a failed *first* deployment has no recovery step; it only frees the address.
                    if matches!(c, Call::StopNew) && !has_previous {
                        continue;
                    }
                    assert_eq!(
                        running,
                        Some(expected_step),
                        "{name}: {c:?} must follow a write with {expected_step:?} running"
                    );
                    if matches!(c, Call::StopCurrent) {
                        assert!(
                            matches!(cutover, CutoverState::Begun { .. }),
                            "{name}: stop_current before the cutover journal was written"
                        );
                    }
                }
            }
        }
        assert!(calls >= 4, "{name}: expected the trace to contain backend calls");
    }
}
