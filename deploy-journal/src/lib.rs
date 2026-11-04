//! Durable journal for the first deployment slice (Milestone B5).
//!
//! One append-only file holds deployment snapshots and resource-ledger records in the order they
//! were made (format: `harpoon_deploy::journal_format`). This crate adds what needs the filesystem:
//!
//! * **durability** — a record is appended with `write_all` (which may take several `write` calls and
//!   is *not* atomic), then the file is synced; only a complete append followed by a successful sync
//!   returns `Ok`, which is what lets the engine act on it. A crash or error mid-append leaves a
//!   torn tail, which recovery detects (no newline) and cuts off. A new file's directory is synced
//!   before its header is written. Files are created owner-only (`0600`).
//! * **one writer** — an exclusive, non-blocking `flock` on `journal.lock`. A second process gets
//!   [`OpenError::Locked`] at once; nothing waits, nothing steals, and the kernel drops the lock
//!   when the holder dies, so there is no stale-lock state to clean up. Read-only inspection
//!   ([`read_only`]) needs no lock and never writes.
//! * **recovery** — a torn trailing write (no newline) is cut off; a damaged *committed* record is an
//!   error and nothing is changed; incomplete deployments are marked `Interrupted` (and that is
//!   persisted, once); nothing is redeployed, removed or adopted here.
//! * **failed writes** — after any failed append or sync the journal refuses all further writes
//!   (the file's state is unknown, and a retried fsync can lie). The engine stops; reopening
//!   recovers from what is actually on disk.
//!
//! Not here: SQLite, a database layer, Docker, a VM, or any decision about the runtime.

use harpoon_deploy::engine::{Journal, JournalError};
use harpoon_deploy::journal_format::{
    encode_header, encode_ledger, encode_snapshot, recover, replay, FormatError, Line, State, Tail,
};
use harpoon_deploy::ledger::{Entry, Ledger, LedgerRecord};
use harpoon_deploy::state::{recover_on_start, Deployment, StartupReport};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

mod sqlite;
pub use sqlite::{writer_lock_status, SqliteJournal, SqliteOpenError, SQLITE_FILE};

pub const JOURNAL_FILE: &str = "journal.wal";
pub const LOCK_FILE: &str = "journal.lock";

// ------------------------------------------------------------------ the storage seam

/// The three operations the journal needs from its file. The real one is [`FileWal`]; tests wrap it
/// to make writes fail or tear at chosen points while still using a real file underneath.
pub trait Wal {
    /// Append all of `bytes` at the end of the file.
    fn append(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// Make everything appended so far durable (data and the size needed to read it back).
    fn sync(&mut self) -> io::Result<()>;
    fn truncate(&mut self, len: u64) -> io::Result<()>;
    /// Make the directory entry of the file durable (needed once, after the file is created).
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()>;
}

pub struct FileWal {
    file: File,
}

impl Wal for FileWal {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)
    }
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        sync_dir(dir)
    }
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

// ------------------------------------------------------------------ errors

#[derive(Debug)]
pub enum OpenError {
    /// Another process (or another handle in this one) holds the journal.
    Locked(PathBuf),
    Io {
        path: PathBuf,
        op: &'static str,
        error: io::Error,
    },
    /// A committed record is damaged, the version is unknown, or the records contradict each other.
    /// The file has not been modified.
    Format(FormatError),
    /// The file exists but does not begin like a Harpoon journal. It has not been modified.
    NotAJournal(PathBuf),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Locked(p) => write!(
                f,
                "another Harpoon process holds the journal ({})",
                p.display()
            ),
            OpenError::Io { path, op, error } => write!(f, "{op} {}: {error}", path.display()),
            OpenError::Format(e) => write!(f, "{e}"),
            OpenError::NotAJournal(p) => write!(
                f,
                "{} is not a Harpoon journal; it was left untouched",
                p.display()
            ),
        }
    }
}

