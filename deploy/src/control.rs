//! Durable pauses, held events and explicit resume decisions (spec D4/D5, Milestone B).
//!
//! Three append-only record kinds share the deployment journal. Replaying them in order rebuilds
//! the exact state; nothing here is derived from the clock or from anything outside the journal.
//!
//! * **`pause`** — why and since when an application's automatic triggers are paused, and which
//!   deployment caused it. Written once per originating deployment, the first time a writer sees a
//!   deployment that ended in a failed or interrupted cutover or a successful rollback. A build
//!   failure, a failed preliminary check and an unconfirmed (never begun) cutover never produce one.
//! * **`event`** — one trigger event received while the application was paused, keyed by its
//!   delivery id. An event is never dropped; it stays `held` until a decision names it.
//! * **`resume`** — one developer decision, in one record (so a crash cannot half-apply it): dismiss
//!   everything and clear the pause, or select a held revision. A **selection does not deploy
//!   anything** and does not clear the pause: there is no source executor in Milestone B, so the
//!   selected event waits as *pending attended execution* and the application stays blocked until
//!   that is dismissed (or, later, executed).
//!
//! The pause that is in force is: every pause-causing deployment that no `resume` record has cleared.
//! A new failed cutover is a new deployment and so pauses again; an old one is never re-derived
//! after it was cleared.
//!
//! There is no event source here. [`ingest`] is the narrow interface a future Git/CI integration
//! calls; nothing in this crate generates events.

use crate::engine::{Journal, JournalError};
use crate::ids::{ApplicationId, DeploymentId};
use crate::journal_format::frame;
use crate::state::{CutoverState, Deployment, DeploymentStatus};

pub const MAX_TOKEN: usize = 128;

// ------------------------------------------------------------------ records

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseKind {
    FailedCutover,
    InterruptedCutover,
    Rollback,
}

