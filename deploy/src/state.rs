//! Deployment records and the state machine (spec D4, D5, D10).
//!
//! Everything here is plain data plus transition rules. Nothing performs I/O. String forms
//! (`as_str`) are provisional until the persistence schema is settled.

use crate::ids::{ApplicationId, DeploymentId, ReleaseId};
use std::fmt;

// ------------------------------------------------------------------ statuses

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeploymentStatus {
    Requested,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// The engine stopped (crash, kill, shutdown) while this deployment was in flight.
    Interrupted,
    /// A queued request replaced by a newer one (newest wins). Not a failure.
    Superseded,
}

impl DeploymentStatus {
    pub const ALL: [DeploymentStatus; 8] = [
        DeploymentStatus::Requested,
        DeploymentStatus::Queued,
        DeploymentStatus::Running,
        DeploymentStatus::Succeeded,
        DeploymentStatus::Failed,
        DeploymentStatus::Cancelled,
        DeploymentStatus::Interrupted,
        DeploymentStatus::Superseded,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DeploymentStatus::Requested => "requested",
            DeploymentStatus::Queued => "queued",
            DeploymentStatus::Running => "running",
            DeploymentStatus::Succeeded => "succeeded",
            DeploymentStatus::Failed => "failed",
            DeploymentStatus::Cancelled => "cancelled",
            DeploymentStatus::Interrupted => "interrupted",
            DeploymentStatus::Superseded => "superseded",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(
            self,
            DeploymentStatus::Requested | DeploymentStatus::Queued | DeploymentStatus::Running
        )
    }

    /// The complete transition relation. Anything not listed is rejected, including
    /// self-transitions and every move out of a terminal state.
    pub fn can_transition_to(self, to: DeploymentStatus) -> bool {
        use DeploymentStatus::*;
        matches!(
            (self, to),
            (Requested, Queued | Cancelled | Interrupted)
                | (Queued, Running | Superseded | Cancelled | Interrupted)
                | (Running, Succeeded | Failed | Cancelled | Interrupted)
        )
    }
}

impl fmt::Display for DeploymentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Manual,
    Cli,
    Poll,
    Webhook,
    Rollback,
    Promote,
}

// ------------------------------------------------------------------ steps

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    Source,
    Build,
    PreliminaryCheck,
    Cutover,
    FinalCheck,
    Recovery,
}

impl Step {
    pub const ORDER: [Step; 6] = [
        Step::Source,
        Step::Build,
        Step::PreliminaryCheck,
        Step::Cutover,
        Step::FinalCheck,
        Step::Recovery,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Step::Source => "source",
            Step::Build => "build",
            Step::PreliminaryCheck => "preliminary_check",
            Step::Cutover => "cutover",
            Step::FinalCheck => "final_check",
            Step::Recovery => "recovery",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    pub step: Step,
    pub status: StepStatus,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub note: String,
}

// ------------------------------------------------------------------ cutover and recovery

/// The cutover journal (spec D10). It is written *before* a healthy release is stopped, so
/// a crash between `Begun` and `Confirmed` is detectable on the next start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutoverState {
    NotStarted,
    /// There was no previous release to replace, so nothing was stopped.
    NotRequired,
    Begun {
        at_ms: u64,
    },
    Confirmed {
        at_ms: u64,
    },
    /// Began, and the replacement failed; recovery follows.
    Failed {
        at_ms: u64,
    },
}

/// Why a restoration was not attempted, or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NoPreviousRelease,
    ImageMissing,
    SecretUnavailable,
    DockerUnreachable,
    Timeout,
    StartFailed,
    Cancelled,
}

/// Outcome of the one bounded restoration attempt (D5). Never "guaranteed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    InProgress,
    /// The previous release started and passed its own health check.
    Restored,
    /// It started, but its check did not pass or was inconclusive.
    RestoredUnverified,
    RestoreFailed(Reason),
    NotAttempted(Reason),
}

