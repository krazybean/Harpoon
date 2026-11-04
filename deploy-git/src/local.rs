//! Registration-time check of a `file://` source: is it a Git repository on this machine, and does it
//! have the branch it will be deployed from? Read-only; the repository is asked through the same hardened
//! `ls_remote` that polling uses, so no hook, helper or prompt can run.

use crate::runner::{branch_heads, ls_remote, Credential, GitError, Settings, Workspace};
use harpoon_deploy::git::file_url_path;
use harpoon_deploy::logs::Redactor;
use std::io::ErrorKind;

/// What is wrong with a local source. Each variant says what the person can do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalProblem {
    Missing(String),
    NotADirectory(String),
    NotARepository(String),
    Empty(String),
    MissingBranch {
        path: String,
        branch: String,
        available: Vec<String>,
    },
    Permission(String, String),
    Other(String),
}

impl std::fmt::Display for LocalProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocalProblem::Missing(p) => write!(f, "the folder {p} does not exist. Check the path, or choose the folder again"),
            LocalProblem::NotADirectory(p) => write!(f, "{p} is a file, not a folder. Choose the folder that holds the repository"),
            LocalProblem::NotARepository(p) => write!(
                f,
                "the folder {p} is not a Git repository. Run `git init` and make a first commit there, or choose a different folder"
            ),
            LocalProblem::Empty(p) => write!(
                f,
                "the Git repository at {p} has no commits yet. Make a first commit on the branch to deploy, then register it"
            ),
            LocalProblem::MissingBranch { path, branch, available } => {
                write!(f, "the Git repository at {path} has no branch {branch:?}. ")?;
                if available.is_empty() {
                    f.write_str("Create it, or register a different branch")
                } else {
                    write!(f, "Branches found: {}. Register one of them, or create {branch:?}", available.join(", "))
                }
            }
            LocalProblem::Permission(p, why) => write!(
                f,
                "Harpoon cannot read the repository at {p}: {why}. Fix its permissions or ownership (Git refuses a repository owned by another user), or choose a different folder"
            ),
            LocalProblem::Other(m) => f.write_str(m),
        }
    }
}

/// Map Git's own failure for a local path onto a problem. The path was checked first, so "not a Git
/// repository" here really is that.
fn from_git(path: &str, e: GitError) -> LocalProblem {
    let text = e.to_string();
    if text.contains("dubious ownership")
        || text.contains("Permission denied")
        || text.contains("Operation not permitted")
    {
        return LocalProblem::Permission(path.to_string(), "Git refused to open it".into());
    }
    match e {
        GitError::NotFound(_) => LocalProblem::NotARepository(path.to_string()),
        _ => LocalProblem::Other(format!("Git could not read the repository at {path}: {text}")),
    }
}

/// The commit `branch` points at, or why the source cannot be registered.
pub fn verify_local(
    ws: &Workspace,
    url: &str,
    branch: &str,
    settings: &Settings,
    redactor: &Redactor,
) -> Result<String, LocalProblem> {
    let path = url
        .strip_prefix("file://")
        .ok_or_else(|| LocalProblem::Other("not a file:// URL".into()))
        .and_then(|p| file_url_path(p).map_err(LocalProblem::Other))?;
    match std::fs::metadata(&path) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(LocalProblem::NotADirectory(path)),
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(LocalProblem::Missing(path)),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => return Err(LocalProblem::Permission(path, e.to_string())),
        Err(e) => return Err(LocalProblem::Other(format!("cannot inspect {path}: {e}"))),
    }
    if let Err(e) = std::fs::read_dir(&path) {
        return Err(LocalProblem::Permission(path, e.to_string()));
    }
    let refs = ls_remote(ws, url, &Credential::None, settings, redactor).map_err(|e| from_git(&path, e))?;
    let heads = branch_heads(&refs);
    if heads.is_empty() {
        return Err(LocalProblem::Empty(path));
    }
    match heads.iter().find(|(b, _)| b == branch) {
        Some((_, commit)) => Ok(commit.clone()),
        None => Err(LocalProblem::MissingBranch {
            path,
            branch: branch.to_string(),
            available: heads.into_iter().map(|(b, _)| b).take(8).collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_refusing_a_repository_owned_by_someone_else_is_a_permission_problem() {
        let e = GitError::Unavailable(
            "fatal: detected dubious ownership in repository at '/srv/app' | fatal: Could not read from remote repository.".into(),
        );
        assert!(matches!(from_git("/srv/app", e), LocalProblem::Permission(..)));
        let e = GitError::NotFound("fatal: '/x' does not appear to be a git repository".into());
        assert!(matches!(from_git("/x", e), LocalProblem::NotARepository(_)));
        assert!(matches!(from_git("/x", GitError::Timeout), LocalProblem::Other(_)));
    }
}
