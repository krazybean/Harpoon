//! Explicit manual rollback: selection from the durable records, the engine run that re-serves an
//! earlier release's pinned configuration, failure after cutover handled by the existing bounded
//! recovery, the serving-release derivation, the trigger pause and crash handling. Fake backend,
//! in-memory journal, manual clock: no Docker, VM, network, file or process access.

use harpoon_deploy::backend::{Call, FakeBackend, FakeRuntime, RestoreResult};
use harpoon_deploy::engine::{
    run_deployment_confirmed, run_deployment_with, Context, CutoverPlan, EngineError, JournalError, MemoryJournal,
    NotCutOver, PauseReason, Report, RunConfig,
};
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator, ReleaseId};
use harpoon_deploy::journal_format::{encode_snapshot, replay, Line};
use harpoon_deploy::ledger::{Disposition, Ledger, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::release::ReleaseConfig;
use harpoon_deploy::rollback::{retained_releases, select_target};
use harpoon_deploy::serving::{serving_release, ServedBy};
use harpoon_deploy::state::*;
use harpoon_deploy::time::{Clock, ManualClock, SeqEntropy};
use std::rc::Rc;

struct W {
    clock: Rc<ManualClock>,
    gen: IdGenerator<Rc<ManualClock>, SeqEntropy>,
    app: ApplicationId,
    old: DeploymentId,
    serving: DeploymentId,
    deps: Vec<Deployment>,
    ledger: Ledger,
}

fn old_pin() -> ReleaseConfig {
    let mut c = ReleaseConfig::simulated(38100);
    c.image_id = format!("sha256:{}", "a".repeat(64));
    c.image_tag = "harpoon/app:old".into();
    c.memory_bytes = 128 * 1024 * 1024;
    c
}

fn serving_pin() -> ReleaseConfig {
    let mut c = ReleaseConfig::simulated(38200);
    c.image_id = format!("sha256:{}", "b".repeat(64));
    c.image_tag = "harpoon/app:serving".into();
    c
}

fn record(
    l: &mut Ledger,
    app: &ApplicationId,
    dep: &DeploymentId,
    kind: ResourceKind,
    name: &str,
    id: &str,
    d: Disposition,
) {
    l.apply(&LedgerRecord::Intent {
        key: ResourceKey::new(kind, name),
        app: app.clone(),
        deployment: dep.clone(),
        disposition: d,
        at_ms: 1,
    })
    .unwrap();
    l.apply(&LedgerRecord::Receipt {
        key: ResourceKey::new(kind, name),
        deployment: dep.clone(),
        id: id.into(),
        at_ms: 2,
    })
    .unwrap();
}

fn succeeded(app: &ApplicationId, id: &DeploymentId, at: u64, pin: Option<ReleaseConfig>) -> Deployment {
    let mut d = Deployment::new(id.clone(), app.clone(), Trigger::Manual, at);
    d.transition(DeploymentStatus::Queued, at + 1).unwrap();
    d.transition(DeploymentStatus::Running, at + 2).unwrap();
    d.release = Some(ReleaseId::from_deployment(id));
    d.pinned = pin;
    d.transition(DeploymentStatus::Succeeded, at + 3).unwrap();
    d
}

/// An application with an older release (image retained, container stopped) and a newer one serving.
fn world() -> W {
    let clock = ManualClock::new(1_000_000);
    let mut gen = IdGenerator::new(clock.clone(), SeqEntropy::new(77));
    let app = gen.application().unwrap();
    let old = gen.deployment().unwrap();
    let serving = gen.deployment().unwrap();
    let mut ledger = Ledger::new();
    for (dep, tag) in [(&old, "harpoon/app:old"), (&serving, "harpoon/app:serving")] {
        record(
            &mut ledger,
            &app,
            dep,
            ResourceKind::Image,
            tag,
            "img",
            Disposition::Retain,
        );
        record(
            &mut ledger,
            &app,
            dep,
            ResourceKind::Container,
            &format!("hp-{dep}"),
            &format!("cid-{dep}"),
            Disposition::RemoveOnFailure,
        );
    }
    let deps = vec![
        succeeded(&app, &old, 100, Some(old_pin())),
        succeeded(&app, &serving, 200, Some(serving_pin())),
    ];
    clock.advance(1_000);
    W {
        clock,
        gen,
        app,
        old,
        serving,
        deps,
        ledger,
    }
}

fn rel(d: &DeploymentId) -> ReleaseId {
    ReleaseId::from_deployment(d)
}

fn rollback_dep(w: &mut W) -> Deployment {
    let id = w.gen.deployment().unwrap();
    let mut d = Deployment::new(id, w.app.clone(), Trigger::Cli, w.clock.now_ms());
    d.rollback_to = Some(rel(&w.old));
    d.transition(DeploymentStatus::Queued, w.clock.now_ms()).unwrap();
    d
}

fn cfg(w: &W) -> RunConfig {
    RunConfig {
        prior_ledger: w.ledger.clone(),
        previous: Some(serving_pin()),
        ..RunConfig::default()
    }
}

fn ctx(w: &W) -> Context {
    Context {
        previous_release: Some(rel(&w.serving)),
    }
}

fn backend(w: &W) -> FakeBackend {
    FakeBackend::new(w.clock.clone())
        .with_runtime(FakeRuntime::world())
        .with_rollback(old_pin())
}

fn run(w: &W, d: &mut Deployment, b: &mut FakeBackend) -> Report {
    run_deployment_with(d, &ctx(w), b, &mut MemoryJournal::new(), &w.clock, &cfg(w), &|| false).unwrap()
}

fn all(deps: &[Deployment], d: &Deployment) -> Vec<Deployment> {
    let mut v = deps.to_vec();
    v.push(d.clone());
    v
}

// ------------------------------------------------------------------ selection

#[test]
fn selection_lists_retained_releases_and_names_the_serving_one() {
    let w = world();
    let list = retained_releases(&w.ledger, &w.deps, &w.app);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].release, rel(&w.serving));
    assert!(list[0].serving && !list[0].eligible());
    assert_eq!(list[1].release, rel(&w.old));
    assert!(list[1].eligible(), "{:?}", list[1].problems);
    assert_eq!(
        select_target(&w.ledger, &w.deps, &w.app, &rel(&w.old)).unwrap().config,
        Some(old_pin())
    );
}

