//! Git-to-deployment (Milestone D), attended only.
//!
//! * `repo-add`, `repo-secret`, `repo-list`: register a repository for an application, store its deploy
//!   credential and webhook secret in the Keychain (value on standard input only, as `secret-set` does).
//! * `git-deploy`: check out a branch, tag or exact commit into a bounded private workspace and run the
//!   ordinary `deploy` or `replace` lifecycle on it. The immutable commit is recorded (a `gitsrc` record)
//!   **before** the deployment starts, so every deployment from Git has its source.
//! * `git-trigger`: queue one normalized push event (a CI job that can reach this machine runs this).
//! * `git-run`: execute what is queued. An unpaused application deploys its **newest** queued event and
//!   supersedes the older ones; a paused one waits for `resume` to select an event. Execution is
//!   sequential, so a running build is never interrupted and events that arrive meanwhile queue behind it.
//! * `watch`: a foreground process that polls the remotes (at least every 60 s), listens for signed
//!   webhooks on loopback if asked, and runs the queue. It is "automatic while Harpoon is running": nothing
//!   here is a daemon, and nothing claims unattended operation (Gate U has not passed).
//!
//! A pushed commit seen for the first time by a **new** process is reported as late: it landed while no
//! Harpoon was running (or since the previous run), and it is deployed like any other event.

use crate::args::{self, Parsed, AUTH_FLAG};
use crate::commands::{self, current_release, now_ms};
use crate::store::{self, Kind, Runtime};
use crate::{exit, Fail, Gate, Services};
use harpoon_deploy::config::{ConfigRecord, SecretSaved};
use harpoon_deploy::control::{self, ExecOutcome, ExecRec, Incoming, Ingress, Next};
use harpoon_deploy::engine::Journal;
use harpoon_deploy::git::{
    self as policy, CredKind, GitRecord, RepoRec, SeenRec, SelectorKind, SourceRec, WebhookKind,
};
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator};
use harpoon_deploy::logs::Redactor;
use harpoon_deploy::secrets::{SecretRef, SecretValue, MAX_SECRET_BYTES};
use harpoon_deploy::state::Trigger;
use harpoon_deploy::time::{OsEntropy, SystemClock};
use harpoon_deploy_git as dg;
use std::fmt::Write as _;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

type R = Result<i32, Fail>;

const MIN_INTERVAL_SECS: u64 = 60;
/// Flags a Git command forwards to the `deploy`/`replace` it runs.
const FORWARD_VALUES: &[&str] = &[
    "resources-dir",
    "docker-socket",
    "mgmt-socket",
    "forwarder-pid",
    "forwarder-command",
    "lsof-path",
    AUTH_FLAG,
    "health-timeout-secs",
];
const FORWARD_SWITCHES: &[&str] = &["simulate", "confirm-cutover"];

fn kind_of(rt: &Runtime, p: &Parsed) -> Result<Kind, Fail> {
    Ok(rt.kind()?.unwrap_or(if p.switch("simulate") {
        Kind::Simulated
    } else {
        Kind::Docker
    }))
}

/// A live run is refused up front, before a repository is contacted, when the build's gate is closed.
/// The same gates guard `deploy` and `replace` themselves; this just does not waste a checkout.
fn check_gate(p: &Parsed, gate: &Gate) -> Result<(), Fail> {
    if !p.switch("simulate") && !gate.live_deploy_enabled {
        return Err(Fail::refused(
            "live deployment is disabled in this build (the shipping gate is closed). Nothing was read, checked out or contacted. Use --simulate against a separate runtime directory for an offline run.",
        ));
    }
    Ok(())
}

/// A private checkout workspace under the runtime's scratch directory (created 0700 first).
fn workspace(rt: &Runtime) -> Result<dg::Workspace, Fail> {
    store::private_dir(&rt.scratch_dir(), true)?;
    dg::Workspace::create(&rt.scratch_dir(), &nonce()).map_err(|e| Fail::failed(e.to_string()))
}

