//! Configuration and secret commands (Milestone C), and the preflight that turns a resolved
//! configuration into what a deployment needs.
//!
//! * `config-set`, `secret-set`, `config-unset` write definitions (append-only records in the same
//!   SQLite store); `config-defs`, `config-show`, `config-diff` read them. None of them prints a
//!   secret value: a secret is shown as `id@version`, and `secret-set` takes its value only from
//!   standard input (never argv, never an environment variable), refusing an interactive terminal.
//! * [`prepare`] resolves the effective configuration for one deployment and fetches **every** secret it
//!   (and, for a replacement, the release it may have to restore) needs from the secret store. If any is
//!   unavailable it fails with the key named, before the journal is written, anything is built, or any
//!   container is touched.

use crate::args::Parsed;
use crate::store::{self, Kind, Runtime};
use crate::{exit, Fail, Services};
use harpoon_deploy::config::{
    self, ConfigRecord, ConfigState, Def, Entry, Flags, Scope, SecretSaved, Setting, Snapshot,
    Value,
};
use harpoon_deploy::engine::Journal;
use harpoon_deploy::ids::{ApplicationId, DeploymentId};
use harpoon_deploy::logs::Redactor;
use harpoon_deploy::secrets::{
    SecretBundle, SecretError, SecretRef, SecretValue, MAX_SECRET_BYTES,
};
use harpoon_deploy::time::{Clock, SystemClock};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Read;

type R = Result<i32, Fail>;

fn now_ms() -> u64 {
    SystemClock.now_ms()
}

fn scope_flag(p: &Parsed) -> Result<Scope, Fail> {
    let s = p.value("scope").ok_or_else(|| {
        Fail::usage("--scope is required: global, project:NAME or app:APPLICATION_ID, each optionally followed by @ENVIRONMENT")
    })?;
    Scope::parse(s).map_err(|e| Fail::usage(format!("--scope: {e}")))
}

fn flags(p: &Parsed) -> Flags {
    let (build, runtime) = (p.switch("build"), p.switch("runtime"));
    if !build && !runtime {
        Flags {
            build: false,
            runtime: true,
        }
    } else {
        Flags { build, runtime }
    }
}

fn name_flag(p: &Parsed) -> Result<String, Fail> {
    let n = p
        .value("name")
        .ok_or_else(|| Fail::usage("--name is required"))?;
    config::check_name(n).map_err(Fail::usage)?;
    Ok(n.to_string())
}

/// The kind of an existing runtime, or Docker for a new one (as `deploy` does).
fn kind_of(rt: &Runtime) -> Result<Kind, Fail> {
    Ok(rt.kind()?.unwrap_or(Kind::Docker))
}

fn read_state(rt: &Runtime) -> Result<ConfigState, Fail> {
    Ok(store::read_restored(rt)?
        .map(|r| r.config)
        .unwrap_or_default())
}

fn journal_err(what: &str, e: impl std::fmt::Debug) -> Fail {
    Fail::journal(format!("cannot record {what}: {e:?}"))
}

// ------------------------------------------------------------------ definitions

pub fn config_set(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let (scope, name) = (scope_flag(p)?, name_flag(p)?);
    let value = p.value("value").ok_or_else(|| {
        Fail::usage("--value is required (a Config value is not secret; use `secret-set` for anything sensitive)")
    })?;
    config::check_config_value(&name, value).map_err(Fail::usage)?;
    let flags = flags(p);
    let (mut j, _) = store::open_writer(&rt, kind_of(&rt)?)?;
    j.config(&ConfigRecord::Def(Def {
        scope: scope.clone(),
        name: name.clone(),
        setting: Setting::Set {
            flags,
            value: Value::Config(value.to_string()),
        },
        at_ms: now_ms(),
    }))
    .map_err(|e| journal_err("the definition", e))?;
    let _ = writeln!(
        out,
        "set config {name} at {scope} ({}); new deployments use it, existing releases keep their own snapshot",
        flags.label()
    );
    Ok(exit::OK)
}

