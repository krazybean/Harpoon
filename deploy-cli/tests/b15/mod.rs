//! B15: the live `replace`, `rollback` and `restore` paths through the real CLI against the fake
//! Docker. The library is driven in process with the acceptance gate; the shipping and acceptance
//! binaries are spawned as real processes where the boundary itself is the point. Nothing here
//! contacts a live Docker endpoint, and there is no fault injection: failures are the ordinary ones
//! (an application that does not answer), and the only real sockets are loopback responders on the
//! logical ports the engine probes.

use super::*;
use harpoon_deploy::control::{self, ControlState, Incoming};
use harpoon_deploy::ids::ReleaseId;
use harpoon_deploy::release::ReleaseConfig;
use harpoon_deploy::state::Step;
use std::io::{Read, Write};
use std::net::TcpListener;

mod packaging;

const APP: &str = "app_00000000000000000000000001";
const PREV: &str = "dep_00000000000000000000000010";
const DEAD: &str = "dep_00000000000000000000000011";
const BYSTANDER_APP: &str = "app_00000000000000000000000002";
const BYSTANDER_DEP: &str = "dep_00000000000000000000000030";
const PORT: u16 = 38500;
const TEST_PORT: u16 = 39000;

fn auth() -> String {
    format!("--{AUTH_FLAG}={AUTH_PHRASE}")
}

fn live_flags(t: &T) -> Vec<String> {
    vec![
        "--docker-socket".into(),
        t.socket(),
        "--resources-dir".into(),
        t.resources(),
        auth(),
        "--forwarder-pid".into(),
        "4242".into(),
        "--mgmt-socket".into(),
        t.mgmt.path().display().to_string(),
    ]
}

fn go(t: &T, gate: &Gate, mut a: Vec<String>) -> Outcome {
    a.extend(live_flags(t));
    t.go(gate, &a.iter().map(String::as_str).collect::<Vec<_>>())
}

fn live(t: &T, lsof: &str, mut a: Vec<String>) -> Outcome {
    a.extend([
        "--forwarder-command".into(),
        "harpoon".into(),
        "--lsof-path".into(),
        lsof.into(),
    ]);
    go(t, &ACCEPTANCE, a)
}

/// A stand-in for `lsof`: the forwarder (pid 4242, `harpoon`) is the one loopback listener asked about.
fn forwarder_lsof(t: &T) -> String {
    let p = t.h.ws.root.join("b15-lsof");
    std::fs::write(
        &p,
        "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in -iTCP:*) port=\"${a#-iTCP:}\";; esac; done\nprintf 'p4242\\ncharpoon\\nn127.0.0.1:%s\\n' \"$port\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.display().to_string()
}

/// A pin's environment, in the canonical (sorted) order the codec requires.
fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    v.sort();
    v
}

/// The request body of every container creation so far (the environment is in it).
fn create_bodies(t: &T) -> Vec<String> {
    t.h.docker
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST" && r.path.contains("/containers/create"))
        .map(|r| r.body)
        .collect()
}

fn markers_in(body: &str) -> Vec<String> {
    body.split("HARPOON_DEPLOYMENT_MARKER=")
        .skip(1)
        .map(|r| r.split('"').next().unwrap().to_string())
        .collect()
}