fn nonce() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        now_ms(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

fn settings(p: &Parsed) -> Result<dg::Settings, Fail> {
    let mut s = dg::Settings::default();
    if let Some(t) = p.value("git-timeout-secs") {
        s.timeout = Duration::from_secs(
            t.parse()
                .ok()
                .filter(|n| (1..=3600).contains(n))
                .ok_or_else(|| Fail::usage("--git-timeout-secs must be 1..=3600"))?,
        );
    }
    if let Some(m) = p.value("git-max-mb") {
        s.max_bytes = m
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=10_000).contains(n))
            .ok_or_else(|| Fail::usage("--git-max-mb must be 1..=10000"))?
            * 1_000_000;
    }
    Ok(s)
}

fn repo_named(state: &harpoon_deploy_journal::Restored, name: &str) -> Result<RepoRec, Fail> {
    state.git.repos.get(name).cloned().ok_or_else(|| {
        Fail::refused(format!(
            "repository {name:?} is not registered; use `repo-add`"
        ))
    })
}

fn state_of(rt: &Runtime) -> Result<harpoon_deploy_journal::Restored, Fail> {
    store::read_restored(rt)?.ok_or_else(|| {
        Fail::refused("there is no store yet; register a repository with `repo-add` first")
    })
}

// ------------------------------------------------------------------ registration

pub fn repo_add(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let need = |f: &str| {
        p.value(f)
            .ok_or_else(|| Fail::usage(format!("--{f} is required")))
    };
    let url = need("url")?.to_string();
    let kind = policy::validate_url(&url).map_err(Fail::usage)?;
    let username = p.value("username").map(str::to_string);
    let cred = match (kind, &username) {
        (policy::UrlKind::Ssh, _) => CredKind::SshKey,
        (_, Some(_)) => CredKind::Token,
        _ => CredKind::None,
    };
    let webhook = match p.value("webhook") {
        None => None,
        Some("github") => Some(WebhookKind::GitHub),
        Some("gitlab") => Some(WebhookKind::GitLab),
        Some(o) => {
            return Err(Fail::usage(format!(
                "--webhook is github or gitlab, not {o:?}"
            )))
        }
    };
    let (mut journal, restored) = store::open_writer(&rt, kind_of(&rt, p)?)?;
    let app: ApplicationId = match p.value("app") {
        Some(a) => a
            .parse()
            .map_err(|e| Fail::usage(format!("--app is not an application id: {e}")))?,
        None => match restored.git.repos.get(need("name")?) {
            Some(r) => r.app.clone(),
            None => IdGenerator::new(SystemClock, OsEntropy::new())
                .application()
                .map_err(|e| Fail::failed(format!("cannot generate an id: {e}")))?,
        },
    };
    let rec = RepoRec {
        name: need("name")?.to_string(),
        url,
        username,
        cred,
        known_host: p.value("known-host").map(str::to_string),
        webhook,
        app,
        project: p.value("project").map(str::to_string),
        environment: p.value("environment").map(str::to_string),
        branch: p.value("branch").unwrap_or("main").to_string(),
        filters: p.values("filter").to_vec(),
        container_port: need("container-port")?
            .parse()
            .ok()
            .filter(|n| *n != 0)
            .ok_or_else(|| Fail::usage("--container-port is not a port number"))?,
        dockerfile: p.value("dockerfile").map(str::to_string),
        context: p.value("context").map(str::to_string),
        target: p.value("target").map(str::to_string),
        health_path: p.value("health-path").map(str::to_string),
        at_ms: now_ms(),
    };
    rec.validate().map_err(Fail::usage)?;
    let r = GitRecord::Repo(Box::new(rec.clone()));
    let mut st = restored.git.clone();
    st.apply(&r).map_err(Fail::refused)?;
    journal
        .git(&r)
        .map_err(|e| Fail::journal(format!("cannot record the repository: {e:?}")))?;
    let _ =
        writeln!(
        out,
        "registered repository {} ({}) for application {}; production branch {}; credential: {}{}",
        rec.name,
        rec.url,
        rec.app,
        rec.branch,
        rec.cred.as_str(),
        match rec.cred {
            CredKind::None => String::new(),
            _ => format!(" (store it with `repo-secret --name {} --kind credential`)", rec.name),
        }
    );
    if rec.webhook.is_some() {
        let _ = writeln!(
            out,
            "webhook secret: `repo-secret --name {} --kind webhook`",
            rec.name
        );
    }
    Ok(exit::OK)
}

