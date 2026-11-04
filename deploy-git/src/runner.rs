//! A bounded, host-side Git checkout (spec D7, Milestone D).
//!
//! One system `git` process at a time, with a cleared environment, in a private workspace that is removed
//! when it is dropped:
//!
//! * **Credentials never travel on a command line or in a URL.** An HTTPS token and username are written
//!   to `0600` files in a `0700` directory inside the workspace and handed to Git through an askpass
//!   script that reads them (`GIT_ASKPASS`; only file *paths* are in the environment). An SSH key and the
//!   pinned `known_hosts` line are files too, and `ssh` runs with `StrictHostKeyChecking=yes`, the
//!   pinned file as the only host-key source, no agent and no prompts: a host key that does not match
//!   fails the connection. There is no trust on first use.
//! * **Bounded.** A wall-clock timeout kills the whole process group; the size of what is being fetched
//!   is watched while it arrives and the process is killed when it exceeds the limit; the checked-out
//!   tree is measured too. The fetch is shallow (one commit) unless an exact commit forces more.
//! * **Nothing leaks.** The workspace (credentials, objects, tree) is removed on success, failure,
//!   timeout and unwinding; [`sweep`] removes what a hard kill left. Git's output is redacted with the
//!   credential before it can appear in an error. Hooks, credential helpers, system and global Git
//!   configuration and terminal prompts are disabled. The checked-out tree has no `.git` directory.
//!
//! Submodules are not initialised. The URL, ref and commit are validated by [`harpoon_deploy::git`]
//! before they reach Git, so a value that looks like an option can never be one.

use harpoon_deploy::git::{check_ref_name, is_commit, validate_url, UrlKind};
use harpoon_deploy::logs::Redactor;
use harpoon_deploy::secrets::SecretValue;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const WORKSPACE_PREFIX: &str = "git-ws-";

pub enum Credential<'a> {
    /// A public repository.
    None,
    /// HTTPS: the username is not secret, the token is.
    Token { username: &'a str, token: &'a SecretValue },
    /// SSH: the private key and the pinned `known_hosts` line (a public key, not secret).
    SshKey { key: &'a SecretValue, known_host: &'a str },
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub git: PathBuf,
    pub timeout: Duration,
    /// Largest number of bytes fetched or checked out.
    pub max_bytes: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            git: PathBuf::from("/usr/bin/git"),
            timeout: Duration::from_secs(120),
            max_bytes: 256 * 1000 * 1000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// The request itself is not acceptable (URL, ref, credential kind); nothing ran.
    Invalid(String),
    /// Git could not reach the remote.
    Unavailable(String),
    /// The credential was refused (revoked, wrong, missing).
    AuthFailed(String),
    /// The pinned SSH host key did not match the server's.
    HostKey(String),
    /// The repository, ref or commit does not exist.
    NotFound(String),
    TooLarge {
        limit: u64,
    },
    Timeout,
    Failed(String),
    Io(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::Invalid(m) | GitError::Failed(m) | GitError::Io(m) => f.write_str(m),
            GitError::Unavailable(m) => write!(f, "the repository is unavailable: {m}"),
            GitError::AuthFailed(m) => write!(f, "the credential was refused: {m}"),
            GitError::HostKey(m) => write!(f, "the SSH host key does not match the pinned one: {m}"),
            GitError::NotFound(m) => write!(f, "not found: {m}"),
            GitError::TooLarge { limit } => write!(f, "the repository is larger than the {limit} byte limit"),
            GitError::Timeout => f.write_str("Git ran past its time limit and was killed"),
        }
    }
}

