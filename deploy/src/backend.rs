//! The execution seam for the **local** Harpoon runtime, plus a scripted fake.
//!
//! The real backend (Docker CLI/API over Harpoon's private socket) does not exist yet and
//! is not part of this foundation. This trait carries only what the lifecycle in the spec
//! needs; it is *not* a provider abstraction, and nothing here assumes how a build,
//! container or health probe is actually performed.

use crate::state::{CutoverState, DeploymentStatus, Reason, Step};
use crate::time::ManualClock;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// What happened, in order, across both the journal and the backend. Tests use it to
/// prove that every side effect was preceded by a write describing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A record was persisted: its status, the step that was `Running` (if any), and
    /// the cutover journal at that moment.
    Persist {
        status: DeploymentStatus,
        running: Option<Step>,
        cutover: CutoverState,
    },
    Call(Call),
}

pub type Trace = Rc<RefCell<Vec<Event>>>;

/// A step failed. `message` is shown to the developer; it never carries secret values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepError {
    pub message: String,
}

impl StepError {
    pub fn new(message: impl Into<String>) -> StepError {
        StepError {
            message: message.into(),
        }
    }
}

/// Result of the one bounded restoration attempt, as reported by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreResult {
    /// Started and passed its own health check.
    Healthy,
    /// Started, but its check failed or was inconclusive.
    StartedUnverified,
    /// Tried and failed.
    Failed(Reason),
    /// Could not even try (for example the retained image is gone).
    NotAttempted(Reason),
}

pub trait Backend {
    /// Resolve the source (clone, copy, or confirm the image exists).
    fn prepare_source(&mut self) -> Result<(), StepError>;
    /// Build the new image without touching the running release.
    fn build(&mut self) -> Result<(), StepError>;
    /// Check the new release while the old one is still running (nothing stopped yet).
    fn preliminary_check(&mut self) -> Result<(), StepError>;
    /// Stop the previous, healthy release. After this, Harpoon owns the interruption.
    fn stop_current(&mut self) -> Result<(), StepError>;
    /// Start the new release on its real address.
    fn start_new(&mut self) -> Result<(), StepError>;
    /// Check the new release after the switch.
    fn final_check(&mut self) -> Result<(), StepError>;
    /// Stop a new release that started but failed, to free its address before restoring.
    fn stop_new(&mut self) -> Result<(), StepError>;
    /// One bounded attempt to bring the previous release back. `budget_ms` is a limit the
    /// backend must respect; it must not retry.
    fn restore_previous(&mut self, budget_ms: u64) -> RestoreResult;
}

// ------------------------------------------------------------------ the fake

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    PrepareSource,
    Build,
    PreliminaryCheck,
    StopCurrent,
    StartNew,
    FinalCheck,
    StopNew,
    RestorePrevious { budget_ms: u64 },
}

#[derive(Debug, Clone)]
struct Scripted {
    result: Result<(), StepError>,
    takes_ms: u64,
}

impl Scripted {
    fn ok(takes_ms: u64) -> Scripted {
        Scripted {
            result: Ok(()),
            takes_ms,
        }
    }
}

/// Deterministic backend: every operation succeeds after a fixed time unless a failure is
/// scripted, calls are recorded in order, and time only advances through the shared
/// [`ManualClock`]. No process, file, network or Docker access of any kind.
pub struct FakeBackend {
    clock: Rc<ManualClock>,
    source: Scripted,
    build: Scripted,
    preliminary: Scripted,
    stop_current: Scripted,
    start_new: Scripted,
    final_check: Scripted,
    stop_new: Scripted,
    restore: VecDeque<(RestoreResult, u64)>,
    trace: Option<Trace>,
    pub calls: Vec<Call>,
}

impl FakeBackend {
    pub fn new(clock: Rc<ManualClock>) -> FakeBackend {
        FakeBackend {
            clock,
            source: Scripted::ok(10),
            build: Scripted::ok(1_000),
            preliminary: Scripted::ok(100),
            stop_current: Scripted::ok(500),
            start_new: Scripted::ok(300),
            final_check: Scripted::ok(200),
            stop_new: Scripted::ok(100),
            restore: VecDeque::from([(RestoreResult::Healthy, 400)]),
            trace: None,
            calls: Vec::new(),
        }
    }

    /// Also record every call into a shared trace (see [`Event`]).
    pub fn with_trace(mut self, t: Trace) -> Self {
        self.trace = Some(t);
        self
    }

    fn note(&mut self, c: &Call) {
        if let Some(t) = &self.trace {
            t.borrow_mut().push(Event::Call(c.clone()));
        }
    }

    pub fn fail_source(mut self, msg: &str) -> Self {
        self.source.result = Err(StepError::new(msg));
        self
    }
    pub fn fail_build(mut self, msg: &str) -> Self {
        self.build.result = Err(StepError::new(msg));
        self
    }
    pub fn fail_preliminary(mut self, msg: &str) -> Self {
        self.preliminary.result = Err(StepError::new(msg));
        self
    }
    pub fn fail_stop_current(mut self, msg: &str) -> Self {
        self.stop_current.result = Err(StepError::new(msg));
        self
    }
    pub fn fail_start_new(mut self, msg: &str) -> Self {
        self.start_new.result = Err(StepError::new(msg));
        self
    }
    pub fn fail_final_check(mut self, msg: &str) -> Self {
        self.final_check.result = Err(StepError::new(msg));
        self
    }
    pub fn restore_with(mut self, result: RestoreResult, takes_ms: u64) -> Self {
        self.restore = VecDeque::from([(result, takes_ms)]);
        self
    }
    pub fn final_check_takes(mut self, ms: u64) -> Self {
        self.final_check.takes_ms = ms;
        self
    }

    fn run(&mut self, call: Call, pick: fn(&FakeBackend) -> Scripted) -> Result<(), StepError> {
        let s = pick(self);
        self.note(&call);
        self.calls.push(call);
        self.clock.advance(s.takes_ms);
        s.result
    }

    pub fn called(&self, c: &Call) -> bool {
        self.calls.contains(c)
    }
}

impl Backend for FakeBackend {
    fn prepare_source(&mut self) -> Result<(), StepError> {
        self.run(Call::PrepareSource, |b| b.source.clone())
    }
    fn build(&mut self) -> Result<(), StepError> {
        self.run(Call::Build, |b| b.build.clone())
    }
    fn preliminary_check(&mut self) -> Result<(), StepError> {
        self.run(Call::PreliminaryCheck, |b| b.preliminary.clone())
    }
    fn stop_current(&mut self) -> Result<(), StepError> {
        self.run(Call::StopCurrent, |b| b.stop_current.clone())
    }
    fn start_new(&mut self) -> Result<(), StepError> {
        self.run(Call::StartNew, |b| b.start_new.clone())
    }
    fn final_check(&mut self) -> Result<(), StepError> {
        self.run(Call::FinalCheck, |b| b.final_check.clone())
    }
    fn stop_new(&mut self) -> Result<(), StepError> {
        self.run(Call::StopNew, |b| b.stop_new.clone())
    }
    fn restore_previous(&mut self, budget_ms: u64) -> RestoreResult {
        let c = Call::RestorePrevious { budget_ms };
        self.note(&c);
        self.calls.push(c);
        let (r, takes) = self
            .restore
            .pop_front()
            .unwrap_or((RestoreResult::NotAttempted(Reason::ImageMissing), 0));
        self.clock.advance(takes);
        r
    }
}