fn read_stdin_secret(svc: &mut Services) -> Result<SecretValue, Fail> {
    if svc.stdin_is_terminal {
        return Err(Fail::usage("repo-secret reads the value from standard input and refuses an interactive terminal; pipe or redirect it"));
    }
    let mut buf = Vec::new();
    svc.stdin
        .take(MAX_SECRET_BYTES as u64 + 2)
        .read_to_end(&mut buf)
        .map_err(|e| Fail::usage(format!("cannot read standard input: {e}")))?;
    if buf.ends_with(b"\n") {
        buf.pop();
        if buf.ends_with(b"\r") {
            buf.pop();
        }
    }
    SecretValue::new(buf).map_err(Fail::usage)
}

pub fn repo_secret(p: &Parsed, svc: &mut Services, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let name = p
        .value("name")
        .ok_or_else(|| Fail::usage("--name is required"))?;
    let what = match p.value("kind") {
        Some("credential") => "credential",
        Some("webhook") => "webhook",
        _ => return Err(Fail::usage("--kind is credential or webhook")),
    };
    let value = read_stdin_secret(svc)?;
    let (mut j, restored) = store::open_writer(&rt, kind_of(&rt, p)?)?;
    let repo = repo_named(&restored, name)?;
    if what == "credential" && repo.cred == CredKind::None {
        return Err(Fail::refused(
            "this repository is registered without a credential",
        ));
    }
    if what == "webhook" && repo.webhook.is_none() {
        return Err(Fail::refused(
            "this repository is registered without a webhook provider (--webhook)",
        ));
    }
    let id = repo.secret_id(what);
    let first = restored.config.next_version(&id);
    let mut version = first;
    let r = loop {
        let r = SecretRef::new(&id, version);
        match svc.secrets.put(&r, &value) {
            Ok(()) => break r,
            Err(harpoon_deploy::secrets::SecretError::Duplicate) if version < first + 8 => {
                version += 1
            }
            Err(e) => {
                return Err(Fail::refused(format!(
                    "the secret was not saved: {e}. Nothing was recorded."
                )))
            }
        }
    };
    if let Err(e) = j.config(&ConfigRecord::SecretSaved(SecretSaved {
        id: id.clone(),
        version: r.version,
        at_ms: now_ms(),
    })) {
        let _ = svc.secrets.delete(&r);
        return Err(Fail::journal(format!(
            "cannot record the secret version: {e:?}"
        )));
    }
    let _ = writeln!(out, "saved the {what} of repository {name} as {r}; the value is stored only in the Keychain and is never shown again");
    Ok(exit::OK)
}

pub fn repo_list(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let Some(state) = store::read_restored(&rt)? else {
        let _ = writeln!(out, "no repository is registered");
        return Ok(exit::OK);
    };
    if state.git.repos.is_empty() {
        let _ = writeln!(out, "no repository is registered");
    }
    for r in state.git.repos.values() {
        let _ = writeln!(
            out,
            "{}  {}  app {}  branch {}{}  credential {}{}",
            r.name,
            r.url,
            r.app,
            r.branch,
            if r.filters.is_empty() {
                String::new()
            } else {
                format!("  filters {}", r.filters.join(","))
            },
            r.cred.as_str(),
            r.webhook
                .map(|w| format!("  webhook {}", w.as_str()))
                .unwrap_or_default()
        );
        for ((repo, rf), s) in &state.git.seen {
            if repo == &r.name {
                let _ = writeln!(out, "  last seen {rf} -> {}", s.commit);
            }
        }
        if let Some(src) = state.git.sources.iter().rev().find(|s| s.repo == r.name) {
            let _ = writeln!(
                out,
                "  last deployment from it: {} at commit {} ({})",
                src.deployment, src.commit, src.trigger
            );
        }
    }
    Ok(exit::OK)
}

// ------------------------------------------------------------------ credentials and a source run

enum Owned {
    None,
    Token(String, SecretValue),
    Ssh(SecretValue, String),
}

impl Owned {
    fn cred(&self) -> dg::Credential<'_> {
        match self {
            Owned::None => dg::Credential::None,
            Owned::Token(u, t) => dg::Credential::Token {
                username: u,
                token: t,
            },
            Owned::Ssh(k, h) => dg::Credential::SshKey {
                key: k,
                known_host: h,
            },
        }
    }
    fn redactor(&self) -> Redactor {
        match self {
            Owned::None => Redactor::none(),
            Owned::Token(_, t) => Redactor::new(&[("GIT_CREDENTIAL", t.expose_str())]),
            Owned::Ssh(k, _) => Redactor::new(&[("GIT_CREDENTIAL", k.expose_str())]),
        }
    }
}