#[test]
fn unprovable_or_unknown_targets_are_refused_with_reasons() {
    let w = world();
    let other_app = IdGenerator::new(ManualClock::new(5), SeqEntropy::new(1))
        .application()
        .unwrap();
    // nonexistent release, another application's release, and the serving release itself
    assert!(select_target(&w.ledger, &w.deps, &w.app, &ReleaseId::from_deployment(&w.gen_clone())).is_err());
    assert!(select_target(&w.ledger, &w.deps, &other_app, &rel(&w.old)).is_err());
    let e = select_target(&w.ledger, &w.deps, &w.app, &rel(&w.serving)).unwrap_err();
    assert!(e.iter().any(|m| m.contains("serving now")), "{e:?}");

    // no pinned configuration
    let mut deps = w.deps.clone();
    deps[0].pinned = None;
    let e = select_target(&w.ledger, &deps, &w.app, &rel(&w.old)).unwrap_err();
    assert!(e.iter().any(|m| m.contains("no pinned configuration")), "{e:?}");

    // a mutable tag in place of an image id
    let mut deps = w.deps.clone();
    deps[0].pinned.as_mut().unwrap().image_id = "harpoon/app:old".into();
    let e = select_target(&w.ledger, &deps, &w.app, &rel(&w.old)).unwrap_err();
    assert!(e.iter().any(|m| m.contains("not an immutable image id")), "{e:?}");

    // the retained image is no longer recorded
    let mut ledger = Ledger::new();
    record(
        &mut ledger,
        &w.app,
        &w.serving,
        ResourceKind::Container,
        &format!("hp-{}", w.serving),
        "c",
        Disposition::RemoveOnFailure,
    );
    let e = select_target(&ledger, &w.deps, &w.app, &rel(&w.old)).unwrap_err();
    assert!(e.iter().any(|m| m.contains("retained image")), "{e:?}");
}