/// A private directory for one operation. Removed when dropped.
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn create(parent: &Path, nonce: &str) -> Result<Workspace, GitError> {
        let io = |e: std::io::Error| GitError::Io(format!("cannot create the Git workspace: {e}"));
        // The nonce becomes one path component: no separators, no dot-dot.
        if nonce.is_empty()
            || nonce == ".."
            || !nonce
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        {
            return Err(GitError::Invalid(
                "the workspace name may only contain letters, digits and _ . -".into(),
            ));
        }
        let root = parent.join(format!("{WORKSPACE_PREFIX}{nonce}"));
        // The directory above may be an ordinary macOS path (spaces, non-ASCII, quotes in names are fine:
        // Git is started with structured arguments, and the one shell-parsed string, GIT_SSH_COMMAND,
        // quotes every path). What cannot be made safe in every consumer is refused: control characters,
        // backslash and double quote (ssh's option parser), `$` and `%` (ssh expands them in paths).
        let Some(text) = root.to_str() else {
            return Err(GitError::Invalid("the workspace path must be valid UTF-8".into()));
        };
        if text
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '\\' | '$' | '%'))
        {
            return Err(GitError::Invalid(
                "the workspace path may not contain control characters or any of \" \\ $ %".into(),
            ));
        }
        std::fs::create_dir_all(parent).map_err(io)?;
        std::fs::DirBuilder::new().mode(0o700).create(&root).map_err(io)?;
        let ws = Workspace { root };
        for d in ["home", "cred", "src"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(ws.root.join(d))
                .map_err(io)?;
        }
        Ok(ws)
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Where the checked-out tree is (no `.git` inside).
    pub fn src(&self) -> PathBuf {
        self.root.join("src")
    }
    fn git_dir(&self) -> PathBuf {
        self.root.join("git")
    }
    fn cred(&self) -> PathBuf {
        self.root.join("cred")
    }
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Workspaces are named `git-ws-<pid>-...` by the CLI. One whose owner process is gone (or whose name
/// carries no pid) is stale; one owned by a live process, including this one, is in use and must never be
/// touched: unlike a build, a checkout outlives the moments the journal's writer lock is held.
fn stale_workspaces(parent: &Path) -> Vec<PathBuf> {
    let alive = |pid: u32| {
        pid == std::process::id()
            || Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(true)
    };
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(parent) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(rest) = name.strip_prefix(WORKSPACE_PREFIX) else {
                continue;
            };
            let owner = rest.split('-').next().and_then(|p| p.parse::<u32>().ok());
            if !owner.is_some_and(alive) {
                out.push(e.path());
            }
        }
    }
    out
}

/// Remove the workspaces of processes that are gone: the residue of a hard kill mid-checkout.
pub fn sweep(parent: &Path) -> usize {
    stale_workspaces(parent)
        .into_iter()
        .filter(|p| std::fs::remove_dir_all(p).is_ok())
        .count()
}

/// How many stale workspaces are on disk under `parent`, without touching them.
pub fn leftovers(parent: &Path) -> usize {
    stale_workspaces(parent).len()
}

// ------------------------------------------------------------------ the command line Git is given

/// The `GIT_SSH_COMMAND` for a workspace: the key and the pinned `known_hosts` are the only inputs.
pub fn ssh_command(cred_dir: &Path) -> String {
    let c = cred_dir.display().to_string();
    let (id, known) = (format!("{c}/id"), format!("{c}/known_hosts"));
    format!(
        "ssh -i {} -o IdentitiesOnly=yes -o IdentityAgent=none -o BatchMode=yes -o PasswordAuthentication=no \
         -o StrictHostKeyChecking=yes -o {} -o GlobalKnownHostsFile=/dev/null \
         -o ConnectTimeout=20",
        sh_word(&id),
        ssh_option("UserKnownHostsFile", &known),
    )
}

fn plain(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '.' | '-'))
}

/// One word for the shell that Git runs `GIT_SSH_COMMAND` through. Ordinary paths are left as they are.
fn sh_word(s: &str) -> String {
    if plain(s) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `-o Name=value` where the value is a path: double-quoted for ssh's own parser when it has anything
/// unusual in it (so a space does not end the value), then quoted for the shell.
fn ssh_option(name: &str, path: &str) -> String {
    if plain(path) {
        format!("{name}={path}")
    } else {
        sh_word(&format!("{name}=\"{path}\""))
    }
}

const ASKPASS: &str = "#!/bin/sh\ncase \"$1\" in\n  [Uu]sername*) cat \"$HARPOON_GIT_USER_FILE\" ;;\n  *) cat \"$HARPOON_GIT_TOKEN_FILE\" ;;\nesac\n";

fn write_private(path: &Path, bytes: &[u8], mode: u32) -> Result<(), GitError> {
    use std::io::Write;
    // a workspace may serve more than one operation: replace what is there, never reuse its permissions
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|e| GitError::Io(format!("cannot write a credential file: {e}")))?;
    f.write_all(bytes)
        .map_err(|e| GitError::Io(format!("cannot write a credential file: {e}")))
}

