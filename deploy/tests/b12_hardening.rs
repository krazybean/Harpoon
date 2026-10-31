//! Recovery hardening after B11: the pin is on disk before any side effect, the serving release and
//! its real container are derived from the durable records (also after a restoration), and an
//! occupied port stops a restoration before anything is recorded or created. Fake backend,
//! in-memory journal, manual clock: no Docker, VM, network, file or process access.

use harpoon_deploy::backend::{Call, ContainerRef, FakeBackend, FakeRuntime, PreviousRelease, RestoreResult};
use harpoon_deploy::engine::{
    recover_cutover, run_deployment_with, Context, EngineError, JournalError, MemoryJournal, RunConfig,
};
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator, ReleaseId};
use harpoon_deploy::journal_format::{decode_line, encode_snapshot, replay, Line};
use harpoon_deploy::ledger::{Disposition, Ledger, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::release::ReleaseConfig;
use harpoon_deploy::serving::{containers_of_release, restored_container_name, serving_release, ServedBy};
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
    let clock = ManualClock::new(2_000_000);
    let mut gen = IdGenerator::new(clock.clone(), SeqEntropy::new(31));
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

/// The previous release as an earlier deployment left it: succeeded, its container recorded.
fn previous_release(w: &W) -> (Deployment, Ledger) {
    let mut d = Deployment::new(w.prev_dep.clone(), w.app.clone(), Trigger::Cli, 1_000);
    d.transition(DeploymentStatus::Queued, 1_001).unwrap();
    d.transition(DeploymentStatus::Running, 1_002).unwrap();
    d.release = Some(ReleaseId::from_deployment(&w.prev_dep));
    d.pinned = Some(pin());
    d.transition(DeploymentStatus::Succeeded, 1_003).unwrap();
    let name = format!("hp-{}", w.prev_dep);
    let mut l = Ledger::new();
    for r in [
        LedgerRecord::Intent {
            key: ResourceKey::new(ResourceKind::Container, &name),
            app: w.app.clone(),
            deployment: w.prev_dep.clone(),
            disposition: Disposition::RemoveOnFailure,
            at_ms: 1_000,
        },
        LedgerRecord::Receipt {
            key: ResourceKey::new(ResourceKind::Container, &name),
            deployment: w.prev_dep.clone(),
            id: "cid-prev".into(),
            at_ms: 1_001,
        },
    ] {
        l.apply(&r).unwrap();
    }
    (d, l)
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

fn cfg(ledger: Ledger) -> RunConfig {
    RunConfig {
        prior_ledger: ledger,
        previous: Some(pin()),
        ..RunConfig::default()
    }
}

// ---------------------------------------------------------------- pin timing

#[test]
fn a_release_is_pinned_on_disk_before_anything_is_stopped_or_created() {
    for with_previous in [true, false] {
        let mut crashed = 0;
        for n in 0..60 {
            let mut w = world();
            let (_, ledger) = previous_release(&w);
            let mut d = candidate(&mut w);
            let mut b = FakeBackend::new(w.clock.clone()).fail_final_check("503");
            let mut j = MemoryJournal::crash_at(n);
            let c = if with_previous {
                ctx(&w)
            } else {
                Context { previous_release: None }
            };
            let res = run_deployment_with(&mut d, &c, &mut b, &mut j, &w.clock, &cfg(ledger), &|| false);
            // The invariant, at every possible crash point: if the backend was ever told to stop the
            // old release or start the new one, the pin had already been written.
            if b.called(&Call::StopCurrent) || b.called(&Call::StartNew) {
                assert_eq!(
                    j.last().and_then(|s| s.pinned.clone()),
                    Some(ReleaseConfig::simulated(39000)),
                    "previous={with_previous} write {n}: a side effect ran before the pin was durable"
                );
            }
            if res.is_ok() {
                break;
            }
            assert!(matches!(res, Err(EngineError::Journal(JournalError::Crash))));
            crashed += 1;
        }
        assert!(crashed > 5, "the sweep really crashed the run");
    }
}

#[test]
fn a_plan_that_cannot_be_made_fails_the_deployment_before_anything_is_stopped() {
    let mut w = world();
    let (_, ledger) = previous_release(&w);
    let mut d = candidate(&mut w);
    let mut b = FakeBackend::new(w.clock.clone()).fail_plan("no free port in 38000-38999");
    let mut j = MemoryJournal::new();
    let r = run_deployment_with(&mut d, &ctx(&w), &mut b, &mut j, &w.clock, &cfg(ledger), &|| false).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(d.failure.as_ref().unwrap().kind, FailureKind::Cutover);
    assert!(d.failure.as_ref().unwrap().message.contains("no free port"));
    assert_eq!(d.cutover, CutoverState::NotStarted);
    assert!(d.replaces.is_none() && d.interruption.is_none());
    assert!(!b.called(&Call::StopCurrent) && !b.called(&Call::StartNew));
    assert_eq!(r.pause, None, "nothing was stopped, so nothing pauses");
}

#[test]
fn the_release_being_replaced_is_recorded_in_the_write_that_begins_the_cutover() {
    let mut w = world();
    let (_, ledger) = previous_release(&w);
    let mut d = candidate(&mut w);
    let mut b = FakeBackend::new(w.clock.clone());
    let mut j = MemoryJournal::new();
    run_deployment_with(&mut d, &ctx(&w), &mut b, &mut j, &w.clock, &cfg(ledger), &|| false).unwrap();
    let begun = j
        .snapshots
        .iter()
        .find(|s| matches!(s.cutover, CutoverState::Begun { .. }))
        .expect("a snapshot with the cutover begun");
    assert_eq!(begun.replaces, Some(ReleaseId::from_deployment(&w.prev_dep)));
    assert!(
        j.snapshots.iter().filter(|s| s.replaces.is_some()).count() >= 1
            && j.snapshots
                .iter()
                .all(|s| s.replaces.is_none() || !matches!(s.cutover, CutoverState::NotStarted)),
        "never recorded without the cutover beginning"
    );
}

#[test]
fn old_and_new_snapshot_shapes_all_decode() {
    let mut w = world();
    let mut d = candidate(&mut w);
    let base = encode_snapshot(&d).split('\t').count();
    let round = |d: &Deployment| match decode_line(&encode_snapshot(d)).unwrap() {
        Line::Snapshot(b) => *b,
        _ => panic!("not a snapshot"),
    };
    assert_eq!(round(&d), d);
    d.pinned = Some(pin());
    assert_eq!(
        encode_snapshot(&d).split('\t').count(),
        base + 1,
        "a pin only: exactly what B11 wrote"
    );
    assert_eq!(round(&d), d);
    d.replaces = Some(ReleaseId::from_deployment(&w.prev_dep));
    assert_eq!(encode_snapshot(&d).split('\t').count(), base + 2);
    assert_eq!(round(&d), d);
    d.pinned = None;
    assert_eq!(round(&d), d, "replaces without a pin");
}

// ---------------------------------------------------------------- serving release and ownership

#[test]
fn after_a_restoration_the_serving_release_is_the_old_one_in_its_restored_container() {
    let mut w = world();
    let (prev, ledger) = previous_release(&w);
    let rt = FakeRuntime::world();
    let mut d = candidate(&mut w);
    let mut b = FakeBackend::new(w.clock.clone())
        .with_runtime(rt.clone())
        .fail_final_check("HTTP 503")
        .restore_with(RestoreResult::Healthy, 400);
    let mut j = MemoryJournal::new();
    let r = run_deployment_with(
        &mut d,
        &ctx(&w),
        &mut b,
        &mut j,
        &w.clock,
        &cfg(ledger.clone()),
        &|| false,
    )
    .unwrap();
    assert_eq!(d.recovery.unwrap().outcome, RecoveryOutcome::Restored);
    let deps = [prev.clone(), d.clone()];

    // before the failed cutover the release served from its own container
    let before = serving_release(&ledger, std::slice::from_ref(&prev), &w.app).unwrap();
    assert_eq!(before.via, ServedBy::Original);
    // afterwards it is the same release, served by the container the restoration created
    let now = serving_release(&r.ledger, &deps, &w.app).unwrap();
    assert_eq!(now.release, ReleaseId::from_deployment(&w.prev_dep));
    assert_eq!(now.deployment, w.prev_dep);
    assert_eq!(now.container_owner, d.id);
    assert_eq!(now.container_name, restored_container_name(&d.id));
    assert_eq!(now.via, ServedBy::Restored { by: d.id.clone() });
    // dismantling the release sees both of its containers, each under its own deployment
    let owners: Vec<_> = containers_of_release(&r.ledger, &deps, &w.prev_dep)
        .iter()
        .map(|e| (e.deployment.clone(), e.key.name.clone()))
        .collect();
    assert!(
        owners.contains(&(w.prev_dep.clone(), format!("hp-{}", w.prev_dep))),
        "{owners:?}"
    );
    assert!(
        owners.contains(&(d.id.clone(), restored_container_name(&d.id))),
        "{owners:?}"
    );
    // the failed candidate is not part of the release
    assert!(!owners.iter().any(|(_, n)| n == &format!("hp-{}", d.id)));
    // the restored container is kept by cleanup (it serves), the failed candidate is kept too
    let world = rt.borrow();
    assert!(world.has(&restored_container_name(&d.id)));
    assert!(world.has(&format!("hp-{}", d.id)));
    assert!(r.cleanup_problems().is_empty());
}

#[test]
fn a_restoration_that_did_not_start_anything_changes_nothing_about_who_serves() {
    for outcome in [
        RestoreResult::Failed(Reason::StartFailed),
        RestoreResult::NotAttempted(Reason::ImageMissing),
    ] {
        let mut w = world();
        let (prev, ledger) = previous_release(&w);
        let mut d = candidate(&mut w);
        let mut b = FakeBackend::new(w.clock.clone())
            .with_runtime(FakeRuntime::world())
            .fail_final_check("503")
            .restore_with(outcome, 10);
        let r = run_deployment_with(
            &mut d,
            &ctx(&w),
            &mut b,
            &mut MemoryJournal::new(),
            &w.clock,
            &cfg(ledger),
            &|| false,
        )
        .unwrap();
        let sv = serving_release(&r.ledger, &[prev, d], &w.app).unwrap();
        assert_eq!(sv.via, ServedBy::Original, "no new container exists");
    }
}

#[test]
fn a_newer_success_supersedes_a_restored_release_and_a_restoration_supersedes_an_older_success() {
    let mut w = world();
    let (prev, ledger) = previous_release(&w);
    let mut f = candidate(&mut w);
    let mut b = FakeBackend::new(w.clock.clone())
        .with_runtime(FakeRuntime::world())
        .fail_final_check("503")
        .restore_with(RestoreResult::StartedUnverified, 10);
    let r = run_deployment_with(
        &mut f,
        &ctx(&w),
        &mut b,
        &mut MemoryJournal::new(),
        &w.clock,
        &cfg(ledger),
        &|| false,
    )
    .unwrap();
    // an unverified restoration still serves (it started), and says so
    let sv = serving_release(&r.ledger, &[prev.clone(), f.clone()], &w.app).unwrap();
    assert!(matches!(sv.via, ServedBy::Restored { .. }));
    // a deployment that succeeded afterwards is what serves
    let mut later = Deployment::new(w.gen.deployment().unwrap(), w.app.clone(), Trigger::Cli, 9_000_000_000);
    later.transition(DeploymentStatus::Queued, 9_000_000_001).unwrap();
    later.transition(DeploymentStatus::Running, 9_000_000_002).unwrap();
    later.release = Some(ReleaseId::from_deployment(&later.id));
    later.transition(DeploymentStatus::Succeeded, 9_000_000_003).unwrap();
    let mut ledger = r.ledger.clone();
    let name = format!("hp-{}", later.id);
    for rec in [
        LedgerRecord::Intent {
            key: ResourceKey::new(ResourceKind::Container, &name),
            app: w.app.clone(),
            deployment: later.id.clone(),
            disposition: Disposition::RemoveOnFailure,
            at_ms: 9_000_000_000,
        },
        LedgerRecord::Receipt {
            key: ResourceKey::new(ResourceKind::Container, &name),
            deployment: later.id.clone(),
            id: "cid-later".into(),
            at_ms: 9_000_000_001,
        },
    ] {
        ledger.apply(&rec).unwrap();
    }
    let sv = serving_release(&ledger, &[prev, f, later.clone()], &w.app).unwrap();
    assert_eq!(sv.deployment, later.id);
    assert_eq!(sv.via, ServedBy::Original);
}

#[test]
fn the_next_replacement_is_told_which_container_really_serves() {
    let mut w = world();
    let (_, ledger) = previous_release(&w);
    let mut d = candidate(&mut w);
    let mut b = FakeBackend::new(w.clock.clone());
    let mut c = cfg(ledger);
    c.previous_container = Some(ContainerRef {
        owner: w.gen.deployment().unwrap(),
        name: "hp-x-restored".into(),
    });
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
    // (the fake ignores it; the Docker backend's use of it is covered in deploy-docker)
    assert!(b.called(&Call::StopCurrent));
}

// ---------------------------------------------------------------- occupied ports

fn interrupted(w: &mut W) -> Deployment {
    let mut d = candidate(w);
    let now = w.clock.now_ms();
    d.transition(DeploymentStatus::Running, now).unwrap();
    d.start_step(Step::Cutover, now).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&w.prev_dep));
    d.begin_cutover(now).unwrap();
    let mut v = [d];
    recover_on_start(&mut v, now + 10);
    let [d] = v;
    d
}

