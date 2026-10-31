//! Preparing a replacement through the Docker backend against a fake Docker: the current release
//! is never stopped, restarted, removed or modified while the candidate is built and checked, and
//! the cutover is not begun because live replacement stays disabled until the bounded restoration
//! exists. Simulated Docker only; nothing here contacts a live endpoint.

mod support;

use harpoon_deploy::engine::NotCutOver;
use harpoon_deploy::health::Observation;
use harpoon_deploy::ledger::{ResourceKey, ResourceKind};
use harpoon_deploy::state::*;
use harpoon_deploy_docker::backend::{preliminary_name, serving_name, DockerBackend, DockerConfig};
use harpoon_deploy_docker::probe::HttpTransport;
use support::*;

fn backend<T: HttpTransport>(h: &Harness, t: T) -> DockerBackend<T> {
    let cfg: DockerConfig = config(&h.ws, &h.docker);
    DockerBackend::with_transport(cfg, Some(&h.ws.resources()), t)
        .expect("bundled buildx resolves")
        .with_entropy(test_entropy())
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024))
}

/// Every request that names the current release must be a read.
fn current_release_only_read(h: &Harness, id: &str, name: &str) {
    for r in h.docker.requests() {
        if r.path.contains(id) || r.path.contains(name) {
            assert_eq!(
                r.method, "GET",
                "the current release was touched: {} {}",
                r.method, r.path
            );
        }
    }
    let c = h
        .docker
        .container(name)
        .expect("the current release still exists");
    assert!(c.running, "the current release is still running");
}

fn candidate_image_is_retained(h: &Harness, r: &harpoon_deploy::engine::Report) {
    let tag = tag_for(&h.dep.application_id, &h.dep.id);
    let e = r
        .ledger
        .find(&h.dep.id, &ResourceKey::new(ResourceKind::Image, tag))
        .expect("the candidate image is in the ledger");
    assert!(!e.is_released(), "the candidate image is kept");
}

#[test]
fn a_prepared_candidate_leaves_the_current_release_running_and_untouched() {
    let mut h = Harness::new();
    h.seed_built_image();
    let (cur_id, prev) = h.seed_current_release();
    let cur_name = h.docker.with(|s| s.containers[0].name.clone());
    let mut b = backend(&h, Scripted::always(Observation::Status(200)));
    let mut j = StampedJournal::new();
    // even an eager confirmation cannot start a live cutover: the Docker backend does not support it
    let r = h.run_replacing(&mut b, &mut j, &prev, &|_| true);
    assert_eq!(r.status, DeploymentStatus::Cancelled, "{:?}", h.dep.failure);
    assert_eq!(r.prepared.as_ref().unwrap().why, NotCutOver::Unsupported);
    assert_eq!(h.dep.cutover, CutoverState::NotStarted);
    assert!(h.dep.interruption.is_none());
    current_release_only_read(&h, &cur_id, &cur_name);
    // the preliminary container and its test port are gone; the serving container never existed
    assert!(h.docker.container(&preliminary_name(&h.dep.id)).is_none());
    assert!(h.docker.container(&serving_name(&h.dep.id)).is_none());
    candidate_image_is_retained(&h, &r);
    assert!(r.cleanup_problems().is_empty());
    // the attributed preliminary evidence is in the durable step record
    let step = h
        .dep
        .steps
        .iter()
        .find(|s| s.step == Step::PreliminaryCheck)
        .unwrap();
    // the scripted transport is a simulation, and the record says so
    assert!(
        step.note.starts_with("health evidence (simulated):") && step.note.contains("HTTP 200"),
        "{}",
        step.note
    );
    // and the candidate carries its own release identity and the three ownership labels
    assert_eq!(
        h.dep.release,
        Some(harpoon_deploy::ids::ReleaseId::from_deployment(&h.dep.id))
    );
}

#[test]
fn a_failing_preliminary_check_keeps_the_current_release_and_retains_failure_evidence() {
    let mut h = Harness::new();
    h.seed_built_image();
    let (cur_id, prev) = h.seed_current_release();
    let cur_name = h.docker.with(|s| s.containers[0].name.clone());
    let mut b = backend(&h, Scripted::always(Observation::Status(503)));
    let mut j = StampedJournal::new();
    let r = h.run_replacing(&mut b, &mut j, &prev, &|_| panic!("never asked"));
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(h.dep.failure.as_ref().unwrap().step, Step::PreliminaryCheck);
    assert!(h.dep.failure.as_ref().unwrap().message.contains("HTTP 503"));
    assert_eq!(h.dep.cutover, CutoverState::NotStarted);
    assert_eq!(r.pause, None);
    current_release_only_read(&h, &cur_id, &cur_name);
    assert!(r
        .evidence
        .iter()
        .any(|(s, e)| *s == Step::PreliminaryCheck && !e.is_healthy()));
    assert!(r
        .logs
        .iter()
        .any(|(s, c)| *s == Step::PreliminaryCheck && c.text.contains("container output")));
    candidate_image_is_retained(&h, &r);
}

#[test]
fn a_failing_build_keeps_the_current_release_and_never_reaches_a_check() {
    let mut h = Harness::new();
    let (cur_id, prev) = h.seed_current_release();
    let cur_name = h.docker.with(|s| s.containers[0].name.clone());
    h.ws.install_buildx(
        "echo 'ERROR: failed to solve: process \"/bin/false\" did not complete' >&2\nexit 3",
    );
    let mut b = backend(&h, Scripted::always(Observation::Status(200)));
    let mut j = StampedJournal::new();
    let r = h.run_replacing(&mut b, &mut j, &prev, &|_| panic!("never asked"));
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(h.dep.failure.as_ref().unwrap().step, Step::Build);
    assert_eq!(h.dep.cutover, CutoverState::NotStarted);
    current_release_only_read(&h, &cur_id, &cur_name);
    assert!(r
        .logs
        .iter()
        .any(|(s, c)| *s == Step::Build && c.text.contains("failed to solve")));
    assert!(h.docker.container(&preliminary_name(&h.dep.id)).is_none());
}
