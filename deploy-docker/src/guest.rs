//! Guest-side free space, fail closed.
//!
//! The Docker Engine API reports usage (`/system/df`) but not free space. Harpoon already has a
//! host-to-guest channel that can run a command in the guest: the management socket (host unix
//! socket → vsock 2377 → the guest's `harpoon-mgmt`, one line of JSON in, one line out, `exec` with a
//! literal argv and no shell — the same channel `harpoon exec` and the stage-8 disk gate use). That is
//! the smallest reliable mechanism, and it creates **no Docker object**, so there is nothing to journal.
//!
//! [`MgmtDfProbe`] asks three read-only questions and believes the answer only if they agree:
//! 1. Docker (`GET /info`) names its host, kernel and data root;
//! 2. the management channel's `hostname` and `uname -r` equal Docker's host and kernel — so both
//!    channels reach the *same guest*;
//! 3. `df -B1 <Docker's data root>` on that guest reports the free bytes of the filesystem **that
//!    holds that path**; its mount point must be a path-prefix of the data root.
//!
//! Anything else — a failed call, a non-zero exit, a malformed or ambiguous line, a mismatch — is a
//! refusal. The rule itself: a build proceeds only if the probe positively reports at least
//! `required` free bytes.

use crate::api::DockerApi;
use crate::json::Json;
use crate::preflight::parse_df;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub trait GuestDiskProbe {
    /// Free bytes on the guest filesystem that holds Docker's data.
    fn free_bytes(&self) -> Result<u64, String>;
}

/// Always refuses, naming why. What a live deployment gets until a real probe exists.
pub struct Unavailable(pub String);

impl GuestDiskProbe for Unavailable {
    fn free_bytes(&self) -> Result<u64, String> {
        Err(self.0.clone())
    }
}

/// Free bytes from the output of `df -Pk <path>` run in the guest.
pub fn free_from_df(output: &str) -> Result<u64, String> {
    parse_df(output)
}

pub fn check_guest_free(probe: &dyn GuestDiskProbe, required: u64) -> Result<u64, String> {
    let free = probe
        .free_bytes()
        .map_err(|e| format!("guest free space could not be verified: {e}"))?;
    if free == 0 {
        return Err("the guest reports no free space (or a value that cannot be trusted)".into());
    }
    if free < required {
        return Err(format!(
            "only {} MB free in the guest; {} MB are required (2 GiB reserve plus the build context). Harpoon never frees space by pruning.",
            free / 1_000_000,
            required / 1_000_000
        ));
    }
    Ok(free)
}

// ------------------------------------------------------------------ the management channel

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub exit: i64,
    pub stdout: String,
    pub stderr: String,
}

/// Client of the guest management channel. Only [`MgmtClient::exec`] exists, and
/// [`MgmtDfProbe`] only ever sends three fixed argv shapes through it.
pub struct MgmtClient {
    socket: PathBuf,
    timeout: Duration,
}

const MAX_REPLY: usize = 64 * 1024;

impl MgmtClient {
    pub fn new(socket: impl Into<PathBuf>, timeout: Duration) -> MgmtClient {
        MgmtClient {
            socket: socket.into(),
            timeout,
        }
    }

    /// `{"op":"exec","argv":[…]}\n` → one JSON line `{"exit":N,"stdout":…,"stderr":…}`, then the
    /// guest closes. One line exactly: extra output, no newline, malformed JSON, wrong types, an
    /// oversized reply, a hang past the timeout are all errors.
    pub fn exec(&self, argv: &[&str]) -> Result<ExecResult, String> {
        let deadline = Instant::now() + self.timeout;
        let mut s = UnixStream::connect(&self.socket).map_err(|e| {
            format!(
                "cannot reach the guest management channel at {}: {e}",
                self.socket.display()
            )
        })?;
        let left = |d: Instant| {
            d.checked_duration_since(Instant::now())
                .filter(|l| !l.is_zero())
                .ok_or_else(|| "the guest management channel timed out".to_string())
        };
        let argv_json = Json::Arr(argv.iter().map(|a| Json::Str((*a).to_string())).collect());
        let req = Json::Obj(vec![
            ("op".into(), Json::Str("exec".into())),
            ("argv".into(), argv_json),
        ]);
        let _ = s.set_write_timeout(Some(left(deadline)?));
        s.write_all(format!("{}\n", req.to_json()).as_bytes())
            .map_err(|e| format!("management channel write: {e}"))?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let _ = s.set_read_timeout(Some(left(deadline)?));
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > MAX_REPLY {
                        return Err("the guest management reply is too large".into());
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err("the guest management channel timed out".into())
                }
                Err(e) => return Err(format!("management channel read: {e}")),
            }
        }
        let text = String::from_utf8(buf)
            .map_err(|_| "the guest management reply is not UTF-8".to_string())?;
        let lines: Vec<&str> = text.split('\n').collect();
        // exactly one non-empty line, newline-terminated
        if !text.ends_with('\n') || lines.len() != 2 || lines[0].is_empty() {
            return Err(format!(
                "the guest management reply is not exactly one line ({} bytes)",
                text.len()
            ));
        }
        let j = Json::parse(lines[0])
            .map_err(|e| format!("the guest management reply is not JSON: {e:?}"))?;
        let exit = match j.get("exit") {
            Some(Json::Num(n)) if n.fract() == 0.0 => *n as i64,
            _ => return Err("the guest management reply has no integer exit status".into()),
        };
        let field = |k: &str| match j.get(k) {
            Some(Json::Str(v)) => Ok(v.clone()),
            _ => Err(format!("the guest management reply has no {k} text")),
        };
        Ok(ExecResult {
            exit,
            stdout: field("stdout")?,
            stderr: field("stderr")?,
        })
    }
}