impl RecoveryOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            RecoveryOutcome::InProgress => "in_progress",
            RecoveryOutcome::Restored => "restored",
            RecoveryOutcome::RestoredUnverified => "restored_unverified",
            RecoveryOutcome::RestoreFailed(_) => "restore_failed",
            RecoveryOutcome::NotAttempted(_) => "not_attempted",
        }
    }
    /// Is a release running again afterwards?
    pub fn service_restored(self) -> bool {
        matches!(self, RecoveryOutcome::Restored | RecoveryOutcome::RestoredUnverified)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recovery {
    pub outcome: RecoveryOutcome,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    pub budget_ms: u64,
}

/// Time during which Harpoon itself had stopped the previous release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interruption {
    pub started_ms: u64,
    /// `None` means it is still going on (the application may be down).
    pub ended_ms: Option<u64>,
}

impl Interruption {
    pub fn duration_ms(&self) -> Option<u64> {
        self.ended_ms.map(|e| e.saturating_sub(self.started_ms))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Source,
    Build,
    PreliminaryCheck,
    /// Stopping the previous release or starting the new one failed.
    Cutover,
    FinalCheck,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub step: Step,
    pub kind: FailureKind,
    pub message: String,
    /// The step exceeded its budget. The result, if one arrived late, is not trusted.
    pub timed_out: bool,
}

// ------------------------------------------------------------------ errors

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    Status {
        from: DeploymentStatus,
        to: DeploymentStatus,
    },
    Cutover {
        from: CutoverState,
        attempted: &'static str,
    },
    Step {
        step: Step,
        from: StepStatus,
        to: StepStatus,
    },
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransitionError::Status { from, to } => write!(f, "cannot move a deployment from {from} to {to}"),
            TransitionError::Cutover { from, attempted } => write!(f, "cannot {attempted} from cutover state {from:?}"),
            TransitionError::Step { step, from, to } => {
                write!(f, "step {} cannot move from {from:?} to {to:?}", step.as_str())
            }
        }
    }
}

impl std::error::Error for TransitionError {}

// ------------------------------------------------------------------ the record

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    pub id: DeploymentId,
    pub application_id: ApplicationId,
    pub trigger: Trigger,
    pub status: DeploymentStatus,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub steps: Vec<StepRecord>,
    pub cutover: CutoverState,
    pub failure: Option<Failure>,
    pub recovery: Option<Recovery>,
    pub interruption: Option<Interruption>,
    /// Set when the build succeeds; always `ReleaseId::from_deployment(&id)`.
    pub release: Option<ReleaseId>,
}

impl Deployment {
    pub fn new(id: DeploymentId, application_id: ApplicationId, trigger: Trigger, now_ms: u64) -> Deployment {
        Deployment {
            id,
            application_id,
            trigger,
            status: DeploymentStatus::Requested,
            created_ms: now_ms,
            started_ms: None,
            finished_ms: None,
            steps: Step::ORDER
                .iter()
                .map(|s| StepRecord {
                    step: *s,
                    status: StepStatus::Pending,
                    started_ms: None,
                    finished_ms: None,
                    note: String::new(),
                })
                .collect(),
            cutover: CutoverState::NotStarted,
            failure: None,
            recovery: None,
            interruption: None,
            release: None,
        }
    }

    pub fn step(&self, s: Step) -> &StepRecord {
        self.steps
            .iter()
            .find(|r| r.step == s)
            .expect("all steps are created up front")
    }

    fn step_mut(&mut self, s: Step) -> &mut StepRecord {
        self.steps
            .iter_mut()
            .find(|r| r.step == s)
            .expect("all steps are created up front")
    }

    /// Move the deployment between statuses. Entering `Running` stamps `started_ms`;
    /// entering a terminal status stamps `finished_ms` and skips any still-pending steps.
    pub fn transition(&mut self, to: DeploymentStatus, now_ms: u64) -> Result<(), TransitionError> {
        if !self.status.can_transition_to(to) {
            return Err(TransitionError::Status { from: self.status, to });
        }
        self.status = to;
        if to == DeploymentStatus::Running {
            self.started_ms = Some(now_ms);
        }
        if to.is_terminal() {
            self.finished_ms = Some(now_ms);
            for r in &mut self.steps {
                if r.status == StepStatus::Pending {
                    r.status = StepStatus::Skipped;
                }
            }
        }
        Ok(())
    }

