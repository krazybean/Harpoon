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

// ---------------------------------------------------------------- B13: preliminary failure correctness

/// Everything a failed preliminary check must leave true, whatever made it fail.
fn preliminary_failure_invariants(
    h: &Harness,
    r: &harpoon_deploy::engine::Report,
    cur_id: &str,
    cur_name: &str,
    other: &str,
) {
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(h.dep.failure.as_ref().unwrap().step, Step::PreliminaryCheck);
    assert_eq!(
        h.dep.failure.as_ref().unwrap().kind,
        FailureKind::PreliminaryCheck
    );
    assert_eq!(h.dep.cutover, CutoverState::NotStarted, "no cutover begins");
    assert!(h.dep.interruption.is_none());
    assert!(
        h.dep.recovery.is_none(),
        "no restoration: nothing was stopped"
    );
    assert_eq!(r.pause, None, "a preliminary failure never pauses triggers");
    assert!(!h.dep.application_may_be_down());
    assert!(
        harpoon_deploy::state::trigger_pauses(std::slice::from_ref(&h.dep)).is_empty(),
        "and the durable record derives no pause"
    );
    current_release_only_read(h, cur_id, cur_name);
    // no stop, restart, removal or force anywhere
    for q in h.docker.requests() {
        // only the candidate's own step-scoped container may be stopped; never the serving release
        assert!(
            !(q.path.contains("/stop") || q.path.contains("/restart") || q.path.contains("/kill"))
                || !q.path.contains(cur_id),
            "{} {}",
            q.method,
            q.path
        );
        assert!(
            !q.path.contains("force=true") && !q.path.contains("prune"),
            "{} {}",
            q.method,
            q.path
        );
        assert!(
            q.method == "GET" || !q.path.contains(other),
            "the other application was touched: {} {}",
            q.method,
            q.path
        );
    }
    assert!(h.docker.container(other).unwrap().running);
    // the failed candidate's image is kept, owned and recorded; its serving container never existed
    candidate_image_is_retained(h, r);
    assert!(h.docker.container(&serving_name(&h.dep.id)).is_none());
    assert!(
        h.docker.container(&preliminary_name(&h.dep.id)).is_none(),
        "the step-scoped container is removed"
    );
    assert!(
        r.cleanup_problems().is_empty(),
        "{:?}",
        r.cleanup_problems()
    );
    assert!(
        r.logs.iter().any(|(s, _)| *s == Step::PreliminaryCheck),
        "bounded logs are kept"
    );
    assert!(
        r.evidence
            .iter()
            .any(|(s, e)| *s == Step::PreliminaryCheck && !e.is_healthy()),
        "health evidence is reported"
    );
}

fn other_app(h: &Harness) -> String {
    let mut g = harpoon_deploy::ids::IdGenerator::new(
        harpoon_deploy::time::SystemClock,
        harpoon_deploy::time::OsEntropy::new(),
    );
    let (a, d) = (g.application().unwrap(), g.deployment().unwrap());
    h.docker.with(|s| {
        s.add_container("other-app", labels(&a, &d), true);
    });
    "other-app".to_string()
}

#[test]
fn a_candidate_that_never_becomes_healthy_fails_the_preliminary_step_without_touching_anything_else(
) {
    for obs in [
        Observation::ConnectFailed("connection refused".into()),
        Observation::Status(503),
    ] {
        let mut h = Harness::new();
        h.seed_built_image();
        let (cur_id, prev) = h.seed_current_release();
        let cur_name = h.docker.with(|s| s.containers[0].name.clone());
        let other = other_app(&h);
        let mut b = backend(&h, Scripted::always(obs));
        let mut j = StampedJournal::new();
        let r = h.run_replacing(&mut b, &mut j, &prev, &|_| panic!("never asked"));
        preliminary_failure_invariants(&h, &r, &cur_id, &cur_name, &other);
    }
}

