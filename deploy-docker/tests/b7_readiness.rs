//! B7 phase 1 (offline): image identity, health attribution, port-mapping and host-listener
//! checks, the private build-context snapshot, the guest free-space rule and the live-execution
//! gates. Everything runs against the fake Docker and fake Buildx on temporary directories; the
//! "applications" are fakes or loopback servers owned by the test.

mod support;

use harpoon_deploy::backend::ResourceOps;
use harpoon_deploy::engine::JournalError;
use harpoon_deploy::health::Observation;
use harpoon_deploy::ledger::{LedgerRecord, RemoveRequest, RemoveResult, ResourceKind};
use harpoon_deploy::state::*;
use harpoon_deploy::time::Entropy;
use harpoon_deploy_docker::backend::{
    preliminary_name, serving_name, Attribution, DockerBackend, DockerConfig, MARKER_ENV,
    MARKER_HEADER,
};
use harpoon_deploy_docker::forwarder::{parse_lsof_f, verify_forwarder, Listener, LsofCheck};
use harpoon_deploy_docker::guest::{check_guest_free, free_from_df, GuestDiskProbe, Unavailable};
use harpoon_deploy_docker::json::Json;
use harpoon_deploy_docker::probe::{HttpTransport, ResponseMarker, TcpHttp};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::fake_docker::{Action, St};
use support::*;

// ------------------------------------------------------------------ helpers

fn backend_with<T: HttpTransport>(
    h: &Harness,
    t: T,
    f: impl FnOnce(&mut DockerConfig),
) -> DockerBackend<T> {
    let mut cfg = config(&h.ws, &h.docker);
    f(&mut cfg);
    DockerBackend::with_transport(cfg, Some(&h.ws.resources()), t)
        .expect("bundled buildx resolves")
        .with_entropy(test_entropy())
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 << 30))
}

fn ok200() -> Scripted {
    Scripted::always(Observation::Status(200))
}

fn run_ok<T: HttpTransport>(
    h: &mut Harness,
    b: &mut DockerBackend<T>,
) -> harpoon_deploy::engine::Report {
    let mut j = StampedJournal::new();
    h.run(b, &mut j)
}

fn failure(h: &Harness) -> (Step, String) {
    let f = h.dep.failure.clone().expect("the deployment failed");
    (f.step, f.message)
}

fn creates(h: &Harness) -> Vec<(String, Json)> {
    h.docker
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST" && r.path.starts_with("/containers/create"))
        .map(|r| (r.path.clone(), Json::parse(&r.body).unwrap()))
        .collect()
}

fn env_of(body: &Json) -> Vec<String> {
    body.get("Env")
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn marker_in(body: &Json) -> Option<String> {
    env_of(body).into_iter().find_map(|e| {
        e.strip_prefix(&format!("{MARKER_ENV}="))
            .map(str::to_string)
    })
}

fn attributed(c: &mut DockerConfig) {
    c.attribution = Attribution::PerContainer;
}

/// A simulated "application": answers a health request the way the fixture would, using the
/// marker Docker was given for whichever container is published on that port.
#[derive(Clone, Copy, PartialEq, Eq)]
enum App {
    Honest,
    NoMarker,
    WrongMarker,
}

struct FakeApp {
    st: Arc<Mutex<St>>,
    mode: App,
    seen: Mutex<BTreeMap<u16, String>>,
    /// On the first probe, re-publish every running container on this host port.
    move_to: Mutex<Option<u16>>,
}

impl FakeApp {
    fn new(h: &Harness, mode: App) -> FakeApp {
        FakeApp {
            st: h.docker.st.clone(),
            mode,
            seen: Mutex::new(BTreeMap::new()),
            move_to: Mutex::new(None),
        }
    }
    fn moving_mapping_on_first_probe(self, port: u16) -> FakeApp {
        *self.move_to.lock().unwrap() = Some(port);
        self
    }
    fn marker_on(&self, port: u16) -> Option<String> {
        let s = self.st.lock().unwrap();
        let found = s.containers.iter().find_map(|c| {
            (c.host_port == Some(port))
                .then(|| marker_in(&c.create_body))
                .flatten()
        });
        drop(s);
        if let Some(m) = &found {
            self.seen.lock().unwrap().insert(port, m.clone());
        }
        found.or_else(|| self.seen.lock().unwrap().get(&port).cloned())
    }
}

impl HttpTransport for FakeApp {
    fn get(&self, _p: u16, _path: &str, _t: Duration) -> Observation {
        Observation::Status(200)
    }
    fn get_expecting(
        &self,
        port: u16,
        _path: &str,
        _t: Duration,
        marker: Option<&ResponseMarker>,
    ) -> Observation {
        let own = self.marker_on(port);
        if let Some(p) = self.move_to.lock().unwrap().take() {
            for c in self
                .st
                .lock()
                .unwrap()
                .containers
                .iter_mut()
                .filter(|c| c.running)
            {
                c.host_port = Some(p);
            }
        }
        let served = match self.mode {
            App::Honest => own,
            App::NoMarker => None,
            App::WrongMarker => Some("0".repeat(32)),
        };
        match (marker, served) {
            (None, _) => Observation::Status(200),
            (Some(ResponseMarker::Header { value, .. }), Some(s)) if *value == s => {
                Observation::Status(200)
            }
            _ => Observation::ConnectFailed(
                "response did not carry the expected deployment marker".into(),
            ),
        }
    }
    fn is_simulated(&self) -> bool {
        true
    }
}

// ------------------------------------------------------------------ 1. image identity

#[test]
fn image_tags_carry_128_random_bits_and_differ_between_attempts() {
    let mut tags = Vec::new();
    for seed in [1u8, 2, 3] {
        let mut h = Harness::new();
        h.seed_built_image();
        let mut b = backend_with(&h, ok200(), |_| {})
            .with_entropy(harpoon_deploy::time::SeqEntropy::new(seed));
        let mut j = StampedJournal::new();
        let r = h.run(&mut b, &mut j);
        assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
        let intent = j
            .stamps
            .iter()
            .find_map(|(_, rec)| match rec {
                LedgerRecord::Intent { key, .. } if key.kind == ResourceKind::Image => {
                    Some(key.name.clone())
                }
                _ => None,
            })
            .unwrap();
        let (head, nonce) = intent.rsplit_once('-').unwrap();
        assert!(
            head.starts_with("harpoon/app_") && head.contains(":dep_"),
            "{intent}"
        );
        assert_eq!(nonce.len(), 32, "{intent}");
        assert!(
            nonce
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{intent}"
        );
        tags.push(intent);
    }
    tags.sort();
    tags.dedup();
    assert_eq!(tags.len(), 3, "a different tag per attempt");
    // real randomness: two OS-entropy backends do not agree
    let mut a = harpoon_deploy::time::OsEntropy::new();
    let (mut x, mut y) = ([0u8; 16], [0u8; 16]);
    a.fill(&mut x).unwrap();
    a.fill(&mut y).unwrap();
    assert_ne!(x, y);
}

#[test]
fn the_image_id_comes_from_the_iid_file_becomes_the_receipt_and_the_file_is_removed() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    let args = h.ws.buildx_record("args.txt");
    assert!(args.contains("--iidfile"), "{args}");
    let tag = tag_for(&h.dep.application_id, &h.dep.id);
    let e = r
        .ledger
        .find(
            &h.dep.id,
            &harpoon_deploy::ledger::ResourceKey::new(ResourceKind::Image, &tag),
        )
        .unwrap();
    assert_eq!(
        e.id.as_deref(),
        Some(fake_iid(1).as_str()),
        "the receipt is the ID Buildx reported"
    );
    let left: Vec<_> = std::fs::read_dir(h.ws.scratch())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        left.iter()
            .all(|n| !n.starts_with("iid-") && !n.starts_with("ctx-")),
        "{left:?}"
    );
}