    pub fn start_step(&mut self, s: Step, now_ms: u64) -> Result<(), TransitionError> {
        let r = self.step_mut(s);
        if r.status != StepStatus::Pending {
            return Err(TransitionError::Step {
                step: s,
                from: r.status,
                to: StepStatus::Running,
            });
        }
        r.status = StepStatus::Running;
        r.started_ms = Some(now_ms);
        Ok(())
    }

    pub fn finish_step(&mut self, s: Step, ok: bool, note: &str, now_ms: u64) -> Result<(), TransitionError> {
        let to = if ok { StepStatus::Succeeded } else { StepStatus::Failed };
        let r = self.step_mut(s);
        if r.status != StepStatus::Running {
            return Err(TransitionError::Step {
                step: s,
                from: r.status,
                to,
            });
        }
        r.status = to;
        r.finished_ms = Some(now_ms);
        r.note = note.to_string();
        Ok(())
    }

    pub fn begin_cutover(&mut self, now_ms: u64) -> Result<(), TransitionError> {
        match self.cutover {
            CutoverState::NotStarted => {
                self.cutover = CutoverState::Begun { at_ms: now_ms };
                self.interruption = Some(Interruption {
                    started_ms: now_ms,
                    ended_ms: None,
                });
                Ok(())
            }
            from => Err(TransitionError::Cutover {
                from,
                attempted: "begin cutover",
            }),
        }
    }

    pub fn cutover_not_required(&mut self) -> Result<(), TransitionError> {
        match self.cutover {
            CutoverState::NotStarted => {
                self.cutover = CutoverState::NotRequired;
                Ok(())
            }
            from => Err(TransitionError::Cutover {
                from,
                attempted: "mark cutover not required",
            }),
        }
    }

    pub fn confirm_cutover(&mut self, now_ms: u64) -> Result<(), TransitionError> {
        match self.cutover {
            CutoverState::Begun { .. } => {
                self.cutover = CutoverState::Confirmed { at_ms: now_ms };
                if let Some(i) = &mut self.interruption {
                    i.ended_ms = Some(now_ms);
                }
                Ok(())
            }
            from => Err(TransitionError::Cutover {
                from,
                attempted: "confirm cutover",
            }),
        }
    }

    pub fn fail_cutover(&mut self, now_ms: u64) -> Result<(), TransitionError> {
        match self.cutover {
            CutoverState::Begun { .. } => {
                self.cutover = CutoverState::Failed { at_ms: now_ms };
                Ok(())
            }
            from => Err(TransitionError::Cutover {
                from,
                attempted: "fail cutover",
            }),
        }
    }

    pub fn start_recovery(&mut self, budget_ms: u64, now_ms: u64) -> Result<(), TransitionError> {
        match (self.cutover, self.recovery) {
            (CutoverState::Failed { .. }, None) => {
                self.recovery = Some(Recovery {
                    outcome: RecoveryOutcome::InProgress,
                    started_ms: now_ms,
                    finished_ms: None,
                    budget_ms,
                });
                Ok(())
            }
            (from, _) => Err(TransitionError::Cutover {
                from,
                attempted: "start recovery",
            }),
        }
    }

    /// Record the final recovery outcome. If service is back, the interruption ends now.
    pub fn finish_recovery(&mut self, outcome: RecoveryOutcome, now_ms: u64) -> Result<(), TransitionError> {
        match &mut self.recovery {
            Some(r) if r.outcome == RecoveryOutcome::InProgress && outcome != RecoveryOutcome::InProgress => {
                r.outcome = outcome;
                r.finished_ms = Some(now_ms);
                if outcome.service_restored() {
                    if let Some(i) = &mut self.interruption {
                        i.ended_ms = Some(now_ms);
                    }
                }
                Ok(())
            }
            _ => Err(TransitionError::Cutover {
                from: self.cutover,
                attempted: "finish recovery",
            }),
        }
    }

    /// Could the application be down because of this deployment? True when Harpoon began a
    /// cutover and no running release is confirmed afterwards.
    pub fn application_may_be_down(&self) -> bool {
        match self.cutover {
            CutoverState::Begun { .. } => true,
            CutoverState::Failed { .. } => !self.recovery.map(|r| r.outcome.service_restored()).unwrap_or(false),
            _ => false,
        }
    }
}