fn previous(w: &W, config: Option<ReleaseConfig>) -> PreviousRelease {
    PreviousRelease {
        release: ReleaseId::from_deployment(&w.prev_dep),
        deployment: w.prev_dep.clone(),
        config,
        container: None,
    }
}

#[test]
fn an_occupied_port_stops_the_automatic_restoration_before_anything_is_created() {
    let mut w = world();
    let (_, ledger) = previous_release(&w);
    let mut d = candidate(&mut w);
    let mut b = FakeBackend::new(w.clock.clone())
        .fail_final_check("503")
        .restore_blocked_by(&["port 38123 is published by the running container hp-x (abc)"]);
    let r = run_deployment_with(
        &mut d,
        &ctx(&w),
        &mut b,
        &mut MemoryJournal::new(),
        &w.clock,
        &cfg(ledger),
        &|| false,
    )
    .unwrap();
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::PortInUse)
    );
    assert!(
        !b.calls.iter().any(|c| matches!(c, Call::RestorePrevious { .. })),
        "nothing was created"
    );
    assert!(r
        .warnings
        .iter()
        .any(|m| m.contains("restoration blocked") && m.contains("hp-x")));
    assert!(d.application_may_be_down());
    assert_eq!(
        d.recovery.unwrap().outcome.as_str(),
        "not_attempted",
        "reported as not attempted, with its reason"
    );
}

