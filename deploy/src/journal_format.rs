//! The on-disk line format of the first-slice journal, as pure functions (no I/O).
//!
//! One file holds both kinds of record in the order they were made, so the order between a ledger
//! intent and a deployment snapshot survives a crash:
//!
//! ```text
//! <crc32>\theader\tharpoon-journal\t1
//! <crc32>\tsnap\t1\t<deployment snapshot fields…>
//! <crc32>\tintent|receipt|attempt|result\t…        (the ledger codec, unchanged)
//! ```
//!
//! Every line is `<crc32 of the rest, 8 hex digits>\t<rest>` ending in `\n`. Two failure shapes are
//! told apart on purpose:
//! * a **torn tail** — the file ends without a newline: the write never completed (the writer only
//!   acts after a write is acknowledged), so those bytes are discarded;
//! * **corruption** — a newline-terminated line that fails its checksum or does not decode: a
//!   committed record is damaged. Recovery stops with an error and nothing is repaired or skipped.
//!
//! Snapshots are versioned (`snap\t1`); an unknown version is an error, never a best-effort parse.

use crate::ids::{ApplicationId, DeploymentId, ReleaseId};
use crate::ledger::{codec, Ledger, LedgerError, LedgerRecord};
use crate::state::{
    CutoverState, Deployment, DeploymentStatus, Failure, FailureKind, Interruption, Reason, Recovery, RecoveryOutcome,
    Step, StepRecord, StepStatus, Trigger,
};

pub const FILE_VERSION: u32 = 1;
pub const SNAPSHOT_VERSION: u32 = 1;
const MAGIC: &str = "harpoon-journal";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Header { version: u32 },
    Snapshot(Box<Deployment>),
    Ledger(LedgerRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(m: impl Into<String>) -> Result<T, DecodeError> {
    Err(DecodeError(m.into()))
}

// ------------------------------------------------------------------ framing

/// `<crc32>\t<body>`: the framing every line shares (no newline).
pub fn frame(body: &str) -> String {
    format!("{:08x}\t{}", codec::crc32(body.as_bytes()), body)
}

/// The first line of every journal file (without the newline).
pub fn encode_header() -> String {
    frame(&format!("header\t{MAGIC}\t{FILE_VERSION}"))
}

pub fn encode_ledger(r: &LedgerRecord) -> String {
    codec::encode(r)
}

// ------------------------------------------------------------------ snapshot encoding

fn opt(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "-".into())
}

fn flag(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        "0"
    }
}

fn trigger_str(t: Trigger) -> &'static str {
    match t {
        Trigger::Manual => "manual",
        Trigger::Cli => "cli",
        Trigger::Poll => "poll",
        Trigger::Webhook => "webhook",
        Trigger::Rollback => "rollback",
        Trigger::Promote => "promote",
    }
}

fn step_status_str(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Pending => "pending",
        StepStatus::Running => "running",
        StepStatus::Succeeded => "succeeded",
        StepStatus::Failed => "failed",
        StepStatus::Skipped => "skipped",
        StepStatus::Interrupted => "interrupted",
    }
}

fn failure_kind_str(k: FailureKind) -> &'static str {
    match k {
        FailureKind::Source => "source",
        FailureKind::Build => "build",
        FailureKind::PreliminaryCheck => "preliminary_check",
        FailureKind::Cutover => "cutover",
        FailureKind::FinalCheck => "final_check",
    }
}

fn reason_str(r: Reason) -> &'static str {
    match r {
        Reason::NoPreviousRelease => "no_previous_release",
        Reason::ImageMissing => "image_missing",
        Reason::SecretUnavailable => "secret_unavailable",
        Reason::DockerUnreachable => "docker_unreachable",
        Reason::Timeout => "timeout",
        Reason::StartFailed => "start_failed",
        Reason::Cancelled => "cancelled",
    }
}

fn outcome_fields(o: RecoveryOutcome) -> (&'static str, &'static str) {
    match o {
        RecoveryOutcome::InProgress => ("in_progress", "-"),
        RecoveryOutcome::Restored => ("restored", "-"),
        RecoveryOutcome::RestoredUnverified => ("restored_unverified", "-"),
        RecoveryOutcome::RestoreFailed(r) => ("restore_failed", reason_str(r)),
        RecoveryOutcome::NotAttempted(r) => ("not_attempted", reason_str(r)),
    }
}