pub fn config_unset(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let (scope, name) = (scope_flag(p)?, name_flag(p)?);
    let (mut j, _) = store::open_writer(&rt, kind_of(&rt)?)?;
    j.config(&ConfigRecord::Def(Def {
        scope: scope.clone(),
        name: name.clone(),
        setting: Setting::Unset,
        at_ms: now_ms(),
    }))
    .map_err(|e| journal_err("the definition", e))?;
    let _ = writeln!(out, "unset {name} at {scope}: it no longer inherits from a less specific scope for new deployments");
    Ok(exit::OK)
}

/// Read the secret from standard input only. One trailing line break is dropped.
fn read_secret(svc: &mut Services) -> Result<SecretValue, Fail> {
    if svc.stdin_is_terminal {
        return Err(Fail::usage(
            "secret-set reads the value from standard input and refuses an interactive terminal (it would echo); pipe or redirect it, for example `read -rs V; printf %s \"$V\" | harpoon-deploy secret-set ...`",
        ));
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

/// Save a new secret version (the first save, or a rotation) and define the name at the scope as
/// referencing it. The Keychain write comes first, so no record ever names an item that was not saved;
/// if the record then cannot be written the item is removed again.
pub fn secret_set(p: &Parsed, svc: &mut Services, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let (scope, name) = (scope_flag(p)?, name_flag(p)?);
    let flags = flags(p);
    let value = read_secret(svc)?;
    let (mut j, restored) = store::open_writer(&rt, kind_of(&rt)?)?;
    let id = config::secret_id(&scope, &name);
    let first = restored.config.next_version(&id);
    let mut version = first;
    // A crashed earlier save may have left an item no record names; versions are immutable, so skip it.
    let r = loop {
        let r = SecretRef::new(&id, version);
        match svc.secrets.put(&r, &value) {
            Ok(()) => break r,
            Err(SecretError::Duplicate) if version < first + 8 => version += 1,
            Err(e) => {
                return Err(Fail::refused(format!(
                    "the secret {name} was not saved: {e}. Nothing was recorded."
                )))
            }
        }
    };
    let recorded = j
        .config(&ConfigRecord::SecretSaved(SecretSaved {
            id: id.clone(),
            version: r.version,
            at_ms: now_ms(),
        }))
        .and_then(|()| {
            j.config(&ConfigRecord::Def(Def {
                scope: scope.clone(),
                name: name.clone(),
                setting: Setting::Set {
                    flags,
                    value: Value::Secret(r.clone()),
                },
                at_ms: now_ms(),
            }))
        });
    if let Err(e) = recorded {
        let _ = svc.secrets.delete(&r);
        return Err(journal_err("the secret reference", e));
    }
    let _ = writeln!(
        out,
        "saved secret {name} at {scope} ({}) as {r}{}. The value is stored only in the Keychain and is never shown again",
        flags.label(),
        if r.version > 1 { " (a rotation: earlier versions are kept for the releases that use them)" } else { "" }
    );
    if value.len() < 8 {
        let _ = writeln!(
            out,
            "warning: a value this short will mask many unrelated characters wherever Harpoon redacts output (accepted: the alternative is leaving it unprotected)"
        );
    }
    Ok(exit::OK)
}

pub fn config_defs(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let only = p
        .value("scope")
        .map(Scope::parse)
        .transpose()
        .map_err(|e| Fail::usage(format!("--scope: {e}")))?;
    let state = read_state(&rt)?;
    let mut n = 0;
    for ((scope, name), d) in state.current() {
        if only.as_ref().is_some_and(|s| s != scope) {
            continue;
        }
        n += 1;
        let _ = match &d.setting {
            Setting::Unset => writeln!(out, "{scope}  {name}  UNSET (blocks inheritance)"),
            Setting::Set {
                flags,
                value: Value::Config(v),
            } => {
                writeln!(out, "{scope}  {name}  config [{}] = {v:?}", flags.label())
            }
            Setting::Set {
                flags,
                value: Value::Secret(r),
            } => {
                writeln!(
                    out,
                    "{scope}  {name}  secret [{}] {r} (value not readable)",
                    flags.label()
                )
            }
        };
    }
    if n == 0 {
        let _ = writeln!(out, "no configuration is defined");
    }
    Ok(exit::OK)
}

// ------------------------------------------------------------------ show and diff

fn resolve_from_flags(
    p: &Parsed,
    state: &ConfigState,
) -> Result<(Vec<Entry>, ApplicationId), Fail> {
    let app: ApplicationId = p
        .value("app")
        .ok_or_else(|| Fail::usage("--app APPLICATION_ID is required (or --deployment DEP to show a recorded snapshot)"))?
        .parse()
        .map_err(|e| Fail::usage(format!("--app is not an application id: {e}")))?;
    let entries = state
        .resolve(p.value("project"), &app, p.value("environment"))
        .map_err(Fail::config)?;
    Ok((entries, app))
}

pub fn config_show(p: &Parsed, svc: &mut Services, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let state = read_state(&rt)?;
    let entries = if let Some(d) = p.value("deployment") {
        let dep: DeploymentId = d
            .parse()
            .map_err(|e| Fail::usage(format!("--deployment: {e}")))?;
        let s = state.snapshot_of(&dep).ok_or_else(|| {
            Fail::refused(format!(
                "deployment {dep} has no recorded configuration snapshot"
            ))
        })?;
        let _ = writeln!(
            out,
            "configuration snapshot of {dep} (application {}, project {}, environment {}), recorded and immutable:",
            s.app,
            s.project.as_deref().unwrap_or("none"),
            s.environment.as_deref().unwrap_or("none")
        );
        s.entries.clone()
    } else {
        let (e, app) = resolve_from_flags(p, &state)?;
        let _ = writeln!(
            out,
            "effective configuration for {app} if deployed now (not recorded):"
        );
        e
    };
    if entries.is_empty() {
        let _ = writeln!(out, "  (empty)");
    }
    let mut missing = 0;
    for e in &entries {
        let mut line = format!("  {}", config::describe(e, true));
        if p.switch("check-secrets") {
            if let Value::Secret(r) = &e.value {
                match svc.secrets.get(r) {
                    Ok(_) => line.push_str(" — available"),
                    Err(err) => {
                        missing += 1;
                        let _ = write!(line, " — UNAVAILABLE: {err}");
                    }
                }
            }
        }
        let _ = writeln!(out, "{line}");
    }
    let _ = writeln!(
        out,
        "secret values are never shown; Config values are readable by design"
    );
    Ok(if missing > 0 { exit::REFUSED } else { exit::OK })
}

pub fn config_diff(p: &Parsed, out: &mut String) -> R {
    let rt = Runtime::from_flag(p.value("runtime-dir"))?;
    let state = read_state(&rt)?;
    let dep = |flag: &str| -> Result<Option<DeploymentId>, Fail> {
        p.value(flag)
            .map(|d| d.parse().map_err(|e| Fail::usage(format!("--{flag}: {e}"))))
            .transpose()
    };
    let from = dep("from")?.ok_or_else(|| Fail::usage("--from DEPLOYMENT is required"))?;
    let a = state.snapshot_of(&from).ok_or_else(|| {
        Fail::refused(format!(
            "deployment {from} has no recorded configuration snapshot"
        ))
    })?;
    let (new, label) = match dep("to")? {
        Some(to) => {
            let b = state.snapshot_of(&to).ok_or_else(|| {
                Fail::refused(format!(
                    "deployment {to} has no recorded configuration snapshot"
                ))
            })?;
            (b.entries.clone(), format!("the snapshot of {to}"))
        }
        None => {
            let project = p.value("project").or(a.project.as_deref());
            let env = p.value("environment").or(a.environment.as_deref());
            (
                state.resolve(project, &a.app, env).map_err(Fail::config)?,
                "the configuration that would apply now".to_string(),
            )
        }
    };
    let d = config::diff(&a.entries, &new);
    let _ = writeln!(out, "configuration changes from the snapshot of {from} to {label} (names, scopes, flags and versions only):");
    if d.is_empty() {
        let _ = writeln!(out, "  no differences");
    }
    for l in d {
        let _ = writeln!(out, "  {l}");
    }
    Ok(exit::OK)
}

// ------------------------------------------------------------------ the preflight

/// Everything a deployment takes from the resolved configuration, with the secret values in memory only.
pub struct Prepared {
    pub entries: Vec<Entry>,
    pub bundle: SecretBundle,
    pub redactor: Redactor,
    pub project: Option<String>,
    pub environment: Option<String>,
    pub target: Option<String>,
    pub build_args: Vec<(String, String)>,
    pub build_secrets: Vec<(String, SecretRef)>,
    pub runtime_env: Vec<(String, String)>,
    pub runtime_secrets: Vec<(String, SecretRef)>,
    /// A canonical description of the build configuration (no secret values).
    pub build_identity: String,
}

/// Fetch every named reference from the store, once each. The first one that cannot be read stops
/// everything with its name (never its value) in the message. Used directly by `rollback` and
/// `restore`, which resolve the *pinned* versions, never current ones.
pub fn fetch(
    svc: &Services,
    refs: &[(String, SecretRef)],
) -> Result<(SecretBundle, Redactor), Fail> {
    let mut bundle = SecretBundle::new();
    let mut labels: Vec<(String, String)> = Vec::new();
    let mut seen = BTreeSet::new();
    for (name, r) in refs {
        if !seen.insert(r.clone()) {
            continue;
        }
        let v = svc.secrets.get(r).map_err(|e| {
            Fail::refused(format!(
                "secret {name} ({r}) cannot be used: {e}. Nothing was built, started, stopped or changed."
            ))
        })?;
        labels.push((name.clone(), v.expose_str().to_string()));
        bundle.insert(r.clone(), v);
    }
    let pairs: Vec<(&str, &str)> = labels
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    Ok((bundle, Redactor::new(&pairs)))
}

/// Resolve the configuration for `app`, add the command-line values (`--env`, `--build-arg`), and
/// fetch every secret the deployment needs plus `extra` (the pinned secrets of a release that may have
/// to be restored). Pure with respect to the journal and Docker.
pub fn prepare(
    p: &Parsed,
    svc: &Services,
    state: &ConfigState,
    app: &ApplicationId,
    cli_env: &[(String, String)],
    extra: &[(String, SecretRef)],
) -> Result<Prepared, Fail> {
    let project = p.value("project").map(str::to_string);
    let environment = p.value("environment").map(str::to_string);
    let build_arg_flags = harpoon_deploy::envvars::parse_all(p.values("build-arg"))
        .map_err(|e| Fail::config(e.replace("--env", "--build-arg")))?;
    let target = p.value("target").map(str::to_string);
    if let Some(t) = &target {
        let ok = !t.is_empty()
            && t.len() <= 128
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !ok {
            return Err(Fail::config(
                "--target must be a Dockerfile stage name (letters, digits, . _ -)",
            ));
        }
    }
    let mut entries = state
        .resolve(project.as_deref(), app, environment.as_deref())
        .map_err(Fail::config)?;
    entries = config::with_command(
        entries,
        cli_env,
        Flags {
            build: false,
            runtime: true,
        },
    );
    entries = config::with_command(
        entries,
        &build_arg_flags,
        Flags {
            build: true,
            runtime: false,
        },
    );
    config::validate_entries(&entries).map_err(Fail::config)?;

    let build_secrets = config::build_secrets(&entries);
    let runtime_secrets = config::runtime_secrets(&entries);
    let mut refs: Vec<(String, SecretRef)> = build_secrets
        .iter()
        .chain(&runtime_secrets)
        .cloned()
        .collect();
    refs.extend(extra.iter().cloned());
    let (bundle, redactor) = fetch(svc, &refs)?;

    let build_args = config::build_args(&entries);
    let runtime_env = config::runtime_env(&entries);
    let build_identity = format!(
        "dockerfile={};target={};args={};secrets={}",
        p.value("dockerfile").unwrap_or("Dockerfile"),
        target.as_deref().unwrap_or("-"),
        build_args
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&"),
        build_secrets
            .iter()
            .map(|(n, r)| format!("{n}:{r}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    Ok(Prepared {
        entries,
        bundle,
        redactor,
        project,
        environment,
        target,
        build_args,
        build_secrets,
        runtime_env,
        runtime_secrets,
        build_identity,
    })
}

impl Prepared {
    /// True when the run used scoped definitions, a secret, a build argument or a target stage. Only then
    /// does the release pin carry a snapshot and build identity (`v3`); a run with nothing but `--env`
    /// keeps the `v1`/`v2` pin exactly as B13 wrote it.
    pub fn extended(&self) -> bool {
        self.target.is_some()
            || self
                .entries
                .iter()
                .any(|e| e.source.is_some() || e.flags.build || matches!(e.value, Value::Secret(_)))
    }

    /// The (snapshot id, build identity) a release is pinned with, if this run is extended.
    pub fn pin_identity(&self, dep: &DeploymentId) -> (Option<String>, Option<String>) {
        if self.extended() {
            (Some(dep.to_string()), Some(self.build_identity.clone()))
        } else {
            (None, None)
        }
    }

    /// Hand the resolved build and runtime configuration to the Docker backend (references and
    /// non-secret values only; the secret values travel separately, in the backend's bundle).
    pub fn apply_to(
        &self,
        cfg: &mut harpoon_deploy_docker::backend::DockerConfig,
        dep: &DeploymentId,
    ) {
        cfg.env = harpoon_deploy::envvars::docker_form(&self.runtime_env);
        cfg.build_target = self.target.clone();
        cfg.build_args = self.build_args.clone();
        cfg.build_secrets = self.build_secrets.clone();
        cfg.runtime_secrets = self.runtime_secrets.clone();
        (cfg.snapshot_id, cfg.build_identity) = self.pin_identity(dep);
    }

    /// The record that makes this deployment's configuration immutable. Written once, before the build.
    pub fn snapshot(&self, dep: &DeploymentId, app: &ApplicationId) -> ConfigRecord {
        ConfigRecord::Snapshot(Box::new(Snapshot {
            deployment: dep.clone(),
            app: app.clone(),
            project: self.project.clone(),
            environment: self.environment.clone(),
            at_ms: now_ms(),
            entries: self.entries.clone(),
        }))
    }

    /// Names only: what this deployment will use. Never a secret value.
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        let names = |it: &mut dyn Iterator<Item = String>| {
            let s: Vec<String> = it.collect();
            if s.is_empty() {
                "none".to_string()
            } else {
                s.join(", ")
            }
        };
        v.push(format!(
            "configuration: {} variables resolved (project {}, environment {}); build arguments: {}; runtime secrets: {}; build secrets: {}",
            self.entries.len(),
            self.project.as_deref().unwrap_or("none"),
            self.environment.as_deref().unwrap_or("none"),
            names(&mut self.build_args.iter().map(|(k, _)| k.clone())),
            names(&mut self.runtime_secrets.iter().map(|(n, r)| format!("{n}={r}"))),
            names(&mut self.build_secrets.iter().map(|(n, r)| format!("{n}={r}"))),
        ));
        v
    }
}
