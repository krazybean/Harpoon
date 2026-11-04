//! Permanent deployment state store.
//!
//! This deliberately reuses the engine's versioned line codecs inside an ordered SQLite table.
//! The engine therefore keeps one replay path while SQLite supplies transactions, migrations and
//! indexed durable storage. The tiny FFI avoids adding a dependency to the offline foundation.

use super::{lines, LockStatus, Restored};
use harpoon_deploy::engine::{Journal, JournalError};
use harpoon_deploy::journal_format::{decode_line, encode_ledger, encode_snapshot, replay, Line};
use harpoon_deploy::ledger::LedgerRecord;
use harpoon_deploy::state::{recover_on_start, Deployment, StartupReport};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs::{File, OpenOptions, TryLockError};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::ptr;

pub const SQLITE_FILE: &str = "deployments.sqlite3";
const LOCK_FILE: &str = "sqlite.lock";
const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READONLY: c_int = 0x0000_0001;
const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
const SQLITE_TRANSIENT: *const c_void = -1isize as *const c_void;

#[repr(C)]
struct sqlite3 {
    _private: [u8; 0],
}
#[repr(C)]
struct sqlite3_stmt {
    _private: [u8; 0],
}

#[link(name = "sqlite3")]
unsafe extern "C" {
    fn sqlite3_open_v2(
        filename: *const c_char,
        db: *mut *mut sqlite3,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close(db: *mut sqlite3) -> c_int;
    fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char;
    fn sqlite3_busy_timeout(db: *mut sqlite3, ms: c_int) -> c_int;
    fn sqlite3_exec(
        db: *mut sqlite3,
        sql: *const c_char,
        cb: *mut c_void,
        arg: *mut c_void,
        error: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_free(p: *mut c_void);
    fn sqlite3_prepare_v2(
        db: *mut sqlite3,
        sql: *const c_char,
        n: c_int,
        stmt: *mut *mut sqlite3_stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut sqlite3_stmt,
        index: c_int,
        value: *const c_char,
        len: c_int,
        destructor: *const c_void,
    ) -> c_int;
    fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_column_text(stmt: *mut sqlite3_stmt, column: c_int) -> *const u8;
    fn sqlite3_column_int64(stmt: *mut sqlite3_stmt, column: c_int) -> i64;
    fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int;
}

#[derive(Debug)]
pub struct SqliteOpenError(pub String);

impl std::fmt::Display for SqliteOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SqliteOpenError {}

fn cstring(s: &str) -> Result<CString, SqliteOpenError> {
    CString::new(s).map_err(|_| SqliteOpenError("SQLite input contains NUL".into()))
}

struct Db(*mut sqlite3);
impl Drop for Db {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                sqlite3_close(self.0);
            }
        }
    }
}

impl Db {
    fn open(path: &Path, flags: c_int) -> Result<Db, SqliteOpenError> {
        let name = cstring(&path.to_string_lossy())?;
        let mut db = ptr::null_mut();
        let rc = unsafe { sqlite3_open_v2(name.as_ptr(), &mut db, flags, ptr::null()) };
        if rc != SQLITE_OK {
            let message = if db.is_null() {
                "cannot open SQLite database".into()
            } else {
                unsafe {
                    CStr::from_ptr(sqlite3_errmsg(db))
                        .to_string_lossy()
                        .into_owned()
                }
            };
            if !db.is_null() {
                unsafe {
                    sqlite3_close(db);
                }
            }
            return Err(SqliteOpenError(format!("{}: {message}", path.display())));
        }
        unsafe {
            sqlite3_busy_timeout(db, 5_000);
        }
        Ok(Db(db))
    }

    fn exec(&self, sql: &str) -> Result<(), SqliteOpenError> {
        let sql = cstring(sql)?;
        let mut error = ptr::null_mut();
        let rc = unsafe {
            sqlite3_exec(
                self.0,
                sql.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut error,
            )
        };
        if rc == SQLITE_OK {
            return Ok(());
        }
        let message = if error.is_null() {
            unsafe {
                CStr::from_ptr(sqlite3_errmsg(self.0))
                    .to_string_lossy()
                    .into_owned()
            }
        } else {
            unsafe { CStr::from_ptr(error).to_string_lossy().into_owned() }
        };
        if !error.is_null() {
            unsafe {
                sqlite3_free(error.cast());
            }
        }
        Err(SqliteOpenError(message))
    }
}