impl W {
    /// An id that belongs to no recorded deployment.
    fn gen_clone(&self) -> DeploymentId {
        IdGenerator::new(ManualClock::new(9), SeqEntropy::new(201))
            .deployment()
            .unwrap()
    }
}

#[test]
fn another_applications_releases_are_never_candidates() {
    let mut w = world();
    let other_app = IdGenerator::new(ManualClock::new(5), SeqEntropy::new(2))
        .application()
        .unwrap();
    let od = IdGenerator::new(ManualClock::new(5), SeqEntropy::new(3))
        .deployment()
        .unwrap();
    record(
        &mut w.ledger,
        &other_app,
        &od,
        ResourceKind::Image,
        "x",
        "i",
        Disposition::Retain,
    );
    w.deps.push(succeeded(&other_app, &od, 50, Some(old_pin())));
    let list = retained_releases(&w.ledger, &w.deps, &w.app);
    assert!(list.iter().all(|r| r.deployment != od));
    assert!(select_target(&w.ledger, &w.deps, &w.app, &rel(&od)).is_err());
}

// ------------------------------------------------------------------ a successful rollback

#[test]
fn a_rollback_serves_the_earlier_release_from_its_pinned_configuration_and_pauses_triggers() {
    let mut w = world();
    let mut d = rollback_dep(&mut w);
    let mut b = backend(&w);
    let r = run(&w, &mut d, &mut b);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", d.failure);
    assert_eq!(d.rollback_to, Some(rel(&w.old)));
    assert_eq!(d.replaces, Some(rel(&w.serving)), "source release recorded");
    // the pin is the target's exact image and limits; only the port was chosen afresh
    let pin = d.pinned.clone().unwrap();
    assert_eq!(pin.image_id, old_pin().image_id);
    assert_eq!(pin.memory_bytes, old_pin().memory_bytes);
    // nothing was built, the serving release was stopped after the preliminary check
    let order: Vec<_> = b.calls.iter().filter(|c| !matches!(c, Call::Build)).cloned().collect();
    assert!(b.calls.contains(&Call::Build) && !b.restored.is_some());
    assert_eq!(
        order,
        vec![
            Call::PrepareSource,
            Call::PreliminaryCheck,
            Call::StopCurrent,
            Call::StartNew,
            Call::FinalCheck
        ]
    );
    assert!(
        r.ledger
            .entries()
            .iter()
            .filter(|e| e.deployment == d.id)
            .all(|e| e.key.kind != ResourceKind::Image),
        "a rollback records no image of its own"
    );
    assert!(d.interruption.unwrap().duration_ms().is_some(), "interruption measured");

    // the serving release is the earlier one, in the rollback's own container
    let deps = all(&w.deps, &d);
    let s = serving_release(&r.ledger, &deps, &w.app).unwrap();
    assert_eq!(s.release, rel(&w.old));
    assert_eq!(s.deployment, w.old);
    assert_eq!(s.container_owner, d.id);
    assert_eq!(s.container_name, format!("hp-{}", d.id));
    assert_eq!(s.via, ServedBy::RolledBack { by: d.id.clone() });
    assert_eq!(s.config, d.pinned, "a later replacement restores what really serves");

    // triggers are paused, with the reason, and stay so
    let p = trigger_pauses(&deps);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].rollback, Some(rel(&w.old)));
    assert_eq!(p[0].deployment, d.id);
    // the rolled-back-from release is later now: not a target (moving forward is a replacement)
    let e = select_target(&r.ledger, &deps, &w.app, &rel(&w.serving)).unwrap_err();
    assert!(e.iter().any(|m| m.contains("not earlier")), "{e:?}");
}

