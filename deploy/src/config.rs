//! Scoped build and runtime configuration, and the immutable snapshots a release is built from
//! (spec D6, Milestone C).
//!
//! Three append-only record kinds share the deployment journal, exactly as B14's control records do:
//!
//! * **`cfgdef`** — one definition: *set* (a Config value or a Secret reference, with build/runtime
//!   flags) or *unset*, at one scope. The latest definition per (scope, name) is current; older ones
//!   are history.
//! * **`cfgsec`** — a secret version was saved (id, version). Versions are consecutive from 1 and
//!   never reused (they only increase); the value is in the secret store, never here.
//! * **`cfgsnap`** — the **effective configuration of one deployment**, written once, before the
//!   build: each key with its source scope, type, flags and (for a secret) exact version.
//!
//! A snapshot is never recomputed from current definitions, so a historical release can never
//! silently inherit a newer value. Resolution uses D6's order, most specific wins, and an explicit
//! unset at a narrower scope blocks inheritance:
//!
//! ```text
//! application+env > application > project+env > project > global+env > global
//! ```
//!
//! Branch overrides are deferred until preview environments exist. Dockerfile settings (context,
//! Dockerfile path, target stage) are command-line choices recorded in the release pin, not scoped
//! variables.

use crate::envvars;
use crate::ids::{ApplicationId, DeploymentId};
use crate::journal_format::frame;
use crate::secrets::SecretRef;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_ENTRIES: usize = 256;
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
pub const MAX_SECRET_ENTRIES: usize = 64;
const MAX_TOKEN: usize = 128;

// ------------------------------------------------------------------ scopes

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Global,
    Project,
    Application,
}

/// Where a definition applies. `id` is empty for `Global`, the project name or the application id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope {
    pub level: Level,
    pub id: String,
    pub env: Option<String>,
}

fn token(what: &str, v: &str) -> Result<(), String> {
    if v.is_empty() || v.len() > MAX_TOKEN {
        return Err(format!("{what} must be 1..={MAX_TOKEN} bytes"));
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

impl Scope {
    pub fn global(env: Option<&str>) -> Scope {
        Scope {
            level: Level::Global,
            id: String::new(),
            env: env.map(str::to_string),
        }
    }
    pub fn project(name: &str, env: Option<&str>) -> Scope {
        Scope {
            level: Level::Project,
            id: name.to_string(),
            env: env.map(str::to_string),
        }
    }
    pub fn application(app: &ApplicationId, env: Option<&str>) -> Scope {
        Scope {
            level: Level::Application,
            id: app.to_string(),
            env: env.map(str::to_string),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self.level {
            Level::Global if !self.id.is_empty() => return Err("the global scope has no id".into()),
            Level::Project => token("a project name", &self.id)?,
            Level::Application => {
                self.id
                    .parse::<ApplicationId>()
                    .map_err(|_| "the application id is not valid".to_string())?;
            }
            _ => {}
        }
        if let Some(e) = &self.env {
            token("an environment name", e)?;
        }
        Ok(())
    }

    /// `global`, `project:NAME`, `app:APP_ID`, each with an optional `@ENV`.
    pub fn slug(&self) -> String {
        let base = match self.level {
            Level::Global => "global".to_string(),
            Level::Project => format!("project:{}", self.id),
            Level::Application => format!("app:{}", self.id),
        };
        match &self.env {
            Some(e) => format!("{base}@{e}"),
            None => base,
        }
    }

    pub fn parse(slug: &str) -> Result<Scope, String> {
        let (base, env) = match slug.split_once('@') {
            Some((b, e)) => (b, Some(e.to_string())),
            None => (slug, None),
        };
        let s = if base == "global" {
            Scope {
                level: Level::Global,
                id: String::new(),
                env,
            }
        } else if let Some(n) = base.strip_prefix("project:") {
            Scope {
                level: Level::Project,
                id: n.to_string(),
                env,
            }
        } else if let Some(a) = base.strip_prefix("app:") {
            Scope {
                level: Level::Application,
                id: a.to_string(),
                env,
            }
        } else {
            return Err(format!("unknown scope {slug:?}"));
        };
        s.validate()?;
        Ok(s)
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.slug())
    }
}

/// The id every version of one secret shares: stable per (scope, name), so re-saving continues the
/// version sequence and a version number is never reused.
pub fn secret_id(scope: &Scope, name: &str) -> String {
    format!("{}/{}", scope.slug(), name)
}

// ------------------------------------------------------------------ definitions

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Config,
    Secret,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Config => "config",
            Kind::Secret => "secret",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    pub build: bool,
    pub runtime: bool,
}

impl Flags {
    pub fn parse(s: &str) -> Result<Flags, String> {
        match s {
            "b" => Ok(Flags {
                build: true,
                runtime: false,
            }),
            "r" => Ok(Flags {
                build: false,
                runtime: true,
            }),
            "br" => Ok(Flags {
                build: true,
                runtime: true,
            }),
            _ => Err(format!("bad flags {s:?}")),
        }
    }
    pub fn code(self) -> &'static str {
        match (self.build, self.runtime) {
            (true, true) => "br",
            (true, false) => "b",
            _ => "r",
        }
    }
    pub fn label(self) -> &'static str {
        match (self.build, self.runtime) {
            (true, true) => "build+runtime",
            (true, false) => "build",
            _ => "runtime",
        }
    }
}