#[test]
fn an_occupied_port_blocks_the_explicit_restoration_without_spending_the_one_attempt() {
    let mut w = world();
    let mut d = interrupted(&mut w);
    let (_, ledger) = previous_release(&w);
    let c = cfg(ledger);

    let mut blocked = FakeBackend::new(w.clock.clone()).restore_blocked_by(&["port 38123 is published by the running container hp-failed (id1): it carries this application's Harpoon labels"]);
    let mut j = MemoryJournal::new();
    let r = recover_cutover(&mut d, previous(&w, None), &mut blocked, &mut j, &w.clock, &c).unwrap();
    assert_eq!(r.blocked.len(), 1);
    assert!(r.blocked[0].contains("hp-failed"));
    assert_eq!(j.writes(), 0, "nothing was recorded");
    assert!(
        d.recovery.is_none() && d.can_recover_explicitly(),
        "the attempt is not spent"
    );
    assert!(blocked.calls.is_empty(), "nothing was created");

    // the developer clears the port; the attempt is then made, once
    let mut clear = FakeBackend::new(w.clock.clone()).restore_with(RestoreResult::Healthy, 50);
    let r = recover_cutover(&mut d, previous(&w, None), &mut clear, &mut j, &w.clock, &c).unwrap();
    assert!(r.blocked.is_empty());
    assert_eq!(d.recovery.unwrap().outcome, RecoveryOutcome::Restored);
    assert!(recover_cutover(&mut d, previous(&w, None), &mut clear, &mut j, &w.clock, &c).is_err());
}