/// The environment of every Git process, with the credential files written into the workspace.
fn environment(
    ws: &Workspace,
    url_kind: UrlKind,
    cred: &Credential,
    settings: &Settings,
) -> Result<Vec<(String, String)>, GitError> {
    let path = match settings.git.parent() {
        Some(p) if p != Path::new("/usr/bin") && p != Path::new("/bin") => format!("{}:/usr/bin:/bin", p.display()),
        _ => "/usr/bin:/bin".to_string(),
    };
    let mut env = vec![
        ("PATH".to_string(), path),
        ("HOME".to_string(), ws.home().display().to_string()),
        ("XDG_CONFIG_HOME".to_string(), ws.home().display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()),
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ("GIT_ALLOW_PROTOCOL".to_string(), "file:https:http:ssh".to_string()),
        ("LC_ALL".to_string(), "C".to_string()),
    ];
    let ssh = url_kind == UrlKind::Ssh;
    match cred {
        Credential::None => {
            if ssh {
                return Err(GitError::Invalid(
                    "an SSH repository needs a private key and its pinned host key".into(),
                ));
            }
        }
        Credential::Token { username, token } => {
            if ssh {
                return Err(GitError::Invalid("a token cannot be used for an SSH URL".into()));
            }
            let dir = ws.cred();
            write_private(&dir.join("user"), username.as_bytes(), 0o600)?;
            write_private(&dir.join("token"), token.expose(), 0o600)?;
            write_private(&dir.join("askpass.sh"), ASKPASS.as_bytes(), 0o700)?;
            env.push(("GIT_ASKPASS".into(), dir.join("askpass.sh").display().to_string()));
            env.push(("HARPOON_GIT_USER_FILE".into(), dir.join("user").display().to_string()));
            env.push(("HARPOON_GIT_TOKEN_FILE".into(), dir.join("token").display().to_string()));
        }
        Credential::SshKey { key, known_host } => {
            if !ssh {
                return Err(GitError::Invalid("an SSH key cannot be used for an https URL".into()));
            }
            if known_host.trim().is_empty() || known_host.contains('\n') {
                return Err(GitError::Invalid(
                    "the pinned host key must be one known_hosts line".into(),
                ));
            }
            let dir = ws.cred();
            let mut k = key.expose().to_vec();
            if !k.ends_with(b"\n") {
                k.push(b'\n');
            }
            write_private(&dir.join("id"), &k, 0o600)?;
            write_private(
                &dir.join("known_hosts"),
                format!("{}\n", known_host.trim()).as_bytes(),
                0o600,
            )?;
            env.push(("GIT_SSH_COMMAND".into(), ssh_command(&dir)));
        }
    }
    Ok(env)
}

// ------------------------------------------------------------------ running Git

struct Output {
    stdout: String,
}

const CAP: usize = 256 * 1024;

fn read_capped(mut r: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let (mut out, mut buf) = (Vec::new(), [0u8; 8192]);
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            if out.len() < CAP {
                out.extend_from_slice(&buf[..n.min(CAP - out.len())]);
            }
        }
        out
    })
}

pub(crate) fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                total += dir_size(&e.path());
            } else {
                total += m.len();
            }
        }
    }
    total
}

fn kill_group(pid: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn run(
    settings: &Settings,
    env: &[(String, String)],
    args: &[String],
    watch: Option<&Path>,
    redactor: &Redactor,
) -> Result<Output, GitError> {
    let mut cmd = Command::new(&settings.git);
    cmd.env_clear()
        .envs(env.iter().cloned())
        .args(args)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| GitError::Unavailable(format!("cannot start {}: {e}", settings.git.display())))?;
    let (out_t, err_t) = (
        read_capped(child.stdout.take().unwrap()),
        read_capped(child.stderr.take().unwrap()),
    );
    let started = Instant::now();
    let mut status = None;
    let mut verdict: Option<GitError> = None;
    while status.is_none() {
        if let Ok(Some(s)) = child.try_wait() {
            status = Some(s);
            break;
        }
        if started.elapsed() >= settings.timeout {
            verdict = Some(GitError::Timeout);
        } else if let Some(dir) = watch {
            if dir_size(dir) > settings.max_bytes {
                verdict = Some(GitError::TooLarge {
                    limit: settings.max_bytes,
                });
            }
        }
        if verdict.is_some() {
            kill_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let stdout = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).to_string();
    let stderr = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).to_string();
    if let Some(v) = verdict {
        return Err(v);
    }
    let (stdout, stderr) = (redactor.apply(&stdout).0, redactor.apply(&stderr).0);
    match status {
        Some(s) if s.success() => Ok(Output { stdout }),
        _ => Err(classify(&stderr)),
    }
}

