//! B1 final corrections: (1) health evidence must be genuine; (2) one cleanup result per
//! cleanup attempt. Offline: no Docker, VM, network, file or process access.

mod common;

use common::*;
use harpoon_deploy::backend::FakeBackend;
use harpoon_deploy::engine::{end_state, reconcile, EngineError, JournalError, MemoryJournal};
use harpoon_deploy::health::*;
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator};
use harpoon_deploy::ledger::codec;
use harpoon_deploy::ledger::*;
use harpoon_deploy::state::{recover_on_start, DeploymentStatus};
use harpoon_deploy::time::{Clock, ManualClock, SeqEntropy};

// ================================================================ health evidence

fn attempt(obs: Observation, running: bool) -> Attempt {
    Attempt {
        at_ms: 0,
        observation: obs,
        container_running: running,
    }
}

fn claimed_healthy(attempts: Vec<Attempt>, required: u8, reached: u8) -> CheckEvidence {
    CheckEvidence {
        target: "http://127.0.0.1:39001/".into(),
        expected: "HTTP 2xx".into(),
        expect: StatusClass::Success2xx,
        simulated: false,
        attempts,
        consecutive_required: required,
        consecutive_reached: reached,
        elapsed_ms: 500,
        verdict: Verdict::Healthy,
        scope_note: SCOPE_NOTE,
    }
}

fn ok(n: usize) -> Vec<Attempt> {
    (0..n).map(|_| attempt(Observation::Status(200), true)).collect()
}

#[test]
fn an_unprobed_check_cannot_establish_health() {
    let e = CheckEvidence::not_probed("nothing ran");
    assert!(!e.is_healthy());
    assert!(e.validate().is_err());
    assert_ne!(e.verdict, Verdict::Healthy);
}

#[test]
fn a_healthy_verdict_with_no_observations_is_rejected() {
    let e = claimed_healthy(vec![], 2, 2);
    assert!(!e.is_healthy());
    assert!(e.validate().unwrap_err().contains("no health observation"));
}

#[test]
fn a_zero_threshold_is_rejected_even_with_observations() {
    let e = claimed_healthy(ok(3), 0, 3);
    assert!(e.validate().unwrap_err().contains("positive"));
}

#[test]
fn passing_evidence_that_contradicts_its_observations_is_rejected() {
    // verdict says healthy, but the last response was a 503
    let mut a = ok(2);
    a.push(attempt(Observation::Status(503), true));
    assert!(!claimed_healthy(a, 2, 2).is_healthy());
    // a refused connection in the trailing run
    let mut a = ok(1);
    a.push(attempt(Observation::ConnectFailed("refused".into()), true));
    assert!(!claimed_healthy(a, 1, 1).is_healthy());
    // the container had exited
    let a = vec![
        attempt(Observation::Status(200), true),
        attempt(Observation::Status(200), false),
    ];
    assert!(!claimed_healthy(a, 2, 2).is_healthy());
    assert!(!claimed_healthy(vec![attempt(Observation::ContainerExited(Some(1)), false)], 1, 1).is_healthy());
    // fewer trailing successes than required
    assert!(!claimed_healthy(ok(1), 2, 2).is_healthy());
    // overstated streak
    assert!(!claimed_healthy(ok(2), 2, 5).is_healthy());
    // understated streak is also a contradiction
    assert!(!claimed_healthy(ok(3), 2, 2).is_healthy());
    // a verdict other than Healthy never passes, whatever the observations say
    for v in [Verdict::NotHealthy, Verdict::Exited] {
        let mut e = claimed_healthy(ok(2), 2, 2);
        e.verdict = v;
        assert!(!e.is_healthy());
    }
    // the honest version of the same shape passes
    assert!(claimed_healthy(ok(2), 2, 2).is_healthy());
}

#[test]
fn a_status_outside_the_expected_class_does_not_count() {
    let mut e = claimed_healthy(vec![attempt(Observation::Status(404), true); 2], 2, 2);
    e.expect = StatusClass::Success2xx;
    assert!(!e.is_healthy());
    e.expect = StatusClass::NotServerError;
    assert!(e.is_healthy());
}