fn latest(
    state: &harpoon_deploy_journal::Restored,
    svc: &Services,
    id: &str,
    what: &str,
) -> Result<SecretValue, String> {
    let v = state
        .config
        .versions
        .get(id)
        .copied()
        .ok_or_else(|| format!("no {what} has been saved; use `repo-secret`"))?;
    let r = SecretRef::new(id, v);
    svc.secrets
        .get(&r)
        .map_err(|e| format!("the {what} ({r}) cannot be used: {e}"))
}

fn credential(
    state: &harpoon_deploy_journal::Restored,
    svc: &Services,
    repo: &RepoRec,
) -> Result<Owned, String> {
    Ok(match repo.cred {
        CredKind::None => Owned::None,
        CredKind::Token => {
            let u = repo
                .username
                .clone()
                .unwrap_or_else(|| "x-access-token".into());
            Owned::Token(
                u,
                latest(state, svc, &repo.secret_id("credential"), "credential")?,
            )
        }
        CredKind::SshKey => Owned::Ssh(
            latest(state, svc, &repo.secret_id("credential"), "credential")?,
            repo.known_host
                .clone()
                .ok_or("the repository has no pinned host key")?,
        ),
    })
}

struct Ran {
    deployment: Option<DeploymentId>,
    code: i32,
}

/// Check out `sel`, record its source, and run `deploy` (first release) or `replace` on it. An error
/// string means nothing was deployed and no deployment exists.
// One flat call site per command; a parameter struct would only rename the arguments.
#[allow(clippy::too_many_arguments)]
fn run_source(
    p: &Parsed,
    gate: &Gate,
    svc: &mut Services,
    rt: &Runtime,
    repo: &RepoRec,
    sel: dg::Selector,
    trigger: &str,
    delivery: Option<&str>,
    out: &mut String,
) -> Result<Ran, String> {
    let set = settings(p).map_err(|f| f.message)?;
    let state = store::read_restored(rt)
        .map_err(|f| f.message)?
        .ok_or("there is no store")?;
    let owned = credential(&state, svc, repo)?;
    let redactor = owned.redactor();
    let ws = workspace(rt).map_err(|f| f.message)?;
    let ck = dg::checkout(&ws, &repo.url, &owned.cred(), &sel, &set, &redactor)
        .map_err(|e| e.to_string())?;
    let (selector, requested) = match &sel {
        dg::Selector::Branch(b) => (SelectorKind::Branch, b.clone()),
        dg::Selector::Tag(t) => (SelectorKind::Tag, t.clone()),
        dg::Selector::Commit(_) => (SelectorKind::Commit, "-".to_string()),
    };
    let dep = IdGenerator::new(SystemClock, OsEntropy::new())
        .deployment()
        .map_err(|e| format!("cannot generate an id: {e}"))?;
    {
        // the source is durable before anything is built
        let (mut j, _) = store::open_writer(rt, kind_of(rt, p).map_err(|f| f.message)?)
            .map_err(|f| f.message)?;
        j.git(&GitRecord::Source(SourceRec {
            deployment: dep.clone(),
            repo: repo.name.clone(),
            url: repo.url.clone(),
            selector,
            requested,
            commit: ck.commit.clone(),
            trigger: trigger.to_string(),
            delivery: delivery.map(str::to_string),
            at_ms: now_ms(),
        }))
        .map_err(|e| format!("cannot record the source: {e:?}"))?;
    }
    let _ = writeln!(
        out,
        "source: {} at commit {} ({} bytes checked out; trigger {trigger})",
        repo.name, ck.commit, ck.bytes
    );

    let state = store::read_restored(rt)
        .map_err(|f| f.message)?
        .ok_or("there is no store")?;
    let replacing = current_release(&state.ledger, &state.deployments, &repo.app).is_some();
    let mut a: Vec<String> = vec![
        if replacing { "replace" } else { "deploy" }.into(),
        "--runtime-dir".into(),
        rt.dir.display().to_string(),
        "--project-dir".into(),
        ck.src.display().to_string(),
        "--app".into(),
        repo.app.to_string(),
        "--container-port".into(),
        repo.container_port.to_string(),
    ];
    let mut opt = |flag: &str, v: &Option<String>| {
        if let Some(v) = v {
            a.push(format!("--{flag}"));
            a.push(v.clone());
        }
    };
    opt("dockerfile", &repo.dockerfile);
    opt("context", &repo.context);
    opt("target", &repo.target);
    opt("health-path", &repo.health_path);
    opt("project", &repo.project);
    opt("environment", &repo.environment);
    for f in FORWARD_VALUES {
        if let Some(v) = p.value(f) {
            a.push(format!("--{f}"));
            a.push(v.to_string());
        }
    }
    for f in FORWARD_SWITCHES {
        // an unconfirmed `replace` only prepares a candidate; `deploy` has no such switch
        if p.switch(f) && !(*f == "confirm-cutover" && !replacing) {
            a.push(format!("--{f}"));
        }
    }
    let inner = args::parse(&a).map_err(|e| format!("internal: {e}"))?;
    let trig = match trigger {
        "poll" | "poll-late" => Trigger::Poll,
        "github" | "gitlab" => Trigger::Webhook,
        _ => Trigger::Manual,
    };
    svc.next = Some((dep.clone(), trig));
    let mut text = String::new();
    let res = if replacing {
        commands::replace(&inner, gate, svc, &mut text)
    } else {
        commands::deploy(&inner, gate, svc, &mut text)
    };
    svc.next = None;
    let code = match res {
        Ok(c) => c,
        Err(f) => {
            let _ = writeln!(text, "{}", f.message);
            f.code
        }
    };
    for l in text.lines() {
        let _ = writeln!(out, "  {l}");
    }
    drop(ws);
    let exists = store::read_restored(rt)
        .ok()
        .flatten()
        .is_some_and(|r| r.deployments.iter().any(|d| d.id == dep));
    Ok(Ran {
        deployment: exists.then_some(dep),
        code,
    })
}

