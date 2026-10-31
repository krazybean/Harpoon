//! The deployment lifecycle driver (spec D5, D10), written against [`Backend`] and a
//! [`Journal`] so it is fully deterministic offline.
//!
//! Ordering guarantees, each covered by tests:
//! * the record is persisted **before** every external side effect;
//! * nothing is stopped until the build and the preliminary check have succeeded;
//! * the cutover journal is written *before* the previous release is stopped;
//! * only a failure **after** the cutover began triggers the single, bounded restoration;
//! * the engine never retries, rolls back, promotes or resumes anything by itself.

use crate::backend::{Backend, Event, Op, OpTrace, ResourceOps, RestoreResult, StepError, Trace};
use crate::health::CheckEvidence;
use crate::ids::{ApplicationId, DeploymentId, ReleaseId};
use crate::ledger::{
    plan, Action, CleanupOutcome, Disposition, EndState, Ledger, LedgerError, LedgerRecord, Observed, RemoveRequest,
    RemoveResult, ResourceKey, ResourceKind,
};
use crate::logs::{LogCapture, LogLimits, LogSink, Redactor};
use crate::ports::PortLease;
use crate::state::{Deployment, DeploymentStatus, Failure, FailureKind, RecoveryOutcome, Step, TransitionError};
use crate::time::Clock;
use std::fmt;

/// Durable storage. The real ones are a JSON-lines file (first slice only) and later SQLite;
/// the engine only needs "write this now".
///
/// **Durability contract:** when `persist` or `record` returns `Ok`, the write has reached
/// stable storage (file `fsync`, and the directory on creation). Returning `Ok` for a write
/// that can still be lost is a bug in the implementation, because the engine acts on the
/// strength of it — a ledger *intent* is only useful if it survives the crash that follows
/// the create call.
pub trait Journal {
    fn persist(&mut self, d: &Deployment) -> Result<(), JournalError>;
    /// Append one resource-ledger record (intent, receipt, cleanup attempt/result).
    fn record(&mut self, r: &LedgerRecord) -> Result<(), JournalError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    /// Used by tests to model the process dying at this exact write.
    Crash,
    Io(String),
}

/// A journal that keeps every write in memory and can "crash" at the Nth write. Deployment
/// snapshots and ledger records share one write counter, so a crash can land between an
/// intent and its receipt, or between a cleanup attempt and its result.
#[derive(Default)]
pub struct MemoryJournal {
    pub snapshots: Vec<Deployment>,
    pub ledger_records: Vec<LedgerRecord>,
    writes: usize,
    crash_at: Option<usize>,
    trace: Option<Trace>,
    ops: Option<OpTrace>,
}

impl MemoryJournal {
    pub fn new() -> MemoryJournal {
        MemoryJournal::default()
    }

    /// The write with this zero-based index fails with `Crash` and is **not** stored,
    /// exactly as if the process had been killed just before it happened.
    pub fn crash_at(n: usize) -> MemoryJournal {
        MemoryJournal {
            crash_at: Some(n),
            ..MemoryJournal::default()
        }
    }

    /// Record every successful write into a shared trace, interleaved with backend calls.
    pub fn with_trace(mut self, t: Trace) -> Self {
        self.trace = Some(t);
        self
    }

    /// Also record every ledger write into a shared [`OpTrace`].
    pub fn with_ops_trace(mut self, t: OpTrace) -> Self {
        self.ops = Some(t);
        self
    }

    pub fn last(&self) -> Option<&Deployment> {
        self.snapshots.last()
    }

    /// Total successful writes (snapshots + ledger records).
    pub fn writes(&self) -> usize {
        self.writes
    }

    fn gate(&mut self) -> Result<(), JournalError> {
        if self.crash_at == Some(self.writes) {
            return Err(JournalError::Crash);
        }
        self.writes += 1;
        Ok(())
    }
}