#[test]
fn containers_are_created_from_the_verified_image_id_never_the_tag() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |_| {});
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Succeeded);
    let cs = creates(&h);
    assert_eq!(cs.len(), 2);
    for (_, body) in cs {
        assert_eq!(
            body.get("Image").and_then(Json::as_str),
            Some(fake_iid(1).as_str())
        );
    }
}

#[test]
fn a_bad_image_id_report_fails_the_build_before_any_container_or_receipt() {
    let cases: [(&str, &str); 5] = [
        ("not a sha256 id", "printf 'latest' > \"$a\""),
        ("uppercase hex", "printf 'sha256:ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB' > \"$a\""),
        ("short id", "printf 'sha256:abab' > \"$a\""),
        ("empty file", ": > \"$a\""),
        ("no file at all", "true"),
    ];
    for (what, write) in cases {
        let mut h = Harness::new();
        h.seed_built_image();
        h.ws.install_buildx(&format!("for a in \"$@\"; do if [ \"$p\" = \"--iidfile\" ]; then rm -f \"$a\"; {write}; fi; p=\"$a\"; done"));
        let mut b = backend_with(&h, ok200(), |_| {});
        let r = run_ok(&mut h, &mut b);
        assert_eq!(r.status, DeploymentStatus::Failed, "{what}");
        let (step, msg) = failure(&h);
        assert_eq!(step, Step::Build, "{what}: {msg}");
        assert!(msg.contains("image ID"), "{what}: {msg}");
        assert!(creates(&h).is_empty(), "{what}");
        assert!(h.objects_for_dep().is_empty(), "{what}");
    }
}

#[test]
fn an_id_docker_does_not_know_or_that_lacks_our_labels_is_never_recorded_or_used() {
    // reported ID unknown to Docker
    let mut h = Harness::new();
    let mut b = backend_with(&h, ok200(), |_| {});
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    assert!(
        failure(&h).1.contains("has no such image"),
        "{}",
        failure(&h).1
    );

    // the ID exists but carries only one of the three labels
    let mut h = Harness::new();
    h.seed_built_image_with(vec![("io.harpoon.managed".into(), "true".into())], vec![]);
    let mut b = backend_with(&h, ok200(), |_| {});
    let r = run_ok(&mut h, &mut b);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(failure(&h)
        .1
        .contains("does not carry this deployment's Harpoon labels"));
    let tag = tag_for(&h.dep.application_id, &h.dep.id);
    let e = r
        .ledger
        .find(
            &h.dep.id,
            &harpoon_deploy::ledger::ResourceKey::new(ResourceKind::Image, &tag),
        )
        .unwrap();
    assert!(
        e.id.is_none() || e.is_released(),
        "an image that is not provably ours gets no receipt: {e:?}"
    );
    assert!(creates(&h).is_empty());
    assert!(
        h.docker
            .with(|s| s.images.iter().any(|i| i.id == fake_iid(1))),
        "and is not removed"
    );

    // another deployment's labels at the reported ID
    let mut h = Harness::new();
    let other = harpoon_deploy::ids::IdGenerator::new(
        harpoon_deploy::time::ManualClock::new(5),
        harpoon_deploy::time::SeqEntropy::new(99),
    )
    .deployment()
    .unwrap();
    let theirs = labels(&h.dep.application_id, &other);
    h.seed_built_image_with(theirs, vec![]);
    let mut b = backend_with(&h, ok200(), |_| {});
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    assert!(creates(&h).is_empty());
}

