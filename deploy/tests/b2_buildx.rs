//! B2: bundled-Buildx resolution. Fake filesystem only; no I/O.

use harpoon_deploy::buildx::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

struct Fs {
    answer: FileKind,
    probed: RefCell<Vec<PathBuf>>,
}

impl Fs {
    fn new(answer: FileKind) -> Fs {
        Fs {
            answer,
            probed: RefCell::new(Vec::new()),
        }
    }
}

impl FileProbe for Fs {
    fn kind(&self, p: &Path) -> FileKind {
        self.probed.borrow_mut().push(p.to_path_buf());
        self.answer.clone()
    }
}

const RES: &str = "/Applications/Harpoon.app/Contents/Resources";

#[test]
fn a_bundled_executable_file_resolves_to_its_fixed_path() {
    let fs = Fs::new(FileKind::File { executable: true });
    let b = resolve(Some(Path::new(RES)), &fs).unwrap();
    assert_eq!(
        b.path,
        PathBuf::from("/Applications/Harpoon.app/Contents/Resources/harpoon/bin/docker-buildx")
    );
}

#[test]
fn resolution_is_deterministic_and_probes_exactly_one_path() {
    let fs = Fs::new(FileKind::File { executable: true });
    let a = resolve(Some(Path::new(RES)), &fs).unwrap();
    let b = resolve(Some(Path::new(RES)), &fs).unwrap();
    assert_eq!(a, b);
    let probed = fs.probed.borrow();
    assert_eq!(probed.len(), 2, "one probe per resolution, nothing else");
    assert!(probed.iter().all(|p| p == &a.path));
}

#[test]
fn a_missing_binary_is_an_actionable_error_with_no_fallback() {
    let fs = Fs::new(FileKind::Missing);
    let e = resolve(Some(Path::new(RES)), &fs).unwrap_err();
    assert!(matches!(e, BuildxError::Missing(_)));
    let msg = e.to_string();
    assert!(msg.contains("docker-buildx"), "{msg}");
    assert!(msg.contains("Reinstall Harpoon"), "{msg}");
    assert!(msg.contains("fetch-buildx.sh"), "tells a developer what to run: {msg}");
    assert!(msg.contains("never falls back"), "{msg}");
    assert!(
        msg.contains("Docker Desktop") && msg.contains("legacy builder"),
        "{msg}"
    );
    assert_eq!(fs.probed.borrow().len(), 1, "no second place was tried");
}

#[test]
fn unusable_files_are_refused_not_worked_around() {
    let p = |k| resolve(Some(Path::new(RES)), &Fs::new(k)).unwrap_err();
    assert!(matches!(
        p(FileKind::File { executable: false }),
        BuildxError::NotExecutable(_)
    ));
    assert!(matches!(p(FileKind::Directory), BuildxError::NotAFile(_)));
    assert!(matches!(p(FileKind::Symlink), BuildxError::Symlink(_)));
    for k in [
        FileKind::File { executable: false },
        FileKind::Directory,
        FileKind::Symlink,
    ] {
        let msg = p(k).to_string();
        assert!(
            msg.contains("Reinstall Harpoon") && msg.contains("never falls back"),
            "{msg}"
        );
    }
}

#[test]
fn an_unknown_resources_dir_fails_without_probing() {
    let fs = Fs::new(FileKind::File { executable: true });
    for dir in [None, Some(Path::new(""))] {
        assert_eq!(resolve(dir, &fs).unwrap_err(), BuildxError::NoResourcesDir);
    }
    assert!(fs.probed.borrow().is_empty());
    assert!(BuildxError::NoResourcesDir.to_string().contains("never falls back"));
}

#[test]
fn the_isolated_environment_is_complete_and_user_free() {
    let env = isolated_env(Path::new("/h/docker.sock"), Path::new("/h/scratch"));
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["PATH", "HOME", "DOCKER_CONFIG", "DOCKER_HOST"]);
    let get = |k: &str| env.iter().find(|(n, _)| n == k).unwrap().1.clone();
    assert_eq!(get("DOCKER_HOST"), "unix:///h/docker.sock");
    assert_eq!(get("HOME"), "/h/scratch");
    assert_eq!(get("DOCKER_CONFIG"), "/h/scratch");
    assert_eq!(get("PATH"), "/usr/bin:/bin");
    for (_, v) in &env {
        assert!(!v.contains("/Users/") && !v.contains(".docker") && !v.contains("homebrew"));
    }
}
