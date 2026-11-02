//! Real SQLite fixtures: what the desktop shows is exactly what the engine recorded.

use super::*;
use harpoon_deploy::engine::Journal;
use harpoon_deploy::ids::ReleaseId;
use harpoon_deploy::secrets::MemorySecretStore;
use harpoon_deploy::state::{DeploymentStatus, Trigger};

const APP: &str = "app_00000000000000000000000001";
const D1: &str = "dep_00000000000000000000000001";
const D2: &str = "dep_00000000000000000000000002";

fn rt(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("harpoon-deploy-api-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn dep(id: &str, finish: Option<DeploymentStatus>) -> Deployment {
    let mut d = Deployment::new(id.parse().unwrap(), APP.parse().unwrap(), Trigger::Cli, 100);
    d.transition(DeploymentStatus::Queued, 101).unwrap();
    d.transition(DeploymentStatus::Running, 102).unwrap();
    if let Some(f) = finish {
        if f == DeploymentStatus::Succeeded {
            d.release = Some(ReleaseId::from_deployment(&d.id));
        }
        d.transition(f, 103).unwrap();
    }
    d
}

fn seed(dir: &Path, deps: &[Deployment]) {
    prepare(dir).unwrap();
    let (mut j, _) = SqliteJournal::open(&journal_dir(dir)).unwrap();
    for d in deps {
        j.persist(d).unwrap();
    }
}

#[test]
fn nothing_recorded_is_reported_as_missing_not_invented() {
    let s = snapshot(&rt("missing"), 5);
    assert_eq!(
        (s.store.as_str(), s.apps.len(), s.repos.len(), s.definitions.len()),
        ("missing", 0, 0, 0)
    );
    assert!(
        !s.gate.live_deploy && !s.gate.live_replacement,
        "the shipping gate stays closed"
    );
}

#[test]
fn recorded_deployments_are_shown_as_recorded() {
    let dir = rt("recorded");
    seed(
        &dir,
        &[
            dep(D1, Some(DeploymentStatus::Succeeded)),
            dep(D2, Some(DeploymentStatus::Failed)),
        ],
    );
    let s = snapshot(&dir, 5);
    assert_eq!(s.store, "ok");
    assert_eq!(s.apps.len(), 1);
    let a = &s.apps[0];
    assert_eq!(a.id, APP);
    assert_eq!(
        a.deployments.iter().map(|d| d.status.as_str()).collect::<Vec<_>>(),
        ["succeeded", "failed"]
    );
    assert!(a.deployments[0].release.is_some() && a.deployments[1].release.is_none());
    assert!(a.serving.is_none(), "no ledger entry, so nothing is claimed as serving");
    assert!(a.pauses.is_empty() && a.events.is_empty());
}

#[test]
fn an_unfinished_deployment_with_no_writer_is_unfinished_and_not_held() {
    let dir = rt("interrupted");
    seed(&dir, &[dep(D1, None)]);
    let s = snapshot(&dir, 5);
    assert!(s.apps[0].deployments[0].unfinished);
    assert!(
        !s.writer_held,
        "nothing is working on it: the UI must call it interrupted, not running"
    );
}

#[test]
fn a_damaged_store_is_an_error_with_a_reason_and_no_apps() {
    let dir = rt("corrupt");
    seed(&dir, &[dep(D1, Some(DeploymentStatus::Succeeded))]);
    let file = journal_dir(&dir).join(harpoon_deploy_journal::SQLITE_FILE);
    std::fs::write(&file, b"this is not a database at all").unwrap();
    let s = snapshot(&dir, 5);
    assert_eq!(s.store, "error");
    assert!(s.store_error.is_some() && s.apps.is_empty());
}

#[test]
fn live_operations_are_refused_by_the_gate_with_the_engines_reason_and_change_nothing() {
    let dir = rt("gate");
    let kc = MemorySecretStore::new();
    for op in [
        DeployOp::Rollback {
            app: APP.into(),
            release: "rel_00000000000000000000000001".into(),
        },
        DeployOp::Restore { deployment: D1.into() },
    ] {
        let r = run_op(&dir, &op, &kc);
        assert!(!r.ok && r.refused_by_gate, "{}", r.detail);
        assert!(!r.summary.is_empty());
    }
    assert_eq!(snapshot(&dir, 1).apps.len(), 0);
}

#[test]
fn a_secret_travels_on_stdin_to_the_keychain_and_is_never_in_output_or_snapshot() {
    let dir = rt("secret");
    let kc = MemorySecretStore::new();
    let value = "s3cr3t-planted-value-42";
    let r = run_op(
        &dir,
        &DeployOp::SetSecret {
            scope: format!("app:{APP}"),
            name: "DB".into(),
            value: value.into(),
            build: false,
            runtime: true,
        },
        &kc,
    );
    assert!(r.ok, "{}", r.detail);
    assert!(!r.detail.contains(value) && !r.summary.contains(value));
    let s = snapshot(&dir, 1);
    let def = s
        .definitions
        .iter()
        .find(|d| d.name == "DB")
        .expect("recorded definition");
    assert_eq!(
        (def.kind.as_str(), def.version, def.value.as_deref()),
        ("secret", Some(1), None)
    );
    assert!(!serde_json::to_string(&s).unwrap().contains(value));
    // config is a plain value and says so
    let r = run_op(
        &dir,
        &DeployOp::SetConfig {
            scope: format!("app:{APP}"),
            name: "MODE".into(),
            value: "prod".into(),
            build: false,
            runtime: true,
        },
        &kc,
    );
    assert!(r.ok, "{}", r.detail);
    let s = snapshot(&dir, 2);
    assert_eq!(
        s.definitions
            .iter()
            .find(|d| d.name == "MODE")
            .unwrap()
            .value
            .as_deref(),
        Some("prod")
    );
}

#[test]
fn a_locked_keychain_is_reported_not_swallowed() {
    let dir = rt("locked");
    let kc = MemorySecretStore::new();
    kc.set_mode(harpoon_deploy::secrets::FakeMode::Locked);
    let r = run_op(
        &dir,
        &DeployOp::SetSecret {
            scope: format!("app:{APP}"),
            name: "DB".into(),
            value: "x".into(),
            build: false,
            runtime: true,
        },
        &kc,
    );
    assert!(!r.ok && !r.refused_by_gate);
    assert!(r.detail.to_lowercase().contains("lock"), "{}", r.detail);
    assert!(snapshot(&dir, 1).definitions.is_empty());
}

#[test]
fn while_a_writer_holds_the_journal_the_snapshot_says_so_and_still_reads() {
    let dir = rt("held");
    prepare(&dir).unwrap();
    let (mut j, _) = SqliteJournal::open(&journal_dir(&dir)).unwrap();
    j.persist(&dep(D1, None)).unwrap();
    let s = snapshot(&dir, 5);
    assert_eq!(s.store, "ok", "{:?}", s.store_error);
    assert!(s.writer_held && s.apps[0].deployments[0].unfinished);
    drop(j);
    assert!(
        !snapshot(&dir, 6).writer_held,
        "after the process is gone the same record is interrupted, not running"
    );
}

#[test]
fn adding_a_repository_records_it_and_deploying_it_is_refused_by_the_gate() {
    let dir = rt("repo");
    let kc = MemorySecretStore::new();
    let add = DeployOp::AddRepo {
        name: "shop".into(),
        url: "https://example.com/acme/shop.git".into(),
        app: None,
        project: Some("acme".into()),
        environment: None,
        branch: Some("main".into()),
        filters: vec![],
        container_port: 3000,
        dockerfile: Some("Dockerfile".into()),
        context: None,
        target: None,
        health_path: Some("/healthz".into()),
        username: None,
        known_host: None,
        webhook: None,
    };
    let r = run_op(&dir, &add, &kc);
    assert!(r.ok, "{}", r.detail);
    let s = snapshot(&dir, 1);
    assert_eq!(
        (
            s.repos[0].name.as_str(),
            s.repos[0].branch.as_str(),
            s.repos[0].container_port,
            s.repos[0].project.as_deref()
        ),
        ("shop", "main", 3000, Some("acme"))
    );
    assert_eq!(
        s.apps.len(),
        1,
        "the registered application exists with no deployments and no serving release"
    );
    assert!(s.apps[0].deployments.is_empty() && s.apps[0].serving.is_none());
    let d = run_op(
        &dir,
        &DeployOp::Deploy {
            repo: "shop".into(),
            branch: None,
            tag: None,
            commit: None,
        },
        &kc,
    );
    assert!(!d.ok && d.refused_by_gate, "{}", d.detail);
    assert!(
        snapshot(&dir, 2).apps[0].deployments.is_empty(),
        "a refused deployment leaves no record"
    );
}