/// One deployment snapshot as a framed line (no newline).
pub fn encode_snapshot(d: &Deployment) -> String {
    let e = codec::esc;
    let mut f: Vec<String> = vec![
        "snap".into(),
        SNAPSHOT_VERSION.to_string(),
        d.id.to_string(),
        d.application_id.to_string(),
        trigger_str(d.trigger).into(),
        d.status.as_str().into(),
        d.created_ms.to_string(),
        opt(d.started_ms),
        opt(d.finished_ms),
    ];
    let (c, at) = match d.cutover {
        CutoverState::NotStarted => ("not_started", None),
        CutoverState::NotRequired => ("not_required", None),
        CutoverState::Begun { at_ms } => ("begun", Some(at_ms)),
        CutoverState::Confirmed { at_ms } => ("confirmed", Some(at_ms)),
        CutoverState::Failed { at_ms } => ("failed", Some(at_ms)),
    };
    f.extend([c.into(), opt(at)]);
    match &d.failure {
        None => f.extend(["0", "-", "-", "-", "0"].map(String::from)),
        Some(x) => f.extend([
            "1".into(),
            x.step.as_str().into(),
            failure_kind_str(x.kind).into(),
            e(&x.message),
            flag(x.timed_out).into(),
        ]),
    }
    match &d.recovery {
        None => f.extend(["0", "-", "-", "-", "-", "-"].map(String::from)),
        Some(r) => {
            let (o, why) = outcome_fields(r.outcome);
            f.extend([
                "1".into(),
                o.into(),
                why.into(),
                r.started_ms.to_string(),
                opt(r.finished_ms),
                r.budget_ms.to_string(),
            ]);
        }
    }
    match &d.interruption {
        None => f.extend(["0", "-", "-"].map(String::from)),
        Some(i) => f.extend(["1".into(), i.started_ms.to_string(), opt(i.ended_ms)]),
    }
    f.push(d.release.as_ref().map(|r| r.to_string()).unwrap_or_else(|| "-".into()));
    for s in Step::ORDER {
        let r = d.step(s);
        f.extend([
            step_status_str(r.status).into(),
            opt(r.started_ms),
            opt(r.finished_ms),
            e(&r.note),
        ]);
    }
    frame(&f.join("\t"))
}

// ------------------------------------------------------------------ snapshot decoding

struct Fields<'a>(std::slice::Iter<'a, &'a str>);

impl<'a> Fields<'a> {
    fn next(&mut self, what: &str) -> Result<&'a str, DecodeError> {
        match self.0.next() {
            Some(s) => Ok(s),
            None => err(format!("snapshot ends before {what}")),
        }
    }
    fn num(&mut self, what: &str) -> Result<u64, DecodeError> {
        let s = self.next(what)?;
        s.parse().or_else(|_| err(format!("{what} is not a number")))
    }
    fn opt(&mut self, what: &str) -> Result<Option<u64>, DecodeError> {
        let s = self.next(what)?;
        if s == "-" {
            Ok(None)
        } else {
            s.parse().map(Some).or_else(|_| err(format!("{what} is not a number")))
        }
    }
    fn flag(&mut self, what: &str) -> Result<bool, DecodeError> {
        match self.next(what)? {
            "1" => Ok(true),
            "0" => Ok(false),
            _ => err(format!("{what} is not 0 or 1")),
        }
    }
    fn text(&mut self, what: &str) -> Result<String, DecodeError> {
        let s = self.next(what)?;
        codec::unesc(s).map_or_else(|| err(format!("{what} has a bad escape")), Ok)
    }
}

fn parse_status(s: &str) -> Result<DeploymentStatus, DecodeError> {
    DeploymentStatus::ALL
        .iter()
        .copied()
        .find(|x| x.as_str() == s)
        .map_or_else(|| err(format!("unknown status {s:?}")), Ok)
}

fn parse_trigger(s: &str) -> Result<Trigger, DecodeError> {
    Ok(match s {
        "manual" => Trigger::Manual,
        "cli" => Trigger::Cli,
        "poll" => Trigger::Poll,
        "webhook" => Trigger::Webhook,
        "rollback" => Trigger::Rollback,
        "promote" => Trigger::Promote,
        _ => return err(format!("unknown trigger {s:?}")),
    })
}

