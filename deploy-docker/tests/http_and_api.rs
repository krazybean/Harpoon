//! The wire layer against a fake engine: normal answers, error statuses, malformed and truncated
//! responses, disconnects and timeouts. No real Docker endpoint is ever contacted.

mod support;

use harpoon_deploy_docker::api::*;
use harpoon_deploy_docker::http::{HttpError, UnixHttp};
use harpoon_deploy_docker::json::Json;
use std::time::Duration;
use support::fake_docker::*;

const T: Duration = Duration::from_secs(2);

fn api(d: &FakeDocker) -> DockerApi {
    DockerApi::new(d.path(), Duration::from_secs(2))
}

#[test]
fn ping_inspect_list_and_logs_work_against_recorded_shapes() {
    let d = FakeDocker::start();
    let img = d.with(|s| s.add_image("alpine:3.22", vec![("a".into(), "b".into())]));
    let cid = d.with(|s| {
        let id = s.add_container(
            "web",
            vec![("io.harpoon.managed".into(), "true".into())],
            true,
        );
        s.containers[0].host_port = Some(38001);
        s.containers[0].logs = b"hello\nworld\n".to_vec();
        id
    });
    let a = api(&d);
    a.ping(T).unwrap();
    let i = a.image_inspect("alpine:3.22", T).unwrap().unwrap();
    assert_eq!((i.id.as_str(), i.labels.len()), (img.as_str(), 1));
    assert!(a.image_inspect("nope:1", T).unwrap().is_none());
    let c = a.container_inspect(&cid, T).unwrap().unwrap();
    assert!(c.running && c.name == "web" && c.labels.len() == 1);
    assert!(a.container_inspect("ghost", T).unwrap().is_none());
    let list = a.list_containers(T).unwrap();
    assert_eq!(list[0].host_ports, vec![38001]);
    assert_eq!(
        a.published_ports(T)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![38001]
    );
    let logs = a.logs(&cid, 100, 1 << 20, T).unwrap();
    assert_eq!(logs.text, "hello\nworld\n");
    assert!(!logs.truncated);
}

#[test]
fn the_create_body_has_limits_labels_no_hostip_no_restart_and_no_swap() {
    let d = FakeDocker::start();
    let a = api(&d);
    let labels = vec![("io.harpoon.managed".to_string(), "true".to_string())];
    let spec = CreateSpec {
        image: "harpoon/x:y",
        labels: &labels,
        env: &["A=1".to_string()],
        container_port: 8080,
        host_port: 38001,
        limits: Limits::FIRST_TEST,
    };
    let out = a.create_container("c1", &spec, T).unwrap();
    assert!(matches!(out, CreateOutcome::Created(_)));
    let body = d.with(|s| s.containers[0].create_body.clone());
    let h = body.get("HostConfig").unwrap();
    assert_eq!(
        h.get("Memory").and_then(Json::as_i64),
        Some(256 * 1024 * 1024)
    );
    assert_eq!(
        h.get("MemorySwap").and_then(Json::as_i64),
        Some(256 * 1024 * 1024),
        "no swap"
    );
    assert_eq!(h.get("NanoCpus").and_then(Json::as_i64), Some(500_000_000));
    assert_eq!(h.get("PidsLimit").and_then(Json::as_i64), Some(64));
    assert_eq!(
        h.get("RestartPolicy")
            .and_then(|r| r.get("Name"))
            .and_then(Json::as_str),
        Some("no")
    );
    let raw = body.to_json();
    assert!(!raw.contains("HostIp"), "published without HostIp: {raw}");
    assert!(raw.contains("\"HostPort\":\"38001\""));
    assert_eq!(
        body.get("Labels")
            .and_then(|l| l.get("io.harpoon.managed"))
            .and_then(Json::as_str),
        Some("true")
    );
}

#[test]
fn a_name_conflict_is_reported_not_overridden() {
    let d = FakeDocker::start();
    d.with(|s| s.add_container("taken", vec![], false));
    let a = api(&d);
    let spec = CreateSpec {
        image: "i",
        labels: &[],
        env: &[],
        container_port: 1,
        host_port: 2,
        limits: Limits::FIRST_TEST,
    };
    assert!(
        matches!(a.create_container("taken", &spec, T).unwrap(), CreateOutcome::NameInUse(m) if m.contains("already in use"))
    );
    assert_eq!(d.with(|s| s.containers.len()), 1, "nothing was replaced");
}