impl Journal for MemoryJournal {
    fn persist(&mut self, d: &Deployment) -> Result<(), JournalError> {
        self.gate()?;
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

    fn record(&mut self, r: &LedgerRecord) -> Result<(), JournalError> {
        self.gate()?;
        if let Some(t) = &self.ops {
            t.borrow_mut().push(Op::Write(r.clone()));
        }
        self.ledger_records.push(r.clone());
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

/// Per-step time budgets (review gap #2). A step that runs past its budget fails with an
/// explicit timeout, **even if the backend eventually returned success**: a late answer is
/// not trusted. Defaults are the first-test values (build 10 min, health 60 s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    pub source_ms: u64,
    pub build_ms: u64,
    pub preliminary_ms: u64,
    pub cutover_ms: u64,
    pub final_ms: u64,
    /// The whole resource-cleanup phase of one deployment.
    pub cleanup_ms: u64,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets {
            source_ms: 120_000,
            build_ms: 600_000,
            preliminary_ms: 60_000,
            cutover_ms: 60_000,
            final_ms: 60_000,
            cleanup_ms: 60_000,
        }
    }
}

impl Budgets {
    pub fn for_step(&self, s: Step) -> u64 {
        match s {
            Step::Source => self.source_ms,
            Step::Build => self.build_ms,
            Step::PreliminaryCheck => self.preliminary_ms,
            Step::Cutover => self.cutover_ms,
            Step::FinalCheck => self.final_ms,
            Step::Recovery => self.cleanup_ms,
        }
    }
}

/// Everything beyond [`Policy`]. `run_deployment` uses the defaults.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub policy: Policy,
    pub budgets: Budgets,
    pub log_limits: LogLimits,
    pub redactor: Redactor,
    /// The ledger as restored from the durable journal. The run starts from it, so leases and
    /// unsettled entries of other deployments are visible to port allocation. Empty by default.
    pub prior_ledger: Ledger,
}

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig {
            policy: Policy::default(),
            budgets: Budgets::default(),
            log_limits: LogLimits::default(),
            redactor: Redactor::none(),
            prior_ledger: Ledger::new(),
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
    pub deployment: DeploymentId,
    pub status: DeploymentStatus,
    pub pause: Option<PauseReason>,
    /// The ledger as it ended. Failed or refused cleanups are in here and stay there.
    pub ledger: Ledger,
    /// What each check actually observed, in step order.
    pub evidence: Vec<(Step, CheckEvidence)>,
    /// Bounded, redacted logs per step.
    pub logs: Vec<(Step, LogCapture)>,
    /// Things that went wrong without changing the status (for example a failed `stop_new`).
    pub warnings: Vec<String>,
    /// Objects labelled as this deployment's but unknown to the ledger. Reported, never removed.
    pub unaccounted: Vec<Observed>,
}

impl Report {
    /// Entries of **this deployment** whose cleanup failed or was refused. A deployment can be
    /// `Succeeded` or `Failed` and still have these; they are never dropped from the report.
    pub fn cleanup_problems(&self) -> Vec<&crate::ledger::Entry> {
        self.ledger
            .needing_attention()
            .into_iter()
            .filter(|e| e.deployment == self.deployment)
            .collect()
    }
    pub fn is_clean(&self) -> bool {
        self.cleanup_problems().is_empty() && self.unaccounted.is_empty() && self.warnings.is_empty()
    }
}

#[derive(Debug)]
pub enum EngineError {
    /// A write to the journal failed. State on disk is whatever was last persisted.
    Journal(JournalError),
    /// A bug in the caller or the engine: an invalid transition was requested.
    Transition(TransitionError),
    /// A bug in the caller or the engine: an invalid ledger record was attempted.
    Ledger(LedgerError),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Journal(e) => write!(f, "journal write failed: {e:?}"),
            EngineError::Transition(e) => write!(f, "{e}"),
            EngineError::Ledger(e) => write!(f, "{e}"),
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
impl From<LedgerError> for EngineError {
    fn from(e: LedgerError) -> Self {
        EngineError::Ledger(e)
    }
}

type R<T> = Result<T, EngineError>;

// ------------------------------------------------------------------ ledger writes

/// Validate against a copy, make the write durable, and only then adopt the new state: the
/// in-memory ledger never gets ahead of the journal.
fn write_record(ledger: &mut Ledger, journal: &mut dyn Journal, r: LedgerRecord) -> R<()> {
    let mut next = ledger.clone();
    next.apply(&r)?;
    journal.record(&r)?;
    *ledger = next;
    Ok(())
}

/// A recorded intent to create one object. Only [`StepIo::intend`] can make one, and
/// [`StepIo::receipt`] needs it, so "receipt without intent" cannot be written by a backend.
#[derive(Debug)]
pub struct Intent {
    key: ResourceKey,
}

impl Intent {
    pub fn key(&self) -> &ResourceKey {
        &self.key
    }
}

/// What a backend step may use: its deadline, a bounded redacting log, and the ledger
/// protocol. **Create nothing before `intend` returns; record the id with `receipt`.**
pub struct StepIo<'a> {
    /// Time the step has. The backend must stop and report a timeout when it is spent.
    pub budget_ms: u64,
    pub log: &'a mut LogSink,
    ledger: &'a mut Ledger,
    journal: &'a mut dyn Journal,
    clock: &'a dyn Clock,
    app: &'a ApplicationId,
    dep: &'a DeploymentId,
}

