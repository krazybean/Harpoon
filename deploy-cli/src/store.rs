//! The runtime directory and its one canonical journal: creation (private, `0700`), the
//! simulated/docker marker, and the translation of journal errors into actionable diagnostics.

use crate::Fail;
use harpoon_deploy_journal::{Restored, SqliteJournal, SqliteOpenError, SQLITE_FILE};
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const JOURNAL_SUBDIR: &str = "deploy-journal";
pub const KIND_FILE: &str = "runtime-kind";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Docker,
    Simulated,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Docker => "docker",
            Kind::Simulated => "simulated",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Runtime {
    pub dir: PathBuf,
}

impl Runtime {
    pub fn from_flag(v: Option<&str>) -> Result<Runtime, Fail> {
        let v = v.ok_or_else(|| {
            Fail::usage("--runtime-dir DIR is required (there is no default and no guess)")
        })?;
        if v.is_empty() {
            return Err(Fail::usage("--runtime-dir must not be empty"));
        }
        let p = PathBuf::from(v);
        if !p.is_absolute() {
            return Err(Fail::usage("--runtime-dir must be an absolute path"));
        }
        // An existing runtime directory is canonicalised; a missing one is created by writers.
        Ok(Runtime {
            dir: std::fs::canonicalize(&p).unwrap_or(p),
        })
    }
    pub fn journal_dir(&self) -> PathBuf {
        self.dir.join(JOURNAL_SUBDIR)
    }
    pub fn scratch_dir(&self) -> PathBuf {
        self.dir.join("scratch")
    }
    pub fn journal_file(&self) -> PathBuf {
        self.journal_dir().join(SQLITE_FILE)
    }
    pub fn initialised(&self) -> bool {
        self.journal_file().exists() || self.journal_dir().join("journal.wal").exists()
    }

    /// `None` when unmarked (a journal that predates the marker is treated as a docker runtime).
    pub fn kind(&self) -> Result<Option<Kind>, Fail> {
        match std::fs::read_to_string(self.dir.join(KIND_FILE)) {
            Ok(s) => match s.trim() {
                "docker" => Ok(Some(Kind::Docker)),
                "simulated" => Ok(Some(Kind::Simulated)),
                other => Err(Fail::journal(format!(
                    "{} holds {other:?}; expected \"docker\" or \"simulated\"",
                    self.dir.join(KIND_FILE).display()
                ))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Fail::journal(format!(
                "cannot read {}: {e}",
                self.dir.join(KIND_FILE).display()
            ))),
        }
    }
}

fn mode_of(p: &Path) -> std::io::Result<u32> {
    Ok(std::fs::symlink_metadata(p)?.permissions().mode() & 0o777)
}

/// A private directory: exists with no group/other access and is not a symlink, or is created `0700`
/// (parent must already exist). With `create == false` a missing directory is returned as `Ok(false)`.
pub fn private_dir(p: &Path, create: bool) -> Result<bool, Fail> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => Err(Fail::journal(format!(
            "{} is a symlink; the journal location must be a real private directory",
            p.display()
        ))),
        Ok(m) if !m.is_dir() => Err(Fail::journal(format!(
            "{} exists and is not a directory",
            p.display()
        ))),
        Ok(_) => {
            let mode = mode_of(p)
                .map_err(|e| Fail::journal(format!("cannot inspect {}: {e}", p.display())))?;
            if mode & 0o077 != 0 {
                return Err(Fail::journal(format!(
                    "{} has mode {mode:o}; it must be private (700). Fix it yourself with: chmod 700 {}",
                    p.display(),
                    p.display()
                )));
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Ok(false);
            }
            let parent = p.parent().filter(|x| x.is_dir()).ok_or_else(|| {
                Fail::journal(format!(
                    "the parent of {} does not exist; create it first",
                    p.display()
                ))
            })?;
            DirBuilder::new()
                .mode(0o700)
                .create(p)
                .map_err(|e| Fail::journal(format!("cannot create {}: {e}", p.display())))?;
            File::open(parent)
                .and_then(|d| d.sync_all())
                .map_err(|e| Fail::journal(format!("cannot sync {}: {e}", parent.display())))?;
            Ok(true)
        }
        Err(e) => Err(Fail::journal(format!(
            "cannot inspect {}: {e}",
            p.display()
        ))),
    }
}

/// Create the runtime directory, the journal directory and the kind marker (all private), or
/// verify them. Refuses a runtime of the other kind.
pub fn prepare_for_writing(rt: &Runtime, want: Kind) -> Result<(), Fail> {
    private_dir(&rt.dir, true)?;
    private_dir(&rt.journal_dir(), true)?;
    match rt.kind()? {
        Some(k) if k != want => {
            return Err(Fail::refused(format!(
                "this runtime directory is a {} runtime; a {} command cannot use it (simulated and real journals are never mixed)",
                k.as_str(),
                want.as_str()
            )))
        }
        Some(_) => {}
        None => {
            if rt.initialised() {
                if want == Kind::Simulated {
                    return Err(Fail::refused(
                        "this runtime directory already has a journal that is not marked simulated; simulations need their own runtime directory",
                    ));
                }
            } else {
                let path = rt.dir.join(KIND_FILE);
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                    .map_err(|e| Fail::journal(format!("cannot create {}: {e}", path.display())))?;
                f.write_all(format!("{}\n", want.as_str()).as_bytes())
                    .and_then(|()| f.sync_all())
                    .map_err(|e| Fail::journal(format!("cannot write {}: {e}", path.display())))?;
                File::open(&rt.dir)
                    .and_then(|d| d.sync_all())
                    .map_err(|e| Fail::journal(format!("cannot sync {}: {e}", rt.dir.display())))?;
            }
        }
    }
    Ok(())
}

/// A held writer lock is retried later; anything else means the store cannot be trusted and
/// deployments are refused, with nothing repaired or discarded.
pub fn open_error(rt: &Runtime, e: SqliteOpenError) -> Fail {
    if e.0.contains("another Harpoon process holds") {
        return Fail::journal(format!(
            "another Harpoon process holds the journal ({}). Nothing waits and nothing was changed; retry when it has finished. \
             Do not create a second journal directory: that would bypass lease coordination.",
            rt.journal_dir().display()
        ));
    }
    Fail::journal(format!(
        "the journal is not usable: {e}.\n\
         Nothing was changed, repaired or discarded, and deployments are refused until you resolve this.\n\
         To keep a copy for investigation: harpoon-deploy export-journal --runtime-dir {} --to <new file>\n\
         Do not delete or reset the journal: it is the record of what Harpoon created.",
        rt.dir.display()
    ))
}

pub fn open_writer(rt: &Runtime, kind: Kind) -> Result<(SqliteJournal, Restored), Fail> {
    prepare_for_writing(rt, kind)?;
    SqliteJournal::open(&rt.journal_dir()).map_err(|e| open_error(rt, e))
}

pub fn open_writer_recovered(
    rt: &Runtime,
    kind: Kind,
    now_ms: u64,
) -> Result<(SqliteJournal, Restored), Fail> {
    prepare_for_writing(rt, kind)?;
    SqliteJournal::open_recovered(&rt.journal_dir(), now_ms).map_err(|e| open_error(rt, e))
}

/// Read-only restore; `None` when there is no journal yet. Never creates or writes anything.
pub fn read_restored(rt: &Runtime) -> Result<Option<Restored>, Fail> {
    if !rt.initialised() {
        return Ok(None);
    }
    SqliteJournal::read_only(&rt.journal_dir())
        .map(Some)
        .map_err(|e| open_error(rt, e))
}
