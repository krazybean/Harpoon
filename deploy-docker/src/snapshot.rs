//! A private snapshot of the confined build context.
//!
//! Buildx reads the context over a window of seconds to minutes. If the build ran straight from
//! the user's folder, a file swapped during that window (for a link to somewhere else) would be
//! sent to the builder. Instead the context is copied once into a directory only Harpoon can
//! write (`<scratch>/ctx-<nonce>`, mode 0700) and Buildx is pointed at the copy.
//!
//! The copy itself is the only remaining window, and it is narrowed per file: the entry is
//! `lstat`ed, opened, and the *open handle* is `fstat`ed; the device and inode must still match
//! and the handle must be a regular file, otherwise the copy stops. Symlinks are recreated as
//! symlinks (never followed); sockets, FIFOs and devices are skipped and reported. The total is
//! capped at the context limit. Paths the `.dockerignore` rules Harpoon understands exclude are
//! not copied (the Dockerfile and ignore files always are).

use crate::confine::Source;
use crate::ignore::{always_sent, IgnoreRules};
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    /// The entry changed between being listed and being read (different file, or not a plain file).
    Changed(PathBuf),
    TooLarge(u64),
    BadNonce,
    Io(PathBuf, String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::Changed(p) => write!(
                f,
                "{} changed while the build context was being copied; nothing was built",
                p.display()
            ),
            SnapshotError::TooLarge(l) => write!(
                f,
                "the build context copy exceeded the {} MB limit",
                l / 1_000_000
            ),
            SnapshotError::BadNonce => write!(f, "the snapshot name is not a plain hex string"),
            SnapshotError::Io(p, e) => write!(f, "cannot copy {}: {e}", p.display()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub dir: PathBuf,
    pub dockerfile: PathBuf,
    pub files: usize,
    pub bytes: u64,
    /// Entries that are not copied (sockets, FIFOs, devices), relative to the context.
    pub skipped: Vec<String>,
}

fn io(p: &Path, e: std::io::Error) -> SnapshotError {
    SnapshotError::Io(p.to_path_buf(), e.to_string())
}

pub fn snapshot_dir(scratch: &Path, nonce: &str) -> Result<PathBuf, SnapshotError> {
    if nonce.is_empty() || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(SnapshotError::BadNonce);
    }
    Ok(scratch.join(format!("ctx-{nonce}")))
}

pub fn create(
    src: &Source,
    rules: &IgnoreRules,
    limit: u64,
    scratch: &Path,
    nonce: &str,
) -> Result<Snapshot, SnapshotError> {
    create_with(src, rules, limit, scratch, nonce, &mut |_| {})
}

/// [`create`] with a hook called with each source path just before it is opened (tests use it to
/// swap a file at the worst moment).
pub fn create_with(
    src: &Source,
    rules: &IgnoreRules,
    limit: u64,
    scratch: &Path,
    nonce: &str,
    before_open: &mut dyn FnMut(&Path),
) -> Result<Snapshot, SnapshotError> {
    let root = snapshot_dir(scratch, nonce)?;
    DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .map_err(|e| io(&root, e))?;
    let df_rel = src.dockerfile_rel();
    let keep = always_sent(&df_rel);
    let mut snap = Snapshot {
        dir: root.clone(),
        dockerfile: root.join(&df_rel),
        files: 0,
        bytes: 0,
        skipped: Vec::new(),
    };
    let result = copy_tree(src, rules, limit, &keep, &mut snap, before_open);
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&root);
        return Err(e);
    }
    Ok(snap)
}

fn copy_tree(
    src: &Source,
    rules: &IgnoreRules,
    limit: u64,
    keep: &[PathBuf],
    snap: &mut Snapshot,
    before_open: &mut dyn FnMut(&Path),
) -> Result<(), SnapshotError> {
    let mut stack: Vec<(PathBuf, PathBuf, Vec<String>)> =
        vec![(src.context.clone(), snap.dir.clone(), Vec::new())];
    while let Some((from, to, rel)) = stack.pop() {
        for entry in std::fs::read_dir(&from).map_err(|e| io(&from, e))? {
            let entry = entry.map_err(|e| io(&from, e))?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
            let mut r = rel.clone();
            r.push(entry.file_name().to_string_lossy().into_owned());
            let as_path: PathBuf = r.iter().collect();
            let kept = keep.contains(&as_path);
            let holds_kept = keep.iter().any(|k| k.starts_with(&as_path));
            if rules.excludes(&r) && !kept && !(meta.is_dir() && holds_kept) {
                continue;
            }
            let dest = to.join(entry.file_name());
            let ft = meta.file_type();
            if ft.is_dir() {
                DirBuilder::new()
                    .mode(0o700)
                    .create(&dest)
                    .map_err(|e| io(&dest, e))?;
                stack.push((path, dest, r));
            } else if ft.is_symlink() {
                let target = std::fs::read_link(&path).map_err(|e| io(&path, e))?;
                std::os::unix::fs::symlink(&target, &dest).map_err(|e| io(&dest, e))?;
            } else if ft.is_file() {
                before_open(&path);
                let mut f = File::open(&path).map_err(|e| io(&path, e))?;
                let m = f.metadata().map_err(|e| io(&path, e))?;
                if !m.is_file() || m.dev() != meta.dev() || m.ino() != meta.ino() {
                    return Err(SnapshotError::Changed(path));
                }
                let mode = (m.permissions().mode() & 0o777) | 0o600;
                let mut out = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&dest)
                    .map_err(|e| io(&dest, e))?;
                let room = limit.saturating_sub(snap.bytes);
                let mut copied = 0u64;
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = f.read(&mut buf).map_err(|e| io(&path, e))?;
                    if n == 0 {
                        break;
                    }
                    copied += n as u64;
                    if copied > room {
                        return Err(SnapshotError::TooLarge(limit));
                    }
                    out.write_all(&buf[..n]).map_err(|e| io(&dest, e))?;
                }
                out.sync_all().map_err(|e| io(&dest, e))?;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode))
                    .map_err(|e| io(&dest, e))?;
                snap.bytes += copied;
                snap.files += 1;
            } else {
                snap.skipped.push(r.join("/"));
            }
        }
    }
    Ok(())
}

/// Delete a snapshot Harpoon made: only a `ctx-<hex>` directory directly inside `scratch`.
pub fn remove(snapshot: &Snapshot, scratch: &Path) -> std::io::Result<()> {
    let name_ok = snapshot
        .dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            n.strip_prefix("ctx-")
                .map(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if snapshot.dir.parent() != Some(scratch) || !name_ok {
        return Err(std::io::Error::other(
            "not a Harpoon snapshot directory; not removed",
        ));
    }
    std::fs::remove_dir_all(&snapshot.dir)
}
