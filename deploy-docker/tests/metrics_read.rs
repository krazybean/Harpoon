//! `metrics::read` against a fake Docker on a Unix socket: it asks only read-only questions, measures a container only
//! after its labels prove it, and measures by the verified full id, never the name.

use harpoon_deploy_docker::api::DockerApi;
use harpoon_deploy_docker::metrics::{read, Outcome};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Fake {
    api: DockerApi,
    requests: Arc<Mutex<Vec<String>>>,
}

fn fake(name: &str, routes: Vec<(&'static str, u16, String)>) -> Fake {
    let dir = std::env::temp_dir().join(format!("hpm-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { return };
            let mut buf = [0u8; 4096];
            let n = c.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let line = head.lines().next().unwrap_or("").to_string();
            log.lock().unwrap().push(line.clone());
            let (status, body) = routes
                .iter()
                .find(|(p, _, _)| line.contains(p))
                .map(|(_, s, b)| (*s, b.clone()))
                .unwrap_or((404, r#"{"message":"no such container"}"#.into()));
            let _ = write!(c, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    Fake { api: DockerApi::new(&sock, Duration::from_secs(5)), requests }
}

const OWN: &str = r#"{"io.harpoon.managed":"true","io.harpoon.app":"app_1","io.harpoon.deployment":"dep_1"}"#;
fn inspect(id: &str, running: bool, labels: &str) -> String {
    format!(r#"{{"Id":"{id}","Name":"/hp-dep_1","State":{{"Running":{running},"StartedAt":"2026-11-14T00:00:00Z"}},"RestartCount":0,"Config":{{"Labels":{labels}}},"HostConfig":{{"Memory":0}}}}"#)
}
const STATS: &str = r#"{"cpu_stats":{"cpu_usage":{"total_usage":150},"system_cpu_usage":2000,"online_cpus":2},"precpu_stats":{"cpu_usage":{"total_usage":100},"system_cpu_usage":1000},"memory_stats":{"usage":1000,"stats":{"inactive_file":200}},"networks":{"eth0":{"rx_bytes":10,"tx_bytes":4}}}"#;
const WAIT: Duration = Duration::from_secs(5);

#[test]
fn the_owned_running_container_is_measured_by_its_full_id_with_read_only_requests() {
    let f = fake("ok", vec![("/containers/hp-dep_1/json", 200, inspect("abc123def456", true, OWN)), ("/containers/abc123def456/stats", 200, STATS.into())]);
    let Outcome::Reading(r) = read(&f.api, "hp-dep_1", "app_1", "dep_1", Some("abc123"), WAIT).unwrap() else { panic!("expected a reading") };
    let s = r.sample.unwrap();
    assert_eq!((s.cpu_percent, s.mem_working_set(), s.net_rx, s.net_tx), (Some(10.0), Some(800), Some(10), Some(4)));
    let seen = f.requests.lock().unwrap().clone();
    assert!(seen.iter().all(|l| l.starts_with("GET ")), "only GET requests: {seen:?}");
    assert!(seen[0].contains("/containers/hp-dep_1/json") && seen[1].contains("/containers/abc123def456/stats"), "{seen:?}");
}

#[test]
fn a_container_that_is_not_the_recorded_one_is_never_measured() {
    for (name, labels, recorded) in [
        ("other-app", r#"{"io.harpoon.managed":"true","io.harpoon.app":"app_2","io.harpoon.deployment":"dep_1"}"#, None),
        ("other-dep", r#"{"io.harpoon.managed":"true","io.harpoon.app":"app_1","io.harpoon.deployment":"dep_0"}"#, None),
        ("no-labels", "{}", None),
        ("other-id", OWN, Some("ffffff")),
    ] {
        let f = fake(name, vec![("/json", 200, inspect("abc123def456", true, labels)), ("/stats", 200, STATS.into())]);
        let out = read(&f.api, "hp-dep_1", "app_1", "dep_1", recorded, WAIT).unwrap();
        assert!(matches!(out, Outcome::NotOwned(_)), "{name}: {out:?}");
        assert!(f.requests.lock().unwrap().iter().all(|l| !l.contains("/stats")), "{name}: no statistics were requested");
    }
}

#[test]
fn a_missing_container_is_absent_and_a_stopped_one_is_not_measured() {
    let gone = fake("gone", vec![]);
    assert_eq!(read(&gone.api, "hp-dep_1", "app_1", "dep_1", None, WAIT).unwrap(), Outcome::Absent);
    let stopped = fake("stopped", vec![("/json", 200, inspect("abc123def456", false, OWN)), ("/stats", 200, STATS.into())]);
    let Outcome::Reading(r) = read(&stopped.api, "hp-dep_1", "app_1", "dep_1", None, WAIT).unwrap() else { panic!() };
    assert!(!r.identity.running && r.sample.is_none());
    assert!(stopped.requests.lock().unwrap().iter().all(|l| !l.contains("/stats")));
}

#[test]
fn docker_errors_and_malformed_answers_are_errors_not_figures() {
    let broken = fake("broken", vec![("/json", 500, r#"{"message":"boom"}"#.into())]);
    assert!(read(&broken.api, "hp-dep_1", "app_1", "dep_1", None, WAIT).is_err());
    let garbled = fake("garbled", vec![("/json", 200, "not json".into())]);
    assert!(read(&garbled.api, "hp-dep_1", "app_1", "dep_1", None, WAIT).is_err());
    let nostats = fake("nostats", vec![("/json", 200, inspect("abc123def456", true, OWN)), ("/stats", 200, "{".into())]);
    assert!(read(&nostats.api, "hp-dep_1", "app_1", "dep_1", None, WAIT).is_err());
    // statistics Docker could not give leave every figure empty rather than zero
    let empty = fake("empty", vec![("/json", 200, inspect("abc123def456", true, OWN)), ("/stats", 200, "{}".into())]);
    let Outcome::Reading(r) = read(&empty.api, "hp-dep_1", "app_1", "dep_1", None, WAIT).unwrap() else { panic!() };
    let s = r.sample.unwrap();
    assert_eq!((s.cpu_percent, s.mem_raw, s.net_rx), (None, None, None));
}