pub fn git_deploy(p: &Parsed, gate: &Gate, svc: &mut Services, out: &mut String) -> R {
    check_gate(p, gate)?;
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let state = state_of(&rt)?;
    let repo = repo_named(
        &state,
        p.value("repo")
            .ok_or_else(|| Fail::usage("--repo NAME is required"))?,
    )?;
    let sel = match (p.value("branch"), p.value("tag"), p.value("commit")) {
        (None, None, None) => dg::Selector::Branch(repo.branch.clone()),
        (Some(b), None, None) => dg::Selector::Branch(b.into()),
        (None, Some(t), None) => dg::Selector::Tag(t.into()),
        (None, None, Some(c)) => dg::Selector::Commit(c.into()),
        _ => {
            return Err(Fail::usage(
                "give at most one of --branch, --tag and --commit",
            ))
        }
    };
    match run_source(p, gate, svc, &rt, &repo, sel, "manual", None, out) {
        Ok(r) => Ok(r.code),
        Err(m) => Err(Fail::failed(format!("nothing was deployed: {m}"))),
    }
}

// ------------------------------------------------------------------ events

pub fn git_trigger(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let need = |f: &str| {
        p.value(f)
            .ok_or_else(|| Fail::usage(format!("--{f} is required")))
    };
    let (mut j, restored) = store::open_writer(&rt, kind_of(&rt, p)?)?;
    let repo = repo_named(&restored, need("repo")?)?;
    let revision = need("revision")?;
    if !policy::is_commit(revision) {
        return Err(Fail::usage(
            "--revision must be the full commit id (40 or 64 lower-case hex digits)",
        ));
    }
    let ref_name = p
        .value("ref")
        .map(str::to_string)
        .unwrap_or_else(|| format!("refs/heads/{}", repo.branch));
    let Some(branch) = ref_name.strip_prefix("refs/heads/") else {
        let _ = writeln!(
            out,
            "ignored: {ref_name} is not a branch (tags are deployed with `git-deploy --tag`)"
        );
        return Ok(exit::OK);
    };
    if !repo.wants_branch(branch) {
        let _ = writeln!(
            out,
            "ignored: branch {branch} does not match the filters of {}",
            repo.name
        );
        return Ok(exit::OK);
    }
    let delivery = need("delivery")?;
    let mut ctl = restored.control.clone();
    let source = p.value("source").unwrap_or("ci");
    let r = control::queue_event(
        &mut j,
        &mut ctl,
        Incoming {
            app: &repo.app,
            delivery_id: delivery,
            source,
            revision,
            received_ms: now_ms(),
        },
    )
    .map_err(|e| Fail::refused(e.to_string()))?;
    let _ = match r {
        Ingress::Duplicate => writeln!(out, "duplicate delivery {delivery}: already recorded, nothing queued"),
        _ => writeln!(out, "queued {revision} for {} (delivery {delivery}, source {source}); `git-run` or a running `watch` executes it", repo.name),
    };
    Ok(exit::OK)
}