#[test]
fn simulated_evidence_passes_but_says_it_is_simulated() {
    let spec = HealthSpec::first_test("/");
    let e = CheckEvidence::simulated_passing("fake", &spec);
    assert!(e.is_healthy() && e.simulated);
    assert_eq!(e.attempts.len(), usize::from(spec.consecutive_ok));
    assert!(e.attempts.iter().all(|a| a.container_running));
    assert!(e.expected.contains("simulated"));
    assert_eq!(e.scope_note, SCOPE_NOTE);
    // even a zero-threshold spec cannot produce zero observations
    let zero = HealthSpec {
        consecutive_ok: 0,
        ..HealthSpec::first_test("/")
    };
    let z = CheckEvidence::simulated_passing("fake", &zero);
    assert!(z.is_healthy() && !z.attempts.is_empty());
}

struct AlwaysOk;
impl Probe for AlwaysOk {
    fn attempt(&mut self, at_ms: u64) -> Attempt {
        Attempt {
            at_ms,
            observation: Observation::Status(200),
            container_running: true,
        }
    }
}

#[test]
fn the_probe_loop_never_declares_health_for_a_zero_threshold() {
    let clock = ManualClock::new(0);
    let spec = HealthSpec {
        consecutive_ok: 0,
        startup_deadline_ms: 2_000,
        ..HealthSpec::first_test("/")
    };
    let e = run_health(&spec, "t", &mut AlwaysOk, &clock, &clock);
    assert_ne!(e.verdict, Verdict::Healthy);
    assert!(!e.is_healthy());
}

#[test]
fn the_engine_does_not_accept_fabricated_or_empty_evidence() {
    let fabricated: Vec<(&str, CheckEvidence)> = vec![
        ("unprobed", CheckEvidence::not_probed("x")),
        ("empty", claimed_healthy(vec![], 2, 2)),
        ("zero threshold", claimed_healthy(ok(2), 0, 2)),
        ("contradicted", {
            let mut a = ok(2);
            a.push(attempt(Observation::Status(500), true));
            claimed_healthy(a, 2, 2)
        }),
    ];
    for (name, ev) in fabricated {
        let mut fx = Fx::new();
        let mut d = fx.deployment();
        let mut b = fx.backend().final_check_reports(ev);
        let mut j = MemoryJournal::new();
        let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
        assert_eq!(d.status, DeploymentStatus::Failed, "{name}: must not succeed");
        let f = d.failure.as_ref().unwrap();
        assert!(f.message.contains("did not establish health"), "{name}: {}", f.message);
        assert!(
            r.evidence.iter().any(|(_, e)| !e.is_healthy()),
            "{name}: rejected evidence is kept"
        );
    }
}

#[test]
fn the_fake_backend_still_scripts_passing_checks_without_networking() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend();
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(d.status, DeploymentStatus::Succeeded);
    assert!(!r.evidence.is_empty());
    assert!(r.evidence.iter().all(|(_, e)| e.simulated && e.is_healthy()));
}

// ================================================================ cleanup attempts and results

fn ids() -> (ApplicationId, DeploymentId) {
    let clock = ManualClock::new(5_000_000);
    let mut g = IdGenerator::new(clock, SeqEntropy::new(3));
    (g.application().unwrap(), g.deployment().unwrap())
}

struct L {
    app: ApplicationId,
    dep: DeploymentId,
    key: ResourceKey,
    ledger: Ledger,
    records: Vec<LedgerRecord>,
}

impl L {
    fn new() -> L {
        let (app, dep) = ids();
        let key = ResourceKey::new(ResourceKind::Container, "hp-test");
        let mut l = L {
            app,
            dep,
            key,
            ledger: Ledger::new(),
            records: Vec::new(),
        };
        l.put(LedgerRecord::Intent {
            key: l.key.clone(),
            app: l.app.clone(),
            deployment: l.dep.clone(),
            disposition: Disposition::RemoveAlways,
            at_ms: 1,
        })
        .unwrap();
        l.put(LedgerRecord::Receipt {
            key: l.key.clone(),
            deployment: l.dep.clone(),
            id: "c1".into(),
            at_ms: 2,
        })
        .unwrap();
        l
    }
    fn put(&mut self, r: LedgerRecord) -> Result<(), LedgerError> {
        self.ledger.apply(&r)?;
        self.records.push(r);
        Ok(())
    }
    fn attempt(&mut self) -> Result<(), LedgerError> {
        self.put(LedgerRecord::CleanupAttempt {
            key: self.key.clone(),
            deployment: self.dep.clone(),
            at_ms: 10,
        })
    }
    fn result(&mut self, outcome: CleanupOutcome) -> Result<(), LedgerError> {
        self.put(LedgerRecord::CleanupResult {
            key: self.key.clone(),
            deployment: self.dep.clone(),
            outcome,
            at_ms: 11,
        })
    }
    fn entry(&self) -> &Entry {
        self.ledger.find(&self.dep, &self.key).unwrap()
    }
}