#[test]
fn explicit_recovery_needs_the_deployment_to_say_what_it_replaced() {
    let mut w = world();
    let mut d = interrupted(&mut w);
    let (_, ledger) = previous_release(&w);
    // the caller names a different release than the one recorded
    let mut other = previous(&w, None);
    other.release = ReleaseId::from_deployment(&w.gen.deployment().unwrap());
    let mut b = FakeBackend::new(w.clock.clone());
    let mut j = MemoryJournal::new();
    assert!(recover_cutover(&mut d, other, &mut b, &mut j, &w.clock, &cfg(ledger.clone())).is_err());
    // a deployment from before `replaces` existed
    let mut old = d.clone();
    old.replaces = None;
    assert!(recover_cutover(&mut old, previous(&w, None), &mut b, &mut j, &w.clock, &cfg(ledger)).is_err());
    assert_eq!(j.writes(), 0);
    assert!(b.calls.is_empty());
}

#[test]
fn the_store_still_accepts_only_the_one_recovery_change_now_that_replaces_is_recorded() {
    let mut w = world();
    let d = interrupted(&mut w);
    let mut done = d.clone();
    done.begin_explicit_recovery(60_000, 5).unwrap();
    done.finish_recovery(RecoveryOutcome::Restored, 6).unwrap();
    let lines = |ds: &[&Deployment]| {
        ds.iter()
            .map(|d| Line::Snapshot(Box::new((*d).clone())))
            .collect::<Vec<_>>()
    };
    assert!(replay(&lines(&[&d, &done])).is_ok());
    let mut other = done.clone();
    other.replaces = Some(ReleaseId::from_deployment(&w.gen.deployment().unwrap()));
    assert!(
        replay(&lines(&[&d, &other])).is_err(),
        "what was replaced never changes"
    );
}
