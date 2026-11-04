//! `verify_local` against real `git` and real directories: each way a local source can be wrong gets its own
//! answer, and a good one yields the branch's commit.

mod support;

use harpoon_deploy::logs::Redactor;
use harpoon_deploy_git::local::{verify_local, LocalProblem};
use harpoon_deploy_git::*;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use support::*;

fn check(t: &Tmp, dir: &Path, branch: &str) -> Result<String, LocalProblem> {
    let ws = Workspace::create(&t.0.join("scratch"), "w").unwrap();
    verify_local(
        &ws,
        &format!("file://{}", dir.display()),
        branch,
        &Settings::default(),
        &Redactor::none(),
    )
}

#[test]
fn a_repository_with_the_branch_yields_its_commit() {
    let t = Tmp::new();
    let (repo, _, second) = fixture(&t.0);
    assert_eq!(check(&t, &repo, "main"), Ok(second));
}

#[test]
fn a_missing_folder_is_reported_as_missing() {
    let t = Tmp::new();
    let p = check(&t, &t.0.join("nope"), "main").unwrap_err();
    assert!(matches!(p, LocalProblem::Missing(_)), "{p:?}");
    assert!(p.to_string().contains("does not exist"));
}

#[test]
fn a_file_is_not_a_folder() {
    let t = Tmp::new();
    let f = t.0.join("f");
    std::fs::write(&f, "x").unwrap();
    assert!(matches!(check(&t, &f, "main"), Err(LocalProblem::NotADirectory(_))));
}

#[test]
fn a_folder_that_is_not_a_repository_says_so() {
    let t = Tmp::new();
    let d = t.0.join("plain");
    std::fs::create_dir(&d).unwrap();
    std::fs::write(d.join("Dockerfile"), "FROM scratch\n").unwrap();
    let p = check(&t, &d, "main").unwrap_err();
    assert!(matches!(p, LocalProblem::NotARepository(_)), "{p:?}");
    assert!(p.to_string().contains("not a Git repository"));
}

#[test]
fn a_subfolder_of_a_repository_is_not_the_repository() {
    let t = Tmp::new();
    let (repo, _, _) = fixture(&t.0);
    let sub = repo.join("sub");
    std::fs::create_dir(&sub).unwrap();
    assert!(matches!(check(&t, &sub, "main"), Err(LocalProblem::NotARepository(_))));
}

#[test]
fn an_empty_repository_is_reported_as_empty() {
    let t = Tmp::new();
    let d = t.0.join("empty");
    std::fs::create_dir(&d).unwrap();
    git(&d, &["init", "-q"]);
    assert!(matches!(check(&t, &d, "main"), Err(LocalProblem::Empty(_))));
}

#[test]
fn a_missing_branch_lists_the_branches_that_exist() {
    let t = Tmp::new();
    let (repo, _, _) = fixture(&t.0);
    match check(&t, &repo, "develop").unwrap_err() {
        LocalProblem::MissingBranch { branch, available, .. } => {
            assert_eq!(branch, "develop");
            assert!(available.iter().any(|b| b == "main"), "{available:?}");
        }
        p => panic!("{p:?}"),
    }
}

#[test]
fn an_unreadable_folder_is_a_permission_problem() {
    let t = Tmp::new();
    let (repo, _, _) = fixture(&t.0);
    std::fs::set_permissions(&repo, std::fs::Permissions::from_mode(0o000)).unwrap();
    let root = std::fs::read_dir(&repo).is_ok(); // permissions do not bind root
    let r = check(&t, &repo, "main");
    std::fs::set_permissions(&repo, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !root {
        assert!(matches!(r, Err(LocalProblem::Permission(..))), "{r:?}");
    }
}