/// A Config value is readable; a Secret is only ever a reference.
#[derive(Clone, PartialEq, Eq)]
pub enum Value {
    Config(String),
    Secret(SecretRef),
}

impl Value {
    pub fn kind(&self) -> Kind {
        match self {
            Value::Config(_) => Kind::Config,
            Value::Secret(_) => Kind::Secret,
        }
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Config(_) => f.write_str("Config(..)"),
            Value::Secret(r) => write!(f, "Secret({r})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setting {
    Set { flags: Flags, value: Value },
    Unset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    pub scope: Scope,
    pub name: String,
    pub setting: Setting,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretSaved {
    pub id: String,
    pub version: u32,
    pub at_ms: u64,
}

/// One resolved key of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub flags: Flags,
    /// The scope that supplied it; `None` for a value given on the command line (`--env`).
    pub source: Option<Scope>,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub deployment: DeploymentId,
    pub app: ApplicationId,
    pub project: Option<String>,
    pub environment: Option<String>,
    pub at_ms: u64,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigRecord {
    Def(Def),
    SecretSaved(SecretSaved),
    Snapshot(Box<Snapshot>),
}

// ------------------------------------------------------------------ validation

pub fn check_name(name: &str) -> Result<(), String> {
    envvars::check_name(name)
}

pub fn check_config_value(name: &str, value: &str) -> Result<(), String> {
    envvars::check_value(name, value)
}

fn check_secret_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 3 * MAX_TOKEN
        || id
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '@' | '/')))
    {
        return Err("a secret id is not valid".into());
    }
    Ok(())
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

fn enc_value(v: &Value) -> String {
    match v {
        Value::Config(s) => format!("c:{}", hex(s)),
        Value::Secret(r) => format!("s:{r}"),
    }
}

fn dec_value(s: &str) -> Result<Value, String> {
    if let Some(h) = s.strip_prefix("c:") {
        Ok(Value::Config(unhex(h).ok_or("a configuration value is not valid")?))
    } else if let Some(r) = s.strip_prefix("s:") {
        let r = SecretRef::parse(r)?;
        check_secret_id(&r.id)?;
        Ok(Value::Secret(r))
    } else {
        Err("a value is neither config nor secret".into())
    }
}