impl StepIo<'_> {
    pub fn app(&self) -> &ApplicationId {
        self.app
    }
    pub fn deployment(&self) -> &DeploymentId {
        self.dep
    }
    pub fn ledger(&self) -> &Ledger {
        self.ledger
    }
    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    fn write(&mut self, r: LedgerRecord) -> Result<(), StepError> {
        let mut next = self.ledger.clone();
        next.apply(&r).map_err(|e| StepError::new(e.to_string()))?;
        self.journal.record(&r)?;
        *self.ledger = next;
        Ok(())
    }

    /// Record, durably, that this object is about to be created. After a crash this is all
    /// that is known: the object may or may not exist.
    pub fn intend(&mut self, key: ResourceKey, disposition: Disposition) -> Result<Intent, StepError> {
        let at_ms = self.now_ms();
        self.write(LedgerRecord::Intent {
            key: key.clone(),
            app: self.app.clone(),
            deployment: self.dep.clone(),
            disposition,
            at_ms,
        })?;
        Ok(Intent { key })
    }

    /// Record the **actual id the runtime returned**. Only now is the object known to exist.
    pub fn receipt(&mut self, intent: &Intent, id: &str) -> Result<(), StepError> {
        let at_ms = self.now_ms();
        self.write(LedgerRecord::Receipt {
            key: intent.key.clone(),
            deployment: self.dep.clone(),
            id: id.to_string(),
            at_ms,
        })
    }

    /// Record that a removal of `key` is about to be tried. Pair with [`StepIo::cleanup_result`];
    /// without a result the attempt stays flagged as interrupted.
    pub fn cleanup_attempt(&mut self, key: &ResourceKey) -> Result<(), StepError> {
        let at_ms = self.now_ms();
        self.write(LedgerRecord::CleanupAttempt {
            key: key.clone(),
            deployment: self.dep.clone(),
            at_ms,
        })
    }

    /// Record how the removal ended. One result per attempt (the ledger enforces it).
    pub fn cleanup_result(&mut self, key: &ResourceKey, outcome: CleanupOutcome) -> Result<(), StepError> {
        let at_ms = self.now_ms();
        self.write(LedgerRecord::CleanupResult {
            key: key.clone(),
            deployment: self.dep.clone(),
            outcome,
            at_ms,
        })
    }

    /// Give a reserved port back right away (for example after a collision replaced it): a cleanup
    /// attempt and its result, so the ledger shows the lease was released rather than forgotten.
    pub fn release_lease(&mut self, host_port: u16) -> Result<(), StepError> {
        let key = ResourceKey::new(ResourceKind::PortLease, host_port.to_string());
        self.cleanup_attempt(&key)?;
        self.cleanup_result(&key, CleanupOutcome::Removed)
    }

    /// Reserve a host port: intent and receipt in one call (a lease has no runtime id).
    pub fn lease_port(&mut self, lease: PortLease) -> Result<Intent, StepError> {
        let intent = self.intend(
            ResourceKey::new(ResourceKind::PortLease, lease.host_port.to_string()),
            Disposition::RemoveOnFailure,
        )?;
        self.receipt(&intent, &lease.host_port.to_string())?;
        Ok(intent)
    }
}

// ------------------------------------------------------------------ cleanup and recovery

fn request_for(entry: &crate::ledger::Entry, id: &str) -> RemoveRequest {
    RemoveRequest {
        kind: entry.key.kind,
        id: id.to_string(),
        name: entry.key.name.clone(),
        app: entry.app.clone(),
        deployment: entry.deployment.clone(),
    }
}

