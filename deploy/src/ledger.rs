//! The durable **resource ledger** (Milestone B1; review gap #1 and the approved JSON-lines
//! journal requirements).
//!
//! Every external object the engine may create — an image, a container, a port lease — has a
//! ledger entry with a deterministic, Harpoon-chosen *name*. Two records bracket creation:
//!
//! * **Intent** — written (and durable) *before* the object is created. It names the object
//!   and the deployment/application that own it. If the process dies right after the create
//!   call, the intent is all the ledger knows: the object may or may not exist.
//! * **Receipt** — written *after* creation with the **actual id** the runtime returned.
//!   Only a receipt proves what was created; an intent never does.
//!
//! Cleanup is recorded the same way: a **CleanupAttempt** before the removal call and a
//! **CleanupResult** after it. A failed or refused cleanup is a permanent part of the ledger
//! and of the deployment outcome; it never disappears.
//!
//! Recovery reconciles *the ledger against what the runtime actually reports* and removes
//! only objects whose Harpoon labels (`managed`, `app`, `deployment`) match the ledger
//! entry. An object with a matching name but different or missing labels is **refused**, and
//! objects that are not in the ledger are never touched.

use crate::ids::{labels, ApplicationId, DeploymentId};
use std::fmt;

// ------------------------------------------------------------------ identity

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Image,
    Container,
    /// A host port reserved for the deployment. Not a runtime object: releasing it is a
    /// ledger-only operation.
    PortLease,
}

impl ResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceKind::Image => "image",
            ResourceKind::Container => "container",
            ResourceKind::PortLease => "port",
        }
    }
    fn parse(s: &str) -> Option<ResourceKind> {
        match s {
            "image" => Some(ResourceKind::Image),
            "container" => Some(ResourceKind::Container),
            "port" => Some(ResourceKind::PortLease),
            _ => None,
        }
    }
}

/// What the ledger knows an object by *before* the runtime has given it an id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceKey {
    pub kind: ResourceKind,
    /// Image tag, container name, or the decimal port number.
    pub name: String,
}

impl ResourceKey {
    pub fn new(kind: ResourceKind, name: impl Into<String>) -> ResourceKey {
        ResourceKey {
            kind,
            name: name.into(),
        }
    }
}

/// What happens to the object when the deployment ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Never cleaned automatically (a built image; kept, labelled, possibly never served).
    Retain,
    /// Cleaned only if the deployment did not succeed (the serving container, its port).
    RemoveOnFailure,
    /// Cleaned whatever the outcome (a temporary preliminary-check container).
    RemoveAlways,
}

impl Disposition {
    fn as_str(self) -> &'static str {
        match self {
            Disposition::Retain => "retain",
            Disposition::RemoveOnFailure => "remove_on_failure",
            Disposition::RemoveAlways => "remove_always",
        }
    }
    fn parse(s: &str) -> Option<Disposition> {
        match s {
            "retain" => Some(Disposition::Retain),
            "remove_on_failure" => Some(Disposition::RemoveOnFailure),
            "remove_always" => Some(Disposition::RemoveAlways),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ records

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    Removed,
    /// Nothing by that identity exists any more.
    AlreadyAbsent,
    /// The removal was tried and failed; the object may still exist.
    Failed(String),
    /// An object with this identity exists but is not provably Harpoon's: it was not touched.
    Refused(String),
}

/// One durable write. The journal persists these in order; [`Ledger::apply`] is the only
/// way to change a ledger, so replaying the same records always yields the same state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerRecord {
    Intent {
        key: ResourceKey,
        app: ApplicationId,
        deployment: DeploymentId,
        disposition: Disposition,
        at_ms: u64,
    },
    Receipt {
        key: ResourceKey,
        deployment: DeploymentId,
        id: String,
        at_ms: u64,
    },
    CleanupAttempt {
        key: ResourceKey,
        deployment: DeploymentId,
        at_ms: u64,
    },
    CleanupResult {
        key: ResourceKey,
        deployment: DeploymentId,
        outcome: CleanupOutcome,
        at_ms: u64,
    },
}