pub fn encode(r: &ConfigRecord) -> String {
    let body = match r {
        ConfigRecord::Def(d) => {
            let (what, flags, value) = match &d.setting {
                Setting::Set { flags, value } => ("set", flags.code(), enc_value(value)),
                Setting::Unset => ("unset", "-", "-".to_string()),
            };
            format!(
                "cfgdef\t1\t{}\t{}\t{what}\t{flags}\t{value}\t{}",
                d.scope.slug(),
                d.name,
                d.at_ms
            )
        }
        ConfigRecord::SecretSaved(s) => format!("cfgsec\t1\t{}\t{}\t{}", s.id, s.version, s.at_ms),
        ConfigRecord::Snapshot(s) => {
            let entries: Vec<String> = s
                .entries
                .iter()
                .map(|e| {
                    format!(
                        "{}|{}|{}|{}",
                        e.name,
                        e.flags.code(),
                        e.source.as_ref().map(Scope::slug).unwrap_or_else(|| "cli".into()),
                        enc_value(&e.value)
                    )
                })
                .collect();
            format!(
                "cfgsnap\t1\t{}\t{}\t{}\t{}\t{}\t{}",
                s.deployment,
                s.app,
                s.project.clone().unwrap_or_else(|| "-".into()),
                s.environment.clone().unwrap_or_else(|| "-".into()),
                s.at_ms,
                if entries.is_empty() {
                    "-".into()
                } else {
                    entries.join(",")
                }
            )
        }
    };
    frame(&body)
}

fn bad<T>(m: impl Into<String>) -> Result<T, String> {
    Err(m.into())
}

/// `f` are the tab-separated fields of the checksummed body (already verified by the caller).
pub fn decode(f: &[&str]) -> Result<ConfigRecord, String> {
    let n = |s: &str, w: &str| s.parse::<u64>().or_else(|_| bad(format!("bad {w}")));
    if f.get(1) != Some(&"1") {
        return bad("unknown configuration record version");
    }
    match f[0] {
        "cfgdef" => {
            if f.len() != 8 {
                return bad("a definition has the wrong number of fields");
            }
            let scope = Scope::parse(f[2])?;
            check_name(f[3])?;
            let setting = match f[4] {
                "unset" => {
                    if f[5] != "-" || f[6] != "-" {
                        return bad("an unset carries no flags or value");
                    }
                    Setting::Unset
                }
                "set" => {
                    let value = dec_value(f[6])?;
                    if let Value::Config(v) = &value {
                        check_config_value(f[3], v)?;
                    }
                    Setting::Set {
                        flags: Flags::parse(f[5])?,
                        value,
                    }
                }
                _ => return bad("a definition is neither set nor unset"),
            };
            Ok(ConfigRecord::Def(Def {
                scope,
                name: f[3].to_string(),
                setting,
                at_ms: n(f[7], "time")?,
            }))
        }
        "cfgsec" => {
            if f.len() != 5 {
                return bad("a secret record has the wrong number of fields");
            }
            check_secret_id(f[2])?;
            let version: u32 = f[3].parse().or_else(|_| bad("bad secret version"))?;
            Ok(ConfigRecord::SecretSaved(SecretSaved {
                id: f[2].to_string(),
                version,
                at_ms: n(f[4], "time")?,
            }))
        }
        "cfgsnap" => {
            if f.len() != 8 {
                return bad("a snapshot has the wrong number of fields");
            }
            let dash = |s: &str| (s != "-").then(|| s.to_string());
            let mut entries = Vec::new();
            if f[7] != "-" {
                for item in f[7].split(',') {
                    let p: Vec<&str> = item.split('|').collect();
                    if p.len() != 4 {
                        return bad("a snapshot entry is malformed");
                    }
                    check_name(p[0])?;
                    let value = dec_value(p[3])?;
                    if let Value::Config(v) = &value {
                        check_config_value(p[0], v)?;
                    }
                    entries.push(Entry {
                        name: p[0].to_string(),
                        flags: Flags::parse(p[1])?,
                        source: if p[2] == "cli" { None } else { Some(Scope::parse(p[2])?) },
                        value,
                    });
                }
            }
            Ok(ConfigRecord::Snapshot(Box::new(Snapshot {
                deployment: f[2].parse().or_else(|_| bad("bad deployment id"))?,
                app: f[3].parse().or_else(|_| bad("bad application id"))?,
                project: dash(f[4]),
                environment: dash(f[5]),
                at_ms: n(f[6], "time")?,
                entries,
            })))
        }
        _ => bad("not a configuration record"),
    }
}

// ------------------------------------------------------------------ state

/// Everything the configuration records say, rebuilt by replaying them in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigState {
    pub defs: Vec<Def>,
    /// Highest saved version per secret id.
    pub versions: BTreeMap<String, u32>,
    pub snapshots: Vec<Snapshot>,
}

