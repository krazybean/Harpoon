//! Marker-free health attribution (offline). An ordinary application that returns HTTP 200 on its health path but does
//! not echo the deployment marker must still deploy, while the safety invariant holds: a response counts only when it is
//! attributable to the exact container Harpoon created, either by the container's own marker (`marker-verified`) or by a
//! proof of the whole host-to-container path (`path-verified`). Everything else fails closed.

mod support;

use harpoon_deploy::health::{Observation, ATTRIBUTION_MARKER, ATTRIBUTION_PATH};
use harpoon_deploy::state::*;
use harpoon_deploy_docker::attribution::{PathProof, Route};
use harpoon_deploy_docker::backend::{Attribution, DockerBackend, DockerConfig};
use harpoon_deploy_docker::json::Json;
use harpoon_deploy_docker::probe::{HttpTransport, MarkedResponse, ResponseMarker};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::fake_docker::St;
use support::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// An ordinary application: 200, no marker header.
    Plain,
    /// A marker-aware application echoing its own container's marker.
    Honest,
    /// Carries a marker header with another value (a stale or foreign marker-aware listener).
    WrongMarker,
}

type Hook = Box<dyn Fn(&mut St) + Send>;

struct App {
    st: Arc<Mutex<St>>,
    mode: Mode,
    /// Run once, on the first response (a container restarting, a mapping moving, a port published another way).
    hook: Mutex<Option<Hook>>,
}

impl App {
    fn new(h: &Harness, mode: Mode) -> App {
        App {
            st: h.docker.st.clone(),
            mode,
            hook: Mutex::new(None),
        }
    }
    fn marker_on(&self, port: u16) -> Option<String> {
        let s = self.st.lock().unwrap();
        s.containers
            .iter()
            .find(|c| c.host_port == Some(port))
            .and_then(|c| {
                c.create_body
                    .get("Env")
                    .and_then(Json::as_array)
                    .and_then(|a| {
                        a.iter().filter_map(|e| e.as_str()).find_map(|e| {
                            e.strip_prefix("HARPOON_DEPLOYMENT_MARKER=")
                                .map(str::to_string)
                        })
                    })
            })
    }
    fn fire_hook(&self) {
        if let Some(f) = self.hook.lock().unwrap().take() {
            f(&mut self.st.lock().unwrap());
        }
    }
}

impl HttpTransport for App {
    fn get(&self, _p: u16, _path: &str, _t: Duration) -> Observation {
        Observation::Status(200)
    }
    /// The strict path (no path proof configured): only a response carrying the container's own marker counts.
    fn get_expecting(
        &self,
        port: u16,
        _path: &str,
        _t: Duration,
        marker: Option<&ResponseMarker>,
    ) -> Observation {
        self.fire_hook();
        match (marker, self.mode) {
            (None, _) => Observation::Status(200),
            (Some(ResponseMarker::Header { value, .. }), Mode::Honest)
                if self.marker_on(port).as_deref() == Some(value) =>
            {
                Observation::Status(200)
            }
            _ => Observation::ConnectFailed(
                "response did not carry the expected deployment marker".into(),
            ),
        }
    }
    fn get_marked(
        &self,
        port: u16,
        _path: &str,
        _t: Duration,
        marker: &ResponseMarker,
    ) -> MarkedResponse {
        self.fire_hook();
        match self.mode {
            Mode::Plain => MarkedResponse::Unmarked(200),
            Mode::Honest => match marker {
                ResponseMarker::Header { value, .. }
                    if self.marker_on(port).as_deref() == Some(value) =>
                {
                    MarkedResponse::Matched(200)
                }
                _ => MarkedResponse::Failed(Observation::ConnectFailed("stale marker".into())),
            },
            Mode::WrongMarker => {
                MarkedResponse::Failed(Observation::ConnectFailed("wrong marker".into()))
            }
        }
    }
    fn is_simulated(&self) -> bool {
        true
    }
}

#[derive(Clone)]
struct Proof {
    calls: Arc<Mutex<Vec<(u16, Route)>>>,
    verdict: Arc<Mutex<Result<(), String>>>,
}