#[test]
fn a_candidate_that_exits_during_the_preliminary_check_fails_without_touching_the_current_release()
{
    let mut h = Harness::new();
    h.seed_built_image();
    let (cur_id, prev) = h.seed_current_release();
    let cur_name = h.docker.with(|s| s.containers[0].name.clone());
    let other = other_app(&h);
    let pre = preliminary_name(&h.dep.id);
    h.docker.fault(
        Some("GET"),
        "/json",
        1_000,
        support::fake_docker::Action::Race(std::sync::Arc::new(move |s| {
            for c in s.containers.iter_mut().filter(|c| c.name == pre) {
                if c.running {
                    c.running = false;
                    c.exit_code = 1;
                }
            }
        })),
    );
    let mut b = backend(&h, Scripted::always(Observation::Status(200)));
    let mut j = StampedJournal::new();
    let r = h.run_replacing(&mut b, &mut j, &prev, &|_| panic!("never asked"));
    preliminary_failure_invariants(&h, &r, &cur_id, &cur_name, &other);
    assert!(h
        .dep
        .failure
        .as_ref()
        .unwrap()
        .message
        .contains("exited with code 1"));
}

#[test]
fn a_failed_build_leaves_the_same_invariants_and_no_candidate_image() {
    let mut h = Harness::new();
    let (cur_id, prev) = h.seed_current_release();
    let cur_name = h.docker.with(|s| s.containers[0].name.clone());
    let other = other_app(&h);
    h.ws.install_buildx("echo 'ERROR: failed to solve' >&2\nexit 3");
    let mut b = backend(&h, Scripted::always(Observation::Status(200)));
    let mut j = StampedJournal::new();
    let r = h.run_replacing(&mut b, &mut j, &prev, &|_| panic!("never asked"));
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(h.dep.failure.as_ref().unwrap().kind, FailureKind::Build);
    assert_eq!(h.dep.cutover, CutoverState::NotStarted);
    assert_eq!(r.pause, None);
    assert!(h.dep.recovery.is_none());
    current_release_only_read(&h, &cur_id, &cur_name);
    assert!(h.docker.container(&other).unwrap().running);
    assert!(r
        .logs
        .iter()
        .any(|(s, c)| *s == Step::Build && c.text.contains("failed to solve")));
    assert!(h.docker.with(|s| s
        .images
        .iter()
        .all(|i| !i.tags.iter().any(|t| t.contains(&h.dep.id.to_string())))));
}

#[test]
fn a_docker_api_that_fails_while_the_candidate_is_prepared_changes_nothing_and_leaks_nothing() {
    use support::fake_docker::Action;
    // a transient outage at each point of the preparation: image inspect, container create, start
    for (method, path) in [
        ("GET", "/images/"),
        ("POST", "/containers/create"),
        ("POST", "/start"),
    ] {
        let mut h = Harness::new();
        h.seed_built_image();
        let (cur_id, prev) = h.seed_current_release();
        let cur_name = h.docker.with(|s| s.containers[0].name.clone());
        let other = other_app(&h);
        h.docker.fault(
            Some(method),
            path,
            1,
            Action::Respond(503, "{\"message\":\"temporarily unavailable\"}".into()),
        );
        let mut b = backend(&h, Scripted::always(Observation::Status(200)));
        let mut j = StampedJournal::new();
        let r = h.run_replacing(&mut b, &mut j, &prev, &|_| panic!("never asked"));
        assert_eq!(
            r.status,
            DeploymentStatus::Failed,
            "{method} {path}: {:?}",
            h.dep.failure
        );
        assert_eq!(h.dep.cutover, CutoverState::NotStarted, "{method} {path}");
        assert_eq!(r.pause, None);
        assert!(h.dep.recovery.is_none());
        current_release_only_read(&h, &cur_id, &cur_name);
        assert!(h.docker.container(&other).unwrap().running);
        assert!(
            r.cleanup_problems().is_empty(),
            "{method} {path}: {:?}",
            r.cleanup_problems()
        );
        assert!(
            h.docker.container(&preliminary_name(&h.dep.id)).is_none(),
            "{method} {path}: nothing leaked"
        );
        assert!(h.docker.container(&serving_name(&h.dep.id)).is_none());
    }
}