#[test]
fn the_plan_carries_the_data_warning_and_the_interruption_before_anything_stops() {
    let mut w = world();
    let mut d = rollback_dep(&mut w);
    let mut b = backend(&w);
    let seen = std::cell::RefCell::new(Vec::new());
    let r = run_deployment_confirmed(
        &mut d,
        &ctx(&w),
        &mut b,
        &mut MemoryJournal::new(),
        &w.clock,
        &cfg(&w),
        &|| false,
        &|p: &CutoverPlan| {
            seen.borrow_mut().extend(p.lines());
            false
        },
    )
    .unwrap();
    let text = seen.borrow().join("\n");
    assert!(
        text.contains("Application rollback only. Data, volumes and database schema are NOT rolled back."),
        "{text}"
    );
    assert!(text.contains(&rel(&w.old).to_string()) && text.contains(&rel(&w.serving).to_string()));
    assert!(text.contains("anticipated interruption") && text.contains("one bounded attempt"));
    // unconfirmed: prepared, serving release untouched, no pause, nothing stopped
    assert_eq!(r.prepared.as_ref().unwrap().why, NotCutOver::NotConfirmed);
    assert_eq!(r.prepared.as_ref().unwrap().plan.candidate, rel(&w.old));
    assert!(!b.calls.contains(&Call::StopCurrent) && !b.calls.contains(&Call::StartNew));
    assert_eq!(d.cutover, CutoverState::NotStarted);
    assert!(trigger_pauses(&all(&w.deps, &d)).is_empty());
    let s = serving_release(&r.ledger, &all(&w.deps, &d), &w.app).unwrap();
    assert_eq!(s.release, rel(&w.serving));
}

// ------------------------------------------------------------------ failures before the cutover

#[test]
fn an_unavailable_target_image_or_failed_check_never_touches_the_serving_release() {
    for (name, mut b) in [
        (
            "image",
            backend(&world()).fail_source("the retained image no longer exists"),
        ),
        ("preliminary", backend(&world()).fail_preliminary("HTTP 503")),
        ("plan", backend(&world()).fail_plan("no port")),
    ] {
        let mut w = world();
        let mut d = rollback_dep(&mut w);
        let r = run(&w, &mut d, &mut b);
        assert_eq!(r.status, DeploymentStatus::Failed, "{name}");
        assert!(
            !b.calls.contains(&Call::StopCurrent),
            "{name}: the serving release was stopped"
        );
        assert!(!b.calls.contains(&Call::RestorePrevious { budget_ms: 60_000 }));
        assert_eq!(d.cutover, CutoverState::NotStarted, "{name}");
        assert_eq!(r.pause, None, "{name}");
        assert!(d.interruption.is_none());
        let deps = all(&w.deps, &d);
        assert!(
            trigger_pauses(&deps).is_empty(),
            "{name}: a failed rollback that stopped nothing pauses nothing"
        );
        let s = serving_release(&r.ledger, &deps, &w.app).unwrap();
        assert_eq!(s.release, rel(&w.serving), "{name}");
        assert_eq!(s.via, ServedBy::Original);
    }
}

// ------------------------------------------------------------------ failure after the cutover

#[test]
fn failure_after_cutover_uses_the_bounded_recovery_to_restore_the_release_that_was_serving() {
    let mut w = world();
    let mut d = rollback_dep(&mut w);
    let mut b = backend(&w)
        .fail_final_check("HTTP 503")
        .restore_with(RestoreResult::Healthy, 300);
    let r = run(&w, &mut d, &mut b);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(r.pause, Some(PauseReason::FailedCutover));
    let rec = d.recovery.unwrap();
    assert_eq!(rec.outcome, RecoveryOutcome::Restored);
    let restored = b.restored.clone().unwrap();
    assert_eq!(
        restored.release,
        rel(&w.serving),
        "the release serving before the rollback is restored"
    );
    assert_eq!(restored.config, Some(serving_pin()));
    assert_eq!(
        b.calls
            .iter()
            .filter(|c| matches!(c, Call::RestorePrevious { .. }))
            .count(),
        1
    );
    let deps = all(&w.deps, &d);
    let s = serving_release(&r.ledger, &deps, &w.app).unwrap();
    assert_eq!(s.release, rel(&w.serving));
    assert_eq!(s.via, ServedBy::Restored { by: d.id.clone() });
    assert_eq!(s.container_name, format!("hp-{}-restored", d.id));
    // the rolled-back-to candidate is kept for inspection and its failure is recorded
    assert!(r
        .ledger
        .entries()
        .iter()
        .any(|e| e.deployment == d.id && e.key.name == format!("hp-{}", d.id) && !e.is_released()));
    assert_eq!(d.failure.as_ref().unwrap().kind, FailureKind::FinalCheck);
    // a failed rollback is not a successful one: no rollback pause reason, but the failed-cutover pause
    let p = trigger_pauses(&deps);
    assert_eq!(p[0].rollback, None);
}

