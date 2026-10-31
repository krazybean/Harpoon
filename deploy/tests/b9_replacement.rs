//! Health-gated replacement: the candidate is built and checked while the current release keeps
//! running, the cutover plan is shown and confirmed before anything is stopped, and the measured
//! interruption is recorded. No Docker, VM, network, file or process access.

use harpoon_deploy::backend::{Call, FakeBackend, FakeRuntime};
use harpoon_deploy::engine::{
    run_deployment_confirmed, Context, CutoverPlan, MemoryJournal, NotCutOver, Report, RunConfig,
};
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator, ReleaseId};
use harpoon_deploy::ledger::ResourceKind;
use harpoon_deploy::state::*;
use harpoon_deploy::time::{Clock, ManualClock, SeqEntropy};
use std::cell::RefCell;
use std::rc::Rc;

struct W {
    clock: Rc<ManualClock>,
    gen: IdGenerator<Rc<ManualClock>, SeqEntropy>,
    app: ApplicationId,
    prev: ReleaseId,
}

fn world() -> W {
    let clock = ManualClock::new(1_000_000);
    let mut gen = IdGenerator::new(clock.clone(), SeqEntropy::new(11));
    let app = gen.application().unwrap();
    let prev_dep: DeploymentId = gen.deployment().unwrap();
    W {
        clock,
        gen,
        app,
        prev: ReleaseId::from_deployment(&prev_dep),
    }
}

fn run(
    w: &mut W,
    b: &mut FakeBackend,
    j: &mut MemoryJournal,
    confirm: &dyn Fn(&CutoverPlan) -> bool,
) -> (Deployment, Report) {
    let id = w.gen.deployment().unwrap();
    let mut d = Deployment::new(id, w.app.clone(), Trigger::Manual, w.clock.now_ms());
    d.transition(DeploymentStatus::Queued, w.clock.now_ms()).unwrap();
    let ctx = Context {
        previous_release: Some(w.prev.clone()),
    };
    let r = run_deployment_confirmed(&mut d, &ctx, b, j, &w.clock, &RunConfig::default(), &|| false, confirm).unwrap();
    (d, r)
}

fn nothing_stopped(b: &FakeBackend, d: &Deployment) {
    for c in [Call::StopCurrent, Call::StartNew, Call::StopNew] {
        assert!(!b.called(&c), "{c:?} must not run");
    }
    assert!(!b.calls.iter().any(|c| matches!(c, Call::RestorePrevious { .. })));
    assert_eq!(d.cutover, CutoverState::NotStarted);
    assert!(d.interruption.is_none());
}

#[test]
fn the_plan_is_shown_after_the_candidate_passed_and_before_anything_is_stopped() {
    let mut w = world();
    let seen: RefCell<Vec<CutoverPlan>> = RefCell::default();
    let mut b = FakeBackend::new(w.clock.clone());
    let mut j = MemoryJournal::new();
    // The plan is only built after the preliminary check, so its existence proves the ordering.
    let (d, r) = run(&mut w, &mut b, &mut j, &|p| {
        seen.borrow_mut().push(p.clone());
        false
    });
    let plans = seen.borrow();
    assert_eq!(plans.len(), 1);
    let p = &plans[0];
    assert_eq!(p.previous, w.prev);
    assert_eq!(p.candidate, ReleaseId::from_deployment(&d.id));
    assert_eq!(p.application, w.app);
    let b_ = RunConfig::default().budgets;
    assert_eq!(p.max_interruption_ms, 2 * b_.cutover_ms + b_.final_ms);
    let text = p.lines().join("\n");
    assert!(text.contains("application is down") && text.contains("unmeasured"));
    assert!(!text.to_lowercase().contains("zero-downtime guarantee"));
    // the candidate was built and checked, nothing else happened
    assert!(b.called(&Call::Build) && b.called(&Call::PreliminaryCheck));
    nothing_stopped(&b, &d);
    assert_eq!(r.status, DeploymentStatus::Cancelled);
    assert_eq!(r.prepared.as_ref().unwrap().why, NotCutOver::NotConfirmed);
    assert_eq!(r.pause, None, "an unconfirmed cutover never pauses triggers");
    assert!(d.release.is_some(), "the prepared candidate has its release identity");
}

#[test]
fn a_declined_cutover_leaves_the_current_release_and_keeps_the_candidate_image() {
    let mut w = world();
    let mut b = FakeBackend::new(w.clock.clone()).with_runtime(FakeRuntime::world());
    let mut j = MemoryJournal::new();
    let (d, r) = run(&mut w, &mut b, &mut j, &|_| false);
    nothing_stopped(&b, &d);
    // the candidate image is retained for the confirmed run or for inspection; nothing else remains
    let images: Vec<_> = r
        .ledger
        .entries()
        .iter()
        .filter(|e| e.key.kind == ResourceKind::Image)
        .collect();
    assert_eq!(images.len(), 1);
    assert!(!images[0].is_released(), "the prepared image stays");
    assert!(r
        .ledger
        .entries()
        .iter()
        .filter(|e| e.key.kind != ResourceKind::Image)
        .all(|e| e.is_released()));
}