// ------------------------------------------------------------------ startup recovery

/// What [`recover_on_start`] found. It never resumes or repairs anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupReport {
    /// Deployments newly marked `Interrupted`.
    pub interrupted: Vec<DeploymentId>,
    /// Of those, the ones where a cutover began and no healthy release was confirmed:
    /// the application may be down and needs an explicit, developer-run recovery.
    pub incomplete_cutovers: Vec<DeploymentId>,
}

/// Mark every non-terminal deployment `Interrupted` (spec D10 — "running or queued"; a
/// `Requested` one that never reached the queue is treated the same, which is the safer
/// reading). Idempotent: terminal deployments are untouched, so a second call is a no-op.
pub fn recover_on_start(deployments: &mut [Deployment], now_ms: u64) -> StartupReport {
    let mut report = StartupReport::default();
    for d in deployments.iter_mut() {
        if d.status.is_terminal() {
            continue;
        }
        for r in &mut d.steps {
            if r.status == StepStatus::Running {
                r.status = StepStatus::Interrupted;
                r.finished_ms = Some(now_ms);
            }
        }
        d.transition(DeploymentStatus::Interrupted, now_ms)
            .expect("every non-terminal status may be interrupted");
        report.interrupted.push(d.id.clone());
        if d.application_may_be_down() {
            report.incomplete_cutovers.push(d.id.clone());
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use DeploymentStatus::*;

    #[test]
    fn transition_table_is_exactly_the_documented_relation() {
        let allowed: &[(DeploymentStatus, DeploymentStatus)] = &[
            (Requested, Queued),
            (Requested, Cancelled),
            (Requested, Interrupted),
            (Queued, Running),
            (Queued, Superseded),
            (Queued, Cancelled),
            (Queued, Interrupted),
            (Running, Succeeded),
            (Running, Failed),
            (Running, Cancelled),
            (Running, Interrupted),
        ];
        for from in DeploymentStatus::ALL {
            for to in DeploymentStatus::ALL {
                let expect = allowed.contains(&(from, to));
                assert_eq!(from.can_transition_to(to), expect, "{from} -> {to}");
            }
        }
    }

    #[test]
    fn terminal_states_have_no_exits_and_no_self_transitions() {
        for s in DeploymentStatus::ALL {
            assert!(!s.can_transition_to(s), "{s} -> {s}");
            if s.is_terminal() {
                assert!(
                    DeploymentStatus::ALL.iter().all(|t| !s.can_transition_to(*t)),
                    "{s} has an exit"
                );
            }
        }
    }

    #[test]
    fn status_strings_are_unique() {
        let set: std::collections::HashSet<_> = DeploymentStatus::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(set.len(), 8);
    }

    fn dep_n(seed: u8) -> Deployment {
        use crate::ids::IdGenerator;
        use crate::time::{ManualClock, SeqEntropy};
        let mut g = IdGenerator::new(ManualClock::new(10), SeqEntropy::new(seed));
        Deployment::new(g.deployment().unwrap(), g.application().unwrap(), Trigger::Manual, 10)
    }

    fn dep() -> Deployment {
        dep_n(3)
    }

    #[test]
    fn terminal_transition_stamps_time_and_skips_pending_steps() {
        let mut d = dep();
        d.transition(Queued, 11).unwrap();
        d.transition(Running, 12).unwrap();
        assert_eq!(d.started_ms, Some(12));
        d.start_step(Step::Source, 12).unwrap();
        d.finish_step(Step::Source, false, "no", 13).unwrap();
        d.transition(Failed, 14).unwrap();
        assert_eq!(d.finished_ms, Some(14));
        assert_eq!(d.step(Step::Build).status, StepStatus::Skipped);
        assert_eq!(d.step(Step::Source).status, StepStatus::Failed);
        assert!(d.transition(Running, 15).is_err(), "terminal deployments never restart");
    }

    #[test]
    fn steps_cannot_be_started_twice_or_finished_unstarted() {
        let mut d = dep();
        assert!(d.finish_step(Step::Build, true, "", 1).is_err());
        d.start_step(Step::Build, 1).unwrap();
        assert!(d.start_step(Step::Build, 2).is_err());
        d.finish_step(Step::Build, true, "ok", 3).unwrap();
        assert!(d.finish_step(Step::Build, true, "again", 4).is_err());
    }

    #[test]
    fn cutover_journal_allows_only_valid_sequences() {
        let mut d = dep();
        assert!(d.confirm_cutover(1).is_err(), "cannot confirm before beginning");
        d.begin_cutover(5).unwrap();
        assert!(d.begin_cutover(6).is_err());
        assert!(d.cutover_not_required().is_err());
        assert!(d.application_may_be_down());
        d.confirm_cutover(9).unwrap();
        assert_eq!(d.interruption.unwrap().duration_ms(), Some(4));
        assert!(!d.application_may_be_down());
        assert!(d.fail_cutover(10).is_err(), "confirmed cutover cannot then fail");
    }

    #[test]
    fn recovery_requires_a_failed_cutover_and_runs_once() {
        let mut d = dep();
        assert!(d.start_recovery(60_000, 1).is_err(), "no recovery without a cutover");
        d.begin_cutover(10).unwrap();
        assert!(
            d.start_recovery(60_000, 11).is_err(),
            "no recovery before the cutover fails"
        );
        d.fail_cutover(12).unwrap();
        d.start_recovery(60_000, 12).unwrap();
        assert!(d.start_recovery(60_000, 13).is_err(), "only one attempt");
        assert!(d.finish_recovery(RecoveryOutcome::InProgress, 14).is_err());
        d.finish_recovery(RecoveryOutcome::Restored, 20).unwrap();
        assert!(
            d.finish_recovery(RecoveryOutcome::Restored, 21).is_err(),
            "outcome is final"
        );
        assert_eq!(d.interruption.unwrap().duration_ms(), Some(10));
        assert!(!d.application_may_be_down());
    }

    #[test]
    fn failed_restore_leaves_the_interruption_open_and_the_app_possibly_down() {
        let mut d = dep();
        d.begin_cutover(10).unwrap();
        d.fail_cutover(12).unwrap();
        d.start_recovery(60_000, 12).unwrap();
        d.finish_recovery(RecoveryOutcome::RestoreFailed(Reason::SecretUnavailable), 14)
            .unwrap();
        assert_eq!(d.interruption.unwrap().ended_ms, None);
        assert!(d.application_may_be_down());
    }

    #[test]
    fn startup_marks_every_non_terminal_deployment_interrupted_and_is_idempotent() {
        let mut ds: Vec<Deployment> = (0..4u8).map(|i| dep_n(10 + i * 20)).collect();
        ds[1].transition(Queued, 1).unwrap();
        ds[2].transition(Queued, 1).unwrap();
        ds[2].transition(Running, 2).unwrap();
        ds[2].start_step(Step::Build, 2).unwrap();
        ds[3].transition(Queued, 1).unwrap();
        ds[3].transition(Running, 2).unwrap();
        ds[3].transition(Succeeded, 3).unwrap();
        let r = recover_on_start(&mut ds, 100);
        assert_eq!(r.interrupted.len(), 3);
        assert_eq!(ds[3].status, Succeeded, "terminal deployments are untouched");
        assert_eq!(ds[2].step(Step::Build).status, StepStatus::Interrupted);
        assert_eq!(ds[0].status, Interrupted);
        assert_eq!(recover_on_start(&mut ds, 200), StartupReport::default());
    }

    #[test]
    fn startup_flags_an_incomplete_cutover_but_not_other_interruptions() {
        let mut a = dep();
        a.transition(Queued, 1).unwrap();
        a.transition(Running, 2).unwrap();
        a.begin_cutover(3).unwrap();
        let mut b = dep_n(100);
        assert_ne!(a.id, b.id);
        b.transition(Queued, 1).unwrap();
        b.transition(Running, 2).unwrap();
        let (aid, bid) = (a.id.clone(), b.id.clone());
        let mut ds = vec![a, b];
        let r = recover_on_start(&mut ds, 9);
        assert_eq!(r.interrupted.len(), 2);
        assert_eq!(r.incomplete_cutovers, vec![aid]);
        assert!(!r.incomplete_cutovers.contains(&bid));
    }
}