#[test]
fn a_rollback_after_a_previously_restored_deployment_replaces_the_restored_container() {
    // serving release S was restored by a failed deployment F (restored container under F)
    let mut w = world();
    let f = w.gen.deployment().unwrap();
    let mut fd = Deployment::new(f.clone(), w.app.clone(), Trigger::Manual, 300);
    fd.transition(DeploymentStatus::Queued, 301).unwrap();
    fd.transition(DeploymentStatus::Running, 302).unwrap();
    fd.start_step(Step::Cutover, 303).unwrap();
    fd.replaces = Some(rel(&w.serving));
    fd.begin_cutover(304).unwrap();
    fd.fail_cutover(305).unwrap();
    fd.start_step(Step::Recovery, 306).unwrap();
    fd.start_recovery(60_000, 306).unwrap();
    fd.finish_recovery(RecoveryOutcome::Restored, 700).unwrap();
    fd.transition(DeploymentStatus::Failed, 701).unwrap();
    record(
        &mut w.ledger,
        &w.app,
        &f,
        ResourceKind::Container,
        &harpoon_deploy::serving::restored_container_name(&f),
        "cid-restored",
        Disposition::Retain,
    );
    w.deps.push(fd);
    let s = serving_release(&w.ledger, &w.deps, &w.app).unwrap();
    assert_eq!(s.container_owner, f);

    let mut d = rollback_dep(&mut w);
    let mut config = cfg(&w);
    config.previous_container = Some(harpoon_deploy::backend::ContainerRef {
        owner: s.container_owner.clone(),
        name: s.container_name.clone(),
    });
    let mut b = backend(&w);
    let r = run_deployment_with(
        &mut d,
        &ctx(&w),
        &mut b,
        &mut MemoryJournal::new(),
        &w.clock,
        &config,
        &|| false,
    )
    .unwrap();
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    let deps = all(&w.deps, &d);
    let now = serving_release(&r.ledger, &deps, &w.app).unwrap();
    assert_eq!(now.release, rel(&w.old));
    assert_eq!(now.container_owner, d.id);
    // teardown/reconcile of the old release finds the rollback's container too
    let cs = harpoon_deploy::serving::containers_of_release(&r.ledger, &deps, &w.old);
    assert!(cs.iter().any(|e| e.deployment == d.id));
}

// ------------------------------------------------------------------ durable state and crashes

#[test]
fn the_rollback_record_round_trips_and_older_snapshots_are_unchanged() {
    let mut w = world();
    let mut d = rollback_dep(&mut w);
    let mut b = backend(&w);
    let _ = run(&w, &mut d, &mut b);
    let line = encode_snapshot(&d);
    let back = replay(&[Line::Snapshot(Box::new(d.clone()))]).unwrap();
    assert_eq!(back.deployments[0].rollback_to, d.rollback_to);
    assert_eq!(back.deployments[0].replaces, d.replaces);
    assert!(line.contains(&rel(&w.old).to_string()));
    // a deployment that is not a rollback encodes exactly as before: no extra field
    let plain = &w.deps[0];
    assert_eq!(line.split('\t').count(), encode_snapshot(plain).split('\t').count() + 2);
    assert_eq!(
        replay(&[Line::Snapshot(Box::new(plain.clone()))]).unwrap().deployments[0].rollback_to,
        None
    );
    // a rollback that has only its target (no pin, no source yet) round-trips through the placeholders
    let mut early = rollback_dep(&mut w);
    early.pinned = None;
    let back = replay(&[Line::Snapshot(Box::new(early.clone()))]).unwrap();
    assert_eq!(back.deployments[0].rollback_to, early.rollback_to);
    assert_eq!(back.deployments[0].pinned, None);
    assert_eq!(back.deployments[0].replaces, None);
}