#[test]
fn a_result_without_any_attempt_is_rejected_and_changes_nothing() {
    let mut l = L::new();
    let before = l.ledger.clone();
    assert!(matches!(
        l.result(CleanupOutcome::Removed),
        Err(LedgerError::OutOfOrder(..))
    ));
    assert_eq!(l.ledger, before);
    assert!(!l.entry().is_released());
}

#[test]
fn a_duplicate_result_cannot_overwrite_a_recorded_failure() {
    let mut l = L::new();
    l.attempt().unwrap();
    l.result(CleanupOutcome::Failed("boom".into())).unwrap();
    assert!(l.entry().needs_attention());
    // the same attempt cannot be answered twice, by success or by anything else
    for outcome in [
        CleanupOutcome::Removed,
        CleanupOutcome::AlreadyAbsent,
        CleanupOutcome::Failed("other".into()),
        CleanupOutcome::Refused("no".into()),
    ] {
        let before = l.ledger.clone();
        assert!(matches!(l.result(outcome), Err(LedgerError::OutOfOrder(..))));
        assert_eq!(l.ledger, before, "a rejected record changes nothing");
    }
    assert_eq!(l.entry().state, EntryState::CleanupFailed("boom".into()));
    assert_eq!(l.entry().cleanup_attempts, 1);
}

#[test]
fn a_duplicate_result_after_success_is_rejected_too() {
    let mut l = L::new();
    l.attempt().unwrap();
    l.result(CleanupOutcome::Removed).unwrap();
    assert!(matches!(
        l.result(CleanupOutcome::Removed),
        Err(LedgerError::OutOfOrder(..))
    ));
    assert!(matches!(
        l.result(CleanupOutcome::Failed("late".into())),
        Err(LedgerError::OutOfOrder(..))
    ));
    assert!(l.entry().is_released());
}

#[test]
fn a_failed_cleanup_followed_by_a_real_retry_may_succeed() {
    let mut l = L::new();
    l.attempt().unwrap();
    l.result(CleanupOutcome::Failed("daemon busy".into())).unwrap();
    assert!(l.entry().needs_attention());
    l.attempt().unwrap(); // the retry is a genuine second attempt
    assert!(l.entry().cleanup_outstanding);
    l.result(CleanupOutcome::Removed).unwrap();
    assert_eq!(l.entry().state, EntryState::Released(Released::Removed));
    assert_eq!(l.entry().cleanup_attempts, 2);
    assert!(!l.entry().needs_attention());
    // and the failure is still in the durable history
    assert!(l.records.iter().any(
        |r| matches!(r, LedgerRecord::CleanupResult { outcome: CleanupOutcome::Failed(m), .. } if m == "daemon busy")
    ));
    // the retry's attempt is consumed: no third result
    assert!(l.result(CleanupOutcome::Removed).is_err());
}

#[test]
fn an_interrupted_cleanup_is_unresolved_and_never_counts_as_success() {
    let mut l = L::new();
    l.attempt().unwrap(); // process dies here: no result
    let e = l.entry();
    assert!(e.cleanup_outstanding && e.needs_attention() && !e.is_released());
    assert_eq!(l.ledger.cleanup_interrupted().len(), 1);
    assert_eq!(l.ledger.needing_attention().len(), 1);
    assert_eq!(l.ledger.unsettled().len(), 1);

    // replay reproduces the same unresolved state
    let replayed = Ledger::replay(&l.records).unwrap();
    assert_eq!(replayed, l.ledger);
    assert_eq!(replayed.cleanup_interrupted().len(), 1);

    // a restart tries again: a new attempt takes over, then exactly one result
    l.attempt().unwrap();
    assert_eq!(l.entry().cleanup_attempts, 2);
    l.result(CleanupOutcome::Failed("still stuck".into())).unwrap();
    assert!(l.entry().needs_attention() && !l.entry().cleanup_outstanding);
    assert!(l.result(CleanupOutcome::Removed).is_err());
}

