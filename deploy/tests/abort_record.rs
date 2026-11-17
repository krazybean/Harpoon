//! A run that stops on an error or a panic must not leave its deployment `running` with nothing said, and must not guess
//! about a cutover that began. Deterministic: fake backend, in-memory journal, manual clock.

mod common;

use common::*;
use harpoon_deploy::engine::{EngineError, Journal, JournalError, MemoryJournal};
use harpoon_deploy::state::*;

/// How many journal writes a whole run makes.
fn total_writes(has_previous: bool) -> usize {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let ctx = if has_previous { fx.previous() } else { Fx::first() };
    let mut b = fx.backend();
    let mut j = MemoryJournal::new();
    run(&fx, &mut d, &ctx, &mut b, &mut j).unwrap();
    j.writes()
}

fn last_of(j: &MemoryJournal) -> Option<&Deployment> {
    j.last()
}

#[test]
fn a_journal_write_that_fails_once_never_leaves_the_deployment_running_unless_a_cutover_began() {
    let mut recorded = 0;
    let mut left_for_recovery = 0;
    for has_previous in [false, true] {
        for n in 0..total_writes(has_previous) {
            let mut fx = Fx::new();
            let mut d = fx.deployment();
            let ctx = if has_previous { fx.previous() } else { Fx::first() };
            let mut b = fx.backend();
            let mut j = MemoryJournal::fail_once_at(n);
            let res = run(&fx, &mut d, &ctx, &mut b, &mut j);
            if !matches!(res, Err(EngineError::Journal(JournalError::Io(_)))) {
                continue; // the failed write was not fatal to the run
            }
            let Some(last) = last_of(&j) else { continue }; // nothing had been written yet: there is no deployment to record
            let cut_over = matches!(d.cutover, CutoverState::Begun { .. } | CutoverState::Failed { .. });
            if cut_over {
                // ambiguous: the previous release may be stopped and the candidate started. Not guessed; `recover`/`restore` decide.
                assert_eq!(
                    last.status,
                    DeploymentStatus::Running,
                    "write {n} (previous={has_previous}): a begun cutover must stay for recovery"
                );
                left_for_recovery += 1;
            } else {
                assert!(
                    last.status.is_terminal(),
                    "write {n} (previous={has_previous}): left {:?}; in memory {:?} cutover {:?} steps {:?}",
                    last.status,
                    d.status,
                    d.cutover,
                    d.steps.iter().map(|s| (s.step, s.status)).collect::<Vec<_>>()
                );
                if let Some(f) = last
                    .failure
                    .as_ref()
                    .filter(|f| f.message.contains("stopped before it could record"))
                {
                    assert_eq!(last.status, DeploymentStatus::Failed, "{}", f.message);
                    recorded += 1;
                }
            }
        }
    }
    assert!(recorded > 0, "no write failure landed in a plain step");
    assert!(left_for_recovery > 0, "no write failure landed inside a cutover");
}

/// A journal that panics on one write, then behaves.
struct PanicOnce {
    inner: MemoryJournal,
    at: usize,
    seen: usize,
}
impl PanicOnce {
    fn tick(&mut self) {
        if self.seen == self.at {
            self.seen += 1;
            panic!("injected panic");
        }
        self.seen += 1;
    }
}
impl Journal for PanicOnce {
    fn persist(&mut self, d: &Deployment) -> Result<(), JournalError> {
        self.tick();
        self.inner.persist(d)
    }
    fn record(&mut self, r: &harpoon_deploy::ledger::LedgerRecord) -> Result<(), JournalError> {
        self.tick();
        self.inner.record(r)
    }
}

#[test]
fn a_panic_inside_the_run_is_recorded_not_lost() {
    // the first write that lands inside the build step (after the deployment is running and a step is open)
    for n in 3..12 {
        let mut fx = Fx::new();
        let mut d = fx.deployment();
        let mut b = fx.backend();
        let mut j = PanicOnce {
            inner: MemoryJournal::new(),
            at: n,
            seen: 0,
        };
        let cfg = harpoon_deploy::engine::RunConfig::default();
        let res =
            harpoon_deploy::engine::run_deployment_with(&mut d, &Fx::first(), &mut b, &mut j, &fx.clock, &cfg, &never);
        let Err(EngineError::Panicked(m)) = res else {
            panic!("write {n}: {res:?}")
        };
        assert_eq!(m, "injected panic");
        let last = j.inner.last().expect("a snapshot");
        if d.status == DeploymentStatus::Failed {
            assert_eq!(
                last.status,
                DeploymentStatus::Failed,
                "write {n}: the panic must be recorded"
            );
            assert!(last.failure.as_ref().unwrap().message.contains("panicked"));
            return;
        }
    }
    panic!("no injected panic landed inside an open step");
}