fn parse_step(s: &str) -> Result<Step, DecodeError> {
    Step::ORDER
        .iter()
        .copied()
        .find(|x| x.as_str() == s)
        .map_or_else(|| err(format!("unknown step {s:?}")), Ok)
}

fn parse_step_status(s: &str) -> Result<StepStatus, DecodeError> {
    Ok(match s {
        "pending" => StepStatus::Pending,
        "running" => StepStatus::Running,
        "succeeded" => StepStatus::Succeeded,
        "failed" => StepStatus::Failed,
        "skipped" => StepStatus::Skipped,
        "interrupted" => StepStatus::Interrupted,
        _ => return err(format!("unknown step status {s:?}")),
    })
}

fn parse_failure_kind(s: &str) -> Result<FailureKind, DecodeError> {
    Ok(match s {
        "source" => FailureKind::Source,
        "build" => FailureKind::Build,
        "preliminary_check" => FailureKind::PreliminaryCheck,
        "cutover" => FailureKind::Cutover,
        "final_check" => FailureKind::FinalCheck,
        _ => return err(format!("unknown failure kind {s:?}")),
    })
}

fn parse_reason(s: &str) -> Result<Reason, DecodeError> {
    Ok(match s {
        "no_previous_release" => Reason::NoPreviousRelease,
        "image_missing" => Reason::ImageMissing,
        "secret_unavailable" => Reason::SecretUnavailable,
        "docker_unreachable" => Reason::DockerUnreachable,
        "timeout" => Reason::Timeout,
        "start_failed" => Reason::StartFailed,
        "cancelled" => Reason::Cancelled,
        _ => return err(format!("unknown reason {s:?}")),
    })
}

fn parse_outcome(o: &str, why: &str) -> Result<RecoveryOutcome, DecodeError> {
    Ok(match o {
        "in_progress" => RecoveryOutcome::InProgress,
        "restored" => RecoveryOutcome::Restored,
        "restored_unverified" => RecoveryOutcome::RestoredUnverified,
        "restore_failed" => RecoveryOutcome::RestoreFailed(parse_reason(why)?),
        "not_attempted" => RecoveryOutcome::NotAttempted(parse_reason(why)?),
        _ => return err(format!("unknown recovery outcome {o:?}")),
    })
}

/// Number of tab-separated fields in a version-1 snapshot body.
const SNAPSHOT_FIELDS: usize = 2 + 7 + 2 + 5 + 6 + 3 + 1 + 6 * 4;