/// Carry out a plan. Every removal is bracketed by an attempt and a result record, and an
/// adopted object gets a receipt with the id the runtime reported **before** it is removed.
/// Returns the objects that were only reported, never touched.
pub fn execute_actions(
    actions: &[Action],
    ledger: &mut Ledger,
    journal: &mut dyn Journal,
    ops: &mut dyn ResourceOps,
    clock: &dyn Clock,
) -> R<Vec<Observed>> {
    let mut unaccounted = Vec::new();
    for a in actions {
        match a {
            Action::ReportOnly { observed, .. } => unaccounted.push(observed.clone()),
            Action::Adopt { entry, observed } => {
                write_record(
                    ledger,
                    journal,
                    LedgerRecord::Receipt {
                        key: entry.1.clone(),
                        deployment: entry.0.clone(),
                        id: observed.id.clone(),
                        at_ms: clock.now_ms(),
                    },
                )?;
            }
            Action::ReleaseLease { entry } => {
                attempt(ledger, journal, clock, entry)?;
                settle(ledger, journal, clock, entry, CleanupOutcome::Removed)?;
            }
            Action::MarkAbsent { entry } => {
                attempt(ledger, journal, clock, entry)?;
                settle(ledger, journal, clock, entry, CleanupOutcome::AlreadyAbsent)?;
            }
            Action::Refuse { entry, reason } => {
                attempt(ledger, journal, clock, entry)?;
                settle(ledger, journal, clock, entry, CleanupOutcome::Refused(reason.clone()))?;
            }
            Action::Remove { entry, observed, adopt } => {
                if *adopt {
                    write_record(
                        ledger,
                        journal,
                        LedgerRecord::Receipt {
                            key: entry.1.clone(),
                            deployment: entry.0.clone(),
                            id: observed.id.clone(),
                            at_ms: clock.now_ms(),
                        },
                    )?;
                }
                attempt(ledger, journal, clock, entry)?;
                let e = ledger.find(&entry.0, &entry.1).expect("planned entries exist").clone();
                let outcome = match ops.remove_resource(&request_for(&e, &observed.id)) {
                    Ok(RemoveResult::Removed) => CleanupOutcome::Removed,
                    Ok(RemoveResult::AlreadyAbsent) => CleanupOutcome::AlreadyAbsent,
                    Ok(RemoveResult::Refused(why)) => CleanupOutcome::Refused(why),
                    // A conflict is typed only where the ledger accepts it: a retained image with a
                    // receipt. Anywhere else it is an ordinary failed cleanup, which blocks.
                    Ok(RemoveResult::Conflict(cause, why)) => {
                        if e.key.kind == crate::ledger::ResourceKind::Image
                            && e.disposition == crate::ledger::Disposition::Retain
                            && e.id.is_some()
                        {
                            CleanupOutcome::Conflict(cause, why)
                        } else {
                            CleanupOutcome::Failed(format!("conflict ({}): {why}", cause.as_str()))
                        }
                    }
                    Err(err) => CleanupOutcome::Failed(err.message),
                };
                settle(ledger, journal, clock, entry, outcome)?;
            }
        }
    }
    Ok(unaccounted)
}

fn attempt(
    ledger: &mut Ledger,
    journal: &mut dyn Journal,
    clock: &dyn Clock,
    entry: &(DeploymentId, ResourceKey),
) -> R<()> {
    write_record(
        ledger,
        journal,
        LedgerRecord::CleanupAttempt {
            key: entry.1.clone(),
            deployment: entry.0.clone(),
            at_ms: clock.now_ms(),
        },
    )
}

fn settle(
    ledger: &mut Ledger,
    journal: &mut dyn Journal,
    clock: &dyn Clock,
    entry: &(DeploymentId, ResourceKey),
    outcome: CleanupOutcome,
) -> R<()> {
    write_record(
        ledger,
        journal,
        LedgerRecord::CleanupResult {
            key: entry.1.clone(),
            deployment: entry.0.clone(),
            outcome,
            at_ms: clock.now_ms(),
        },
    )
}

