use harpoon_deploy::engine::Journal;
use harpoon_deploy::ids::{ApplicationId, DeploymentId};
use harpoon_deploy::ledger::{Disposition, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::state::{Deployment, DeploymentStatus, Trigger};
use harpoon_deploy_journal::{SqliteJournal, SQLITE_FILE};
use std::path::PathBuf;

fn temp_dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "harpoon-sqlite-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&p).unwrap();
    p
}

fn ids() -> (ApplicationId, DeploymentId) {
    (
        "app_00000000000000000000000000".parse().unwrap(),
        "dep_00000000000000000000000000".parse().unwrap(),
    )
}

#[test]
fn sqlite_schema_replays_ordered_state_and_marks_inflight_work_interrupted() {
    let dir = temp_dir();
    let (app, dep) = ids();
    let mut deployment = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, 10);
    deployment.transition(DeploymentStatus::Queued, 11).unwrap();
    deployment
        .transition(DeploymentStatus::Running, 12)
        .unwrap();
    let record = LedgerRecord::Intent {
        key: ResourceKey::new(ResourceKind::PortLease, "38000"),
        app,
        deployment: dep,
        disposition: Disposition::RemoveOnFailure,
        at_ms: 12,
    };

    {
        let (mut db, restored) = SqliteJournal::open(&dir).unwrap();
        assert!(restored.deployments.is_empty());
        db.persist(&deployment).unwrap();
        db.record(&record).unwrap();
    }
    assert!(dir.join(SQLITE_FILE).is_file());

    let (db, restored) = SqliteJournal::open_recovered(&dir, 99).unwrap();
    assert_eq!(restored.deployments.len(), 1);
    assert_eq!(
        restored.deployments[0].status,
        DeploymentStatus::Interrupted
    );
    assert_eq!(restored.startup.interrupted.len(), 1);
    assert_eq!(restored.ledger.leased_ports(), vec![38000]);
    drop(db);

    let restored = SqliteJournal::read_only(&dir).unwrap();
    assert_eq!(
        restored.deployments[0].status,
        DeploymentStatus::Interrupted
    );
    assert_eq!(restored.ledger.leased_ports(), vec![38000]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sqlite_writer_lock_is_fail_fast() {
    let dir = temp_dir();
    let (db, _) = SqliteJournal::open(&dir).unwrap();
    let error = match SqliteJournal::open(&dir) {
        Ok(_) => panic!("second writer unexpectedly acquired the lock"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("another Harpoon process holds"));
    drop(db);
    let _ = std::fs::remove_dir_all(dir);
}