/// Execute what is queued for one application. Returns whether anything ran.
fn drain_app(
    p: &Parsed,
    gate: &Gate,
    svc: &mut Services,
    rt: &Runtime,
    repo: &RepoRec,
    out: &mut String,
) -> Result<bool, Fail> {
    let state = state_of(rt)?;
    let (event, superseded) =
        match control::next_execution(&state.deployments, &state.control, &repo.app) {
            Next::Nothing => return Ok(false),
            Next::Selected(e) => (e, vec![]),
            Next::Newest { chosen, superseded } => (chosen, superseded),
        };
    let _ = writeln!(
        out,
        "executing delivery {} ({}) at {}{}",
        event.rec.delivery,
        event.rec.source,
        event.rec.revision,
        if superseded.is_empty() {
            String::new()
        } else {
            format!("; supersedes {} older queued event(s)", superseded.len())
        }
    );
    let ran = run_source(
        p,
        gate,
        svc,
        rt,
        repo,
        dg::Selector::Commit(event.rec.revision.clone()),
        &event.rec.source,
        Some(&event.rec.delivery),
        out,
    );
    let (outcome, deployment, reason) = match &ran {
        Ok(r) if r.deployment.is_some() => {
            (ExecOutcome::Deployed, r.deployment.clone(), String::new())
        }
        Ok(r) => (
            ExecOutcome::Failed,
            None,
            format!(
                "the deployment was refused before it started (exit {})",
                r.code
            ),
        ),
        Err(m) => (ExecOutcome::Failed, None, m.clone()),
    };
    let reason: String = reason
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect();
    let (mut j, restored) = store::open_writer(rt, kind_of(rt, p)?)?;
    let mut ctl = restored.control.clone();
    control::record_exec(
        &mut j,
        &mut ctl,
        ExecRec {
            app: repo.app.clone(),
            delivery: event.rec.delivery.clone(),
            outcome,
            deployment: deployment.clone(),
            reason: reason.clone(),
            superseded,
            at_ms: now_ms(),
        },
    )
    .map_err(|e| Fail::journal(e.to_string()))?;
    let _ = match (&outcome, &deployment) {
        (ExecOutcome::Deployed, Some(d)) => writeln!(
            out,
            "delivery {} executed as deployment {d}",
            event.rec.delivery
        ),
        _ => writeln!(
            out,
            "delivery {} NOT deployed: {reason}",
            event.rec.delivery
        ),
    };
    Ok(true)
}

fn drain_all(
    p: &Parsed,
    gate: &Gate,
    svc: &mut Services,
    rt: &Runtime,
    only: Option<&str>,
    out: &mut String,
) -> Result<usize, Fail> {
    let state = state_of(rt)?;
    let mut n = 0;
    for repo in state
        .git
        .repos
        .values()
        .filter(|r| only.is_none_or(|o| o == r.name))
    {
        // each pass executes one event; a few passes cover events that arrive meanwhile
        for _ in 0..16 {
            if !drain_app(p, gate, svc, rt, repo, out)? {
                break;
            }
            n += 1;
        }
    }
    Ok(n)
}

pub fn git_run(p: &Parsed, gate: &Gate, svc: &mut Services, out: &mut String) -> R {
    check_gate(p, gate)?;
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let n = drain_all(p, gate, svc, &rt, p.value("repo"), out)?;
    if n == 0 {
        let _ = writeln!(out, "nothing is queued to run (a paused application waits for `resume --newest` or `--revision`)");
    }
    Ok(exit::OK)
}

// ------------------------------------------------------------------ polling

