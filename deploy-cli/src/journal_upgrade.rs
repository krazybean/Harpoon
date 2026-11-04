//! The one-way journal upgrade that the first `repo-remove` performs, and the backup that makes it recoverable.
//!
//! Before schema 2 is stamped, a complete backup of the schema-1 journal is made with SQLite's own `VACUUM INTO`,
//! from the writer's connection while this process holds the journal's exclusive writer lock, so nothing can change
//! the journal between the backup, its verification and the upgrade. The backup is then proved to be a faithful,
//! readable copy; only after every proof holds does the caller write the removal. Recovery is offline and explicit
//! (see docs/results/DEPLOY_GIT_REGISTRATION_REMOVAL.md): nothing here restores, downgrades or edits a backup.

use crate::Fail;
use harpoon_deploy_git::sha256::{sha256, to_hex};
use harpoon_deploy_journal::{inspect_file, replay_file, Restored, SqliteJournal, SQLITE_FILE};
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Free-space proof. The default writes (and fsyncs, then removes) as many bytes as the backup needs in the
/// backups directory, which is the one fact that matters: that the bytes can be stored there now.
pub trait Space {
    fn reserve(&self, dir: &Path, bytes: u64) -> std::io::Result<()>;
}

pub struct WriteProbe;

impl Space for WriteProbe {
    fn reserve(&self, dir: &Path, bytes: u64) -> std::io::Result<()> {
        let probe = dir.join(".space-probe");
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&probe)?;
        let chunk = vec![0u8; 1 << 20];
        let mut left = bytes;
        let result = (|| {
            while left > 0 {
                let n = left.min(chunk.len() as u64) as usize;
                f.write_all(&chunk[..n])?;
                left -= n as u64;
            }
            f.sync_all()
        })();
        drop(f);
        let _ = std::fs::remove_file(&probe);
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupReport {
    pub path: PathBuf,
    pub digest: String,
    pub records: usize,
    pub bytes: u64,
}

const NOTHING_WRITTEN: &str = "No removal was written; the journal is unchanged.";

fn fail(why: impl std::fmt::Display) -> Fail {
    Fail::journal(format!(
        "cannot make the journal backup that comes before the first removal: {why}. {NOTHING_WRITTEN}"
    ))
}

/// A private directory of ours: a real directory (not a link), mode 0700, owned by whoever owns the journal's.
fn private_backups_dir(journal_dir: &Path) -> Result<PathBuf, Fail> {
    let dir = journal_dir.join("backups");
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(fail(format!("{} is a symbolic link", dir.display())))
        }
        Ok(m) if !m.is_dir() => return Err(fail(format!("{} is not a directory", dir.display()))),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .map_err(|e| fail(format!("cannot create {}: {e}", dir.display())))?;
        }
        Err(e) => return Err(fail(format!("cannot inspect {}: {e}", dir.display()))),
    }
    let m = std::fs::metadata(&dir).map_err(fail)?;
    let owner = std::fs::metadata(journal_dir).map_err(fail)?.uid();
    if m.permissions().mode() & 0o777 != 0o700 || m.uid() != owner {
        return Err(fail(format!(
            "{} must be mode 700 and owned by the journal's owner",
            dir.display()
        )));
    }
    Ok(dir)
}

fn sync_dir(dir: &Path) -> Result<(), Fail> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| fail(format!("cannot flush {}: {e}", dir.display())))
}

