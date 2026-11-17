//! A record of why a deployment run stopped that does not depend on the journal. When the journal is what failed, or the run
//! panicked, nothing else says why: this file does. Plain `std::fs` append, private to the user, one line per stop.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const FILE: &str = "deploy-diagnostics.log";
// shortcut: one generation of rotation at this size, a proper log policy if these ever need to be kept longer
const LIMIT: u64 = 256 * 1024;

pub fn path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(FILE)
}

/// Append one line; best effort, and it returns where it wrote (or nothing) so the caller can point at it.
pub fn note(runtime_dir: &Path, text: &str) -> Option<PathBuf> {
    let p = path(runtime_dir);
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > LIMIT) {
        let _ = std::fs::rename(&p, runtime_dir.join(format!("{FILE}.1")));
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&p)
        .ok()?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    writeln!(f, "{at}\t{}", text.replace(['\n', '\r'], " ")).ok()?;
    Some(p)
}