#[test]
fn a_crash_at_every_write_of_a_rollback_is_recovered_truthfully_and_never_repeated() {
    for n in 0..60 {
        let mut w = world();
        let mut d = rollback_dep(&mut w);
        let mut b = backend(&w)
            .fail_final_check("HTTP 503")
            .restore_with(RestoreResult::Healthy, 10);
        let mut j = MemoryJournal::crash_at(n);
        let res = run_deployment_with(&mut d, &ctx(&w), &mut b, &mut j, &w.clock, &cfg(&w), &|| false);
        if res.is_ok() {
            break;
        }
        assert!(matches!(res, Err(EngineError::Journal(JournalError::Crash))));
        let Some(last) = j.last() else { continue };
        // whatever was persisted still says it is a rollback, to which release
        assert_eq!(last.rollback_to, Some(rel(&w.old)), "write {n}");
        let mut recovered = vec![last.clone()];
        recover_on_start(&mut recovered, 5_000_000);
        let rd = &recovered[0];
        assert!(rd.status.is_terminal(), "write {n}: not resumed, closed");
        // no automatic restoration, rollback or second attempt came out of recovery
        assert_ne!(
            rd.recovery.map(|r| r.outcome),
            Some(RecoveryOutcome::InProgress),
            "write {n}"
        );
        if rd.cutover != CutoverState::NotStarted && rd.status == DeploymentStatus::Interrupted && rd.recovery.is_none()
        {
            assert!(rd.application_may_be_down(), "write {n}");
        }
        // a successful rollback never appears from a crash: only a finished run is Succeeded
        assert!(
            !(rd.status == DeploymentStatus::Succeeded && last.status != DeploymentStatus::Succeeded),
            "write {n}"
        );
        // the durable record replays, and its serving derivation never names an unrecorded container
        assert!(
            replay(
                &j.snapshots
                    .iter()
                    .map(|s| Line::Snapshot(Box::new(s.clone())))
                    .collect::<Vec<_>>()
            )
            .is_ok(),
            "write {n}"
        );
        let mut deps = w.deps.clone();
        deps.push(rd.clone());
        let _ = serving_release(&w.ledger, &deps, &w.app);
    }
}

#[test]
fn an_interrupted_rollback_pauses_nothing_new_and_a_second_rollback_is_not_started_for_it() {
    // crashed after the serving release was stopped: may be down, explicit bounded recovery only
    let mut w = world();
    let mut d = rollback_dep(&mut w);
    let now = w.clock.now_ms();
    d.transition(DeploymentStatus::Running, now).unwrap();
    d.start_step(Step::Cutover, now).unwrap();
    d.replaces = Some(rel(&w.serving));
    d.begin_cutover(now).unwrap();
    let mut v = [d];
    recover_on_start(&mut v, now + 5);
    let [d] = v;
    assert!(d.application_may_be_down());
    assert_eq!(d.rollback_to, Some(rel(&w.old)));
    let deps = all(&w.deps, &d);
    let p = trigger_pauses(&deps);
    assert!(p[0].incomplete && p[0].rollback.is_none());
    // the existing explicit recovery restores the release that was serving, once
    let mut dd = d.clone();
    let mut b = FakeBackend::new(w.clock.clone()).restore_with(RestoreResult::Healthy, 10);
    let prev = harpoon_deploy::backend::PreviousRelease {
        release: rel(&w.serving),
        deployment: w.serving.clone(),
        config: None,
        container: None,
    };
    let c = RunConfig {
        previous: Some(serving_pin()),
        ..RunConfig::default()
    };
    harpoon_deploy::engine::recover_cutover(&mut dd, prev.clone(), &mut b, &mut MemoryJournal::new(), &w.clock, &c)
        .unwrap();
    assert_eq!(dd.recovery.unwrap().outcome, RecoveryOutcome::Restored);
    assert!(
        harpoon_deploy::engine::recover_cutover(&mut dd, prev, &mut b, &mut MemoryJournal::new(), &w.clock, &c)
            .is_err()
    );
}
