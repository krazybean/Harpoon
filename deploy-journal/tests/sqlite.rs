use harpoon_deploy::engine::Journal;
use harpoon_deploy::ids::{ApplicationId, DeploymentId};
use harpoon_deploy::ledger::{Disposition, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::state::{Deployment, DeploymentStatus, Trigger};
use harpoon_deploy_journal::{SqliteJournal, SQLITE_FILE};
use std::path::PathBuf;

fn temp_dir() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let p = std::env::temp_dir().join(format!(
        "harpoon-sqlite-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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

fn legacy_journal(dir: &std::path::Path) -> (Deployment, LedgerRecord) {
    let (app, dep) = ids();
    let mut deployment = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, 10);
    deployment.transition(DeploymentStatus::Queued, 11).unwrap();
    let record = LedgerRecord::Intent {
        key: ResourceKey::new(ResourceKind::PortLease, "38001"),
        app,
        deployment: dep,
        disposition: Disposition::RemoveOnFailure,
        at_ms: 11,
    };
    let (mut old, _) = harpoon_deploy_journal::FileJournal::open(dir).unwrap();
    old.persist(&deployment).unwrap();
    old.record(&record).unwrap();
    (deployment, record)
}

#[test]
fn a_runtime_with_only_the_legacy_journal_is_readable_without_creating_anything() {
    let dir = temp_dir();
    legacy_journal(&dir);
    let listing = || {
        let mut v: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        v.sort();
        v
    };
    let before = listing();
    let restored = SqliteJournal::read_only(&dir).unwrap();
    assert_eq!(restored.deployments.len(), 1);
    assert_eq!(restored.ledger.leased_ports(), vec![38001]);
    assert_eq!(listing(), before, "reading created nothing");
    assert!(!dir.join(SQLITE_FILE).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_legacy_journal_is_imported_once_and_left_untouched() {
    let dir = temp_dir();
    legacy_journal(&dir);
    let wal = std::fs::read(dir.join("journal.wal")).unwrap();
    {
        let (_db, restored) = SqliteJournal::open(&dir).unwrap();
        assert_eq!(restored.deployments.len(), 1);
        assert_eq!(restored.ledger.leased_ports(), vec![38001]);
    }
    let (_db, again) = SqliteJournal::open(&dir).unwrap();
    assert_eq!(again.deployments.len(), 1, "imported once, not twice");
    assert_eq!(again.ledger.entries().len(), 1);
    assert_eq!(std::fs::read(dir.join("journal.wal")).unwrap(), wal);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_damaged_legacy_journal_imports_nothing_and_fails_closed() {
    let dir = temp_dir();
    legacy_journal(&dir);
    let mut wal = std::fs::read(dir.join("journal.wal")).unwrap();
    let at = wal.len() / 2;
    wal[at] ^= 0x55;
    std::fs::write(dir.join("journal.wal"), &wal).unwrap();
    assert!(SqliteJournal::open(&dir).is_err());
    assert!(SqliteJournal::read_only(&dir).is_err());
    // nothing half-imported: once the journal is whole again the import starts from scratch
    wal[at] ^= 0x55;
    std::fs::write(dir.join("journal.wal"), &wal).unwrap();
    let (_db, restored) = SqliteJournal::open(&dir).unwrap();
    assert_eq!(restored.deployments.len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_writer_lock_is_visible_to_readers_without_taking_it() {
    use harpoon_deploy_journal::{writer_lock_status, LockStatus};
    let dir = temp_dir();
    assert_eq!(
        writer_lock_status(&dir).unwrap(),
        LockStatus::NotInitialized
    );
    let (db, _) = SqliteJournal::open(&dir).unwrap();
    assert_eq!(writer_lock_status(&dir).unwrap(), LockStatus::Held);
    drop(db);
    assert_eq!(writer_lock_status(&dir).unwrap(), LockStatus::Free);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_pinned_configuration_and_the_single_explicit_recovery_survive_restarts_and_nothing_else_may_change(
) {
    use harpoon_deploy::release::ReleaseConfig;
    use harpoon_deploy::state::{CutoverState, Reason, RecoveryOutcome, Step};
    let dir = temp_dir();
    let (app, dep) = ids();
    let mut d = Deployment::new(dep, app, Trigger::Cli, 10);
    d.transition(DeploymentStatus::Queued, 11).unwrap();
    d.transition(DeploymentStatus::Running, 12).unwrap();
    d.pinned = Some(ReleaseConfig::simulated(38042));
    d.start_step(Step::Cutover, 13).unwrap();
    d.begin_cutover(14).unwrap();
    {
        let (mut db, _) = SqliteJournal::open(&dir).unwrap();
        db.persist(&d).unwrap();
    }
    // restart: interrupted, pin intact, application possibly down
    let (mut db, restored) = SqliteJournal::open_recovered(&dir, 20).unwrap();
    let mut got = restored.deployments[0].clone();
    assert_eq!(got.status, DeploymentStatus::Interrupted);
    assert_eq!(got.pinned, Some(ReleaseConfig::simulated(38042)));
    assert!(got.application_may_be_down());
    // the one explicit restoration: in progress, then its outcome
    got.begin_explicit_recovery(60_000, 30).unwrap();
    db.persist(&got).unwrap();
    got.finish_recovery(RecoveryOutcome::Restored, 31).unwrap();
    got.finish_step(Step::Recovery, true, "restored", 31)
        .unwrap();
    db.persist(&got).unwrap();
    drop(db);
    let r = SqliteJournal::read_only(&dir).unwrap();
    assert_eq!(
        r.deployments[0].recovery.unwrap().outcome,
        RecoveryOutcome::Restored
    );
    assert!(matches!(
        r.deployments[0].cutover,
        CutoverState::Failed { .. }
    ));
    assert!(!r.deployments[0].application_may_be_down());
    // a second, different outcome is not accepted: the store refuses to open (fail closed)
    let mut again = r.deployments[0].clone();
    again.recovery.as_mut().unwrap().outcome = RecoveryOutcome::RestoreFailed(Reason::StartFailed);
    {
        let (mut db, _) = SqliteJournal::open(&dir).unwrap();
        db.persist(&again).unwrap();
    }
    assert!(SqliteJournal::open(&dir).is_err());
    assert!(SqliteJournal::read_only(&dir).is_err());
    let _ = std::fs::remove_dir_all(dir);
}
