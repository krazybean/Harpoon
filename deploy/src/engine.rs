//! The deployment lifecycle driver (spec D5, D10), written against [`Backend`] and a
//! [`Journal`] so it is fully deterministic offline.
//!
//! Ordering guarantees, each covered by tests:
//! * the record is persisted **before** every external side effect;
//! * nothing is stopped until the build and the preliminary check have succeeded;
//! * the cutover journal is written *before* the previous release is stopped;
//! * only a failure **after** the cutover began triggers the single, bounded restoration;
//! * the engine never retries, rolls back, promotes or resumes anything by itself.

use crate::backend::{Backend, Event, RestoreResult, StepError, Trace};
use crate::ids::ReleaseId;
use crate::state::{Deployment, DeploymentStatus, Failure, FailureKind, RecoveryOutcome, Step, TransitionError};
use crate::time::Clock;
use std::fmt;

/// Durable storage of deployment records. The real one will be SQLite; the engine only
/// needs "write this record now".
pub trait Journal {
    fn persist(&mut self, d: &Deployment) -> Result<(), JournalError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    /// Used by tests to model the process dying at this exact write.
    Crash,
    Io(String),
}

/// A journal that keeps every snapshot in memory and can "crash" at the Nth write.
#[derive(Default)]
pub struct MemoryJournal {
    pub snapshots: Vec<Deployment>,
    crash_at: Option<usize>,
    trace: Option<Trace>,
}

impl MemoryJournal {
    pub fn new() -> MemoryJournal {
        MemoryJournal::default()
    }

    /// The write with this zero-based index fails with `Crash` and is **not** stored,
    /// exactly as if the process had been killed just before it happened.
    pub fn crash_at(n: usize) -> MemoryJournal {
        MemoryJournal {
            snapshots: Vec::new(),
            crash_at: Some(n),
            trace: None,
        }
    }

    /// Record every successful write into a shared trace, interleaved with backend calls.
    pub fn with_trace(mut self, t: Trace) -> Self {
        self.trace = Some(t);
        self
    }

    pub fn last(&self) -> Option<&Deployment> {
        self.snapshots.last()
    }
}

impl Journal for MemoryJournal {
    fn persist(&mut self, d: &Deployment) -> Result<(), JournalError> {
        if self.crash_at == Some(self.snapshots.len()) {
            return Err(JournalError::Crash);
        }
        if let Some(t) = &self.trace {
            let running = d
                .steps
                .iter()
                .find(|s| s.status == crate::state::StepStatus::Running)
                .map(|s| s.step);
            t.borrow_mut().push(Event::Persist {
                status: d.status,
                running,
                cutover: d.cutover,
            });
        }
        self.snapshots.push(d.clone());
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Time budget for the single restoration attempt. Default 60 s (spec D5).
    pub restore_budget_ms: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            restore_budget_ms: 60_000,
        }
    }
}

/// What the caller supplies about the application's current state.
#[derive(Debug, Clone)]
pub struct Context {
    /// The release currently serving, if any. `None` means a first deployment: there is
    /// nothing to stop, so no cutover and no recovery.
    pub previous_release: Option<ReleaseId>,
}

/// Why automatic triggers should pause. Only a failed cutover produces one; an ordinary
/// build or preliminary-check failure never does (nothing was stopped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    FailedCutover,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: DeploymentStatus,
    pub pause: Option<PauseReason>,
}