#[test]
fn replaying_a_journal_with_a_stray_or_duplicate_result_stops_at_that_record() {
    let mut l = L::new();
    l.attempt().unwrap();
    l.result(CleanupOutcome::Failed("boom".into())).unwrap();
    let good = l.records.clone();
    assert!(Ledger::replay(&good).is_ok());

    // duplicate result appended
    let mut dup = good.clone();
    dup.push(LedgerRecord::CleanupResult {
        key: l.key.clone(),
        deployment: l.dep.clone(),
        outcome: CleanupOutcome::Removed,
        at_ms: 99,
    });
    let (idx, err) = Ledger::replay(&dup).unwrap_err();
    assert_eq!(idx, good.len());
    assert!(matches!(err, LedgerError::OutOfOrder(..)));

    // result with no attempt at all
    let mut stray = good[..2].to_vec();
    stray.push(dup.last().unwrap().clone());
    let (idx, err) = Ledger::replay(&stray).unwrap_err();
    assert_eq!(idx, 2);
    assert!(matches!(err, LedgerError::OutOfOrder(..)));
}

#[test]
fn the_wire_codec_round_trips_the_new_state() {
    let mut l = L::new();
    l.attempt().unwrap();
    l.result(CleanupOutcome::Failed("boom".into())).unwrap();
    l.attempt().unwrap();
    let mut text = String::new();
    for r in &l.records {
        text.push_str(&codec::encode(r));
        text.push('\n');
    }
    let rec = codec::recover(text.as_bytes()).expect("clean log");
    let again = Ledger::replay(&rec.records).unwrap();
    assert_eq!(again, l.ledger);
    assert!(again.find(&l.dep, &l.key).unwrap().cleanup_outstanding);
}

// ---- crash injection through the engine

fn container_name(d: &harpoon_deploy::state::Deployment) -> String {
    format!("hp-{}", d.id)
}

/// A failed final check with a first deployment: the new container must be removed.
fn failing_deployment(fx: &Fx, d: &mut harpoon_deploy::state::Deployment, remove_fails: bool) -> FakeBackend {
    let b = fx.backend().fail_final_check("x");
    if remove_fails {
        b.fail_remove(&container_name(d))
    } else {
        b
    }
}

fn restart(
    fx: &Fx,
    old: &MemoryJournal,
    new: &mut MemoryJournal,
    backend: &mut FakeBackend,
) -> Result<Ledger, EngineError> {
    let mut ledger = Ledger::replay(&old.ledger_records).expect("ledger records replay");
    let mut d = old.last().expect("a snapshot exists").clone();
    recover_on_start(std::slice::from_mut(&mut d), fx.clock.now_ms());
    let end = end_state(d.status).unwrap();
    reconcile(&mut ledger, &[(d.id.clone(), end)], new, backend, &fx.clock)?;
    Ok(ledger)
}

fn no_released_entry_still_exists(fx: &Fx, ledger: &Ledger, ctx: &str) {
    let w = fx.world.borrow();
    for e in ledger.entries().iter().filter(|e| e.is_released()) {
        if e.key.kind == ResourceKind::PortLease {
            continue;
        }
        assert!(
            !w.objects
                .iter()
                .any(|o| o.kind == e.key.kind && o.name == e.key.name && o.labelled_for(&e.app, &e.deployment)),
            "{ctx}: {} is recorded as released but still exists",
            e.key.name
        );
    }
}

