//! Source deployments: detect how an ordinary repository builds and starts, and render the
//! Dockerfile Harpoon builds it with when the repository has none.
//!
//! Everything here is pure (reads a directory, returns text). The generated Dockerfile is private to
//! one build: the backend writes it into the build snapshot, never into the user's workspace.
//! Node (npm only) and Python (pip and uv only) are supported; anything else is refused with a reason
//! rather than guessed at.

use crate::json::{obj, s, Json};
use std::path::Path;

/// Where the generated Dockerfile lives inside the private snapshot.
pub const GENERATED_DOCKERFILE: &str = ".harpoon-generated.Dockerfile";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Install, build, then run the production start command.
    Production,
    /// Install and run the development command; no build step.
    Development,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Long-running; ready when it answers its health check.
    Service,
    /// Runs to completion; succeeds when it exits 0.
    Job,
}

/// What the user chose on top of detection. Every field is optional; stored with the registration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub install: Option<String>,
    pub build: Option<String>,
    pub start: Option<String>,
    pub style: Option<Style>,
    pub kind: Option<Kind>,
    /// An existing Docker network (for example a Compose project's) the container joins.
    pub network: Option<String>,
}

const MAX_COMMAND: usize = 1024;

fn command_problem(what: &str, v: &str) -> Result<(), String> {
    if v.trim().is_empty() {
        return Err(format!("the {what} command is empty"));
    }
    if v.len() > MAX_COMMAND {
        return Err(format!("the {what} command is longer than {MAX_COMMAND} characters"));
    }
    if let Some(c) = v.chars().find(|c| c.is_control()) {
        return Err(format!(
            "the {what} command contains a control character (U+{:04X}); use one line",
            c as u32
        ));
    }
    Ok(())
}

/// A Docker network name: what Docker itself accepts for a user-defined network.
pub fn network_problem(n: &str) -> Result<(), String> {
    let ok = !n.is_empty()
        && n.len() <= 128
        && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if ok && !matches!(n, "host" | "none" | "bridge") {
        Ok(())
    } else {
        Err(format!(
            "network {n:?} is not a user-defined Docker network name (letters, digits, _ . -; not host, none or bridge)"
        ))
    }
}

impl Settings {
    pub fn is_empty(&self) -> bool {
        *self == Settings::default()
    }

    pub fn parse(text: &str) -> Result<Settings, String> {
        let j = Json::parse(text).map_err(|e| format!("settings are not valid JSON: {}", e.0))?;
        let fields = j.as_object().ok_or("settings must be a JSON object")?;
        let mut out = Settings::default();
        for (k, v) in fields {
            if v.is_null() {
                continue;
            }
            let text = v
                .as_str()
                .ok_or_else(|| format!("setting {k:?} must be text"))?
                .trim()
                .to_string();
            if text.is_empty() {
                continue;
            }
            match k.as_str() {
                "install" | "build" | "start" => {
                    command_problem(k, &text)?;
                    match k.as_str() {
                        "install" => out.install = Some(text),
                        "build" => out.build = Some(text),
                        _ => out.start = Some(text),
                    }
                }
                "style" => {
                    out.style = Some(match text.as_str() {
                        "production" => Style::Production,
                        "development" => Style::Development,
                        _ => return Err("style must be production or development".into()),
                    })
                }
                "kind" => {
                    out.kind = Some(match text.as_str() {
                        "service" => Kind::Service,
                        "job" => Kind::Job,
                        _ => return Err("kind must be service or job".into()),
                    })
                }
                "network" => {
                    network_problem(&text)?;
                    out.network = Some(text)
                }
                other => return Err(format!("unknown setting {other:?}")),
            }
        }
        Ok(out)
    }