fn poll_repo(
    p: &Parsed,
    svc: &Services,
    rt: &Runtime,
    repo: &RepoRec,
    first_pass: bool,
    out: &mut String,
) -> Result<usize, Fail> {
    let state = state_of(rt)?;
    let owned = credential(&state, svc, repo).map_err(Fail::refused)?;
    let ws = workspace(rt)?;
    let refs = dg::ls_remote(
        &ws,
        &repo.url,
        &owned.cred(),
        &settings(p)?,
        &owned.redactor(),
    )
    .map_err(|e| Fail::failed(format!("polling {}: {e}", repo.name)))?;
    drop(ws);
    let (mut j, restored) = store::open_writer(rt, kind_of(rt, p)?)?;
    let mut ctl = restored.control.clone();
    let mut queued = 0;
    for (branch, sha) in dg::branch_heads(&refs) {
        if !repo.wants_branch(&branch) {
            continue;
        }
        let ref_name = format!("refs/heads/{branch}");
        let key = (repo.name.clone(), ref_name.clone());
        let previous = restored.git.seen.get(&key).map(|s| s.commit.clone());
        if previous.as_deref() == Some(sha.as_str()) {
            continue;
        }
        if let Some(old) = &previous {
            let late = first_pass;
            let delivery = format!("poll:{}:{sha}", repo.name);
            let source = if late { "poll-late" } else { "poll" };
            let r = control::queue_event(
                &mut j,
                &mut ctl,
                Incoming {
                    app: &repo.app,
                    delivery_id: &delivery,
                    source,
                    revision: &sha,
                    received_ms: now_ms(),
                },
            )
            .map_err(|e| Fail::refused(e.to_string()))?;
            if r == Ingress::Held {
                queued += 1;
                let _ = if late {
                    writeln!(out, "LATE: {} branch {branch} moved {old} -> {sha} while Harpoon was not running; queued as {delivery}", repo.name)
                } else {
                    writeln!(
                        out,
                        "{} branch {branch} moved {old} -> {sha}; queued as {delivery}",
                        repo.name
                    )
                };
            }
        } else {
            let _ = writeln!(out, "{} branch {branch} first seen at {sha} (baseline; nothing is deployed for history that already existed)", repo.name);
        }
        // recorded after the event, so a crash in between re-queues the same delivery id: idempotent
        j.git(&GitRecord::Seen(SeenRec {
            repo: repo.name.clone(),
            ref_name,
            commit: sha,
            at_ms: now_ms(),
        }))
        .map_err(|e| Fail::journal(format!("cannot record the observation: {e:?}")))?;
    }
    Ok(queued)
}

/// Queue a verified webhook push (the listener already checked the signature or token).
fn accept_webhook(
    rt: &Runtime,
    p: &Parsed,
    inb: &dg::webhook::Inbound,
    out: &mut String,
) -> Result<(), Fail> {
    let (mut j, restored) = store::open_writer(rt, kind_of(rt, p)?)?;
    let Some(repo) = restored.git.repos.get(&inb.repo) else {
        return Ok(());
    };
    let Some(branch) = inb.event.branch() else {
        let _ = writeln!(
            out,
            "webhook {}: {} is not a branch; ignored",
            inb.event.delivery, inb.event.ref_name
        );
        return Ok(());
    };
    if !repo.wants_branch(branch) {
        let _ = writeln!(
            out,
            "webhook {}: branch {branch} does not match the filters of {}; ignored",
            inb.event.delivery, repo.name
        );
        return Ok(());
    }
    let mut ctl = restored.control.clone();
    let r = control::queue_event(
        &mut j,
        &mut ctl,
        Incoming {
            app: &repo.app,
            delivery_id: &inb.event.delivery,
            source: inb.event.source(),
            revision: &inb.event.revision,
            received_ms: now_ms(),
        },
    );
    match r {
        Ok(Ingress::Held) => {
            let _ = writeln!(
                out,
                "webhook {} ({}): queued {} for {}",
                inb.event.delivery,
                inb.event.source(),
                inb.event.revision,
                repo.name
            );
        }
        Ok(_) => {
            let _ = writeln!(
                out,
                "webhook {}: duplicate delivery, nothing queued",
                inb.event.delivery
            );
        }
        Err(e) => {
            let _ = writeln!(out, "webhook {}: refused ({e})", inb.event.delivery);
        }
    }
    Ok(())
}