fn decode_snapshot(f: &[&str]) -> Result<Deployment, DecodeError> {
    match f.get(1).and_then(|v| v.parse::<u32>().ok()) {
        Some(SNAPSHOT_VERSION) => {}
        Some(v) => {
            return err(format!(
                "snapshot version {v} is not supported (this build reads {SNAPSHOT_VERSION})"
            ))
        }
        None => return err("snapshot has no version"),
    }
    if f.len() != SNAPSHOT_FIELDS {
        return err(format!("snapshot has {} fields, expected {SNAPSHOT_FIELDS}", f.len()));
    }
    let mut it = Fields(f[2..].iter());
    let id: DeploymentId = it.next("id")?.parse().or_else(|_| err("bad deployment id"))?;
    let application_id: ApplicationId = it
        .next("application id")?
        .parse()
        .or_else(|_| err("bad application id"))?;
    let trigger = parse_trigger(it.next("trigger")?)?;
    let status = parse_status(it.next("status")?)?;
    let created_ms = it.num("created")?;
    let started_ms = it.opt("started")?;
    let finished_ms = it.opt("finished")?;
    let cutover_kind = it.next("cutover")?;
    let cutover_at = it.opt("cutover time")?;
    let cutover = match (cutover_kind, cutover_at) {
        ("not_started", None) => CutoverState::NotStarted,
        ("not_required", None) => CutoverState::NotRequired,
        ("begun", Some(at_ms)) => CutoverState::Begun { at_ms },
        ("confirmed", Some(at_ms)) => CutoverState::Confirmed { at_ms },
        ("failed", Some(at_ms)) => CutoverState::Failed { at_ms },
        _ => return err("inconsistent cutover fields"),
    };
    let has_failure = it.flag("failure flag")?;
    let (fs, fk, fm, ft) = (
        it.next("failure step")?,
        it.next("failure kind")?,
        it.next("failure message")?,
        it.flag("failure timed-out flag")?,
    );
    let failure = if has_failure {
        Some(Failure {
            step: parse_step(fs)?,
            kind: parse_failure_kind(fk)?,
            message: codec::unesc(fm).map_or_else(|| err("failure message has a bad escape"), Ok)?,
            timed_out: ft,
        })
    } else {
        if (fs, fk, fm, ft) != ("-", "-", "-", false) {
            return err("failure fields set without a failure");
        }
        None
    };
    let has_rec = it.flag("recovery flag")?;
    let (ro, rw) = (it.next("recovery outcome")?, it.next("recovery reason")?);
    let (rs, rf, rb) = (
        it.next("recovery start")?,
        it.opt("recovery end")?,
        it.next("recovery budget")?,
    );
    let recovery = if has_rec {
        Some(Recovery {
            outcome: parse_outcome(ro, rw)?,
            started_ms: rs.parse().or_else(|_| err("recovery start is not a number"))?,
            finished_ms: rf,
            budget_ms: rb.parse().or_else(|_| err("recovery budget is not a number"))?,
        })
    } else {
        if (ro, rw, rs, rf, rb) != ("-", "-", "-", None, "-") {
            return err("recovery fields set without a recovery");
        }
        None
    };
    let has_int = it.flag("interruption flag")?;
    let (is, ie) = (it.next("interruption start")?, it.opt("interruption end")?);
    let interruption = if has_int {
        Some(Interruption {
            started_ms: is.parse().or_else(|_| err("interruption start is not a number"))?,
            ended_ms: ie,
        })
    } else {
        if (is, ie) != ("-", None) {
            return err("interruption fields set without an interruption");
        }
        None
    };
    let release = match it.next("release")? {
        "-" => None,
        r => {
            let rel: ReleaseId = r.parse().or_else(|_| err("bad release id"))?;
            if rel != ReleaseId::from_deployment(&id) {
                return err("release id does not belong to this deployment");
            }
            Some(rel)
        }
    };
    let mut steps = Vec::new();
    for s in Step::ORDER {
        steps.push(StepRecord {
            step: s,
            status: parse_step_status(it.next("step status")?)?,
            started_ms: it.opt("step start")?,
            finished_ms: it.opt("step end")?,
            note: it.text("step note")?,
        });
    }
    Ok(Deployment {
        id,
        application_id,
        trigger,
        status,
        created_ms,
        started_ms,
        finished_ms,
        steps,
        cutover,
        failure,
        recovery,
        interruption,
        release,
    })
}

// ------------------------------------------------------------------ line decoding

/// Decode one line (no newline). Checks the checksum before looking at anything else.
pub fn decode_line(line: &str) -> Result<Line, DecodeError> {
    let (crc, body) = line.split_once('\t').map_or_else(|| err("no checksum"), Ok)?;
    let kind = body.split('\t').next().unwrap_or("");
    match kind {
        "header" | "snap" => {
            let want = u32::from_str_radix(crc, 16).or_else(|_| err("bad checksum field"))?;
            if crc.len() != 8 || codec::crc32(body.as_bytes()) != want {
                return err("checksum mismatch");
            }
            let f: Vec<&str> = body.split('\t').collect();
            if kind == "header" {
                if f.len() != 3 || f[1] != MAGIC {
                    return err("not a Harpoon journal header");
                }
                let version = f[2].parse().or_else(|_| err("bad file version"))?;
                Ok(Line::Header { version })
            } else {
                decode_snapshot(&f).map(|d| Line::Snapshot(Box::new(d)))
            }
        }
        _ => codec::decode(line)
            .map(Line::Ledger)
            .map_err(|e| DecodeError(e.0.to_string())),
    }
}