impl Proof {
    fn ok() -> Proof {
        Proof {
            calls: Arc::new(Mutex::new(vec![])),
            verdict: Arc::new(Mutex::new(Ok(()))),
        }
    }
    fn denying(why: &str) -> Proof {
        Proof {
            calls: Arc::new(Mutex::new(vec![])),
            verdict: Arc::new(Mutex::new(Err(why.into()))),
        }
    }
    fn calls(&self) -> Vec<(u16, Route)> {
        self.calls.lock().unwrap().clone()
    }
}

impl PathProof for Proof {
    fn prove(&self, port: u16, route: Route) -> Result<(), String> {
        self.calls.lock().unwrap().push((port, route));
        self.verdict.lock().unwrap().clone()
    }
}

fn backend(h: &Harness, app: App, proof: Option<Proof>) -> DockerBackend<App> {
    let mut cfg: DockerConfig = config(&h.ws, &h.docker);
    cfg.attribution = Attribution::PerContainer;
    let b = DockerBackend::with_transport(cfg, Some(&h.ws.resources()), app)
        .expect("bundled buildx resolves")
        .with_entropy(test_entropy())
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 << 30));
    match proof {
        Some(p) => b.with_path_proof(p),
        None => b,
    }
}

fn deploy(
    mode: Mode,
    proof: Option<Proof>,
    hook: Option<Hook>,
) -> (Harness, harpoon_deploy::engine::Report) {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut app = App::new(&h, mode);
    if let Some(f) = hook {
        *app.hook.lock().unwrap() = Some(f);
    }
    let _ = &mut app;
    let mut b = backend(&h, app, proof);
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    (h, r)
}

fn labels_of(r: &harpoon_deploy::engine::Report) -> Vec<Option<&'static str>> {
    r.evidence.iter().map(|(_, e)| e.attribution()).collect()
}

// ------------------------------------------------------------------ an ordinary application deploys

#[test]
fn a_plain_http_200_application_deploys_when_the_whole_path_is_proven() {
    let proof = Proof::ok();
    let (h, r) = deploy(Mode::Plain, Some(proof.clone()), None);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    // both the preliminary and the final check are labelled path-verified, never marker-verified
    assert_eq!(
        labels_of(&r),
        vec![Some(ATTRIBUTION_PATH), Some(ATTRIBUTION_PATH)]
    );
    // every accepted check was proven for an ordinary guest-address route, before and again after its responses
    let calls = proof.calls();
    assert!(
        calls.len() >= 4 && calls.iter().all(|(_, route)| *route == Route::GuestAddress),
        "{calls:?}"
    );
    // two consecutive successes are still required
    assert!(r
        .evidence
        .iter()
        .all(|(_, e)| e.consecutive_required >= 2 && e.consecutive_reached >= 2));
}

#[test]
fn a_marker_aware_application_stays_marker_verified_and_never_needs_the_path_proof() {
    let proof = Proof::denying("must not be consulted");
    let (h, r) = deploy(Mode::Honest, Some(proof.clone()), None);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    assert_eq!(
        labels_of(&r),
        vec![Some(ATTRIBUTION_MARKER), Some(ATTRIBUTION_MARKER)]
    );
    assert!(
        proof.calls().is_empty(),
        "the marker is the preferred proof: {:?}",
        proof.calls()
    );
}

/// The durable notes of both checks (what the journal and the desktop show).
fn check_notes(h: &Harness) -> Vec<String> {
    [Step::PreliminaryCheck, Step::FinalCheck]
        .iter()
        .map(|st| h.dep.step(*st).note.clone())
        .collect()
}

