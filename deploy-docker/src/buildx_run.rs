//! Runs the **bundled** Buildx and nothing else. The binary path comes from
//! `harpoon_deploy::buildx::resolve`; the process gets a cleared environment plus exactly
//! `harpoon_deploy::buildx::isolated_env` (Harpoon's socket, an empty HOME/DOCKER_CONFIG), so the
//! user's shell, Docker contexts, plugins and credentials cannot reach it.

use harpoon_deploy::buildx::{isolated_env, BuildxBinary, FileKind, FileProbe};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
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
    /// `--target`: the Dockerfile stage to build.
    pub target: Option<&'a str>,
    /// `--build-arg NAME=VALUE`: **non-secret** values only (they are visible in `ps` and persist in
    /// image history). Secrets never come through here.
    pub build_args: &'a [(String, String)],
    /// `--secret id=NAME,src=FILE` for BuildKit: (id, file) of a [`SecretFiles`] set.
    pub secrets: &'a [(String, PathBuf)],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildRunError {
    Spawn(String),
    Timeout,
    Failed(Option<i32>),
}

/// Build secrets as files for BuildKit's `--secret`: a private directory beside (never inside) the
/// build-context snapshot, one `0600` file per secret, removed when this value is dropped, so on
/// success, failure, timeout and panic. A hard kill of the process skips `Drop`; [`SecretFiles::sweep`] removes
/// what such a crash left, at the start of the next build (only one writer exists at a time).
pub struct SecretFiles {
    dir: PathBuf,
    pub files: Vec<(String, PathBuf)>,
}

const SECRET_DIR_PREFIX: &str = "buildsecrets-";

impl SecretFiles {
    /// Remove every build-secret directory under `scratch`: leftovers of a crashed build.
    pub fn sweep(scratch: &Path) -> usize {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(scratch) {
            for e in rd.flatten() {
                if e.file_name()
                    .to_string_lossy()
                    .starts_with(SECRET_DIR_PREFIX)
                    && std::fs::remove_dir_all(e.path()).is_ok()
                {
                    n += 1;
                }
            }
        }
        n
    }

    /// How many build-secret directories are on disk under `scratch`, without touching them (a
    /// read-only `status` uses this to say that an interrupted build left some).
    pub fn leftovers(scratch: &Path) -> usize {
        std::fs::read_dir(scratch)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with(SECRET_DIR_PREFIX)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn create(
        scratch: &Path,
        nonce: &str,
        secrets: &[(String, &[u8])],
    ) -> std::io::Result<SecretFiles> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let dir = scratch.join(format!("{SECRET_DIR_PREFIX}{nonce}"));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let mut out = SecretFiles {
            dir,
            files: Vec::new(),
        };
        for (i, (id, bytes)) in secrets.iter().enumerate() {
            let path = out.dir.join(format!("s{i}"));
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            f.write_all(bytes)?;
            out.files.push((id.clone(), path));
        }
        Ok(out)
    }
}

impl Drop for SecretFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Decode a stream of bytes into text without splitting a multi-byte character across chunks.
fn decode_chunk(carry: &mut Vec<u8>, bytes: &[u8]) -> String {
    carry.extend_from_slice(bytes);
    let mut out = String::new();
    loop {
        match std::str::from_utf8(carry) {
            Ok(s) => {
                out.push_str(s);
                carry.clear();
                return out;
            }
            Err(e) => {
                let (good, rest) = carry.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(good).unwrap_or(""));
                match e.error_len() {
                    // an incomplete character at the end: keep it for the next chunk
                    None => {
                        *carry = rest.to_vec();
                        return out;
                    }
                    Some(n) => {
                        out.push('\u{fffd}');
                        *carry = rest[n..].to_vec();
                    }
                }
            }
        }
    }
}

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
    if let Some(t) = req.target {
        a.push("--target".into());
        a.push(t.to_string());
    }
    for (k, v) in req.build_args {
        a.push("--build-arg".into());
        a.push(format!("{k}={v}"));
    }
    for (id, file) in req.secrets {
        a.push("--secret".into());
        a.push(format!("id={id},src={}", file.display()));
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

/// Run the build, feeding its output (stdout and stderr) to `on_text` as it arrives, in arbitrary
/// chunks with the line breaks intact, so a redactor sees secrets that straddle chunks and lines. Kills
/// the process when `deadline` passes.
pub fn run_build(
    req: &BuildRequest<'_>,
    deadline: Duration,
    on_text: &mut dyn FnMut(&str),
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

    let mut carry: Vec<u8> = Vec::new();
    let emit = |bytes: &[u8], carry: &mut Vec<u8>, on_text: &mut dyn FnMut(&str)| {
        let text = decode_chunk(carry, bytes);
        if !text.is_empty() {
            on_text(&text);
        }
    };
    let mut status = None;
    let mut timed_out = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(chunk) => emit(&chunk, &mut carry, on_text),
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
                    Ok(chunk) => emit(&chunk, &mut carry, on_text),
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
    if !carry.is_empty() {
        on_text(&String::from_utf8_lossy(&carry));
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
