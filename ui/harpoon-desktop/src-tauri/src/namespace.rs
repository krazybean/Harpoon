//! The development/test namespace: lets a DEBUG build of the desktop drive an isolated Harpoon engine
//! (`HARPOON_TEST_TMPDIR`, see the engine's `HarpoonRoots`) instead of the serving one.
//!
//! * A release build never reads `HARPOON_TEST_TMPDIR`: [`active`] is the constant `Ok(None)` there, so no
//!   environment variable can redirect a shipped desktop. The pure [`resolve`] takes the build kind as an
//!   argument so that rule is tested.
//! * An invalid namespace in a debug build is an error, never a fallback to production: the app refuses to start
//!   and every operation reports why.
//! * In a namespace every path comes from the validated root: the engine's state files (`runtime.json`,
//!   `runtime.pid`), its Docker and management sockets, the desktop's own deployment store and Docker client
//!   configuration. The desktop's real deployment store, the Keychain and the user's Docker contexts are not used.
//!
//! The rules mirror the engine's (`HarpoonRoots.isolationError`) so the two sides accept and refuse the same roots.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const ENV: &str = "HARPOON_TEST_TMPDIR";
/// `sun_path` is 104 bytes and the longest socket name is `/harpoon-docker.sock`; the engine uses the same budget.
const SOCKET_BUDGET: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    root: PathBuf,
}

/// The namespace for `value` (the environment variable, if set) in a build of the given kind, or why it is unusable.
/// `home` locates the production directories the root must never be or contain.
#[cfg_attr(not(debug_assertions), allow(dead_code))] // a release build never resolves a namespace; tests exercise this for both build kinds
pub fn resolve(value: Option<&str>, debug_build: bool, home: &Path) -> Result<Option<Namespace>, String> {
    if !debug_build {
        return Ok(None);
    }
    let Some(raw) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let root = Path::new(raw);
    if !root.is_absolute() {
        return Err(format!("{ENV} must be an absolute path, got {raw}"));
    }
    let real = std::fs::canonicalize(root).map_err(|e| format!("{ENV} {raw} cannot be resolved ({e}); create it first (the engine does when it starts)"))?;
    let meta = std::fs::metadata(&real).map_err(|e| format!("{ENV} {raw}: {e}"))?;
    if !meta.is_dir() {
        return Err(format!("{ENV} {raw} is not a directory"));
    }
    let tmp = std::fs::canonicalize("/tmp").unwrap_or_else(|_| PathBuf::from("/private/tmp"));
    if real == tmp {
        return Err(format!("{ENV} resolves to /tmp: its sockets and state would be the production names"));
    }
    for prod in [home.join("Library/Application Support/Harpoon"), home.join("Library/Application Support/com.harpoon.desktop")] {
        let prod = std::fs::canonicalize(&prod).unwrap_or(prod);
        if real.starts_with(&prod) || prod.starts_with(&real) {
            return Err(format!("{ENV} {raw} is, is inside, or contains a production Harpoon directory ({})", prod.display()));
        }
    }
    if meta.mode() & 0o022 != 0 {
        return Err(format!("{ENV} {raw} is writable by group or others; refusing a root another user could change"));
    }
    // owned by the person running the app: the home directory is the reference
    match std::fs::metadata(home) {
        Ok(h) if h.uid() == meta.uid() => {}
        _ => return Err(format!("{ENV} {raw} is not owned by the user running Harpoon")),
    }
    let trimmed = raw.trim_end_matches('/');
    if format!("{trimmed}/harpoon-docker.sock").len() > SOCKET_BUDGET {
        return Err(format!("{ENV} is too long for a unix socket path ({raw})"));
    }
    Ok(Some(Namespace { root: PathBuf::from(trimmed) }))
}

/// The namespace of this process. A release build has none, whatever the environment says; it does not even read it.
#[cfg(not(debug_assertions))]
pub fn active() -> Result<Option<Namespace>, String> {
    Ok(None)
}

#[cfg(debug_assertions)]
pub fn active() -> Result<Option<Namespace>, String> {
    static ACTIVE: std::sync::OnceLock<Result<Option<Namespace>, String>> = std::sync::OnceLock::new();
    ACTIVE
        .get_or_init(|| {
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            resolve(std::env::var(ENV).ok().as_deref(), true, &home)
        })
        .clone()
}

/// Refuse a feature that would reach the serving engine or the user's Docker setup while a namespace is active.
pub fn refuse_in_namespace(feature: &str) -> Result<(), String> {
    match active()? {
        None => Ok(()),
        Some(ns) => Err(format!("{feature} is not available while this desktop drives the test namespace {}", ns.root().display())),
    }
}