#[derive(Debug)]
pub enum EngineError {
    /// A write to the journal failed. State on disk is whatever was last persisted.
    Journal(JournalError),
    /// A bug in the caller or the engine: an invalid transition was requested.
    Transition(TransitionError),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Journal(e) => write!(f, "journal write failed: {e:?}"),
            EngineError::Transition(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<JournalError> for EngineError {
    fn from(e: JournalError) -> Self {
        EngineError::Journal(e)
    }
}
impl From<TransitionError> for EngineError {
    fn from(e: TransitionError) -> Self {
        EngineError::Transition(e)
    }
}

type R<T> = Result<T, EngineError>;

struct Run<'a> {
    d: &'a mut Deployment,
    backend: &'a mut dyn Backend,
    journal: &'a mut dyn Journal,
    clock: &'a dyn Clock,
    cancel: &'a dyn Fn() -> bool,
}

impl Run<'_> {
    fn now(&self) -> u64 {
        self.clock.now_ms()
    }
    fn save(&mut self) -> R<()> {
        self.journal.persist(self.d)?;
        Ok(())
    }

    /// Run one pre-cutover step. `Ok(true)` = continue, `Ok(false)` = the deployment ended.
    fn pre_step(&mut self, step: Step, kind: FailureKind, f: fn(&mut dyn Backend) -> Result<(), StepError>) -> R<bool> {
        if (self.cancel)() {
            return self.end_cancelled().map(|_| false);
        }
        let now = self.now();
        self.d.start_step(step, now)?;
        self.save()?; // before the side effect
        let res = f(&mut *self.backend);
        let now = self.now();
        match res {
            Ok(()) => {
                self.d.finish_step(step, true, "", now)?;
                if step == Step::Build {
                    self.d.release = Some(ReleaseId::from_deployment(&self.d.id));
                }
                self.save()?;
                Ok(true)
            }
            Err(e) => {
                self.d.finish_step(step, false, &e.message, now)?;
                self.d.failure = Some(Failure {
                    step,
                    kind,
                    message: e.message,
                });
                self.d.transition(DeploymentStatus::Failed, now)?;
                self.save()?;
                Ok(false)
            }
        }
    }

    fn end_cancelled(&mut self) -> R<()> {
        let now = self.now();
        self.d.transition(DeploymentStatus::Cancelled, now)?;
        self.save()
    }

    fn succeed(&mut self) -> R<()> {
        let now = self.now();
        self.d.transition(DeploymentStatus::Succeeded, now)?;
        self.save()
    }

    /// A failure after the cutover began: persist it, then make the one bounded
    /// restoration attempt and record the outcome.
    fn fail_after_cutover(
        &mut self,
        step: Step,
        kind: FailureKind,
        message: String,
        new_started: bool,
        budget_ms: u64,
    ) -> R<()> {
        let now = self.now();
        self.d.finish_step(step, false, &message, now)?;
        self.d.failure = Some(Failure { step, kind, message });
        self.d.fail_cutover(now)?;
        self.d.start_step(Step::Recovery, now)?;
        self.d.start_recovery(budget_ms, now)?;
        self.save()?; // recovery is recorded as in progress before it starts

        if new_started {
            let _ = self.backend.stop_new(); // best effort: free the address for the restore
        }
        let outcome = match self.backend.restore_previous(budget_ms) {
            RestoreResult::Healthy => RecoveryOutcome::Restored,
            RestoreResult::StartedUnverified => RecoveryOutcome::RestoredUnverified,
            RestoreResult::Failed(r) => RecoveryOutcome::RestoreFailed(r),
            RestoreResult::NotAttempted(r) => RecoveryOutcome::NotAttempted(r),
        };
        let now = self.now();
        self.d.finish_recovery(outcome, now)?;
        let ok = outcome.service_restored();
        self.d.finish_step(Step::Recovery, ok, outcome.as_str(), now)?;
        self.d.transition(DeploymentStatus::Failed, now)?;
        self.save()
    }
}