impl LedgerRecord {
    pub fn deployment(&self) -> &DeploymentId {
        match self {
            LedgerRecord::Intent { deployment, .. }
            | LedgerRecord::Receipt { deployment, .. }
            | LedgerRecord::CleanupAttempt { deployment, .. }
            | LedgerRecord::CleanupResult { deployment, .. } => deployment,
        }
    }
    pub fn key(&self) -> &ResourceKey {
        match self {
            LedgerRecord::Intent { key, .. }
            | LedgerRecord::Receipt { key, .. }
            | LedgerRecord::CleanupAttempt { key, .. }
            | LedgerRecord::CleanupResult { key, .. } => key,
        }
    }
}

// ------------------------------------------------------------------ materialised state

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryState {
    /// Intent recorded; no receipt. The object may or may not exist.
    Intended,
    /// Receipt recorded: the runtime returned this id.
    Created,
    Released(Released),
    CleanupFailed(String),
    Refused(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Released {
    Removed,
    AlreadyAbsent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: ResourceKey,
    pub app: ApplicationId,
    pub deployment: DeploymentId,
    pub disposition: Disposition,
    /// `None` until a receipt exists.
    pub id: Option<String>,
    pub state: EntryState,
    pub cleanup_attempts: u32,
    pub intent_ms: u64,
}

impl Entry {
    /// Needs the user's attention: cleanup failed or was refused. Never silently dropped.
    pub fn needs_attention(&self) -> bool {
        matches!(self.state, EntryState::CleanupFailed(_) | EntryState::Refused(_))
    }
    pub fn is_released(&self) -> bool {
        matches!(self.state, EntryState::Released(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    DuplicateIntent(ResourceKey),
    UnknownEntry(ResourceKey),
    /// A receipt for something never intended, or a second, different id for the same entry.
    ReceiptConflict(ResourceKey),
    /// A result without a preceding attempt, or an attempt on an entry that is settled.
    OutOfOrder(ResourceKey, &'static str),
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LedgerError::DuplicateIntent(k) => write!(f, "ledger: duplicate intent for {} {}", k.kind.as_str(), k.name),
            LedgerError::UnknownEntry(k) => write!(f, "ledger: no entry for {} {}", k.kind.as_str(), k.name),
            LedgerError::ReceiptConflict(k) => {
                write!(f, "ledger: conflicting receipt for {} {}", k.kind.as_str(), k.name)
            }
            LedgerError::OutOfOrder(k, what) => {
                write!(f, "ledger: {what} (out of order) for {} {}", k.kind.as_str(), k.name)
            }
        }
    }
}

impl std::error::Error for LedgerError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    entries: Vec<Entry>,
}

impl Ledger {
    pub fn new() -> Ledger {
        Ledger::default()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn find(&self, deployment: &DeploymentId, key: &ResourceKey) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| &e.deployment == deployment && &e.key == key)
    }

    fn find_mut(&mut self, deployment: &DeploymentId, key: &ResourceKey) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .find(|e| &e.deployment == deployment && &e.key == key)
    }

    /// Entries whose cleanup failed or was refused. These stay in every outcome.
    pub fn needing_attention(&self) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.needs_attention()).collect()
    }

    /// Entries that are not settled: still `Intended`/`Created`, or cleanup failed.
    pub fn unsettled(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| !e.is_released() && !matches!(e.state, EntryState::Refused(_)))
            .collect()
    }

    /// Port numbers currently held by an unreleased lease. Input to port allocation.
    pub fn leased_ports(&self) -> Vec<u16> {
        self.entries
            .iter()
            .filter(|e| e.key.kind == ResourceKind::PortLease && !e.is_released())
            .filter_map(|e| e.key.name.parse().ok())
            .collect()
    }

    pub fn apply(&mut self, r: &LedgerRecord) -> Result<(), LedgerError> {
        match r {
            LedgerRecord::Intent {
                key,
                app,
                deployment,
                disposition,
                at_ms,
            } => {
                if self.find(deployment, key).is_some() {
                    return Err(LedgerError::DuplicateIntent(key.clone()));
                }
                self.entries.push(Entry {
                    key: key.clone(),
                    app: app.clone(),
                    deployment: deployment.clone(),
                    disposition: *disposition,
                    id: None,
                    state: EntryState::Intended,
                    cleanup_attempts: 0,
                    intent_ms: *at_ms,
                });
            }
            LedgerRecord::Receipt {
                key, deployment, id, ..
            } => {
                let e = self
                    .find_mut(deployment, key)
                    .ok_or_else(|| LedgerError::ReceiptConflict(key.clone()))?;
                match (&e.id, &e.state) {
                    (None, EntryState::Intended) => {
                        e.id = Some(id.clone());
                        e.state = EntryState::Created;
                    }
                    (Some(existing), _) if existing == id => {} // idempotent replay
                    _ => return Err(LedgerError::ReceiptConflict(key.clone())),
                }
            }
            LedgerRecord::CleanupAttempt { key, deployment, .. } => {
                let e = self
                    .find_mut(deployment, key)
                    .ok_or_else(|| LedgerError::UnknownEntry(key.clone()))?;
                if e.is_released() || matches!(e.state, EntryState::Refused(_)) {
                    return Err(LedgerError::OutOfOrder(
                        key.clone(),
                        "cleanup attempt on a settled entry",
                    ));
                }
                e.cleanup_attempts += 1;
            }
            LedgerRecord::CleanupResult {
                key,
                deployment,
                outcome,
                ..
            } => {
                let e = self
                    .find_mut(deployment, key)
                    .ok_or_else(|| LedgerError::UnknownEntry(key.clone()))?;
                if e.cleanup_attempts == 0 {
                    return Err(LedgerError::OutOfOrder(
                        key.clone(),
                        "cleanup result without an attempt",
                    ));
                }
                e.state = match outcome {
                    CleanupOutcome::Removed => EntryState::Released(Released::Removed),
                    CleanupOutcome::AlreadyAbsent => EntryState::Released(Released::AlreadyAbsent),
                    CleanupOutcome::Failed(m) => EntryState::CleanupFailed(m.clone()),
                    CleanupOutcome::Refused(m) => EntryState::Refused(m.clone()),
                };
            }
        }
        Ok(())
    }

    /// Rebuild from the journal. Stops at the first record that cannot apply and says which.
    pub fn replay<'a>(records: impl IntoIterator<Item = &'a LedgerRecord>) -> Result<Ledger, (usize, LedgerError)> {
        let mut l = Ledger::new();
        for (i, r) in records.into_iter().enumerate() {
            l.apply(r).map_err(|e| (i, e))?;
        }
        Ok(l)
    }
}