#[test]
fn docker_answering_for_a_different_image_than_the_one_asked_about_is_refused() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker.fault_after(
        Some("GET"),
        "/images/sha256:",
        0,
        1,
        Action::Respond(
            200,
            format!(r#"{{"Id":"{}","Config":{{"Labels":{{}}}}}}"#, fake_iid(99)),
        ),
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    let (step, msg) = failure(&h);
    assert_eq!(step, Step::PreliminaryCheck, "{msg}");
    assert!(msg.contains("no longer the image that was built"), "{msg}");
    assert!(creates(&h).is_empty());
}

#[test]
fn a_tag_that_is_moved_after_the_build_stops_the_deployment_and_touches_nothing_foreign() {
    let mut h = Harness::new();
    h.seed_built_image();
    // Right when the backend re-checks the tag (the second look at it), another writer has taken it.
    let foreign_id = fake_iid(500);
    let fid = foreign_id.clone();
    h.docker.fault_after(
        Some("GET"),
        "/images/harpoon/",
        1,
        1,
        Action::Race(Arc::new(move |s: &mut St| {
            let tag = s
                .requests
                .last()
                .map(|r| {
                    r.path
                        .trim_start_matches("/images/")
                        .trim_end_matches("/json")
                        .to_string()
                })
                .unwrap();
            for i in s.images.iter_mut() {
                i.tags.retain(|t| *t != tag);
            }
            s.add_image_with_id(&fid, &[&tag], vec![], vec![]);
        })),
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    let r = run_ok(&mut h, &mut b);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let (step, msg) = failure(&h);
    assert_eq!(step, Step::Build, "{msg}");
    assert!(msg.contains("names a different image"), "{msg}");
    assert!(creates(&h).is_empty(), "nothing ran from the moved tag");
    // our image was recorded (Retain) and is still there; the foreign one was not touched
    let tag = tag_for(&h.dep.application_id, &h.dep.id);
    let e = r
        .ledger
        .find(
            &h.dep.id,
            &harpoon_deploy::ledger::ResourceKey::new(ResourceKind::Image, &tag),
        )
        .unwrap();
    assert_eq!(e.id.as_deref(), Some(fake_iid(1).as_str()));
    assert!(h.docker.with(|s| s
        .images
        .iter()
        .any(|i| i.id == foreign_id && i.tags.contains(&tag))));
    assert!(h
        .docker
        .with(|s| s.images.iter().any(|i| i.id == fake_iid(1))));
    assert!(!h.docker.requests().iter().any(|q| q.method == "DELETE"));
}

#[test]
fn removal_is_by_the_verified_id_even_when_the_tag_now_names_something_else() {
    let h = Harness::new();
    let tag = tag_for(&h.dep.application_id, &h.dep.id);
    let ours = fake_iid(1);
    let theirs = fake_iid(2);
    h.docker.with(|s| {
        s.add_image_with_id(&ours, &[], labels(&h.dep.application_id, &h.dep.id), vec![]);
        s.add_image_with_id(&theirs, &[&tag], vec![], vec![]);
    });
    let mut b = backend_with(&h, ok200(), |_| {});
    let req = |id: &str| RemoveRequest {
        kind: ResourceKind::Image,
        id: id.to_string(),
        name: tag.clone(),
        app: h.dep.application_id.clone(),
        deployment: h.dep.id.clone(),
    };
    // asked to remove the foreign image that now holds the name: refused (not our labels)
    assert!(matches!(
        b.remove_resource(&req(&theirs)).unwrap(),
        RemoveResult::Refused(_)
    ));
    assert!(h.docker.with(|s| s.images.iter().any(|i| i.id == theirs)));
    // asked to remove ours by its ID: removed, by ID, never by the tag
    assert_eq!(
        b.remove_resource(&req(&ours)).unwrap(),
        RemoveResult::Removed
    );
    let dels: Vec<_> = h
        .docker
        .requests()
        .into_iter()
        .filter(|r| r.method == "DELETE")
        .collect();
    assert_eq!(dels.len(), 1);
    assert!(
        dels[0].path.contains(&ours) && !dels[0].path.contains("harpoon/"),
        "{}",
        dels[0].path
    );
    assert!(dels[0].path.contains("force=false") && dels[0].path.contains("noprune=true"));
    assert!(h.docker.with(|s| s
        .images
        .iter()
        .any(|i| i.id == theirs && i.tags.contains(&tag))));
}

#[test]
fn two_deployments_of_one_application_never_share_a_tag_or_an_image() {
    let mut h = Harness::new();
    let mut seen = Vec::new();
    for _ in 0..2 {
        h.seed_built_image();
        // the same randomness both times: only the deployment ID keeps the tags apart
        let mut b = backend_with(&h, ok200(), |_| {});
        let r = run_ok(&mut h, &mut b);
        assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
        let tag = tag_for(&h.dep.application_id, &h.dep.id);
        let e = r
            .ledger
            .find(
                &h.dep.id,
                &harpoon_deploy::ledger::ResourceKey::new(ResourceKind::Image, &tag),
            )
            .unwrap();
        seen.push((tag, e.id.clone().unwrap()));
        h.dep = new_deployment(&mut h.ids);
    }
    assert_ne!(seen[0].0, seen[1].0);
    assert_ne!(seen[0].1, seen[1].1);
}

// ------------------------------------------------------------------ 2. health attribution

#[test]
fn each_container_gets_its_own_random_marker_in_its_environment_and_nowhere_else() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, FakeApp::new(&h, App::Honest), attributed);
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    let cs = creates(&h);
    let markers: Vec<String> = cs
        .iter()
        .map(|(_, b)| marker_in(b).expect("marker env"))
        .collect();
    assert_eq!(markers.len(), 2);
    assert_ne!(
        markers[0], markers[1],
        "preliminary and serving containers differ"
    );
    for m in &markers {
        assert_eq!(m.len(), 32);
        assert!(m.bytes().all(|c| c.is_ascii_hexdigit()));
        assert!(
            !j.stamps
                .iter()
                .any(|(_, rec)| format!("{rec:?}").contains(m)),
            "the marker is not journaled"
        );
        assert!(
            !h.ws.buildx_record("env.txt").contains(m)
                && !h.ws.buildx_record("args.txt").contains(m),
            "nor given to the builder"
        );
    }
    assert!(
        cs.iter().all(|(_, b)| env_of(b).len() == 1),
        "only the marker was added"
    );
}

#[test]
fn a_response_must_carry_this_containers_marker_unrelated_missing_wrong_and_stale_fail() {
    for (what, mode) in [
        ("unrelated listener, no marker", App::NoMarker),
        ("incorrect marker", App::WrongMarker),
    ] {
        let mut h = Harness::new();
        h.seed_built_image();
        let app = FakeApp::new(&h, mode);
        let mut b = backend_with(&h, app, attributed);
        let r = run_ok(&mut h, &mut b);
        assert_eq!(r.status, DeploymentStatus::Failed, "{what}");
        let (s, msg) = failure(&h);
        assert_eq!(s, Step::PreliminaryCheck, "{what}: {msg}");
        assert!(msg.contains("did not pass"), "{what}: {msg}");
        assert!(
            h.objects_for_dep().is_empty(),
            "{what}: failed attempts leave no container"
        );
    }
    // the application port answers with the marker the (now removed) preliminary container had
    let mut h = Harness::new();
    h.seed_built_image();
    let app = FakeApp::new(&h, App::Honest);
    let mut b = backend_with(&h, StaleOnFinal(app), attributed);
    let r = run_ok(&mut h, &mut b);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let (s, msg) = failure(&h);
    assert_eq!(s, Step::FinalCheck, "{msg}");
    assert!(msg.contains("did not pass"), "{msg}");
    assert!(h.objects_for_dep().is_empty());
}