impl ConfigState {
    /// Apply one record, refusing any that contradicts the state so far (replay fails closed).
    pub fn apply(&mut self, r: &ConfigRecord) -> Result<(), String> {
        match r {
            ConfigRecord::SecretSaved(s) => {
                // Strictly increasing, not necessarily consecutive: a save that crashed after the
                // Keychain write and before this record leaves an orphan item, and the next save skips it.
                let last = self.versions.get(&s.id).copied().unwrap_or(0);
                if s.version <= last {
                    return bad(format!(
                        "secret {} version {} does not follow version {last}",
                        s.id, s.version
                    ));
                }
                self.versions.insert(s.id.clone(), s.version);
            }
            ConfigRecord::Def(d) => {
                if let Setting::Set {
                    value: Value::Secret(r),
                    ..
                } = &d.setting
                {
                    self.check_saved(r)?;
                }
                self.defs.push(d.clone());
            }
            ConfigRecord::Snapshot(s) => {
                if self.snapshots.iter().any(|x| x.deployment == s.deployment) {
                    return bad(format!(
                        "a configuration snapshot for deployment {} is recorded twice",
                        s.deployment
                    ));
                }
                let mut seen = BTreeSet::new();
                for e in &s.entries {
                    if !seen.insert(&e.name) {
                        return bad(format!("snapshot repeats {}", e.name));
                    }
                    if let Value::Secret(r) = &e.value {
                        self.check_saved(r)?;
                    }
                }
                self.snapshots.push((**s).clone());
            }
        }
        Ok(())
    }

    fn check_saved(&self, r: &SecretRef) -> Result<(), String> {
        match self.versions.get(&r.id) {
            Some(v) if r.version <= *v => Ok(()),
            _ => bad(format!("a record names secret {r}, which was never saved")),
        }
    }

    /// The version a re-save of `id` will get.
    pub fn next_version(&self, id: &str) -> u32 {
        self.versions.get(id).copied().unwrap_or(0) + 1
    }

    /// The current definition of each (scope, name): the latest record wins.
    pub fn current(&self) -> BTreeMap<(&Scope, &str), &Def> {
        let mut m = BTreeMap::new();
        for d in &self.defs {
            m.insert((&d.scope, d.name.as_str()), d);
        }
        m
    }

    pub fn snapshot_of(&self, d: &DeploymentId) -> Option<&Snapshot> {
        self.snapshots.iter().find(|s| s.deployment == *d)
    }

    /// Resolve the effective configuration for an application. Pure: reads definitions only.
    pub fn resolve(&self, project: Option<&str>, app: &ApplicationId, env: Option<&str>) -> Result<Vec<Entry>, String> {
        let mut order: Vec<Scope> = vec![Scope::global(None)];
        if let Some(e) = env {
            order.push(Scope::global(Some(e)));
        }
        if let Some(p) = project {
            order.push(Scope::project(p, None));
            if let Some(e) = env {
                order.push(Scope::project(p, Some(e)));
            }
        }
        order.push(Scope::application(app, None));
        if let Some(e) = env {
            order.push(Scope::application(app, Some(e)));
        }
        let cur = self.current();
        let mut out: BTreeMap<String, Entry> = BTreeMap::new();
        for scope in &order {
            for ((s, name), d) in &cur {
                if *s != scope {
                    continue;
                }
                match &d.setting {
                    // an explicit unset blocks everything inherited from a less specific scope
                    Setting::Unset => {
                        out.remove(*name);
                    }
                    Setting::Set { flags, value } => {
                        out.insert(
                            name.to_string(),
                            Entry {
                                name: name.to_string(),
                                flags: *flags,
                                source: Some(scope.clone()),
                                value: value.clone(),
                            },
                        );
                    }
                }
            }
        }
        let entries: Vec<Entry> = out.into_values().collect();
        validate_entries(&entries)?;
        Ok(entries)
    }

