//! The disposable HTTP fixture for the first live deployment, tested without Docker: its Dockerfile
//! against Harpoon's own preflight, and its responder script under /bin/sh behind a loopback
//! listener, probed with Harpoon's own HTTP client.

use harpoon_deploy::health::Observation;
use harpoon_deploy_docker::confine::confine;
use harpoon_deploy_docker::dockerfile::required_images;
use harpoon_deploy_docker::probe::{HttpTransport, ResponseMarker, TcpHttp};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/http-marker")
}

const MARKER: &str = "3fa91c07d2b84e56a0c1f7e9b2d4a688";

/// Serve connections by running srv.sh for each, as `nc -lk -e` would.
fn serve(marker_env: Option<&'static str>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let script = fixture().join("srv.sh");
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let script = script.clone();
            std::thread::spawn(move || run_script(c, &script, marker_env));
        }
    });
    port
}

fn run_script(mut c: TcpStream, script: &PathBuf, marker_env: Option<&str>) {
    let mut cmd =
        Command::new(std::env::var("HARPOON_TEST_SHELL").unwrap_or_else(|_| "/bin/sh".into()));
    cmd.arg(script)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(m) = marker_env {
        cmd.env("HARPOON_DEPLOYMENT_MARKER", m);
    }
    let mut child = cmd.spawn().unwrap();
    // request in
    let mut req = Vec::new();
    let mut buf = [0u8; 1024];
    let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
        match c.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => req.extend_from_slice(&buf[..n]),
        }
    }
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(&req);
    drop(stdin);
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    let _ = child.wait();
    let _ = c.write_all(&out);
}

fn header_marker(v: &str) -> ResponseMarker {
    ResponseMarker::header("X-Harpoon-Deployment-Marker", v).unwrap()
}

#[test]
fn the_dockerfile_passes_harpoons_own_preflight_and_declares_nothing_risky() {
    let src = confine(&fixture(), &fixture(), "Dockerfile").unwrap();
    let images = required_images(&src.dockerfile_text).unwrap();
    assert_eq!(
        images.into_iter().collect::<Vec<_>>(),
        ["alpine:3.22"],
        "one literal local base image"
    );
    let text = src.dockerfile_text.to_ascii_lowercase();
    for forbidden in [
        "volume",
        "add ",
        "user ",
        "apk ",
        "pip ",
        "curl",
        "wget",
        "npm ",
        "git ",
        "--mount",
        "--from",
        "healthcheck",
        "privileged",
        "restart",
        "arg ",
        "env ",
    ] {
        let code: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains(forbidden),
            "the fixture must not use {forbidden:?}"
        );
    }
    assert!(src.ignore.is_none());
}

#[test]
fn health_returns_the_deployment_marker_in_the_header_and_the_body() {
    let port = serve(Some(MARKER));
    let t = Duration::from_secs(3);
    assert_eq!(
        TcpHttp.get_expecting(port, "/health", t, Some(&header_marker(MARKER))),
        Observation::Status(200)
    );
    let body = ResponseMarker::body_contains(MARKER).unwrap();
    assert_eq!(
        TcpHttp.get_expecting(port, "/health", t, Some(&body)),
        Observation::Status(200)
    );
    // query strings do not change the answer; a different marker is not accepted
    assert_eq!(
        TcpHttp.get_expecting(port, "/health?probe=1", t, Some(&header_marker(MARKER))),
        Observation::Status(200)
    );
    assert!(matches!(
        TcpHttp.get_expecting(
            port,
            "/health",
            t,
            Some(&header_marker("0".repeat(32).as_str()))
        ),
        Observation::ConnectFailed(_)
    ));
}

