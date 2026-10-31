//! The execution seam for the **local** Harpoon runtime, plus a scripted fake.
//!
//! The real backend (Docker CLI/API over Harpoon's private socket) does not exist yet and
//! is not part of this foundation. This trait carries only what the lifecycle in the spec
//! needs; it is *not* a provider abstraction, and nothing here assumes how a build,
//! container or health probe is actually performed.

use crate::engine::{JournalError, StepIo};
use crate::health::{CheckEvidence, HealthSpec};
use crate::ids::{labels, ApplicationId, DeploymentId, ReleaseId};
use crate::ledger::{Disposition, LedgerRecord, Observed, RemoveRequest, RemoveResult, ResourceKey, ResourceKind};
use crate::ports::{self, Occupancy, Purpose};
use crate::release::ReleaseConfig;
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

/// The resource-level counterpart of [`Event`], kept separate so the original trace type
/// stays exactly as it was. Used to prove "intent is durable before the object exists".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// A ledger record reached the journal.
    Write(LedgerRecord),
    /// The (fake) runtime created an object.
    Created {
        kind: ResourceKind,
        name: String,
        id: String,
    },
    /// The (fake) runtime removed an object.
    Removed { kind: ResourceKind, id: String },
}

pub type OpTrace = Rc<RefCell<Vec<Op>>>;

/// Calls made through [`ResourceOps`], recorded by the fake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceCall {
    List,
    Remove { kind: ResourceKind, id: String },
}

pub type Trace = Rc<RefCell<Vec<Event>>>;

/// A step failed. `message` is shown to the developer; the engine redacts it before it is
/// recorded, but a backend must still never put secret values in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepError {
    pub message: String,
    /// The backend gave up because the deadline in [`StepIo::budget_ms`] passed.
    pub timed_out: bool,
    /// For a failed health check: what was actually observed.
    pub evidence: Option<Box<CheckEvidence>>,
    /// A journal write failed inside the step. This is not a step failure: the engine stops
    /// and returns the error, exactly as if the process had died.
    pub fatal: Option<JournalError>,
}

impl StepError {
    pub fn new(message: impl Into<String>) -> StepError {
        StepError {
            message: message.into(),
            timed_out: false,
            evidence: None,
            fatal: None,
        }
    }
    pub fn timeout(message: impl Into<String>) -> StepError {
        StepError {
            timed_out: true,
            ..StepError::new(message)
        }
    }
    pub fn with_evidence(mut self, e: CheckEvidence) -> StepError {
        self.evidence = Some(Box::new(e));
        self
    }
}