impl std::error::Error for OpenError {}

fn io_err<'a>(path: &'a Path, op: &'static str) -> impl FnOnce(io::Error) -> OpenError + 'a {
    move |error| OpenError::Io {
        path: path.to_path_buf(),
        op,
        error,
    }
}

// ------------------------------------------------------------------ what a restart learns

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The latest snapshot of every deployment (after interruption marking when recovered).
    pub deployments: Vec<Deployment>,
    pub ledger: Ledger,
    /// Pauses, held events and resume decisions (empty for a store that never recorded any).
    pub control: harpoon_deploy::control::ControlState,
    /// Configuration definitions, saved secret versions and snapshots (references, never values).
    pub config: harpoon_deploy::config::ConfigState,
    /// Registered repositories, observed remote commits and each deployment's source commit.
    pub git: harpoon_deploy::git::GitState,
    /// The recorded, redacted log of each deployment step that has one.
    pub logs: harpoon_deploy::steplog::LogState,
    /// A torn final write that was (or, read-only, would be) discarded.
    pub discarded_tail_bytes: usize,
    /// Deployments newly marked interrupted by this open. Empty on every later open.
    pub startup: StartupReport,
}

impl Restored {
    /// Ports still leased: held until a verified cleanup releases them.
    pub fn outstanding_leases(&self) -> Vec<u16> {
        self.ledger.leased_ports()
    }
    /// Cleanups that failed, were refused, or were attempted with no recorded result.
    pub fn cleanup_problems(&self) -> Vec<&Entry> {
        self.ledger.needing_attention()
    }
    /// Entries that are neither released nor deliberately refused: they still need a decision,
    /// made with a verified identity, by whoever reconciles against the runtime.
    pub fn unsettled(&self) -> Vec<&Entry> {
        self.ledger.unsettled()
    }
    pub fn deployment(&self, id: &str) -> Option<&Deployment> {
        self.deployments.iter().find(|d| d.id.as_str() == id)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub appends: u64,
    pub file_syncs: u64,
    pub dir_syncs: u64,
}

// ------------------------------------------------------------------ the journal

pub struct FileJournal<W: Wal = FileWal> {
    wal: W,
    /// Held for the life of the journal; dropping it (or dying) releases the lock.
    _lock: File,
    len: u64,
    broken: Option<String>,
    stats: Stats,
}

fn lock_dir(dir: &Path) -> Result<File, OpenError> {
    let path = dir.join(LOCK_FILE);
    let existed = path.exists();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(io_err(&path, "cannot open the lock file"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(OpenError::Locked(path)),
        Err(TryLockError::Error(e)) => return Err(io_err(&path, "cannot lock")(e)),
    }
    if !existed {
        sync_dir(dir).map_err(io_err(dir, "cannot sync the directory"))?;
    }
    Ok(lock)
}

impl FileJournal<FileWal> {
    /// Open (creating if needed) the journal in `dir`, which must exist. Takes the exclusive
    /// lock, cuts off a torn tail, and returns what the file holds. Nothing is marked interrupted:
    /// see [`FileJournal::open_recovered`].
    pub fn open(dir: &Path) -> Result<(FileJournal, Restored), OpenError> {
        FileJournal::open_with(dir, |w| w)
    }

    /// [`open`](FileJournal::open), then mark every incomplete deployment `Interrupted` and persist
    /// that. Idempotent: a second call finds nothing to mark and writes nothing.
    pub fn open_recovered(dir: &Path, now_ms: u64) -> Result<(FileJournal, Restored), OpenError> {
        FileJournal::open_recovered_with(dir, now_ms, |w| w)
    }
}

impl<W: Wal> FileJournal<W> {
    pub fn open_recovered_with(
        dir: &Path,
        now_ms: u64,
        wrap: impl FnOnce(FileWal) -> W,
    ) -> Result<(FileJournal<W>, Restored), OpenError> {
        let (mut j, mut r) = FileJournal::open_with(dir, wrap)?;
        j.mark_interrupted(&mut r, now_ms)
            .map_err(|e| OpenError::Io {
                path: dir.join(JOURNAL_FILE),
                op: "cannot record interrupted deployments:",
                error: io::Error::other(format!("{e:?}")),
            })?;
        Ok((j, r))
    }

    pub fn open_with(
        dir: &Path,
        wrap: impl FnOnce(FileWal) -> W,
    ) -> Result<(FileJournal<W>, Restored), OpenError> {
        let lock = lock_dir(dir)?;
        let path = dir.join(JOURNAL_FILE);
        let stats = Stats::default();
        if !path.exists() {
            let f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(io_err(&path, "cannot create"))?;
            drop(f);
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .and_then(|mut f| f.read_to_end(&mut bytes))
            .map_err(io_err(&path, "cannot read"))?;
        let rec = recover(&bytes).map_err(OpenError::Format)?;
        let header = encode_header();
        let discarded = match rec.tail {
            Tail::Clean => 0,
            Tail::PartialDiscarded(n) => n,
        };
        if !rec.has_header {
            // empty file, or a torn first write — but never someone else's file
            let partial = &bytes[rec.valid_len..];
            if !header.as_bytes().starts_with(partial) {
                return Err(OpenError::NotAJournal(path));
            }
        }
        let state: State = replay(&rec.lines).map_err(OpenError::Format)?;

        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(io_err(&path, "cannot open for appending"))?;
        let mut j = FileJournal {
            wal: wrap(FileWal { file }),
            _lock: lock,
            len: bytes.len() as u64,
            broken: None,
            stats,
        };
        if discarded > 0 {
            j.wal
                .truncate(rec.valid_len as u64)
                .and_then(|()| j.wal.sync())
                .map_err(io_err(&path, "cannot cut off the torn tail of"))?;
            j.stats.file_syncs += 1;
            j.len = rec.valid_len as u64;
        }
        if !rec.has_header {
            // The file was just created (or never got its header): make its directory entry
            // durable before anything is written into it.
            j.wal
                .sync_dir(dir)
                .map_err(io_err(dir, "cannot sync the directory"))?;
            j.stats.dir_syncs += 1;
            j.append_line(&header).map_err(|e| OpenError::Io {
                path: path.clone(),
                op: "cannot write the header to",
                error: io::Error::other(format!("{e:?}")),
            })?;
        }
        Ok((
            j,
            Restored {
                deployments: state.deployments,
                ledger: state.ledger,
                control: state.control,
                config: state.config,
                git: state.git,
                logs: state.logs,
                discarded_tail_bytes: discarded,
                startup: StartupReport::default(),
            },
        ))
    }

    /// Mark incomplete deployments interrupted and persist each change. Conservative by design:
    /// it only changes snapshots. Ledger entries stay as they are — their leases stay held — until
    /// something that can verify the runtime reconciles them.
    pub fn mark_interrupted(&mut self, r: &mut Restored, now_ms: u64) -> Result<(), JournalError> {
        let before: Vec<Deployment> = r.deployments.clone();
        let report = recover_on_start(&mut r.deployments, now_ms);
        for (old, new) in before.iter().zip(r.deployments.iter()) {
            if old != new {
                self.persist(new)?;
            }
        }
        r.startup = report;
        Ok(())
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// `Some(why)` once a write failed; every later write is refused.
    pub fn broken(&self) -> Option<&str> {
        self.broken.as_deref()
    }

    /// Bytes of valid records in the file as far as this handle knows.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn append_line(&mut self, line: &str) -> Result<(), JournalError> {
        if let Some(why) = &self.broken {
            return Err(JournalError::Io(format!(
                "the journal is unusable after an earlier failed write ({why}); reopen it to recover"
            )));
        }
        let mut buf = String::with_capacity(line.len() + 1);
        buf.push_str(line);
        buf.push('\n');
        self.stats.appends += 1;
        if let Err(e) = self.wal.append(buf.as_bytes()) {
            // Part of the line may be on disk. Cut back to the last whole record if possible;
            // either way nothing more is written through this handle.
            let healed = self
                .wal
                .truncate(self.len)
                .and_then(|()| self.wal.sync())
                .is_ok();
            self.broken = Some(format!(
                "append failed: {e}{}",
                if healed { "" } else { " (tail not cleaned)" }
            ));
            return Err(JournalError::Io(format!("journal append failed: {e}")));
        }
        self.stats.file_syncs += 1;
        if let Err(e) = self.wal.sync() {
            // The record may or may not be durable; an fsync is never retried.
            self.broken = Some(format!("sync failed: {e}"));
            return Err(JournalError::Io(format!("journal sync failed: {e}")));
        }
        self.len += buf.len() as u64;
        Ok(())
    }
}

impl<W: Wal> Journal for FileJournal<W> {
    fn persist(&mut self, d: &Deployment) -> Result<(), JournalError> {
        self.append_line(&encode_snapshot(d))
    }
    fn record(&mut self, r: &LedgerRecord) -> Result<(), JournalError> {
        self.append_line(&encode_ledger(r))
    }
}

// ------------------------------------------------------------------ read-only inspection

/// Read the journal without locking, writing or truncating anything. A write that is in flight
/// shows up as a torn tail and is ignored; damage to a committed record is an error.
pub fn read_only(dir: &Path) -> Result<Restored, OpenError> {
    let path = dir.join(JOURNAL_FILE);
    let mut bytes = Vec::new();
    File::open(&path)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(io_err(&path, "cannot read"))?;
    let rec = recover(&bytes).map_err(OpenError::Format)?;
    if !rec.has_header
        && !encode_header()
            .as_bytes()
            .starts_with(&bytes[rec.valid_len..])
    {
        return Err(OpenError::NotAJournal(path));
    }
    let state = replay(&rec.lines).map_err(OpenError::Format)?;
    Ok(Restored {
        deployments: state.deployments,
        ledger: state.ledger,
        control: state.control,
        config: state.config,
        git: state.git,
        logs: state.logs,
        discarded_tail_bytes: match rec.tail {
            Tail::Clean => 0,
            Tail::PartialDiscarded(n) => n,
        },
        startup: StartupReport::default(),
    })
}

/// Convenience for callers that only want the lines (tests, diagnostics).
pub fn lines(dir: &Path) -> Result<Vec<Line>, OpenError> {
    let path = dir.join(JOURNAL_FILE);
    let bytes = std::fs::read(&path).map_err(io_err(&path, "cannot read"))?;
    recover(&bytes).map(|r| r.lines).map_err(OpenError::Format)
}

// ------------------------------------------------------------------ lock inspection

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockStatus {
    /// No lock file yet: the journal has never been opened for writing.
    NotInitialized,
    Free,
    /// A writer holds it right now.
    Held,
}

/// Is a writer holding the journal? Takes the lock *shared* for an instant and releases it; creates
/// and writes nothing. (A writer trying to open in that instant would see `Locked` and can retry.)
pub fn lock_status(dir: &Path) -> Result<LockStatus, OpenError> {
    lock_status_of(&dir.join(LOCK_FILE))
}

fn lock_status_of(path: &Path) -> Result<LockStatus, OpenError> {
    let path = path.to_path_buf();
    let f = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(LockStatus::NotInitialized),
        Err(e) => return Err(io_err(&path, "cannot open the lock file")(e)),
    };
    match f.try_lock_shared() {
        Ok(()) => Ok(LockStatus::Free),
        Err(TryLockError::WouldBlock) => Ok(LockStatus::Held),
        Err(TryLockError::Error(e)) => Err(io_err(&path, "cannot inspect the lock")(e)),
    }
}
