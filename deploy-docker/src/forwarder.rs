//! Evidence that the host listener for a published port belongs to Harpoon's port forwarder and is
//! bound to loopback only.
//!
//! How a published guest port reaches the host (audit of `harpoon/Sources/PortForwardManager.swift`,
//! see `docs/results/DEPLOY_B7_FORWARDING_AUDIT.md`): the app polls the guest's Docker for running
//! containers and, for each published TCP port, binds `127.0.0.1:<same port>` in its own process and
//! relays every accepted connection to `<guest ip>:<same port>`. So the proof needed is "exactly one
//! listener on `127.0.0.1:<port>`, and it is the Harpoon app's own process". This module parses
//! `lsof -F` output into that statement. Nothing here has been run against a live host.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub pid: u32,
    pub command: String,
    /// As lsof prints it: `127.0.0.1:38000`, `*:38000`, `[::1]:38000`.
    pub address: String,
}

impl Listener {
    pub fn port(&self) -> Option<u16> {
        self.address
            .rsplit_once(':')
            .and_then(|(_, p)| p.parse().ok())
    }
    pub fn host(&self) -> &str {
        self.address.rsplit_once(':').map(|(h, _)| h).unwrap_or("")
    }
}

/// Parse `lsof -Fpcn` output (`p<pid>`, `c<command>`, `n<address>` lines). Empty output means no
/// listeners. Anything that is not that shape is an error: unreadable evidence is never "fine".
pub fn parse_lsof_f(text: &str) -> Result<Vec<Listener>, String> {
    let mut out = Vec::new();
    let mut pid: Option<u32> = None;
    let mut command: Option<String> = None;
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let (tag, rest) = line.split_at(line.chars().next().map(char::len_utf8).unwrap_or(0));
        match tag {
            "p" => {
                pid = Some(
                    rest.parse()
                        .map_err(|_| format!("line {n}: bad pid {rest:?}"))?,
                );
                command = None;
            }
            "c" => command = Some(rest.to_string()),
            "n" => {
                let (Some(p), Some(c)) = (pid, command.clone()) else {
                    return Err(format!("line {n}: an address before its process"));
                };
                if rest.is_empty() {
                    return Err(format!("line {n}: empty address"));
                }
                out.push(Listener {
                    pid: p,
                    command: c,
                    address: rest.to_string(),
                });
            }
            "f" | "P" | "T" => {}
            "" => return Err(format!("line {n}: empty line")),
            other => return Err(format!("line {n}: unexpected field {other:?}")),
        }
    }
    Ok(out)
}

/// lsof truncates command names (9 characters by default); compare on that basis.
fn command_matches(seen: &str, expected: &str) -> bool {
    let cut = |s: &str| s.chars().take(9).collect::<String>();
    cut(seen) == cut(expected)
}

/// Exactly one listener on the port, on `127.0.0.1`, owned by the expected process.
pub fn verify_forwarder(
    listeners: &[Listener],
    port: u16,
    expected_pid: u32,
    expected_command: &str,
) -> Result<(), Vec<String>> {
    let here: Vec<&Listener> = listeners
        .iter()
        .filter(|l| l.port() == Some(port))
        .collect();
    let mut problems = Vec::new();
    if here.is_empty() {
        problems.push(format!("nothing listens on port {port} on this host (the forwarder has not bound it yet, or lost it)"));
    }
    for l in &here {
        if l.host() != "127.0.0.1" {
            problems.push(format!(
                "pid {} ({}) listens on {}, not on 127.0.0.1: that is not loopback-only",
                l.pid, l.command, l.address
            ));
        }
        if l.pid != expected_pid || !command_matches(&l.command, expected_command) {
            problems.push(format!(
                "pid {} ({}) listens on {}, but the Harpoon forwarder is pid {expected_pid} ({expected_command})",
                l.pid, l.command, l.address
            ));
        }
    }
    if here.len() > 1 {
        problems.push(format!(
            "{} listeners share port {port}; exactly one is required",
            here.len()
        ));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Something that can check a host port. The backend calls it after a healthy response.
pub trait HostListenerCheck {
    fn verify(&self, port: u16) -> Result<(), String>;
}

impl<F: Fn(u16) -> Result<(), String>> HostListenerCheck for F {
    fn verify(&self, port: u16) -> Result<(), String> {
        self(port)
    }
}

/// Runs `lsof -nP -iTCP:<port> -sTCP:LISTEN -Fpcn` (fixed arguments, cleared environment, 5 s limit,
/// read-only) and checks the answer with [`verify_forwarder`].
pub struct LsofCheck {
    pub lsof: PathBuf,
    pub pid: u32,
    pub command: String,
}

impl LsofCheck {
    pub const DEFAULT_PATH: &'static str = "/usr/sbin/lsof";
}

/// Run lsof with fixed arguments, a cleared environment and a 5 s limit; parse its `-F` output.
fn run_lsof(lsof: &std::path::Path, args: &[String]) -> Result<Vec<Listener>, String> {
    let mut child = Command::new(lsof)
        .env_clear()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", lsof.display()))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > Duration::from_secs(5) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("lsof did not finish in 5 s; the host listeners are unverified".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("lsof: {e}")),
        }
    }
    let mut text = String::new();
    use std::io::Read;
    child
        .stdout
        .take()
        .ok_or("lsof printed nothing readable")?
        .take(4 * 1024 * 1024)
        .read_to_string(&mut text)
        .map_err(|e| format!("lsof output: {e}"))?;
    parse_lsof_f(&text).map_err(|e| format!("cannot read lsof output: {e}"))
}

/// Every TCP listener on the host whose port is in `lo..=hi` (read-only; for inventories).
pub fn listeners_in_range(
    lsof: &std::path::Path,
    lo: u16,
    hi: u16,
) -> Result<Vec<Listener>, String> {
    let all = run_lsof(
        lsof,
        &[
            "-nP".into(),
            "-iTCP".into(),
            "-sTCP:LISTEN".into(),
            "-Fpcn".into(),
        ],
    )?;
    let mut v: Vec<Listener> = all
        .into_iter()
        .filter(|l| l.port().is_some_and(|p| (lo..=hi).contains(&p)))
        .collect();
    v.sort_by(|a, b| (a.port(), a.pid, &a.address).cmp(&(b.port(), b.pid, &b.address)));
    Ok(v)
}

impl HostListenerCheck for LsofCheck {
    fn verify(&self, port: u16) -> Result<(), String> {
        let listeners = run_lsof(
            &self.lsof,
            &[
                "-nP".into(),
                format!("-iTCP:{port}"),
                "-sTCP:LISTEN".into(),
                "-Fpcn".into(),
            ],
        )?;
        verify_forwarder(&listeners, port, self.pid, &self.command).map_err(|p| p.join("; "))
    }
}
