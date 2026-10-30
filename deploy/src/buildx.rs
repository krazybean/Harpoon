//! Resolution of Harpoon's bundled Docker Buildx (Milestone B2). Pure: no filesystem or process
//! access of its own — the caller supplies a [`FileProbe`], so the rule is testable offline.
//!
//! The rule is deliberately narrow. The only acceptable builder is
//! `<Resources>/harpoon/bin/docker-buildx` inside Harpoon.app. There is **no search path and no
//! fallback**: not `PATH`, not Homebrew, not Docker Desktop's CLI plugin directory, not
//! `~/.docker/cli-plugins`, not the legacy `docker build`. If the bundled file is missing or
//! unusable the build step fails with an actionable error.

use std::fmt;
use std::path::{Path, PathBuf};

/// Where the bundled binary lives, relative to the app's `Contents/Resources`.
pub const BUNDLED_RELATIVE: &str = "harpoon/bin/docker-buildx";

/// What a filesystem probe can say about one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    Missing,
    Directory,
    Symlink,
    File { executable: bool },
}

/// The only filesystem question the resolver asks. It must not follow symlinks.
pub trait FileProbe {
    fn kind(&self, path: &Path) -> FileKind;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildxBinary {
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildxError {
    /// The app's Resources directory could not be determined.
    NoResourcesDir,
    Missing(PathBuf),
    NotAFile(PathBuf),
    /// A symlink could point anywhere (including a user-installed buildx); refused.
    Symlink(PathBuf),
    NotExecutable(PathBuf),
}

const NO_FALLBACK: &str =
    "Harpoon never falls back to Docker Desktop, Homebrew, a user-installed buildx or the legacy builder.";

impl fmt::Display for BuildxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildxError::NoResourcesDir => write!(
                f,
                "Cannot locate Harpoon's bundled Docker Buildx: the application's Resources directory is unknown. {NO_FALLBACK}"
            ),
            BuildxError::Missing(p) => write!(
                f,
                "Harpoon's bundled Docker Buildx is missing ({}). Reinstall Harpoon; in a development checkout run `sh harpoon/fetch-buildx.sh` and `bash ui/harpoon-desktop/src-tauri/prepare-bundle.sh`. {NO_FALLBACK}",
                p.display()
            ),
            BuildxError::NotAFile(p) => write!(
                f,
                "Harpoon's bundled Docker Buildx is not a regular file ({}). Reinstall Harpoon. {NO_FALLBACK}",
                p.display()
            ),
            BuildxError::Symlink(p) => write!(
                f,
                "Harpoon's bundled Docker Buildx is a symbolic link ({}); it must be the verified file itself. Reinstall Harpoon. {NO_FALLBACK}",
                p.display()
            ),
            BuildxError::NotExecutable(p) => write!(
                f,
                "Harpoon's bundled Docker Buildx is not executable ({}). Reinstall Harpoon, or run `chmod 755` on it in a development checkout. {NO_FALLBACK}",
                p.display()
            ),
        }
    }
}

impl std::error::Error for BuildxError {}

/// Resolve the bundled binary. Probes exactly one path.
pub fn resolve(resources_dir: Option<&Path>, probe: &dyn FileProbe) -> Result<BuildxBinary, BuildxError> {
    let dir = resources_dir
        .filter(|d| !d.as_os_str().is_empty())
        .ok_or(BuildxError::NoResourcesDir)?;
    let path = dir.join(BUNDLED_RELATIVE);
    match probe.kind(&path) {
        FileKind::File { executable: true } => Ok(BuildxBinary { path }),
        FileKind::File { executable: false } => Err(BuildxError::NotExecutable(path)),
        FileKind::Missing => Err(BuildxError::Missing(path)),
        FileKind::Symlink => Err(BuildxError::Symlink(path)),
        FileKind::Directory => Err(BuildxError::NotAFile(path)),
    }
}

/// The complete environment a buildx process gets. The caller must clear the inherited
/// environment (`env_clear`) and pass exactly these variables, so nothing from the user's shell
/// or Docker configuration (`DOCKER_CONTEXT`, `DOCKER_CONFIG`, `BUILDX_*`, `DOCKER_BUILDKIT`,
/// registry credentials, proxies) can reach it.
pub fn isolated_env(harpoon_socket: &Path, scratch_dir: &Path) -> Vec<(String, String)> {
    let scratch = scratch_dir.display().to_string();
    vec![
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOME".into(), scratch.clone()),
        ("DOCKER_CONFIG".into(), scratch),
        ("DOCKER_HOST".into(), format!("unix://{}", harpoon_socket.display())),
    ]
}