// ------------------------------------------------------------------ df parsing

/// Free bytes from `df -B1 <path>` output, requiring that the line is about `path`'s filesystem.
///
/// Exactly a header and one data line; the data line has at least six whitespace-separated fields
/// of which the last five are `total used available capacity% mountpoint`; numbers are plain
/// digits; `total > 0`; `used + available <= total`; the capacity ends in `%`; and the mount
/// point is `/` or a path-prefix of `path`.
pub fn free_bytes_from_df(output: &str, path: &str) -> Result<u64, String> {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() != 2 {
        return Err(format!(
            "df printed {} lines, expected a header and one data line",
            lines.len()
        ));
    }
    if !lines[0]
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("filesystem")
    {
        return Err("df output has no header".into());
    }
    let f: Vec<&str> = lines[1].split_whitespace().collect();
    if f.len() < 6 {
        return Err(
            "df data line has too few fields (a long device name may have wrapped it)".into(),
        );
    }
    let n = f.len();
    let num = |s: &str, what: &str| -> Result<u64, String> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("df {what} is not a plain number: {s:?}"));
        }
        s.parse().map_err(|_| format!("df {what} is out of range"))
    };
    let (total, used, avail) = (
        num(f[n - 5], "total")?,
        num(f[n - 4], "used")?,
        num(f[n - 3], "available")?,
    );
    if !f[n - 2].ends_with('%') {
        return Err("df capacity column is not a percentage".into());
    }
    let mount = f[n - 1];
    if total == 0 {
        return Err("df reports a zero-size filesystem".into());
    }
    if used.saturating_add(avail) > total {
        return Err("df numbers are inconsistent (used + available exceeds total)".into());
    }
    let covers = mount == "/"
        || path == mount
        || path.starts_with(&format!("{}/", mount.trim_end_matches('/')));
    if !mount.starts_with('/') || !covers {
        return Err(format!(
            "df's filesystem is mounted at {mount:?}, which does not contain {path:?}"
        ));
    }
    Ok(avail)
}

fn plain_absolute_path(p: &str) -> Result<(), String> {
    if !p.starts_with('/')
        || p.len() > 4096
        || p.chars().any(|c| c.is_control())
        || p.split('/').any(|seg| seg == ".." || seg == ".")
    {
        return Err(format!(
            "Docker's data root {p:?} is not a plain absolute path"
        ));
    }
    Ok(())
}

/// Free space of the guest filesystem backing Docker's data, via Docker's `/info` and the
/// management channel. Read-only; creates nothing.
pub struct MgmtDfProbe {
    docker: DockerApi,
    mgmt: MgmtClient,
}

impl MgmtDfProbe {
    pub fn new(
        docker_socket: impl Into<PathBuf>,
        mgmt_socket: impl Into<PathBuf>,
        timeout: Duration,
    ) -> MgmtDfProbe {
        MgmtDfProbe {
            docker: DockerApi::new(docker_socket, timeout),
            mgmt: MgmtClient::new(mgmt_socket, timeout),
        }
    }

    fn run(&self, argv: &[&str]) -> Result<String, String> {
        let r = self.mgmt.exec(argv)?;
        if r.exit != 0 {
            return Err(format!(
                "`{}` exited {} in the guest ({})",
                argv.join(" "),
                r.exit,
                r.stderr.trim()
            ));
        }
        Ok(r.stdout)
    }

    /// Everything the measurement rests on, for diagnostics.
    pub fn measure(&self) -> Result<(u64, String), String> {
        let t = Duration::from_secs(10);
        let info = self
            .docker
            .info(t)
            .map_err(|e| format!("Docker /info: {e}"))?;
        plain_absolute_path(&info.root_dir)?;
        let single = |what: &str, out: String| -> Result<String, String> {
            let v = out.trim().to_string();
            if v.is_empty() || v.contains('\n') || v.chars().any(|c| c.is_control()) {
                return Err(format!("the guest's {what} is empty or not a single line"));
            }
            Ok(v)
        };
        let host = single("hostname", self.run(&["hostname"])?)?;
        if host != info.name {
            return Err(format!(
                "the management channel reached host {host:?} but Docker reports {:?}: they are not the same guest",
                info.name
            ));
        }
        let kernel = single("kernel release", self.run(&["uname", "-r"])?)?;
        if kernel != info.kernel_version {
            return Err(format!(
                "the management channel reports kernel {kernel:?} but Docker reports {:?}: they are not the same guest",
                info.kernel_version
            ));
        }
        let df = self.run(&["df", "-B1", &info.root_dir])?;
        let free = free_bytes_from_df(&df, &info.root_dir)?;
        Ok((
            free,
            format!("{host} ({kernel}), filesystem holding {}", info.root_dir),
        ))
    }
}

impl GuestDiskProbe for MgmtDfProbe {
    fn free_bytes(&self) -> Result<u64, String> {
        self.measure().map(|(f, _)| f)
    }
}