/// Honest for the test port, then replays the preliminary container's marker on the application port.
struct StaleOnFinal(FakeApp);
impl HttpTransport for StaleOnFinal {
    fn get(&self, p: u16, path: &str, t: Duration) -> Observation {
        self.0.get(p, path, t)
    }
    fn get_expecting(
        &self,
        port: u16,
        path: &str,
        t: Duration,
        m: Option<&ResponseMarker>,
    ) -> Observation {
        if (38000..39000).contains(&port) {
            let stale = self.0.seen.lock().unwrap().get(&39000).cloned();
            return match (m, stale) {
                (Some(ResponseMarker::Header { value, .. }), Some(s)) if *value == s => {
                    Observation::Status(200)
                }
                _ => Observation::ConnectFailed(
                    "response did not carry the expected deployment marker".into(),
                ),
            };
        }
        self.0.get_expecting(port, path, t, m)
    }
    fn is_simulated(&self) -> bool {
        true
    }
}

static PORTS: Mutex<()> = Mutex::new(());

fn serve_header(
    port: u16,
    header: impl Fn() -> Option<String> + Send + Sync + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    let l = TcpListener::bind(("127.0.0.1", port)).ok()?;
    Some(std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { return };
            let mut buf = [0u8; 2048];
            let _ = c.set_read_timeout(Some(Duration::from_secs(1)));
            let _ = c.read(&mut buf);
            let h = header()
                .map(|v| format!("{MARKER_HEADER}: {v}\r\n"))
                .unwrap_or_default();
            let _ = write!(
                c,
                "HTTP/1.1 200 OK\r\n{h}Content-Length: 2\r\nConnection: close\r\n\r\nok"
            );
        }
    }))
}

#[test]
fn with_real_http_a_listener_that_echoes_the_marker_passes_and_an_unrelated_one_does_not() {
    let _g = PORTS.lock().unwrap_or_else(|e| e.into_inner());
    // application: reads the marker Docker gave the container published on its port (its environment)
    let mut h = Harness::new();
    h.seed_built_image();
    let st = h.docker.st.clone();
    let app = |port: u16, st: Arc<Mutex<St>>| {
        move || {
            let s = st.lock().unwrap();
            s.containers.iter().find_map(|c| {
                (c.host_port == Some(port))
                    .then(|| marker_in(&c.create_body))
                    .flatten()
            })
        }
    };
    let (Some(_a), Some(_b)) = (
        serve_header(39000, app(39000, st.clone())),
        serve_header(38000, app(38000, st.clone())),
    ) else {
        eprintln!("ports 38000/39000 are busy on this machine: skipped");
        return;
    };
    let mut b = backend_with(&h, TcpHttp, |c| {
        attributed(c);
        c.health.startup_deadline_ms = 1500;
    });
    let r = run_ok(&mut h, &mut b);
    assert_eq!(
        r.status,
        DeploymentStatus::Succeeded,
        "{:?} {:?}",
        h.dep.failure,
        r.evidence
    );
    assert!(
        r.evidence.iter().all(|(_, e)| !e.simulated),
        "real HTTP evidence"
    );
}

#[test]
fn with_real_http_an_unrelated_listener_on_the_test_port_fails_the_preliminary_check() {
    let _g = PORTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(_l) = serve_header(39000, || None) else {
        eprintln!("port 39000 is busy on this machine: skipped");
        return;
    };
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, TcpHttp, |c| {
        attributed(c);
        c.health.startup_deadline_ms = 400;
    });
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    let (step, msg) = failure(&h);
    assert_eq!(step, Step::PreliminaryCheck);
    assert!(msg.contains("marker"), "{msg}");
    assert!(h.docker.container(&serving_name(&h.dep.id)).is_none());
}

#[test]
fn a_transport_that_cannot_check_markers_never_accepts_an_attributed_deployment() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), attributed);
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    assert!(failure(&h).1.contains("marker"), "{}", failure(&h).1);
}

// ------------------------------------------------------------------ 2b. port mapping before and after

/// Change what Docker reports for the preliminary container the moment it is started.
fn on_start_of_preliminary(
    h: &Harness,
    change: impl Fn(&mut support::fake_docker::Ctr) + Send + Sync + 'static,
) {
    let pre = preliminary_name(&h.dep.id);
    h.docker.fault(
        Some("POST"),
        "/start",
        1,
        Action::Race(Arc::new(move |s: &mut St| {
            for c in s.containers.iter_mut().filter(|c| c.name == pre) {
                change(c);
            }
        })),
    );
}

#[test]
fn a_mapping_that_is_not_the_leased_port_stops_the_check_before_any_probe() {
    let mut h = Harness::new();
    h.seed_built_image();
    on_start_of_preliminary(&h, |c| c.host_port = Some(38999));
    let mut b = backend_with(&h, FakeApp::new(&h, App::Honest), attributed);
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    let (step, msg) = failure(&h);
    assert_eq!(step, Step::PreliminaryCheck);
    assert!(
        msg.contains("port mapping is not what was leased") && msg.contains("38999"),
        "{msg}"
    );
}

#[test]
fn a_mapping_that_changes_during_the_probe_makes_a_healthy_response_untrustworthy() {
    let mut h = Harness::new();
    h.seed_built_image();
    // the application answers correctly, and while it does Docker's mapping moves
    let app = FakeApp::new(&h, App::Honest).moving_mapping_on_first_probe(38998);
    let mut b = backend_with(&h, app, attributed);
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    let (step, msg) = failure(&h);
    assert_eq!(step, Step::PreliminaryCheck);
    assert!(
        msg.contains("could not be trusted") && msg.contains("38998"),
        "{msg}"
    );
}

