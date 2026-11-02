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

impl RepoRec {
    pub fn secret_id(&self, what: &str) -> String {
        format!("git/{}/{what}", self.name)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRecord {
    Repo(Box<RepoRec>),
    Seen(SeenRec),
    Source(SourceRec),
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
                self.repos.insert(x.name.clone(), (**x).clone());
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
}