struct Statement(*mut sqlite3_stmt);
impl Drop for Statement {
    fn drop(&mut self) {
        unsafe {
            sqlite3_finalize(self.0);
        }
    }
}

fn statement(db: &Db, sql: &str) -> Result<Statement, SqliteOpenError> {
    let sql = cstring(sql)?;
    let mut stmt = ptr::null_mut();
    let rc = unsafe { sqlite3_prepare_v2(db.0, sql.as_ptr(), -1, &mut stmt, ptr::null_mut()) };
    if rc != SQLITE_OK {
        return Err(SqliteOpenError(unsafe {
            CStr::from_ptr(sqlite3_errmsg(db.0))
                .to_string_lossy()
                .into_owned()
        }));
    }
    Ok(Statement(stmt))
}

fn bind(stmt: &Statement, index: c_int, value: &str) -> Result<(), SqliteOpenError> {
    let value = cstring(value)?;
    let rc = unsafe { sqlite3_bind_text(stmt.0, index, value.as_ptr(), -1, SQLITE_TRANSIENT) };
    if rc == SQLITE_OK {
        Ok(())
    } else {
        Err(SqliteOpenError("cannot bind SQLite text".into()))
    }
}

fn message(db: &Db) -> String {
    unsafe {
        CStr::from_ptr(sqlite3_errmsg(db.0))
            .to_string_lossy()
            .into_owned()
    }
}

