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

fn restore(db: &Db) -> Result<Restored, SqliteOpenError> {
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
        let db = Db::open(&dir.join(SQLITE_FILE), SQLITE_OPEN_READONLY)?;
        let stmt = statement(&db, "SELECT body FROM records ORDER BY seq")?;
        let mut out = Vec::new();
        loop {
            match unsafe { sqlite3_step(stmt.0) } {
                SQLITE_ROW => {
                    let p = unsafe { sqlite3_column_text(stmt.0, 0) };
                    if p.is_null() {
                        return Err(SqliteOpenError("SQLite record has NULL body".into()));
                    }
                    out.push(
                        unsafe { CStr::from_ptr(p.cast()) }
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                SQLITE_DONE => return Ok(out),
                _ => return Err(SqliteOpenError(message(&db))),
            }
        }
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
}