#[test]
fn error_statuses_carry_docker_messages() {
    let d = FakeDocker::start();
    d.fault(
        None,
        "/_ping",
        1,
        Action::Respond(500, "{\"message\":\"boom\"}".into()),
    );
    let e = api(&d).ping(T).unwrap_err();
    assert!(
        matches!(&e, ApiError::Status { status: 500, message, .. } if message == "boom"),
        "{e}"
    );
}

#[test]
fn port_collision_is_recognised_from_dockers_message_only() {
    let d = FakeDocker::start();
    d.with(|s| {
        s.add_container("c", vec![], false);
        s.containers[0].host_port = Some(38000);
        s.taken_ports.insert(38000);
    });
    let e = api(&d).start_container("c", T).unwrap_err();
    assert!(e.is_port_collision(), "{e}");
    let other = ApiError::Status {
        op: "x",
        status: 500,
        message: "something else".into(),
    };
    assert!(!other.is_port_collision());
}

#[test]
fn malformed_json_and_missing_fields_are_errors_not_guesses() {
    let d = FakeDocker::start();
    let a = api(&d);
    d.fault(
        None,
        "/images/",
        1,
        Action::Respond(200, "{not json".into()),
    );
    assert!(matches!(
        a.image_inspect("x", T),
        Err(ApiError::Malformed { .. })
    ));
    d.fault(
        None,
        "/images/",
        1,
        Action::Respond(200, "{\"RepoTags\":[]}".into()),
    );
    assert!(
        matches!(a.image_inspect("x", T), Err(ApiError::Malformed { detail, .. }) if detail.contains("Id"))
    );
    d.fault(
        None,
        "/containers/",
        1,
        Action::Respond(
            200,
            "{\"Id\":\"a\",\"Name\":\"/n\",\"State\":{\"Running\":\"yes\"}}".into(),
        ),
    );
    assert!(
        matches!(a.container_inspect("a", T), Err(ApiError::Malformed { detail, .. }) if detail.contains("Running"))
    );
    d.fault(
        None,
        "/containers/json",
        1,
        Action::Respond(200, "{\"not\":\"an array\"}".into()),
    );
    assert!(matches!(
        a.list_containers(T),
        Err(ApiError::Malformed { .. })
    ));
    d.fault(
        None,
        "/containers/json",
        1,
        Action::Respond(200, "[{\"Id\":\"a\",\"Labels\":{\"k\":5}}]".into()),
    );
    assert!(
        matches!(a.list_containers(T), Err(ApiError::Malformed { detail, .. }) if detail.contains("label"))
    );
    d.fault(None, "/_ping", 1, Action::Respond(200, "NOT OK".into()));
    assert!(matches!(a.ping(T), Err(ApiError::Malformed { .. })));
    d.fault(
        None,
        "/containers/create",
        1,
        Action::Respond(201, "{\"Warnings\":[]}".into()),
    );
    let spec = CreateSpec {
        image: "i",
        labels: &[],
        env: &[],
        container_port: 1,
        host_port: 2,
        limits: Limits::FIRST_TEST,
    };
    assert!(matches!(
        a.create_container("n", &spec, T),
        Err(ApiError::Malformed { .. })
    ));
    assert_eq!(d.with(|s| s.containers.len()), 0);
}

#[test]
fn garbage_that_is_not_http_is_malformed() {
    let d = FakeDocker::start();
    for (raw, what) in [
        (b"this is not http\r\n\r\n".to_vec(), "status line"),
        (b"HTTP/1.1 999 Nope\r\n\r\n".to_vec(), "status code"),
        (
            b"HTTP/1.1 200 OK\r\nbroken header\r\n\r\n".to_vec(),
            "header",
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\n".to_vec(),
            "Content-Length",
        ),
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n".to_vec(),
            "chunk",
        ),
    ] {
        d.fault(None, "/_ping", 1, Action::Raw(raw));
        let e = UnixHttp::new(d.path())
            .request("GET", "/_ping", None, T)
            .unwrap_err();
        assert!(matches!(e, HttpError::Malformed(_)), "{what}: {e:?}");
    }
}

