//! The packaging boundary: the acceptance executable exists only when its Cargo feature is asked
//! for by name, nothing that packages a release builds or names it, and the shipping binary keeps
//! the closed gate whatever features are on. These read the repository's own files.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(p: impl AsRef<Path>) -> String {
    std::fs::read_to_string(p.as_ref()).unwrap_or_else(|e| panic!("{}: {e}", p.as_ref().display()))
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if matches!(name.as_str(), "target" | "node_modules" | ".git") {
            continue;
        }
        if p.is_dir() {
            files_under(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn the_acceptance_binary_needs_its_feature_and_the_feature_is_not_a_default() {
    let manifest = read(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"));
    let bin = manifest
        .split("[[bin]]")
        .find(|b| b.contains("harpoon-deploy-acceptance"))
        .expect("the acceptance bin is declared");
    assert!(
        bin.contains("required-features = [\"acceptance\"]"),
        "{bin}"
    );
    assert!(manifest.contains("[features]") && manifest.contains("acceptance = []"));
    assert!(
        !manifest.contains("default ="),
        "no default features: a plain build must not select `acceptance`"
    );
    // the feature adds no dependency and the shipping bin is a different target
    assert!(manifest.contains("name = \"harpoon-deploy\"\npath = \"src/main.rs\""));
    assert_eq!(manifest.matches("[[bin]]").count(), 2);
}

#[test]
fn the_shipping_binary_ignores_features_and_keeps_the_closed_gate() {
    let main = read(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"));
    assert!(main.contains("Gate::shipping()"));
    assert!(
        !main.contains("cfg(") && !main.contains("feature") && !main.contains("env::var"),
        "main.rs must not depend on features or the environment:\n{main}"
    );
    let lib = read(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"));
    let shipping = lib.split("pub const fn shipping()").nth(1).unwrap();
    let shipping = &shipping[..shipping.find('}').unwrap() + 1];
    assert!(
        shipping.contains("live_deploy_enabled: false")
            && shipping.contains("live_replacement_enabled: false"),
        "{shipping}"
    );
    // no library source branches on a feature: `--all-features` cannot open the gate
    let mut files = Vec::new();
    files_under(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    for f in files {
        let s = read(&f);
        assert!(
            !s.contains("cfg(feature") && !s.contains("cfg!(feature"),
            "{} branches on a Cargo feature",
            f.display()
        );
    }
}

#[test]
fn nothing_that_packages_a_release_builds_or_names_the_acceptance_executable() {
    let mut files = Vec::new();
    for d in [
        ".github/workflows",
        "tools",
        "ui/harpoon-desktop/src-tauri",
        "dist",
    ] {
        files_under(&repo().join(d), &mut files);
    }
    files.push(repo().join("package.json"));
    let mut deploy_workflows = 0;
    for f in files {
        let rel = f.strip_prefix(repo()).unwrap().display().to_string();
        let Ok(s) = std::fs::read_to_string(&f) else {
            continue; // binary assets
        };
        // the deploy-cli workflow is the one place that builds and tests it (never uploads it)
        if !rel.ends_with("deploy-cli.yml") {
            assert!(
                !s.contains("harpoon-deploy-acceptance") && !s.contains("acceptance_main"),
                "{rel} names the acceptance executable"
            );
        }
        // a release or bundle step never builds the deploy CLI, with or without --all-features
        let packaging = rel.contains("universal-dmg")
            || rel.starts_with("tools/release")
            || rel.contains("tauri.conf")
            || rel == "package.json";
        if packaging {
            assert!(
                !s.contains("deploy-cli") && !s.contains("harpoon-deploy"),
                "{rel} refers to the deploy CLI"
            );
        }
        if rel.ends_with("deploy-cli.yml") {
            deploy_workflows += 1;
            // the only place the feature is built or tested is the deploy-cli workflow's own job
            assert!(
                s.contains("--features acceptance"),
                "the workflow checks the acceptance build"
            );
            assert!(!s.contains("--all-features"), "{rel}");
            assert!(!s.contains("cargo build --release"), "{rel}");
        }
    }
    assert_eq!(deploy_workflows, 1);
}

#[test]
fn the_acceptance_source_has_no_fault_injection_crash_control_or_synthetic_failure() {
    let src = read(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/acceptance_main.rs"));
    for bad in [
        "experimental",
        "SIGKILL",
        "kill(",
        "INJECT",
        "fault",
        "crash",
    ] {
        assert!(
            !src.to_lowercase().contains(&bad.to_lowercase()) || src.contains("no fault injection"),
            "{bad}"
        );
    }
    // the only mention is the statement that there is none
    let lower = src.to_lowercase();
    assert_eq!(lower.matches("fault").count(), 1);
    assert_eq!(lower.matches("crash").count(), 1);
    let mut files = Vec::new();
    for d in [
        "deploy/src",
        "deploy-docker/src",
        "deploy-journal/src",
        "deploy-cli/src",
    ] {
        files_under(&repo().join(d), &mut files);
    }
    for f in files {
        let s = read(&f);
        let rel = f.strip_prefix(repo()).unwrap().display().to_string();
        for bad in [
            "experimental-fail-final-check",
            "experimental-crash-after-stop",
            "INJECTED FAULT",
            "CrashAfterStop",
            "SIGKILL",
        ] {
            assert!(!s.contains(bad), "{rel} contains {bad}");
        }
        if rel.ends_with("acceptance_main.rs") {
            continue;
        }
        assert!(!s.contains("extern \"C\" {\n    fn kill"), "{rel}");
    }
}

#[cfg(feature = "acceptance")]
mod built {
    use std::process::Command;

    #[test]
    fn the_acceptance_binary_identifies_itself_and_still_demands_authorization() {
        let bin = env!("CARGO_BIN_EXE_harpoon-deploy-acceptance");
        let o = Command::new(bin).arg("--acceptance-info").output().unwrap();
        let s = String::from_utf8_lossy(&o.stdout);
        assert!(s.contains("ACCEPTANCE BUILD") && s.contains("OPEN"), "{s}");
        // open gate, but no authorization phrase: refused before anything is read
        let o = Command::new(bin)
            .args([
                "restore",
                "--deployment",
                super::super::DEAD,
                "--runtime-dir",
                "/nonexistent-b15",
            ])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(6));
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("opt-in"),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        // and it has no fault flags
        let o = Command::new(bin)
            .args([
                "replace",
                "--app",
                super::super::APP,
                "--experimental-crash-after-stop",
            ])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2));
    }
}
