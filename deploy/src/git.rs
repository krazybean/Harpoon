//! Git sources for deployments (spec D7, Milestone D). Pure records and rules: no process, no network.
//!
//! Three append-only record families share the deployment journal, as B14's and C's do:
//!
//! * **`gitrepo`** — a registered repository: its URL (never with a password), which application it
//!   deploys, branch filters, the Dockerfile settings to deploy it with, what credential kind it needs
//!   (the credential itself is a Keychain secret, see [`crate::secrets`]) and, for SSH, the pinned host key.
//! * **`gitseen`** — the last commit observed on a remote ref by a poll. It is how a push made while no
//!   Harpoon process was running is recognised on the next run (and reported as late).
//! * **`gitsrc`** — the source of one deployment: repository, ref, the immutable commit and what
//!   triggered it. Written before the deployment starts.
//!
//! Events that ask for a deployment are B14's [`crate::control`] events (delivery-id idempotency, held,
//! selected, superseded); the executor that consumes them lives in the CLI. Automatic deployment here is
//! **attended only**: it happens while a developer-started Harpoon process is running.

use crate::ids::{ApplicationId, DeploymentId};
use crate::journal_format::frame;
use std::collections::BTreeMap;

pub const MAX_NAME: usize = 64;
pub const MAX_URL: usize = 512;
pub const MAX_FILTERS: usize = 16;

// ------------------------------------------------------------------ URLs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlKind {
    Https,
    /// `http://` is accepted only for a loopback host (local test servers), never for a real remote.
    HttpLoopback,
    Ssh,
    /// `file:///absolute/path`: a repository on this machine.
    File,
}

/// Validate a repository URL before anything runs it. Refuses an option-looking value, whitespace and
/// control characters, an embedded password (credentials come from the Keychain, never the URL), unknown
/// transports (`ext::`, `git://`, ...), and plain `http` to anything but loopback.
pub fn validate_url(url: &str) -> Result<UrlKind, String> {
    if url.is_empty() || url.len() > MAX_URL {
        return Err(format!("a repository URL must be 1..={MAX_URL} bytes"));
    }
    if url.starts_with('-') {
        return Err("a repository URL cannot start with '-'".into());
    }
    if let Some(c) = url.chars().find(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!(
            "a repository URL cannot contain whitespace or control characters (U+{:04X})",
            c as u32
        ));
    }
    let authority = |rest: &str| -> String { rest.split('/').next().unwrap_or("").to_string() };
    if let Some(rest) = url.strip_prefix("https://") {
        let a = authority(rest);
        if a.contains(':') && a.contains('@') && a.split('@').next().is_some_and(|u| u.contains(':')) {
            return Err("a repository URL cannot contain a password; the credential is stored in the Keychain".into());
        }
        if a.is_empty() || a.starts_with('@') {
            return Err("the URL has no host".into());
        }
        return Ok(UrlKind::Https);
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let a = authority(rest);
        let host = a.rsplit('@').next().unwrap_or("");
        let host = host
            .rsplit_once(':')
            .map(|(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host })
            .unwrap_or(host);
        if a.contains('@') && a.split('@').next().is_some_and(|u| u.contains(':')) {
            return Err("a repository URL cannot contain a password".into());
        }
        if matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
            return Ok(UrlKind::HttpLoopback);
        }
        return Err("plain http is accepted only for a loopback host; use https".into());
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let a = authority(rest);
        if a.contains(':') && a.contains('@') && a.split('@').next().is_some_and(|u| u.contains(':')) {
            return Err("a repository URL cannot contain a password".into());
        }
        if a.is_empty() {
            return Err("the URL has no host".into());
        }
        return Ok(UrlKind::Ssh);
    }
    if let Some(path) = url.strip_prefix("file://") {
        if !path.starts_with('/') || path.contains("..") {
            return Err("a file:// URL needs an absolute path without '..'".into());
        }
        file_url_path(path)?;
        return Ok(UrlKind::File);
    }
    // scp-like: user@host:path
    if let Some((left, path)) = url.split_once(':') {
        if left.contains('@') && !left.contains('/') && !path.is_empty() && !url.contains("://") {
            return Ok(UrlKind::Ssh);
        }
    }
    Err("unsupported repository URL: use https://, ssh://, user@host:path or file:///absolute/path".into())
}

/// The filesystem path a `file://` URL names, decoded exactly once. Git decodes `%XX` in a file URL once,
/// so this is the path Git will open; validating this value (and passing the URL on unchanged) keeps the
/// check and the execution on the same bytes. Refused: a malformed or truncated escape, bytes that are not
/// UTF-8, a decoded control character (NUL and newline included), an encoded `/` (it would hide a
/// separator from the component checks), and a `.` or `..` component. A literal `%` in a name must arrive
/// as `%25`; decoding it once yields a plain file name that is not decoded again.
pub fn file_url_path(path: &str) -> Result<String, String> {
    let bad = |why: &str| Err(format!("a file:// URL path is not acceptable: {why}"));
    let raw = path.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != b'%' {
            out.push(raw[i]);
            i += 1;
            continue;
        }
        let hex = |b: Option<&u8>| b.and_then(|c| (*c as char).to_digit(16));
        let (Some(h), Some(l)) = (hex(raw.get(i + 1)), hex(raw.get(i + 2))) else {
            return bad("a malformed %-escape");
        };
        let byte = (h * 16 + l) as u8;
        if byte == b'/' {
            return bad("an encoded '/'");
        }
        out.push(byte);
        i += 3;
    }
    let Ok(decoded) = String::from_utf8(out) else {
        return bad("it is not valid UTF-8");
    };
    if decoded.chars().any(|c| c.is_control()) {
        return bad("it decodes to a control character");
    }
    if !decoded.starts_with('/') || decoded.split('/').any(|c| c == ".." || c == ".") {
        return bad("it must be absolute, without '.' or '..' components");
    }
    Ok(decoded)
}

/// The host part of an SSH URL (for the pinned host key line).
pub fn ssh_host(url: &str) -> Option<String> {
    let rest = if let Some(r) = url.strip_prefix("ssh://") {
        r.split('/').next()?.to_string()
    } else {
        url.split_once(':')?.0.to_string()
    };
    let host = rest.rsplit('@').next()?;
    let host = host
        .rsplit_once(':')
        .map(|(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host })
        .unwrap_or(host);
    (!host.is_empty()).then(|| host.to_string())
}

/// How a repository is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Https,
    Http,
    Ssh,
}

