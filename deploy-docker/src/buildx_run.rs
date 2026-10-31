//! Runs the **bundled** Buildx and nothing else. The binary path comes from
//! `harpoon_deploy::buildx::resolve`; the process gets a cleared environment plus exactly
//! `harpoon_deploy::buildx::isolated_env` (Harpoon's socket, an empty HOME/DOCKER_CONFIG), so the
//! user's shell, Docker contexts, plugins and credentials cannot reach it.

use harpoon_deploy::buildx::{isolated_env, BuildxBinary, FileKind, FileProbe};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The real filesystem answer for [`harpoon_deploy::buildx::resolve`]. Does not follow symlinks.
pub struct StdFileProbe;

impl FileProbe for StdFileProbe {
    fn kind(&self, path: &Path) -> FileKind {
        match std::fs::symlink_metadata(path) {
            Err(_) => FileKind::Missing,
            Ok(m) if m.file_type().is_symlink() => FileKind::Symlink,
            Ok(m) if m.is_dir() => FileKind::Directory,
            Ok(m) => FileKind::File {
                executable: m.permissions().mode() & 0o111 != 0,
            },
        }
    }
}

pub struct BuildRequest<'a> {
    pub binary: &'a BuildxBinary,
    pub context: &'a Path,
    pub dockerfile: &'a Path,
    pub tag: &'a str,
    pub labels: &'a [(String, String)],
    /// `--iidfile`: where Buildx writes the ID of the image it produced. Must not exist yet.
    pub iidfile: &'a Path,
    pub socket: &'a Path,
    /// Empty directory used as HOME and DOCKER_CONFIG.
    pub scratch: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildRunError {
    Spawn(String),
    Timeout,
    Failed(Option<i32>),
}

const MAX_LINE: usize = 16 * 1024;

pub fn build_args(req: &BuildRequest<'_>) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "build".into(),
        "--load".into(),
        "--progress=plain".into(),
        "--pull=false".into(),
    ];
    for (k, v) in req.labels {
        a.push("--label".into());
        a.push(format!("{k}={v}"));
    }
    a.push("--iidfile".into());
    a.push(req.iidfile.display().to_string());
    a.push("-t".into());
    a.push(req.tag.to_string());
    a.push("-f".into());
    a.push(req.dockerfile.display().to_string());
    a.push(req.context.display().to_string());
    a
}

/// Run the build, feeding each output line (stdout and stderr) to `on_line`. Kills the process
/// when `deadline` passes.
pub fn run_build(
    req: &BuildRequest<'_>,
    deadline: Duration,
    on_line: &mut dyn FnMut(&str),
) -> Result<(), BuildRunError> {
    let started = Instant::now();
    let mut cmd = Command::new(&req.binary.path);
    cmd.env_clear()
        .envs(isolated_env(req.socket, req.scratch))
        .current_dir(req.context)
        .args(build_args(req))
        .process_group(0) // its own group, so a timeout can take the whole process tree down
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| BuildRunError::Spawn(format!("{}: {e}", req.binary.path.display())))?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let mut readers = Vec::new();
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut pipe = pipe;
            let mut buf = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);

    let mut pending: Vec<u8> = Vec::new();
    let emit = |bytes: &[u8], pending: &mut Vec<u8>, on_line: &mut dyn FnMut(&str)| {
        for &b in bytes {
            if b == b'\n' {
                on_line(&String::from_utf8_lossy(pending));
                pending.clear();
            } else if pending.len() < MAX_LINE {
                pending.push(b);
            }
        }
    };
    let mut status = None;
    let mut timed_out = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(chunk) => emit(&chunk, &mut pending, on_line),
            Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
        if status.is_none() {
            if let Ok(Some(s)) = child.try_wait() {
                status = Some(s);
            }
        }
        if status.is_some() {
            // Drain what is left, but do not wait forever for a pipe a grandchild may hold open.
            let drain_until = Instant::now() + Duration::from_millis(300);
            while Instant::now() < drain_until {
                match rx.recv_timeout(Duration::from_millis(25)) {
                    Ok(chunk) => emit(&chunk, &mut pending, on_line),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            break;
        }
        if started.elapsed() >= deadline {
            // Kill the group (Buildx may have children holding our pipes), then the child itself.
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            status = child.wait().ok();
            timed_out = true;
            break;
        }
    }
    if !pending.is_empty() {
        on_line(&String::from_utf8_lossy(&pending));
    }
    // Reader threads end by themselves when the pipes close; never block on them.
    drop(rx);
    drop(readers);
    if timed_out {
        return Err(BuildRunError::Timeout);
    }
    match status {
        Some(s) if s.success() => Ok(()),
        Some(s) => Err(BuildRunError::Failed(s.code())),
        None => Err(BuildRunError::Failed(None)),
    }
}