impl PauseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PauseKind::FailedCutover => "failed_cutover",
            PauseKind::InterruptedCutover => "interrupted_cutover",
            PauseKind::Rollback => "rollback",
        }
    }
    fn parse(s: &str) -> Option<PauseKind> {
        Some(match s {
            "failed_cutover" => PauseKind::FailedCutover,
            "interrupted_cutover" => PauseKind::InterruptedCutover,
            "rollback" => PauseKind::Rollback,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PauseRec {
    pub app: ApplicationId,
    /// The deployment that caused the pause.
    pub origin: DeploymentId,
    pub kind: PauseKind,
    /// When the cutover began or failed, or the rollback finished (from the deployment record).
    pub at_ms: u64,
    /// Recovery outcome or rollback target as recorded when the pause was written. Free text.
    pub context: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRec {
    pub app: ApplicationId,
    pub delivery: String,
    /// What kind of trigger delivered it (`webhook`, `poll`, `ci`, …).
    pub source: String,
    pub revision: String,
    pub received_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Dismiss,
    Newest,
    Revision(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeRec {
    pub app: ApplicationId,
    pub at_ms: u64,
    pub choice: Choice,
    /// Delivery id of the event this decision selected (pending attended execution).
    pub selected: Option<String>,
    /// Held events dismissed by this decision.
    pub dismissed: Vec<String>,
    /// Held events superseded by the selection, each with why.
    pub superseded: Vec<(String, SupersededWhy)>,
    /// Pauses this decision cleared, by originating deployment. Empty for a selection.
    pub cleared: Vec<DeploymentId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupersededWhy {
    /// An older event, replaced by a newer selected one (newest wins).
    NewerSelected,
    /// Not the revision the developer chose.
    NotChosen,
}

impl SupersededWhy {
    pub fn as_str(self) -> &'static str {
        match self {
            SupersededWhy::NewerSelected => "newer_selected",
            SupersededWhy::NotChosen => "not_chosen",
        }
    }
    fn parse(s: &str) -> Option<SupersededWhy> {
        match s {
            "newer_selected" => Some(SupersededWhy::NewerSelected),
            "not_chosen" => Some(SupersededWhy::NotChosen),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlRecord {
    Pause(PauseRec),
    Event(EventRec),
    Resume(ResumeRec),
}

// ------------------------------------------------------------------ validation

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// The input is malformed; nothing was recorded.
    Invalid(String),
    /// The input contradicts what is recorded; nothing was recorded.
    Conflict(String),
    Journal(JournalError),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::Invalid(m) | ControlError::Conflict(m) => f.write_str(m),
            ControlError::Journal(e) => write!(f, "journal write failed: {e:?}"),
        }
    }
}

impl From<JournalError> for ControlError {
    fn from(e: JournalError) -> Self {
        ControlError::Journal(e)
    }
}

fn token(what: &str, v: &str, extra: &[char]) -> Result<(), String> {
    if v.is_empty() || v.len() > MAX_TOKEN {
        return Err(format!("{what} must be 1..={MAX_TOKEN} bytes"));
    }
    if let Some(c) = v
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-' || extra.contains(c)))
    {
        return Err(format!(
            "{what} contains {c:?}; only letters, digits and . _ - {extra:?} are accepted"
        ));
    }
    Ok(())
}

pub fn check_delivery(v: &str) -> Result<(), String> {
    token("a delivery id", v, &[':'])
}
pub fn check_source(v: &str) -> Result<(), String> {
    token("an event source", v, &[]).and_then(|()| {
        if v.len() > 32 {
            Err("an event source is at most 32 bytes".into())
        } else {
            Ok(())
        }
    })
}
pub fn check_revision(v: &str) -> Result<(), String> {
    token("a source revision", v, &['/'])
}

// ------------------------------------------------------------------ codec

fn csv<T: AsRef<str>>(v: &[T]) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        v.iter().map(|s| s.as_ref()).collect::<Vec<_>>().join(",")
    }
}

fn unhex(s: &str) -> Option<String> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b: Option<Vec<u8>> = (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok())
        .collect();
    String::from_utf8(b?).ok()
}

pub fn encode(r: &ControlRecord) -> String {
    let body = match r {
        ControlRecord::Pause(p) => format!(
            "pause\t1\t{}\t{}\t{}\t{}\t{}",
            p.app,
            p.origin,
            p.kind.as_str(),
            p.at_ms,
            p.context.bytes().map(|b| format!("{b:02x}")).collect::<String>()
        ),
        ControlRecord::Event(e) => format!(
            "event\t1\t{}\t{}\t{}\t{}\t{}",
            e.app, e.delivery, e.source, e.revision, e.received_ms
        ),
        ControlRecord::Resume(x) => {
            let (choice, rev) = match &x.choice {
                Choice::Dismiss => ("dismiss", "-".to_string()),
                Choice::Newest => ("newest", "-".to_string()),
                Choice::Revision(r) => ("revision", r.clone()),
            };
            let sup: Vec<String> = x
                .superseded
                .iter()
                .map(|(d, w)| format!("{d}={}", w.as_str()))
                .collect();
            let cleared: Vec<String> = x.cleared.iter().map(|d| d.to_string()).collect();
            format!(
                "resume\t1\t{}\t{}\t{choice}\t{rev}\t{}\t{}\t{}\t{}",
                x.app,
                x.at_ms,
                x.selected.clone().unwrap_or_else(|| "-".into()),
                csv(&x.dismissed),
                csv(&sup),
                csv(&cleared)
            )
        }
    };
    frame(&body)
}

fn bad<T>(m: impl Into<String>) -> Result<T, String> {
    Err(m.into())
}

/// `f` are the tab-separated fields of the checksummed body (already verified by the caller).
pub fn decode(f: &[&str]) -> Result<ControlRecord, String> {
    let n = |s: &str, w: &str| s.parse::<u64>().or_else(|_| bad(format!("bad {w}")));
    let app = |s: &str| s.parse::<ApplicationId>().or_else(|_| bad("bad application id"));
    let dep = |s: &str| s.parse::<DeploymentId>().or_else(|_| bad("bad deployment id"));
    let list = |s: &str| -> Vec<String> {
        if s == "-" {
            vec![]
        } else {
            s.split(',').map(String::from).collect()
        }
    };
    if f.get(1) != Some(&"1") {
        return bad("unknown control record version");
    }
    match f[0] {
        "pause" => {
            if f.len() != 7 {
                return bad("a pause record has the wrong number of fields");
            }
            Ok(ControlRecord::Pause(PauseRec {
                app: app(f[2])?,
                origin: dep(f[3])?,
                kind: PauseKind::parse(f[4]).ok_or("unknown pause kind")?,
                at_ms: n(f[5], "pause time")?,
                context: unhex(f[6]).ok_or("bad pause context")?,
            }))
        }
        "event" => {
            if f.len() != 7 {
                return bad("an event record has the wrong number of fields");
            }
            check_delivery(f[3])?;
            check_source(f[4])?;
            check_revision(f[5])?;
            Ok(ControlRecord::Event(EventRec {
                app: app(f[2])?,
                delivery: f[3].into(),
                source: f[4].into(),
                revision: f[5].into(),
                received_ms: n(f[6], "received time")?,
            }))
        }
        "resume" => {
            if f.len() != 10 {
                return bad("a resume record has the wrong number of fields");
            }
            let choice = match f[4] {
                "dismiss" if f[5] == "-" => Choice::Dismiss,
                "newest" if f[5] == "-" => Choice::Newest,
                "revision" => {
                    check_revision(f[5])?;
                    Choice::Revision(f[5].into())
                }
                _ => return bad("unknown resume choice"),
            };
            let selected = match f[6] {
                "-" => None,
                d => {
                    check_delivery(d)?;
                    Some(d.to_string())
                }
            };
            let dismissed = list(f[7]);
            let mut superseded = Vec::new();
            for item in list(f[8]) {
                let (d, w) = item.split_once('=').ok_or("bad superseded entry")?;
                superseded.push((d.to_string(), SupersededWhy::parse(w).ok_or("bad superseded reason")?));
            }
            let cleared = list(f[9]).iter().map(|d| dep(d)).collect::<Result<Vec<_>, _>>()?;
            for d in dismissed.iter().chain(superseded.iter().map(|(d, _)| d)) {
                check_delivery(d)?;
            }
            Ok(ControlRecord::Resume(ResumeRec {
                app: app(f[2])?,
                at_ms: n(f[3], "decision time")?,
                choice,
                selected,
                dismissed,
                superseded,
                cleared,
            }))
        }
        other => bad(format!("unknown control record {other:?}")),
    }
}

// ------------------------------------------------------------------ state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStatus {
    Held,
    /// Chosen by a resume decision; **pending attended execution**: nothing has run.
    Selected,
    Dismissed,
    Superseded(SupersededWhy),
}

impl EventStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            EventStatus::Held => "held",
            EventStatus::Selected => "selected",
            EventStatus::Dismissed => "dismissed",
            EventStatus::Superseded(_) => "superseded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub rec: EventRec,
    pub status: EventStatus,
    /// The decision (time) that settled it, if any.
    pub decided_ms: Option<u64>,
    /// For a superseded event: the delivery id of the selected event.
    pub related: Option<String>,
}

/// Everything the control records say. Rebuilt by replaying them in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControlState {
    pub pauses: Vec<PauseRec>,
    pub events: Vec<Event>,
    pub resumes: Vec<ResumeRec>,
}