/// List the runtime, plan, execute. If the runtime cannot even be listed, every object that
/// would have needed removal is recorded as a **failed** cleanup (not skipped) and port
/// leases are released (ledger-only). Used right after a deployment and again at startup.
pub fn reconcile(
    ledger: &mut Ledger,
    ended: &[(DeploymentId, EndState)],
    journal: &mut dyn Journal,
    ops: &mut dyn ResourceOps,
    clock: &dyn Clock,
) -> R<Vec<Observed>> {
    match ops.list_resources() {
        Ok(observed) => {
            let actions = plan(ledger, &observed, ended);
            execute_actions(&actions, ledger, journal, ops, clock)
        }
        Err(e) => {
            // Plan against an empty runtime to learn *which* entries need cleanup, then fail them honestly.
            let actions = plan(ledger, &[], ended);
            for a in &actions {
                match a {
                    Action::ReleaseLease { entry } => {
                        attempt(ledger, journal, clock, entry)?;
                        settle(ledger, journal, clock, entry, CleanupOutcome::Removed)?;
                    }
                    Action::MarkAbsent { entry } => {
                        attempt(ledger, journal, clock, entry)?;
                        settle(
                            ledger,
                            journal,
                            clock,
                            entry,
                            CleanupOutcome::Failed(format!("could not list the runtime: {}", e.message)),
                        )?;
                    }
                    _ => {}
                }
            }
            Ok(Vec::new())
        }
    }
}

/// `Some` for a deployment that has finished (or was interrupted); `None` while it may still run.
pub fn end_state(status: DeploymentStatus) -> Option<EndState> {
    match status {
        DeploymentStatus::Succeeded => Some(EndState::Succeeded),
        s if s.is_terminal() => Some(EndState::NotSucceeded),
        _ => None,
    }
}

// ------------------------------------------------------------------ the run

struct Run<'a> {
    d: &'a mut Deployment,
    backend: &'a mut dyn Backend,
    journal: &'a mut dyn Journal,
    clock: &'a dyn Clock,
    cancel: &'a dyn Fn() -> bool,
    cfg: &'a RunConfig,
    ledger: Ledger,
    evidence: Vec<(Step, CheckEvidence)>,
    logs: Vec<(Step, LogCapture)>,
    warnings: Vec<String>,
    unaccounted: Vec<Observed>,
}

type StepOut = Result<Option<CheckEvidence>, StepError>;

