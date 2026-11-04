//! Real SQLite fixtures: what the desktop shows is exactly what the engine recorded.

use super::*;
use harpoon_deploy::engine::Journal;
use harpoon_deploy::ids::ReleaseId;
use harpoon_deploy::secrets::MemorySecretStore;
use harpoon_deploy::state::{DeploymentStatus, Trigger};

const APP: &str = "app_00000000000000000000000001";
const D1: &str = "dep_00000000000000000000000001";
const D2: &str = "dep_00000000000000000000000002";

/// Every gate closed: tests that prove "refused at the gate" use this, whatever `Gate::shipping()` opens.
const CLOSED: Gate = Gate {
    live_deploy_enabled: false,
    live_replacement_enabled: false,
    live_triggered_enabled: false,
};

/// `run_op`, but with a closed gate (the test seam `run_op` uses the shipping gate).
fn run_op_closed(rt: &Path, op: &DeployOp, keychain: &dyn SecretStore) -> OpResult {
    run_op_with(rt, op, &Confirmation::default(), keychain, &CLOSED, &|| {
        Err("no runtime in this test".into())
    })
}

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
        s.gate.live_deploy && s.gate.live_replacement && !s.gate.live_triggered,
        "the shipping gate opens attended deployment and replacement and keeps trigger-driven execution closed"
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
        let r = run_op_closed(&dir, &op, &kc);
        assert!(!r.ok && r.refused_by_gate, "{}", r.detail);
        assert!(!r.summary.is_empty() && r.hint.is_some());
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
    let d = run_op_closed(
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

    pub(super) fn info() -> RuntimeInfo {
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
        live_triggered_enabled: false,
    };
    const AUTH_ARG: &str =
        "--authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE";

    fn args_of(op: &DeployOp, gate: &Gate, r: Result<RuntimeInfo, String>) -> Result<Vec<String>, OpResult> {
        args_with(op, &Confirmation::default(), gate, r)
    }
    fn args_with(op: &DeployOp, c: &Confirmation, gate: &Gate, r: Result<RuntimeInfo, String>) -> Result<Vec<String>, OpResult> {
        build_args(op, c, gate, &|| r.clone()).map(|(a, _)| a)
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
            let r = run_op_with(&dir, &op, &Confirmation::default(), &kc, &CLOSED, &|| {
                found.clone()
            });
            assert!(!r.ok && r.refused_by_gate, "{}", r.detail);
        }
        assert!(snapshot(&dir, 1).apps.is_empty(), "nothing was recorded");
    }

    pub(super) fn deploy_op_for(repo: &str) -> DeployOp {
        DeployOp::Deploy {
            repo: repo.into(),
            branch: None,
            tag: None,
            commit: None,
        }
    }
    fn has_auth(a: &[String]) -> bool {
        a.iter()
            .any(|x| x.contains("authorize-live") || x.contains("I-UNDERSTAND"))
    }
    pub(super) const CONFIRMED: Confirmation = Confirmation {
        live_changes: true,
        cutover: false,
    };

    #[test]
    fn the_authorization_is_added_only_for_a_confirmed_docker_changing_operation() {
        for op in [
            deploy_op_for("shop"),
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
                // not confirmed (Cancel never reaches here at all; an absent confirmation is the same as none)
                assert!(!has_auth(&args_of(&op, &gate, Ok(info())).unwrap()));
                assert!(!has_auth(
                    &args_with(
                        &op,
                        &Confirmation {
                            live_changes: false,
                            cutover: false
                        },
                        &gate,
                        Ok(info())
                    )
                    .unwrap()
                ));
                // confirmed: exactly one authorization argument, exactly the engine's phrase, and no cutover switch
                let a = args_with(&op, &CONFIRMED, &gate, Ok(info())).unwrap();
                assert_eq!(
                    a.iter().filter(|x| x.as_str() == AUTH_ARG).count(),
                    1,
                    "{a:?}"
                );
                assert!(!a.contains(&"--confirm-cutover".to_string()), "{a:?}");
            }
        }
    }

    #[test]
    fn a_confirmation_is_not_sent_without_a_runtime_and_the_refusal_says_what_to_do() {
        let r = args_with(
            &deploy_op_for("shop"),
            &CONFIRMED,
            &OPEN,
            Err("process 4242 is not running".into()),
        )
        .unwrap_err();
        assert!(!r.ok && r.exit_code == harpoon_deploy_cli::exit::REFUSED);
        let hint = r.hint.expect("an actionable hint");
        assert!(
            hint.contains("Start Harpoon") && hint.contains("Nothing was changed"),
            "{hint}"
        );
        // while the gate is closed the same missing runtime is not the reason: the engine will say the gate is
        let ok = args_with(
            &deploy_op_for("shop"),
            &CONFIRMED,
            &CLOSED,
            Err("no runtime".into()),
        )
        .unwrap();
        assert!(!has_auth(&ok));
    }

    #[test]
    fn operations_that_change_no_docker_state_get_no_authorization_even_when_confirmed() {
        for op in [
            DeployOp::Poll { repo: None },
            DeployOp::Recover {},
            DeployOp::Resume {
                app: "app_x".into(),
                mode: "dismiss".into(),
                revision: None,
            },
            DeployOp::Unset {
                scope: "global".into(),
                name: "X".into(),
            },
        ] {
            let a = args_with(&op, &CONFIRMED, &Gate::shipping(), Ok(info())).unwrap();
            assert!(
                !has_auth(&a) && pair(&a, "--docker-socket").is_none(),
                "{a:?}"
            );
        }
        // checking for new commits queues events and can never deploy
        let a = args_with(
            &DeployOp::Poll { repo: None },
            &CONFIRMED,
            &Gate::shipping(),
            Ok(info()),
        )
        .unwrap();
        assert_eq!(a, ["watch", "--once", "--queue-only"]);
    }

    #[test]
    fn the_cutover_confirmation_is_separate_deploy_only_and_never_without_the_authorization() {
        let both = Confirmation {
            live_changes: true,
            cutover: true,
        };
        let a = args_with(&deploy_op_for("shop"), &both, &Gate::shipping(), Ok(info())).unwrap();
        assert!(
            a.contains(&"--confirm-cutover".to_string()) && a.contains(&AUTH_ARG.to_string()),
            "{a:?}"
        );
        // confirming the deployment alone never switches an existing release
        let a = args_with(
            &deploy_op_for("shop"),
            &CONFIRMED,
            &Gate::shipping(),
            Ok(info()),
        )
        .unwrap();
        assert!(!a.contains(&"--confirm-cutover".to_string()), "{a:?}");
        // a cutover with no authorization, or on anything but a deployment, is refused here, before the engine
        let only_cutover = Confirmation {
            live_changes: false,
            cutover: true,
        };
        for (op, c) in [
            (deploy_op_for("shop"), only_cutover),
            (
                DeployOp::Rollback {
                    app: "app_x".into(),
                    release: "rel_x".into(),
                },
                both,
            ),
            (DeployOp::Poll { repo: None }, both),
        ] {
            let r = args_with(&op, &c, &Gate::shipping(), Ok(info())).unwrap_err();
            assert!(!r.ok && r.hint.is_some(), "{}", r.detail);
        }
    }

    #[test]
    fn a_confirmation_is_strict_and_an_operation_payload_cannot_carry_one() {
        assert_eq!(
            serde_json::from_str::<Confirmation>("{}").unwrap(),
            Confirmation::default()
        );
        assert_eq!(
            serde_json::from_str::<Confirmation>(r#"{"liveChanges":true,"cutover":true}"#).unwrap(),
            Confirmation {
                live_changes: true,
                cutover: true
            }
        );
        assert!(
            serde_json::from_str::<Confirmation>(r#"{"liveChanges":true,"authorize":true}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<DeployOp>(
            r#"{"op":"deploy","repo":"shop","liveChanges":true}"#
        )
        .is_err());
        assert!(serde_json::from_str::<DeployOp>(
            r#"{"op":"deploy","repo":"shop","authorize":true}"#
        )
        .is_err());
        assert!(
            serde_json::from_str::<DeployOp>(r#"{"op":"poll","confirmCutover":true}"#).is_err()
        );
    }

    #[test]
    fn every_failure_has_an_actionable_hint_in_plain_words_and_success_has_none() {
        use harpoon_deploy_cli::exit;
        let op = deploy_op_for("shop");
        assert!(actionable(&op, exit::OK, "ok", "").is_none());
        let cases: [(i32, &str, &str, &str); 9] = [
            (exit::REFUSED, "", "this changes Docker state and needs the explicit opt-in --authorize-live-docker-changes=…", "confirmation"),
            (exit::REFUSED, "", "executing queued Git events is not enabled in this build (unattended execution, Gate U, has not passed).", "queued Git events"),
            (exit::REFUSED, "", "live replacement is not enabled in this build", "does not allow"),
            (exit::REFUSED, "", "another Harpoon process holds the deployment journal", "Wait for it"),
            (exit::USAGE, "", "usage error: bad flag", "internal error"),
            (exit::CONFIG, "", "--env NAME=VALUE: bad name", "configuration was rejected"),
            (exit::JOURNAL, "", "the journal is not usable: checksum mismatch", "Do not delete"),
            (exit::RUNTIME, "", "cannot reach /tmp/harpoon-docker.sock", "Docker as ready"),
            (exit::FAILED, "failure at preliminary_check: timed out after 60 s\n", "", "preliminary_check"),
        ];
        for (code, out, err, want) in cases {
            let h = actionable(&op, code, out, err).unwrap_or_else(|| panic!("no hint for {code}"));
            assert!(h.contains(want), "{code}: {h}");
            assert!(
                !h.to_lowercase().contains("exit code") && !h.contains(&format!("({code})")),
                "{h}"
            );
        }
    }

    #[test]
    fn the_snapshot_reports_attended_operations_open_and_trigger_driven_execution_closed() {
        let g = snapshot(&rt("snap-gate"), 1).gate;
        assert!(g.live_deploy && g.live_replacement && !g.live_triggered);
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
    use super::runtime::{deploy_op_for, info, CONFIRMED};
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

    #[test]
    fn an_unconfirmed_live_deploy_is_refused_before_the_repository_is_contacted_with_an_actionable_hint(
    ) {
        let (dir, kc) = registered("unconfirmed-deploy");
        let before = snapshot(&dir, 1);
        let r = run_op_with(
            &dir,
            &deploy_op_for("shop"),
            &Confirmation::default(),
            &kc,
            &Gate::shipping(),
            &|| Ok(info()),
        );
        assert!(
            !r.ok && r.exit_code == harpoon_deploy_cli::exit::REFUSED,
            "{}",
            r.detail
        );
        assert!(
            !r.refused_by_gate,
            "the gate is open; the missing confirmation is the reason"
        );
        let hint = r.hint.expect("hint");
        assert!(
            hint.contains("confirmation") && !hint.to_lowercase().contains("exit code"),
            "{hint}"
        );
        assert!(r.detail.contains("explicit opt-in"), "{}", r.detail);
        let after = snapshot(&dir, 1);
        assert_eq!(after.apps.len(), before.apps.len());
        assert!(
            after.apps[0].deployments.is_empty(),
            "no deployment was recorded"
        );
    }

    #[test]
    fn queued_event_execution_stays_closed_in_the_shipping_gate_even_when_confirmed() {
        let (dir, kc) = registered("run-queue-closed");
        let r = run_op_with(
            &dir,
            &DeployOp::RunQueue { repo: None },
            &CONFIRMED,
            &kc,
            &Gate::shipping(),
            &|| Ok(info()),
        );
        assert!(!r.ok && r.refused_by_gate, "{}", r.detail);
        assert!(r.detail.contains("Gate U"), "{}", r.detail);
        let hint = r.hint.expect("hint");
        assert!(hint.contains("Use Deploy"), "{hint}");
        assert!(snapshot(&dir, 1).apps[0].deployments.is_empty());
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

    // ---- remembering attachments across restarts: references only, restored read-only ------------------------------------

    /// The desktop's own configuration directory for a test: a sibling of the runtime, never inside it.
    fn app_data(runtime: &Path) -> PathBuf {
        runtime.parent().unwrap().join("appdata")
    }

    fn list_file(data: &Path) -> PathBuf {
        data.join("external-runtimes.json")
    }

    #[test]
    fn an_attachment_is_remembered_as_a_path_and_nothing_from_inside_the_runtime() {
        let dir = b7_copy("ext-persist-write");
        let (data, o) = (app_data(&dir), own("ext-persist-write"));
        let before = tree(&dir);
        let entry = ExternalStore::new(&data).add(dir.to_str().unwrap(), &o, 5).unwrap();
        assert!(entry.error.is_none() && entry.snapshot.is_some());
        assert_eq!(entry.path, canonical(&dir).display().to_string());
        let file = list_file(&data);
        let text = std::fs::read_to_string(&file).unwrap();
        // exactly a version and the folder; no deployment, container, image or journal content
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap(),
            serde_json::json!({ "version": 1, "runtimes": [{ "path": entry.path }] })
        );
        for secret in [DEP, APP, "journal.wal", "2f70855fd334", "hp-dep"] {
            assert!(!text.contains(secret), "{secret} must not be stored");
        }
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600,
            "private to the user"
        );
        // kept in the desktop's configuration directory: not inside the runtime, not in the deployment store
        assert!(!canonical(&file).starts_with(canonical(&dir)));
        assert!(!file.starts_with(own("ext-persist-write")));
        assert_eq!(
            std::fs::read_dir(&data).unwrap().count(),
            1,
            "no temporary file is left behind"
        );
        assert_eq!(tree(&dir), before, "adding it wrote nothing to the runtime");
    }

    #[test]
    fn after_a_restart_the_attachment_is_back_read_only_without_being_opened_again() {
        let dir = b7_copy("ext-persist-restart");
        let (data, o) = (app_data(&dir), own("ext-persist-restart"));
        ExternalStore::new(&data).add(dir.to_str().unwrap(), &o, 5).unwrap();
        let before = tree(&dir);
        // a restart keeps nothing in memory: a new store reads the list from disk
        let restored = ExternalStore::new(&data).restore(&o, 9).unwrap();
        assert_eq!(restored.len(), 1);
        let s = restored[0].snapshot.as_ref().expect("read again");
        assert_eq!(
            (
                s.source.kind,
                s.source.journal_format,
                s.source.read_only,
                s.store.as_str()
            ),
            ("external", "legacy", true, "ok")
        );
        assert_eq!(s.apps[0].deployments[0].id, DEP);
        assert!(restored[0].error.is_none());
        // restoring, however often, writes nothing and migrates nothing
        for i in 0..3 {
            ExternalStore::new(&data).restore(&o, 10 + i).unwrap();
        }
        assert_eq!(tree(&dir), before, "no file, lock or timestamp in the runtime changed");
        assert!(
            !dir.join("deploy-journal")
                .join(harpoon_deploy_journal::SQLITE_FILE)
                .exists(),
            "the legacy journal was not converted"
        );
        assert_eq!(std::fs::read_dir(&data).unwrap().count(), 1);
    }

    #[test]
    fn a_missing_or_unreadable_runtime_stays_listed_as_unavailable_and_returns_when_it_does() {
        let dir = b7_copy("ext-persist-missing");
        let (data, o) = (app_data(&dir), own("ext-persist-missing"));
        let store = ExternalStore::new(&data);
        store.add(dir.to_str().unwrap(), &o, 5).unwrap();
        let list = std::fs::read(list_file(&data)).unwrap();
        let away = dir.with_file_name("runtime-away");
        std::fs::rename(&dir, &away).unwrap();
        let gone = store.restore(&o, 6).unwrap();
        assert_eq!(gone.len(), 1, "the reference is kept");
        assert!(
            gone[0].snapshot.is_none() && gone[0].error.as_deref().unwrap().contains("Cannot read"),
            "{:?}",
            gone[0].error
        );
        assert_eq!(
            std::fs::read(list_file(&data)).unwrap(),
            list,
            "an unavailable runtime does not change the list"
        );
        // the folder comes back: it is read again with no further action
        std::fs::rename(&away, &dir).unwrap();
        assert!(store.restore(&o, 7).unwrap()[0].snapshot.is_some());
        // a folder with no journal in it is unavailable with the reason, and still listed
        std::fs::remove_file(dir.join("deploy-journal/journal.wal")).unwrap();
        let bare = store.restore(&o, 8).unwrap();
        assert!(bare[0].snapshot.is_none() && bare[0].error.as_deref().unwrap().contains("No deployment journal"));
        // a journal that cannot be decoded is an attached runtime whose store is shown as unreadable, not dropped
        std::fs::write(dir.join("deploy-journal/journal.wal"), b"this is not a journal").unwrap();
        let bad = store.restore(&o, 9).unwrap();
        let snap = bad[0].snapshot.as_ref().expect("still attached");
        assert_eq!(snap.store, "error");
        assert!(snap.store_error.is_some() && snap.apps.is_empty());
        assert_eq!(
            std::fs::read(list_file(&data)).unwrap(),
            list,
            "the reference survives all of it"
        );
        assert_eq!(
            std::fs::read(dir.join("deploy-journal/journal.wal")).unwrap(),
            b"this is not a journal",
            "an unreadable journal is not repaired"
        );
    }

    #[test]
    fn detaching_forgets_only_the_reference() {
        let a = b7_copy("ext-persist-detach");
        let (data, o) = (app_data(&a), own("ext-persist-detach"));
        let b = a.with_file_name("runtime-b");
        std::fs::create_dir_all(b.join("deploy-journal")).unwrap();
        std::fs::copy(
            a.join("deploy-journal/journal.wal"),
            b.join("deploy-journal/journal.wal"),
        )
        .unwrap();
        let store = ExternalStore::new(&data);
        let (ea, eb) = (
            store.add(a.to_str().unwrap(), &o, 5).unwrap(),
            store.add(b.to_str().unwrap(), &o, 5).unwrap(),
        );
        let (tree_a, tree_b) = (tree(&a), tree(&b));
        assert!(store.remove(&ea.path).unwrap());
        let left: Vec<String> = ExternalStore::new(&data)
            .restore(&o, 6)
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(
            left,
            std::slice::from_ref(&eb.path),
            "only the other reference is left, after a restart too"
        );
        assert_eq!(
            (tree(&a), tree(&b)),
            (tree_a, tree_b),
            "no file in either runtime was touched"
        );
        assert!(
            a.join("deploy-journal/journal.wal").exists(),
            "the detached runtime's files are all still there"
        );
        // detaching something that is not attached changes nothing
        let list = std::fs::read(list_file(&data)).unwrap();
        assert!(!store.remove(&ea.path).unwrap());
        assert!(!store.remove("/nonexistent/runtime").unwrap());
        assert_eq!(std::fs::read(list_file(&data)).unwrap(), list);
        // a runtime that is gone can still be detached
        let away = b.with_file_name("runtime-b-away");
        std::fs::rename(&b, &away).unwrap();
        assert!(store.remove(&eb.path).unwrap());
        assert!(ExternalStore::new(&data).restore(&o, 7).unwrap().is_empty());
    }

    #[test]
    fn a_damaged_list_is_reported_and_never_overwritten() {
        let dir = b7_copy("ext-persist-damaged");
        let (data, o) = (app_data(&dir), own("ext-persist-damaged"));
        std::fs::create_dir_all(&data).unwrap();
        let store = ExternalStore::new(&data);
        for bad in [
            "{ not json".to_string(),
            r#"{"version":2,"runtimes":[]}"#.to_string(),
            r#"{"version":1,"runtimes":[{"path":"relative/runtime"}]}"#.to_string(),
            r#"{"version":1,"runtimes":[{"path":"/x","extra":1}]}"#.to_string(),
            format!(
                r#"{{"version":1,"runtimes":[{}]}}"#,
                vec![r#"{"path":"/x"}"#; 33].join(",")
            ),
        ] {
            std::fs::write(list_file(&data), &bad).unwrap();
            assert!(store.restore(&o, 5).unwrap_err().contains("left as it is"), "{bad}");
            assert!(store.add(dir.to_str().unwrap(), &o, 5).is_err());
            assert!(store.remove("/x").is_err());
            assert_eq!(
                std::fs::read_to_string(list_file(&data)).unwrap(),
                bad,
                "the damaged list is exactly as it was"
            );
        }
    }

    #[test]
    fn duplicates_collapse_and_the_list_is_bounded() {
        let dir = b7_copy("ext-persist-bounds");
        let (data, o) = (app_data(&dir), own("ext-persist-bounds"));
        let store = ExternalStore::new(&data);
        let link = dir.with_file_name("alias");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let first = store.add(dir.to_str().unwrap(), &o, 5).unwrap();
        let again = store.add(link.to_str().unwrap(), &o, 5).unwrap();
        assert_eq!(
            first.path, again.path,
            "an alias of the same folder is the same reference"
        );
        assert_eq!(store.restore(&o, 6).unwrap().len(), 1);
        // a full list refuses one more, and writes nothing
        let full: Vec<_> = (0..32)
            .map(|i| format!(r#"{{"path":"/nonexistent/runtime-{i}"}}"#))
            .collect();
        std::fs::write(
            list_file(&data),
            format!(r#"{{"version":1,"runtimes":[{}]}}"#, full.join(",")),
        )
        .unwrap();
        let list = std::fs::read(list_file(&data)).unwrap();
        assert!(store
            .add(dir.to_str().unwrap(), &o, 5)
            .unwrap_err()
            .contains("At most 32"));
        assert_eq!(std::fs::read(list_file(&data)).unwrap(), list);
    }

    #[test]
    fn a_path_that_is_not_a_runtime_is_refused_and_not_remembered() {
        let dir = b7_copy("ext-persist-refuse");
        let (data, o) = (app_data(&dir), own("ext-persist-refuse"));
        let store = ExternalStore::new(&data);
        for bad in ["relative/path", "/nonexistent/runtime"] {
            assert!(store.add(bad, &o, 5).is_err(), "{bad}");
        }
        assert!(store.add(dir.join("deploy-journal").to_str().unwrap(), &o, 5).is_err());
        assert!(!list_file(&data).exists(), "nothing was written for a refused path");
        assert!(!data.exists(), "not even the configuration directory");
    }

    #[test]
    fn a_remembered_runtime_is_guarded_against_mutation_even_while_it_is_unavailable() {
        let dir = b7_copy("ext-persist-guard");
        let (data, o) = (app_data(&dir), own("ext-persist-guard"));
        let later = dir.with_file_name("not-yet-there");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            list_file(&data),
            format!(
                r#"{{"version":1,"runtimes":[{{"path":{:?}}}]}}"#,
                later.display().to_string()
            ),
        )
        .unwrap();
        // restored after a "restart": unavailable, but already guarded
        let e = ExternalStore::new(&data).restore(&o, 5).unwrap();
        assert!(e[0].snapshot.is_none());
        assert!(is_attached(&later), "guarded before the folder exists");
        // the folder appears (a runtime that was not mounted yet): an operation aimed at it is refused before anything is created
        std::fs::create_dir_all(later.join("deploy-journal")).unwrap();
        std::fs::copy(
            dir.join("deploy-journal/journal.wal"),
            later.join("deploy-journal/journal.wal"),
        )
        .unwrap();
        let before = tree(&later);
        let kc = MemorySecretStore::new();
        for target in [later.clone(), later.join("deploy-journal")] {
            let r = run_op(
                &target,
                &DeployOp::Resume {
                    app: APP.into(),
                    mode: "dismiss".into(),
                    revision: None,
                },
                &kc,
            );
            assert!(
                !r.ok && r.exit_code == harpoon_deploy_cli::exit::REFUSED && r.summary.contains("attached read-only"),
                "{target:?}: {}",
                r.detail
            );
        }
        assert_eq!(tree(&later), before);
        // after detaching, the guard no longer claims it
        assert!(ExternalStore::new(&data).remove(later.to_str().unwrap()).unwrap());
        assert!(!is_attached(&later));
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

#[test]
fn the_hostname_and_route_come_from_the_records_and_survive_a_restart_without_writing() {
    let dir = rt("hostname");
    seed(&dir, &[dep(D1, Some(DeploymentStatus::Succeeded)), dep(D2, Some(DeploymentStatus::Failed))]);
    let jd = journal_dir(&dir);
    let sizes = || {
        let mut v: Vec<_> = std::fs::read_dir(&jd).unwrap().flatten().map(|e| (e.file_name(), e.metadata().unwrap().len())).collect();
        v.sort();
        v
    };
    let before = sizes();
    let app: ApplicationId = APP.parse().unwrap();
    let host = harpoon_deploy::route::hostname(&app);
    // "Restart": a fresh read of the same journal, twice. Same name, same table, nothing written.
    for _ in 0..2 {
        let s = snapshot_when_released(&dir, 5);
        assert_eq!(s.apps[0].hostname, host);
        assert!(host.ends_with(".harpoon.localhost") && host.len() < 40);
        // Succeeded but no container in the ledger: the application is registered, not serving.
        assert_eq!(s.apps[0].route, "idle");
        let t = route_table(&dir).unwrap();
        assert_eq!(t.resolve(&host), Some(&harpoon_deploy::route::Route::Idle { app: app.clone() }));
    }
    assert_eq!(sizes(), before, "reading the route table wrote nothing");
    // No store yet: an empty table, not an error; a damaged store is an error so the router keeps its table.
    assert!(route_table(&rt("hostname-none")).unwrap().resolve(&host).is_none());
    let bad = rt("hostname-bad");
    let bj = journal_dir(&bad);
    std::fs::create_dir_all(&bj).unwrap();
    std::fs::write(bj.join(harpoon_deploy_journal::SQLITE_FILE), b"not a database").unwrap();
    assert!(route_table(&bad).is_err());
}