#[test]
fn truncated_bodies_and_disconnects_are_disconnect_errors() {
    let d = FakeDocker::start();
    let h = UnixHttp::new(d.path());
    d.fault(
        None,
        "/_ping",
        1,
        Action::Truncate {
            claimed: 100,
            body: "{\"partial\":".into(),
        },
    );
    assert!(
        matches!(h.request("GET", "/_ping", None, T), Err(HttpError::Disconnected(m)) if m.contains("body ended"))
    );
    d.fault(None, "/_ping", 1, Action::Disconnect);
    assert!(
        matches!(h.request("GET", "/_ping", None, T), Err(HttpError::Disconnected(m)) if m.contains("before sending"))
    );
    d.fault(
        None,
        "/_ping",
        1,
        Action::Raw(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabc".to_vec()),
    );
    assert!(matches!(
        h.request("GET", "/_ping", None, T),
        Err(HttpError::Disconnected(_))
    ));
    // and the API layer surfaces it as a transport error, never as a partial success
    d.fault(
        None,
        "/images/",
        1,
        Action::Truncate {
            claimed: 500,
            body: "{\"Id\":\"sha256:ab".into(),
        },
    );
    assert!(matches!(
        api(&d).image_inspect("x", T),
        Err(ApiError::Transport(HttpError::Disconnected(_)))
    ));
}

#[test]
fn a_slow_engine_times_out_at_the_deadline() {
    let d = FakeDocker::start();
    d.fault(None, "/_ping", 1, Action::Delay(Duration::from_millis(600)));
    let t0 = std::time::Instant::now();
    let e = api(&d).ping(Duration::from_millis(100)).unwrap_err();
    assert!(e.is_timeout(), "{e}");
    assert!(
        t0.elapsed() < Duration::from_millis(500),
        "gave up at the deadline, not after the delay"
    );
}

#[test]
fn a_missing_socket_is_a_connect_error() {
    let p = std::env::temp_dir().join("hdd-no-such.sock");
    let e = UnixHttp::new(&p)
        .request("GET", "/_ping", None, T)
        .unwrap_err();
    assert!(matches!(e, HttpError::Connect(m) if m.contains("hdd-no-such.sock")));
}

#[test]
fn chunked_responses_are_decoded() {
    let d = FakeDocker::start();
    d.fault(
        None,
        "/_ping",
        1,
        Action::Chunked("{\"a\":[1,2,3],\"b\":\"hello world\"}".into()),
    );
    let r = UnixHttp::new(d.path())
        .request("GET", "/_ping", None, T)
        .unwrap();
    assert_eq!(r.text(), "{\"a\":[1,2,3],\"b\":\"hello world\"}");
}

#[test]
fn oversized_bodies_are_cut_off_by_the_cap() {
    let d = FakeDocker::start();
    d.fault(None, "/_ping", 1, Action::Respond(200, "x".repeat(10_000)));
    let e = UnixHttp::new(d.path())
        .with_max_body(1_000)
        .request("GET", "/_ping", None, T)
        .unwrap_err();
    assert_eq!(e, HttpError::TooLarge(1_000));
}

#[test]
fn logs_are_bounded_and_demultiplexed() {
    let d = FakeDocker::start();
    let id = d.with(|s| {
        let id = s.add_container("c", vec![], true);
        s.containers[0].logs = "line\n".repeat(2_000).into_bytes();
        id
    });
    let big = api(&d).logs(&id, 5, 1_000, T).unwrap();
    assert!(
        big.truncated && big.text.is_empty(),
        "over the cap: reported truncated, not partially trusted"
    );
    let ok = api(&d).logs(&id, 5, 1 << 20, T).unwrap();
    assert_eq!(ok.text.lines().count(), 2_000);
    // frames cut mid-way are reported as truncated, keeping what is whole
    let cut = demux(&[1, 0, 0, 0, 0, 0, 0, 10, b'a', b'b']);
    assert_eq!((cut.text.as_str(), cut.truncated), ("ab", true));
    // a body that is not framed is a TTY stream
    assert_eq!(demux(b"plain tty text").text, "plain tty text");
}

#[test]
fn control_characters_in_a_path_are_refused_before_sending() {
    let d = FakeDocker::start();
    let e = UnixHttp::new(d.path())
        .request("GET", "/containers/a b/json", None, T)
        .unwrap_err();
    assert!(matches!(e, HttpError::Malformed(_)));
    assert!(d.requests().is_empty());
}

#[test]
fn json_parser_rejects_what_it_should() {
    for bad in [
        "",
        "{",
        "[1,]",
        "{\"a\":}",
        "nul",
        "\"\\x\"",
        "[1] x",
        "\"\\ud800\"",
        "01a",
        "{\"a\" 1}",
        "[\"\u{1}\"]",
    ] {
        assert!(Json::parse(bad).is_err(), "{bad:?} should not parse");
    }
    let deep = "[".repeat(100) + &"]".repeat(100);
    assert!(Json::parse(&deep).is_err(), "depth is bounded");
    let j = Json::parse("{\"a\":[1,2.5,\"\\u00e9\\ud83d\\ude00\"],\"b\":null}").unwrap();
    assert_eq!(
        j.get("a").unwrap().as_array().unwrap()[2].as_str(),
        Some("é😀")
    );
    assert_eq!(Json::parse(&j.to_json()).unwrap(), j);
}