#[test]
fn other_paths_are_404_and_never_carry_the_marker() {
    let port = serve(Some(MARKER));
    let t = Duration::from_secs(3);
    for path in ["/", "/healthz", "/health/x", "/admin"] {
        assert!(
            matches!(
                TcpHttp.get_expecting(port, path, t, Some(&header_marker(MARKER))),
                Observation::ConnectFailed(_)
            ),
            "{path}"
        );
        assert_eq!(
            TcpHttp.get(port, path, t),
            Observation::Status(404),
            "{path}"
        );
    }
}

#[test]
fn without_a_valid_marker_in_the_environment_the_fixture_is_never_healthy() {
    for (what, env) in [
        ("unset", None),
        ("empty", Some("")),
        ("not hex", Some("MARKER; rm -rf")),
    ] {
        let port = serve(env);
        let t = Duration::from_secs(3);
        assert_eq!(
            TcpHttp.get(port, "/health", t),
            Observation::Status(503),
            "{what}"
        );
        assert!(
            matches!(
                TcpHttp.get_expecting(port, "/health", t, Some(&header_marker(MARKER))),
                Observation::ConnectFailed(_)
            ),
            "{what}"
        );
    }
}

#[test]
fn the_script_cannot_be_made_to_echo_request_data() {
    let port = serve(Some(MARKER));
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(b"GET /health HTTP/1.1\r\nX-Evil: \"; echo pwned; $(id)\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut out = String::new();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
    assert!(!out.contains("pwned") && !out.contains("uid="), "{out}");
    // an odd request line is answered, not executed
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(b"GET /health;id HTTP/1.1\r\n\r\n").unwrap();
    let mut out = String::new();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 404"), "{out}");
    assert!(!out.contains("uid="));
}

#[test]
fn the_fixture_directory_holds_only_what_the_build_needs() {
    let mut names: Vec<_> = std::fs::read_dir(fixture())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["Dockerfile", "README.md", "srv.sh"]);
}

// ------------------------------------------------------------------ the whole contract, without a Docker build

mod support;
use support::fake_docker::St;
use support::*;

/// What the two fixture listeners (38000 application, 39000 test) consult for each connection.
struct Route {
    st: Option<std::sync::Arc<std::sync::Mutex<St>>>,
    /// When set, the test port is answered by a fixture started with THIS marker (a stale instance).
    stale_on_test_port: Option<&'static str>,
}

static ROUTE: std::sync::Mutex<Route> = std::sync::Mutex::new(Route {
    st: None,
    stale_on_test_port: None,
});
/// The backend keeps its logical 38000/39000 port contract; this test routes those ports to
/// independently allocated loopback listeners to avoid collisions with other CI processes.
#[derive(Clone, Copy)]
struct FixturePorts {
    app: u16,
    pre: u16,
}

impl FixturePorts {
    fn physical(self, logical: u16) -> Option<u16> {
        match logical {
            38000 => Some(self.app),
            39000 => Some(self.pre),
            _ => None,
        }
    }
}

/// A real TCP transport with test-only port translation. Evidence stays non-simulated.
struct RoutedHttp(FixturePorts);

impl HttpTransport for RoutedHttp {
    fn get(&self, port: u16, path: &str, timeout: Duration) -> Observation {
        match self.0.physical(port) {
            Some(actual) => TcpHttp.get(actual, path, timeout),
            None => Observation::ConnectFailed(format!("unexpected logical fixture port {port}")),
        }
    }

    fn get_expecting(
        &self,
        port: u16,
        path: &str,
        timeout: Duration,
        marker: Option<&ResponseMarker>,
    ) -> Observation {
        match self.0.physical(port) {
            Some(actual) => TcpHttp.get_expecting(actual, path, timeout, marker),
            None => Observation::ConnectFailed(format!("unexpected logical fixture port {port}")),
        }
    }
}