    /// After replay: every pinned snapshot id exists and every pinned secret was saved.
    pub fn check_pins(&self, deployments: &[crate::state::Deployment]) -> Result<(), String> {
        for d in deployments {
            let Some(p) = &d.pinned else { continue };
            if let Some(origin) = &p.snapshot {
                let id: DeploymentId = origin
                    .parse()
                    .map_err(|_| format!("deployment {} pins a malformed configuration snapshot id", d.id))?;
                if self.snapshot_of(&id).is_none() {
                    return bad(format!(
                        "deployment {} pins snapshot {origin}, which is not recorded",
                        d.id
                    ));
                }
            }
            for (_, r) in &p.secrets {
                self.check_saved(r).map_err(|e| format!("deployment {}: {e}", d.id))?;
            }
        }
        Ok(())
    }
}

/// Sizes and names of a resolved set: a bad set is refused before anything is built.
pub fn validate_entries(entries: &[Entry]) -> Result<(), String> {
    if entries.len() > MAX_ENTRIES {
        return bad(format!(
            "{} variables resolved; at most {MAX_ENTRIES} are accepted",
            entries.len()
        ));
    }
    let (mut bytes, mut secrets) = (0usize, 0usize);
    for e in entries {
        check_name(&e.name)?;
        match &e.value {
            Value::Config(v) => {
                check_config_value(&e.name, v)?;
                bytes += e.name.len() + v.len() + 1;
            }
            Value::Secret(_) => secrets += 1,
        }
    }
    if bytes > MAX_CONFIG_BYTES {
        return bad(format!(
            "the configuration values total {bytes} bytes; at most {MAX_CONFIG_BYTES} are accepted"
        ));
    }
    if secrets > MAX_SECRET_ENTRIES {
        return bad(format!(
            "{secrets} secrets resolved; at most {MAX_SECRET_ENTRIES} are accepted"
        ));
    }
    Ok(())
}