impl From<JournalError> for StepError {
    fn from(e: JournalError) -> StepError {
        StepError {
            fatal: Some(e),
            ..StepError::new("journal write failed")
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

/// The release a cutover replaces, as the engine knows it. `config` is the pinned configuration
/// stored when that release started serving; `None` means it was never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousRelease {
    pub release: ReleaseId,
    /// The deployment that built the release (its image carries this deployment's labels).
    pub deployment: DeploymentId,
    pub config: Option<ReleaseConfig>,
    /// The container that actually serves it, when that is not simply the release's own (after a
    /// restoration it belongs to the deployment that restored it). `None` = the release's own.
    pub container: Option<ContainerRef>,
}

/// A recorded container: which deployment's ledger holds it, and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRef {
    pub owner: DeploymentId,
    pub name: String,
}

/// What the engine needs to look at and remove runtime objects. Separate from [`Backend`] so
/// startup recovery can use it without a deployment in flight.
pub trait ResourceOps {
    /// Everything the runtime reports, with its labels. Ownership is decided from these labels only.
    fn list_resources(&mut self) -> Result<Vec<Observed>, StepError>;
    /// Remove one object. The implementation must **re-verify** the labels in `req` at removal
    /// time and answer `Refused` if they no longer match; it must never remove by name pattern.
    fn remove_resource(&mut self, req: &RemoveRequest) -> Result<RemoveResult, StepError>;
}

pub trait Backend: ResourceOps {
    /// Resolve the source (clone, copy, or confirm the image exists).
    fn prepare_source(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError>;
    /// Build the new image without touching the running release.
    fn build(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError>;
    /// Check the new release while the old one is still running (nothing stopped yet).
    fn preliminary_check(&mut self, io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError>;
    /// Stop the previous, healthy release. After this, Harpoon owns the interruption.
    fn stop_current(&mut self, io: &mut StepIo<'_>, previous: &PreviousRelease) -> Result<(), StepError>;
    /// Decide, **before anything is created or stopped**, the configuration the new release will
    /// serve with (reserving its port lease through the ledger) and return it so the engine can
    /// pin it durably first. `start_new` then uses exactly this plan. `None` = the backend cannot
    /// plan, and the release is not pinned.
    fn plan_release(&mut self, _io: &mut StepIo<'_>) -> Result<Option<ReleaseConfig>, StepError> {
        Ok(None)
    }
    /// The configuration the release actually started with. Compared with the plan after
    /// `start_new`; a difference (a port collision moved it) is pinned again at once.
    fn pinned_config(&self) -> Option<ReleaseConfig> {
        None
    }
    /// Problems that make a restoration unsafe to start, found by reading only: for example a
    /// running container that already publishes the previous release's port. Nothing is created,
    /// stopped or removed to answer this. An empty list means nothing was found.
    fn restore_blockers(
        &mut self,
        _io: &mut StepIo<'_>,
        _previous: &PreviousRelease,
    ) -> Result<Vec<String>, StepError> {
        Ok(Vec::new())
    }
    /// Start the new release on its real address.
    fn start_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError>;
    /// Check the new release after the switch.
    fn final_check(&mut self, io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError>;
    /// Stop a new release that started but failed, to free its address before restoring.
    fn stop_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError>;
    /// Whether passing evidence marked `simulated` may establish health. Only fakes say yes; a
    /// real backend must keep the default of `false` for the engine to enforce.
    fn accepts_simulated_evidence(&self) -> bool {
        false
    }
    /// Whether this backend can replace a running release (stop it, start the new one, and make
    /// the bounded restoration attempt). A backend that cannot must say so: the engine then
    /// prepares the candidate (build and preliminary check) and stops before anything is
    /// stopped, instead of beginning a cutover it cannot complete or undo.
    fn supports_cutover(&self) -> bool {
        true
    }
    /// One bounded attempt to bring the previous release back from its retained image and
    /// `previous.config` (never `None` here: the engine reports that case itself). `budget_ms` is
    /// a limit the backend must respect; it must not retry. Every object it creates goes through
    /// the ledger protocol on `io`. An `Err` with a journal failure aborts the run; any other `Err`
    /// is a failed restoration.
    fn restore_previous(
        &mut self,
        io: &mut StepIo<'_>,
        previous: &PreviousRelease,
        budget_ms: u64,
    ) -> Result<RestoreResult, StepError>;
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

// ------------------------------------------------------------------ the fake runtime

/// A pretend container runtime: just a list of labelled objects, plus the occupancy a port
/// allocator would see. Shared (`Rc`) so a test can add bystanders before a run and inspect
/// exactly what survived afterwards.
#[derive(Debug, Default)]
pub struct FakeRuntime {
    pub objects: Vec<Observed>,
    pub listeners: std::collections::BTreeSet<u16>,
    pub mappings: std::collections::BTreeSet<u16>,
    next_id: u32,
}

pub type World = Rc<RefCell<FakeRuntime>>;

impl FakeRuntime {
    pub fn world() -> World {
        Rc::new(RefCell::new(FakeRuntime::default()))
    }

    pub fn create(&mut self, kind: ResourceKind, name: &str, labels: Vec<(String, String)>) -> String {
        self.next_id += 1;
        let id = format!("fake-{:04}", self.next_id);
        self.objects.push(Observed {
            kind,
            name: name.to_string(),
            id: id.clone(),
            labels,
        });
        id
    }

    /// An object the engine did not create: another application, or no Harpoon labels at all.
    pub fn add_foreign(&mut self, kind: ResourceKind, name: &str, labels: Vec<(String, String)>) -> String {
        self.create(kind, name, labels)
    }

    pub fn names(&self) -> Vec<String> {
        self.objects.iter().map(|o| o.name.clone()).collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.objects.iter().any(|o| o.name == name)
    }
}

pub fn harpoon_labels(app: &ApplicationId, dep: &DeploymentId) -> Vec<(String, String)> {
    labels::for_deployment(app, dep)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

/// Deterministic backend: every operation succeeds after a fixed time unless a failure is
/// scripted, calls are recorded in order, and time only advances through the shared
/// [`ManualClock`]. No process, file, network or Docker access of any kind.
///
/// With [`FakeBackend::with_runtime`] it also *creates* resources through the ledger
/// protocol (intent → create → receipt) in a [`FakeRuntime`].
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
    pub resource_calls: Vec<ResourceCall>,
    ops: Option<OpTrace>,
    world: Option<World>,
    create_fails: Option<ResourceKind>,
    remove_fails: Vec<String>,
    remove_refuses: Vec<String>,
    list_fails: bool,
    final_evidence: Option<CheckEvidence>,
    cutover_supported: bool,
    /// What the last restoration attempt was told to restore.
    pub restored: Option<PreviousRelease>,
    restore_error: Option<StepError>,
    blockers: Vec<String>,
    plan_error: Option<StepError>,
    rollback: Option<ReleaseConfig>,
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
            resource_calls: Vec::new(),
            ops: None,
            world: None,
            create_fails: None,
            remove_fails: Vec::new(),
            remove_refuses: Vec::new(),
            list_fails: false,
            final_evidence: None,
            cutover_supported: true,
            restored: None,
            restore_error: None,
            blockers: Vec::new(),
            plan_error: None,
            rollback: None,
        }
    }

    /// Also record every call into a shared trace (see [`Event`]).
    pub fn with_trace(mut self, t: Trace) -> Self {
        self.trace = Some(t);
        self
    }

    /// Record creations and removals into a shared [`OpTrace`] (the journal can write the same one).
    pub fn with_ops_trace(mut self, t: OpTrace) -> Self {
        self.ops = Some(t);
        self
    }

    /// Create image/container/port resources in `world` through the ledger protocol.
    pub fn with_runtime(mut self, world: World) -> Self {
        self.world = Some(world);
        self
    }
    /// The create call for this kind fails *after* the intent was recorded.
    pub fn fail_create(mut self, kind: ResourceKind) -> Self {
        self.create_fails = Some(kind);
        self
    }
    /// Removing the object with this name fails (it stays in the runtime).
    pub fn fail_remove(mut self, name: &str) -> Self {
        self.remove_fails.push(name.to_string());
        self
    }
    /// Removing the object with this name is refused because its labels changed.
    pub fn refuse_remove(mut self, name: &str) -> Self {
        self.remove_refuses.push(name.to_string());
        self
    }
    /// The backend cannot replace a running release (like the Docker backend today).
    pub fn without_cutover(mut self) -> Self {
        self.cutover_supported = false;
        self
    }
    pub fn fail_list(mut self) -> Self {
        self.list_fails = true;
        self
    }
    /// The final check "succeeds" but reports this evidence (used to prove the engine does
    /// not accept a pass whose evidence says otherwise).
    pub fn final_check_reports(mut self, e: CheckEvidence) -> Self {
        self.final_evidence = Some(e);
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
    pub fn fail_stop_new(mut self, msg: &str) -> Self {
        self.stop_new.result = Err(StepError::new(msg));
        self
    }
    pub fn restore_with(mut self, result: RestoreResult, takes_ms: u64) -> Self {
        self.restore = VecDeque::from([(result, takes_ms)]);
        self
    }
    /// The restoration call itself fails (for example the budget ran out inside the backend).
    pub fn restore_errors(mut self, e: StepError, takes_ms: u64) -> Self {
        self.restore_error = Some(e);
        self.restore = VecDeque::from([(RestoreResult::Healthy, takes_ms)]);
        self
    }
    /// A restoration would find these problems (for example an occupied port) before creating anything.
    pub fn restore_blocked_by(mut self, problems: &[&str]) -> Self {
        self.blockers = problems.iter().map(|p| p.to_string()).collect();
        self
    }
    /// A rollback to the release pinned as `target`: nothing is built, no image is created, and the
    /// release serves with the target's pinned configuration (only the port is chosen afresh).
    pub fn with_rollback(mut self, target: ReleaseConfig) -> Self {
        self.rollback = Some(target);
        self
    }
    /// Deciding the new release's configuration fails, before anything is created or stopped.
    pub fn fail_plan(mut self, msg: &str) -> Self {
        self.plan_error = Some(StepError::new(msg));
        self
    }
    pub fn final_check_takes(mut self, ms: u64) -> Self {
        self.final_check.takes_ms = ms;
        self
    }
    pub fn build_takes(mut self, ms: u64) -> Self {
        self.build.takes_ms = ms;
        self
    }
    pub fn preliminary_takes(mut self, ms: u64) -> Self {
        self.preliminary.takes_ms = ms;
        self
    }

    fn run(&mut self, call: Call, pick: fn(&FakeBackend) -> Scripted) -> Result<(), StepError> {
        let s = pick(self);
        self.note(&call);
        self.calls.push(call);
        self.clock.advance(s.takes_ms);
        s.result
    }

    /// intent → create → receipt, the only order the ledger accepts.
    fn create(
        &mut self,
        io: &mut StepIo<'_>,
        kind: ResourceKind,
        name: &str,
        disposition: Disposition,
    ) -> Result<(), StepError> {
        let Some(world) = self.world.clone() else { return Ok(()) };
        let intent = io.intend(ResourceKey::new(kind, name), disposition)?;
        if self.create_fails == Some(kind) {
            return Err(StepError::new(format!("could not create {name}")));
        }
        // Like a real runtime, refuse a name that is already taken (by anyone).
        if world.borrow().objects.iter().any(|o| o.kind == kind && o.name == name) {
            return Err(StepError::new(format!("name {name} is already in use")));
        }
        let id = world
            .borrow_mut()
            .create(kind, name, harpoon_labels(io.app(), io.deployment()));
        if let Some(o) = &self.ops {
            o.borrow_mut().push(Op::Created {
                kind,
                name: name.to_string(),
                id: id.clone(),
            });
        }
        io.receipt(&intent, &id)?;
        Ok(())
    }

    pub fn called(&self, c: &Call) -> bool {
        self.calls.contains(c)
    }
}

impl ResourceOps for FakeBackend {
    fn list_resources(&mut self) -> Result<Vec<Observed>, StepError> {
        self.resource_calls.push(ResourceCall::List);
        if self.list_fails {
            return Err(StepError::new("runtime unreachable"));
        }
        Ok(self
            .world
            .as_ref()
            .map(|w| w.borrow().objects.clone())
            .unwrap_or_default())
    }

    fn remove_resource(&mut self, req: &RemoveRequest) -> Result<RemoveResult, StepError> {
        self.resource_calls.push(ResourceCall::Remove {
            kind: req.kind,
            id: req.id.clone(),
        });
        if self.remove_fails.contains(&req.name) {
            return Err(StepError::new(format!("could not remove {}", req.name)));
        }
        if self.remove_refuses.contains(&req.name) {
            return Ok(RemoveResult::Refused("labels changed since listing".to_string()));
        }
        let Some(world) = &self.world else {
            return Ok(RemoveResult::AlreadyAbsent);
        };
        let mut w = world.borrow_mut();
        let Some(pos) = w.objects.iter().position(|o| o.kind == req.kind && o.id == req.id) else {
            return Ok(RemoveResult::AlreadyAbsent);
        };
        // Re-verify ownership at removal time, as the trait requires.
        if !w.objects[pos].labelled_for(&req.app, &req.deployment) {
            return Ok(RemoveResult::Refused(
                "object is not labelled for this deployment".to_string(),
            ));
        }
        w.objects.remove(pos);
        if let Some(o) = &self.ops {
            o.borrow_mut().push(Op::Removed {
                kind: req.kind,
                id: req.id.clone(),
            });
        }
        Ok(RemoveResult::Removed)
    }
}

impl Backend for FakeBackend {
    fn supports_cutover(&self) -> bool {
        self.cutover_supported
    }
    fn accepts_simulated_evidence(&self) -> bool {
        true
    }
    fn prepare_source(&mut self, _io: &mut StepIo<'_>) -> Result<(), StepError> {
        self.run(Call::PrepareSource, |b| b.source.clone())
    }
    fn build(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        let r = self.run(Call::Build, |b| b.build.clone());
        io.log.push_line("fake build output");
        r?;
        if self.rollback.is_some() {
            return Ok(());
        }
        let tag = format!("harpoon/{}:{}", io.app(), io.deployment());
        self.create(io, ResourceKind::Image, &tag, Disposition::Retain)
    }
    fn preliminary_check(&mut self, io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError> {
        let name = format!("hp-{}-pre", io.deployment());
        self.create(io, ResourceKind::Container, &name, Disposition::RemoveAlways)?;
        self.run(Call::PreliminaryCheck, |b| b.preliminary.clone())?;
        Ok(CheckEvidence::simulated_passing(
            "fake preliminary check",
            &HealthSpec::first_test("/"),
        ))
    }
    fn stop_current(&mut self, _io: &mut StepIo<'_>, _previous: &PreviousRelease) -> Result<(), StepError> {
        self.run(Call::StopCurrent, |b| b.stop_current.clone())
    }
    fn plan_release(&mut self, _io: &mut StepIo<'_>) -> Result<Option<ReleaseConfig>, StepError> {
        if let Some(e) = self.plan_error.clone() {
            return Err(e);
        }
        Ok(self.pinned_config())
    }
    fn pinned_config(&self) -> Option<ReleaseConfig> {
        Some(match &self.rollback {
            Some(t) => ReleaseConfig {
                host_port: 39000,
                ..t.clone()
            },
            None => ReleaseConfig::simulated(39000),
        })
    }
    fn restore_blockers(
        &mut self,
        _io: &mut StepIo<'_>,
        _previous: &PreviousRelease,
    ) -> Result<Vec<String>, StepError> {
        Ok(self.blockers.clone())
    }
    fn start_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        if let Some(world) = self.world.clone() {
            let occ = {
                let w = world.borrow();
                Occupancy {
                    host_listeners: w.listeners.clone(),
                    docker_mappings: w.mappings.clone(),
                    ..Occupancy::default()
                }
                .with_ledger(io.ledger())
            };
            let lease =
                ports::allocate(Purpose::Test, 8080, &occ, None, &[]).map_err(|e| StepError::new(e.to_string()))?;
            io.lease_port(lease)?;
        }
        let name = format!("hp-{}", io.deployment());
        self.create(io, ResourceKind::Container, &name, Disposition::RemoveOnFailure)?;
        self.run(Call::StartNew, |b| b.start_new.clone())
    }
    fn final_check(&mut self, _io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError> {
        self.run(Call::FinalCheck, |b| b.final_check.clone())?;
        Ok(self
            .final_evidence
            .clone()
            .unwrap_or_else(|| CheckEvidence::simulated_passing("fake final check", &HealthSpec::first_test("/"))))
    }
    fn stop_new(&mut self, _io: &mut StepIo<'_>) -> Result<(), StepError> {
        self.run(Call::StopNew, |b| b.stop_new.clone())
    }
    fn restore_previous(
        &mut self,
        io: &mut StepIo<'_>,
        previous: &PreviousRelease,
        budget_ms: u64,
    ) -> Result<RestoreResult, StepError> {
        let c = Call::RestorePrevious { budget_ms };
        self.note(&c);
        self.calls.push(c);
        self.restored = Some(previous.clone());
        let (r, takes) = self
            .restore
            .pop_front()
            .unwrap_or((RestoreResult::NotAttempted(Reason::ImageMissing), 0));
        self.clock.advance(takes);
        if matches!(r, RestoreResult::Healthy | RestoreResult::StartedUnverified) && self.restore_error.is_none() {
            // A restoration that started something has created a new, retained container.
            let name = crate::serving::restored_container_name(io.deployment());
            self.create(io, ResourceKind::Container, &name, Disposition::Retain)?;
        }
        match self.restore_error.take() {
            Some(e) => Err(e),
            None => Ok(r),
        }
    }
}