    /// The one spelling stored in the journal and compared for equality: fixed key order, only set keys.
    pub fn canonical(&self) -> String {
        let mut items: Vec<(&str, Json)> = Vec::new();
        if let Some(v) = &self.install {
            items.push(("install", s(v.clone())));
        }
        if let Some(v) = &self.build {
            items.push(("build", s(v.clone())));
        }
        if let Some(v) = &self.start {
            items.push(("start", s(v.clone())));
        }
        if let Some(v) = self.style {
            items.push(("style", s(if v == Style::Production { "production" } else { "development" })));
        }
        if let Some(v) = self.kind {
            items.push(("kind", s(if v == Kind::Service { "service" } else { "job" })));
        }
        if let Some(v) = &self.network {
            items.push(("network", s(v.clone())));
        }
        obj(items).to_json()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Node,
    Python,
}

/// What detection found, before the user's settings are applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub language: Language,
    /// Human label: "Next.js", "Node.js", "Python (uv)", …
    pub framework: String,
    pub base_image: String,
    pub install: String,
    /// Whether install must see the whole source (a lifecycle script, such as `prisma generate`, needs it).
    pub install_needs_source: bool,
    pub build: Option<String>,
    pub start: Option<String>,
    pub dev_start: Option<String>,
    pub suggested_port: u16,
    pub notes: Vec<String>,
}

fn read(dir: &Path, name: &str) -> Option<String> {
    let p = dir.join(name);
    // Not through a link and not huge: detection only needs small manifests.
    let m = std::fs::symlink_metadata(&p).ok()?;
    if !m.is_file() || m.len() > 1024 * 1024 {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

fn exists(dir: &Path, name: &str) -> bool {
    std::fs::symlink_metadata(dir.join(name)).is_ok()
}

/// The Node major to build with: the highest major an `engines.node` range allows, else `.nvmrc`, else 22.
fn node_major(engines: Option<&str>, nvmrc: Option<&str>) -> u32 {
    let from_range = engines.and_then(|r| {
        let nums: Vec<(char, u32)> = r
            .split(|c: char| c.is_whitespace() || c == '|')
            .filter(|t| !t.is_empty())
            .filter_map(|t| {
                let op = t.chars().next().filter(|c| matches!(c, '<' | '>' | '^' | '~' | '=')).unwrap_or('=');
                let digits: String = t
                    .trim_start_matches(|c: char| !c.is_ascii_digit())
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                digits.parse::<u32>().ok().map(|n| (op, n))
            })
            .collect();
        // "<23" means 22; ">=22.1 <23" means 22; ">=20" alone means the lowest named (20).
        let upper = nums.iter().filter(|(o, _)| *o == '<').map(|(_, n)| n.saturating_sub(1)).max();
        upper.or_else(|| nums.iter().map(|(_, n)| *n).max())
    });
    let from_rc = nvmrc.and_then(|t| {
        t.trim()
            .trim_start_matches(['v', 'V'])
            .split('.')
            .next()
            .and_then(|m| m.parse().ok())
    });
    from_range.or(from_rc).filter(|m| (14..=40).contains(m)).unwrap_or(22)
}

fn detect_node(dir: &Path, pkg: &str) -> Result<Detected, String> {
    let j = Json::parse(pkg).map_err(|e| format!("package.json is not valid JSON: {}", e.0))?;
    if exists(dir, "pnpm-lock.yaml") || exists(dir, "yarn.lock") || exists(dir, "bun.lockb") || exists(dir, "bun.lock") {
        if !exists(dir, "package-lock.json") {
            return Err("this project uses a package manager other than npm; Harpoon builds npm projects only (add a Dockerfile, or an npm package-lock.json)".into());
        }
    }
    let scripts = j.get("scripts");
    let script = |n: &str| scripts.and_then(|s| s.get(n)).and_then(Json::as_str).is_some();
    let has_dep = |n: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|k| j.get(k).and_then(|d| d.get(n)).is_some())
    };
    let engines = j.get("engines").and_then(|e| e.get("node")).and_then(Json::as_str);
    let major = node_major(engines, read(dir, ".nvmrc").as_deref());
    let framework = if has_dep("next") {
        "Next.js"
    } else if has_dep("nuxt") {
        "Nuxt"
    } else if has_dep("@sveltejs/kit") {
        "SvelteKit"
    } else if has_dep("@remix-run/node") || has_dep("@remix-run/react") {
        "Remix"
    } else if has_dep("express") {
        "Express"
    } else {
        "Node.js"
    };
    let install = if exists(dir, "package-lock.json") {
        "npm ci"
    } else {
        "npm install"
    };
    let build = script("build").then(|| "npm run build".to_string());
    let main = j.get("main").and_then(Json::as_str).map(str::to_string);
    let start = if script("start") {
        Some("npm start".to_string())
    } else if has_dep("next") {
        Some("npx next start".to_string())
    } else if let Some(m) = main.filter(|m| exists(dir, m)) {
        Some(format!("node {m}"))
    } else if exists(dir, "server.js") {
        Some("node server.js".to_string())
    } else if exists(dir, "index.js") {
        Some("node index.js".to_string())
    } else {
        None
    };
    let mut notes = Vec::new();
    if start.is_none() {
        notes.push("no start command found (no \"start\" script); set one".into());
    }
    let needs_source = ["preinstall", "install", "postinstall", "prepare"]
        .iter()
        .any(|h| script(h));
    if needs_source {
        notes.push("an install lifecycle script exists, so install runs on the full source".into());
    }
    Ok(Detected {
        language: Language::Node,
        framework: framework.into(),
        base_image: format!("node:{major}-slim"),
        install: install.into(),
        install_needs_source: needs_source,
        build,
        start,
        dev_start: script("dev").then(|| "npm run dev".to_string()),
        suggested_port: 3000,
        notes,
    })
}