impl Transport {
    fn name(self) -> &'static str {
        match self {
            Transport::Https => "https",
            Transport::Http => "http",
            Transport::Ssh => "ssh",
        }
    }
    fn default_port(self) -> u16 {
        match self {
            Transport::Https => 443,
            Transport::Http => 80,
            Transport::Ssh => 22,
        }
    }
}

/// Where a repository URL sends its requests (and so any saved credential): the transport, the host and the
/// port, with defaults filled in so `https://h/x` and `https://h:443/x` are the same destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub transport: Transport,
    pub host: String,
    pub port: u16,
}

impl std::fmt::Display for Destination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}:{}", self.transport.name(), self.host, self.port)
    }
}

fn user_ok(u: &str) -> bool {
    !u.is_empty()
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
}

/// A DNS name or dotted address in plain ASCII: labels of letters, digits and inner hyphens, no empty label, no
/// trailing dot. Anything else (percent-escapes, Unicode, `#`, `?`, backslashes, ...) is not a host we can compare.
fn host_ok(h: &str) -> bool {
    h.len() <= 253
        && h.split('.').all(|l| {
            (1..=63).contains(&l.len())
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// `[user@]host[:port]`, or a bracketed IPv6 literal for `host`.
fn authority(a: &str, default: u16) -> Option<(String, u16)> {
    if a.matches('@').count() > 1 {
        return None;
    }
    let hostport = match a.split_once('@') {
        Some((u, h)) if user_ok(u) => h,
        Some(_) => return None,
        None => a,
    };
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let (inner, after) = rest.split_once(']')?;
        if !after.is_empty() && !after.starts_with(':') {
            return None;
        }
        let ok = inner.contains(':') && inner.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.'));
        (
            ok.then(|| format!("[{}]", inner.to_ascii_lowercase()))?,
            after.strip_prefix(':'),
        )
    } else {
        match hostport.split_once(':') {
            Some((h, p)) => (host_ok(h).then(|| h.to_ascii_lowercase())?, Some(p)),
            None => (host_ok(hostport).then(|| hostport.to_ascii_lowercase())?, None),
        }
    };
    let port = match port {
        None => default,
        Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => {
            p.parse::<u16>().ok().filter(|n| *n != 0)?
        }
        Some(_) => return None,
    };
    Some((host, port))
}

/// The destination a repository URL names, or `None` when it cannot be stated unambiguously. It is deliberately
/// stricter than the URL grammar: Git hands URLs to libcurl and OpenSSH, whose readings of an odd authority (a
/// `#` or `?` that ends it early, `%2e`, backslashes, a second `@`) can differ from any other parser's, so
/// anything but plain `[user@]host[:port]` is refused, and so is any URL that is not https, http, ssh or
/// scp-style. A saved credential may only follow a URL whose destination is known and unchanged.
pub fn destination(url: &str) -> Option<Destination> {
    if url.is_empty() || url.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\') {
        return None;
    }
    let schemed = [
        ("https://", Transport::Https),
        ("http://", Transport::Http),
        ("ssh://", Transport::Ssh),
    ]
    .into_iter()
    .find_map(|(prefix, t)| url.strip_prefix(prefix).map(|rest| (t, rest)));
    let (transport, host, port) = if let Some((t, rest)) = schemed {
        let (host, port) = authority(rest.split('/').next()?, t.default_port())?;
        (t, host, port)
    } else if url.contains("://") {
        return None;
    } else {
        // scp-style `[user@]host:path`: a colon before any slash
        let (left, path) = url.split_once(':')?;
        if left.contains('/') || path.is_empty() {
            return None;
        }
        let (host, _) = authority(left, 22)?;
        (Transport::Ssh, host, 22)
    };
    Some(Destination { transport, host, port })
}

/// `*` matches any run of characters, `?` one character. Used for branch filters.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn token(what: &str, v: &str) -> Result<(), String> {
    if v.is_empty() || v.len() > MAX_NAME {
        return Err(format!("{what} must be 1..={MAX_NAME} bytes"));
    }
    if let Some(c) = v
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return Err(format!(
            "{what} contains {c:?}; only letters, digits and . _ - are accepted"
        ));
    }
    Ok(())
}

pub fn check_name(v: &str) -> Result<(), String> {
    token("a repository name", v)
}