impl ControlState {
    /// Apply one record, refusing any that contradicts the state so far (replay fails closed).
    pub fn apply(&mut self, r: &ControlRecord) -> Result<(), String> {
        match r {
            ControlRecord::Pause(p) => {
                if self.pauses.iter().any(|x| x.origin == p.origin) {
                    return bad(format!("pause for deployment {} is recorded twice", p.origin));
                }
                self.pauses.push(p.clone());
            }
            ControlRecord::Event(e) => {
                if let Some(old) = self.event(&e.app, &e.delivery) {
                    return bad(format!(
                        "delivery {} of application {} is recorded twice{}",
                        e.delivery,
                        e.app,
                        if old.rec == *e { "" } else { " with different content" }
                    ));
                }
                self.events.push(Event {
                    rec: e.clone(),
                    status: EventStatus::Held,
                    decided_ms: None,
                    related: None,
                });
            }
            ControlRecord::Resume(x) => self.apply_resume(x)?,
        }
        Ok(())
    }

    fn apply_resume(&mut self, x: &ResumeRec) -> Result<(), String> {
        let find = |s: &ControlState, d: &str| -> Result<usize, String> {
            s.events
                .iter()
                .position(|e| e.rec.app == x.app && e.rec.delivery == d)
                .ok_or_else(|| format!("a decision names delivery {d}, which is not recorded for {}", x.app))
        };
        let pending = self.pending_selection(&x.app).map(|e| e.rec.delivery.clone());
        match (&x.choice, &x.selected) {
            (Choice::Dismiss, None) => {
                if !x.superseded.is_empty() {
                    return bad("a dismissal supersedes nothing");
                }
            }
            (Choice::Newest | Choice::Revision(_), Some(_)) => {
                if !x.cleared.is_empty() || !x.dismissed.is_empty() {
                    return bad("a selection neither clears a pause nor dismisses events");
                }
                if pending.is_some() {
                    return bad("a selection is already pending for this application");
                }
            }
            _ => return bad("a decision's choice and selection disagree"),
        }
        // validate every reference before changing anything
        let mut touched = Vec::new();
        for d in &x.dismissed {
            let i = find(self, d)?;
            let ok = self.events[i].status == EventStatus::Held
                || (self.events[i].status == EventStatus::Selected && matches!(x.choice, Choice::Dismiss));
            if !ok {
                return bad(format!("delivery {d} is {}, not held", self.events[i].status.as_str()));
            }
            touched.push(i);
        }
        if let Some(p) = &pending {
            if matches!(x.choice, Choice::Dismiss) && !x.dismissed.contains(p) {
                return bad("a dismissal must also dismiss the pending selection");
            }
        }
        let mut sel = None;
        if let Some(d) = &x.selected {
            let i = find(self, d)?;
            if self.events[i].status != EventStatus::Held {
                return bad(format!("selected delivery {d} is not held"));
            }
            sel = Some(i);
        }
        let mut sup = Vec::new();
        for (d, _) in &x.superseded {
            let i = find(self, d)?;
            if self.events[i].status != EventStatus::Held || Some(i) == sel {
                return bad(format!("superseded delivery {d} is not held"));
            }
            sup.push(i);
        }
        // a decision must account for every held event of the application: none is left silently
        let held: Vec<usize> = self
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| e.rec.app == x.app && e.status == EventStatus::Held)
            .map(|(i, _)| i)
            .collect();
        for i in held {
            if !touched.contains(&i) && !sup.contains(&i) && Some(i) != sel {
                return bad(format!(
                    "a decision leaves held delivery {} unaccounted for",
                    self.events[i].rec.delivery
                ));
            }
        }
        for i in touched {
            self.events[i].status = EventStatus::Dismissed;
            self.events[i].decided_ms = Some(x.at_ms);
        }
        if let Some(i) = sel {
            self.events[i].status = EventStatus::Selected;
            self.events[i].decided_ms = Some(x.at_ms);
        }
        let by = x.selected.clone();
        for (i, (_, why)) in sup.into_iter().zip(&x.superseded) {
            self.events[i].status = EventStatus::Superseded(*why);
            self.events[i].decided_ms = Some(x.at_ms);
            self.events[i].related = by.clone();
        }
        self.resumes.push(x.clone());
        Ok(())
    }

    pub fn event(&self, app: &ApplicationId, delivery: &str) -> Option<&Event> {
        self.events
            .iter()
            .find(|e| e.rec.app == *app && e.rec.delivery == delivery)
    }

    pub fn events_of<'a>(&'a self, app: &'a ApplicationId) -> impl Iterator<Item = &'a Event> {
        self.events.iter().filter(move |e| e.rec.app == *app)
    }

    pub fn held<'a>(&'a self, app: &'a ApplicationId) -> Vec<&'a Event> {
        self.events_of(app).filter(|e| e.status == EventStatus::Held).collect()
    }

    pub fn pending_selection<'a>(&'a self, app: &'a ApplicationId) -> Option<&'a Event> {
        self.events_of(app).find(|e| e.status == EventStatus::Selected)
    }

    pub fn pause_of(&self, origin: &DeploymentId) -> Option<&PauseRec> {
        self.pauses.iter().find(|p| p.origin == *origin)
    }

    pub fn is_cleared(&self, origin: &DeploymentId) -> bool {
        self.resumes.iter().any(|r| r.cleared.contains(origin))
    }

    /// After replay: every decision refers to deployments that exist and that really caused a pause.
    pub fn check_against(&self, deployments: &[Deployment]) -> Result<(), String> {
        let cands = pause_candidates(deployments);
        for p in &self.pauses {
            let Some(c) = cands.iter().find(|c| c.origin == p.origin && c.app == p.app) else {
                return bad(format!(
                    "pause names deployment {}, which did not cause a pause of {}",
                    p.origin, p.app
                ));
            };
            // The kind and time are recorded when the pause is first seen; the deployment may move on
            // afterwards. An interrupted cutover later recorded as failed is the one legitimate change,
            // and its pause time must be exactly the cutover's original start (`interruption.started_ms`,
            // persisted by `begin_cutover`). With no such start on record, nothing can vouch for the
            // time, so replay fails closed. Every other kind and time is fixed by the deployment record.
            let started = deployments
                .iter()
                .find(|d| d.id == p.origin)
                .and_then(|d| d.interruption.map(|i| i.started_ms));
            let compatible = match (p.kind, c.kind) {
                (a, b) if a == b => p.at_ms == c.at_ms,
                (PauseKind::InterruptedCutover, PauseKind::FailedCutover) => started == Some(p.at_ms),
                _ => false,
            };
            if !compatible {
                return bad(format!(
                    "pause for deployment {} ({} at {}) contradicts its deployment record ({} at {})",
                    p.origin,
                    p.kind.as_str(),
                    p.at_ms,
                    c.kind.as_str(),
                    c.at_ms
                ));
            }
        }
        for r in &self.resumes {
            for o in &r.cleared {
                if !cands.iter().any(|c| c.origin == *o && c.app == r.app) {
                    return bad(format!(
                        "a decision clears {o}, which did not cause a pause of {}",
                        r.app
                    ));
                }
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ pauses

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PauseCandidate {
    pub app: ApplicationId,
    pub origin: DeploymentId,
    pub kind: PauseKind,
    pub at_ms: u64,
    pub context: String,
}

/// Every deployment whose record says it paused its application: a cutover that began and did not
/// end in a confirmed new release (failed or interrupted), or a successful manual rollback. Pure.
pub fn pause_candidates(deployments: &[Deployment]) -> Vec<PauseCandidate> {
    let mut out = Vec::new();
    for d in deployments {
        let recovery = d
            .recovery
            .map(|r| format!("recovery {}", r.outcome.as_str()))
            .unwrap_or_else(|| "no recovery recorded".into());
        let (kind, at_ms, context) = match d.cutover {
            CutoverState::Begun { at_ms } => (PauseKind::InterruptedCutover, at_ms, recovery),
            CutoverState::Failed { at_ms } => (PauseKind::FailedCutover, at_ms, recovery),
            _ => match (&d.rollback_to, d.status) {
                (Some(t), DeploymentStatus::Succeeded) => (
                    PauseKind::Rollback,
                    d.finished_ms.unwrap_or(d.created_ms),
                    format!("rolled back to {t}"),
                ),
                _ => continue,
            },
        };
        out.push(PauseCandidate {
            app: d.application_id.clone(),
            origin: d.id.clone(),
            kind,
            at_ms,
            context,
        });
    }
    out
}

/// A pause that is in force: caused by a deployment and not cleared by any decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePause {
    pub app: ApplicationId,
    pub origin: DeploymentId,
    pub kind: PauseKind,
    pub since_ms: u64,
    pub context: String,
    /// Whether a `pause` record exists (otherwise it is only derived; the next writer records it).
    pub recorded: bool,
}

pub fn active_pauses(deployments: &[Deployment], ctl: &ControlState) -> Vec<ActivePause> {
    pause_candidates(deployments)
        .into_iter()
        .filter(|c| !ctl.is_cleared(&c.origin))
        .map(|c| match ctl.pause_of(&c.origin) {
            Some(p) => ActivePause {
                app: c.app,
                origin: c.origin,
                kind: p.kind,
                since_ms: p.at_ms,
                context: p.context.clone(),
                recorded: true,
            },
            None => ActivePause {
                app: c.app,
                origin: c.origin,
                kind: c.kind,
                since_ms: c.at_ms,
                context: c.context,
                recorded: false,
            },
        })
        .collect()
}

pub fn is_paused(app: &ApplicationId, deployments: &[Deployment], ctl: &ControlState) -> bool {
    active_pauses(deployments, ctl).iter().any(|p| p.app == *app)
}

/// Record, once each, the pause of every pause-causing deployment that has no `pause` record yet.
/// Idempotent; a crash between two writes just leaves the rest for the next call.
pub fn sync_pauses<J: Journal>(
    j: &mut J,
    deployments: &[Deployment],
    ctl: &mut ControlState,
) -> Result<usize, ControlError> {
    sync_pauses_where(j, deployments, ctl, None)
}

/// As [`sync_pauses`], but only for one application (writes nothing for any other).
fn sync_pauses_where<J: Journal>(
    j: &mut J,
    deployments: &[Deployment],
    ctl: &mut ControlState,
    only: Option<&ApplicationId>,
) -> Result<usize, ControlError> {
    let mut n = 0;
    for c in pause_candidates(deployments) {
        if ctl.pause_of(&c.origin).is_some() || only.is_some_and(|a| *a != c.app) {
            continue;
        }
        let rec = ControlRecord::Pause(PauseRec {
            app: c.app,
            origin: c.origin,
            kind: c.kind,
            at_ms: c.at_ms,
            context: c.context,
        });
        j.control(&rec)?;
        ctl.apply(&rec).map_err(ControlError::Conflict)?;
        n += 1;
    }
    Ok(n)
}

// ------------------------------------------------------------------ event ingress

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Incoming<'a> {
    pub app: &'a ApplicationId,
    pub delivery_id: &'a str,
    pub source: &'a str,
    pub revision: &'a str,
    pub received_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingress {
    /// The application is not paused: nothing was recorded and the caller decides what to do.
    NotPaused,
    /// Recorded as `held`.
    Held,
    /// This delivery id is already recorded with the same content: nothing was written.
    Duplicate,
}

/// The one way a trigger integration hands an event to the engine. A paused application records it
/// as `held` (never dropped); a duplicate delivery changes nothing; a delivery id reused with
/// different content is refused. This application's pauses are recorded first, so a held event always has its pause; nothing is
/// written for an unpaused application or for any other application.
pub fn ingest<J: Journal>(
    j: &mut J,
    deployments: &[Deployment],
    ctl: &mut ControlState,
    ev: Incoming<'_>,
) -> Result<Ingress, ControlError> {
    check_delivery(ev.delivery_id).map_err(ControlError::Invalid)?;
    check_source(ev.source).map_err(ControlError::Invalid)?;
    check_revision(ev.revision).map_err(ControlError::Invalid)?;
    let rec = EventRec {
        app: ev.app.clone(),
        delivery: ev.delivery_id.into(),
        source: ev.source.into(),
        revision: ev.revision.into(),
        received_ms: ev.received_ms,
    };
    if let Some(old) = ctl.event(ev.app, ev.delivery_id) {
        return if old.rec == rec {
            Ok(Ingress::Duplicate)
        } else {
            Err(ControlError::Conflict(format!(
                "delivery {} of application {} is already recorded with different content; refused",
                ev.delivery_id, ev.app
            )))
        };
    }
    // decide from the records alone: NotPaused writes nothing, for any application
    if !is_paused(ev.app, deployments, ctl) {
        return Ok(Ingress::NotPaused);
    }
    sync_pauses_where(j, deployments, ctl, Some(ev.app))?;
    let r = ControlRecord::Event(rec);
    j.control(&r)?;
    ctl.apply(&r).map_err(ControlError::Conflict)?;
    Ok(Ingress::Held)
}

// ------------------------------------------------------------------ resume

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resumed {
    /// Every held event (and a pending selection) was dismissed and the pause was cleared.
    Dismissed { events: usize, pauses: usize },
    /// A held event was selected. **Nothing was deployed.** The pause stays.
    Selected {
        delivery: String,
        revision: String,
        superseded: usize,
    },
    /// The same selection was already recorded; nothing was written.
    AlreadySelected { delivery: String },
    /// Nothing was paused and nothing was held; nothing was written.
    NothingToDo,
}

/// One developer decision for one application. Every choice is validated against the records before
/// anything is written, so a refusal (`Err`) or a no-op writes nothing, even when a legacy deployment
/// has a pause that no `pause` record yet materialises. Only an accepted dismissal records that
/// application's pauses (so the history shows what it cleared) and then its one `resume` record.
pub fn resume<J: Journal>(
    j: &mut J,
    deployments: &[Deployment],
    ctl: &mut ControlState,
    app: &ApplicationId,
    choice: Choice,
    now_ms: u64,
) -> Result<Resumed, ControlError> {
    let active: Vec<ActivePause> = active_pauses(deployments, ctl)
        .into_iter()
        .filter(|p| p.app == *app)
        .collect();
    let held: Vec<(String, String, u64, usize)> = ctl
        .events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.rec.app == *app && e.status == EventStatus::Held)
        .map(|(i, e)| (e.rec.delivery.clone(), e.rec.revision.clone(), e.rec.received_ms, i))
        .collect();
    let pending = ctl.pending_selection(app).cloned();
    match choice {
        Choice::Dismiss => {
            if active.is_empty() && held.is_empty() && pending.is_none() {
                return Ok(Resumed::NothingToDo);
            }
            let mut dismissed: Vec<String> = held.iter().map(|h| h.0.clone()).collect();
            if let Some(p) = &pending {
                dismissed.push(p.rec.delivery.clone());
            }
            let rec = ResumeRec {
                app: app.clone(),
                at_ms: now_ms,
                choice: Choice::Dismiss,
                selected: None,
                dismissed: dismissed.clone(),
                superseded: vec![],
                cleared: active.iter().map(|p| p.origin.clone()).collect(),
            };
            let n = rec.cleared.len();
            sync_pauses_where(j, deployments, ctl, Some(app))?;
            let r = ControlRecord::Resume(rec);
            j.control(&r)?;
            ctl.apply(&r).map_err(ControlError::Conflict)?;
            Ok(Resumed::Dismissed {
                events: dismissed.len(),
                pauses: n,
            })
        }
        Choice::Newest | Choice::Revision(_) => {
            if let Choice::Revision(r) = &choice {
                check_revision(r).map_err(ControlError::Invalid)?;
                if let Some(p) = &pending {
                    if p.rec.revision == *r {
                        return Ok(Resumed::AlreadySelected {
                            delivery: p.rec.delivery.clone(),
                        });
                    }
                }
            }
            if let Some(p) = &pending {
                return Err(ControlError::Conflict(format!(
                    "revision {} (delivery {}) is already selected and pending attended execution; dismiss it with `resume --app {app} --dismiss` before choosing another",
                    p.rec.revision, p.rec.delivery
                )));
            }
            if held.is_empty() {
                return Err(ControlError::Conflict(format!(
                    "application {app} has no held event to select"
                )));
            }
            // newest = latest received, then latest recorded
            let chosen = match &choice {
                Choice::Newest => held.iter().max_by_key(|h| (h.2, h.3)),
                Choice::Revision(r) => {
                    let m = held.iter().filter(|h| h.1 == *r).max_by_key(|h| (h.2, h.3));
                    if m.is_none() {
                        return Err(ControlError::Conflict(format!(
                            "no held event has revision {r}; held revisions: {}",
                            held.iter().map(|h| h.1.as_str()).collect::<Vec<_>>().join(", ")
                        )));
                    }
                    m
                }
                Choice::Dismiss => None,
            }
            .expect("held is not empty");
            let (sel, rev) = (chosen.0.clone(), chosen.1.clone());
            let newest = matches!(choice, Choice::Newest);
            let superseded: Vec<(String, SupersededWhy)> = held
                .iter()
                .filter(|h| h.0 != sel)
                .map(|h| {
                    (
                        h.0.clone(),
                        if newest {
                            SupersededWhy::NewerSelected
                        } else {
                            SupersededWhy::NotChosen
                        },
                    )
                })
                .collect();
            let n = superseded.len();
            let rec = ResumeRec {
                app: app.clone(),
                at_ms: now_ms,
                choice,
                selected: Some(sel.clone()),
                dismissed: vec![],
                superseded,
                cleared: vec![],
            };
            let r = ControlRecord::Resume(rec);
            j.control(&r)?;
            ctl.apply(&r).map_err(ControlError::Conflict)?;
            Ok(Resumed::Selected {
                delivery: sel,
                revision: rev,
                superseded: n,
            })
        }
    }
}