fn detect_python(dir: &Path) -> Result<Detected, String> {
    let uv = exists(dir, "uv.lock");
    let reqs = exists(dir, "requirements.txt");
    let pyproject = read(dir, "pyproject.toml");
    if !uv && !reqs && pyproject.is_none() {
        return Err("no requirements.txt, uv.lock or pyproject.toml found".into());
    }
    let (framework, install) = if uv {
        ("Python (uv)", "pip install --no-cache-dir uv && uv sync --frozen --no-dev")
    } else if reqs {
        ("Python (pip)", "pip install --no-cache-dir -r requirements.txt")
    } else {
        ("Python (pip)", "pip install --no-cache-dir .")
    };
    let version = read(dir, ".python-version")
        .and_then(|t| {
            let mut it = t.trim().split('.');
            Some(format!("{}.{}", it.next()?, it.next()?))
        })
        .filter(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .unwrap_or_else(|| "3.12".into());
    let mut start = read(dir, "Procfile").and_then(|t| {
        t.lines()
            .find_map(|l| l.strip_prefix("web:").map(|c| c.trim().to_string()))
            .filter(|c| !c.is_empty())
    });
    if start.is_none() {
        start = ["main.py", "app.py", "server.py"]
            .iter()
            .find(|f| exists(dir, f))
            .map(|f| format!("python {f}"));
    }
    let start = start.map(|c| if uv { format!("uv run {c}") } else { c });
    let mut notes = Vec::new();
    if start.is_none() {
        notes.push("no start command found (no Procfile web line, main.py or app.py); set one".into());
    }
    Ok(Detected {
        language: Language::Python,
        framework: framework.into(),
        base_image: format!("python:{version}-slim"),
        install: install.into(),
        install_needs_source: reqs || uv || pyproject.is_some(),
        build: None,
        dev_start: start.clone(),
        start,
        suggested_port: 8000,
        notes,
    })
}

/// Detect how the project in `dir` builds and starts.
pub fn detect(dir: &Path) -> Result<Detected, String> {
    if let Some(pkg) = read(dir, "package.json") {
        return detect_node(dir, &pkg);
    }
    if exists(dir, "requirements.txt") || exists(dir, "uv.lock") || exists(dir, "pyproject.toml") {
        return detect_python(dir);
    }
    Err("no Dockerfile, package.json, requirements.txt or pyproject.toml found; Harpoon builds Node (npm) and Python (pip, uv) projects, or any project with a Dockerfile".into())
}

/// The resolved recipe: detection plus the user's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub detected: Detected,
    pub install: String,
    pub build: Option<String>,
    pub start: String,
    pub style: Style,
    pub kind: Kind,
}