pub fn watch(p: &Parsed, gate: &Gate, svc: &mut Services, out: &mut String) -> R {
    check_gate(p, gate)?;
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let once = p.switch("once");
    let interval = match p.value("interval-secs") {
        None => MIN_INTERVAL_SECS,
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|n| *n >= MIN_INTERVAL_SECS)
            .ok_or_else(|| {
                Fail::usage(format!(
                    "--interval-secs must be a number of at least {MIN_INTERVAL_SECS}"
                ))
            })?,
    };
    let iterations: Option<u64> = if once {
        Some(1)
    } else {
        p.value("iterations")
            .map(|v| {
                v.parse()
                    .map_err(|_| Fail::usage("--iterations is a number"))
            })
            .transpose()?
    };
    let state = state_of(&rt)?;
    let repos: Vec<RepoRec> = state
        .git
        .repos
        .values()
        .filter(|r| p.value("repo").is_none_or(|n| n == r.name))
        .cloned()
        .collect();
    if repos.is_empty() {
        return Err(Fail::refused("no repository is registered to watch"));
    }
    // Webhooks: verified on loopback, buffered while a build runs, queued when it finishes.
    let (tx, rx) = std::sync::mpsc::channel();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let mut listener_thread = None;
    if let Some(port) = p.value("webhook-port") {
        let port: u16 = port
            .parse()
            .map_err(|_| Fail::usage("--webhook-port is a port number"))?;
        let mut routes = dg::webhook::Routes::new();
        for r in &repos {
            if let Some(kind) = r.webhook {
                let secret = latest(&state, svc, &r.secret_id("webhook"), "webhook secret")
                    .map_err(Fail::refused)?;
                routes.add(
                    &r.name,
                    if kind == WebhookKind::GitHub {
                        dg::webhook::Provider::GitHub
                    } else {
                        dg::webhook::Provider::GitLab
                    },
                    secret.expose().to_vec(),
                );
            }
        }
        let l = dg::webhook::Listener::bind_loopback(port)
            .map_err(|e| Fail::runtime(format!("cannot listen on 127.0.0.1:{port}: {e}")))?;
        let _ = writeln!(out, "webhook listener on {} (loopback only; signed GitHub and token-verified GitLab pushes)", l.local_addr());
        let flag = stop.clone();
        listener_thread = Some(std::thread::spawn(move || l.serve(&routes, &tx, &flag)));
    }
    let _ = writeln!(
        out,
        "watching {} repositor{} {}: automatic while Harpoon is running; nothing is deployed when this process is not",
        repos.len(),
        if repos.len() == 1 { "y" } else { "ies" },
        if once { "(one pass)".to_string() } else { format!("every {interval} s") }
    );
    let stream = !once;
    let flush = |out: &mut String| {
        if stream {
            print!("{out}");
            out.clear();
        }
    };
    let mut i: u64 = 0;
    let mut code = exit::OK;
    loop {
        for r in &repos {
            match poll_repo(p, svc, &rt, r, i == 0, out) {
                Ok(_) => {}
                Err(f) => {
                    let _ = writeln!(out, "{}: {}", r.name, f.message);
                    code = exit::FAILED;
                }
            }
        }
        // drain: run the queue, take in whatever arrived while that was running, until nothing is left
        let wait_until = Instant::now()
            + Duration::from_secs(if once {
                p.value("webhook-wait-secs")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
            } else {
                interval
            });
        loop {
            while let Ok(inb) = rx.try_recv() {
                accept_webhook(&rt, p, &inb, out)?;
            }
            if drain_all(p, gate, svc, &rt, p.value("repo"), out)? > 0 {
                flush(out);
                continue;
            }
            if Instant::now() >= wait_until {
                break;
            }
            flush(out);
            std::thread::sleep(Duration::from_millis(100));
        }
        i += 1;
        if iterations.is_some_and(|n| i >= n) {
            break;
        }
        // jitter: up to 10% so several machines do not poll in step
        let jitter = (now_ms() % (interval * 100)) / 1000;
        std::thread::sleep(Duration::from_secs(jitter.min(interval / 10)));
    }
    stop.store(true, Ordering::SeqCst);
    if let Some(t) = listener_thread {
        let _ = t.join();
    }
    Ok(code)
}