// ------------------------------------------------------------------ what the runtime reports

/// An object as the runtime reports it (later: a Docker list response). Labels are the
/// runtime's, not the ledger's, and are the only proof of ownership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub kind: ResourceKind,
    pub name: String,
    pub id: String,
    pub labels: Vec<(String, String)>,
}

impl Observed {
    fn label(&self, k: &str) -> Option<&str> {
        self.labels.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
    }

    /// Carries `managed=true`, this application and this deployment — all three, exactly.
    pub fn labelled_for(&self, app: &ApplicationId, dep: &DeploymentId) -> bool {
        self.label(labels::MANAGED) == Some("true")
            && self.label(labels::APP) == Some(app.as_str())
            && self.label(labels::DEPLOYMENT) == Some(dep.as_str())
    }

    pub fn harpoon_deployment(&self) -> Option<&str> {
        if self.label(labels::MANAGED) == Some("true") {
            self.label(labels::DEPLOYMENT)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndState {
    Succeeded,
    /// Failed, cancelled or interrupted.
    NotSucceeded,
}

/// Everything the runtime must be told to remove one object, including the labels it must
/// re-verify at removal time (the object could have changed since it was listed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveRequest {
    pub kind: ResourceKind,
    pub id: String,
    pub name: String,
    pub app: ApplicationId,
    pub deployment: DeploymentId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveResult {
    Removed,
    AlreadyAbsent,
    /// The labels no longer match what the ledger recorded; the object was left alone.
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Remove this verified object. `adopt` = no receipt exists yet: record one with the
    /// observed id first, so the ledger names what was actually created.
    Remove {
        entry: (DeploymentId, ResourceKey),
        observed: Observed,
        adopt: bool,
    },
    /// Record the id of an object that exists and is ours but was never receipted, and keep
    /// it (its disposition says Retain). Nothing is removed.
    Adopt {
        entry: (DeploymentId, ResourceKey),
        observed: Observed,
    },
    /// Release a port lease (ledger-only).
    ReleaseLease { entry: (DeploymentId, ResourceKey) },
    /// Nothing by that identity exists.
    MarkAbsent { entry: (DeploymentId, ResourceKey) },
    /// An object with this identity exists but is not provably ours. Untouched.
    Refuse {
        entry: (DeploymentId, ResourceKey),
        reason: String,
    },
    /// Labelled as ours but not in the ledger. Reported, never removed.
    ReportOnly { observed: Observed, reason: String },
}

fn should_clean(d: Disposition, end: EndState) -> bool {
    match d {
        Disposition::Retain => false,
        Disposition::RemoveOnFailure => end == EndState::NotSucceeded,
        Disposition::RemoveAlways => true,
    }
}

/// Decide what to do, purely. `ended` lists the deployments known to be finished (or
/// interrupted); entries of any other deployment are left alone — they may be running.
///
/// Invariants (each under test): only objects whose labels match the ledger entry's app and
/// deployment are ever returned as `Remove`; an unlabelled or differently-labelled object is
/// never referenced except as a `Refuse` reason about an entry that already names it.
pub fn plan(ledger: &Ledger, observed: &[Observed], ended: &[(DeploymentId, EndState)]) -> Vec<Action> {
    let mut actions = Vec::new();
    for e in ledger.entries() {
        let Some((_, end)) = ended.iter().find(|(d, _)| d == &e.deployment) else {
            continue;
        };
        if e.is_released() || matches!(e.state, EntryState::Refused(_)) {
            continue;
        }
        let clean = should_clean(e.disposition, *end);
        let unresolved_intent = e.id.is_none();
        // A retained, receipted object is nothing to do; everything else needs a decision.
        if !clean && !unresolved_intent {
            continue;
        }
        let entry = (e.deployment.clone(), e.key.clone());
        if e.key.kind == ResourceKind::PortLease {
            // A lease is a ledger-only reservation. Release it when it is to be cleaned, or when
            // its intent never got a receipt (nothing was reserved).
            actions.push(Action::ReleaseLease { entry });
            continue;
        }
        let by_id =
            e.id.as_ref()
                .and_then(|id| observed.iter().find(|o| o.kind == e.key.kind && &o.id == id));
        let found = by_id.or_else(|| {
            if unresolved_intent {
                observed.iter().find(|o| o.kind == e.key.kind && o.name == e.key.name)
            } else {
                None
            }
        });
        match found {
            Some(o) if o.labelled_for(&e.app, &e.deployment) => {
                if clean {
                    actions.push(Action::Remove {
                        entry,
                        observed: o.clone(),
                        adopt: unresolved_intent,
                    });
                } else {
                    actions.push(Action::Adopt {
                        entry,
                        observed: o.clone(),
                    });
                }
            }
            Some(_) => actions.push(Action::Refuse {
                entry,
                reason: format!(
                    "a {} named {} exists but does not carry this deployment's Harpoon labels; not touched",
                    e.key.kind.as_str(),
                    e.key.name
                ),
            }),
            None => actions.push(Action::MarkAbsent { entry }),
        }
    }
    // Objects labelled for a finished deployment that the ledger has never heard of.
    for o in observed {
        let Some(dep) = o.harpoon_deployment() else { continue };
        if !ended.iter().any(|(d, _)| d.as_str() == dep) {
            continue;
        }
        let known = ledger.entries().iter().any(|e| {
            e.deployment.as_str() == dep
                && e.key.kind == o.kind
                && (e.key.name == o.name || e.id.as_deref() == Some(o.id.as_str()))
        });
        if !known {
            actions.push(Action::ReportOnly {
                observed: o.clone(),
                reason: "labelled for a finished deployment but absent from the ledger; not removed".to_string(),
            });
        }
    }
    actions
}

// ------------------------------------------------------------------ durable line codec

/// Hand-rolled, dependency-free line format for the first-slice JSON-lines-style journal:
/// `<crc32 hex>\t<kind>\t<fields…>\n`, tabs/newlines/backslashes escaped. A torn trailing
/// write is detected (no newline, or a bad checksum on an unterminated tail) and discarded;
/// a *terminated* line that fails its checksum is corruption and stops recovery.
pub mod codec {
    use super::*;

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn esc(s: &str) -> String {
        let mut o = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '\\' => o.push_str("\\\\"),
                '\t' => o.push_str("\\t"),
                '\n' => o.push_str("\\n"),
                '\r' => o.push_str("\\r"),
                c => o.push(c),
            }
        }
        o
    }

    fn unesc(s: &str) -> Option<String> {
        let mut o = String::with_capacity(s.len());
        let mut it = s.chars();
        while let Some(c) = it.next() {
            if c == '\\' {
                match it.next()? {
                    '\\' => o.push('\\'),
                    't' => o.push('\t'),
                    'n' => o.push('\n'),
                    'r' => o.push('\r'),
                    _ => return None,
                }
            } else {
                o.push(c);
            }
        }
        Some(o)
    }

    /// One record as a single line **without** the trailing newline.
    pub fn encode(r: &LedgerRecord) -> String {
        let key = |k: &ResourceKey| format!("{}\t{}", k.kind.as_str(), esc(&k.name));
        let body = match r {
            LedgerRecord::Intent {
                key: k,
                app,
                deployment,
                disposition,
                at_ms,
            } => {
                format!(
                    "intent\t{}\t{}\t{}\t{}\t{}",
                    key(k),
                    app,
                    deployment,
                    disposition.as_str(),
                    at_ms
                )
            }
            LedgerRecord::Receipt {
                key: k,
                deployment,
                id,
                at_ms,
            } => format!("receipt\t{}\t{}\t{}\t{}", key(k), deployment, esc(id), at_ms),
            LedgerRecord::CleanupAttempt {
                key: k,
                deployment,
                at_ms,
            } => format!("attempt\t{}\t{}\t{}", key(k), deployment, at_ms),
            LedgerRecord::CleanupResult {
                key: k,
                deployment,
                outcome,
                at_ms,
            } => {
                let (o, m) = match outcome {
                    CleanupOutcome::Removed => ("removed", String::new()),
                    CleanupOutcome::AlreadyAbsent => ("absent", String::new()),
                    CleanupOutcome::Failed(m) => ("failed", esc(m)),
                    CleanupOutcome::Refused(m) => ("refused", esc(m)),
                };
                format!("result\t{}\t{}\t{}\t{}\t{}", key(k), deployment, o, m, at_ms)
            }
        };
        format!("{:08x}\t{}", crc32(body.as_bytes()), body)
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DecodeError(pub &'static str);

    pub fn decode(line: &str) -> Result<LedgerRecord, DecodeError> {
        let (crc, body) = line.split_once('\t').ok_or(DecodeError("no checksum"))?;
        let want = u32::from_str_radix(crc, 16).map_err(|_| DecodeError("bad checksum field"))?;
        if crc.len() != 8 || crc32(body.as_bytes()) != want {
            return Err(DecodeError("checksum mismatch"));
        }
        let f: Vec<&str> = body.split('\t').collect();
        let kind = |s: &str| ResourceKind::parse(s).ok_or(DecodeError("bad kind"));
        let num = |s: &str| s.parse::<u64>().map_err(|_| DecodeError("bad number"));
        let dep = |s: &str| s.parse::<DeploymentId>().map_err(|_| DecodeError("bad deployment id"));
        let un = |s: &str| unesc(s).ok_or(DecodeError("bad escape"));
        decode_fields(&f, kind, num, dep, un)
    }

    fn decode_fields(
        f: &[&str],
        kind: impl Fn(&str) -> Result<ResourceKind, DecodeError>,
        num: impl Fn(&str) -> Result<u64, DecodeError>,
        dep: impl Fn(&str) -> Result<DeploymentId, DecodeError>,
        un: impl Fn(&str) -> Result<String, DecodeError>,
    ) -> Result<LedgerRecord, DecodeError> {
        match (f.first().copied(), f.len()) {
            (Some("intent"), 7) => Ok(LedgerRecord::Intent {
                key: ResourceKey::new(kind(f[1])?, un(f[2])?),
                app: f[3].parse().map_err(|_| DecodeError("bad application id"))?,
                deployment: dep(f[4])?,
                disposition: Disposition::parse(f[5]).ok_or(DecodeError("bad disposition"))?,
                at_ms: num(f[6])?,
            }),
            (Some("receipt"), 6) => Ok(LedgerRecord::Receipt {
                key: ResourceKey::new(kind(f[1])?, un(f[2])?),
                deployment: dep(f[3])?,
                id: un(f[4])?,
                at_ms: num(f[5])?,
            }),
            (Some("attempt"), 5) => Ok(LedgerRecord::CleanupAttempt {
                key: ResourceKey::new(kind(f[1])?, un(f[2])?),
                deployment: dep(f[3])?,
                at_ms: num(f[4])?,
            }),
            (Some("result"), 7) => Ok(LedgerRecord::CleanupResult {
                key: ResourceKey::new(kind(f[1])?, un(f[2])?),
                deployment: dep(f[3])?,
                outcome: match f[4] {
                    "removed" => CleanupOutcome::Removed,
                    "absent" => CleanupOutcome::AlreadyAbsent,
                    "failed" => CleanupOutcome::Failed(un(f[5])?),
                    "refused" => CleanupOutcome::Refused(un(f[5])?),
                    _ => return Err(DecodeError("bad outcome")),
                },
                at_ms: num(f[6])?,
            }),
            _ => Err(DecodeError("unknown record")),
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Tail {
        /// The stream ended exactly on a record boundary.
        Clean,
        /// A torn final write of this many bytes was ignored. The writer must truncate the
        /// file to `valid_len` before appending, or the next record would be glued to it.
        PartialDiscarded(usize),
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum RecoverError {
        /// A newline-terminated line failed to decode: real corruption, not a torn write.
        Corrupt { line: usize, why: &'static str },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Recovered {
        pub records: Vec<LedgerRecord>,
        pub valid_len: usize,
        pub tail: Tail,
    }

    pub fn recover(bytes: &[u8]) -> Result<Recovered, RecoverError> {
        let mut records = Vec::new();
        let mut pos = 0usize;
        let mut line_no = 0usize;
        while pos < bytes.len() {
            match bytes[pos..].iter().position(|&b| b == b'\n') {
                None => {
                    return Ok(Recovered {
                        records,
                        valid_len: pos,
                        tail: Tail::PartialDiscarded(bytes.len() - pos),
                    });
                }
                Some(n) => {
                    line_no += 1;
                    let line = std::str::from_utf8(&bytes[pos..pos + n]).map_err(|_| RecoverError::Corrupt {
                        line: line_no,
                        why: "not utf-8",
                    })?;
                    let rec = decode(line).map_err(|e| RecoverError::Corrupt {
                        line: line_no,
                        why: e.0,
                    })?;
                    records.push(rec);
                    pos += n + 1;
                }
            }
        }
        Ok(Recovered {
            records,
            valid_len: pos,
            tail: Tail::Clean,
        })
    }
}