#[test]
fn a_published_extra_port_or_a_specific_bind_address_is_refused() {
    for (what, ports, want) in [
        (
            "lan address",
            r#"{"8080/tcp":[{"HostIp":"192.168.1.20","HostPort":"39000"}]}"#,
            "bound to 192.168.1.20",
        ),
        (
            "extra published port",
            r#"{"8080/tcp":[{"HostIp":"0.0.0.0","HostPort":"39000"}],"9090/tcp":[{"HostIp":"0.0.0.0","HostPort":"39001"}]}"#,
            "9090",
        ),
        ("nothing published", r#"{}"#, "no published port"),
        (
            "malformed",
            r#"{"8080/tcp":[{"HostPort":"x"}]}"#,
            "not a port number",
        ),
        ("not an object", r#"[]"#, "not an object"),
    ] {
        let mut h = Harness::new();
        h.seed_built_image();
        let parsed = Json::parse(ports).unwrap();
        on_start_of_preliminary(&h, move |c| c.ports_override = Some(parsed.clone()));
        let mut b = backend_with(&h, FakeApp::new(&h, App::Honest), attributed);
        assert_eq!(
            run_ok(&mut h, &mut b).status,
            DeploymentStatus::Failed,
            "{what}"
        );
        let (_, msg) = failure(&h);
        assert!(msg.contains(want), "{what}: {msg}");
    }
}

// ------------------------------------------------------------------ 3. host listener check and live gates

#[test]
fn the_host_listener_check_runs_after_a_healthy_response_and_can_veto_it() {
    let calls = Arc::new(Mutex::new(Vec::<u16>::new()));
    for (verdict, expect_ok) in [
        (Ok(()), true),
        (Err("pid 1 (other) listens on *:39000".to_string()), false),
    ] {
        calls.lock().unwrap().clear();
        let mut h = Harness::new();
        h.seed_built_image();
        let c = calls.clone();
        let v = verdict.clone();
        let check = move |port: u16| {
            c.lock().unwrap().push(port);
            v.clone()
        };
        let mut b =
            backend_with(&h, FakeApp::new(&h, App::Honest), attributed).with_forwarder_check(check);
        let r = run_ok(&mut h, &mut b);
        if expect_ok {
            assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
            assert_eq!(
                *calls.lock().unwrap(),
                vec![39000, 38000],
                "once per check, with the leased port"
            );
        } else {
            assert_eq!(r.status, DeploymentStatus::Failed);
            let (step, msg) = failure(&h);
            assert_eq!(step, Step::PreliminaryCheck);
            assert!(
                msg.contains("could not be trusted")
                    && msg.contains("host listener")
                    && msg.contains("*:39000"),
                "{msg}"
            );
            assert_eq!(
                *calls.lock().unwrap(),
                vec![39000],
                "nothing went live after the veto"
            );
        }
    }
    // never asked when the response was not healthy
    let mut h = Harness::new();
    h.seed_built_image();
    let c = calls.clone();
    calls.lock().unwrap().clear();
    let mut b = backend_with(&h, FakeApp::new(&h, App::NoMarker), attributed).with_forwarder_check(
        move |p: u16| {
            c.lock().unwrap().push(p);
            Ok(())
        },
    );
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn live_gates_refuse_to_start_unless_attribution_listener_check_and_guest_probe_are_all_present() {
    struct Plenty;
    impl GuestDiskProbe for Plenty {
        fn free_bytes(&self) -> Result<u64, String> {
            Ok(100 << 30)
        }
    }
    let ok_check = |_p: u16| -> Result<(), String> { Ok(()) };
    // nothing configured: every missing requirement is named, and nothing is built
    let mut h = Harness::new();
    let mut b = backend_with(&h, FakeApp::new(&h, App::Honest), |c| c.live_gates = true);
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    let (step, msg) = failure(&h);
    assert_eq!(step, Step::Source);
    for want in [
        "attribution is off",
        "host-listener",
        "guest-side free-space",
    ] {
        assert!(msg.contains(want), "{msg}");
    }
    assert!(h.ws.buildx_record("args.txt").is_empty());
    assert!(!h.docker.requests().iter().any(|r| r.method != "GET"));

    // each one missing on its own
    for (label, attr, fw, gd) in [
        ("attribution", false, true, true),
        ("listener check", true, false, true),
        ("guest probe", true, true, false),
    ] {
        let mut h = Harness::new();
        h.seed_built_image();
        let mut b = backend_with(&h, FakeApp::new(&h, App::Honest), |c| {
            c.live_gates = true;
            if attr {
                attributed(c);
            }
        });
        if fw {
            b = b.with_forwarder_check(ok_check);
        }
        if gd {
            b = b.with_guest_disk(Plenty);
        }
        assert_eq!(
            run_ok(&mut h, &mut b).status,
            DeploymentStatus::Failed,
            "{label}"
        );
        assert_eq!(failure(&h).0, Step::Source, "{label}");
    }

    // all present: the deployment proceeds
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, FakeApp::new(&h, App::Honest), |c| {
        c.live_gates = true;
        attributed(c);
    })
    .with_forwarder_check(ok_check)
    .with_guest_disk(Plenty);
    let r = run_ok(&mut h, &mut b);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
}

// ------------------------------------------------------------------ 4. guest free space

struct Fixed(Result<u64, String>);
impl GuestDiskProbe for Fixed {
    fn free_bytes(&self) -> Result<u64, String> {
        self.0.clone()
    }
}

#[test]
fn guest_free_space_is_fail_closed() {
    let need = 3 << 30;
    assert_eq!(
        check_guest_free(&Fixed(Ok(10 << 30)), need).unwrap(),
        10 << 30
    );
    assert_eq!(
        check_guest_free(&Fixed(Ok(need)), need).unwrap(),
        need,
        "exactly enough is enough"
    );
    for (what, p, want) in [
        ("short", Fixed(Ok(need - 1)), "required"),
        ("zero", Fixed(Ok(0)), "no free space"),
        (
            "error",
            Fixed(Err("vsock closed".into())),
            "could not be verified",
        ),
    ] {
        let e = check_guest_free(&p, need).unwrap_err();
        assert!(e.contains(want), "{what}: {e}");
    }
    let e = check_guest_free(&Unavailable("no probe is implemented".into()), need).unwrap_err();
    assert!(
        e.contains("could not be verified") && e.contains("no probe is implemented"),
        "{e}"
    );
    assert_eq!(free_from_df("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/vda 100000 1 5000 1% /\n").unwrap(), 5000 * 1024);
    assert!(free_from_df("garbage").is_err() && free_from_df("").is_err());
}

#[test]
fn the_backend_refuses_a_build_the_guest_cannot_hold_or_cannot_confirm() {
    for (what, probe, ok) in [
        ("plenty", Fixed(Ok(50 << 30)), true),
        ("short", Fixed(Ok(1 << 30)), false),
        ("unknown", Fixed(Err("no answer".into())), false),
    ] {
        let mut h = Harness::new();
        h.seed_built_image();
        let mut b = backend_with(&h, ok200(), |_| {}).with_guest_disk(probe);
        let r = run_ok(&mut h, &mut b);
        if ok {
            assert_eq!(
                r.status,
                DeploymentStatus::Succeeded,
                "{what}: {:?}",
                h.dep.failure
            );
        } else {
            assert_eq!(r.status, DeploymentStatus::Failed, "{what}");
            let (step, msg) = failure(&h);
            assert_eq!(step, Step::Source, "{what}: {msg}");
            assert!(msg.contains("guest"), "{what}: {msg}");
            assert!(
                h.ws.buildx_record("args.txt").is_empty()
                    && !h.docker.requests().iter().any(|q| q.method != "GET"),
                "{what}"
            );
        }
    }
}

// ------------------------------------------------------------------ 5. forwarding evidence (parsing and verification)

const OK_LSOF: &str = "p4242\ncHarpoon\nn127.0.0.1:38000\n";

#[test]
fn lsof_output_is_parsed_strictly() {
    assert_eq!(
        parse_lsof_f(OK_LSOF).unwrap(),
        vec![Listener {
            pid: 4242,
            command: "Harpoon".into(),
            address: "127.0.0.1:38000".into()
        }]
    );
    assert!(
        parse_lsof_f("").unwrap().is_empty(),
        "no output means no listeners"
    );
    let two = parse_lsof_f("p1\ncA\nn127.0.0.1:38000\np2\ncB\nn*:38000\nn[::1]:38000\n").unwrap();
    assert_eq!(two.len(), 3);
    assert_eq!(
        (two[1].pid, two[1].host(), two[1].port()),
        (2, "*", Some(38000))
    );
    assert_eq!(two[2].host(), "[::1]");
    for bad in [
        "n127.0.0.1:1\n",
        "pxyz\n",
        "p1\ncA\nn\n",
        "p1\ncA\nZoops\n",
        "p1\n\ncA\n",
    ] {
        assert!(parse_lsof_f(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn only_one_loopback_listener_owned_by_the_forwarder_verifies() {
    let l = |pid: u32, cmd: &str, addr: &str| Listener {
        pid,
        command: cmd.into(),
        address: addr.into(),
    };
    assert!(verify_forwarder(
        &[l(4242, "Harpoon", "127.0.0.1:38000")],
        38000,
        4242,
        "Harpoon"
    )
    .is_ok());
    assert!(
        verify_forwarder(
            &[l(4242, "HarpoonMa", "127.0.0.1:38000")],
            38000,
            4242,
            "HarpoonManager"
        )
        .is_ok(),
        "lsof truncates names to 9 characters"
    );
    for (what, ls, want) in [
        ("nothing listening", vec![], "nothing listens"),
        (
            "another process",
            vec![l(99, "nginx", "127.0.0.1:38000")],
            "but the Harpoon forwarder is pid 4242",
        ),
        (
            "wildcard bind (LAN reachable)",
            vec![l(4242, "Harpoon", "*:38000")],
            "not loopback-only",
        ),
        (
            "all-interfaces bind",
            vec![l(4242, "Harpoon", "0.0.0.0:38000")],
            "not loopback-only",
        ),
        (
            "ipv6 loopback only",
            vec![l(4242, "Harpoon", "[::1]:38000")],
            "not on 127.0.0.1",
        ),
        (
            "right pid, wrong program",
            vec![l(4242, "python3", "127.0.0.1:38000")],
            "forwarder is pid",
        ),
        (
            "forwarder plus an interloper",
            vec![
                l(4242, "Harpoon", "127.0.0.1:38000"),
                l(7, "evil", "*:38000"),
            ],
            "2 listeners share port",
        ),
        (
            "a listener on a different port is not evidence",
            vec![l(4242, "Harpoon", "127.0.0.1:38001")],
            "nothing listens",
        ),
    ] {
        let e = verify_forwarder(&ls, 38000, 4242, "Harpoon")
            .unwrap_err()
            .join("; ");
        assert!(e.contains(want), "{what}: {e}");
    }
}

#[test]
fn the_lsof_check_runs_only_the_given_program_with_fixed_arguments_and_a_clean_environment() {
    let dir = std::env::temp_dir().join(format!("hdd-lsof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = |name: &str, out: &str| -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\necho \"$@\" > \"{d}/{name}.args\"\nenv > \"{d}/{name}.env\"\nprintf '%s' '{out}'\n", d = dir.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    };
    std::env::set_var("HARPOON_CANARY", "leaked");
    let good = LsofCheck {
        lsof: script("good", "p4242\ncHarpoon\nn127.0.0.1:38000\n"),
        pid: 4242,
        command: "Harpoon".into(),
    };
    use harpoon_deploy_docker::forwarder::HostListenerCheck;
    good.verify(38000).unwrap();
    let args = std::fs::read_to_string(dir.join("good.args")).unwrap();
    assert_eq!(args.trim(), "-nP -iTCP:38000 -sTCP:LISTEN -Fpcn");
    assert!(!std::fs::read_to_string(dir.join("good.env"))
        .unwrap()
        .contains("HARPOON_CANARY"));
    let wild = LsofCheck {
        lsof: script("wild", "p4242\ncHarpoon\nn*:38000\n"),
        pid: 4242,
        command: "Harpoon".into(),
    };
    assert!(wild
        .verify(38000)
        .unwrap_err()
        .contains("not loopback-only"));
    let garbage = LsofCheck {
        lsof: script("garbage", "what is this\n"),
        pid: 4242,
        command: "Harpoon".into(),
    };
    assert!(garbage
        .verify(38000)
        .unwrap_err()
        .contains("cannot read lsof output"));
    let missing = LsofCheck {
        lsof: dir.join("nope"),
        pid: 1,
        command: "x".into(),
    };
    assert!(missing.verify(1).unwrap_err().contains("cannot run"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ 6. the private source snapshot

fn confined(h: &Harness) -> harpoon_deploy_docker::confine::Source {
    harpoon_deploy_docker::confine::confine(&h.ws.context(), &h.ws.context(), "Dockerfile").unwrap()
}

#[test]
fn the_snapshot_is_a_private_faithful_copy_that_never_follows_links() {
    use harpoon_deploy_docker::ignore::IgnoreRules;
    use harpoon_deploy_docker::snapshot;
    let h = Harness::new();
    let ctx = h.ws.context();
    std::fs::create_dir_all(ctx.join("src/deep")).unwrap();
    std::fs::write(ctx.join("src/deep/app.sh"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(
        ctx.join("src/deep/app.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(ctx.join("src/data.txt"), "data").unwrap();
    std::fs::set_permissions(
        ctx.join("src/data.txt"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    std::fs::create_dir_all(ctx.join("node_modules")).unwrap();
    std::fs::write(ctx.join("node_modules/big"), vec![0u8; 5000]).unwrap();
    std::fs::write(ctx.join(".dockerignore"), "node_modules\n").unwrap();
    // links: one inside, one pointing out of the project
    symlink("data.txt", ctx.join("src/inner-link")).unwrap();
    let outside = h.ws.root.join("outside.txt");
    std::fs::write(&outside, "SECRET").unwrap();
    symlink(&outside, ctx.join("out-link")).unwrap();
    // a socket is not copied
    let _sock = std::os::unix::net::UnixListener::bind(ctx.join("s.sock")).unwrap();

    std::fs::create_dir_all(h.ws.scratch()).unwrap();
    let src = confined(&h);
    let rules = IgnoreRules::parse(&std::fs::read_to_string(ctx.join(".dockerignore")).unwrap());
    let snap = snapshot::create(&src, &rules, 1_000_000, &h.ws.scratch(), "abcd1234").unwrap();
    let d = &snap.dir;
    assert_eq!(
        std::fs::metadata(d).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::read_to_string(d.join("src/data.txt")).unwrap(),
        "data"
    );
    assert_eq!(
        std::fs::metadata(d.join("src/deep/app.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::metadata(d.join("src/data.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert!(
        !d.join("node_modules").exists(),
        "what .dockerignore excludes is not copied"
    );
    assert!(d.join(".dockerignore").is_file() && snap.dockerfile.is_file());
    // links stay links, with their text unchanged; the outside file's content was not copied
    assert!(std::fs::symlink_metadata(d.join("src/inner-link"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(d.join("out-link")).unwrap(), outside);
    assert!(std::fs::symlink_metadata(d.join("out-link"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(
        snap.skipped.iter().any(|s| s == "s.sock"),
        "{:?}",
        snap.skipped
    );
    assert!(!d.join("s.sock").exists());
    assert!(snap.files >= 4);
    // removal only for Harpoon's own snapshot directories
    assert!(snapshot::remove(&snap, &h.ws.scratch()).is_ok() && !d.exists());
    let fake = snapshot::Snapshot {
        dir: h.ws.root.join("src"),
        dockerfile: PathBuf::new(),
        files: 0,
        bytes: 0,
        skipped: vec![],
    };
    assert!(snapshot::remove(&fake, &h.ws.root).is_err());
    assert!(
        h.ws.context().join("Dockerfile").exists(),
        "the user's folder was not touched"
    );
    assert!(snapshot::snapshot_dir(&h.ws.scratch(), "../x").is_err());
    assert!(snapshot::snapshot_dir(&h.ws.scratch(), "").is_err());
}

#[test]
fn a_file_swapped_for_a_link_while_it_is_being_copied_stops_the_copy_and_leaves_no_snapshot() {
    use harpoon_deploy_docker::ignore::IgnoreRules;
    use harpoon_deploy_docker::snapshot::{self, SnapshotError};
    let h = Harness::new();
    let ctx = h.ws.context();
    let secret = h.ws.root.join("secret.txt");
    std::fs::write(&secret, "TOPSECRET").unwrap();
    std::fs::write(ctx.join("config.txt"), "harmless").unwrap();
    std::fs::create_dir_all(h.ws.scratch()).unwrap();
    let src = confined(&h);
    let target = ctx.join("config.txt");
    let secret2 = secret.clone();
    let mut swapped = false;
    let r = snapshot::create_with(
        &src,
        &IgnoreRules::none(),
        1 << 20,
        &h.ws.scratch(),
        "beef",
        &mut |p| {
            if p.ends_with("config.txt") && !swapped {
                swapped = true;
                std::fs::remove_file(&target).unwrap();
                symlink(&secret2, &target).unwrap();
            }
        },
    );
    assert!(
        matches!(&r, Err(SnapshotError::Changed(p)) if p.ends_with("config.txt")),
        "{r:?}"
    );
    assert!(
        !h.ws.scratch().join("ctx-beef").exists(),
        "a failed snapshot removes itself"
    );
    // swapped for a different regular file (another inode) is caught the same way
    std::fs::remove_file(&target).unwrap();
    std::fs::write(&target, "harmless").unwrap();
    let t2 = target.clone();
    // keep the old inode alive elsewhere, so the replacement cannot be handed the same inode
    // number (file systems reuse freed inodes at once, which made this check pass on Linux CI)
    let keep_alive = h.ws.root.join("old-inode");
    let mut done = false;
    let r = snapshot::create_with(
        &src,
        &IgnoreRules::none(),
        1 << 20,
        &h.ws.scratch(),
        "cafe",
        &mut |p| {
            if p.ends_with("config.txt") && !done {
                done = true;
                std::fs::hard_link(&t2, &keep_alive).unwrap();
                std::fs::remove_file(&t2).unwrap();
                std::fs::write(&t2, "different file").unwrap();
            }
        },
    );
    assert!(matches!(r, Err(SnapshotError::Changed(_))), "{r:?}");
}

#[test]
fn the_snapshot_respects_the_context_limit() {
    use harpoon_deploy_docker::ignore::IgnoreRules;
    use harpoon_deploy_docker::snapshot::{self, SnapshotError};
    let h = Harness::new();
    std::fs::write(h.ws.context().join("big.bin"), vec![1u8; 20_000]).unwrap();
    std::fs::create_dir_all(h.ws.scratch()).unwrap();
    let src = confined(&h);
    let r = snapshot::create(&src, &IgnoreRules::none(), 10_000, &h.ws.scratch(), "aa");
    assert!(matches!(r, Err(SnapshotError::TooLarge(10_000))), "{r:?}");
    assert!(!h.ws.scratch().join("ctx-aa").exists());
}

#[test]
fn the_build_runs_from_the_snapshot_so_a_later_change_to_the_folder_cannot_reach_it() {
    let mut h = Harness::new();
    h.seed_built_image();
    std::fs::write(h.ws.context().join("data.txt"), "version one").unwrap();
    // while the (fake) build runs, the user changes the folder
    let ctx = h.ws.context();
    h.ws.install_buildx(&format!(
        "cp data.txt \"{d}/seen-by-build.txt\"\necho 'version two' > \"{c}/data.txt\"",
        d = h.ws.resources().join("harpoon/bin").display(),
        c = ctx.display()
    ));
    let mut b = backend_with(&h, ok200(), |_| {});
    let r = run_ok(&mut h, &mut b);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    assert_eq!(
        h.ws.buildx_record("seen-by-build.txt").trim(),
        "version one"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.join("data.txt"))
            .unwrap()
            .trim(),
        "version two"
    );
}

#[test]
fn a_dockerfile_changed_after_the_preflight_is_not_built() {
    let mut h = Harness::new();
    h.seed_built_image();
    let df = h.ws.context().join("Dockerfile");
    // between the preflight (which checked the base image) and the build, the Dockerfile changes
    h.docker.fault(
        Some("GET"),
        "/images/harpoon/",
        1,
        Action::Race(Arc::new(move |_| {
            std::fs::write(&df, "FROM nowhere:1\nCMD [\"true\"]\n").unwrap();
        })),
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    let (step, msg) = failure(&h);
    assert_eq!(step, Step::Build);
    assert!(msg.contains("changed after the preflight"), "{msg}");
    assert!(h.ws.buildx_record("args.txt").is_empty());
}

// ------------------------------------------------------------------ journal / foreign-resource regressions

#[test]
fn a_journal_that_fails_after_the_image_intent_leaves_no_build_and_no_container() {
    // find the write that records the image intent and fail exactly there
    let mut probe = Harness::new();
    probe.seed_built_image();
    let mut pj = StampedJournal::new();
    let mut pb = backend_with(&probe, ok200(), |_| {});
    probe.run(&mut pb, &mut pj);
    let n = pj
        .stamps
        .iter()
        .position(|(_, r)| matches!(r, LedgerRecord::Intent { key, .. } if key.kind == ResourceKind::Image))
        .unwrap();
    let total_before = pj.stamps.len();
    assert!(n < total_before);
    // crash index counts snapshots and ledger writes together; sweep to find the intent's index
    for crash in 0..total_before + 3 {
        let mut h = Harness::new();
        h.seed_built_image();
        let mut j = StampedJournal::crash_at(crash);
        let mut b = backend_with(&h, ok200(), |_| {});
        let res = h.try_run(
            &mut b,
            &mut j,
            &harpoon_deploy::engine::RunConfig::default(),
        );
        let intent_durable = j.stamps.iter().any(|(_, r)| matches!(r, LedgerRecord::Intent { key, .. } if key.kind == ResourceKind::Image));
        let build_ran = !h.ws.buildx_record("args.txt").is_empty();
        if res.is_err() {
            assert!(matches!(
                res,
                Err(harpoon_deploy::engine::EngineError::Journal(
                    JournalError::Crash
                ))
            ));
            assert!(
                !build_ran || intent_durable,
                "crash@{crash}: Buildx ran without a durable image intent"
            );
        }
        assert!(
            h.docker.with(|s| s.images.iter().all(|i| i
                .tags
                .iter()
                .all(|t| !t.starts_with("harpoon/"))
                || intent_durable)),
            "crash@{crash}: an image exists with no durable intent"
        );
    }
}

#[test]
fn foreign_images_and_containers_survive_every_new_path() {
    let mut h = Harness::new();
    h.seed_built_image();
    let foreign_c = h
        .docker
        .with(|s| s.add_container("bystander", vec![], true));
    let foreign_i = h.docker.with(|s| {
        s.add_image_with_id(&fake_iid(900), &["redis:7"], vec![], vec![]);
        fake_iid(900)
    });
    let mut b = backend_with(&h, FakeApp::new(&h, App::NoMarker), attributed);
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    for q in h.docker.requests().iter().filter(|q| q.method != "GET") {
        assert!(
            !q.path.contains(&foreign_c)
                && !q.path.contains(&foreign_i)
                && !q.path.contains("bystander")
                && !q.path.contains("redis"),
            "{} {}",
            q.method,
            q.path
        );
    }
    assert!(h.docker.with(
        |s| s.containers.iter().any(|c| c.id == foreign_c && c.running)
            && s.images.iter().any(|i| i.id == foreign_i)
    ));
}

#[test]
fn a_dockerfile_edited_while_the_context_is_copied_is_not_built_even_if_its_base_image_is_unchanged(
) {
    let mut h = Harness::new();
    h.seed_built_image();
    let df = h.ws.context().join("Dockerfile");
    let swap = df.clone();
    let mut done = false;
    let mut b = backend_with(&h, ok200(), |_| {}).with_snapshot_hook(move |p| {
        if p.ends_with("Dockerfile") && !done {
            done = true;
            // same FROM, new instructions — the preflight's image check alone would not notice
            std::fs::write(
                &swap,
                "FROM alpine:3.22\nRUN echo injected\nCMD [\"true\"]\n",
            )
            .unwrap();
        }
    });
    assert_eq!(run_ok(&mut h, &mut b).status, DeploymentStatus::Failed);
    let (step, msg) = failure(&h);
    assert_eq!(step, Step::Build);
    assert!(msg.contains("changed after the preflight"), "{msg}");
    assert!(h.ws.buildx_record("args.txt").is_empty());
    let left: Vec<_> = std::fs::read_dir(h.ws.scratch())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(left.iter().all(|n| !n.starts_with("ctx-")), "{left:?}");
}
