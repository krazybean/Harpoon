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

#[test]
fn a_rollback_record_survives_restarts_and_its_target_cannot_be_rewritten() {
    use harpoon_deploy::ids::ReleaseId;
    use harpoon_deploy::release::ReleaseConfig;
    let dir = temp_dir();
    let (app, dep) = ids();
    let target: ReleaseId = "rel_00000000000000000000000007".parse().unwrap();
    let source: ReleaseId = "rel_00000000000000000000000008".parse().unwrap();
    let mut d = Deployment::new(dep, app, Trigger::Cli, 10);
    d.rollback_to = Some(target.clone());
    d.transition(DeploymentStatus::Queued, 11).unwrap();
    d.transition(DeploymentStatus::Running, 12).unwrap();
    d.replaces = Some(source.clone());
    d.pinned = Some(ReleaseConfig::simulated(38077));
    {
        let (mut db, _) = SqliteJournal::open(&dir).unwrap();
        db.persist(&d).unwrap();
    }
    // the process died mid-run: the record still says what it was rolling back to
    let (mut db, restored) = SqliteJournal::open_recovered(&dir, 20).unwrap();
    let got = restored.deployments[0].clone();
    assert_eq!(got.status, DeploymentStatus::Interrupted);
    assert_eq!(got.rollback_to, Some(target.clone()));
    assert_eq!(got.replaces, Some(source));
    assert_eq!(got.pinned, Some(ReleaseConfig::simulated(38077)));
    db.persist(&got).unwrap();
    drop(db);
    let r = SqliteJournal::read_only(&dir).unwrap();
    assert_eq!(r.deployments[0].rollback_to, Some(target));
    // a terminal record cannot be turned into a different operation: the store fails closed
    let mut forged = r.deployments[0].clone();
    forged.rollback_to = None;
    {
        let (mut db, _) = SqliteJournal::open(&dir).unwrap();
        db.persist(&forged).unwrap();
    }
    assert!(SqliteJournal::open(&dir).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_pinned_environment_round_trips_through_sqlite_and_the_legacy_form_is_unchanged() {
    use harpoon_deploy::release::ReleaseConfig;
    let dir = temp_dir();
    let (app, dep) = ids();
    let mut with_env = Deployment::new(dep, app.clone(), Trigger::Cli, 10);
    with_env.transition(DeploymentStatus::Queued, 11).unwrap();
    with_env.transition(DeploymentStatus::Running, 12).unwrap();
    let mut pin = ReleaseConfig::simulated(38050);
    pin.env = vec![("A".into(), "".into()), ("B".into(), "x=y;z,w".into())];
    with_env.pinned = Some(pin.clone());
    let other_dep: DeploymentId = "dep_00000000000000000000000001".parse().unwrap();
    let mut legacy = Deployment::new(other_dep, app, Trigger::Cli, 20);
    legacy.transition(DeploymentStatus::Queued, 21).unwrap();
    legacy.transition(DeploymentStatus::Running, 22).unwrap();
    legacy.pinned = Some(ReleaseConfig::simulated(38051));
    {
        let (mut db, _) = SqliteJournal::open(&dir).unwrap();
        db.persist(&with_env).unwrap();
        db.persist(&legacy).unwrap();
    }
    // restart (read-only and writer): both pins come back exactly
    let r = SqliteJournal::read_only(&dir).unwrap();
    let got = |n: u16| {
        r.deployments
            .iter()
            .find(|d| d.pinned.as_ref().is_some_and(|p| p.host_port == n))
            .unwrap()
            .pinned
            .clone()
            .unwrap()
    };
    assert_eq!(got(38050), pin);
    assert_eq!(got(38051).env, Vec::<(String, String)>::new());
    // on disk: the environment release is v2, a release without one is byte-for-byte the old v1
    let bytes = std::fs::read(dir.join(SQLITE_FILE)).unwrap();
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"v2;img="), "an environment pin is stored as v2");
    assert!(has(b"v1;img="), "a release without one keeps the v1 form");
    // damage inside the stored environment: the checksummed record no longer verifies, and the store
    // refuses to open rather than guessing at the variables
    let at = bytes.windows(5).position(|w| w == b";env=").unwrap() + 8;
    let mut bad = bytes.clone();
    bad[at] = if bad[at] == b'0' { b'1' } else { b'0' };
    std::fs::write(dir.join(SQLITE_FILE), &bad).unwrap();
    assert!(SqliteJournal::read_only(&dir).is_err());
    assert!(SqliteJournal::open(&dir).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------- B14: pauses, held events, decisions

mod control_records {
    use super::*;
    use harpoon_deploy::control::*;
    use harpoon_deploy::state::{RecoveryOutcome, Step};

    fn failed(id: &str, app: &ApplicationId, at: u64) -> Deployment {
        let mut d = Deployment::new(id.parse().unwrap(), app.clone(), Trigger::Cli, at);
        d.transition(DeploymentStatus::Queued, at + 1).unwrap();
        d.transition(DeploymentStatus::Running, at + 2).unwrap();
        d.start_step(Step::Cutover, at + 3).unwrap();
        d.begin_cutover(at + 4).unwrap();
        d.fail_cutover(at + 5).unwrap();
        d.start_recovery(60_000, at + 6).unwrap();
        d.finish_recovery(RecoveryOutcome::Restored, at + 7)
            .unwrap();
        d.transition(DeploymentStatus::Failed, at + 8).unwrap();
        d
    }

    fn sequence(dir: &std::path::Path) -> (ControlState, Vec<Deployment>) {
        let (app, _) = ids();
        let deps = vec![failed("dep_00000000000000000000000010", &app, 100)];
        let (mut db, restored) = SqliteJournal::open(dir).unwrap();
        assert_eq!(restored.control, ControlState::default());
        for d in &deps {
            db.persist(d).unwrap();
        }
        let mut ctl = ControlState::default();
        for (id, rev, t) in [("d1", "r1", 300), ("d2", "r2", 310), ("d3", "r3", 320)] {
            let e = Incoming {
                app: &app,
                delivery_id: id,
                source: "webhook",
                revision: rev,
                received_ms: t,
            };
            assert_eq!(ingest(&mut db, &deps, &mut ctl, e).unwrap(), Ingress::Held);
        }
        resume(&mut db, &deps, &mut ctl, &app, Choice::Newest, 400).unwrap();
        (ctl, deps)
    }

    #[test]
    fn pauses_events_and_decisions_replay_exactly_after_a_restart_and_legacy_stores_stay_readable()
    {
        let dir = temp_dir();
        let (want, _) = sequence(&dir);
        assert_eq!(want.pauses.len(), 1);
        assert_eq!(want.events.len(), 3);
        // read-only and writer opens both reconstruct the same state
        assert_eq!(SqliteJournal::read_only(&dir).unwrap().control, want);
        let (_db, again) = SqliteJournal::open(&dir).unwrap();
        assert_eq!(again.control, want);
        drop(_db);
        // a store written before these records existed has none and reads as before
        let old = temp_dir();
        let (app, dep) = ids();
        let mut d = Deployment::new(dep, app, Trigger::Cli, 1);
        d.transition(DeploymentStatus::Queued, 2).unwrap();
        {
            let (mut db, _) = SqliteJournal::open(&old).unwrap();
            db.persist(&d).unwrap();
        }
        let r = SqliteJournal::read_only(&old).unwrap();
        assert_eq!(r.control, ControlState::default());
        assert_eq!(r.deployments.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(old);
    }

    #[test]
    fn a_valid_checksum_pause_that_contradicts_its_deployment_fails_closed() {
        let (app, _) = ids();
        let write = |origin: &str, kind: PauseKind, at_ms: u64| {
            let dir = temp_dir();
            let (mut db, _) = SqliteJournal::open(&dir).unwrap();
            db.persist(&failed("dep_00000000000000000000000010", &app, 100))
                .unwrap();
            // a deployment of the same application that caused no pause
            let mut plain = Deployment::new(
                "dep_00000000000000000000000011".parse().unwrap(),
                app.clone(),
                Trigger::Cli,
                200,
            );
            plain.transition(DeploymentStatus::Queued, 201).unwrap();
            db.persist(&plain).unwrap();
            db.control(&ControlRecord::Pause(PauseRec {
                app: app.clone(),
                origin: origin.parse().unwrap(),
                kind,
                at_ms,
                context: String::new(),
            }))
            .unwrap();
            drop(db);
            let r = SqliteJournal::read_only(&dir).map(|_| ());
            let _ = std::fs::remove_dir_all(dir);
            r
        };
        let ten = "dep_00000000000000000000000010";
        assert!(write(ten, PauseKind::FailedCutover, 105).is_ok());
        assert!(write(ten, PauseKind::InterruptedCutover, 104).is_ok());
        // inside the old interval but not the cutover's persisted start (104): contradiction
        assert!(write(ten, PauseKind::InterruptedCutover, 103).is_err());
        assert!(write(ten, PauseKind::InterruptedCutover, 102).is_err());
        assert!(write(ten, PauseKind::Rollback, 105).is_err());
        assert!(write(ten, PauseKind::FailedCutover, 106).is_err());
        assert!(write(
            "dep_00000000000000000000000011",
            PauseKind::FailedCutover,
            201
        )
        .is_err());
    }

    #[test]
    fn refusals_on_a_legacy_store_write_nothing_and_a_crash_between_pause_and_decision_completes() {
        let (app, _) = ids();
        let deps = vec![failed("dep_00000000000000000000000010", &app, 100)];
        let dir = temp_dir();
        let (mut db, restored) = SqliteJournal::open(&dir).unwrap();
        db.persist(&deps[0]).unwrap();
        let mut ctl = restored.control;
        let file = || std::fs::read(dir.join(SQLITE_FILE)).unwrap();
        let before = file();
        // not paused for another app, invalid revision, selection with nothing held: zero writes
        let other: ApplicationId = "app_00000000000000000000000099".parse().unwrap();
        let e = Incoming {
            app: &other,
            delivery_id: "d1",
            source: "webhook",
            revision: "r1",
            received_ms: 1,
        };
        assert_eq!(
            ingest(&mut db, &deps, &mut ctl, e).unwrap(),
            Ingress::NotPaused
        );
        for c in [Choice::Revision("nope".into()), Choice::Newest] {
            assert!(resume(&mut db, &deps, &mut ctl, &app, c, 5).is_err());
        }
        assert_eq!(
            resume(&mut db, &deps, &mut ctl, &other, Choice::Dismiss, 5).unwrap(),
            Resumed::NothingToDo
        );
        assert_eq!(file(), before, "byte-identical after refusals");
        drop(SqliteJournal::read_only(&dir).unwrap());
        assert_eq!(file(), before, "a read-only open and check mutates nothing");
        assert_eq!(ctl, ControlState::default());
        // a crash after the pause record and before the decision: reopen, repeat, same end state
        let pause = ControlRecord::Pause(PauseRec {
            app: app.clone(),
            origin: deps[0].id.clone(),
            kind: PauseKind::FailedCutover,
            at_ms: 105,
            context: "recovery restored".into(),
        });
        db.control(&pause).unwrap();
        drop(db);
        let (mut db, restored) = SqliteJournal::open(&dir).unwrap();
        let mut ctl = restored.control;
        assert_eq!(ctl.pauses.len(), 1);
        resume(&mut db, &deps, &mut ctl, &app, Choice::Dismiss, 6).unwrap();
        drop(db);
        let done = SqliteJournal::read_only(&dir).unwrap().control;
        assert_eq!((done.pauses.len(), done.resumes.len()), (1, 1));
        assert!(!is_paused(&app, &deps, &done));
        // the same decision from a clean store writes the same two records
        let dir2 = temp_dir();
        let (mut db2, _) = SqliteJournal::open(&dir2).unwrap();
        db2.persist(&deps[0]).unwrap();
        let mut c2 = ControlState::default();
        resume(&mut db2, &deps, &mut c2, &app, Choice::Dismiss, 6).unwrap();
        drop(db2);
        assert_eq!(SqliteJournal::read_only(&dir2).unwrap().control, done);
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(dir2);
    }

    #[test]
    fn damaged_or_inconsistent_control_state_fails_closed() {
        let dir = temp_dir();
        let (_, _) = sequence(&dir);
        let bytes = std::fs::read(dir.join(SQLITE_FILE)).unwrap();
        // flip a byte inside a stored event record: its checksum no longer verifies
        let at = bytes.windows(9).position(|w| w == b"\tevent\t1\t").unwrap() + 20;
        let mut bad = bytes.clone();
        bad[at] = if bad[at] == b'a' { b'b' } else { b'a' };
        let path = dir.join(SQLITE_FILE);
        std::fs::write(&path, &bad).unwrap();
        assert!(SqliteJournal::read_only(&dir).is_err());
        assert!(SqliteJournal::open(&dir).is_err());
        // an intact record that contradicts the state (a delivery recorded twice) is refused too
        std::fs::write(&path, &bytes).unwrap();
        let (app, _) = ids();
        let dup = ControlRecord::Event(EventRec {
            app,
            delivery: "d1".into(),
            source: "webhook".into(),
            revision: "r-other".into(),
            received_ms: 1,
        });
        {
            let (mut db, _) = SqliteJournal::open(&dir).unwrap();
            db.append_unchecked("control", &encode(&dup)).unwrap();
        }
        let e = SqliteJournal::read_only(&dir).unwrap_err();
        assert!(e.0.contains("recorded twice"), "{}", e.0);
        assert!(SqliteJournal::open(&dir).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_second_writer_cannot_touch_the_event_history() {
        let dir = temp_dir();
        let (want, _) = sequence(&dir);
        let (held, _) = SqliteJournal::open(&dir).unwrap();
        let before = std::fs::read(dir.join(SQLITE_FILE)).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let d = dir.clone();
                std::thread::spawn(move || SqliteJournal::open(&d).err().map(|e| e.0))
            })
            .collect();
        for h in handles {
            let e = h.join().unwrap().expect("a second writer is refused");
            assert!(e.contains("another Harpoon process holds"), "{e}");
        }
        assert_eq!(std::fs::read(dir.join(SQLITE_FILE)).unwrap(), before);
        assert_eq!(SqliteJournal::read_only(&dir).unwrap().control, want);
        drop(held);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The control sequence is cut after every write (the store keeps exactly a prefix of what a full
    /// run writes), reopened, and the same requests are made again: the final state is always the
    /// one an uninterrupted run reaches, and every prefix replays.
    #[test]
    fn a_store_cut_after_every_control_write_replays_and_completes_to_the_same_state() {
        use harpoon_deploy::engine::MemoryJournal;
        let (app, _) = ids();
        let deps = vec![failed("dep_00000000000000000000000010", &app, 100)];
        fn run<J: Journal>(
            j: &mut J,
            deps: &[Deployment],
            ctl: &mut ControlState,
            app: &ApplicationId,
        ) {
            for (id, rev, t) in [
                ("d1", "r1", 300),
                ("d2", "r2", 310),
                ("d3", "r3", 320),
                ("d2", "r2", 310),
            ] {
                let e = Incoming {
                    app,
                    delivery_id: id,
                    source: "webhook",
                    revision: rev,
                    received_ms: t,
                };
                ingest(j, deps, ctl, e).unwrap();
            }
            let _ = resume(j, deps, ctl, app, Choice::Newest, 400);
            let _ = resume(j, deps, ctl, app, Choice::Dismiss, 410);
        }
        let mut reference = MemoryJournal::new();
        let mut want = ControlState::default();
        run(&mut reference, &deps, &mut want, &app);
        let records = reference.control_records.clone();
        assert_eq!(records.len(), 6, "one pause, three events, two decisions");
        for n in 0..=records.len() {
            let dir = temp_dir();
            {
                let (mut db, _) = SqliteJournal::open(&dir).unwrap();
                for d in &deps {
                    db.persist(d).unwrap();
                }
                for r in &records[..n] {
                    db.control(r).unwrap();
                }
            }
            let (mut db, restored) = SqliteJournal::open(&dir).unwrap();
            assert_eq!(
                restored.control.pauses.len()
                    + restored.control.events.len()
                    + restored.control.resumes.len(),
                n,
                "prefix {n} replays"
            );
            let mut ctl = restored.control.clone();
            run(&mut db, &restored.deployments, &mut ctl, &app);
            drop(db);
            assert_eq!(ctl, want, "cut after write {n}");
            assert_eq!(
                SqliteJournal::read_only(&dir).unwrap().control,
                want,
                "and it replays from disk"
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
