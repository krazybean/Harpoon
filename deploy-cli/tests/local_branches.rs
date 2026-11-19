//! Branch discovery for the desktop's branch dropdown: local branches only, read-only.

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("/usr/bin/git")
        .args([
            "-C",
            dir.to_str().unwrap(),
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

fn repo(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("harpoon-branches-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    git(&d, &["init", "-q", "-b", "main"]);
    d
}

fn head(dir: &Path) -> String {
    let o = Command::new("/usr/bin/git")
        .args(["-C", dir.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(o.stdout).unwrap()
}

#[test]
fn lists_local_branches_only_and_changes_nothing() {
    let d = repo("list");
    git(&d, &["commit", "-q", "--allow-empty", "-m", "one"]);
    git(&d, &["branch", "feature/x"]);
    // a remote-tracking ref and a tag are not local branches
    git(
        &d,
        &["update-ref", "refs/remotes/origin/remote-only", "HEAD"],
    );
    git(&d, &["tag", "v1"]);
    let before = head(&d);
    let mut got = harpoon_deploy_cli::local_branches(d.to_str().unwrap()).unwrap();
    got.sort();
    assert_eq!(got, ["feature/x", "main"]);
    assert_eq!(head(&d), before, "listing never moves HEAD");
    assert!(
        std::fs::read_dir(&d)
            .unwrap()
            .all(|e| e.unwrap().file_name() == ".git"),
        "working tree untouched"
    );
}

#[test]
fn detached_head_still_lists_the_branches() {
    let d = repo("detached");
    git(&d, &["commit", "-q", "--allow-empty", "-m", "one"]);
    git(&d, &["checkout", "-q", "--detach"]);
    assert_eq!(
        harpoon_deploy_cli::local_branches(d.to_str().unwrap()).unwrap(),
        ["main"]
    );
}

#[test]
fn an_empty_repository_has_no_branches() {
    let d = repo("empty");
    assert_eq!(
        harpoon_deploy_cli::local_branches(d.to_str().unwrap()).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn a_folder_that_is_not_a_repository_or_not_absolute_is_an_error() {
    let d = std::env::temp_dir().join(format!("harpoon-branches-{}-plain", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    assert!(harpoon_deploy_cli::local_branches(d.to_str().unwrap()).is_err());
    assert!(harpoon_deploy_cli::local_branches("relative/dir").is_err());
    assert!(harpoon_deploy_cli::local_branches("/no/such/folder/anywhere").is_err());
}
