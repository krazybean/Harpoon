//! Folder confinement: the build context and the Dockerfile must resolve, after every symlink
//! is followed, to a location inside the project root the user selected.
//!
//! Everything here is resolved with `canonicalize`, so `..`, absolute paths and symlinks are
//! judged by where they actually lead, not by how they are spelled. The canonical paths are what
//! the build is given; the check is repeated immediately before the build starts. A path that is
//! swapped between that second check and Buildx opening it cannot be excluded from user space
//! (see the B4 notes in `docs/results/DEPLOY_DOCKER_BACKEND.md`).

use std::fmt;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Dockerfiles and ignore files are small text; refuse anything unreasonable rather than read it.
pub const MAX_TEXT_FILE: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfineError {
    RootMissing(PathBuf, String),
    RootNotDirectory(PathBuf),
    ContextMissing(PathBuf, String),
    ContextNotDirectory(PathBuf),
    ContextEscapes {
        context: PathBuf,
        root: PathBuf,
    },
    /// The Dockerfile name is absolute, empty, contains `..`/`.` or a NUL.
    DockerfileName(String),
    DockerfileMissing(PathBuf),
    DockerfileNotFile(PathBuf),
    DockerfileEscapes {
        dockerfile: PathBuf,
        context: PathBuf,
    },
    /// Exists but cannot be read or searched (permissions) or is too large.
    Unreadable(PathBuf, String),
    IgnoreEscapes {
        file: PathBuf,
        context: PathBuf,
    },
}

impl fmt::Display for ConfineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ConfineError::*;
        match self {
            RootMissing(p, e) => write!(f, "the project folder {} cannot be opened: {e}", p.display()),
            RootNotDirectory(p) => write!(f, "the project path {} is not a folder", p.display()),
            ContextMissing(p, e) => write!(f, "the build context {} cannot be opened: {e}", p.display()),
            ContextNotDirectory(p) => write!(f, "the build context {} is not a folder", p.display()),
            ContextEscapes { context, root } => write!(
                f,
                "the build context resolves to {}, which is outside the selected project folder {}",
                context.display(),
                root.display()
            ),
            DockerfileName(n) => write!(
                f,
                "the Dockerfile path {n:?} must be a plain relative path inside the build context (no leading '/', no '.' or '..' parts)"
            ),
            DockerfileMissing(p) => write!(f, "the Dockerfile {} does not exist", p.display()),
            DockerfileNotFile(p) => write!(f, "the Dockerfile {} is not a regular file", p.display()),
            DockerfileEscapes { dockerfile, context } => write!(
                f,
                "the Dockerfile resolves to {}, which is outside the build context {}",
                dockerfile.display(),
                context.display()
            ),
            Unreadable(p, e) => write!(f, "cannot read {}: {e}", p.display()),
            IgnoreEscapes { file, context } => write!(
                f,
                "the ignore file resolves to {}, which is outside the build context {}",
                file.display(),
                context.display()
            ),
        }
    }
}

/// A confined source: every path is canonical and inside `root`.
#[derive(Debug, Clone)]
pub struct Source {
    pub root: PathBuf,
    pub context: PathBuf,
    pub dockerfile: PathBuf,
    pub dockerfile_text: String,
    /// The `.dockerignore` that applies, if any, and its text.
    pub ignore: Option<(PathBuf, String)>,
}

impl Source {
    /// Path of the Dockerfile relative to the context (for "always sent" handling).
    pub fn dockerfile_rel(&self) -> PathBuf {
        self.dockerfile
            .strip_prefix(&self.context)
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }
}

fn io_text(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => e.to_string(),
    }
}

fn plain_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\0')
        && !Path::new(name).is_absolute()
        && Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn read_capped(p: &Path) -> Result<String, ConfineError> {
    let f = std::fs::File::open(p)
        .map_err(|e| ConfineError::Unreadable(p.to_path_buf(), io_text(&e)))?;
    let mut s = String::new();
    f.take(MAX_TEXT_FILE + 1)
        .read_to_string(&mut s)
        .map_err(|e| ConfineError::Unreadable(p.to_path_buf(), io_text(&e)))?;
    if s.len() as u64 > MAX_TEXT_FILE {
        return Err(ConfineError::Unreadable(
            p.to_path_buf(),
            format!("larger than {} KiB", MAX_TEXT_FILE / 1024),
        ));
    }
    Ok(s)
}

/// Resolve an optional file inside `context`: `Ok(None)` when it does not exist.
fn optional_inside(path: &Path, context: &Path) -> Result<Option<(PathBuf, String)>, ConfineError> {
    match std::fs::canonicalize(path) {
        Ok(real) => {
            if !real.starts_with(context) {
                return Err(ConfineError::IgnoreEscapes {
                    file: real,
                    context: context.to_path_buf(),
                });
            }
            if !real.is_file() {
                return Err(ConfineError::Unreadable(real, "not a regular file".into()));
            }
            let text = read_capped(&real)?;
            Ok(Some((real, text)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ConfineError::Unreadable(path.to_path_buf(), io_text(&e))),
    }
}

/// Confine `context` and `dockerfile` (a relative path inside the context) to `root`.
pub fn confine(root: &Path, context: &Path, dockerfile: &str) -> Result<Source, ConfineError> {
    let root_real = std::fs::canonicalize(root)
        .map_err(|e| ConfineError::RootMissing(root.to_path_buf(), io_text(&e)))?;
    if !root_real.is_dir() {
        return Err(ConfineError::RootNotDirectory(root_real));
    }
    let ctx_real = std::fs::canonicalize(context)
        .map_err(|e| ConfineError::ContextMissing(context.to_path_buf(), io_text(&e)))?;
    if !ctx_real.starts_with(&root_real) {
        return Err(ConfineError::ContextEscapes {
            context: ctx_real,
            root: root_real,
        });
    }
    if !ctx_real.is_dir() {
        return Err(ConfineError::ContextNotDirectory(ctx_real));
    }
    if !plain_relative(dockerfile) {
        return Err(ConfineError::DockerfileName(dockerfile.to_string()));
    }
    let wanted = ctx_real.join(dockerfile);
    let df_real = match std::fs::canonicalize(&wanted) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConfineError::DockerfileMissing(wanted))
        }
        Err(e) => return Err(ConfineError::Unreadable(wanted, io_text(&e))),
    };
    if !df_real.starts_with(&ctx_real) {
        return Err(ConfineError::DockerfileEscapes {
            dockerfile: df_real,
            context: ctx_real,
        });
    }
    if !df_real.is_file() {
        return Err(ConfineError::DockerfileNotFile(df_real));
    }
    let dockerfile_text = read_capped(&df_real)?;

    // BuildKit prefers `<Dockerfile>.dockerignore` next to the Dockerfile, then `.dockerignore`
    // in the context root.
    let mut sibling = wanted.clone().into_os_string();
    sibling.push(".dockerignore");
    let ignore = match optional_inside(Path::new(&sibling), &ctx_real)? {
        Some(i) => Some(i),
        None => optional_inside(&ctx_real.join(".dockerignore"), &ctx_real)?,
    };
    Ok(Source {
        root: root_real,
        context: ctx_real,
        dockerfile: df_real,
        dockerfile_text,
        ignore,
    })
}