impl BuildPlan {
    pub fn resolve(d: Detected, set: &Settings) -> Result<BuildPlan, String> {
        let style = set.style.unwrap_or(Style::Production);
        let default_start = match style {
            Style::Production => d.start.clone(),
            Style::Development => d.dev_start.clone().or_else(|| d.start.clone()),
        };
        let start = set
            .start
            .clone()
            .or(default_start)
            .ok_or_else(|| format!("{}: no start command was detected; set one", d.framework))?;
        let build = match style {
            Style::Development => None,
            Style::Production => set.build.clone().or_else(|| d.build.clone()),
        };
        Ok(BuildPlan {
            install: set.install.clone().unwrap_or_else(|| d.install.clone()),
            build,
            start,
            style,
            kind: set.kind.unwrap_or(Kind::Service),
            detected: d,
        })
    }

    /// The canonical description pinned with a release (never contains a value, only commands).
    pub fn identity(&self) -> String {
        format!(
            "generated:{}|{}|{}|{}|{}",
            self.detected.base_image,
            self.install,
            self.build.as_deref().unwrap_or("-"),
            self.start,
            if self.style == Style::Production { "production" } else { "development" }
        )
    }

    /// The Dockerfile text. `build_args` are non-secret names (values come from `--build-arg`),
    /// `build_secrets` are BuildKit secret ids mounted as same-named environment variables for the
    /// install and build commands only, so no secret ends up in a layer or in the image.
    pub fn dockerfile(&self, port: u16, build_args: &[String], build_secrets: &[String]) -> String {
        let mounts: String = build_secrets
            .iter()
            .map(|id| format!("--mount=type=secret,id={id},env={id} "))
            .collect();
        let mut out = String::new();
        out.push_str(&format!("FROM {}\nWORKDIR /app\n", self.detected.base_image));
        for a in build_args {
            out.push_str(&format!("ARG {a}\n"));
        }
        let manifests: &[&str] = match self.detected.language {
            Language::Node => &["package.json", "package-lock.json*"],
            Language::Python => &[],
        };
        let early = !self.detected.install_needs_source && !manifests.is_empty();
        if early {
            out.push_str(&format!("COPY {} ./\n", manifests.join(" ")));
        } else {
            out.push_str("COPY . .\n");
        }
        out.push_str(&format!("RUN {mounts}{}\n", self.install));
        if early {
            out.push_str("COPY . .\n");
        }
        if let Some(b) = &self.build {
            out.push_str(&format!("RUN {mounts}{b}\n"));
        }
        let env = match (self.detected.language, self.style) {
            (Language::Node, Style::Production) => "NODE_ENV=production ",
            (Language::Node, Style::Development) => "NODE_ENV=development ",
            _ => "",
        };
        out.push_str(&format!(
            "ENV {env}PORT={port} HOSTNAME=0.0.0.0\nEXPOSE {port}\n"
        ));
        // exec form around `sh -c exec …`: the start command becomes PID 1's replacement and gets SIGTERM.
        let inner = s(format!("exec {}", self.start)).to_json();
        out.push_str(&format!("CMD [\"/bin/sh\",\"-c\",{inner}]\n"));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("harpoon-plan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        for (f, t) in files {
            fs::write(d.join(f), t).unwrap();
        }
        d
    }