impl Run<'_> {
    fn now(&self) -> u64 {
        self.clock.now_ms()
    }
    fn save(&mut self) -> R<()> {
        self.journal.persist(self.d)?;
        Ok(())
    }

    /// Call the backend with a fresh log sink and the step's deadline. A journal failure
    /// inside the step aborts the run; a result that arrives past the budget becomes a timeout.
    fn exec<T>(
        &mut self,
        step: Step,
        budget: u64,
        f: impl FnOnce(&mut dyn Backend, &mut StepIo<'_>) -> Result<T, StepError>,
    ) -> R<(Result<T, StepError>, u64)> {
        let mut sink = LogSink::new(self.cfg.log_limits, self.cfg.redactor.clone());
        let start = self.clock.now_ms();
        let res = {
            let mut io = StepIo {
                budget_ms: budget,
                log: &mut sink,
                ledger: &mut self.ledger,
                journal: &mut *self.journal,
                clock: self.clock,
                app: &self.d.application_id,
                dep: &self.d.id,
            };
            f(&mut *self.backend, &mut io)
        };
        let elapsed = self.clock.now_ms().saturating_sub(start);
        let cap = sink.capture();
        if cap.summary.lines_seen > 0 {
            self.logs.push((step, cap));
        }
        let res = match res {
            Err(StepError { fatal: Some(j), .. }) => return Err(EngineError::Journal(j)),
            Err(e) => Err(e),
            Ok(_) if elapsed > budget => Err(StepError::timeout(format!(
                "the step finished after {elapsed} ms, past its {budget} ms budget; its result is not trusted"
            ))),
            Ok(v) => Ok(v),
        };
        Ok((res, elapsed))
    }

    /// A check that "passed" must say so in its evidence.
    fn judge(res: Result<CheckEvidence, StepError>, accepts_simulated: bool) -> StepOut {
        match res {
            Ok(ev) if ev.simulated && !accepts_simulated => Err(StepError::new(
                "the check returned simulated evidence; a real backend cannot establish health from it",
            )
            .with_evidence(ev)),
            Ok(ev) => match ev.validate() {
                Ok(()) => Ok(Some(ev)),
                Err(why) => Err(StepError::new(format!("the check did not establish health: {why}")).with_evidence(ev)),
            },
            Err(e) => Err(e),
        }
    }

    fn describe(&mut self, step: Step, e: &StepError, elapsed: u64, budget: u64) -> String {
        let mut msg = self.cfg.redactor.apply(&e.message).0;
        if e.timed_out {
            msg = format!("timed out after {elapsed} ms (budget {budget} ms): {msg}");
        }
        if let Some(ev) = e.evidence.as_deref() {
            msg = format!("{msg} ({})", ev.summary());
            self.evidence.push((step, ev.clone()));
        }
        msg
    }

    /// Run one pre-cutover step. `Ok(true)` = continue, `Ok(false)` = the deployment ended.
    fn pre_step(
        &mut self,
        step: Step,
        kind: FailureKind,
        f: impl FnOnce(&mut dyn Backend, &mut StepIo<'_>) -> StepOut,
    ) -> R<bool> {
        if (self.cancel)() {
            return self.end_cancelled().map(|_| false);
        }
        let now = self.now();
        self.d.start_step(step, now)?;
        self.save()?; // before the side effect
        let budget = self.cfg.budgets.for_step(step);
        let first_new = self.ledger.entries().len();
        let (res, elapsed) = self.exec(step, budget, f)?;
        let res = if step == Step::PreliminaryCheck {
            // The preliminary container and its test port must be gone before the cutover begins.
            match (self.release_preliminary(first_new)?, res) {
                (Some(problem), Ok(_)) => Err(StepError::new(problem)),
                (Some(problem), Err(e)) => {
                    self.warnings.push(problem);
                    Err(e)
                }
                (None, res) => res,
            }
        } else {
            res
        };
        let now = self.now();
        match res {
            Ok(ev) => {
                if let Some(ev) = ev {
                    self.evidence.push((step, ev));
                }
                self.d.finish_step(step, true, "", now)?;
                if step == Step::Build {
                    self.d.release = Some(ReleaseId::from_deployment(&self.d.id));
                }
                self.save()?;
                Ok(true)
            }
            Err(e) => {
                let message = self.describe(step, &e, elapsed, budget);
                self.d.finish_step(step, false, &message, now)?;
                self.d.failure = Some(Failure {
                    step,
                    kind,
                    message,
                    timed_out: e.timed_out,
                });
                self.d.transition(DeploymentStatus::Failed, now)?;
                self.save()?;
                Ok(false)
            }
        }
    }

    /// Remove what the preliminary check created (entries added from `first_new` with
    /// `RemoveAlways`) — through the same plan, labels and ledger records as any cleanup.
    /// Returns a description of anything that could not be removed.
    fn release_preliminary(&mut self, first_new: usize) -> R<Option<String>> {
        let dep = self.d.id.clone();
        let targets: Vec<ResourceKey> = self.ledger.entries()[first_new..]
            .iter()
            .filter(|e| e.deployment == dep && e.disposition == Disposition::RemoveAlways)
            .map(|e| e.key.clone())
            .collect();
        if targets.is_empty() {
            return Ok(None);
        }
        let ended = [(dep.clone(), EndState::Succeeded)];
        // If the runtime cannot be listed, nothing can be confirmed absent: every object target is
        // recorded as a *failed* cleanup (never as "already absent"); leases are ledger-only.
        let (observed, list_error) = match self.backend.list_resources() {
            Ok(o) => (o, None),
            Err(e) => (Vec::new(), Some(e.message)),
        };
        let planned = plan(&self.ledger, &observed, &ended);
        let actions: Vec<Action> = planned
            .into_iter()
            .filter(|a| match a {
                Action::ReportOnly { .. } => false,
                Action::Adopt { entry, .. }
                | Action::Remove { entry, .. }
                | Action::ReleaseLease { entry }
                | Action::MarkAbsent { entry }
                | Action::Refuse { entry, .. } => targets.contains(&entry.1),
            })
            .collect();
        if let Some(why) = list_error {
            for a in &actions {
                match a {
                    Action::MarkAbsent { entry } | Action::Adopt { entry, .. } | Action::Remove { entry, .. } => {
                        attempt(&mut self.ledger, &mut *self.journal, self.clock, entry)?;
                        settle(
                            &mut self.ledger,
                            &mut *self.journal,
                            self.clock,
                            entry,
                            CleanupOutcome::Failed(format!("could not list the runtime: {why}")),
                        )?;
                    }
                    other => {
                        execute_actions(
                            std::slice::from_ref(other),
                            &mut self.ledger,
                            &mut *self.journal,
                            &mut *self.backend,
                            self.clock,
                        )?;
                    }
                }
            }
        } else {
            execute_actions(
                &actions,
                &mut self.ledger,
                &mut *self.journal,
                &mut *self.backend,
                self.clock,
            )?;
        }
        let stuck: Vec<String> = targets
            .iter()
            .filter_map(|k| self.ledger.find(&dep, k))
            .filter(|e| !e.is_released())
            .map(|e| format!("{} {}", e.key.kind.as_str(), e.key.name))
            .collect();
        Ok((!stuck.is_empty()).then(|| {
            format!(
                "could not remove the preliminary check's resources before cutover ({}); nothing was cut over",
                stuck.join(", ")
            )
        }))
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

    /// Best-effort stop of a new release that started but failed. A failure is kept as a
    /// warning in the report instead of being thrown away.
    fn stop_failed_new_release(&mut self) -> R<()> {
        let budget = self.cfg.budgets.cleanup_ms;
        let (res, _) = self.exec(Step::Cutover, budget, |b, io| b.stop_new(io))?;
        if let Err(e) = res {
            let m = self.cfg.redactor.apply(&e.message).0;
            self.warnings
                .push(format!("could not stop the failed new release: {m}"));
        }
        Ok(())
    }

    /// A failure after the cutover began: persist it, then make the one bounded
    /// restoration attempt and record the outcome.
    fn fail_after_cutover(
        &mut self,
        step: Step,
        kind: FailureKind,
        message: String,
        timed_out: bool,
        new_started: bool,
        budget_ms: u64,
    ) -> R<()> {
        let now = self.now();
        self.d.finish_step(step, false, &message, now)?;
        self.d.failure = Some(Failure {
            step,
            kind,
            message,
            timed_out,
        });
        self.d.fail_cutover(now)?;
        self.d.start_step(Step::Recovery, now)?;
        self.d.start_recovery(budget_ms, now)?;
        self.save()?; // recovery is recorded as in progress before it starts

        if new_started {
            self.stop_failed_new_release()?; // best effort: free the address for the restore
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

    /// One cutover-phase backend call. `Ok(Ok(()))` = done; `Ok(Err((message, timed_out)))` = failed.
    fn cutover_call(
        &mut self,
        f: impl FnOnce(&mut dyn Backend, &mut StepIo<'_>) -> Result<(), StepError>,
    ) -> R<Result<(), (String, bool)>> {
        let budget = self.cfg.budgets.cutover_ms;
        let (res, elapsed) = self.exec(Step::Cutover, budget, f)?;
        Ok(res.map_err(|e| (self.describe(Step::Cutover, &e, elapsed, budget), e.timed_out)))
    }

    fn final_check(&mut self) -> R<Result<(), (String, bool)>> {
        let budget = self.cfg.budgets.final_ms;
        let (res, elapsed) = self.exec(Step::FinalCheck, budget, |b, io| b.final_check(io))?;
        let res = match res {
            Ok(ev) => Run::judge(Ok(ev), self.backend.accepts_simulated_evidence()),
            Err(e) => Err(e),
        };
        Ok(match res {
            Ok(ev) => {
                if let Some(ev) = ev {
                    self.evidence.push((Step::FinalCheck, ev));
                }
                Ok(())
            }
            Err(e) => Err((self.describe(Step::FinalCheck, &e, elapsed, budget), e.timed_out)),
        })
    }

    /// After the terminal state is on disk: release what this deployment created.
    fn cleanup(&mut self) -> R<()> {
        let Some(end) = end_state(self.d.status) else {
            return Ok(());
        };
        if !self.ledger.entries().iter().any(|e| e.deployment == self.d.id) {
            return Ok(());
        }
        let ended = [(self.d.id.clone(), end)];
        let un = reconcile(
            &mut self.ledger,
            &ended,
            &mut *self.journal,
            &mut *self.backend,
            self.clock,
        )?;
        self.unaccounted.extend(un);
        Ok(())
    }
}

/// Drive one deployment from `Queued` to a terminal state with the default [`RunConfig`]
/// (apart from `policy`).
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
    let cfg = RunConfig {
        policy: *policy,
        ..RunConfig::default()
    };
    run_deployment_with(d, ctx, backend, journal, clock, &cfg, cancel)
}

/// [`run_deployment`] with explicit budgets, log limits and secrets to redact.
pub fn run_deployment_with(
    d: &mut Deployment,
    ctx: &Context,
    backend: &mut dyn Backend,
    journal: &mut dyn Journal,
    clock: &dyn Clock,
    cfg: &RunConfig,
    cancel: &dyn Fn() -> bool,
) -> Result<Report, EngineError> {
    let mut run = Run {
        d,
        backend,
        journal,
        clock,
        cancel,
        cfg,
        ledger: cfg.prior_ledger.clone(),
        evidence: Vec::new(),
        logs: Vec::new(),
        warnings: Vec::new(),
        unaccounted: Vec::new(),
    };
    let now = run.now();
    run.d.transition(DeploymentStatus::Running, now)?;
    run.save()?;

    if !run.pre_step(Step::Source, FailureKind::Source, |b, io| {
        b.prepare_source(io).map(|()| None)
    })? {
        return finish(run);
    }
    if !run.pre_step(Step::Build, FailureKind::Build, |b, io| b.build(io).map(|()| None))? {
        return finish(run);
    }
    if !run.pre_step(Step::PreliminaryCheck, FailureKind::PreliminaryCheck, |b, io| {
        Run::judge(b.preliminary_check(io), b.accepts_simulated_evidence())
    })? {
        return finish(run);
    }

    // Last cancellation point: after this the cutover is owned by the engine.
    if (run.cancel)() {
        run.end_cancelled()?;
        return finish(run);
    }

    let budget = cfg.policy.restore_budget_ms;
    match &ctx.previous_release {
        None => {
            // First deployment: nothing to stop, nothing to restore.
            run.d.cutover_not_required()?;
            let now = run.now();
            run.d.start_step(Step::Cutover, now)?;
            run.save()?;
            let started = run.cutover_call(|b, io| b.start_new(io))?;
            let now = run.now();
            if let Err((message, timed_out)) = started {
                run.d.finish_step(Step::Cutover, false, &message, now)?;
                run.d.failure = Some(Failure {
                    step: Step::Cutover,
                    kind: FailureKind::Cutover,
                    message,
                    timed_out,
                });
                run.d.transition(DeploymentStatus::Failed, now)?;
                run.save()?;
                return finish(run);
            }
            run.d.finish_step(Step::Cutover, true, "", now)?;
            run.d.start_step(Step::FinalCheck, now)?;
            run.save()?;
            let chk = run.final_check()?;
            let now = run.now();
            match chk {
                Ok(()) => {
                    run.d.finish_step(Step::FinalCheck, true, "", now)?;
                    run.succeed()?;
                }
                Err((message, timed_out)) => {
                    run.d.finish_step(Step::FinalCheck, false, &message, now)?;
                    run.d.failure = Some(Failure {
                        step: Step::FinalCheck,
                        kind: FailureKind::FinalCheck,
                        message,
                        timed_out,
                    });
                    run.stop_failed_new_release()?;
                    let now = run.now();
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

            if let Err((m, to)) = run.cutover_call(|b, io| b.stop_current(io))? {
                run.fail_after_cutover(Step::Cutover, FailureKind::Cutover, m, to, false, budget)?;
                return finish(run);
            }
            if let Err((m, to)) = run.cutover_call(|b, io| b.start_new(io))? {
                run.fail_after_cutover(Step::Cutover, FailureKind::Cutover, m, to, false, budget)?;
                return finish(run);
            }
            let now = run.now();
            run.d.finish_step(Step::Cutover, true, "", now)?;
            run.d.start_step(Step::FinalCheck, now)?;
            run.save()?;
            match run.final_check()? {
                Ok(()) => {
                    let now = run.now();
                    run.d.finish_step(Step::FinalCheck, true, "", now)?;
                    run.d.confirm_cutover(now)?;
                    run.succeed()?;
                }
                Err((m, to)) => {
                    run.fail_after_cutover(Step::FinalCheck, FailureKind::FinalCheck, m, to, true, budget)?;
                }
            }
            finish(run)
        }
    }
}

fn finish(mut run: Run<'_>) -> Result<Report, EngineError> {
    run.cleanup()?;
    let pause = match run.d.cutover {
        crate::state::CutoverState::Failed { .. } => Some(PauseReason::FailedCutover),
        _ => None,
    };
    Ok(Report {
        deployment: run.d.id.clone(),
        status: run.d.status,
        pause,
        ledger: run.ledger,
        evidence: run.evidence,
        logs: run.logs,
        warnings: run.warnings,
        unaccounted: run.unaccounted,
    })
}