fn classify(stderr: &str) -> GitError {
    let tail: String = {
        let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
        let keep = &lines[lines.len().saturating_sub(5)..];
        keep.join(" | ").chars().take(500).collect()
    };
    let has = |needles: &[&str]| needles.iter().any(|n| stderr.contains(n));
    if has(&[
        "Host key verification failed",
        "REMOTE HOST IDENTIFICATION HAS CHANGED",
        "No matching host key",
    ]) {
        GitError::HostKey(tail)
    } else if has(&[
        "Authentication failed",
        "could not read Username",
        "could not read Password",
        "Permission denied (publickey",
        "The requested URL returned error: 401",
        "The requested URL returned error: 403",
        "HTTP Basic: Access denied",
    ]) {
        GitError::AuthFailed(tail)
    } else if has(&[
        "not found",
        "does not appear to be a git repository",
        "couldn't find remote ref",
        "not our ref",
        "Repository not found",
        "unknown revision",
        "invalid object name",
    ]) {
        GitError::NotFound(tail)
    } else if has(&[
        "Could not resolve host",
        "Connection refused",
        "Connection timed out",
        "unable to access",
        "Network is unreachable",
        "Could not read from remote repository",
        "Failed to connect",
    ]) {
        GitError::Unavailable(tail)
    } else {
        GitError::Failed(format!("git failed: {tail}"))
    }
}

