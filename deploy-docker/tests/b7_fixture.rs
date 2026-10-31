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
    let mut cmd = Command::new("/bin/sh");
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