fn lock_dir(dir: &Path) -> Result<File, SqliteOpenError> {
    let path = dir.join(LOCK_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(|e| SqliteOpenError(format!("cannot open {}: {e}", path.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(SqliteOpenError(format!(
            "another Harpoon process holds {}",
            path.display()
        ))),
        Err(TryLockError::Error(e)) => Err(SqliteOpenError(format!(
            "cannot lock {}: {e}",
            path.display()
        ))),
    }
}

fn migrate(db: &Db) -> Result<(), SqliteOpenError> {
    db.exec("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY); CREATE TABLE IF NOT EXISTS records (seq INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, body TEXT NOT NULL); INSERT OR IGNORE INTO schema_migrations(version) VALUES (1);")
}

/// The schema a journal declares once it may hold a `gitdrop` (see [`SqliteJournal::remove_registration`]).
pub const REMOVAL_SCHEMA: i64 = 2;

/// What a journal file contains, read without writing: whether SQLite finds it intact, the schema it declares, and
/// every record body in order. Used to prove a backup is a faithful, readable copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFileFacts {
    pub integrity: String,
    pub schema: i64,
    pub rows: Vec<String>,
}

/// Inspect a journal file read-only (a backup, typically).
pub fn inspect_file(path: &Path) -> Result<JournalFileFacts, SqliteOpenError> {
    let db = Db::open(path, SQLITE_OPEN_READONLY)?;
    let integrity = strings_of(&db, "PRAGMA integrity_check")?.join("; ");
    Ok(JournalFileFacts {
        integrity,
        schema: declared_schema(&db),
        rows: strings_of(&db, "SELECT body FROM records ORDER BY seq")?,
    })
}

/// Replay a journal file read-only into its full state (a backup, typically).
pub fn replay_file(path: &Path) -> Result<Restored, SqliteOpenError> {
    restore(&Db::open(path, SQLITE_OPEN_READONLY)?)
}

/// The journal schema this build can read. 1: every record up to the removal record. 2: the journal may also hold
/// `gitdrop` (a removed Git registration). A writer that emits a record older builds cannot decode records the
/// higher number first (see docs/results/DEPLOY_GIT_REGISTRATION_REMOVAL.md), so this build names the problem
/// instead of reporting an obscure undecodable record. No build writes 2 yet.
pub const SUPPORTED_SCHEMA: i64 = 2;

/// The newest schema a journal declares (0 when it declares none).
fn declared_schema(db: &Db) -> i64 {
    let Ok(stmt) = statement(
        db,
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
    ) else {
        return 0;
    };
    if unsafe { sqlite3_step(stmt.0) } == SQLITE_ROW {
        unsafe { sqlite3_column_int64(stmt.0, 0) }
    } else {
        0
    }
}

/// The first text column of every row of `sql`, in order.
fn strings_of(db: &Db, sql: &str) -> Result<Vec<String>, SqliteOpenError> {
    let stmt = statement(db, sql)?;
    let mut out = Vec::new();
    loop {
        match unsafe { sqlite3_step(stmt.0) } {
            SQLITE_ROW => {
                let p = unsafe { sqlite3_column_text(stmt.0, 0) };
                if p.is_null() {
                    return Err(SqliteOpenError("SQLite returned a NULL value".into()));
                }
                out.push(
                    unsafe { CStr::from_ptr(p.cast()) }
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            SQLITE_DONE => return Ok(out),
            _ => return Err(SqliteOpenError(message(db))),
        }
    }
}

fn check_schema(declared: i64) -> Result<(), SqliteOpenError> {
    if declared > SUPPORTED_SCHEMA {
        return Err(SqliteOpenError(format!(
            "this deployment journal was written by a newer Harpoon (journal schema {declared}); this build reads up to schema {SUPPORTED_SCHEMA}. Update Harpoon. The journal is not damaged and was not changed"
        )));
    }
    Ok(())
}

fn restore(db: &Db) -> Result<Restored, SqliteOpenError> {
    check_schema(declared_schema(db))?;
    let stmt = statement(db, "SELECT body FROM records ORDER BY seq")?;
    let mut lines = Vec::new();
    loop {
        match unsafe { sqlite3_step(stmt.0) } {
            SQLITE_ROW => {
                let p = unsafe { sqlite3_column_text(stmt.0, 0) };
                if p.is_null() {
                    return Err(SqliteOpenError("SQLite record has NULL body".into()));
                }
                let body = unsafe {
                    CStr::from_ptr(p.cast())
                        .to_str()
                        .map_err(|_| SqliteOpenError("SQLite record is not UTF-8".into()))?
                };
                lines.push(
                    decode_line(body)
                        .map_err(|e| SqliteOpenError(format!("invalid SQLite record: {e}")))?,
                );
            }
            SQLITE_DONE => break,
            _ => return Err(SqliteOpenError(message(db))),
        }
    }
    let state =
        replay(&lines).map_err(|e| SqliteOpenError(format!("invalid SQLite state: {e}")))?;
    Ok(Restored {
        deployments: state.deployments,
        ledger: state.ledger,
        control: state.control,
        config: state.config,
        git: state.git,
        logs: state.logs,
        discarded_tail_bytes: 0,
        startup: StartupReport::default(),
    })
}

pub struct SqliteJournal {
    db: Db,
    _lock: File,
}

impl SqliteJournal {
    pub fn open(dir: &Path) -> Result<(SqliteJournal, Restored), SqliteOpenError> {
        let lock = lock_dir(dir)?;
        let path = dir.join(SQLITE_FILE);
        // Create the file privately first: SQLite would use the process umask (typically 0644).
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|e| SqliteOpenError(format!("cannot create {}: {e}", path.display())))?;
        let db = Db::open(&path, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE)?;
        migrate(&db)?;
        let mut journal = SqliteJournal { db, _lock: lock };
        let mut restored = restore(&journal.db)?;
        if restored.deployments.is_empty()
            && restored.ledger.entries().is_empty()
            && dir.join(super::JOURNAL_FILE).exists()
        {
            // One-time upgrade only: the legacy journal remains untouched as an exportable
            // recovery artifact, while all future reads and writes use SQLite. The import is a
            // single transaction, so an interruption leaves SQLite empty and the next open retries.
            journal.import_legacy(dir)?;
            restored = restore(&journal.db)?;
        }
        Ok((journal, restored))
    }

    pub fn open_recovered(
        dir: &Path,
        now_ms: u64,
    ) -> Result<(SqliteJournal, Restored), SqliteOpenError> {
        let (mut journal, mut restored) = Self::open(dir)?;
        let before = restored.deployments.clone();
        restored.startup = recover_on_start(&mut restored.deployments, now_ms);
        for (old, new) in before.iter().zip(&restored.deployments) {
            if old != new {
                journal
                    .persist(new)
                    .map_err(|e| SqliteOpenError(format!("{e:?}")))?;
            }
        }
        Ok((journal, restored))
    }

    /// Read without locking or writing. A runtime that has only the legacy journal (no writer has
    /// upgraded it yet) is read from that journal, exactly as before the SQLite store existed.
    pub fn read_only(dir: &Path) -> Result<Restored, SqliteOpenError> {
        let path = dir.join(SQLITE_FILE);
        if !path.exists() {
            return legacy_read(dir);
        }
        let restored = restore(&Db::open(&path, SQLITE_OPEN_READONLY)?)?;
        if restored.deployments.is_empty()
            && restored.ledger.entries().is_empty()
            && dir.join(super::JOURNAL_FILE).exists()
        {
            return legacy_read(dir);
        }
        Ok(restored)
    }

    /// The stored record lines in order, for diagnostics and tests. Reads only.
    pub fn record_lines(dir: &Path) -> Result<Vec<String>, SqliteOpenError> {
        strings_of(
            &Db::open(&dir.join(SQLITE_FILE), SQLITE_OPEN_READONLY)?,
            "SELECT body FROM records ORDER BY seq",
        )
    }

    /// The journal schema this open journal declares (0 when it declares none).
    pub fn declared_schema(&self) -> i64 {
        declared_schema(&self.db)
    }

    /// Every stored record body in order, read through this writer's own connection.
    pub fn record_rows(&self) -> Result<Vec<String>, SqliteOpenError> {
        strings_of(&self.db, "SELECT body FROM records ORDER BY seq")
    }

    /// A consistent copy of the whole journal in a new SQLite file, made with `VACUUM INTO` from this connection
    /// (outside any transaction). `target` must not exist (SQLite refuses an existing file, which some versions accept only when it is empty,
    /// so callers must not depend on that), so an existing backup is never overwritten. The copy is compacted, not byte-identical: compare it by record, not by hash.
    pub fn backup_into(&self, target: &Path) -> Result<(), SqliteOpenError> {
        let stmt = statement(&self.db, "VACUUM INTO ?1")?;
        bind(&stmt, 1, &target.to_string_lossy())?;
        if unsafe { sqlite3_step(stmt.0) } != SQLITE_DONE {
            return Err(SqliteOpenError(format!(
                "cannot write the journal backup {}: {}",
                target.display(),
                message(&self.db)
            )));
        }
        Ok(())
    }

    /// Record the removal of a Git registration. In **one SQLite transaction** it stamps schema 2 on a journal that
    /// does not declare it yet and appends the `gitdrop` record, so there is never a `gitdrop` without the stamp, nor
    /// a stamp from a removal that failed. Returns whether this call stamped the journal (the first removal).
    pub fn remove_registration(
        &mut self,
        rec: &harpoon_deploy::git::RemovedRec,
    ) -> Result<bool, JournalError> {
        let io = |e: String| JournalError::Io(e);
        self.db
            .exec("BEGIN IMMEDIATE")
            .map_err(|e| io(e.to_string()))?;
        let result = (|| {
            let stamp = declared_schema(&self.db) < REMOVAL_SCHEMA;
            if stamp {
                self.db
                    .exec("INSERT OR IGNORE INTO schema_migrations(version) VALUES (2)")
                    .map_err(|e| e.to_string())?;
            }
            self.insert_row(
                "git",
                &harpoon_deploy::git::encode(&harpoon_deploy::git::GitRecord::Removed(rec.clone())),
            )?;
            self.db.exec("COMMIT").map_err(|e| e.to_string())?;
            Ok(stamp)
        })();
        if result.is_err() {
            let _ = self.db.exec("ROLLBACK");
        }
        result.map_err(io)
    }

    /// Store `body` without validating it. Only for tests that forge damaged or misplaced records
    /// to prove replay fails closed; the engine never calls it.
    #[doc(hidden)]
    pub fn append_unchecked(&mut self, kind: &str, body: &str) -> Result<(), SqliteOpenError> {
        self.insert(kind, body)
            .map_err(|e| SqliteOpenError(format!("{e:?}")))
    }

    fn import_legacy(&mut self, dir: &Path) -> Result<(), SqliteOpenError> {
        let fail = |e: String| SqliteOpenError(format!("cannot migrate legacy journal: {e}"));
        let old = lines(dir).map_err(|e| fail(e.to_string()))?;
        self.db.exec("BEGIN IMMEDIATE")?;
        let imported = old.iter().try_for_each(|line| match line {
            Line::Header { .. } => Ok(()),
            Line::Snapshot(d) => self.insert_row("snapshot", &encode_snapshot(d)),
            Line::Ledger(r) => self.insert_row("ledger", &encode_ledger(r)),
            Line::Control(c) => self.insert_row("control", &harpoon_deploy::control::encode(c)),
            Line::Config(c) => self.insert_row("config", &harpoon_deploy::config::encode(c)),
            Line::Git(g) => self.insert_row("git", &harpoon_deploy::git::encode(g)),
            Line::StepLog(l) => self.insert_row("steplog", &harpoon_deploy::steplog::encode(l)),
        });
        match imported.and_then(|()| self.db.exec("COMMIT").map_err(|e| e.to_string())) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = self.db.exec("ROLLBACK");
                Err(fail(e))
            }
        }
    }

    fn insert_row(&self, kind: &str, body: &str) -> Result<(), String> {
        let stmt = statement(&self.db, "INSERT INTO records(kind, body) VALUES (?1, ?2)")
            .map_err(|e| e.to_string())?;
        bind(&stmt, 1, kind).map_err(|e| e.to_string())?;
        bind(&stmt, 2, body).map_err(|e| e.to_string())?;
        if unsafe { sqlite3_step(stmt.0) } != SQLITE_DONE {
            return Err(message(&self.db));
        }
        Ok(())
    }

    fn insert(&mut self, kind: &str, body: &str) -> Result<(), JournalError> {
        self.db
            .exec("BEGIN IMMEDIATE")
            .map_err(|e| JournalError::Io(e.to_string()))?;
        let result = self
            .insert_row(kind, body)
            .and_then(|()| self.db.exec("COMMIT").map_err(|e| e.to_string()));
        if result.is_err() {
            let _ = self.db.exec("ROLLBACK");
        }
        result.map_err(JournalError::Io)
    }
}

fn legacy_read(dir: &Path) -> Result<Restored, SqliteOpenError> {
    super::read_only(dir).map_err(|e| SqliteOpenError(format!("legacy journal: {e}")))
}

/// Is a writer holding the store right now? Looks at the SQLite writer lock and, for a runtime
/// that has not been upgraded yet, the legacy journal lock. Creates and writes nothing.
pub fn writer_lock_status(dir: &Path) -> Result<LockStatus, SqliteOpenError> {
    for name in [LOCK_FILE, super::LOCK_FILE] {
        match super::lock_status_of(&dir.join(name)) {
            Ok(LockStatus::NotInitialized) => continue,
            Ok(status) => return Ok(status),
            Err(e) => return Err(SqliteOpenError(e.to_string())),
        }
    }
    Ok(LockStatus::NotInitialized)
}

impl Journal for SqliteJournal {
    fn persist(&mut self, d: &Deployment) -> Result<(), JournalError> {
        self.insert("snapshot", &encode_snapshot(d))
    }
    fn record(&mut self, r: &LedgerRecord) -> Result<(), JournalError> {
        self.insert("ledger", &encode_ledger(r))
    }
    fn control(&mut self, r: &harpoon_deploy::control::ControlRecord) -> Result<(), JournalError> {
        self.insert("control", &harpoon_deploy::control::encode(r))
    }
    fn config(&mut self, r: &harpoon_deploy::config::ConfigRecord) -> Result<(), JournalError> {
        self.insert("config", &harpoon_deploy::config::encode(r))
    }
    fn git(&mut self, r: &harpoon_deploy::git::GitRecord) -> Result<(), JournalError> {
        self.insert("git", &harpoon_deploy::git::encode(r))
    }
    fn step_log(&mut self, r: &harpoon_deploy::steplog::StepLogRec) -> Result<(), JournalError> {
        self.insert("steplog", &harpoon_deploy::steplog::encode(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> (std::path::PathBuf, Db) {
        // unique per call; a CI hygiene check keeps process APIs out of this crate's sources
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("harpoon-schema-{nanos}-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(
            &dir.join(SQLITE_FILE),
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
        )
        .unwrap();
        migrate(&db).unwrap();
        (dir, db)
    }

    #[test]
    fn a_journal_declaring_a_newer_schema_is_refused_by_name_and_left_unchanged() {
        let (dir, db) = temp_db("newer");
        assert_eq!(declared_schema(&db), 1, "a fresh journal declares schema 1");
        assert!(restore(&db).is_ok());
        // schema 2 (may hold a removed registration) is understood by this build
        db.exec("INSERT INTO schema_migrations(version) VALUES (2)")
            .unwrap();
        assert_eq!(declared_schema(&db), 2);
        assert!(restore(&db).is_ok());
        // a schema this build does not know is refused, with an actionable message, before any record is read
        db.exec("INSERT INTO schema_migrations(version) VALUES (3)")
            .unwrap();
        let before = std::fs::read(dir.join(SQLITE_FILE)).unwrap();
        let err = restore(&db).expect_err("refused").0;
        assert!(
            err.contains("newer Harpoon")
                && err.contains("journal schema 3")
                && err.contains("Update Harpoon"),
            "{err}"
        );
        assert!(err.contains("not damaged"), "{err}");
        drop(db);
        assert_eq!(
            std::fs::read(dir.join(SQLITE_FILE)).unwrap(),
            before,
            "refusing wrote nothing"
        );
        // the public entry points report it too
        let read = SqliteJournal::read_only(&dir)
            .expect_err("read_only refuses")
            .0;
        assert!(read.contains("newer Harpoon"), "{read}");
        let open = match SqliteJournal::open(&dir) {
            Err(e) => e.0,
            Ok(_) => panic!("open must refuse"),
        };
        assert!(open.contains("newer Harpoon"), "{open}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    use harpoon_deploy::git::{GitRecord, RemovedRec, RepoRec};

    fn repo(name: &str, app: &str, at_ms: u64) -> RepoRec {
        // built by decoding a stored form, as a replay would, rather than from a struct literal
        let hex = |s: &str| s.bytes().map(|b| format!("{b:02x}")).collect::<String>();
        let at = at_ms.to_string();
        let url = hex("https://example.com/acme/shop.git");
        let branch = hex("main");
        let f = [
            "gitrepo", "1", name, &url, "-", "none", "-", "-", app, "-", "-", &branch, "-", "8080",
            "-", "-", "-", "-", &at,
        ];
        match harpoon_deploy::git::decode(&f).expect("a valid registration") {
            GitRecord::Repo(r) => *r,
            other => panic!("{other:?}"),
        }
    }

    fn removal(r: &RepoRec) -> RemovedRec {
        RemovedRec {
            name: r.name.clone(),
            app: r.app.clone(),
            registration: r.at_ms,
            url: r.url.clone(),
            cred: r.cred,
            at_ms: r.at_ms + 1,
        }
    }

    const APP: &str = "app_00000000000000000000000001";

    fn journal_with_one_registration(name: &str) -> (std::path::PathBuf, SqliteJournal, RepoRec) {
        let (dir, db) = temp_db(name);
        drop(db);
        let (mut j, _) = SqliteJournal::open(&dir).unwrap();
        let r = repo("shop", APP, 5);
        j.git(&GitRecord::Repo(Box::new(r.clone()))).unwrap();
        (dir, j, r)
    }

    fn schemas(db: &Db) -> Vec<String> {
        strings_of(db, "SELECT version FROM schema_migrations ORDER BY version").unwrap()
    }

    #[test]
    fn the_first_removal_stamps_schema_2_in_the_same_transaction_and_later_ones_do_not() {
        let (dir, mut j, r) = journal_with_one_registration("stamp");
        assert_eq!(
            (j.declared_schema(), schemas(&j.db)),
            (1, vec!["1".to_string()])
        );
        assert!(
            j.remove_registration(&removal(&r)).unwrap(),
            "the first removal stamps"
        );
        assert_eq!(
            (j.declared_schema(), schemas(&j.db)),
            (2, vec!["1".to_string(), "2".to_string()])
        );
        let rows = j.record_rows().unwrap();
        assert!(rows.last().unwrap().contains("gitdrop"), "{rows:?}");
        // a second removal (after the name is registered again) only appends
        let again = repo("shop", APP, 50);
        j.git(&GitRecord::Repo(Box::new(again.clone()))).unwrap();
        assert!(!j.remove_registration(&removal(&again)).unwrap());
        assert_eq!(
            schemas(&j.db),
            vec!["1".to_string(), "2".to_string()],
            "no second stamp row"
        );
        assert_eq!(
            j.record_rows()
                .unwrap()
                .iter()
                .filter(|b| b.contains("gitdrop"))
                .count(),
            2
        );
        drop(j);
        // and the journal replays, with both removals
        assert_eq!(SqliteJournal::read_only(&dir).unwrap().git.retired.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_removal_that_fails_leaves_neither_the_record_nor_the_stamp() {
        let (dir, mut j, r) = journal_with_one_registration("atomic");
        // make the SECOND statement of the transaction (the record insert) fail, after the stamp statement ran
        j.db.exec("CREATE TRIGGER refuse_removal BEFORE INSERT ON records WHEN NEW.body LIKE '%gitdrop%' BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
        let before = j.record_rows().unwrap();
        let err = j.remove_registration(&removal(&r)).unwrap_err();
        assert!(format!("{err:?}").contains("refused"), "{err:?}");
        assert_eq!(
            j.declared_schema(),
            1,
            "the stamp was rolled back with the record"
        );
        assert_eq!(schemas(&j.db), vec!["1".to_string()]);
        assert_eq!(j.record_rows().unwrap(), before);
        // the connection is usable afterwards (the failed transaction was rolled back, not left open)
        j.db.exec("DROP TRIGGER refuse_removal").unwrap();
        assert!(j.remove_registration(&removal(&r)).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_backup_is_a_faithful_readable_copy_and_never_overwrites() {
        let (dir, j, _r) = journal_with_one_registration("backup");
        let backup = dir.join("backup.sqlite3");
        j.backup_into(&backup).unwrap();
        let facts = inspect_file(&backup).unwrap();
        assert_eq!(facts.integrity, "ok");
        assert_eq!(facts.schema, 1);
        assert_eq!(
            facts.rows,
            j.record_rows().unwrap(),
            "record for record the same"
        );
        assert_eq!(
            replay_file(&backup).unwrap(),
            restore(&j.db).unwrap(),
            "and it replays to the same state"
        );
        // an existing backup is not overwritten
        let before = std::fs::read(&backup).unwrap();
        assert!(j.backup_into(&backup).is_err());
        assert_eq!(std::fs::read(&backup).unwrap(), before);
        // a directory that does not exist is an error, not a silent skip
        assert!(j
            .backup_into(&dir.join("missing").join("b.sqlite3"))
            .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restoring_the_backup_returns_the_state_from_before_the_removal() {
        let (dir, mut j, r) = journal_with_one_registration("restore");
        let backup = dir.join("pre-schema2.sqlite3");
        j.backup_into(&backup).unwrap();
        let before = restore(&j.db).unwrap();
        assert!(j.remove_registration(&removal(&r)).unwrap());
        drop(j); // the writer is gone: the recovery is offline
        let live = dir.join(SQLITE_FILE);
        assert_eq!(SqliteJournal::read_only(&dir).unwrap().git.retired.len(), 1);
        std::fs::copy(&backup, &live).unwrap();
        let restored = SqliteJournal::read_only(&dir).unwrap();
        assert_eq!(restored, before, "exactly the pre-removal state");
        assert!(restored.git.retired.is_empty() && restored.git.repos.contains_key("shop"));
        assert_eq!(
            declared_schema(&Db::open(&live, SQLITE_OPEN_READONLY).unwrap()),
            1,
            "and schema 1 again, with nothing edited"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_journal_uses_rollback_journaling_so_recovery_has_no_wal_or_shm_to_handle() {
        let (dir, j, _r) = journal_with_one_registration("mode");
        assert_eq!(
            strings_of(&j.db, "PRAGMA journal_mode").unwrap(),
            vec!["delete".to_string()]
        );
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names
                .iter()
                .any(|n| n.ends_with("-wal") || n.ends_with("-shm")),
            "{names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_checks_accept_what_this_build_reads() {
        for v in [0, 1, SUPPORTED_SCHEMA] {
            assert!(check_schema(v).is_ok(), "{v}");
        }
        assert!(check_schema(SUPPORTED_SCHEMA + 1).is_err());
    }
}