#[test]
fn the_attribution_survives_into_the_persisted_step_notes() {
    for (mode, proof, label) in [
        (Mode::Plain, Some(Proof::ok()), ATTRIBUTION_PATH),
        (
            Mode::Honest,
            Some(Proof::denying("unused")),
            ATTRIBUTION_MARKER,
        ),
    ] {
        let (h, r) = deploy(mode, proof, None);
        assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
        let notes = check_notes(&h);
        assert_eq!(
            notes.len(),
            2,
            "the preliminary and the final check each leave a note"
        );
        for note in notes {
            assert!(
                note.starts_with("health evidence")
                    && note.ends_with(&format!("; attribution: {label}")),
                "{note}"
            );
            // only the fixed label: no marker, challenge or address
            assert!(
                !note.contains("HARPOON_DEPLOYMENT_MARKER") && !note.contains("192.168."),
                "{note}"
            );
        }
    }
}

#[test]
fn a_refused_check_leaves_no_verified_label_in_its_note_or_failure() {
    let (h, r) = deploy(Mode::WrongMarker, Some(Proof::ok()), None);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let note = &h.dep.step(Step::PreliminaryCheck).note;
    assert!(!note.contains("verified"), "{note}");
    assert!(!h.dep.failure.as_ref().unwrap().message.contains("verified"));
    assert!(r.evidence.iter().all(|(_, e)| e.attribution().is_none()));
}

#[test]
fn without_a_path_proof_only_the_marker_counts_as_before() {
    let (h, r) = deploy(Mode::Plain, None, None);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(h.dep.failure.as_ref().unwrap().step, Step::PreliminaryCheck);
    let (h, r) = deploy(Mode::Honest, None, None);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    assert_eq!(
        labels_of(&r),
        vec![Some(ATTRIBUTION_MARKER), Some(ATTRIBUTION_MARKER)]
    );
}

// ------------------------------------------------------------------ everything fails closed

#[test]
fn a_wrong_or_stale_marker_is_refused_even_though_a_path_proof_exists() {
    let proof = Proof::ok();
    let (h, r) = deploy(Mode::WrongMarker, Some(proof.clone()), None);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(h.dep.failure.as_ref().unwrap().step, Step::PreliminaryCheck);
    assert!(
        proof.calls().is_empty(),
        "a marker that is present and wrong is not rescued by a path proof"
    );
}

#[test]
fn a_path_that_cannot_be_proven_fails_the_check_with_the_reason_and_switches_nothing() {
    for why in [
        "the machine at 192.168.64.3 did not answer the identity challenge as the Harpoon VM does, so it is not that VM",
        "the Harpoon engine did not publish a guest address it discovered",
        "host listener: pid 99 (nc) listens on 127.0.0.1:39000, but the Harpoon forwarder is pid 4242 (harpoon)",
        "the Docker socket /tmp/x/d.sock is not owned by the forwarder process 4242, so they are not the same engine",
    ] {
        let (h, r) = deploy(Mode::Plain, Some(Proof::denying(why)), None);
        assert_eq!(r.status, DeploymentStatus::Failed, "{why}");
        let f = h.dep.failure.as_ref().unwrap();
        assert_eq!(f.step, Step::PreliminaryCheck, "{why}");
        assert!(f.message.contains(why), "the reason reaches the person: {}", f.message);
        assert_eq!(h.dep.cutover, CutoverState::NotStarted, "nothing was switched");
    }
}

#[test]
fn a_failure_message_does_not_repeat_the_evidence() {
    let (h, _) = deploy(Mode::Plain, Some(Proof::denying("wrong VM")), None);
    let m = h.dep.failure.unwrap().message;
    assert_eq!(m.matches("attempts,").count(), 1, "{m}");
}

#[test]
fn a_container_that_restarts_during_the_check_is_not_attributed() {
    let (h, r) = deploy(
        Mode::Plain,
        Some(Proof::ok()),
        Some(Box::new(|s: &mut St| {
            for c in s.containers.iter_mut().filter(|c| c.running) {
                c.restart_count += 1;
                c.started_at = "2026-11-04T00:00:05Z".into();
            }
        })),
    );
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = h.dep.failure.unwrap();
    assert!(
        f.message.contains("restarted while it was being checked"),
        "{}",
        f.message
    );
}