/// Answers for the new release on whatever application port the engine leased for it: binds the
/// port of the first running container in the application range that is not in `skip`.
fn new_release_responder(t: &T, skip: Vec<u16>) -> Serving {
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (flag, docker) = (stop.clone(), t.h.docker.st.clone());
    std::thread::spawn(move || {
        let port = loop {
            if flag.load(Ordering::SeqCst) {
                return;
            }
            let found = docker.lock().unwrap().containers.iter().find_map(|k| {
                k.host_port
                    .filter(|p| k.running && (38000..39000).contains(p) && !skip.contains(p))
            });
            if let Some(p) = found {
                break p;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        let l = TcpListener::bind(("127.0.0.1", port)).expect("bind the new release's port");
        l.set_nonblocking(true).unwrap();
        while !flag.load(Ordering::SeqCst) {
            let Ok((mut c, _)) = l.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            };
            c.set_nonblocking(false).unwrap();
            let mut buf = [0u8; 1024];
            let _ = c.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let _ = c.read(&mut buf);
            let m = docker.lock().unwrap().containers.iter().find_map(|k| {
                (k.running && k.host_port == Some(port))
                    .then(|| {
                        k.create_body
                            .get("Env")
                            .and_then(harpoon_deploy_docker::json::Json::as_array)
                            .and_then(|a| {
                                a.iter().filter_map(|e| e.as_str()).find_map(|e| {
                                    e.strip_prefix("HARPOON_DEPLOYMENT_MARKER=")
                                        .map(str::to_string)
                                })
                            })
                    })
                    .flatten()
            });
            let reply = match m {
                Some(m) => format!(
                    "HTTP/1.1 200 OK\r\nX-Harpoon-Deployment-Marker: {m}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{m}\n",
                    m.len() + 1
                ),
                None => "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            };
            let _ = c.write_all(reply.as_bytes());
        }
    });
    Serving(stop)
}

/// What a real deployment leaves: a container, its image, a pinned configuration (with `env`), the
/// lease, in a docker-kind runtime and in the fake Docker.
#[allow(clippy::too_many_arguments)]
fn seed(
    t: &T,
    app: &str,
    dep: &str,
    at: u64,
    port: u16,
    running: bool,
    n: u64,
    env: &[(&str, &str)],
) -> (String, String) {
    t.docker_runtime();
    let (app, dep): (ApplicationId, DeploymentId) = (app.parse().unwrap(), dep.parse().unwrap());
    let image = support::fake_iid(n);
    let name = format!("hp-{dep}");
    let lbl = harpoon_deploy::backend::harpoon_labels(&app, &dep);
    let cid = t.h.docker.with(|s| {
        s.add_image_with_id(&image, &[&format!("harpoon/{dep}:1")], lbl.clone(), vec![]);
        let c = s.add_container(&name, lbl.clone(), running);
        s.containers.last_mut().unwrap().host_port = Some(port);
        c
    });
    let mut pin = ReleaseConfig::simulated(port);
    pin.image_id = image.clone();
    pin.image_tag = format!("harpoon/{dep}:1");
    pin.health.interval_ms = 5;
    pin.health.startup_deadline_ms = 300;
    pin.env = vars(env);
    let mut d = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, at);
    d.transition(DeploymentStatus::Queued, at + 1).unwrap();
    d.transition(DeploymentStatus::Running, at + 2).unwrap();
    d.release = Some(ReleaseId::from_deployment(&dep));
    d.pinned = Some(pin);
    d.transition(DeploymentStatus::Succeeded, at + 3).unwrap();
    let (tag, lease) = (format!("harpoon/{}:1", d.id), port.to_string());
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&d).unwrap();
    for (kind, n, id, disp) in [
        (
            ResourceKind::Image,
            tag.as_str(),
            image.as_str(),
            Disposition::Retain,
        ),
        (
            ResourceKind::Container,
            name.as_str(),
            cid.as_str(),
            Disposition::RemoveOnFailure,
        ),
        (
            ResourceKind::PortLease,
            lease.as_str(),
            lease.as_str(),
            Disposition::RemoveOnFailure,
        ),
    ] {
        j.record(&intent(&app, &dep, kind, n, disp)).unwrap();
        j.record(&receipt(&dep, kind, n, id)).unwrap();
    }
    (image, cid)
}

fn seed_release(t: &T, running: bool, env: &[(&str, &str)]) -> (String, String) {
    seed(t, APP, PREV, 100, PORT, running, 9001, env)
}

/// A running container of another application, which no command here may ever change.
fn bystander(t: &T) -> String {
    let (bapp, bdep): (ApplicationId, DeploymentId) = (
        BYSTANDER_APP.parse().unwrap(),
        BYSTANDER_DEP.parse().unwrap(),
    );
    t.h.docker.with(|s| {
        let c = s.add_container(
            &format!("hp-{BYSTANDER_DEP}"),
            harpoon_deploy::backend::harpoon_labels(&bapp, &bdep),
            true,
        );
        s.containers.last_mut().unwrap().host_port = Some(38600);
        c
    })
}

fn untouched(t: &T, cid: &str) {
    for r in t.h.docker.requests() {
        assert!(
            r.method == "GET" || !r.path.contains(cid),
            "a container that must not change was touched: {} {}",
            r.method,
            r.path
        );
    }
}

fn dead_cutover(t: &T) {
    let (app, dep): (ApplicationId, DeploymentId) = (APP.parse().unwrap(), DEAD.parse().unwrap());
    let mut d = Deployment::new(dep, app, Trigger::Cli, 200);
    d.transition(DeploymentStatus::Queued, 201).unwrap();
    d.transition(DeploymentStatus::Running, 202).unwrap();
    d.start_step(Step::Cutover, 203).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&PREV.parse().unwrap()));
    d.begin_cutover(204).unwrap();
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&d).unwrap();
}

/// (name, running, host port, image, Env entries) of every container in the fake Docker.
type Seen = (String, bool, Option<u16>, String, Vec<String>);

fn containers(t: &T) -> Vec<Seen> {
    t.h.docker.with(|s| {
        s.containers
            .iter()
            .map(|c| {
                let env = c
                    .create_body
                    .get("Env")
                    .and_then(harpoon_deploy_docker::json::Json::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| e.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                (c.name.clone(), c.running, c.host_port, c.image.clone(), env)
            })
            .collect()
    })
}

fn container(t: &T, name: &str) -> Seen {
    containers(t)
        .into_iter()
        .find(|c| c.0 == name)
        .unwrap_or_else(|| panic!("no container {name}"))
}

fn without_marker(env: &[String]) -> Vec<String> {
    env.iter()
        .filter(|e| !e.starts_with("HARPOON_DEPLOYMENT_MARKER="))
        .cloned()
        .collect()
}

fn control_of(t: &T) -> ControlState {
    harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir())
        .unwrap()
        .control
}

// ---- loopback responders (the only real sockets)