// ------------------------------------------------------------------ whole-file recovery

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tail {
    /// The file ended exactly on a record boundary.
    Clean,
    /// A torn final write of this many bytes is to be discarded; the writer truncates the file to
    /// `valid_len` before appending anything.
    PartialDiscarded(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// A newline-terminated line is damaged (checksum, encoding or content). 1-based line number.
    Corrupt {
        line: usize,
        why: String,
    },
    /// The first line is not a journal header of a version this build reads.
    BadHeader(String),
    UnsupportedVersion(u32),
    /// The records are individually fine but contradict each other.
    Inconsistent(String),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::Corrupt { line, why } => write!(f, "journal line {line} is damaged: {why}"),
            FormatError::BadHeader(w) => write!(f, "not a usable journal: {w}"),
            FormatError::UnsupportedVersion(v) => {
                write!(
                    f,
                    "journal file version {v} is not supported (this build reads {FILE_VERSION})"
                )
            }
            FormatError::Inconsistent(w) => write!(f, "journal contents contradict each other: {w}"),
        }
    }
}

impl std::error::Error for FormatError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    /// Every record after the header, in file order.
    pub lines: Vec<Line>,
    /// Bytes of the file that are whole, valid records (header included).
    pub valid_len: usize,
    pub tail: Tail,
    /// False when the file holds no complete header yet (empty, or a torn first write).
    pub has_header: bool,
}

/// Parse a journal file's bytes. Pure: nothing is written or repaired here.
pub fn recover(bytes: &[u8]) -> Result<Recovered, FormatError> {
    let mut lines = Vec::new();
    let mut pos = 0usize;
    let mut n = 0usize;
    let mut has_header = false;
    while pos < bytes.len() {
        let Some(len) = bytes[pos..].iter().position(|&b| b == b'\n') else {
            return Ok(Recovered {
                lines,
                valid_len: pos,
                tail: Tail::PartialDiscarded(bytes.len() - pos),
                has_header,
            });
        };
        n += 1;
        let text = std::str::from_utf8(&bytes[pos..pos + len]).map_err(|_| FormatError::Corrupt {
            line: n,
            why: "not utf-8".into(),
        })?;
        let line = decode_line(text).map_err(|e| {
            if n == 1 {
                FormatError::BadHeader(e.0)
            } else {
                FormatError::Corrupt { line: n, why: e.0 }
            }
        })?;
        match (n, line) {
            (1, Line::Header { version }) => {
                if version != FILE_VERSION {
                    return Err(FormatError::UnsupportedVersion(version));
                }
                has_header = true;
            }
            (1, _) => return Err(FormatError::BadHeader("the first line is not a header".into())),
            (_, Line::Header { .. }) => {
                return Err(FormatError::Corrupt {
                    line: n,
                    why: "a second header".into(),
                })
            }
            (_, other) => lines.push(other),
        }
        pos += len + 1;
    }
    Ok(Recovered {
        lines,
        valid_len: pos,
        tail: Tail::Clean,
        has_header,
    })
}

/// The state a journal describes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    /// The latest snapshot of each deployment, in order of first appearance.
    pub deployments: Vec<Deployment>,
    pub ledger: Ledger,
}

fn bad_ledger(i: usize, e: LedgerError) -> FormatError {
    FormatError::Inconsistent(format!("ledger record {} does not apply: {e}", i + 1))
}

/// Rebuild deployments and ledger. A deployment that already reached a terminal status must never
/// change again; a snapshot that says otherwise means the file is not what the engine wrote.
pub fn replay(lines: &[Line]) -> Result<State, FormatError> {
    let mut st = State::default();
    let mut ledger_idx = 0usize;
    for l in lines {
        match l {
            Line::Header { .. } => {}
            Line::Snapshot(d) => match st.deployments.iter_mut().find(|x| x.id == d.id) {
                Some(old) => {
                    if old.status.is_terminal() && **d != *old {
                        return Err(FormatError::Inconsistent(format!(
                            "deployment {} changed after reaching {}",
                            d.id, old.status
                        )));
                    }
                    *old = (**d).clone();
                }
                None => st.deployments.push((**d).clone()),
            },
            Line::Ledger(r) => {
                st.ledger.apply(r).map_err(|e| bad_ledger(ledger_idx, e))?;
                ledger_idx += 1;
            }
        }
    }
    Ok(st)
}