/// Drive one deployment from `Queued` to a terminal state.
///
/// `cancel` is polled before each step up to and including the start of the cutover.
/// Once the cutover has begun it is ignored: the engine never leaves a half-applied
/// switch behind because of a cancel request.
pub fn run_deployment(
    d: &mut Deployment,
    ctx: &Context,
    backend: &mut dyn Backend,
    journal: &mut dyn Journal,
    clock: &dyn Clock,
    policy: &Policy,
    cancel: &dyn Fn() -> bool,
) -> Result<Report, EngineError> {
    let mut run = Run {
        d,
        backend,
        journal,
        clock,
        cancel,
    };
    let now = run.now();
    run.d.transition(DeploymentStatus::Running, now)?;
    run.save()?;

    if !run.pre_step(Step::Source, FailureKind::Source, |b| b.prepare_source())? {
        return finish(run);
    }
    if !run.pre_step(Step::Build, FailureKind::Build, |b| b.build())? {
        return finish(run);
    }
    if !run.pre_step(Step::PreliminaryCheck, FailureKind::PreliminaryCheck, |b| {
        b.preliminary_check()
    })? {
        return finish(run);
    }

    // Last cancellation point: after this the cutover is owned by the engine.
    if (run.cancel)() {
        run.end_cancelled()?;
        return finish(run);
    }

    let budget = policy.restore_budget_ms;
    match &ctx.previous_release {
        None => {
            // First deployment: nothing to stop, nothing to restore.
            run.d.cutover_not_required()?;
            let now = run.now();
            run.d.start_step(Step::Cutover, now)?;
            run.save()?;
            let started = run.backend.start_new();
            let now = run.now();
            if let Err(e) = started {
                run.d.finish_step(Step::Cutover, false, &e.message, now)?;
                run.d.failure = Some(Failure {
                    step: Step::Cutover,
                    kind: FailureKind::Cutover,
                    message: e.message,
                });
                run.d.transition(DeploymentStatus::Failed, now)?;
                run.save()?;
                return finish(run);
            }
            run.d.finish_step(Step::Cutover, true, "", now)?;
            run.d.start_step(Step::FinalCheck, now)?;
            run.save()?;
            let chk = run.backend.final_check();
            let now = run.now();
            match chk {
                Ok(()) => {
                    run.d.finish_step(Step::FinalCheck, true, "", now)?;
                    run.succeed()?;
                }
                Err(e) => {
                    run.d.finish_step(Step::FinalCheck, false, &e.message, now)?;
                    run.d.failure = Some(Failure {
                        step: Step::FinalCheck,
                        kind: FailureKind::FinalCheck,
                        message: e.message,
                    });
                    let _ = run.backend.stop_new();
                    run.d.transition(DeploymentStatus::Failed, now)?;
                    run.save()?;
                }
            }
            finish(run)
        }
        Some(_) => {
            // The journal entry goes to disk BEFORE the healthy release is stopped.
            let now = run.now();
            run.d.begin_cutover(now)?;
            run.d.start_step(Step::Cutover, now)?;
            run.save()?;

            if let Err(e) = run.backend.stop_current() {
                run.fail_after_cutover(Step::Cutover, FailureKind::Cutover, e.message, false, budget)?;
                return finish(run);
            }
            if let Err(e) = run.backend.start_new() {
                run.fail_after_cutover(Step::Cutover, FailureKind::Cutover, e.message, false, budget)?;
                return finish(run);
            }
            let now = run.now();
            run.d.finish_step(Step::Cutover, true, "", now)?;
            run.d.start_step(Step::FinalCheck, now)?;
            run.save()?;
            match run.backend.final_check() {
                Ok(()) => {
                    let now = run.now();
                    run.d.finish_step(Step::FinalCheck, true, "", now)?;
                    run.d.confirm_cutover(now)?;
                    run.succeed()?;
                }
                Err(e) => {
                    run.fail_after_cutover(Step::FinalCheck, FailureKind::FinalCheck, e.message, true, budget)?;
                }
            }
            finish(run)
        }
    }
}

fn finish(run: Run<'_>) -> Result<Report, EngineError> {
    let pause = match run.d.cutover {
        crate::state::CutoverState::Failed { .. } => Some(PauseReason::FailedCutover),
        _ => None,
    };
    Ok(Report {
        status: run.d.status,
        pause,
    })
}