/// Tests that bind loopback ports (the preliminary range, the application ports) run one at a time.
fn ports() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Serving(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Drop for Serving {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Answers `/health` on `port` with the marker the fake Docker holds for the *running* container
/// that publishes it (what the real application does with its own `HARPOON_DEPLOYMENT_MARKER`), and
/// 503 when none runs. With `lazy` it binds only once such a container exists, so the engine's own
/// free-port check still sees the port free.
fn responder(t: &T, port: u16, lazy: bool) -> Serving {
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (flag, docker) = (stop.clone(), t.h.docker.st.clone());
    let marker_of = move || {
        docker.lock().unwrap().containers.iter().find_map(|k| {
            (k.running && k.host_port == Some(port))
                .then(|| {
                    k.create_body
                        .get("Env")
                        .and_then(harpoon_deploy_docker::json::Json::as_array)
                        .and_then(|a| {
                            a.iter().filter_map(|e| e.as_str()).find_map(|e| {
                                e.strip_prefix("HARPOON_DEPLOYMENT_MARKER=")
                                    .map(str::to_string)
                            })
                        })
                })
                .flatten()
        })
    };
    let ready = std::sync::Arc::new(AtomicBool::new(false));
    let signal = ready.clone();
    std::thread::spawn(move || {
        while lazy && marker_of().is_none() {
            if flag.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let l = TcpListener::bind(("127.0.0.1", port)).expect("bind a loopback responder");
        l.set_nonblocking(true).unwrap();
        signal.store(true, Ordering::SeqCst);
        while !flag.load(Ordering::SeqCst) {
            let Ok((mut c, _)) = l.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(2));
                continue;
            };
            c.set_nonblocking(false).unwrap();
            let mut buf = [0u8; 1024];
            let _ = c.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let _ = c.read(&mut buf);
            let reply = match marker_of() {
                Some(m) => format!(
                    "HTTP/1.1 200 OK\r\nX-Harpoon-Deployment-Marker: {m}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{m}\n",
                    m.len() + 1
                ),
                None => "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            };
            let _ = c.write_all(reply.as_bytes());
        }
    });
    if !lazy {
        while !ready.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    Serving(stop)
}

/// Occupy every preliminary-check port but the first, so the engine's own free-port check can only
/// choose `TEST_PORT`. Dropped as soon as the preliminary check has been made.
fn occupy_all_but_first() -> Vec<TcpListener> {
    let held: Vec<TcpListener> = (TEST_PORT + 1..TEST_PORT + 100)
        .filter_map(|p| TcpListener::bind(("127.0.0.1", p)).ok())
        .collect();
    assert_eq!(
        held.len(),
        99,
        "the preliminary-check port range must be free on this machine"
    );
    held
}

fn replace_args(t: &T, extra: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = ["replace", "--app", APP, "--health-timeout-secs", "2"]
        .map(String::from)
        .to_vec();
    a.extend(extra.iter().map(|s| s.to_string()));
    a.extend(t.src());
    a
}

// ------------------------------------------------------------------ the gate

const LIVE_COMMANDS: [(&str, &[&str]); 3] = [
    (
        "replace",
        &[
            "--app",
            APP,
            "--project-dir",
            "/x",
            "--container-port",
            "8080",
        ],
    ),
    (
        "rollback",
        &["--app", APP, "--to", "rel_00000000000000000000000010"],
    ),
    ("restore", &["--deployment", DEAD, "--apply"]),
];

#[test]
fn the_shipping_gate_refuses_every_live_operation_even_with_every_authorization_in_process() {
    let t = T::new();
    for (cmd, extra) in LIVE_COMMANDS {
        let mut a = vec![cmd.to_string()];
        a.extend(extra.iter().map(|s| s.to_string()));
        for gate in [&SHIPPING, &OPEN] {
            let o = go(&t, gate, a.clone());
            assert_exit(&o, 6);
            assert!(
                o.stderr.contains("is not enabled in this build")
                    && o.stderr.contains("until the live acceptance evidence"),
                "{cmd}: {}",
                o.stderr
            );
            assert!(!t.rt.exists(), "{cmd}: nothing was created");
            assert_eq!(
                t.docker_requests(),
                0,
                "{cmd}: the socket was not contacted"
            );
        }
    }
}

#[test]
fn the_real_shipping_binary_refuses_them_too_and_no_flag_or_environment_variable_opens_it() {
    let t = T::new();
    for (cmd, extra) in LIVE_COMMANDS {
        let mut a: Vec<String> = vec![cmd.to_string()];
        a.extend(extra.iter().map(|s| s.to_string()));
        a.extend(live_flags(&t));
        a.extend(["--runtime-dir".into(), t.rt.display().to_string()]);
        let args: Vec<&str> = a.iter().map(String::as_str).collect();
        let o = spawn_binary(
            &args,
            &[
                ("HARPOON_ACCEPTANCE", "1"),
                ("HARPOON_LIVE", "1"),
                ("DOCKER_HOST", "unix:///var/run/docker.sock"),
            ],
        );
        assert_eq!(
            o.status.code(),
            Some(6),
            "{cmd}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("is not enabled in this build"),
            "{cmd}"
        );
    }
    assert!(!t.rt.exists());
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn unauthorized_or_incomplete_requests_change_nothing_for_all_three_operations() {
    let t = T::new();
    seed_release(&t, true, &[]);
    dead_cutover(&t);
    let lsof = forwarder_lsof(&t);
    let (jb, n) = (t.journal_bytes(), t.docker_requests());
    let full = |cmd: &str| -> Vec<String> {
        let mut a: Vec<String> = match cmd {
            "replace" => replace_args(&t, &["--confirm-cutover"]),
            "rollback" => [
                "rollback",
                "--app",
                APP,
                "--to",
                "rel_00000000000000000000000010",
                "--confirm-rollback",
            ]
            .map(String::from)
            .to_vec(),
            _ => ["restore", "--deployment", DEAD, "--apply"]
                .map(String::from)
                .to_vec(),
        };
        a.extend(live_flags(&t));
        a.extend([
            "--forwarder-command".into(),
            "harpoon".into(),
            "--lsof-path".into(),
            lsof.clone(),
        ]);
        a
    };
    for cmd in ["replace", "rollback", "restore"] {
        // each requirement removed in turn (flag and value), and a wrong phrase
        for drop in [
            format!("--{AUTH_FLAG}={AUTH_PHRASE}"),
            "--docker-socket".into(),
            "--forwarder-pid".into(),
            "--mgmt-socket".into(),
            "--resources-dir".into(),
        ] {
            let mut a = full(cmd);
            let i = a.iter().position(|x| *x == drop).unwrap();
            a.drain(i..=if drop.contains('=') { i } else { i + 1 });
            let o = t.go(
                &ACCEPTANCE,
                &a.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            assert!(
                o.code != 0,
                "{cmd} without {drop} must not run: {}",
                o.stdout
            );
        }
        let mut a = full(cmd);
        let i = a
            .iter()
            .position(|x| x.starts_with(&format!("--{AUTH_FLAG}=")))
            .unwrap();
        a[i] = format!("--{AUTH_FLAG}=yes");
        let o = t.go(
            &ACCEPTANCE,
            &a.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert_exit(&o, 6);
        assert_eq!(
            t.journal_bytes(),
            jb,
            "{cmd}: the journal is byte-identical"
        );
        assert_eq!(t.docker_requests(), n, "{cmd}: Docker was not contacted");
    }
}

#[test]
fn no_experimental_fault_flag_exists_in_the_production_parser() {
    let t = T::new();
    for flag in [
        "--experimental-fail-final-check",
        "--experimental-crash-after-stop",
    ] {
        for gate in [&SHIPPING, &ACCEPTANCE] {
            let mut a = replace_args(&t, &["--confirm-cutover", flag]);
            a.extend(live_flags(&t));
            let o = t.go(gate, &a.iter().map(String::as_str).collect::<Vec<_>>());
            assert_exit(&o, 2);
            assert!(o.stderr.contains(flag), "{}", o.stderr);
        }
    }
    assert!(!harpoon_deploy_cli::HELP.contains("experimental"));
    assert_eq!(t.docker_requests(), 0);
}

// ------------------------------------------------------------------ replace

#[test]
fn an_unconfirmed_live_replacement_stops_nothing_and_pins_the_environment_it_was_given() {
    let _ports = ports();
    let t = T::new();
    let lsof = forwarder_lsof(&t);
    let (_, cid) = seed_release(&t, true, &[("OLD", "1")]);
    let by = bystander(&t);
    let held = occupy_all_but_first();
    let _pre = responder(&t, TEST_PORT, true);
    image_appears_after_build(&t);
    let o = live(
        &t,
        &lsof,
        replace_args(&t, &["--env", "EMPTY=", "--env", "EQ=a=b"]),
    );
    drop(held);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("LIVE REPLACEMENT")
            && o.stdout.contains("— Docker;")
            && !o.stdout.contains("SIMULATED")
            && o.stdout
                .contains("candidate prepared, current release untouched")
            && !o.stdout.contains("(simulated)"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("EMPTY") && o.stdout.contains("EQ"),
        "{}",
        o.stdout
    );
    untouched(&t, &cid);
    untouched(&t, &by);
    assert!(
        container(&t, &format!("hp-{PREV}")).1,
        "the serving release still runs"
    );
    // every container created for the candidate got exactly the variables given now (and its own
    // marker); none got the serving release's
    let bodies = create_bodies(&t);
    assert!(!bodies.is_empty(), "a preliminary container was created");
    for b in &bodies {
        assert!(
            b.contains("EMPTY=") && b.contains("EQ=a=b") && !b.contains("OLD="),
            "{b}"
        );
    }
    assert!(
        control_of(&t).pauses.is_empty(),
        "an unconfirmed run pauses nothing"
    );
}

#[test]
fn a_live_replacement_that_fails_its_preliminary_check_keeps_the_previous_release_and_records_no_pause(
) {
    let t = T::new();
    let lsof = forwarder_lsof(&t);
    let (_, cid) = seed_release(&t, true, &[]);
    image_appears_after_build(&t);
    let o = live(&t, &lsof, replace_args(&t, &["--confirm-cutover"]));
    // nothing answers on the preliminary port, so the check fails before any stop
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("failure at preliminary_check"),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains("--simulate"),
        "a live run is not told to use --simulate:\n{}",
        o.stdout
    );
    untouched(&t, &cid);
    assert!(container(&t, &format!("hp-{PREV}")).1);
    assert!(control_of(&t).pauses.is_empty());
    assert_eq!(
        std::fs::read_to_string(t.rt.join("runtime-kind")).unwrap(),
        "docker\n"
    );
}

#[test]
fn a_healthy_live_replacement_measures_the_interruption_and_gives_each_container_its_own_marker() {
    let _ports = ports();
    let t = T::new();
    let lsof = forwarder_lsof(&t);
    seed_release(&t, true, &[("OLD", "1")]);
    let by = bystander(&t);
    let held = occupy_all_but_first();
    let _pre = responder(&t, TEST_PORT, true);
    let _app = new_release_responder(&t, vec![PORT, 38600]);
    image_appears_after_build(&t);
    let o = live(
        &t,
        &lsof,
        replace_args(&t, &["--confirm-cutover", "--env", "NEW=2"]),
    );
    drop(held);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("measured interruption:")
            && !o.stdout.contains("simulated clock")
            && !o.stdout.contains("SIMULATED"),
        "{}",
        o.stdout
    );
    assert!(
        !container(&t, &format!("hp-{PREV}")).1,
        "the old release was stopped, not removed"
    );
    let all = containers(&t);
    let new: Vec<&Seen> = all
        .iter()
        .filter(|c| c.1 && c.0 != format!("hp-{BYSTANDER_DEP}"))
        .collect();
    assert_eq!(new.len(), 1, "exactly one new release serves: {new:?}");
    assert!(without_marker(&new[0].4).contains(&"NEW=2".to_string()));
    let mut markers: Vec<String> = create_bodies(&t)
        .iter()
        .flat_map(|b| markers_in(b))
        .collect();
    let n = markers.len();
    assert!(n >= 2, "a preliminary and a final container were created");
    markers.sort_unstable();
    markers.dedup();
    assert_eq!(
        markers.len(),
        n,
        "every container has its own health marker"
    );
    untouched(&t, &by);
    assert!(
        control_of(&t).pauses.is_empty(),
        "a successful replacement pauses nothing"
    );
    // the new release's pin carries exactly the variables it was given
    let r = harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir()).unwrap();
    let pinned = r
        .deployments
        .iter()
        .filter(|d| d.status == DeploymentStatus::Succeeded && d.id.to_string() != PREV)
        .filter_map(|d| d.pinned.as_ref())
        .next()
        .expect("the new release is pinned");
    assert_eq!(pinned.env, vars(&[("NEW", "2")]));
}

#[test]
fn a_failed_final_check_gets_one_bounded_restoration_from_the_old_pin_and_a_durable_pause() {
    let _ports = ports();
    let t = T::new();
    let lsof = forwarder_lsof(&t);
    let (prev_image, _) = seed_release(&t, true, &[("OLD", "1")]);
    let by = bystander(&t);
    let held = occupy_all_but_first();
    let _pre = responder(&t, TEST_PORT, true);
    // nothing answers on the candidate's own application port, so its final check fails; the
    // restored release answers on the pinned port
    let _restored = responder(&t, PORT, true);
    image_appears_after_build(&t);
    // a held, selected event must stay exactly as it is
    let pending = hold_and_select(&t);
    let o = live(
        &t,
        &lsof,
        replace_args(&t, &["--confirm-cutover", "--env", "NEW=2"]),
    );
    drop(held);
    assert_exit(&o, 7);
    assert!(o.stdout.contains("recovery: restored"), "{}", o.stdout);
    assert!(!o.stdout.contains("(simulated"), "{}", o.stdout);
    let restored: Vec<Seen> = containers(&t)
        .into_iter()
        .filter(|c| c.0.ends_with("-restored"))
        .collect();
    assert_eq!(restored.len(), 1, "exactly one restoration");
    assert_eq!(restored[0].3, prev_image, "from the exact pinned image id");
    assert_eq!(restored[0].2, Some(PORT));
    let e = without_marker(&restored[0].4);
    assert!(
        e.contains(&"OLD=1".to_string()) && !e.iter().any(|x| x.starts_with("NEW=")),
        "the restored release uses its own pin, not this run's variables: {e:?}"
    );
    untouched(&t, &by);
    let c = control_of(&t);
    // the earlier failure that holds the selection, and this run's own
    assert_eq!(c.pauses.len(), 2, "one durable pause per failed cutover");
    assert!(c
        .pauses
        .iter()
        .all(|p| p.kind == control::PauseKind::FailedCutover));
    assert_eq!(
        c.events, pending.events,
        "the held selection was not executed or changed"
    );
    assert_eq!(c.resumes, pending.resumes);
    let s = t.cli(&["status"]).stdout;
    assert!(s.contains("automatic triggers paused"), "{s}");
}

/// A held event that a developer has selected: pending attended execution. Pauses the application
/// first (a failed cutover record, as a previous failure would leave).
fn hold_and_select(t: &T) -> ControlState {
    let app: ApplicationId = APP.parse().unwrap();
    let dep: DeploymentId = "dep_00000000000000000000000005".parse().unwrap();
    let mut d = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, 50);
    d.transition(DeploymentStatus::Queued, 51).unwrap();
    d.transition(DeploymentStatus::Running, 52).unwrap();
    d.start_step(Step::Cutover, 53).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(&PREV.parse().unwrap()));
    d.begin_cutover(54).unwrap();
    d.fail_cutover(55).unwrap();
    d.start_recovery(60_000, 56).unwrap();
    d.finish_recovery(harpoon_deploy::state::RecoveryOutcome::Restored, 57)
        .unwrap();
    d.transition(DeploymentStatus::Failed, 58).unwrap();
    let (mut j, r) = open_j(&t.journal_dir()).unwrap();
    j.persist(&d).unwrap();
    let mut deps = r.deployments.clone();
    deps.push(d);
    let mut ctl = r.control.clone();
    control::ingest(
        &mut *j,
        &deps,
        &mut ctl,
        Incoming {
            app: &app,
            delivery_id: "d1",
            source: "webhook",
            revision: "aaa",
            received_ms: 300,
        },
    )
    .unwrap();
    control::resume(&mut *j, &deps, &mut ctl, &app, control::Choice::Newest, 400).unwrap();
    ctl
}

// ------------------------------------------------------------------ restore

#[test]
fn explicit_live_restoration_is_dry_run_first_uses_the_pinned_env_once_and_keeps_the_pause() {
    let _ports = ports();
    for (env, label) in [
        (vec![("OLD", "1"), ("EQ", "a=b"), ("EMPTY", "")], "v2"),
        (vec![], "v1"),
    ] {
        let t = T::new();
        let lsof = forwarder_lsof(&t);
        let (image, _) = seed_release(&t, false, &env);
        let by = bystander(&t);
        dead_cutover(&t);
        assert_exit(&t.cli(&["recover"]), 0);
        let (jb, n) = (t.journal_bytes(), t.docker_requests());
        let dry = live(
            &t,
            &lsof,
            vec!["restore".into(), "--deployment".into(), DEAD.into()],
        );
        assert_exit(&dry, 0);
        assert!(
            dry.stdout.contains("dry run: nothing was changed") && dry.stdout.contains(&image),
            "{label}: {}",
            dry.stdout
        );
        assert_eq!(t.journal_bytes(), jb, "{label}: a dry run changes nothing");
        assert_eq!(t.docker_requests(), n);

        let _app = responder(&t, PORT, true);
        let o = live(
            &t,
            &lsof,
            vec![
                "restore".into(),
                "--deployment".into(),
                DEAD.into(),
                "--apply".into(),
            ],
        );
        assert_exit(&o, 0);
        assert!(
            o.stdout.contains("LIVE RESTORATION")
                && o.stdout.contains("recovery: restored")
                && !o.stdout.contains("simulated clock"),
            "{label}: {}",
            o.stdout
        );
        let c = container(&t, &format!("hp-{DEAD}-restored"));
        assert!(c.1);
        assert_eq!((c.2, c.3.as_str()), (Some(PORT), image.as_str()));
        let mut want: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        want.sort();
        let mut got = without_marker(&c.4);
        got.sort();
        assert_eq!(
            got, want,
            "{label}: the restored container carries its own pin"
        );
        untouched(&t, &by);
        let ctl = control_of(&t);
        assert_eq!(ctl.pauses.len(), 1, "{label}");
        assert_eq!(ctl.pauses[0].kind, control::PauseKind::InterruptedCutover);

        // exactly one attempt, ever: refused untouched, with Docker not contacted
        let (jb, n) = (t.journal_bytes(), t.docker_requests());
        let o = live(
            &t,
            &lsof,
            vec![
                "restore".into(),
                "--deployment".into(),
                DEAD.into(),
                "--apply".into(),
            ],
        );
        assert_exit(&o, 6);
        assert!(
            o.stderr
                .contains("already has a recorded restoration attempt"),
            "{}",
            o.stderr
        );
        assert_eq!((t.journal_bytes(), t.docker_requests()), (jb, n));
        let s = t.cli(&["status"]).stdout;
        assert!(
            s.contains(&format!("(restored by {DEAD})")) && !s.contains("--simulate"),
            "{s}"
        );
        // the journal replays after the whole sequence
        drop(open_j(&t.journal_dir()).expect("the journal replays"));
    }
}

#[test]
fn a_restoration_whose_pinned_port_is_held_by_a_foreign_container_leaves_it_alone() {
    let _ports = ports();
    let t = T::new();
    let lsof = forwarder_lsof(&t);
    seed_release(&t, false, &[]);
    dead_cutover(&t);
    assert_exit(&t.cli(&["recover"]), 0);
    // a container with no Harpoon labels, running, on the port the pin needs
    let foreign = t.h.docker.with(|s| {
        let c = s.add_container("someone-elses", vec![], true);
        s.containers.last_mut().unwrap().host_port = Some(PORT);
        c
    });
    let o = live(
        &t,
        &lsof,
        vec![
            "restore".into(),
            "--deployment".into(),
            DEAD.into(),
            "--apply".into(),
        ],
    );
    let _ = o;
    untouched(&t, &foreign);
    assert!(t
        .h
        .docker
        .with(|s| s.containers.iter().any(|c| c.id == foreign && c.running)));
    // the one attempt is spent or the run was refused before spending it; either way it is recorded
    // truthfully and the journal replays
    drop(open_j(&t.journal_dir()).expect("the journal replays"));
}

// ------------------------------------------------------------------ rollback

#[test]
fn a_live_rollback_recreates_the_target_from_its_pin_and_pauses_and_a_failed_one_touches_nothing() {
    let _ports = ports();
    let old = "dep_00000000000000000000000020";
    let new = "dep_00000000000000000000000021";
    let rel = format!("rel_{}", &old[4..]);

    // success: the target answers on its pinned port
    let t = T::new();
    let lsof = forwarder_lsof(&t);
    let (old_image, _) = seed(
        &t,
        APP,
        old,
        100,
        38400,
        false,
        9101,
        &[("OLD", "1"), ("EQ", "a=b")],
    );
    let (_, new_cid) = seed(&t, APP, new, 200, 38401, true, 9102, &[("NEW", "2")]);
    let by = bystander(&t);
    let held = occupy_all_but_first();
    let _pre = responder(&t, TEST_PORT, true);
    let _target = new_release_responder(&t, vec![38401, 38600]);
    let pending = hold_and_select(&t);
    let o = live(
        &t,
        &lsof,
        ["rollback", "--app", APP, "--to", &rel, "--confirm-rollback"]
            .map(String::from)
            .to_vec(),
    );
    drop(held);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("LIVE ROLLBACK") && !o.stdout.contains("SIMULATED"),
        "{}",
        o.stdout
    );
    let rolled: Vec<Seen> = containers(&t)
        .into_iter()
        .filter(|c| c.1 && c.3 == old_image)
        .collect();
    assert_eq!(rolled.len(), 1, "{rolled:?}");
    assert_eq!(
        rolled[0].3, old_image,
        "created from the target's pinned image id"
    );
    let mut e = without_marker(&rolled[0].4);
    e.sort();
    assert_eq!(
        e,
        ["EQ=a=b", "OLD=1"],
        "the target's own pin, not the replaced release's"
    );
    assert!(
        !container(&t, &format!("hp-{new}")).1,
        "the replaced release is stopped"
    );
    assert!(
        t.h.docker
            .requests()
            .iter()
            .all(|r| !r.path.contains("/build")),
        "a rollback never builds"
    );
    untouched(&t, &by);
    let c = control_of(&t);
    assert!(c
        .pauses
        .iter()
        .any(|p| p.kind == control::PauseKind::Rollback));
    assert_eq!(
        c.events, pending.events,
        "a held selection is never executed by a rollback"
    );
    drop(open_j(&t.journal_dir()).expect("the journal replays"));

    // failure: nothing answers, the serving release is never touched
    let t = T::new();
    let lsof = forwarder_lsof(&t);
    let (old_image, _) = seed(&t, APP, old, 100, 38400, false, 9101, &[]);
    let (_, new_cid2) = seed(&t, APP, new, 200, 38401, true, 9102, &[]);
    let _ = new_cid;
    let o = live(
        &t,
        &lsof,
        ["rollback", "--app", APP, "--to", &rel, "--confirm-rollback"]
            .map(String::from)
            .to_vec(),
    );
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("failure at preliminary_check"),
        "{}",
        o.stdout
    );
    assert!(
        t.h.docker
            .requests()
            .iter()
            .any(|r| r.method == "GET" && r.path.contains(&old_image)),
        "the pinned image id was inspected"
    );
    untouched(&t, &new_cid2);
    assert!(container(&t, &format!("hp-{new}")).1);
    assert!(control_of(&t).pauses.is_empty());
    // a target that cannot be proven is refused before Docker is contacted again
    let n = t.docker_requests();
    let o = live(
        &t,
        &lsof,
        [
            "rollback",
            "--app",
            APP,
            "--to",
            "rel_00000000000000000000000077",
        ]
        .map(String::from)
        .to_vec(),
    );
    assert_exit(&o, 6);
    assert_eq!(t.docker_requests(), n);
}