    const NEXT: &str = r#"{"scripts":{"build":"next build","start":"next start","postinstall":"prisma generate"},
        "engines":{"node":">=22.23.4 <23"},"dependencies":{"next":"15.5.0"}}"#;

    #[test]
    fn next_with_lockfile_and_postinstall() {
        let d = tmp("next", &[("package.json", NEXT), ("package-lock.json", "{}")]);
        let det = detect(&d).unwrap();
        assert_eq!(det.framework, "Next.js");
        assert_eq!(det.base_image, "node:22-slim");
        assert_eq!(det.install, "npm ci");
        assert!(det.install_needs_source);
        let plan = BuildPlan::resolve(det, &Settings::default()).unwrap();
        let df = plan.dockerfile(3000, &[], &["DATABASE_URL".into()]);
        assert!(df.contains("COPY . .\nRUN --mount=type=secret,id=DATABASE_URL,env=DATABASE_URL npm ci\n"));
        assert!(df.contains("RUN --mount=type=secret,id=DATABASE_URL,env=DATABASE_URL npm run build\n"));
        assert!(df.contains("CMD [\"/bin/sh\",\"-c\",\"exec npm start\"]"));
        assert!(!df.contains("DATABASE_URL=")); // the id, never a value
        assert!(crate::dockerfile::required_images(&df).is_ok());
    }

    #[test]
    fn plain_node_caches_dependencies_first() {
        let d = tmp("plain", &[("package.json", r#"{"scripts":{"start":"node a.js"}}"#)]);
        let plan = BuildPlan::resolve(detect(&d).unwrap(), &Settings::default()).unwrap();
        let df = plan.dockerfile(8080, &["A".into()], &[]);
        assert!(df.starts_with("FROM node:22-slim\nWORKDIR /app\nARG A\nCOPY package.json package-lock.json* ./\nRUN npm install\nCOPY . .\n"));
        assert!(!df.contains("npm run build"));
    }

    #[test]
    fn other_package_managers_are_refused_not_guessed() {
        let d = tmp("pnpm", &[("package.json", "{}"), ("pnpm-lock.yaml", "")]);
        assert!(detect(&d).unwrap_err().contains("other than npm"));
        let d = tmp("none", &[("README", "")]);
        assert!(detect(&d).unwrap_err().contains("no Dockerfile"));
    }

    #[test]
    fn overrides_and_development_style() {
        let d = tmp("dev", &[("package.json", NEXT), ("package-lock.json", "{}")]);
        let set = Settings::parse(r#"{"style":"development","start":"npx next dev","install":"npm install"}"#).unwrap();
        let plan = BuildPlan::resolve(detect(&d).unwrap(), &set).unwrap();
        assert!(plan.build.is_none());
        let df = plan.dockerfile(3000, &[], &[]);
        assert!(df.contains("NODE_ENV=development") && df.contains("exec npx next dev"));
        assert!(!df.contains("npm run build"));
    }

    #[test]
    fn python_pip_and_uv() {
        let d = tmp("pip", &[("requirements.txt", "flask"), ("app.py", "")]);
        let det = detect(&d).unwrap();
        assert_eq!(det.start.as_deref(), Some("python app.py"));
        assert!(det.base_image.starts_with("python:3.12"));
        let d = tmp("uv", &[("pyproject.toml", "[project]"), ("uv.lock", ""), ("main.py", "")]);
        let det = detect(&d).unwrap();
        assert_eq!(det.start.as_deref(), Some("uv run python main.py"));
        assert!(det.install.contains("uv sync --frozen"));
        let d = tmp("nostart", &[("requirements.txt", "x")]);
        assert!(BuildPlan::resolve(detect(&d).unwrap(), &Settings::default()).is_err());
    }

    #[test]
    fn settings_validate_and_round_trip() {
        let s = Settings::parse(r#"{"start":"npm start","kind":"job","network":"collectiv-harpoon-test_default"}"#).unwrap();
        assert_eq!(Settings::parse(&s.canonical()).unwrap(), s);
        for bad in [
            r#"{"start":"a\nb"}"#,
            r#"{"network":"host"}"#,
            r#"{"style":"fast"}"#,
            r#"{"nope":"x"}"#,
            "[]",
        ] {
            assert!(Settings::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn node_major_from_ranges() {
        assert_eq!(node_major(Some(">=22.23.4 <23"), None), 22);
        assert_eq!(node_major(Some("^20.1.0"), None), 20);
        assert_eq!(node_major(None, Some("v18.19.0\n")), 18);
        assert_eq!(node_major(None, None), 22);
    }
}