impl Namespace {
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Where the engine writes `runtime.json` and `runtime.pid` (the engine's state directory in a namespace).
    pub fn support_dir(&self) -> PathBuf {
        self.root.clone()
    }
    pub fn docker_socket(&self) -> PathBuf {
        self.root.join("harpoon-docker.sock")
    }
    pub fn mgmt_socket(&self) -> PathBuf {
        self.root.join("harpoon-mgmt.sock")
    }
    /// The desktop's own data directory (deployment store, attached-runtime references): never the real app data.
    pub fn data_dir(&self) -> PathBuf {
        self.root.join("desktop-data")
    }
    /// An empty Docker client configuration: the user's contexts, credential helpers and `~/.docker` are not read.
    pub fn docker_config_dir(&self) -> PathBuf {
        self.data_dir().join("docker-config")
    }

    /// The arguments for a Docker CLI call in this namespace. The desktop addresses Docker as `--context harpoon`; here that
    /// context is replaced by the namespace's own socket. A call that does not name the context is not translatable and is refused.
    pub fn docker_args(&self, args: &[&str]) -> Result<Vec<String>, String> {
        match args {
            ["--context", "harpoon", rest @ ..] => {
                let mut out = vec!["-H".to_string(), format!("unix://{}", self.docker_socket().display())];
                out.extend(rest.iter().map(|s| s.to_string()));
                Ok(out)
            }
            _ => Err(format!("docker {} is not available while this desktop drives the test namespace {}", args.join(" "), self.root.display())),
        }
    }

    /// Prepare a Docker CLI command for the namespace: its own client configuration, no inherited host or context.
    pub fn confine_docker(&self, cmd: &mut std::process::Command) {
        let _ = std::fs::create_dir_all(self.docker_config_dir());
        cmd.env("DOCKER_CONFIG", self.docker_config_dir());
        cmd.env_remove("DOCKER_HOST");
        cmd.env_remove("DOCKER_CONTEXT");
    }
}

/// The root of the active namespace, for display; `None` in every production configuration.
#[tauri::command]
pub fn test_namespace() -> Option<String> {
    active().ok().flatten().map(|n| n.root().display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("harpoon-ns-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn home(name: &str) -> PathBuf {
        let h = dir(name);
        std::fs::create_dir_all(h.join("Library/Application Support/Harpoon")).unwrap();
        std::fs::create_dir_all(h.join("Library/Application Support/com.harpoon.desktop")).unwrap();
        h
    }
    fn some(v: &Path) -> Option<&str> {
        v.to_str()
    }

    #[test]
    fn a_release_build_ignores_the_environment_whatever_it_says() {
        let h = home("release");
        let ns = dir("release-root");
        assert_eq!(resolve(some(&ns), false, &h), Ok(None));
        assert_eq!(resolve(Some("relative/garbage"), false, &h), Ok(None), "not even validated: it is not read");
        assert_eq!(resolve(Some("/tmp"), false, &h), Ok(None));
        // and the shipping function never touches the environment at all
        let src = include_str!("namespace.rs");
        let release = src.split("#[cfg(not(debug_assertions))]").nth(1).unwrap().split("#[cfg(debug_assertions)]").next().unwrap();
        assert!(release.contains("Ok(None)") && !release.contains("env::") && !release.contains("ENV"), "{release}");
    }

    #[test]
    fn unset_or_empty_is_no_namespace_in_a_debug_build() {
        let h = home("unset");
        assert_eq!(resolve(None, true, &h), Ok(None));
        assert_eq!(resolve(Some(""), true, &h), Ok(None));
    }

    #[test]
    fn a_valid_root_derives_every_path_from_itself() {
        let h = home("valid");
        let root = dir("valid-root");
        let ns = resolve(some(&root), true, &h).unwrap().unwrap();
        assert_eq!(ns.root(), root.as_path());
        assert_eq!(ns.support_dir(), root, "runtime.json and runtime.pid are read from the root, as the engine writes them there");
        assert_eq!(ns.docker_socket(), root.join("harpoon-docker.sock"));
        assert_eq!(ns.mgmt_socket(), root.join("harpoon-mgmt.sock"));
        assert_eq!(ns.data_dir(), root.join("desktop-data"));
        assert_eq!(ns.docker_config_dir(), root.join("desktop-data/docker-config"));
        for p in [ns.support_dir(), ns.docker_socket(), ns.mgmt_socket(), ns.data_dir(), ns.docker_config_dir()] {
            assert!(p.starts_with(&root), "{}", p.display());
            assert!(!p.to_string_lossy().contains("Library/Application Support"), "{}", p.display());
        }
        // a trailing slash does not change the derived names
        let slashed = format!("{}/", root.display());
        assert_eq!(resolve(Some(&slashed), true, &h).unwrap().unwrap().docker_socket(), root.join("harpoon-docker.sock"));
    }

    #[test]
    fn an_unsafe_root_is_an_error_never_a_fallback_to_production() {
        let h = home("unsafe");
        let err = |v: Option<&str>| resolve(v, true, &h).unwrap_err();
        assert!(err(Some("relative/dir")).contains("absolute"));
        assert!(err(Some("/definitely/not/there")).contains("cannot be resolved"));
        assert!(err(Some("/tmp")).contains("/tmp"));
        // a symlink to /tmp is /tmp
        let link = dir("link-parent").join("to-tmp");
        std::os::unix::fs::symlink("/tmp", &link).unwrap();
        assert!(err(some(&link)).contains("/tmp"));
        // the production directories, a path inside one, a symlink to one, and a root that contains one
        let prod = h.join("Library/Application Support/Harpoon");
        assert!(err(some(&prod)).contains("production"));
        let desk = h.join("Library/Application Support/com.harpoon.desktop");
        assert!(err(some(&desk)).contains("production"));
        let inside = prod.join("sub");
        std::fs::create_dir_all(&inside).unwrap();
        assert!(err(some(&inside)).contains("production"));
        let to_prod = dir("link-prod").join("p");
        std::os::unix::fs::symlink(&prod, &to_prod).unwrap();
        assert!(err(some(&to_prod)).contains("production"));
        assert!(err(some(&h.join("Library/Application Support"))).contains("production"));
        // not a directory
        let file = dir("file-root").join("f");
        std::fs::write(&file, "x").unwrap();
        assert!(err(some(&file)).contains("not a directory"));
    }

    #[test]
    fn a_root_others_can_write_or_that_is_too_long_for_a_socket_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let h = home("modes");
        let open = dir("open-root");
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(resolve(some(&open), true, &h).unwrap_err().contains("writable by group or others"));
        let long = dir(&"x".repeat(120));
        assert!(resolve(some(&long), true, &h).unwrap_err().contains("too long"));
        // a root owned by someone other than the home directory's owner is refused
        let other_home = PathBuf::from("/");
        let root = dir("owner-root");
        assert!(resolve(some(&root), true, &other_home).unwrap_err().contains("not owned"));
    }

    #[test]
    fn docker_calls_are_translated_to_the_namespace_socket_or_refused() {
        let h = home("docker");
        let root = dir("docker-root");
        let ns = resolve(some(&root), true, &h).unwrap().unwrap();
        let a = ns.docker_args(&["--context", "harpoon", "ps", "-a", "--format", "{{json .}}"]).unwrap();
        assert_eq!(a[..2], ["-H".to_string(), format!("unix://{}/harpoon-docker.sock", root.display())]);
        assert_eq!(a[2..], ["ps", "-a", "--format", "{{json .}}"]);
        assert!(!a.iter().any(|x| x == "--context" || x == "harpoon"), "the production context is never named: {a:?}");
        // anything that does not name the Harpoon context could reach the user's default Docker or contexts
        for bad in [vec!["ps"], vec!["context", "inspect", "harpoon"], vec!["--context", "desktop-linux", "ps"], vec!["-H", "unix:///tmp/harpoon-docker.sock", "ps"], vec![]] {
            assert!(ns.docker_args(&bad).is_err(), "{bad:?}");
        }
        let mut cmd = std::process::Command::new("docker");
        ns.confine_docker(&mut cmd);
        let envs: Vec<_> = cmd.get_envs().map(|(k, v)| (k.to_string_lossy().to_string(), v.map(|v| v.to_string_lossy().to_string()))).collect();
        assert!(envs.contains(&("DOCKER_CONFIG".into(), Some(ns.docker_config_dir().display().to_string()))), "{envs:?}");
        assert!(envs.contains(&("DOCKER_HOST".into(), None)) && envs.contains(&("DOCKER_CONTEXT".into(), None)), "inherited host and context are removed: {envs:?}");
        assert!(ns.docker_config_dir().is_dir());
    }

    #[test]
    fn a_namespace_that_is_active_refuses_features_that_reach_the_serving_engine() {
        // this test binary is a debug build; unless the variable is set for the whole test run, no namespace is active
        if std::env::var(ENV).is_err() {
            assert_eq!(active(), Ok(None));
            assert!(refuse_in_namespace("anything").is_ok());
        }
    }
}