#[test]
fn a_container_replaced_by_another_image_is_not_attributed() {
    let (h, r) = deploy(
        Mode::Plain,
        Some(Proof::ok()),
        Some(Box::new(|s: &mut St| {
            for c in s.containers.iter_mut().filter(|c| c.running) {
                c.image = "sha256:somethingelse".into();
            }
        })),
    );
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h
        .dep
        .failure
        .unwrap()
        .message
        .contains("image changed while it was being checked"));
}

#[test]
fn a_mapping_that_moves_to_another_port_or_gains_one_is_not_attributed() {
    let (h, r) = deploy(
        Mode::Plain,
        Some(Proof::ok()),
        Some(Box::new(|s: &mut St| {
            for c in s.containers.iter_mut().filter(|c| c.running) {
                c.host_port = Some(41999);
            }
        })),
    );
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h.dep.failure.unwrap().message.contains("not the leased"));
}

#[test]
fn a_port_published_on_the_guest_loopback_is_proven_through_the_relay_route() {
    let proof = Proof::ok();
    let (h, r) = deploy(
        Mode::Plain,
        Some(proof.clone()),
        Some(Box::new(|s: &mut St| {
            for c in s.containers.iter_mut().filter(|c| c.running) {
                let p = c.host_port.unwrap();
                c.ports_override = Some(Json::Obj(vec![(
                    "8080/tcp".to_string(),
                    Json::Arr(vec![Json::Obj(vec![
                        ("HostIp".into(), Json::Str("127.0.0.1".into())),
                        ("HostPort".into(), Json::Str(p.to_string())),
                    ])]),
                )]));
            }
        })),
    );
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    assert!(
        proof
            .calls()
            .iter()
            .any(|(_, route)| *route == Route::GuestLoopbackRelay),
        "{:?}",
        proof.calls()
    );
}

#[test]
fn a_port_published_both_ways_has_no_single_path_and_is_refused() {
    let (h, r) = deploy(
        Mode::Plain,
        Some(Proof::ok()),
        Some(Box::new(|s: &mut St| {
            for c in s.containers.iter_mut().filter(|c| c.running) {
                let p = c.host_port.unwrap().to_string();
                c.ports_override = Some(Json::Obj(vec![(
                    "8080/tcp".to_string(),
                    Json::Arr(vec![
                        Json::Obj(vec![
                            ("HostIp".into(), Json::Str("0.0.0.0".into())),
                            ("HostPort".into(), Json::Str(p.clone())),
                        ]),
                        Json::Obj(vec![
                            ("HostIp".into(), Json::Str("127.0.0.1".into())),
                            ("HostPort".into(), Json::Str(p)),
                        ]),
                    ]),
                )]));
            }
        })),
    );
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h.dep.failure.unwrap().message.contains("single route"));
}

/// `TcpHttp` over a real socket (the fake transports above replace it): a matching header, an absent one and a
/// different one are three different answers, and only the absent one can ever be accepted through the path proof.
#[test]
fn the_real_http_transport_tells_matching_absent_and_different_markers_apart() {
    use std::io::{Read, Write};
    fn serve(header: Option<&'static str>) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = c.read(&mut buf);
                let h = header.map(|h| format!("{h}\r\n")).unwrap_or_default();
                let _ = write!(
                    c,
                    "HTTP/1.1 200 OK\r\n{h}Content-Length: 2\r\nConnection: close\r\n\r\nok"
                );
            }
        });
        port
    }
    let marker = ResponseMarker::Header {
        name: "X-Harpoon-Deployment-Marker".into(),
        value: "abc123".into(),
    };
    let t = Duration::from_secs(2);
    let ask = |h| harpoon_deploy_docker::probe::TcpHttp.get_marked(serve(h), "/", t, &marker);
    assert!(matches!(
        ask(Some("X-Harpoon-Deployment-Marker: abc123")),
        MarkedResponse::Matched(200)
    ));
    assert!(matches!(ask(None), MarkedResponse::Unmarked(200)));
    assert!(matches!(
        ask(Some("X-Harpoon-Deployment-Marker: stale")),
        MarkedResponse::Failed(_)
    ));
}