fn base_args(ws: &Workspace) -> Vec<String> {
    vec![
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-c".into(),
        "credential.helper=".into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "advice.detachedHead=false".into(),
        format!("--git-dir={}", ws.git_dir().display()),
        format!("--work-tree={}", ws.src().display()),
    ]
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

// ------------------------------------------------------------------ the public operations

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRef {
    /// `refs/heads/main`, `refs/tags/v1`, `refs/tags/v1^{}` (the peeled commit of an annotated tag), `HEAD`.
    pub name: String,
    pub commit: String,
}

/// `git ls-remote`: what the remote's refs point at now. Used by polling.
pub fn ls_remote(
    ws: &Workspace,
    url: &str,
    cred: &Credential,
    settings: &Settings,
    redactor: &Redactor,
) -> Result<Vec<RemoteRef>, GitError> {
    let kind = validate_url(url).map_err(GitError::Invalid)?;
    let env = environment(ws, kind, cred, settings)?;
    let mut args = vec![
        "-c".to_string(),
        "credential.helper=".to_string(),
        "ls-remote".to_string(),
        "--".to_string(),
        url.to_string(),
    ];
    args.splice(0..0, s(&["-c", "core.hooksPath=/dev/null"]));
    let out = run(settings, &env, &args, None, redactor)?;
    let mut refs = Vec::new();
    for line in out.stdout.lines() {
        let Some((sha, name)) = line.split_once('\t') else {
            continue;
        };
        if is_commit(sha) {
            refs.push(RemoteRef {
                name: name.to_string(),
                commit: sha.to_string(),
            });
        }
    }
    Ok(refs)
}

/// The branches in a listing: `("main", commit)`.
pub fn branch_heads(refs: &[RemoteRef]) -> Vec<(String, String)> {
    refs.iter()
        .filter_map(|r| {
            r.name
                .strip_prefix("refs/heads/")
                .map(|b| (b.to_string(), r.commit.clone()))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Branch(String),
    Tag(String),
    /// A full 40- or 64-digit object id; an abbreviation is not accepted.
    Commit(String),
}

#[derive(Debug)]
pub struct Checkout {
    /// The immutable commit the tree is exactly.
    pub commit: String,
    pub src: PathBuf,
    pub bytes: u64,
}

/// Check out one selected revision into the workspace's `src` directory.
pub fn checkout(
    ws: &Workspace,
    url: &str,
    cred: &Credential,
    sel: &Selector,
    settings: &Settings,
    redactor: &Redactor,
) -> Result<Checkout, GitError> {
    let kind = validate_url(url).map_err(GitError::Invalid)?;
    let (refspec, wanted) = match sel {
        Selector::Branch(b) => {
            check_ref_name(b).map_err(GitError::Invalid)?;
            (format!("refs/heads/{b}"), None)
        }
        Selector::Tag(t) => {
            check_ref_name(t).map_err(GitError::Invalid)?;
            (format!("refs/tags/{t}"), None)
        }
        Selector::Commit(c) => {
            if !is_commit(c) {
                return Err(GitError::Invalid(
                    "an exact commit must be the full object id (40 or 64 lower-case hex digits)".into(),
                ));
            }
            (c.clone(), Some(c.clone()))
        }
    };
    let env = environment(ws, kind, cred, settings)?;
    let base = base_args(ws);
    let git = |extra: &[&str]| {
        let mut a = base.clone();
        a.extend(s(extra));
        a
    };
    run(settings, &env, &git(&["init", "-q"]), None, redactor)?;
    let fetch = |extra: &[&str], spec: &str| -> Result<Output, GitError> {
        let mut a = git(&["fetch", "-q", "--no-tags", "--no-recurse-submodules", "--no-auto-gc"]);
        a.extend(s(extra));
        a.extend(["--".to_string(), url.to_string(), spec.to_string()]);
        run(settings, &env, &a, Some(&ws.git_dir()), redactor)
    };
    let first = fetch(&["--depth=1"], &refspec);
    let commit = match (first, &wanted) {
        (Ok(_), None) => {
            let o = run(
                settings,
                &env,
                &git(&["rev-parse", "--verify", "FETCH_HEAD^{commit}"]),
                None,
                redactor,
            )?;
            o.stdout.trim().to_string()
        }
        (Ok(_), Some(c)) => c.clone(),
        // A server that will not serve an arbitrary commit by id: fall back to every branch head, still bounded.
        (Err(GitError::Failed(_) | GitError::NotFound(_)), Some(c)) => {
            fetch(&[], "+refs/heads/*:refs/harpoon/heads/*")?;
            let present = run(
                settings,
                &env,
                &git(&["cat-file", "-e", &format!("{c}^{{commit}}")]),
                None,
                redactor,
            );
            if present.is_err() {
                return Err(GitError::NotFound(format!(
                    "commit {c} is not reachable from any branch of the repository"
                )));
            }
            c.clone()
        }
        (Err(e), _) => return Err(e),
    };
    if !is_commit(&commit) {
        return Err(GitError::Failed("git did not resolve the revision to a commit".into()));
    }
    run(
        settings,
        &env,
        &git(&["checkout", "-q", "-f", "--detach", &commit]),
        None,
        redactor,
    )?;
    let head = run(settings, &env, &git(&["rev-parse", "HEAD"]), None, redactor)?;
    if head.stdout.trim() != commit {
        return Err(GitError::Failed(
            "the checked-out commit is not the requested one".into(),
        ));
    }
    let _ = std::fs::remove_dir_all(ws.git_dir());
    let bytes = dir_size(&ws.src());
    if bytes > settings.max_bytes {
        return Err(GitError::TooLarge {
            limit: settings.max_bytes,
        });
    }
    let _ = std::fs::set_permissions(ws.src(), std::fs::Permissions::from_mode(0o700));
    Ok(Checkout {
        commit,
        src: ws.src(),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_of_git_failures() {
        assert!(matches!(
            classify("Host key verification failed.\nfatal: Could not read from remote repository."),
            GitError::HostKey(_)
        ));
        assert!(matches!(
            classify("fatal: Authentication failed for 'https://x/'"),
            GitError::AuthFailed(_)
        ));
        assert!(matches!(
            classify("fatal: could not read Username for 'https://x': terminal prompts disabled"),
            GitError::AuthFailed(_)
        ));
        assert!(matches!(
            classify("fatal: couldn't find remote ref refs/heads/nope"),
            GitError::NotFound(_)
        ));
        assert!(matches!(
            classify("fatal: unable to access 'https://x/': Could not resolve host: x"),
            GitError::Unavailable(_)
        ));
        assert!(matches!(classify("something odd"), GitError::Failed(_)));
    }

    #[test]
    fn the_ssh_command_pins_the_host_key_and_names_only_paths() {
        let c = ssh_command(Path::new("/w/cred"));
        for must in [
            "StrictHostKeyChecking=yes",
            "UserKnownHostsFile=/w/cred/known_hosts",
            "GlobalKnownHostsFile=/dev/null",
            "IdentitiesOnly=yes",
            "IdentityAgent=none",
            "BatchMode=yes",
            "-i /w/cred/id",
        ] {
            assert!(c.contains(must), "{must} in {c}");
        }
        assert!(!c.contains("StrictHostKeyChecking=no") && !c.contains("accept-new"));
    }
}