#[test]
fn a_crash_at_every_write_never_turns_an_unfinished_or_failed_cleanup_into_success() {
    let total = {
        let mut fx = Fx::new();
        let mut d = fx.deployment();
        let mut b = failing_deployment(&fx, &mut d, true);
        let mut j = MemoryJournal::new();
        let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
        assert!(!r.cleanup_problems().is_empty(), "the failed removal is in the outcome");
        j.writes()
    };
    let mut saw_interrupted = false;
    let mut saw_persisted_failure = false;
    for crash_at in 0..total {
        let ctx = format!("crash @{crash_at}");
        let mut fx = Fx::new();
        let bystanders = fx.add_bystanders();
        let mut d = fx.deployment();
        let mut b = failing_deployment(&fx, &mut d, true);
        let mut first = MemoryJournal::crash_at(crash_at);
        let res = run(&fx, &mut d, &Fx::first(), &mut b, &mut first);
        assert!(matches!(res, Err(EngineError::Journal(JournalError::Crash))), "{ctx}");

        if first.last().is_none() {
            assert!(
                first.ledger_records.is_empty(),
                "{ctx}: nothing durable, nothing to recover"
            );
            continue;
        }
        let before = Ledger::replay(&first.ledger_records).expect("replays after a crash");
        for e in before.entries() {
            if e.cleanup_outstanding {
                saw_interrupted = true;
                assert!(e.needs_attention() && !e.is_released(), "{ctx}: {e:?}");
            }
        }
        assert_eq!(
            before.cleanup_interrupted().len(),
            before.entries().iter().filter(|e| e.cleanup_outstanding).count()
        );

        // Restart while removal still fails: nothing may be reported as cleaned.
        let mut second = MemoryJournal::new();
        let mut still_failing = failing_deployment(&fx, &mut d, true);
        let after = restart(&fx, &first, &mut second, &mut still_failing).expect(&ctx);
        no_released_entry_still_exists(&fx, &after, &ctx);
        let container_exists = fx
            .world
            .borrow()
            .objects
            .iter()
            .any(|o| o.kind == ResourceKind::Container && o.name == container_name(&d));
        if container_exists {
            let e = after
                .find(&d.id, &ResourceKey::new(ResourceKind::Container, container_name(&d)))
                .unwrap_or_else(|| panic!("{ctx}: a live labelled container has no ledger entry"));
            assert!(e.needs_attention() && !e.is_released(), "{ctx}: {e:?}");
            saw_persisted_failure |= second.ledger_records.iter().any(|r| {
                matches!(
                    r,
                    LedgerRecord::CleanupResult {
                        outcome: CleanupOutcome::Failed(_),
                        ..
                    }
                )
            });

            // A legitimate retry with a working runtime resolves it, with a fresh attempt.
            let attempts_before = e.cleanup_attempts;
            let mut durable = MemoryJournal::new();
            durable.snapshots = first.snapshots.clone();
            durable.ledger_records = first
                .ledger_records
                .iter()
                .chain(second.ledger_records.iter())
                .cloned()
                .collect();
            let mut third = MemoryJournal::new();
            let mut healthy = fx.backend();
            let done = restart(&fx, &durable, &mut third, &mut healthy).expect(&ctx);
            let e = done
                .find(&d.id, &ResourceKey::new(ResourceKind::Container, container_name(&d)))
                .unwrap();
            assert!(e.is_released() && !e.cleanup_outstanding, "{ctx}: {e:?}");
            assert!(
                e.cleanup_attempts > attempts_before,
                "{ctx}: the retry is its own attempt"
            );
            no_released_entry_still_exists(&fx, &done, &ctx);
        }
        for by in &bystanders {
            assert!(fx.world.borrow().objects.contains(by), "{ctx}: touched {}", by.name);
        }
    }
    assert!(
        saw_interrupted,
        "some crash point lands between a cleanup attempt and its result"
    );
    assert!(saw_persisted_failure, "some recovery recorded a failed cleanup durably");
}

#[test]
fn a_crash_while_recovering_leaves_the_interrupted_attempt_visible_and_retryable() {
    let mut seen_outstanding = false;
    for first_crash in 0..40 {
        for crash_at in 0..40 {
            let ctx = format!("deployment crash @{first_crash}, recovery crash @{crash_at}");
            let mut fx = Fx::new();
            let mut d = fx.deployment();
            let mut b = fx.backend().fail_final_check("x");
            let mut first = MemoryJournal::crash_at(first_crash);
            if run(&fx, &mut d, &Fx::first(), &mut b, &mut first).is_ok() || first.last().is_none() {
                continue;
            }
            let mut rec1 = MemoryJournal::crash_at(crash_at);
            let mut h = fx.backend();
            if restart(&fx, &first, &mut rec1, &mut h).is_ok() {
                break; // recovery finished before this crash point
            }
            let durable_records: Vec<LedgerRecord> = first
                .ledger_records
                .iter()
                .chain(rec1.ledger_records.iter())
                .cloned()
                .collect();
            let mid = Ledger::replay(&durable_records).expect("replays");
            seen_outstanding |= !mid.cleanup_interrupted().is_empty();
            for e in mid.cleanup_interrupted() {
                assert!(e.needs_attention() && !e.is_released(), "{ctx}");
            }
            let mut durable = MemoryJournal::new();
            durable.snapshots = first.snapshots.clone();
            durable.ledger_records = durable_records;
            let mut rec2 = MemoryJournal::new();
            let mut h2 = fx.backend();
            let done = restart(&fx, &durable, &mut rec2, &mut h2).expect(&ctx);
            assert!(done.cleanup_interrupted().is_empty(), "{ctx}");
            no_released_entry_still_exists(&fx, &done, &ctx);
        }
    }
    assert!(
        seen_outstanding,
        "a crash between a recovery attempt and its result was exercised"
    );
}