/// Command-line values (`--env`, `--build-arg`) take precedence over every scope; they are non-secret.
pub fn with_command(mut entries: Vec<Entry>, pairs: &[(String, String)], flags: Flags) -> Vec<Entry> {
    for (k, v) in pairs {
        entries.retain(|e| e.name != *k);
        entries.push(Entry {
            name: k.clone(),
            flags,
            source: None,
            value: Value::Config(v.clone()),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// The resolved set split the way the build and the container need it.
pub fn build_args(entries: &[Entry]) -> Vec<(String, String)> {
    pick_config(entries, |f| f.build)
}
pub fn runtime_env(entries: &[Entry]) -> Vec<(String, String)> {
    pick_config(entries, |f| f.runtime)
}
pub fn build_secrets(entries: &[Entry]) -> Vec<(String, SecretRef)> {
    pick_secret(entries, |f| f.build)
}
pub fn runtime_secrets(entries: &[Entry]) -> Vec<(String, SecretRef)> {
    pick_secret(entries, |f| f.runtime)
}

fn pick_config(entries: &[Entry], keep: impl Fn(Flags) -> bool) -> Vec<(String, String)> {
    entries
        .iter()
        .filter(|e| keep(e.flags))
        .filter_map(|e| match &e.value {
            Value::Config(v) => Some((e.name.clone(), v.clone())),
            Value::Secret(_) => None,
        })
        .collect()
}

fn pick_secret(entries: &[Entry], keep: impl Fn(Flags) -> bool) -> Vec<(String, SecretRef)> {
    entries
        .iter()
        .filter(|e| keep(e.flags))
        .filter_map(|e| match &e.value {
            Value::Secret(r) => Some((e.name.clone(), r.clone())),
            Value::Config(_) => None,
        })
        .collect()
}

// ------------------------------------------------------------------ display and diff

/// One line per key, never a secret value. A Config value is readable by design and is shown.
pub fn describe(e: &Entry, show_config_values: bool) -> String {
    let src = e
        .source
        .as_ref()
        .map(Scope::slug)
        .unwrap_or_else(|| "command line".into());
    let val = match &e.value {
        Value::Config(v) if show_config_values => format!("config = {v:?}"),
        Value::Config(_) => "config".to_string(),
        Value::Secret(r) => format!("secret {r}"),
    };
    format!("{} [{}] {val} (from {src})", e.name, e.flags.label())
}

/// A difference between two resolved sets: names, scopes, flags and secret versions; **never values**
/// (not even Config values, so a diff is safe to paste anywhere).
pub fn diff(old: &[Entry], new: &[Entry]) -> Vec<String> {
    let mut out = Vec::new();
    let find = |s: &'_ [Entry], n: &str| s.iter().find(|e| e.name == n).cloned();
    let mut names: Vec<&str> = old.iter().chain(new).map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    for n in names {
        match (find(old, n), find(new, n)) {
            (None, Some(b)) => out.push(format!("+ {}", describe(&b, false))),
            (Some(a), None) => out.push(format!("- {}", describe(&a, false))),
            (Some(a), Some(b)) if a != b => {
                let mut what = Vec::new();
                if a.value.kind() != b.value.kind() {
                    what.push(format!(
                        "type {} -> {}",
                        a.value.kind().as_str(),
                        b.value.kind().as_str()
                    ));
                }
                match (&a.value, &b.value) {
                    (Value::Secret(x), Value::Secret(y)) if x != y => {
                        if x.id == y.id {
                            what.push(format!("secret version {} -> {}", x.version, y.version));
                        } else {
                            what.push(format!("secret {x} -> {y}"));
                        }
                    }
                    (Value::Config(x), Value::Config(y)) if x != y => what.push("value changed".to_string()),
                    _ => {}
                }
                if a.flags != b.flags {
                    what.push(format!("flags {} -> {}", a.flags.label(), b.flags.label()));
                }
                if a.source != b.source {
                    let s = |e: &Entry| {
                        e.source
                            .as_ref()
                            .map(Scope::slug)
                            .unwrap_or_else(|| "command line".into())
                    };
                    what.push(format!("scope {} -> {}", s(&a), s(&b)));
                }
                out.push(format!("~ {n}: {}", what.join("; ")));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> ApplicationId {
        "app_00000000000000000000000001".parse().unwrap()
    }
    fn set(scope: Scope, name: &str, value: Value, flags: &str) -> ConfigRecord {
        ConfigRecord::Def(Def {
            scope,
            name: name.into(),
            setting: Setting::Set {
                flags: Flags::parse(flags).unwrap(),
                value,
            },
            at_ms: 1,
        })
    }
    fn unset(scope: Scope, name: &str) -> ConfigRecord {
        ConfigRecord::Def(Def {
            scope,
            name: name.into(),
            setting: Setting::Unset,
            at_ms: 2,
        })
    }
    fn st(records: &[ConfigRecord]) -> ConfigState {
        let mut s = ConfigState::default();
        for r in records {
            s.apply(r).unwrap();
        }
        s
    }
    fn val(s: &ConfigState, name: &str, env: Option<&str>) -> Option<String> {
        s.resolve(Some("shop"), &app(), env)
            .unwrap()
            .into_iter()
            .find(|e| e.name == name)
            .map(|e| match e.value {
                Value::Config(v) => v,
                Value::Secret(r) => r.to_string(),
            })
    }
    fn c(v: &str) -> Value {
        Value::Config(v.into())
    }

    #[test]
    fn the_six_scopes_resolve_most_specific_first_and_an_unset_blocks_inheritance() {
        let a = app();
        let all = [
            set(Scope::global(None), "K", c("global"), "r"),
            set(Scope::global(Some("prod")), "K", c("global+env"), "r"),
            set(Scope::project("shop", None), "K", c("project"), "r"),
            set(Scope::project("shop", Some("prod")), "K", c("project+env"), "r"),
            set(Scope::application(&a, None), "K", c("app"), "r"),
            set(Scope::application(&a, Some("prod")), "K", c("app+env"), "r"),
        ];
        // peel the most specific definitions off one at a time
        let want = ["app+env", "app", "project+env", "project", "global+env", "global"];
        for (i, w) in want.iter().enumerate() {
            let s = st(&all[..6 - i]);
            assert_eq!(val(&s, "K", Some("prod")).as_deref(), Some(*w));
        }
        // without an environment the +env scopes do not apply
        assert_eq!(val(&st(&all), "K", None).as_deref(), Some("app"));
        // a different environment does not see prod's
        assert_eq!(val(&st(&all), "K", Some("dev")).as_deref(), Some("app"));
        // an unset at a narrower scope blocks every less specific value
        let mut with_unset = all[..4].to_vec();
        with_unset.push(unset(Scope::application(&a, None), "K"));
        assert_eq!(val(&st(&with_unset), "K", Some("prod")), None);
        // and a still narrower set restores it
        with_unset.push(set(Scope::application(&a, Some("prod")), "K", c("again"), "r"));
        assert_eq!(val(&st(&with_unset), "K", Some("prod")).as_deref(), Some("again"));
        // the latest definition at one scope wins
        let twice = st(&[
            set(Scope::global(None), "K", c("one"), "r"),
            set(Scope::global(None), "K", c("two"), "r"),
        ]);
        assert_eq!(val(&twice, "K", None).as_deref(), Some("two"));
        // another application's definitions never apply
        let other: ApplicationId = "app_00000000000000000000000002".parse().unwrap();
        let s = st(&[set(Scope::application(&other, None), "K", c("theirs"), "r")]);
        assert_eq!(val(&s, "K", None), None);
    }

    #[test]
    fn secret_versions_are_consecutive_immutable_references_and_a_def_needs_a_saved_one() {
        let mut s = ConfigState::default();
        let id = secret_id(&Scope::global(None), "DB");
        let unsaved = set(Scope::global(None), "DB", Value::Secret(SecretRef::new(&id, 1)), "r");
        assert!(
            s.apply(&unsaved).is_err(),
            "a reference to a version that was never saved"
        );
        assert_eq!(s.next_version(&id), 1);
        for v in 1..=2 {
            s.apply(&ConfigRecord::SecretSaved(SecretSaved {
                id: id.clone(),
                version: v,
                at_ms: 1,
            }))
            .unwrap();
        }
        assert_eq!(s.next_version(&id), 3);
        assert!(s
            .apply(&ConfigRecord::SecretSaved(SecretSaved {
                id: id.clone(),
                version: 2,
                at_ms: 1
            }))
            .is_err());
        assert!(s
            .apply(&set(
                Scope::global(None),
                "DB",
                Value::Secret(SecretRef::new(&id, 3)),
                "r"
            ))
            .is_err());
        s.apply(&set(
            Scope::global(None),
            "DB",
            Value::Secret(SecretRef::new(&id, 2)),
            "br",
        ))
        .unwrap();
        // a skipped number (an orphan Keychain item from a crashed save) is fine; going back is not
        s.apply(&ConfigRecord::SecretSaved(SecretSaved {
            id: id.clone(),
            version: 4,
            at_ms: 1,
        }))
        .unwrap();
        assert!(s
            .apply(&ConfigRecord::SecretSaved(SecretSaved {
                id: id.clone(),
                version: 3,
                at_ms: 1
            }))
            .is_err());
        assert_eq!(s.next_version(&id), 5);
    }

    #[test]
    fn records_round_trip_and_damage_is_rejected() {
        let a = app();
        let id = secret_id(&Scope::application(&a, Some("prod")), "TOKEN");
        let dep: DeploymentId = "dep_00000000000000000000000010".parse().unwrap();
        let records = [
            set(Scope::project("shop", Some("prod")), "MODE", c("a b=c;d,e|f"), "b"),
            unset(Scope::global(None), "GONE"),
            ConfigRecord::SecretSaved(SecretSaved {
                id: id.clone(),
                version: 1,
                at_ms: 3,
            }),
            set(
                Scope::application(&a, Some("prod")),
                "TOKEN",
                Value::Secret(SecretRef::new(&id, 1)),
                "br",
            ),
            ConfigRecord::Snapshot(Box::new(Snapshot {
                deployment: dep,
                app: a.clone(),
                project: Some("shop".into()),
                environment: Some("prod".into()),
                at_ms: 9,
                entries: vec![
                    Entry {
                        name: "A".into(),
                        flags: Flags::parse("r").unwrap(),
                        source: None,
                        value: c("1"),
                    },
                    Entry {
                        name: "TOKEN".into(),
                        flags: Flags::parse("br").unwrap(),
                        source: Some(Scope::application(&a, Some("prod"))),
                        value: Value::Secret(SecretRef::new(&id, 1)),
                    },
                ],
            })),
        ];
        for r in &records {
            let line = encode(r);
            let body = line.split_once('\t').unwrap().1;
            let f: Vec<&str> = body.split('\t').collect();
            assert_eq!(&decode(&f).unwrap(), r, "{body}");
            assert!(!body.contains('\n'));
        }
        // a snapshot's replay requires its secrets to exist
        let mut s = ConfigState::default();
        assert!(s.apply(&records[4]).is_err());
        for r in &records[2..] {
            s.apply(r).unwrap();
        }
        assert!(
            s.apply(&records[4]).is_err(),
            "a snapshot is written once per deployment"
        );
        for bad in [
            vec!["cfgdef", "1", "galaxy", "A", "unset", "-", "-", "1"],
            vec!["cfgdef", "1", "global", "1A", "unset", "-", "-", "1"],
            vec!["cfgdef", "1", "global", "A", "set", "x", "c:41", "1"],
            vec!["cfgdef", "1", "global", "A", "set", "r", "s:nover", "1"],
            vec!["cfgsec", "2", "id", "1", "1"],
            vec!["cfgsnap", "1", "dep", "app", "-", "-", "1", "-"],
        ] {
            assert!(decode(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn diff_and_display_show_names_scopes_flags_and_versions_never_values() {
        let a = app();
        let id = secret_id(&Scope::global(None), "DB");
        let e = |name: &str, flags: &str, source: Option<Scope>, value: Value| Entry {
            name: name.into(),
            flags: Flags::parse(flags).unwrap(),
            source,
            value,
        };
        let old = vec![
            e("KEEP", "r", Some(Scope::global(None)), c("same")),
            e(
                "DB",
                "r",
                Some(Scope::global(None)),
                Value::Secret(SecretRef::new(&id, 1)),
            ),
            e("OLDVAL", "r", Some(Scope::global(None)), c("super-private-old")),
            e("GONE", "b", Some(Scope::global(None)), c("x")),
        ];
        let new = vec![
            e("KEEP", "r", Some(Scope::global(None)), c("same")),
            e(
                "DB",
                "br",
                Some(Scope::application(&a, None)),
                Value::Secret(SecretRef::new(&id, 2)),
            ),
            e("OLDVAL", "r", Some(Scope::global(None)), c("super-private-new")),
            e("ADDED", "r", None, c("hidden-value")),
        ];
        let d = diff(&old, &new).join("\n");
        assert!(d.contains("+ ADDED") && d.contains("- GONE"), "{d}");
        assert!(
            d.contains("secret version 1 -> 2") && d.contains("flags runtime -> build+runtime"),
            "{d}"
        );
        assert!(d.contains("scope global -> app:app_"), "{d}");
        assert!(d.contains("~ OLDVAL: value changed") && !d.contains("KEEP"), "{d}");
        for v in ["super-private-old", "super-private-new", "hidden-value"] {
            assert!(!d.contains(v), "{d}");
        }
        assert!(!describe(&new[3], false).contains("hidden-value"));
        assert!(
            describe(&new[3], true).contains("hidden-value"),
            "Config values are readable on request"
        );
        assert!(
            !format!("{:?}", new[3]).contains("hidden-value"),
            "Debug does not print config values either"
        );
    }

    #[test]
    fn a_very_large_set_is_refused_and_command_env_wins() {
        let many: Vec<Entry> = (0..=MAX_ENTRIES)
            .map(|i| Entry {
                name: format!("V{i}"),
                flags: Flags::parse("r").unwrap(),
                source: None,
                value: c("1"),
            })
            .collect();
        assert!(validate_entries(&many).is_err());
        let one = vec![Entry {
            name: "A".into(),
            flags: Flags::parse("b").unwrap(),
            source: None,
            value: c("scoped"),
        }];
        let merged = with_command(
            one,
            &[("A".into(), "cli".into()), ("B".into(), "2".into())],
            Flags {
                build: false,
                runtime: true,
            },
        );
        assert_eq!(
            runtime_env(&merged),
            vec![("A".to_string(), "cli".to_string()), ("B".to_string(), "2".to_string())]
        );
        assert!(build_args(&merged).is_empty());
    }
}