#[test]
fn a_confirmed_cutover_records_the_measured_interruption_and_preserves_identity() {
    let mut w = world();
    let mut b = FakeBackend::new(w.clock.clone());
    let mut j = MemoryJournal::new();
    let (d, r) = run(&mut w, &mut b, &mut j, &|_| true);
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    assert!(r.prepared.is_none());
    assert!(matches!(d.cutover, CutoverState::Confirmed { .. }));
    // stop (500) + start (300) + final check (200), from the fake's scripted durations
    let i = d.interruption.expect("interruption recorded");
    assert_eq!(i.duration_ms(), Some(1_000));
    assert_eq!(d.release, Some(ReleaseId::from_deployment(&d.id)));
    assert_eq!(d.application_id, w.app);
    // the durable record carries it: the last persisted snapshot has the closed interruption
    assert_eq!(j.last().unwrap().interruption.unwrap().duration_ms(), Some(1_000));
}

#[test]
fn a_backend_that_cannot_cut_over_prepares_the_candidate_and_stops_before_stopping_anything() {
    let mut w = world();
    let mut b = FakeBackend::new(w.clock.clone()).without_cutover();
    let mut j = MemoryJournal::new();
    // even an eager confirmation cannot make an unsupported cutover begin
    let (d, r) = run(&mut w, &mut b, &mut j, &|_| true);
    nothing_stopped(&b, &d);
    assert_eq!(r.status, DeploymentStatus::Cancelled);
    assert_eq!(r.prepared.unwrap().why, NotCutOver::Unsupported);
    assert_eq!(r.pause, None);
}

#[test]
fn build_and_preliminary_failures_keep_the_current_release_and_never_ask_for_confirmation() {
    for (fail_build, expect_step) in [(true, Step::Build), (false, Step::PreliminaryCheck)] {
        let mut w = world();
        let asked = RefCell::new(0);
        let mut b = FakeBackend::new(w.clock.clone());
        b = if fail_build {
            b.fail_build("RUN false: exit 1")
        } else {
            b.fail_preliminary("candidate answered 500")
        };
        let mut j = MemoryJournal::new();
        let (d, r) = run(&mut w, &mut b, &mut j, &|_| {
            *asked.borrow_mut() += 1;
            true
        });
        assert_eq!(*asked.borrow(), 0);
        nothing_stopped(&b, &d);
        assert_eq!(r.status, DeploymentStatus::Failed);
        assert_eq!(r.pause, None, "nothing was stopped, so triggers are not paused");
        let f = d.failure.as_ref().unwrap();
        assert_eq!(f.step, expect_step);
        // the failure evidence is in the durable record
        let saved = j.last().unwrap();
        assert!(saved.failure.is_some());
        assert!(r.prepared.is_none());
    }
}

#[test]
fn a_passing_preliminary_check_leaves_its_evidence_in_the_durable_step_record() {
    let mut w = world();
    let mut b = FakeBackend::new(w.clock.clone());
    let mut j = MemoryJournal::new();
    let (_, r) = run(&mut w, &mut b, &mut j, &|_| false);
    assert!(r.evidence.iter().any(|(s, _)| *s == Step::PreliminaryCheck));
    let saved = j.last().unwrap();
    let step = saved.steps.iter().find(|s| s.step == Step::PreliminaryCheck).unwrap();
    assert!(step.note.starts_with("health evidence (simulated):"), "{}", step.note);
}

#[test]
fn a_first_deployment_is_never_asked_to_confirm() {
    let mut w = world();
    let asked = RefCell::new(0);
    let mut b = FakeBackend::new(w.clock.clone());
    let mut j = MemoryJournal::new();
    let id = w.gen.deployment().unwrap();
    let mut d = Deployment::new(id, w.app.clone(), Trigger::Manual, w.clock.now_ms());
    d.transition(DeploymentStatus::Queued, w.clock.now_ms()).unwrap();
    let r = run_deployment_confirmed(
        &mut d,
        &Context { previous_release: None },
        &mut b,
        &mut j,
        &w.clock,
        &RunConfig::default(),
        &|| false,
        &|_| {
            *asked.borrow_mut() += 1;
            false
        },
    )
    .unwrap();
    assert_eq!(*asked.borrow(), 0);
    assert_eq!(r.status, DeploymentStatus::Succeeded);
}