/// A full object id: 40 (SHA-1) or 64 (SHA-256) lower-case hex digits.
pub fn is_commit(v: &str) -> bool {
    (v.len() == 40 || v.len() == 64) && v.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// A ref name that is safe to hand to Git as an argument: no leading '-', no whitespace, control
/// characters, `..`, `~^:?*[\` or `@{`.
pub fn check_ref_name(v: &str) -> Result<(), String> {
    let bad = v.is_empty()
        || v.len() > 200
        || v.starts_with('-')
        || v.contains("..")
        || v.contains("@{")
        || v.ends_with('/')
        || v.ends_with(".lock")
        || v.chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'));
    if bad {
        Err(format!("{v:?} is not a usable Git ref name"))
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------------ records

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredKind {
    /// Public repository.
    None,
    /// HTTPS: a username (in the record) and a token (the Keychain secret).
    Token,
    /// SSH: a private key (the Keychain secret) and a pinned host key.
    SshKey,
}

impl CredKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CredKind::None => "none",
            CredKind::Token => "token",
            CredKind::SshKey => "sshkey",
        }
    }
    fn parse(s: &str) -> Result<CredKind, String> {
        match s {
            "none" => Ok(CredKind::None),
            "token" => Ok(CredKind::Token),
            "sshkey" => Ok(CredKind::SshKey),
            _ => Err("unknown credential kind".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookKind {
    GitHub,
    GitLab,
}

impl WebhookKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WebhookKind::GitHub => "github",
            WebhookKind::GitLab => "gitlab",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRec {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub cred: CredKind,
    /// A `known_hosts` line (public host key, not secret) for SSH; required, there is no trust-on-first-use.
    pub known_host: Option<String>,
    pub webhook: Option<WebhookKind>,
    pub app: ApplicationId,
    pub project: Option<String>,
    pub environment: Option<String>,
    /// The production branch, deployed by default.
    pub branch: String,
    /// Branch patterns that trigger automatic deployment; empty means just `branch`.
    pub filters: Vec<String>,
    pub container_port: u16,
    pub dockerfile: Option<String>,
    pub context: Option<String>,
    pub target: Option<String>,
    pub health_path: Option<String>,
    pub at_ms: u64,
}

/// The delivery id of a commit that a poll found. Event de-duplication is per (application, delivery id), so the id
/// must say which incarnation of the application's registration found it: a registration that was removed and
/// claimed again must be able to queue a commit that an earlier incarnation already queued, dismissed or deployed.
///
/// Incarnation 0 (never removed) keeps the original `poll:<name>:<commit>`; later ones are `poll:<name>:r<n>:<commit>`.
/// The `r<n>` segment cannot be a commit id, and names contain no `:`, so the two forms cannot collide. Events already
/// recorded keep the id they were recorded with.
pub fn poll_delivery(name: &str, incarnation: usize, commit: &str) -> String {
    if incarnation == 0 {
        format!("poll:{name}:{commit}")
    } else {
        format!("poll:{name}:r{incarnation}:{commit}")
    }
}

/// The Keychain id of a repository's credential or webhook secret. It depends on the repository's name alone, so it
/// outlives the registration: see [`RemovedRec`].
pub fn secret_id_for(name: &str, what: &str) -> String {
    format!("git/{name}/{what}")
}

impl RepoRec {
    pub fn secret_id(&self, what: &str) -> String {
        secret_id_for(&self.name, what)
    }
    /// Whether a push to `branch` should deploy automatically.
    pub fn wants_branch(&self, branch: &str) -> bool {
        if self.filters.is_empty() {
            branch == self.branch
        } else {
            self.filters.iter().any(|f| glob_match(f, branch))
        }
    }
    pub fn validate(&self) -> Result<UrlKind, String> {
        check_name(&self.name)?;
        let kind = validate_url(&self.url)?;
        check_ref_name(&self.branch)?;
        if self.filters.len() > MAX_FILTERS {
            return Err(format!("at most {MAX_FILTERS} branch filters"));
        }
        for f in &self.filters {
            if f.is_empty() || f.len() > 100 || f.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err(format!("{f:?} is not a usable branch filter"));
            }
        }
        match (kind, self.cred) {
            (UrlKind::Ssh, CredKind::Token) | (UrlKind::Https | UrlKind::HttpLoopback, CredKind::SshKey) => {
                return Err("the credential kind does not match the URL (token for https, sshkey for ssh)".into())
            }
            _ => {}
        }
        if kind == UrlKind::Ssh && self.known_host.is_none() {
            return Err(
                "an SSH repository needs its pinned host key (--known-host): there is no trust on first use".into(),
            );
        }
        if self.container_port == 0 {
            return Err("a container port is required".into());
        }
        for s in [&self.username, &self.project, &self.environment].into_iter().flatten() {
            token("a name", s)?;
        }
        Ok(kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenRec {
    pub repo: String,
    pub ref_name: String,
    pub commit: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorKind {
    Branch,
    Tag,
    Commit,
}

impl SelectorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SelectorKind::Branch => "branch",
            SelectorKind::Tag => "tag",
            SelectorKind::Commit => "commit",
        }
    }
    fn parse(s: &str) -> Result<SelectorKind, String> {
        match s {
            "branch" => Ok(SelectorKind::Branch),
            "tag" => Ok(SelectorKind::Tag),
            "commit" => Ok(SelectorKind::Commit),
            _ => Err("unknown selector".into()),
        }
    }
}

/// The immutable source of one deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRec {
    pub deployment: DeploymentId,
    pub repo: String,
    pub url: String,
    pub selector: SelectorKind,
    /// The branch or tag asked for, or `-` for an exact commit.
    pub requested: String,
    pub commit: String,
    /// `manual`, `poll`, `poll-late`, `github`, `gitlab` or `ci`.
    pub trigger: String,
    pub delivery: Option<String>,
    pub at_ms: u64,
}

/// A registration that was removed: the source link is gone, the application, its deployments and its saved
/// credentials are not. **Readable and replayable only: no command in this build writes it** (see
/// `docs/results/DEPLOY_GIT_REGISTRATION_REMOVAL.md`; a build that does not know this record cannot open a journal
/// that holds one).
///
/// What it keeps, and why:
/// * `name` and `app`: the application stays, and only this name may claim it again (`GitState::apply`);
/// * `registration`: the version (`at_ms`) of the registration removed, so a stale removal cannot apply;
/// * `url` and `cred`: the destination a saved credential was issued for. The secret's id depends on the name alone
///   and survives in the journal, so a later registration of this name must keep that destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedRec {
    pub name: String,
    pub app: ApplicationId,
    pub registration: u64,
    pub url: String,
    pub cred: CredKind,
    pub at_ms: u64,
}

impl RemovedRec {
    pub fn validate(&self) -> Result<(), String> {
        check_name(&self.name)?;
        validate_url(&self.url)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRecord {
    Repo(Box<RepoRec>),
    Seen(SeenRec),
    Source(SourceRec),
    Removed(RemovedRec),
}

// ------------------------------------------------------------------ codec

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<String> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b: Option<Vec<u8>> = (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok())
        .collect();
    String::from_utf8(b?).ok()
}

fn opt(v: &Option<String>) -> String {
    v.as_ref().map(|s| hex(s)).unwrap_or_else(|| "-".into())
}

fn unopt(s: &str) -> Result<Option<String>, String> {
    if s == "-" {
        Ok(None)
    } else {
        unhex(s).map(Some).ok_or_else(|| "a field is not valid".to_string())
    }
}

pub fn encode(r: &GitRecord) -> String {
    let body = match r {
        GitRecord::Repo(x) => format!(
            "gitrepo\t1\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            x.name,
            hex(&x.url),
            opt(&x.username),
            x.cred.as_str(),
            opt(&x.known_host),
            x.webhook.map(WebhookKind::as_str).unwrap_or("-"),
            x.app,
            opt(&x.project),
            opt(&x.environment),
            hex(&x.branch),
            if x.filters.is_empty() {
                "-".to_string()
            } else {
                x.filters.iter().map(|f| hex(f)).collect::<Vec<_>>().join(",")
            },
            x.container_port,
            opt(&x.dockerfile),
            opt(&x.context),
            opt(&x.target),
            opt(&x.health_path),
            x.at_ms
        ),
        GitRecord::Removed(x) => format!(
            "gitdrop\t1\t{}\t{}\t{}\t{}\t{}\t{}",
            x.name,
            x.app,
            x.registration,
            hex(&x.url),
            x.cred.as_str(),
            x.at_ms
        ),
        GitRecord::Seen(s) => format!(
            "gitseen\t1\t{}\t{}\t{}\t{}",
            s.repo,
            hex(&s.ref_name),
            s.commit,
            s.at_ms
        ),
        GitRecord::Source(s) => format!(
            "gitsrc\t1\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            s.deployment,
            s.repo,
            hex(&s.url),
            s.selector.as_str(),
            hex(&s.requested),
            s.commit,
            s.trigger,
            s.delivery.clone().unwrap_or_else(|| "-".into()),
            s.at_ms
        ),
    };
    frame(&body)
}

fn bad<T>(m: impl Into<String>) -> Result<T, String> {
    Err(m.into())
}

pub fn decode(f: &[&str]) -> Result<GitRecord, String> {
    let n = |s: &str, w: &str| s.parse::<u64>().or_else(|_| bad(format!("bad {w}")));
    if f.get(1) != Some(&"1") {
        return bad("unknown git record version");
    }
    match f[0] {
        "gitrepo" => {
            if f.len() != 19 {
                return bad("a repository record has the wrong number of fields");
            }
            let filters = if f[12] == "-" {
                Vec::new()
            } else {
                f[12]
                    .split(',')
                    .map(|x| unhex(x).ok_or("a filter is not valid".to_string()))
                    .collect::<Result<_, _>>()?
            };
            let rec = RepoRec {
                name: f[2].to_string(),
                url: unhex(f[3]).ok_or("the URL is not valid")?,
                username: unopt(f[4])?,
                cred: CredKind::parse(f[5])?,
                known_host: unopt(f[6])?,
                webhook: match f[7] {
                    "-" => None,
                    "github" => Some(WebhookKind::GitHub),
                    "gitlab" => Some(WebhookKind::GitLab),
                    _ => return bad("unknown webhook kind"),
                },
                app: f[8].parse().or_else(|_| bad("bad application id"))?,
                project: unopt(f[9])?,
                environment: unopt(f[10])?,
                branch: unhex(f[11]).ok_or("the branch is not valid")?,
                filters,
                container_port: f[13].parse().or_else(|_| bad("bad container port"))?,
                dockerfile: unopt(f[14])?,
                context: unopt(f[15])?,
                target: unopt(f[16])?,
                health_path: unopt(f[17])?,
                at_ms: n(f[18], "time")?,
            };
            rec.validate()?;
            Ok(GitRecord::Repo(Box::new(rec)))
        }
        "gitdrop" => {
            if f.len() != 8 {
                return bad("a removal record has the wrong number of fields");
            }
            let rec = RemovedRec {
                name: f[2].to_string(),
                app: f[3].parse().or_else(|_| bad("bad application id"))?,
                registration: n(f[4], "registration version")?,
                url: unhex(f[5]).ok_or("the URL is not valid")?,
                cred: CredKind::parse(f[6])?,
                at_ms: n(f[7], "time")?,
            };
            rec.validate()?;
            Ok(GitRecord::Removed(rec))
        }
        "gitseen" => {
            if f.len() != 6 {
                return bad("an observation has the wrong number of fields");
            }
            check_name(f[2])?;
            if !is_commit(f[4]) {
                return bad("an observation names no commit");
            }
            Ok(GitRecord::Seen(SeenRec {
                repo: f[2].to_string(),
                ref_name: unhex(f[3]).ok_or("the ref is not valid")?,
                commit: f[4].to_string(),
                at_ms: n(f[5], "time")?,
            }))
        }
        "gitsrc" => {
            if f.len() != 11 {
                return bad("a source record has the wrong number of fields");
            }
            check_name(f[3])?;
            if !is_commit(f[7]) {
                return bad("a source record names no commit");
            }
            let url = unhex(f[4]).ok_or("the URL is not valid")?;
            validate_url(&url)?;
            Ok(GitRecord::Source(SourceRec {
                deployment: f[2].parse().or_else(|_| bad("bad deployment id"))?,
                repo: f[3].to_string(),
                url,
                selector: SelectorKind::parse(f[5])?,
                requested: unhex(f[6]).ok_or("the requested ref is not valid")?,
                commit: f[7].to_string(),
                trigger: f[8].to_string(),
                delivery: (f[9] != "-").then(|| f[9].to_string()),
                at_ms: n(f[10], "time")?,
            }))
        }
        _ => bad("not a git record"),
    }
}

// ------------------------------------------------------------------ state

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitState {
    /// The latest registration of each repository.
    pub repos: BTreeMap<String, RepoRec>,
    /// The latest observed commit per (repository, ref).
    pub seen: BTreeMap<(String, String), SeenRec>,
    pub sources: Vec<SourceRec>,
    /// Every removal, in the order they happened. Never pruned: a name that is registered again must not escape
    /// what an earlier registration of it bound (its credential's destination, its application's identity).
    pub retired: Vec<RemovedRec>,
}

impl GitState {
    pub fn apply(&mut self, r: &GitRecord) -> Result<(), String> {
        match r {
            GitRecord::Repo(x) => {
                // one application per repository name, and one repository per application
                if let Some(other) = self.repos.values().find(|o| o.name != x.name && o.app == x.app) {
                    return bad(format!(
                        "application {} is already deployed from repository {}",
                        x.app, other.name
                    ));
                }
                // An application whose registration was removed keeps its identity: only the name it was
                // registered under may claim it again.
                if let Some(owner) = self.retired_owner(&x.app) {
                    if owner != x.name {
                        return bad(format!(
                            "application {} belongs to the removed repository {owner}; only that name can claim it again",
                            x.app
                        ));
                    }
                }
                self.repos.insert(x.name.clone(), (**x).clone());
            }
            GitRecord::Removed(x) => {
                let Some(live) = self.repos.get(&x.name) else {
                    return bad(format!(
                        "a removal names repository {}, which is not registered",
                        x.name
                    ));
                };
                if live.app != x.app {
                    return bad(format!(
                        "a removal of repository {} names application {}, which is not its application",
                        x.name, x.app
                    ));
                }
                if live.at_ms != x.registration {
                    return bad(format!(
                        "a removal of repository {} names another version of its registration",
                        x.name
                    ));
                }
                if live.url != x.url || live.cred != x.cred {
                    return bad(format!(
                        "a removal of repository {} does not match the registration it removes",
                        x.name
                    ));
                }
                self.repos.remove(&x.name);
                // observations belong to the registration, not to the name
                self.seen.retain(|(repo, _), _| repo != &x.name);
                self.retired.push(x.clone());
            }
            GitRecord::Seen(s) => {
                if !self.repos.contains_key(&s.repo) {
                    return bad(format!(
                        "an observation names repository {}, which is not registered",
                        s.repo
                    ));
                }
                self.seen.insert((s.repo.clone(), s.ref_name.clone()), s.clone());
            }
            GitRecord::Source(s) => {
                if self.sources.iter().any(|x| x.deployment == s.deployment) {
                    return bad(format!("deployment {} has two recorded sources", s.deployment));
                }
                if !self.repos.contains_key(&s.repo) {
                    return bad(format!("a source names repository {}, which is not registered", s.repo));
                }
                self.sources.push(s.clone());
            }
        }
        Ok(())
    }

    /// The name an application was registered under when its registration was removed (a removed application
    /// belongs to one name for good).
    pub fn retired_owner(&self, app: &ApplicationId) -> Option<&str> {
        self.retired.iter().find(|x| x.app == *app).map(|x| x.name.as_str())
    }

    /// How many times this application's registration has been removed: its **incarnation**. Derived from the full,
    /// immutable removal history (`retired` is never pruned), so every reader agrees, an edit never changes it, and
    /// every removal and explicit reclaim gives the next number. A never-removed application is incarnation 0.
    pub fn incarnation(&self, app: &ApplicationId) -> usize {
        self.retired.iter().filter(|x| x.app == *app).count()
    }

    /// The latest removal of a repository name.
    pub fn retired_name(&self, name: &str) -> Option<&RemovedRec> {
        self.retired.iter().rev().find(|x| x.name == name)
    }

    /// The removal that disconnected an application from its source, while no repository is registered for it.
    pub fn disconnected(&self, app: &ApplicationId) -> Option<&RemovedRec> {
        if self.repo_of_app(app).is_some() {
            return None;
        }
        self.retired.iter().rev().find(|x| x.app == *app)
    }

    pub fn source_of(&self, d: &DeploymentId) -> Option<&SourceRec> {
        self.sources.iter().find(|s| s.deployment == *d)
    }

    pub fn repo_of_app(&self, app: &ApplicationId) -> Option<&RepoRec> {
        self.repos.values().find(|r| r.app == *app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> ApplicationId {
        "app_00000000000000000000000001".parse().unwrap()
    }

    fn repo() -> RepoRec {
        RepoRec {
            name: "shop".into(),
            url: "https://example.com/acme/shop.git".into(),
            username: Some("x-access-token".into()),
            cred: CredKind::Token,
            known_host: None,
            webhook: Some(WebhookKind::GitHub),
            app: app(),
            project: Some("shop".into()),
            environment: Some("prod".into()),
            branch: "main".into(),
            filters: vec!["main".into(), "release/*".into()],
            container_port: 8080,
            dockerfile: Some("docker/Dockerfile".into()),
            context: None,
            target: Some("release".into()),
            health_path: Some("/health".into()),
            at_ms: 5,
        }
    }

    #[test]
    fn file_urls_are_decoded_once_and_checked_on_the_decoded_path() {
        // ordinary macOS names, encoded: spaces, non-ASCII, a literal percent
        for (url, path) in [
            ("file:///Users/x/Harpoon%20UI%20Test", "/Users/x/Harpoon UI Test"),
            ("file:///Users/x/caf%C3%A9", "/Users/x/caf\u{e9}"),
            ("file:///Users/x/100%25", "/Users/x/100%"),
            // decoded once only: %252e is the literal name "%2e", not a dot
            ("file:///a/%252e%252e/b", "/a/%2e%2e/b"),
            ("file:///var/repos/shop.git", "/var/repos/shop.git"),
        ] {
            assert_eq!(validate_url(url), Ok(UrlKind::File), "{url}");
            assert_eq!(
                file_url_path(url.strip_prefix("file://").unwrap()).unwrap(),
                path,
                "{url}"
            );
        }
        for bad in [
            "file:///a/%2e%2e/b",
            "file:///a/%2E%2E/b",
            "file:///a/.%2e/b",
            "file:///a/%2e/b",
            "file:///a/%2fetc",
            "file:///a%2F..%2Fb",
            "file:///a/%00",
            "file:///a/%0a",
            "file:///a/%09b",
            "file:///a/%7f",
            "file:///a/%",
            "file:///a/%4",
            "file:///a/%zz",
            "file:///a/%ff",
            "file:///a/%c3",
            "file:///a/b c",
            "file:///a/b\nc",
            "file://host/a",
            "file://relative",
        ] {
            assert!(validate_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn urls_are_checked_before_anything_runs_them() {
        for ok in [
            "https://github.com/acme/shop.git",
            "http://127.0.0.1:8123/repo.git",
            "http://localhost/repo.git",
            "ssh://git@github.com/acme/shop.git",
            "git@github.com:acme/shop.git",
            "file:///var/repos/shop.git",
        ] {
            assert!(validate_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "-oProxyCommand=evil",
            "--upload-pack=evil",
            "https://user:pass@example.com/r.git",
            "ssh://user:pass@example.com/r.git",
            "http://example.com/r.git",
            "git://example.com/r.git",
            "ext::sh -c evil",
            "ftp://example.com/r",
            "https://exa mple.com/r",
            "https://example.com/r\n",
            "file://relative/path",
            "file:///a/../etc",
            "just-a-name",
        ] {
            assert!(validate_url(bad).is_err(), "{bad:?}");
        }
        assert_eq!(ssh_host("git@github.com:acme/shop.git").as_deref(), Some("github.com"));
        assert_eq!(
            ssh_host("ssh://git@example.com:2222/r.git").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn filters_refs_and_commits() {
        assert!(glob_match("main", "main") && !glob_match("main", "maintenance"));
        assert!(
            glob_match("release/*", "release/1.2")
                && glob_match("feat-?", "feat-a")
                && !glob_match("release/*", "main")
        );
        assert!(glob_match("*", "anything/at/all") && glob_match("a*b*c", "aXXbYYc"));
        let r = repo();
        assert!(r.wants_branch("main") && r.wants_branch("release/2") && !r.wants_branch("dev"));
        let mut only_prod = repo();
        only_prod.filters.clear();
        assert!(only_prod.wants_branch("main") && !only_prod.wants_branch("release/2"));
        for bad in [
            "", "-x", "a..b", "a b", "a~1", "a^", "a:b", "a@{1}", "a/", "x.lock", "a\n",
        ] {
            assert!(check_ref_name(bad).is_err(), "{bad:?}");
        }
        assert!(check_ref_name("release/1.2").is_ok());
        assert!(
            is_commit(&"a".repeat(40))
                && is_commit(&"0".repeat(64))
                && !is_commit(&"A".repeat(40))
                && !is_commit("abc")
        );
    }

    #[test]
    fn registration_rules() {
        let mut r = repo();
        assert!(r.validate().is_ok());
        r.cred = CredKind::SshKey;
        assert!(r.validate().is_err(), "an ssh key for an https URL");
        let mut s = repo();
        s.url = "ssh://git@example.com/r.git".into();
        s.cred = CredKind::SshKey;
        assert!(s.validate().unwrap_err().contains("host key"), "no trust on first use");
        s.known_host = Some("example.com ssh-ed25519 AAAA".into());
        assert!(s.validate().is_ok());
        let mut p = repo();
        p.container_port = 0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn records_round_trip_and_replay_enforces_references() {
        let dep: DeploymentId = "dep_00000000000000000000000010".parse().unwrap();
        let recs = [
            GitRecord::Repo(Box::new(repo())),
            GitRecord::Seen(SeenRec {
                repo: "shop".into(),
                ref_name: "refs/heads/main".into(),
                commit: "a".repeat(40),
                at_ms: 9,
            }),
            GitRecord::Source(SourceRec {
                deployment: dep.clone(),
                repo: "shop".into(),
                url: "https://example.com/acme/shop.git".into(),
                selector: SelectorKind::Branch,
                requested: "main".into(),
                commit: "b".repeat(40),
                trigger: "poll".into(),
                delivery: Some("poll:shop:bbb".into()),
                at_ms: 10,
            }),
        ];
        for r in &recs {
            let line = encode(r);
            let body = line.split_once('\t').unwrap().1;
            let f: Vec<&str> = body.split('\t').collect();
            assert_eq!(&decode(&f).unwrap(), r, "{body}");
        }
        let mut s = GitState::default();
        assert!(
            s.apply(&recs[1]).is_err(),
            "an observation of an unregistered repository"
        );
        for r in &recs {
            s.apply(r).unwrap();
        }
        assert_eq!(s.source_of(&dep).unwrap().commit, "b".repeat(40));
        assert!(s.apply(&recs[2]).is_err(), "one source per deployment");
        let mut other = repo();
        other.name = "other".into();
        assert!(
            s.apply(&GitRecord::Repo(Box::new(other))).is_err(),
            "one repository per application"
        );
        // damage
        let bad_commit = encode(&recs[1]).replace(&"a".repeat(40), "zz");
        let body = bad_commit.split_once('\t').unwrap().1.to_string();
        let f: Vec<&str> = body.split('\t').collect();
        assert!(decode(&f).is_err());
    }

    fn dest(url: &str) -> Option<(Transport, String, u16)> {
        destination(url).map(|d| (d.transport, d.host, d.port))
    }

    #[test]
    fn plain_urls_name_one_destination_with_defaults_filled_in() {
        use Transport::*;
        for (url, want) in [
            ("https://github.com/acme/shop.git", (Https, "github.com", 443)),
            ("https://GitHub.com:443/acme/shop.git", (Https, "github.com", 443)),
            (
                "https://x-access-token@github.com/acme/shop.git",
                (Https, "github.com", 443),
            ),
            (
                "https://git.example.com:8443/acme/shop.git",
                (Https, "git.example.com", 8443),
            ),
            ("https://git.example.com", (Https, "git.example.com", 443)),
            ("http://127.0.0.1:8080/r.git", (Http, "127.0.0.1", 8080)),
            ("http://localhost/r.git", (Http, "localhost", 80)),
            ("http://[::1]:9000/r.git", (Http, "[::1]", 9000)),
            ("ssh://git@Example.com:2222/acme/shop.git", (Ssh, "example.com", 2222)),
            ("ssh://git@example.com/acme/shop.git", (Ssh, "example.com", 22)),
            ("git@example.com:acme/shop.git", (Ssh, "example.com", 22)),
        ] {
            assert_eq!(dest(url), Some((want.0, want.1.to_string(), want.2)), "{url}");
        }
        // the same destination written two ways, and destinations that differ
        for (a, b) in [
            ("https://h.example/a", "https://H.example:443/b"),
            ("git@h.example:a/b.git", "ssh://git@h.example/c.git"),
            ("https://u@h.example/a", "https://h.example/a"),
        ] {
            assert!(destination(a).is_some());
            assert_eq!(destination(a), destination(b), "{a} {b}");
        }
        for (a, b) in [
            ("https://h.example/a", "http://h.example/a"),
            ("https://h.example/a", "https://h.example:8443/a"),
            ("https://h.example/a", "https://h.example.evil.example/a"),
            ("https://h.example/a", "ssh://h.example/a"),
            ("ssh://git@h.example:22/a", "ssh://git@h.example:2222/a"),
            ("http://127.0.0.1:1/a", "https://127.0.0.1/a"),
        ] {
            assert_ne!(destination(a), destination(b), "{a} {b}");
        }
    }

    #[test]
    fn urls_that_could_be_read_two_ways_name_no_destination() {
        for url in [
            // a `#` or `?` ends the authority for libcurl: Git would contact the part before it
            "https://evil.example#@good.example/x.git",
            "https://evil.example?@good.example/x.git",
            "https://evil.example:8443#@good.example/x.git",
            "http://127.0.0.1:9#@good.example/x.git",
            "https://good.example#/x.git",
            "https://good.example?x=1",
            "ssh://evil.example#@good.example/x.git",
            "evil.example#@good.example:x.git",
            // backslashes, percent-escapes, doubled and empty parts
            "https://evil.example\\@good.example/x.git",
            "https://good%2eexample/x.git",
            "https://good.example%2fevil/x.git",
            "https://a@b@good.example/x.git",
            "https://:pw@good.example/x.git",
            "https://u:pw@good.example/x.git",
            "https://@good.example/x.git",
            "https://user@/x.git",
            "https:///x.git",
            "https://",
            // ports
            "https://good.example:/x.git",
            "https://good.example:0/x.git",
            "https://good.example:65536/x.git",
            "https://good.example:+443/x.git",
            "https://good.example:44a/x.git",
            "https://good.example:1:2/x.git",
            // hosts
            "https://good.example./x.git",
            "https://./x.git",
            "https://a..b/x.git",
            "https://-bad.example/x.git",
            "https://bad-.example/x.git",
            "https://b\u{fc}cher.example/x.git",
            "https://[::1/x.git",
            "https://[]/x.git",
            "https://[zz]/x.git",
            "https://[::1]x/x.git",
            // other schemes and shapes
            "file:///Users/x/shop",
            "git://good.example/x.git",
            "ftp://good.example/x.git",
            "ext::sh -c id",
            "HTTPS://good.example/x.git",
            "https://good.example/x.git\n",
            "https://good .example/x.git",
            "good.example/x:y",
            "good.example:",
            "",
            "-oProxyCommand=x@good.example:y",
        ] {
            assert_eq!(destination(url), None, "{url:?}");
        }
        // a path may contain anything harmless: only the authority decides the destination
        assert!(destination("https://good.example/a%20b/c#d?e").is_some());
    }

    // ------------------------------------------------------------------ removal records (readable, not yet written)

    fn removal_of(r: &RepoRec, at_ms: u64) -> RemovedRec {
        RemovedRec {
            name: r.name.clone(),
            app: r.app.clone(),
            registration: r.at_ms,
            url: r.url.clone(),
            cred: r.cred,
            at_ms,
        }
    }

    fn registered(name: &str, app: &str, at_ms: u64) -> RepoRec {
        RepoRec {
            name: name.into(),
            app: app.parse().unwrap(),
            at_ms,
            ..repo()
        }
    }

    const APP_A: &str = "app_00000000000000000000000001";
    const APP_B: &str = "app_00000000000000000000000002";
    const APP_C: &str = "app_00000000000000000000000003";

    fn fields(line: &str) -> Vec<String> {
        line.split('\t').map(str::to_string).collect()
    }

    /// A record as the journal stores it, without its checksum frame.
    fn body_of(r: &GitRecord) -> String {
        encode(r).split_once('\t').unwrap().1.to_string()
    }

    fn decoded(body: &str) -> Result<GitRecord, String> {
        let f = fields(body);
        decode(&f.iter().map(String::as_str).collect::<Vec<_>>())
    }

    #[test]
    fn a_removal_record_round_trips_and_keeps_what_credential_safety_needs() {
        let r = GitRecord::Removed(removal_of(&repo(), 99));
        let line = body_of(&r);
        assert!(line.starts_with("gitdrop\t1\tshop\t"), "{line}");
        assert_eq!(decoded(&line), Ok(r.clone()));
        // the registration it removes, the application and the destination are all in the record
        let GitRecord::Removed(x) = r else { unreachable!() };
        assert_eq!(
            (x.registration, x.app.to_string(), x.url.as_str(), x.cred),
            (
                5,
                APP_A.to_string(),
                "https://example.com/acme/shop.git",
                CredKind::Token
            )
        );
        // and no secret: the record carries no credential, only the kind
        assert!(!line.contains("x-access-token"));
    }

    #[test]
    fn a_malformed_removal_record_is_rejected_not_skipped() {
        let good = body_of(&GitRecord::Removed(removal_of(&repo(), 99)));
        let with = |i: usize, v: &str| {
            let mut f = fields(&good);
            f[i] = v.to_string();
            f.join("\t")
        };
        for (what, line) in [
            ("an unknown version", with(1, "2")),
            ("no version", with(1, "")),
            ("a bad name", with(2, "no spaces")),
            ("an empty name", with(2, "")),
            ("a bad application id", with(3, "app_x")),
            ("a non-numeric registration version", with(4, "x")),
            ("an unhexable URL", with(5, "zz")),
            ("an empty URL", with(5, "")),
            (
                "a URL with a control character",
                with(5, &hex("https://example.com/a\nb")),
            ),
            ("a URL with a password", with(5, &hex("https://u:pw@example.com/x.git"))),
            ("an unknown credential kind", with(6, "oauth")),
            ("a non-numeric time", with(7, "x")),
            ("a missing field", good.rsplit_once('\t').unwrap().0.to_string()),
            ("an extra field", format!("{good}\textra")),
        ] {
            assert!(decoded(&line).is_err(), "{what} must be rejected: {line}");
        }
        // the older record families are unaffected
        assert_eq!(
            decoded(&body_of(&GitRecord::Repo(Box::new(repo())))),
            Ok(GitRecord::Repo(Box::new(repo())))
        );
    }

    #[test]
    fn replaying_a_removal_disconnects_the_source_but_keeps_the_application_and_its_history() {
        let mut st = GitState::default();
        let r = registered("shop", APP_A, 5);
        st.apply(&GitRecord::Repo(Box::new(r.clone()))).unwrap();
        let src = SourceRec {
            deployment: "dep_00000000000000000000000001".parse().unwrap(),
            repo: "shop".into(),
            url: r.url.clone(),
            selector: SelectorKind::Branch,
            requested: "main".into(),
            commit: "a".repeat(40),
            trigger: "manual".into(),
            delivery: None,
            at_ms: 6,
        };
        st.apply(&GitRecord::Source(src.clone())).unwrap();
        for ref_name in ["refs/heads/main", "refs/heads/release"] {
            st.apply(&GitRecord::Seen(SeenRec {
                repo: "shop".into(),
                ref_name: ref_name.into(),
                commit: "b".repeat(40),
                at_ms: 7,
            }))
            .unwrap();
        }
        // another repository's observation must survive
        st.apply(&GitRecord::Repo(Box::new(registered("blog", APP_B, 8))))
            .unwrap();
        st.apply(&GitRecord::Seen(SeenRec {
            repo: "blog".into(),
            ref_name: "refs/heads/main".into(),
            commit: "c".repeat(40),
            at_ms: 9,
        }))
        .unwrap();

        st.apply(&GitRecord::Removed(removal_of(&r, 20))).unwrap();
        assert!(!st.repos.contains_key("shop") && st.repos.contains_key("blog"));
        assert!(st.repo_of_app(&r.app).is_none());
        assert_eq!(
            st.seen.keys().map(|k| k.0.as_str()).collect::<Vec<_>>(),
            vec!["blog"],
            "polling state does not outlive the registration"
        );
        assert_eq!(
            st.sources,
            vec![src.clone()],
            "the recorded source of a deployment is history"
        );
        assert_eq!(st.source_of(&src.deployment), Some(&src));
        assert_eq!(
            st.retired_name("shop").map(|x| x.app.to_string()),
            Some(APP_A.to_string())
        );
        assert_eq!(st.retired_owner(&r.app), Some("shop"));
        assert_eq!(st.disconnected(&r.app).map(|x| x.name.as_str()), Some("shop"));
        assert!(
            st.disconnected(&"app_00000000000000000000000002".parse().unwrap())
                .is_none(),
            "a registered application is not disconnected"
        );
    }

    #[test]
    fn a_removal_that_does_not_match_a_live_registration_does_not_apply() {
        let mut st = GitState::default();
        let r = registered("shop", APP_A, 5);
        let rm = removal_of(&r, 20);
        assert!(st
            .apply(&GitRecord::Removed(rm.clone()))
            .unwrap_err()
            .contains("not registered"));
        st.apply(&GitRecord::Repo(Box::new(r.clone()))).unwrap();
        let before = st.clone();
        for (what, bad_rm, want) in [
            (
                "another application",
                RemovedRec {
                    app: APP_B.parse().unwrap(),
                    ..rm.clone()
                },
                "not its application",
            ),
            (
                "an earlier version",
                RemovedRec {
                    registration: 4,
                    ..rm.clone()
                },
                "another version",
            ),
            (
                "a later version",
                RemovedRec {
                    registration: 6,
                    ..rm.clone()
                },
                "another version",
            ),
            (
                "another URL",
                RemovedRec {
                    url: "https://example.com/other.git".into(),
                    ..rm.clone()
                },
                "does not match",
            ),
            (
                "another credential kind",
                RemovedRec {
                    cred: CredKind::None,
                    ..rm.clone()
                },
                "does not match",
            ),
            (
                "another name",
                RemovedRec {
                    name: "blog".into(),
                    ..rm.clone()
                },
                "not registered",
            ),
        ] {
            let e = st.apply(&GitRecord::Removed(bad_rm)).unwrap_err();
            assert!(e.contains(want), "{what}: {e}");
            assert_eq!(st, before, "{what}: a refused removal changed the state");
        }
        st.apply(&GitRecord::Removed(rm.clone())).unwrap();
        // a second removal of the same registration is stale
        assert!(st
            .apply(&GitRecord::Removed(rm))
            .unwrap_err()
            .contains("not registered"));
        assert_eq!(st.retired.len(), 1);
    }

    #[test]
    fn nothing_can_be_recorded_for_a_removed_registration() {
        let mut st = GitState::default();
        let r = registered("shop", APP_A, 5);
        st.apply(&GitRecord::Repo(Box::new(r.clone()))).unwrap();
        st.apply(&GitRecord::Removed(removal_of(&r, 20))).unwrap();
        // a late poll or a late deployment would make the journal unreadable if it were recorded, so replay says no
        let late_seen = GitRecord::Seen(SeenRec {
            repo: "shop".into(),
            ref_name: "refs/heads/main".into(),
            commit: "d".repeat(40),
            at_ms: 21,
        });
        assert!(st.apply(&late_seen).unwrap_err().contains("not registered"));
        let late_source = GitRecord::Source(SourceRec {
            deployment: "dep_00000000000000000000000009".parse().unwrap(),
            repo: "shop".into(),
            url: r.url.clone(),
            selector: SelectorKind::Branch,
            requested: "main".into(),
            commit: "e".repeat(40),
            trigger: "poll".into(),
            delivery: None,
            at_ms: 22,
        });
        assert!(st.apply(&late_source).unwrap_err().contains("not registered"));
        assert!(st.seen.is_empty() && st.sources.is_empty());
    }

    #[test]
    fn a_removed_application_can_only_be_claimed_again_by_the_name_it_was_registered_under() {
        let mut st = GitState::default();
        let r = registered("shop", APP_A, 5);
        st.apply(&GitRecord::Repo(Box::new(r.clone()))).unwrap();
        st.apply(&GitRecord::Removed(removal_of(&r, 20))).unwrap();
        let other = st
            .apply(&GitRecord::Repo(Box::new(registered("blog", APP_A, 30))))
            .unwrap_err();
        assert!(other.contains("belongs to the removed repository shop"), "{other}");
        assert!(st.repos.is_empty());
        // a fresh application for any name, the original name included, is not a claim
        st.apply(&GitRecord::Repo(Box::new(registered("blog", APP_B, 31))))
            .unwrap();
        st.apply(&GitRecord::Repo(Box::new(registered("shop", APP_C, 32))))
            .unwrap();
        // the original name may claim it again, once it is free
        let mut st2 = GitState::default();
        st2.apply(&GitRecord::Repo(Box::new(r.clone()))).unwrap();
        st2.apply(&GitRecord::Removed(removal_of(&r, 20))).unwrap();
        st2.apply(&GitRecord::Repo(Box::new(registered("shop", APP_A, 40))))
            .unwrap();
        assert_eq!(st2.repo_of_app(&r.app).map(|x| x.at_ms), Some(40));
        assert!(
            st2.disconnected(&r.app).is_none(),
            "claimed again, no longer disconnected"
        );
        assert_eq!(st2.retired_owner(&r.app), Some("shop"), "but still remembered");
    }

    #[test]
    fn repeated_removal_and_registration_cycles_never_forget_what_an_earlier_registration_bound() {
        let mut st = GitState::default();
        let mut t = 10;
        let mut cycle = |st: &mut GitState, app: &str| {
            t += 10;
            let r = registered("shop", app, t);
            st.apply(&GitRecord::Repo(Box::new(r.clone()))).unwrap();
            st.apply(&GitRecord::Seen(SeenRec {
                repo: "shop".into(),
                ref_name: "refs/heads/main".into(),
                commit: "f".repeat(40),
                at_ms: t + 1,
            }))
            .unwrap();
            st.apply(&GitRecord::Removed(removal_of(&r, t + 2))).unwrap();
            r
        };
        let first = cycle(&mut st, APP_A);
        cycle(&mut st, APP_B); // the reused name got a fresh application
        cycle(&mut st, APP_A); // and may claim the first one again
        assert_eq!(st.retired.len(), 3, "every removal stays on record");
        assert_eq!(st.retired_owner(&first.app), Some("shop"));
        assert_eq!(st.retired_owner(&APP_B.parse().unwrap()), Some("shop"));
        assert!(st.seen.is_empty(), "no observation survives a removal");
        assert_eq!(
            st.retired_name("shop").map(|x| x.registration),
            Some(40),
            "the latest removal is the name's last destination"
        );
        // another name still cannot take either application
        for app in [APP_A, APP_B] {
            assert!(
                st.apply(&GitRecord::Repo(Box::new(registered("blog", app, 90))))
                    .is_err(),
                "{app}"
            );
        }
    }

    #[test]
    fn a_journal_written_before_removals_existed_replays_unchanged() {
        // the exact lines older builds wrote for the three families
        let lines = [
            body_of(&GitRecord::Repo(Box::new(repo()))),
            body_of(&GitRecord::Seen(SeenRec {
                repo: "shop".into(),
                ref_name: "refs/heads/main".into(),
                commit: "a".repeat(40),
                at_ms: 6,
            })),
        ];
        let mut st = GitState::default();
        for l in &lines {
            st.apply(&decoded(l).unwrap()).unwrap();
        }
        assert_eq!(st.repos.len(), 1);
        assert_eq!(st.seen.len(), 1);
        assert!(st.retired.is_empty());
        assert!(st.disconnected(&app()).is_none());
    }
}
