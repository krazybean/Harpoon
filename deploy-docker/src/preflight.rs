//! Host-side preflight: build-context size and free disk space. The disk probe is injectable so
//! tests never depend on the machine's disk.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

/// Walk `dir` summing file sizes, never following symlinks, stopping as soon as `limit` is
/// exceeded. `Ok(size)` may be a lower bound only when it is greater than `limit`.
pub fn context_size(dir: &Path, limit: u64) -> std::io::Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
                if total > limit {
                    return Ok(total);
                }
            }
        }
    }
    Ok(total)
}

pub trait DiskProbe {
    /// Free bytes on the volume holding `path`.
    fn free_bytes(&self, path: &Path) -> Result<u64, String>;
}

/// Uses `/bin/df -Pk` with a cleared environment.
pub struct DfProbe;

impl DiskProbe for DfProbe {
    fn free_bytes(&self, path: &Path) -> Result<u64, String> {
        let out = Command::new("/bin/df")
            .env_clear()
            .args(["-Pk"])
            .arg(path)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("could not run df: {e}"))?;
        if !out.status.success() {
            return Err("df failed".into());
        }
        parse_df(&String::from_utf8_lossy(&out.stdout))
    }
}

/// POSIX `df -Pk` output: header line, then `fs 1024-blocks used available capacity mount`.
pub fn parse_df(text: &str) -> Result<u64, String> {
    let line = text.lines().nth(1).ok_or("df printed no data line")?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    let avail = cols.get(3).ok_or("df line has too few columns")?;
    let kb: u64 = avail
        .parse()
        .map_err(|_| format!("df available column is not a number: {avail:?}"))?;
    kb.checked_mul(1024)
        .ok_or_else(|| "df value overflows".to_string())
}