// ------------------------------------------------------------------ concurrency, reporting

#[test]
fn a_writer_holding_the_journal_blocks_every_live_operation_before_docker_is_touched() {
    let t = T::new();
    seed_release(&t, true, &[]);
    dead_cutover(&t);
    assert_exit(&t.cli(&["recover"]), 0);
    let (_w, _) = open_j(&t.journal_dir()).unwrap();
    let n = t.docker_requests();
    for a in [
        replace_args(&t, &["--confirm-cutover"]),
        [
            "rollback",
            "--app",
            APP,
            "--to",
            "rel_00000000000000000000000010",
        ]
        .map(String::from)
        .to_vec(),
        ["restore", "--deployment", DEAD, "--apply"]
            .map(String::from)
            .to_vec(),
    ] {
        let cmd = a[0].clone();
        // no --lsof-path: a thread that holds a journal must not spawn a process (see ForkGate)
        let o = go(&t, &ACCEPTANCE, a);
        assert!(o.code != 0, "{cmd}: {}", o.stdout);
        assert!(
            o.stderr
                .contains("another Harpoon process holds the journal")
                || o.stderr.contains("no running release")
                || o.stderr.contains("refused"),
            "{cmd}: {}",
            o.stderr
        );
    }
    assert_eq!(t.docker_requests(), n, "Docker was never contacted");
}

