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

/// The journal's lock belongs to an open file description, so a child that another test thread forks while a journal is
/// open (the `broadcast` tests start processes) holds a copy until it execs, and the lock is not free the instant its owner
/// lets go. That is the operating system's behaviour and the product stays fail-fast; the tests wait for the condition.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

fn lock_refused(text: &str) -> bool {
    text.contains("another Harpoon process holds")
}

/// Open the journal for writing, waiting out a lock that is still held by a stray copy.
fn open_journal(dir: &Path) -> (SqliteJournal, harpoon_deploy_journal::Restored) {
    let end = std::time::Instant::now() + LOCK_WAIT;
    loop {
        match SqliteJournal::open(&journal_dir(dir)) {
            Ok(x) => return x,
            Err(e) if lock_refused(&e.0) && std::time::Instant::now() < end => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(e) => panic!("cannot open the journal: {}", e.0),
        }
    }
}

/// A snapshot taken once the writer lock reads as free (it is free as soon as no process holds a copy of it).
fn snapshot_when_released(dir: &Path, now_ms: u64) -> EngineSnapshot {
    let end = std::time::Instant::now() + LOCK_WAIT;
    loop {
        let s = snapshot(dir, now_ms);
        if !s.writer_held || std::time::Instant::now() >= end {
            return s;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
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
    let (mut j, _) = open_journal(dir);
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
    let (mut j, _) = open_journal(&dir);
    j.persist(&dep(D1, None)).unwrap();
    let s = snapshot(&dir, 5);
    assert_eq!(s.store, "ok", "{:?}", s.store_error);
    assert!(s.writer_held && s.apps[0].deployments[0].unfinished);
    drop(j);
    assert!(
        !snapshot_when_released(&dir, 6).writer_held,
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

// ---- E.1: the runtime's coordinates reach Docker-touching commands, and only when they are true

mod runtime {
    use super::*;
    use std::os::unix::net::UnixListener;

    struct Fixture {
        support: PathBuf,
        mgmt: PathBuf,
        resources: PathBuf,
        _socks: Vec<UnixListener>,
    }

    fn fixture(name: &str, pid: u32) -> Fixture {
        let base = rt(&format!("runtime-{name}"));
        let support = base.join("support");
        let resources = base.join("resources");
        std::fs::create_dir_all(&support).unwrap();
        std::fs::create_dir_all(resources.join("harpoon/bin")).unwrap();
        std::fs::write(resources.join("harpoon/bin/docker-buildx"), b"#!/bin/sh\n").unwrap();
        // short socket paths: unix socket names are limited to about 100 bytes
        let short = std::env::temp_dir().join(format!("hpe1-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&short).unwrap();
        let docker = short.join("d.sock");
        let mgmt = short.join("m.sock");
        let socks = vec![UnixListener::bind(&docker).unwrap(), UnixListener::bind(&mgmt).unwrap()];
        std::fs::write(
            support.join("runtime.json"),
            format!(r#"{{"socketPath":"{}","pid":{pid}}}"#, docker.display()),
        )
        .unwrap();
        std::fs::write(support.join("runtime.pid"), format!("{pid}\n")).unwrap();
        Fixture {
            support,
            mgmt,
            resources,
            _socks: socks,
        }
    }

    fn runtime_of(f: &Fixture, cmd: Option<&'static str>) -> Result<RuntimeInfo, String> {
        discover_runtime(&f.support, &f.mgmt, &f.resources, &|_| cmd.map(str::to_string))
    }

    #[test]
    fn a_live_runtime_is_read_from_its_own_state_and_verified() {
        let f = fixture("ok", 4242);
        let r = runtime_of(&f, Some("harpoon")).unwrap();
        assert_eq!((r.forwarder_pid, r.forwarder_command.as_str()), (4242, "harpoon"));
        assert_eq!(r.docker_socket.file_name().unwrap(), "d.sock");
        assert_eq!(
            (r.mgmt_socket.as_path(), r.resources_dir.as_path()),
            (f.mgmt.as_path(), f.resources.as_path())
        );
    }

    #[test]
    fn missing_or_stale_runtime_state_is_refused_with_a_reason() {
        let f = fixture("stale", 4242);
        assert!(runtime_of(&f, None).unwrap_err().contains("not running"));
        assert!(runtime_of(&f, Some("bash"))
            .unwrap_err()
            .contains("not the Harpoon runtime"));
        std::fs::write(f.support.join("runtime.pid"), "999\n").unwrap();
        assert!(runtime_of(&f, Some("harpoon")).unwrap_err().contains("disagree"));
        std::fs::write(f.support.join("runtime.json"), "{not json").unwrap();
        assert!(runtime_of(&f, Some("harpoon")).unwrap_err().contains("unreadable"));
        std::fs::remove_file(f.support.join("runtime.json")).unwrap();
        assert!(runtime_of(&f, Some("harpoon")).unwrap_err().contains("missing"));
    }

    #[test]
    fn missing_sockets_and_buildx_are_refused_and_nothing_is_taken_from_a_default() {
        let f = fixture("parts", 4242);
        let no_mgmt = discover_runtime(&f.support, &f.support.join("none.sock"), &f.resources, &|_| {
            Some("harpoon".into())
        });
        assert!(no_mgmt.unwrap_err().contains("management socket"));
        let no_buildx = discover_runtime(&f.support, &f.mgmt, &f.support, &|_| Some("harpoon".into()));
        assert!(no_buildx.unwrap_err().contains("Buildx"));
        std::fs::write(
            f.support.join("runtime.json"),
            r#"{"socketPath":"/nonexistent/d.sock","pid":4242}"#,
        )
        .unwrap();
        assert!(runtime_of(&f, Some("harpoon")).unwrap_err().contains("Docker socket"));
    }

    fn info() -> RuntimeInfo {
        RuntimeInfo {
            docker_socket: "/d.sock".into(),
            mgmt_socket: "/m.sock".into(),
            resources_dir: "/res".into(),
            forwarder_pid: 4242,
            forwarder_command: "harpoon".into(),
        }
    }
    const OPEN: Gate = Gate {
        live_deploy_enabled: true,
        live_replacement_enabled: true,
    };

    fn args_of(op: &DeployOp, gate: &Gate, r: Result<RuntimeInfo, String>) -> Result<Vec<String>, OpResult> {
        build_args(op, gate, &|| r.clone()).map(|(a, _)| a)
    }
    fn pair<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
        a.iter().position(|x| x == flag).map(|i| a[i + 1].as_str())
    }

    #[test]
    fn docker_touching_commands_receive_every_coordinate_explicitly_and_never_the_authorization() {
        for op in [
            DeployOp::Deploy {
                repo: "shop".into(),
                branch: None,
                tag: None,
                commit: None,
            },
            DeployOp::RunQueue { repo: None },
            DeployOp::Rollback {
                app: "app_x".into(),
                release: "rel_x".into(),
            },
            DeployOp::Restore {
                deployment: "dep_x".into(),
            },
        ] {
            for gate in [Gate::shipping(), OPEN] {
                let a = args_of(&op, &gate, Ok(info())).unwrap();
                assert_eq!(
                    [
                        pair(&a, "--docker-socket"),
                        pair(&a, "--mgmt-socket"),
                        pair(&a, "--resources-dir"),
                        pair(&a, "--forwarder-pid"),
                        pair(&a, "--forwarder-command")
                    ],
                    [
                        Some("/d.sock"),
                        Some("/m.sock"),
                        Some("/res"),
                        Some("4242"),
                        Some("harpoon")
                    ]
                );
                assert!(
                    !a.iter()
                        .any(|x| x.contains("authorize-live") || x.contains("I-UNDERSTAND")),
                    "the opt-in is the user's act, never implicit: {a:?}"
                );
            }
        }
    }

    #[test]
    fn operations_that_do_not_reach_docker_get_no_coordinates() {
        for op in [
            DeployOp::Poll { repo: None },
            DeployOp::Recover {},
            DeployOp::Resume {
                app: "app_x".into(),
                mode: "dismiss".into(),
                revision: None,
            },
            DeployOp::SetConfig {
                scope: "global".into(),
                name: "A".into(),
                value: "1".into(),
                build: false,
                runtime: true,
            },
        ] {
            let a = args_of(&op, &OPEN, Ok(info())).unwrap();
            assert!(
                !a.iter().any(|x| x == "--docker-socket" || x == "--forwarder-pid"),
                "{a:?}"
            );
        }
    }

    #[test]
    fn once_the_gate_would_let_it_through_a_missing_runtime_refuses_before_the_engine() {
        let op = DeployOp::Deploy {
            repo: "shop".into(),
            branch: None,
            tag: None,
            commit: None,
        };
        let r = args_of(&op, &OPEN, Err("process 4242 is not running".into())).unwrap_err();
        assert!(!r.ok && r.exit_code == harpoon_deploy_cli::exit::REFUSED && !r.refused_by_gate);
        assert!(r.summary.contains("Refused before contacting Docker") && r.summary.contains("not running"));
    }

    #[test]
    fn while_the_gate_is_closed_the_gate_stays_the_first_and_only_reason() {
        let dir = rt("e1-gate");
        let kc = MemorySecretStore::new();
        let op = DeployOp::Deploy {
            repo: "shop".into(),
            branch: None,
            tag: None,
            commit: None,
        };
        // a runtime that is found: still refused by the gate; one that is not found: the same refusal
        for found in [Ok(info()), Err("no runtime".to_string())] {
            let r = run_op_with(&dir, &op, &kc, &Gate::shipping(), &|| found.clone());
            assert!(!r.ok && r.refused_by_gate, "{}", r.detail);
        }
        assert!(snapshot(&dir, 1).apps.is_empty(), "nothing was recorded");
    }
}

// ---- E.3: an empty journal directory is an uninitialised store; real damage still fails closed

#[test]
fn an_empty_journal_directory_is_uninitialised_not_damaged() {
    let dir = rt("empty-journal");
    prepare(&dir).unwrap();
    assert!(std::fs::read_dir(journal_dir(&dir)).unwrap().next().is_none());
    let s = snapshot(&dir, 1);
    assert_eq!(
        (s.store.as_str(), s.store_error.as_deref(), s.apps.len()),
        ("missing", None, 0)
    );
}

#[test]
fn a_failed_first_registration_leaves_a_store_that_still_reads_as_missing() {
    let dir = rt("failed-first");
    let kc = MemorySecretStore::new();
    let add = |url: &str| DeployOp::AddRepo {
        name: "shop".into(),
        url: url.into(),
        app: None,
        project: None,
        environment: None,
        branch: Some("main".into()),
        filters: vec![],
        container_port: 3000,
        dockerfile: None,
        context: None,
        target: None,
        health_path: None,
        username: None,
        known_host: None,
        webhook: None,
    };
    let r = run_op(&dir, &add("file:///Users/x/Harpoon UI Test"), &kc);
    assert!(!r.ok && r.summary.contains("whitespace"), "{}", r.detail);
    let s = snapshot(&dir, 1);
    assert_eq!(
        (s.store.as_str(), s.store_error.as_deref()),
        ("missing", None),
        "the refusal must not blank the screen"
    );
    // the encoded form registers, and the store then reads as ok
    let r = run_op(&dir, &add("file:///Users/x/Harpoon%20UI%20Test"), &kc);
    assert!(r.ok, "{}", r.detail);
    assert_eq!(snapshot(&dir, 2).store, "ok");
}

#[test]
fn an_unreadable_journal_file_is_still_an_error_whichever_format_it_is() {
    for (name, file) in [
        ("sqlite", harpoon_deploy_journal::SQLITE_FILE),
        ("legacy", harpoon_deploy_journal::JOURNAL_FILE),
    ] {
        let dir = rt(&format!("damaged-{name}"));
        prepare(&dir).unwrap();
        std::fs::write(journal_dir(&dir).join(file), b"not a journal at all").unwrap();
        let s = snapshot(&dir, 1);
        assert_eq!(s.store, "error", "{name}");
        assert!(s.store_error.is_some() && s.apps.is_empty());
    }
}

// ---- held Git events: recorded, decided, and still there after a restart

mod held {
    use super::*;
    use harpoon_deploy_cli::{run_with, Gate, Services};

    const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// Queue an event the way a poll or a webhook does, without Git or a network: `git-trigger`.
    fn queue(dir: &Path, commit: &str, delivery: &str) {
        let args: Vec<String> = [
            "git-trigger",
            "--repo",
            "shop",
            "--revision",
            commit,
            "--delivery",
            delivery,
            "--source",
            "poll",
            "--runtime-dir",
        ]
        .iter()
        .map(|s| s.to_string())
        .chain([dir.display().to_string()])
        .collect();
        let kc = MemorySecretStore::new();
        let end = std::time::Instant::now() + LOCK_WAIT;
        let o = loop {
            let mut input: &[u8] = b"";
            let mut svc = Services {
                secrets: &kc,
                stdin: &mut input,
                stdin_is_terminal: false,
                next: None,
            };
            let o = run_with(&args, &Gate::shipping(), &mut svc);
            if o.code != 0 && lock_refused(&o.stderr) && std::time::Instant::now() < end {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            break o;
        };
        assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    }

    fn registered(name: &str) -> (PathBuf, MemorySecretStore) {
        let dir = rt(name);
        let kc = MemorySecretStore::new();
        let add = DeployOp::AddRepo {
            name: "shop".into(),
            url: "https://example.com/acme/shop.git".into(),
            app: None,
            project: None,
            environment: None,
            branch: Some("main".into()),
            filters: vec![],
            container_port: 3000,
            dockerfile: None,
            context: None,
            target: None,
            health_path: None,
            username: None,
            known_host: None,
            webhook: None,
        };
        assert!(run_op(&dir, &add, &kc).ok);
        (dir, kc)
    }

    fn events(dir: &Path) -> Vec<(String, String, String)> {
        // a fresh read of the journal is exactly what a restarted desktop does
        snapshot(dir, 9).apps[0]
            .events
            .iter()
            .map(|e| (e.revision.clone(), e.status.clone(), e.source.clone()))
            .collect()
    }

    fn app_id(dir: &Path) -> String {
        snapshot(dir, 1).apps[0].id.clone()
    }

    #[test]
    fn a_queued_commit_is_a_held_event_with_everything_the_review_dialog_shows() {
        let (dir, _) = registered("held-shows");
        queue(&dir, COMMIT_A, "poll:shop:a");
        let s = snapshot(&dir, 5);
        let e = &s.apps[0].events[0];
        assert_eq!(
            (e.revision.as_str(), e.status.as_str(), e.source.as_str()),
            (COMMIT_A, "held", "poll")
        );
        assert!(e.received_ms > 0 && e.decided_ms.is_none());
        assert!(s.apps[0].deployments.is_empty(), "queuing a commit deploys nothing");
    }

    #[test]
    fn dismissing_is_recorded_and_survives_a_restart() {
        let (dir, kc) = registered("held-dismiss");
        queue(&dir, COMMIT_A, "poll:shop:a");
        queue(&dir, COMMIT_B, "poll:shop:b");
        let app = app_id(&dir);
        let r = run_op(
            &dir,
            &DeployOp::Resume {
                app,
                mode: "dismiss".into(),
                revision: None,
            },
            &kc,
        );
        assert!(r.ok, "{}", r.detail);
        let after = events(&dir);
        assert_eq!(
            after.iter().map(|e| e.1.as_str()).collect::<Vec<_>>(),
            ["dismissed", "dismissed"]
        );
        // "restart": nothing is kept in memory, so a second independent read is the restarted desktop's view
        assert_eq!(events(&dir), after);
        let s = snapshot(&dir, 10);
        assert!(s.apps[0].deployments.is_empty() && s.apps[0].pauses.is_empty());
        // a dismissed commit is not queued again
        queue(&dir, COMMIT_A, "poll:shop:a");
        assert_eq!(events(&dir).len(), 2, "the same delivery is a duplicate");
    }

    #[test]
    fn selecting_a_revision_is_recorded_deploys_nothing_and_survives_a_restart() {
        let (dir, kc) = registered("held-select");
        queue(&dir, COMMIT_A, "poll:shop:a");
        queue(&dir, COMMIT_B, "poll:shop:b");
        let app = app_id(&dir);
        let r = run_op(
            &dir,
            &DeployOp::Resume {
                app,
                mode: "revision".into(),
                revision: Some(COMMIT_A.into()),
            },
            &kc,
        );
        assert!(r.ok, "{}", r.detail);
        let after = events(&dir);
        let status = |c: &str| after.iter().find(|e| e.0 == c).unwrap().1.clone();
        assert_eq!(status(COMMIT_A), "selected");
        assert_ne!(status(COMMIT_B), "selected");
        assert_eq!(events(&dir), after, "unchanged on a second, independent read");
        assert!(
            snapshot(&dir, 11).apps[0].deployments.is_empty(),
            "choosing a revision deploys nothing"
        );
    }

    #[test]
    fn a_decision_for_a_revision_that_was_never_held_is_refused_and_changes_nothing() {
        let (dir, kc) = registered("held-unknown");
        queue(&dir, COMMIT_A, "poll:shop:a");
        let before = events(&dir);
        let r = run_op(
            &dir,
            &DeployOp::Resume {
                app: app_id(&dir),
                mode: "revision".into(),
                revision: Some(COMMIT_B.into()),
            },
            &kc,
        );
        assert!(!r.ok, "{}", r.detail);
        assert_eq!(events(&dir), before);
    }
}

#[test]
fn the_folder_pickers_answer_is_one_clean_absolute_path_or_nothing() {
    assert_eq!(
        parse_picked_folder("/Users/juan/Harpoon UI Test/\n").as_deref(),
        Some("/Users/juan/Harpoon UI Test")
    );
    assert_eq!(parse_picked_folder("/\n").as_deref(), Some("/"));
    for bad in ["", "\n", "relative/path\n", "/a/b\nc\n", "/a/\u{7}b\n"] {
        assert_eq!(parse_picked_folder(bad), None, "{bad:?}");
    }
}

// ---- external runtimes: read-only, no migration, no way into an operation

mod external {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const DEP: &str = "dep_01M6DPQ543ASWBHW7PW17ZAJ3C";
    const APP: &str = "app_01M6DPQ543ASWBHW7PW17ZAJ3B";

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/b7-legacy-runtime")
    }

    /// A private copy of the B7 fixture to attach (the committed fixture itself is never attached).
    fn b7_copy(name: &str) -> PathBuf {
        let dir = rt(name).join("runtime"); // `rt` clears and creates the test's own directory
        std::fs::create_dir_all(dir.join("deploy-journal")).unwrap();
        std::fs::copy(
            fixture().join("deploy-journal/journal.wal"),
            dir.join("deploy-journal/journal.wal"),
        )
        .unwrap();
        std::fs::copy(fixture().join("runtime-kind"), dir.join("runtime-kind")).unwrap();
        dir
    }

    /// Every path under `dir` with its kind, size, modification time and bytes: equal before and after means nothing was written.
    fn tree(dir: &Path) -> Vec<(String, bool, u64, std::time::SystemTime, Vec<u8>)> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, bool, u64, std::time::SystemTime, Vec<u8>)>) {
            let mut es: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
            es.sort_by_key(|e| e.file_name());
            for e in es {
                let m = std::fs::symlink_metadata(e.path()).unwrap();
                let rel = e.path().strip_prefix(base).unwrap().display().to_string();
                if m.is_dir() {
                    out.push((rel, true, 0, m.modified().unwrap(), vec![]));
                    walk(base, &e.path(), out);
                } else {
                    out.push((
                        rel,
                        false,
                        m.len(),
                        m.modified().unwrap(),
                        std::fs::read(e.path()).unwrap(),
                    ));
                }
            }
        }
        let mut out = vec![];
        walk(dir, dir, &mut out);
        out
    }

    /// Harpoon's own (stand-in) store for a test: a sibling of the attached runtime, never created by the helper.
    fn own(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("harpoon-deploy-api-{}-{name}", std::process::id()))
            .join("own")
    }

    fn attach(dir: &Path, own: &Path) -> Result<ExternalRuntime, String> {
        ExternalRuntime::attach(dir.to_str().unwrap(), own)
    }

    #[test]
    fn the_b7_legacy_journal_is_shown_as_it_was_recorded() {
        let dir = b7_copy("ext-shows");
        let s = attach(&dir, &own("ext-shows")).unwrap().snapshot(5);
        assert_eq!(
            (s.source.kind, s.source.journal_format, s.source.read_only),
            ("external", "legacy", true)
        );
        assert_eq!(
            (s.store.as_str(), s.store_error.as_deref(), s.apps.len()),
            ("ok", None, 1)
        );
        let a = &s.apps[0];
        assert_eq!(a.id, APP);
        assert_eq!(a.deployments.len(), 1);
        let d = &a.deployments[0];
        assert_eq!(
            (d.id.as_str(), d.status.as_str(), d.cutover.as_str()),
            (DEP, "succeeded", "not_required") // the first deployment: nothing to cut over from
        );
        let steps: Vec<_> = d.steps.iter().map(|x| (x.name.as_str(), x.status.as_str())).collect();
        assert_eq!(
            steps,
            [
                ("source", "succeeded"),
                ("build", "succeeded"),
                ("preliminary_check", "succeeded"),
                ("cutover", "succeeded"),
                ("final_check", "succeeded"),
                ("recovery", "skipped")
            ]
        );
        // what the ledger recorded: the lease for the serving container's port is the only place the port survives
        let lease = |state: &str| {
            d.resources
                .iter()
                .filter(|r| r.kind == "port" && r.state == state)
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(lease("created"), ["38000"], "{:?}", d.resources);
        assert_eq!(lease("released"), ["39000"], "the preliminary-check port was released");
        let c = d
            .resources
            .iter()
            .find(|r| r.kind == "container" && r.state == "created")
            .expect("serving container");
        assert_eq!(c.name, format!("hp-{DEP}"));
        assert_eq!(
            c.id.as_deref(),
            Some("2f70855fd334312f11ae3b2c5cb2ef42af49203820869637c71c02ead50ed936")
        );
        let sv = a.serving.as_ref().expect("serving release derived from the ledger");
        assert_eq!(
            (sv.deployment.as_str(), sv.container.as_str(), sv.via.as_str()),
            (DEP, format!("hp-{DEP}").as_str(), "original")
        );
        // a pre-configuration journal pins nothing: no port, no health path, no secrets - and says so
        assert!(sv.host_port.is_none() && d.pin.is_none());
        assert!(s.repos.is_empty() && s.definitions.is_empty());
    }

    /// The exact JSON the desktop receives for the B7 journal. The frontend tests load this same file, so what they
    /// assume about the contract is what the backend produces. `UPDATE_GOLDEN=1 cargo test` rewrites it.
    #[test]
    fn the_snapshot_json_for_b7_is_the_checked_in_contract() {
        let dir = b7_copy("ext-golden");
        let mut v =
            serde_json::to_value(attach(&dir, &own("ext-golden")).unwrap().snapshot(1_793_609_000_000)).unwrap();
        v["runtimeDir"] = "<runtime>".into();
        v["source"]["path"] = "<runtime>".into();
        let text = serde_json::to_string_pretty(&v).unwrap() + "\n";
        let golden = fixture().parent().unwrap().join("b7-legacy-snapshot.json");
        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&golden, &text).unwrap();
        }
        assert_eq!(
            text,
            std::fs::read_to_string(&golden).expect("golden file"),
            "the snapshot contract changed; rerun with UPDATE_GOLDEN=1 if intended"
        );
    }

    #[test]
    fn reading_it_writes_nothing_and_migrates_nothing() {
        let dir = b7_copy("ext-nowrite");
        let before = tree(&dir);
        let ext = attach(&dir, &own("ext-nowrite")).unwrap();
        for i in 0..3 {
            let s = ext.snapshot(10 + i);
            assert_eq!(s.store, "ok");
        }
        assert_eq!(tree(&dir), before, "no file, directory, lock or timestamp changed");
        assert!(
            !dir.join("deploy-journal")
                .join(harpoon_deploy_journal::SQLITE_FILE)
                .exists(),
            "a legacy journal is read where it is, not converted"
        );
    }

    #[test]
    fn it_can_be_read_from_a_folder_nobody_can_write_to() {
        let dir = b7_copy("ext-readonly-fs");
        let jd = dir.join("deploy-journal");
        std::fs::set_permissions(jd.join("journal.wal"), std::fs::Permissions::from_mode(0o400)).unwrap();
        for d in [&jd, &dir] {
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        let before = tree(&dir);
        let s = attach(&dir, &own("ext-readonly-fs")).unwrap().snapshot(5);
        assert_eq!((s.store.as_str(), s.apps.len()), ("ok", 1), "{:?}", s.store_error);
        assert_eq!(tree(&dir), before);
        for d in [&dir, &jd] {
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn a_sqlite_runtime_is_read_without_creating_anything_either() {
        let dir = rt("ext-sqlite").join("runtime");
        prepare(&dir).unwrap();
        {
            let (mut j, _) = open_journal(&dir);
            j.persist(&dep(D1, Some(DeploymentStatus::Succeeded))).unwrap();
        }
        let before = tree(&dir);
        let s = attach(&dir, &own("ext-sqlite")).unwrap().snapshot(5);
        assert_eq!(
            (s.source.journal_format, s.store.as_str(), s.apps[0].deployments.len()),
            ("sqlite", "ok", 1)
        );
        assert_eq!(tree(&dir), before, "no -journal, -wal, -shm or lock file appeared");
    }

    #[test]
    fn only_a_real_runtime_folder_can_be_attached() {
        let o = own("ext-validate");
        let good = b7_copy("ext-validate");
        assert!(attach(&good, &o).is_ok());
        let err = |p: &str| {
            ExternalRuntime::attach(p, &o)
                .err()
                .unwrap_or_else(|| format!("ACCEPTED {p:?}"))
        };
        assert!(err("relative/runtime").contains("absolute"));
        assert!(err(&format!("{}/../runtime", good.display())).contains("absolute path without"));
        assert!(err("/nonexistent-harpoon-runtime").contains("Cannot read"));
        assert!(err("/tmp/hv\u{7}x").contains("control character"));
        assert!(err(good.join("deploy-journal/journal.wal").to_str().unwrap()).contains("not a folder"));
        assert!(err(good.join("deploy-journal").to_str().unwrap()).contains("not deploy-journal itself"));
        let empty = rt("ext-validate-empty");
        assert!(err(empty.to_str().unwrap()).contains("No deploy-journal"));
        std::fs::create_dir_all(empty.join("deploy-journal")).unwrap();
        assert!(err(empty.to_str().unwrap()).contains("No deployment journal"));
        // symbolic links are refused, not followed into
        let linked = rt("ext-validate-link");
        std::os::unix::fs::symlink(good.join("deploy-journal"), linked.join("deploy-journal")).unwrap();
        assert!(err(linked.to_str().unwrap()).contains("symbolic link"));
        let linked2 = rt("ext-validate-link2");
        std::fs::create_dir_all(linked2.join("deploy-journal")).unwrap();
        std::os::unix::fs::symlink(
            good.join("deploy-journal/journal.wal"),
            linked2.join("deploy-journal/journal.wal"),
        )
        .unwrap();
        assert!(err(linked2.to_str().unwrap()).contains("not a regular file"));
        // Harpoon's own store is not an "external" one
        prepare(&o).unwrap();
        assert!(err(o.to_str().unwrap()).contains("own deployment store"));
    }

    #[test]
    fn nothing_can_aim_an_operation_at_an_attached_runtime() {
        let o = own("ext-guard");
        let dir = b7_copy("ext-guard");
        let _attached = attach(&dir, &o).unwrap();
        let before = tree(&dir);
        let kc = MemorySecretStore::new();
        let ops = [
            DeployOp::AddRepo {
                name: "shop".into(),
                url: "https://example.com/acme/shop.git".into(),
                app: None,
                project: None,
                environment: None,
                branch: None,
                filters: vec![],
                container_port: 3000,
                dockerfile: None,
                context: None,
                target: None,
                health_path: None,
                username: None,
                known_host: None,
                webhook: None,
            },
            DeployOp::Resume {
                app: APP.into(),
                mode: "dismiss".into(),
                revision: None,
            },
            DeployOp::SetConfig {
                scope: "global".into(),
                name: "A".into(),
                value: "1".into(),
                build: false,
                runtime: true,
            },
            DeployOp::Recover {},
        ];
        // the runtime itself, the journal folder inside it, and a path that merely climbs back into it
        for target in [
            dir.clone(),
            dir.join("deploy-journal"),
            dir.join("deploy-journal").join(".."),
        ] {
            for op in &ops {
                let r = run_op(&target, op, &kc);
                assert!(
                    !r.ok
                        && r.exit_code == harpoon_deploy_cli::exit::REFUSED
                        && r.summary.contains("attached read-only"),
                    "{target:?}: {}",
                    r.detail
                );
            }
        }
        assert_eq!(tree(&dir), before, "refused before anything was created or opened");
        // Harpoon's own store is unaffected by the guard
        let own_ok = run_op(
            &o,
            &DeployOp::SetConfig {
                scope: "global".into(),
                name: "A".into(),
                value: "1".into(),
                build: false,
                runtime: true,
            },
            &kc,
        );
        assert!(own_ok.ok, "{}", own_ok.detail);
    }

    #[test]
    fn an_operation_cannot_carry_a_runtime_path_at_all() {
        let ok: Result<DeployOp, _> = serde_json::from_str(r#"{"op":"recover"}"#);
        assert!(ok.is_ok());
        for smuggled in [
            r#"{"op":"recover","runtimeDir":"/tmp/x"}"#,
            r#"{"op":"recover","runtime":"/tmp/x"}"#,
            r#"{"op":"resume","app":"a","mode":"dismiss","revision":null,"runtimeDir":"/tmp/x"}"#,
            r#"{"op":"set-config","scope":"global","name":"A","value":"1","build":false,"runtime":true,"path":"/tmp/x"}"#,
        ] {
            let e = serde_json::from_str::<DeployOp>(smuggled)
                .err()
                .expect(smuggled)
                .to_string();
            assert!(e.contains("unknown field"), "{smuggled}: {e}");
        }
    }
}

// ---- host listeners: what is actually bound on this Mac, kept apart from what Docker reports
// (no process is started here: tests run beside tests that hold journals open, and a child forked meanwhile keeps a copy of a
// held lock until it execs; the lsof runner and its parser are tested in deploy-docker)

mod listeners {
    use super::*;
    use harpoon_deploy_docker::forwarder::parse_lsof_f;

    #[test]
    fn listeners_are_reported_with_their_real_binding_and_scope() {
        let out = "p67874\ncharpoon\nn127.0.0.1:38000\np1\nclaunchd\nn*:5000\nn[::1]:631\np9\ncnode\nn192.168.1.5:3000\nn[::]:8080\n";
        let r = host_listeners_from(parse_lsof_f(out), 7);
        assert!(r.verified && r.error.is_none());
        let by = |port: u16| {
            r.listeners
                .iter()
                .filter(|l| l.port == port)
                .map(|l| (l.address.as_str(), l.scope, l.pid, l.command.as_str()))
                .collect::<Vec<_>>()
        };
        assert_eq!(by(38000), [("127.0.0.1:38000", "loopback", 67874, "harpoon")]);
        assert_eq!(by(5000), [("*:5000", "wildcard", 1, "launchd")]);
        assert_eq!(by(631), [("[::1]:631", "loopback", 1, "launchd")]);
        assert_eq!(by(3000), [("192.168.1.5:3000", "interface", 9, "node")]);
        assert_eq!(by(8080), [("[::]:8080", "wildcard", 9, "node")]);
    }

    #[test]
    fn a_check_that_cannot_run_is_unverified_and_claims_nothing() {
        // (what lsof gave, whether the answer can be trusted)
        let cases = [
            (
                "lsof is not there",
                Err("cannot run /nonexistent/lsof: No such file or directory".to_string()),
                false,
            ),
            ("its output cannot be read", parse_lsof_f("n127.0.0.1:1\n"), false),
            // lsof exits 1 and prints nothing when no TCP port is listening: a real answer, "none"
            ("nothing is listening", parse_lsof_f(""), true),
        ];
        for (what, found, trusted) in cases {
            let r = host_listeners_from(found, 7);
            assert_eq!(r.verified, trusted, "{what}: {r:?}");
            assert_eq!(r.error.is_some(), !trusted, "{what}");
            assert!(r.listeners.is_empty(), "{what}");
        }
        let r = host_listeners_from(
            Err("lsof did not finish in 5 s; the host listeners are unverified".into()),
            7,
        );
        assert!(!r.verified && r.error.as_deref().unwrap().contains("did not finish"));
    }

    #[test]
    fn scope_is_classified_from_the_bound_address_alone() {
        for (host, scope) in [
            ("127.0.0.1", "loopback"),
            ("127.5.5.5", "loopback"),
            ("[::1]", "loopback"),
            ("localhost", "loopback"),
            ("*", "wildcard"),
            ("0.0.0.0", "wildcard"),
            ("[::]", "wildcard"),
            ("10.0.0.7", "interface"),
            ("[fe80::1]", "interface"),
        ] {
            assert_eq!(listener_scope(host), scope, "{host}");
        }
    }
}