/// Serve the real fixture script on two dynamically allocated loopback TCP ports.
/// The fake Docker still publishes logical 38000 (application) and 39000 (preliminary).
/// Each accepted connection uses the marker recorded for the container on that logical port.
fn fixture_listeners() -> FixturePorts {
    let app = TcpListener::bind(("127.0.0.1", 0))
        .expect("bind fixture application listener on an available loopback port");
    let pre = TcpListener::bind(("127.0.0.1", 0))
        .expect("bind fixture preliminary listener on an available loopback port");
    let ports = FixturePorts {
        app: app.local_addr().unwrap().port(),
        pre: pre.local_addr().unwrap().port(),
    };
    for (logical_port, listener) in [(38000u16, app), (39000u16, pre)] {
        let script = fixture().join("srv.sh");
        std::thread::spawn(move || {
            for c in listener.incoming().flatten() {
                let script = script.clone();
                std::thread::spawn(move || {
                    let (st, stale) = {
                        let r = ROUTE.lock().unwrap();
                        (r.st.clone(), r.stale_on_test_port)
                    };
                    let marker: Option<String> = match (logical_port, stale) {
                        (39000, Some(m)) => Some(m.to_string()),
                        _ => st.and_then(|st| {
                            let s = st.lock().unwrap();
                            s.containers.iter().find_map(|k| {
                                (k.host_port == Some(logical_port))
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
                        }),
                    };
                    let leaked: Option<&'static str> =
                        marker.map(|m| &*Box::leak(m.into_boxed_str()));
                    run_script(c, &script, leaked);
                });
            }
        });
    }
    ports
}

fn deploy_against_the_fixture(
    h: &mut Harness,
    deadline_ms: u64,
    ports: FixturePorts,
) -> harpoon_deploy::engine::Report {
    use harpoon_deploy_docker::backend::{Attribution, DockerBackend};
    let mut cfg = config(&h.ws, &h.docker);
    cfg.attribution = Attribution::PerContainer;
    // the fixture answers /health only; a deployment of it must be told so (`--health-path /health`)
    cfg.health.path = "/health".into();
    cfg.health.startup_deadline_ms = deadline_ms;
    let mut backend =
        DockerBackend::with_transport(cfg, Some(&h.ws.resources()), RoutedHttp(ports))
            .unwrap()
            .with_entropy(test_entropy())
            .with_port_probe(NoListeners)
            .with_disk_probe(FakeDisk(10 << 30));
    let mut j = StampedJournal::new();
    h.run(&mut backend, &mut j)
}

#[test]
fn the_real_fixture_script_is_healthy_only_with_its_own_containers_marker_over_real_http() {
    use harpoon_deploy::state::DeploymentStatus;
    let ports = fixture_listeners();
    assert_ne!(
        ports.app, ports.pre,
        "the two test listeners must be distinct"
    );
    // 1. a stale instance (some other marker) answers on the test port: the deployment must not pass
    let mut h = Harness::new();
    h.seed_built_image();
    {
        let mut r = ROUTE.lock().unwrap();
        r.st = Some(h.docker.st.clone());
        r.stale_on_test_port = Some("deadbeefdeadbeefdeadbeefdeadbeef");
    }
    let r = deploy_against_the_fixture(&mut h, 600, ports);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h.dep.failure.clone().unwrap().message.contains("marker"));
    assert!(
        h.objects_for_dep().is_empty(),
        "the failed attempt left no container"
    );

    // 2. every container is answered by the fixture started with that container's own marker
    let mut h = Harness::new();
    h.seed_built_image();
    {
        let mut r = ROUTE.lock().unwrap();
        r.st = Some(h.docker.st.clone());
        r.stale_on_test_port = None;
    }
    let r = deploy_against_the_fixture(&mut h, 3000, ports);
    // (message kept short on purpose: the evidence of a long failing probe is hundreds of attempts)
    let why = h
        .dep
        .failure
        .as_ref()
        .map(|f| f.message.chars().take(500).collect::<String>());
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{why:?}");
    assert_eq!(r.evidence.len(), 2);
    assert!(
        r.evidence.iter().all(|(_, e)| !e.simulated),
        "the evidence is from real HTTP, not a simulation"
    );
}