#[test]
fn simulated_and_live_runs_are_reported_as_what_they_are() {
    let t = T::new();
    let o = t.sim_deploy();
    assert_exit(&o, 0);
    let app = super::b9::app_of(&o);
    let mut a = vec![
        "replace".to_string(),
        "--simulate".into(),
        "--app".into(),
        app,
    ];
    a.extend(t.src());
    let sim = t.cli(&a.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(
        sim.stdout.contains("SIMULATED, no Docker"),
        "{}",
        sim.stdout
    );
    assert!(!sim.stdout.contains("LIVE REPLACEMENT"), "{}", sim.stdout);
    // a live runtime's status never offers --simulate, and names the live command when it is open
    let t2 = T::new();
    seed_release(&t2, false, &[]);
    dead_cutover(&t2);
    assert_exit(&t2.cli(&["recover"]), 0);
    let shipping = t2.go(&SHIPPING, &["status"]).stdout;
    assert!(
        shipping.contains("this build cannot restore a live runtime"),
        "{shipping}"
    );
    let acceptance = t2.go(&ACCEPTANCE, &["status"]).stdout;
    assert!(
        acceptance.contains(&format!("restore --deployment {DEAD}"))
            && !acceptance.contains("--simulate"),
        "{acceptance}"
    );
}

// ------------------------------------------------------------------ P1 cleanup procedure

#[test]
fn an_unconfirmed_live_replacements_retained_image_is_removed_by_id_dry_run_first_and_only_that_image(
) {
    let _ports = ports();
    let t = T::new();
    let lsof = forwarder_lsof(&t);
    let (serving_image, serving_cid) = seed_release(&t, true, &[]);
    let by = bystander(&t);
    let held = occupy_all_but_first();
    let _pre = responder(&t, TEST_PORT, true);
    image_appears_after_build(&t);
    let o = live(&t, &lsof, replace_args(&t, &[]));
    drop(held);
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains("candidate prepared, current release untouched"),
        "{}",
        o.stdout
    );

    // what the journal says about the unconfirmed run: finished, not succeeded, one retained image,
    // everything else released
    let r = harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir()).unwrap();
    let d = r
        .deployments
        .iter()
        .find(|d| d.id.to_string() != PREV)
        .expect("the unconfirmed run is recorded");
    assert!(
        d.status.is_terminal() && d.status != DeploymentStatus::Succeeded,
        "{:?}",
        d.status
    );
    let mine: Vec<_> = r
        .ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == d.id)
        .collect();
    let images: Vec<_> = mine
        .iter()
        .filter(|e| e.key.kind == ResourceKind::Image)
        .collect();
    assert_eq!(images.len(), 1);
    let image = images[0].id.clone().expect("the image has a receipt");
    assert!(
        mine.iter()
            .filter(|e| e.key.kind != ResourceKind::Image)
            .all(|e| e.is_released()),
        "the preliminary container and its lease were released: {mine:?}"
    );
    assert!(image != serving_image);

    let dep = d.id.to_string();
    let cf = |extra: &[&str]| -> Vec<String> {
        let mut a: Vec<String> = ["cleanup-image", "--deployment", &dep, "--image", &image]
            .map(String::from)
            .to_vec();
        a.extend(extra.iter().map(|s| s.to_string()));
        a.extend([
            "--docker-socket".into(),
            t.socket(),
            "--resources-dir".into(),
            t.resources(),
        ]);
        a
    };
    let run = |a: Vec<String>| t.go(&SHIPPING, &a.iter().map(String::as_str).collect::<Vec<_>>());

    // dry run first: reads only, writes nothing, names exactly this id
    let (jb, n) = (t.journal_bytes(), t.docker_requests());
    let o = run(cf(&[]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("dry run: nothing will be changed")
            && o.stdout
                .contains(&format!("remove image {image} by id (force=false"))
            && o.stdout
                .contains("no container, running or stopped, uses it"),
        "{}",
        o.stdout
    );
    assert_eq!(t.journal_bytes(), jb);
    assert!(t.h.docker.requests()[n..].iter().all(|r| r.method == "GET"));
    assert!(t.h.docker.with(|s| s.images.iter().any(|i| i.id == image)));

    // apply: needs the authorization phrase; removes that one image by id, force=false
    let before_apply = t.docker_requests();
    let o = run(cf(&[])
        .into_iter()
        .chain([auth()])
        .chain(["--apply".to_string()])
        .collect());
    assert_exit(&o, 0);
    let deletes: Vec<_> = t.h.docker.requests()[before_apply..]
        .iter()
        .filter(|r| r.method != "GET")
        .cloned()
        .collect();
    assert_eq!(deletes.len(), 1, "only the one image removal: {deletes:?}");
    assert_eq!(deletes[0].method, "DELETE");
    assert!(
        deletes[0].path.contains(&image) && !deletes[0].path.contains("force=true"),
        "{}",
        deletes[0].path
    );
    assert!(t.h.docker.with(|s| !s.images.iter().any(|i| i.id == image)));
    assert!(
        t.h.docker
            .with(|s| s.images.iter().any(|i| i.id == serving_image)),
        "the serving release's image stays"
    );
    untouched(&t, &serving_cid);
    untouched(&t, &by);
    // repeating it is a no-op
    let n = t.docker_requests();
    let o = run(cf(&[])
        .into_iter()
        .chain([auth()])
        .chain(["--apply".to_string()])
        .collect());
    assert_exit(&o, 0);
    assert!(o.stdout.contains("already released"), "{}", o.stdout);
    assert!(t.h.docker.requests()[n..].iter().all(|r| r.method == "GET"));
    drop(open_j(&t.journal_dir()).expect("the journal replays"));
}