/// Make and prove the schema-1 backup. `journal` must be the open writer (it holds the writer lock) and `restored`
/// the state it was opened with. Returns the verified backup, or an error after removing anything it created.
pub fn backup_before_schema2(
    journal: &SqliteJournal,
    restored: &Restored,
    journal_dir: &Path,
    now_ms: u64,
    space: &dyn Space,
) -> Result<BackupReport, Fail> {
    let live = journal_dir.join(SQLITE_FILE);
    let size = std::fs::metadata(&live).map_err(fail)?.len();
    let dir = private_backups_dir(journal_dir)?;
    // the copy plus slack for SQLite's compaction and the sidecar
    space
        .reserve(&dir, size.saturating_mul(2).saturating_add(1 << 20))
        .map_err(|e| fail(format!("not enough free space in {} ({e})", dir.display())))?;

    let path = dir.join(format!("pre-schema2-{now_ms}.sqlite3"));
    let sidecar = dir.join(format!("pre-schema2-{now_ms}.sqlite3.sha256"));
    if path.exists() || sidecar.exists() {
        return Err(fail(format!("{} already exists", path.display())));
    }
    let cleanup = |why: Fail| {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&sidecar);
        why
    };
    // From here until the caller writes the removal, the writer lock this process holds keeps every other process
    // from changing the journal; `before` proves nothing changed this process's own view either.
    let before = journal.record_rows().map_err(fail)?;
    journal.backup_into(&path).map_err(|e| cleanup(fail(e)))?;
    // The directory is 0700, so no one else could reach the file in the instant before it is tightened.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| cleanup(fail(format!("cannot set the backup's permissions: {e}"))))?;
    File::open(&path)
        .and_then(|f| f.sync_all())
        .map_err(|e| cleanup(fail(format!("cannot flush the backup: {e}"))))?;
    sync_dir(&dir).map_err(&cleanup)?;

    // --- proofs ---
    let meta = std::fs::metadata(&path).map_err(|e| cleanup(fail(e)))?;
    if meta.permissions().mode() & 0o777 != 0o600 {
        return Err(cleanup(fail("the backup is not mode 600")));
    }
    let facts = inspect_file(&path).map_err(|e| cleanup(fail(e)))?;
    if facts.integrity != "ok" {
        return Err(cleanup(fail(format!(
            "SQLite's integrity check of the backup says {:?}",
            facts.integrity
        ))));
    }
    if facts.schema > 1 {
        return Err(cleanup(fail(format!(
            "the backup declares schema {}; it must be the schema-1 journal an older build can read",
            facts.schema
        ))));
    }
    if facts.rows != before {
        return Err(cleanup(fail(
            "the backup's records are not the journal's records, one for one",
        )));
    }
    if facts.rows.iter().any(|b| b.contains("gitdrop")) {
        return Err(cleanup(fail(
            "the journal already holds a removal record, so there is no schema-1 state to back up",
        )));
    }
    let replayed = replay_file(&path).map_err(|e| cleanup(fail(e)))?;
    if &replayed != restored {
        return Err(cleanup(fail(
            "the backup does not replay to the state the journal has",
        )));
    }
    if journal.record_rows().map_err(|e| cleanup(fail(e)))? != before {
        return Err(cleanup(fail(
            "the journal changed while it was being backed up",
        )));
    }

    // --- the digest, kept beside the backup ---
    let bytes = std::fs::read(&path).map_err(|e| cleanup(fail(e)))?;
    let digest = to_hex(&sha256(&bytes));
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let text = format!(
        "{digest}  {name}\nrecords={}\nschema=1\nsource={}\ncreated-ms={now_ms}\n",
        facts.rows.len(),
        live.display()
    );
    let mut sc = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&sidecar)
        .map_err(|e| cleanup(fail(e)))?;
    sc.write_all(text.as_bytes())
        .and_then(|()| sc.sync_all())
        .map_err(|e| cleanup(fail(e)))?;
    sync_dir(&dir).map_err(&cleanup)?;
    Ok(BackupReport {
        path,
        digest,
        records: facts.rows.len(),
        bytes: bytes.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use harpoon_deploy::engine::Journal;
    use harpoon_deploy::git::{decode, GitRecord, RemovedRec};

    const APP: &str = "app_00000000000000000000000001";

    fn scratch(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "harpoon-upgrade-{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .unwrap();
        dir
    }

    /// A journal holding one registration, opened as a writer (so it holds the lock).
    fn journal(dir: &Path) -> (SqliteJournal, Restored) {
        let (mut j, _) = SqliteJournal::open(dir).unwrap();
        let hex = |s: &str| s.bytes().map(|b| format!("{b:02x}")).collect::<String>();
        let url = hex("https://example.com/acme/shop.git");
        let branch = hex("main");
        let f = [
            "gitrepo", "1", "shop", &url, "-", "none", "-", "-", APP, "-", "-", &branch, "-",
            "8080", "-", "-", "-", "-", "5",
        ];
        let GitRecord::Repo(r) = decode(&f).unwrap() else {
            unreachable!()
        };
        j.git(&GitRecord::Repo(r)).unwrap();
        drop(j);
        SqliteJournal::open(dir).unwrap()
    }

    struct NoSpace;
    impl Space for NoSpace {
        fn reserve(&self, _: &Path, _: u64) -> std::io::Result<()> {
            Err(std::io::Error::other("no space left on device"))
        }
    }
    struct Plenty;
    impl Space for Plenty {
        fn reserve(&self, _: &Path, _: u64) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir.join("backups"))
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_verified_backup_is_private_faithful_and_has_its_digest_beside_it() {
        let dir = scratch("ok");
        let (j, restored) = journal(&dir);
        let r = backup_before_schema2(&j, &restored, &dir, 1234, &Plenty).unwrap();
        assert_eq!(r.records, j.record_rows().unwrap().len());
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir.join("backups")), mode(&r.path)), (0o700, 0o600));
        let side = r.path.with_extension("sqlite3.sha256");
        assert_eq!(mode(&side), 0o600);
        let text = std::fs::read_to_string(&side).unwrap();
        assert!(
            text.starts_with(&format!("{}  pre-schema2-1234.sqlite3\n", r.digest)),
            "{text}"
        );
        assert_eq!(r.digest, to_hex(&sha256(&std::fs::read(&r.path).unwrap())));
        assert_eq!(
            inspect_file(&r.path).unwrap().rows,
            j.record_rows().unwrap()
        );
        assert_eq!(leftovers(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_proof_leaves_no_backup_behind_and_the_journal_untouched() {
        let dir = scratch("nospace");
        let (j, restored) = journal(&dir);
        let before = j.record_rows().unwrap();
        let err = backup_before_schema2(&j, &restored, &dir, 1, &NoSpace).unwrap_err();
        assert!(
            err.message.contains("not enough free space")
                && err.message.contains("No removal was written"),
            "{}",
            err.message
        );
        assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        assert_eq!(j.record_rows().unwrap(), before);
        // a journal that already holds a removal has no schema-1 state to back up
        let (mut j, _) = {
            drop(j);
            SqliteJournal::open(&dir).unwrap()
        };
        let live = j.record_rows().unwrap();
        assert!(!live.is_empty());
        j.remove_registration(&RemovedRec {
            name: "shop".into(),
            app: APP.parse().unwrap(),
            registration: 5,
            url: "https://example.com/acme/shop.git".into(),
            cred: harpoon_deploy::git::CredKind::None,
            at_ms: 6,
        })
        .unwrap();
        let restored = SqliteJournal::read_only(&dir).unwrap();
        let err = backup_before_schema2(&j, &restored, &dir, 2, &Plenty).unwrap_err();
        assert!(
            err.message.contains("declares schema 2")
                || err.message.contains("already holds a removal"),
            "{}",
            err.message
        );
        assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_backups_directory_that_is_not_ours_alone_is_refused() {
        for (label, make) in [("a file", 0), ("a link", 1), ("mode 755", 2)] {
            let dir = scratch(label.replace(' ', "-").as_str());
            let (j, restored) = journal(&dir);
            let b = dir.join("backups");
            match make {
                0 => std::fs::write(&b, "x").unwrap(),
                1 => std::os::unix::fs::symlink(&dir, &b).unwrap(),
                _ => {
                    DirBuilder::new().mode(0o755).create(&b).unwrap();
                    std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o755)).unwrap();
                }
            }
            let err = backup_before_schema2(&j, &restored, &dir, 3, &Plenty).unwrap_err();
            assert!(
                err.message.contains("No removal was written"),
                "{label}: {}",
                err.message
            );
            assert!(
                !dir.read_dir()
                    .unwrap()
                    .flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("pre-schema2")),
                "{label}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn an_existing_backup_of_the_same_name_is_never_overwritten() {
        let dir = scratch("exists");
        let (j, restored) = journal(&dir);
        let first = backup_before_schema2(&j, &restored, &dir, 7, &Plenty).unwrap();
        let bytes = std::fs::read(&first.path).unwrap();
        let err = backup_before_schema2(&j, &restored, &dir, 7, &Plenty).unwrap_err();
        assert!(err.message.contains("already exists"), "{}", err.message);
        assert_eq!(std::fs::read(&first.path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
