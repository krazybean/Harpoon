//! Bounded cutover recovery through the Docker backend against a simulated Docker. Replacement is
//! enabled only inside these tests' configuration; the CLI never enables it. Nothing here contacts
//! a live endpoint.

mod support;

use harpoon_deploy::engine::{
    recover_cutover, EngineError, JournalError, MemoryJournal, RunConfig,
};
use harpoon_deploy::health::Observation;
use harpoon_deploy::ids::{DeploymentId, ReleaseId};
use harpoon_deploy::ledger::{Disposition, Ledger, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::release::ReleaseConfig;
use harpoon_deploy::state::*;
use harpoon_deploy_docker::backend::{DockerBackend, RESTORES_LABEL};
use harpoon_deploy_docker::probe::HttpTransport;
use std::cell::RefCell;
use std::time::Duration;
use support::fake_docker::Action;
use support::*;

const PREV_PORT: u16 = 38500;

/// Answers by port: test ports (the preliminary check) pass, the previous release's port answers as
/// scripted for the restoration, everything else (the failing candidate) answers 503.
struct ByPort {
    restored: Observation,
    seen: RefCell<Vec<u16>>,
}

impl HttpTransport for ByPort {
    fn get(&self, port: u16, _p: &str, _t: Duration) -> Observation {
        self.seen.borrow_mut().push(port);
        if port >= 39000 {
            Observation::Status(200)
        } else if port == PREV_PORT {
            self.restored.clone()
        } else {
            Observation::Status(503)
        }
    }
    fn is_simulated(&self) -> bool {
        true
    }
}

fn transport(restored: Observation) -> ByPort {
    ByPort {
        restored,
        seen: RefCell::new(Vec::new()),
    }
}

struct Rig {
    h: Harness,
    prev: DeploymentId,
    prev_image: String,
    prev_container: String,
    prev_name: String,
    cfg: RunConfig,
    bystanders: Vec<(String, bool)>,
}

fn rig() -> Rig {
    let mut h = Harness::new();
    h.seed_built_image();
    let prev = h.ids.gen.deployment().unwrap();
    let app = h.dep.application_id.clone();
    let l = labels(&app, &prev);
    let prev_image = fake_iid(7001);
    let prev_name = format!("hp-{prev}");
    let (container, bystanders) = h.docker.with(|s| {
        s.add_image_with_id(&prev_image, &["harpoon/previous:1"], l.clone(), vec![]);
        let c = s.add_container(&prev_name, l.clone(), true);
        s.containers.last_mut().unwrap().host_port = Some(PREV_PORT);
        // an unrelated application's container and an unlabelled one, both running
        let other_app = {
            let mut g = harpoon_deploy::ids::IdGenerator::new(
                harpoon_deploy::time::SystemClock,
                harpoon_deploy::time::OsEntropy::new(),
            );
            (g.application().unwrap(), g.deployment().unwrap())
        };
        s.add_container("other-app", labels(&other_app.0, &other_app.1), true);
        s.add_container("bystander", vec![], true);
        (
            c,
            vec![
                ("other-app".to_string(), true),
                ("bystander".to_string(), true),
            ],
        )
    });
    let mut ledger = Ledger::new();
    for r in [
        LedgerRecord::Intent {
            key: ResourceKey::new(ResourceKind::Container, &prev_name),
            app: app.clone(),
            deployment: prev.clone(),
            disposition: Disposition::RemoveOnFailure,
            at_ms: 1,
        },
        LedgerRecord::Receipt {
            key: ResourceKey::new(ResourceKind::Container, &prev_name),
            deployment: prev.clone(),
            id: container.clone(),
            at_ms: 2,
        },
        LedgerRecord::Intent {
            key: ResourceKey::new(ResourceKind::PortLease, PREV_PORT.to_string()),
            app: app.clone(),
            deployment: prev.clone(),
            disposition: Disposition::RemoveOnFailure,
            at_ms: 3,
        },
        LedgerRecord::Receipt {
            key: ResourceKey::new(ResourceKind::PortLease, PREV_PORT.to_string()),
            deployment: prev.clone(),
            id: PREV_PORT.to_string(),
            at_ms: 4,
        },
    ] {
        ledger.apply(&r).unwrap();
    }
    let cfg = RunConfig {
        prior_ledger: ledger,
        previous: Some(ReleaseConfig {
            image_id: prev_image.clone(),
            image_tag: "harpoon/previous:1".into(),
            container_port: 8080,
            host_port: PREV_PORT,
            health: {
                let mut x = harpoon_deploy::health::HealthSpec::first_test("/pinned-health");
                x.interval_ms = 5;
                x.startup_deadline_ms = 400;
                x
            },
            memory_bytes: 128 * 1024 * 1024,
            nano_cpus: 250_000_000,
            pids: 32,
        }),
        ..RunConfig::default()
    };
    Rig {
        h,
        prev,
        prev_image,
        prev_container: container,
        prev_name,
        cfg,
        bystanders,
    }
}

fn backend(r: &Rig, t: ByPort) -> DockerBackend<ByPort> {
    let mut cfg = config(&r.h.ws, &r.h.docker);
    cfg.enable_replacement = true;
    DockerBackend::with_transport(cfg, Some(&r.h.ws.resources()), t)
        .expect("bundled buildx resolves")
        .with_entropy(test_entropy())
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024))
}

fn release(r: &Rig) -> ReleaseId {
    ReleaseId::from_deployment(&r.prev)
}

fn run(r: &mut Rig, b: &mut DockerBackend<ByPort>) -> harpoon_deploy::engine::Report {
    let (rel, cfg) = (release(r), r.cfg.clone());
    r.h.run_replacing_with(b, &mut MemoryJournal::new(), &rel, &cfg)
        .expect("the engine itself did not error")
}

/// The previous release's container was stopped and nothing else was done to it; no unrelated
/// object, volume or network was touched; nothing was forced, pruned or removed behind Docker's back.
fn only_the_previous_release_was_stopped(r: &Rig) {
    for q in r.h.docker.requests() {
        assert!(
            ![
                "prune",
                "/volumes",
                "/networks",
                "/exec",
                "/commit",
                "force=true"
            ]
            .iter()
            .any(|f| q.path.contains(f)),
            "{} {}",
            q.method,
            q.path
        );
        for (name, _) in &r.bystanders {
            let id = r.h.docker.container(name).unwrap().id;
            assert!(
                q.method == "GET" || !q.path.contains(&id),
                "an unrelated container was touched: {} {}",
                q.method,
                q.path
            );
        }
        if q.path.contains(&r.prev_container) {
            assert!(
                q.method == "GET" || (q.method == "POST" && q.path.contains("/stop")),
                "the previous release was changed beyond a stop: {} {}",
                q.method,
                q.path
            );
        }
    }
    for (name, running) in &r.bystanders {
        assert_eq!(
            r.h.docker.container(name).unwrap().running,
            *running,
            "{name}"
        );
    }
    assert!(
        r.h.docker
            .with(|s| s.images.iter().any(|i| i.id == r.prev_image)),
        "the previous image stays"
    );
}

#[test]
fn a_failed_cutover_restores_the_previous_release_from_its_pinned_configuration() {
    let mut r = rig();
    let mut b = backend(&r, transport(Observation::Status(200)));
    let rep = run(&mut r, &mut b);
    let d = &r.h.dep;
    assert_eq!(rep.status, DeploymentStatus::Failed);
    assert_eq!(d.failure.as_ref().unwrap().kind, FailureKind::FinalCheck);
    let rec = d.recovery.unwrap();
    assert_eq!(rec.outcome, RecoveryOutcome::Restored, "{:?}", rep.warnings);
    assert!(rec.finished_ms.unwrap() >= rec.started_ms);
    assert!(!d.application_may_be_down());
    assert_eq!(
        rep.pause,
        Some(harpoon_deploy::engine::PauseReason::FailedCutover)
    );

    // the previous release: stopped by Harpoon, otherwise exactly as it was
    assert!(!r.h.docker.container(&r.prev_name).unwrap().running);
    // the restored release: a new container from the pinned image id and configuration
    let name = format!("hp-{}-restored", d.id);
    let c = r.h.docker.container(&name).expect("restored container");
    assert!(c.running);
    assert_eq!(
        c.image, r.prev_image,
        "recreated from the exact pinned image id"
    );
    assert_eq!(c.host_port, Some(PREV_PORT));
    for (k, v) in labels(&d.application_id, &d.id) {
        assert!(
            c.labels.iter().any(|(a, b)| *a == k && *b == v),
            "ownership label {k}"
        );
    }
    assert!(c
        .labels
        .iter()
        .any(|(k, v)| k == RESTORES_LABEL && *v == r.prev.to_string()));
    let body = c.create_body.to_json();
    assert!(
        body.contains("134217728")
            && body.contains("250000000")
            && body.contains("\"PidsLimit\":32"),
        "{body}"
    );
    assert!(
        body.contains(&PREV_PORT.to_string()) && !body.contains("HostIp"),
        "{body}"
    );
    // the failed candidate is kept for inspection, stopped, with its image
    let cand =
        r.h.docker
            .container(&format!("hp-{}", d.id))
            .expect("failed candidate kept");
    assert!(!cand.running);
    assert!(r.h.docker.with(|s| s
        .images
        .iter()
        .any(|i| i.tags.iter().any(|t| t.contains(&d.id.to_string())))));
    // the restoration's health check used the pinned path
    assert!(rep
        .logs
        .iter()
        .any(|(s, c)| *s == Step::Recovery && c.text.contains("restoration check")));
    only_the_previous_release_was_stopped(&r);
    assert!(
        rep.cleanup_problems().is_empty(),
        "{:?}",
        rep.cleanup_problems()
    );
}

#[test]
fn a_restored_release_that_never_passes_its_check_is_reported_unverified() {
    let mut r = rig();
    let mut b = backend(&r, transport(Observation::Status(503)));
    let rep = run(&mut r, &mut b);
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::RestoredUnverified
    );
    assert!(
        r.h.docker
            .container(&format!("hp-{}-restored", r.h.dep.id))
            .unwrap()
            .running
    );
    assert!(
        !r.h.dep.application_may_be_down(),
        "it started; whether it is healthy is stated, not assumed"
    );
    only_the_previous_release_was_stopped(&r);
    let _ = rep;
}

#[test]
fn a_restoration_that_cannot_start_the_container_is_restore_failed_and_not_retried() {
    let mut r = rig();
    // starts: preliminary, candidate, then the restored container
    r.h.docker.fault_after(
        Some("POST"),
        "/start",
        2,
        1,
        Action::Respond(500, "{\"message\":\"boom\"}".into()),
    );
    let mut b = backend(&r, transport(Observation::Status(200)));
    let rep = run(&mut r, &mut b);
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::RestoreFailed(Reason::StartFailed)
    );
    assert!(r.h.dep.application_may_be_down());
    let starts =
        r.h.docker
            .requests()
            .iter()
            .filter(|q| q.method == "POST" && q.path.contains("/start"))
            .count();
    assert_eq!(starts, 3, "one attempt, no retry");
    only_the_previous_release_was_stopped(&r);
    let _ = rep;
}

#[test]
fn a_missing_previous_image_or_unreachable_docker_is_not_attempted() {
    // the pinned image is gone
    let mut r = rig();
    let img = r.prev_image.clone();
    r.h.docker.with(|s| s.images.retain(|i| i.id != img));
    let mut b = backend(&r, transport(Observation::Status(200)));
    run(&mut r, &mut b);
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::ImageMissing)
    );
    assert!(r
        .h
        .docker
        .container(&format!("hp-{}-restored", r.h.dep.id))
        .is_none());
    assert!(r.h.dep.application_may_be_down());

    // the image now carries somebody else's labels: not the previous release's image
    let mut r = rig();
    let img = r.prev_image.clone();
    r.h.docker.with(|s| {
        for i in s.images.iter_mut().filter(|i| i.id == img) {
            i.labels.clear();
        }
    });
    let mut b = backend(&r, transport(Observation::Status(200)));
    run(&mut r, &mut b);
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::ImageMissing)
    );

    // Docker stops answering when the restoration asks for the image
    let mut r = rig();
    let id = r.prev_image.clone();
    r.h.docker.fault(
        Some("GET"),
        &format!("/images/{id}"),
        100,
        Action::Disconnect,
    );
    let mut b = backend(&r, transport(Observation::Status(200)));
    run(&mut r, &mut b);
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::DockerUnreachable)
    );
    assert!(r.h.dep.application_may_be_down());
}

#[test]
fn a_missing_pinned_configuration_is_not_attempted_and_creates_nothing() {
    let mut r = rig();
    r.cfg.previous = None;
    let before = r.h.docker.requests().len();
    let mut b = backend(&r, transport(Observation::Status(200)));
    run(&mut r, &mut b);
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::ConfigUnavailable)
    );
    assert!(r
        .h
        .docker
        .container(&format!("hp-{}-restored", r.h.dep.id))
        .is_none());
    assert!(r.h.docker.requests().len() > before);
    only_the_previous_release_was_stopped(&r);
}

#[test]
fn the_restoration_deadline_is_enforced() {
    let mut r = rig();
    r.cfg.policy.restore_budget_ms = 1;
    // Docker answers the image check slowly enough to exhaust a 1 ms budget
    let id = r.prev_image.clone();
    r.h.docker.fault(
        Some("GET"),
        &format!("/images/{id}"),
        10,
        Action::Delay(Duration::from_millis(40)),
    );
    let mut b = backend(&r, transport(Observation::Status(200)));
    run(&mut r, &mut b);
    let out = r.h.dep.recovery.unwrap().outcome;
    assert!(
        matches!(
            out,
            RecoveryOutcome::RestoreFailed(Reason::Timeout) | RecoveryOutcome::RestoredUnverified
        ),
        "{out:?}"
    );
    assert!(
        r.h.docker
            .container(&format!("hp-{}-restored", r.h.dep.id))
            .is_none(),
        "nothing was created past the deadline"
    );
}

#[test]
fn only_the_previous_releases_own_container_can_be_stopped() {
    let mut r = rig();
    // the recorded container has been replaced by something with the same id but foreign labels
    let id = r.prev_container.clone();
    r.h.docker.with(|s| {
        for c in s.containers.iter_mut().filter(|c| c.id == id) {
            c.labels.clear();
        }
    });
    let mut b = backend(&r, transport(Observation::Status(200)));
    let rep = run(&mut r, &mut b);
    // the stop is refused, so the cutover fails, and nothing foreign was stopped
    assert_eq!(r.h.dep.failure.as_ref().unwrap().step, Step::Cutover);
    assert!(
        r.h.docker.container(&r.prev_name).unwrap().running,
        "the unrecognised container was not stopped"
    );
    let _ = rep;
}

#[test]
fn a_crash_at_every_write_never_duplicates_the_restoration_or_touches_the_previous_container() {
    for n in 0..80 {
        let mut r = rig();
        let mut b = backend(&r, transport(Observation::Status(200)));
        let (rel, cfg) = (release(&r), r.cfg.clone());
        let mut j = StampedJournal::crash_at(n);
        let res = r.h.run_replacing_with(&mut b, &mut j, &rel, &cfg);
        let name = format!("hp-{}-restored", r.h.dep.id);
        let restored =
            r.h.docker
                .with(|s| s.containers.iter().filter(|c| c.name == name).count());
        assert!(restored <= 1, "write {n}: one restoration at most");
        assert!(
            r.h.docker.container(&r.prev_name).is_some(),
            "write {n}: previous container never removed"
        );
        only_the_previous_release_was_stopped_quiet(&r);
        if res.is_ok() {
            break;
        }
        assert!(
            matches!(res, Err(EngineError::Journal(JournalError::Crash))),
            "write {n}"
        );
        // restart: whatever was in flight is interrupted and never resumed
        let Some(last) = j.inner.last() else { continue };
        let mut v = [last.clone()];
        harpoon_deploy::state::recover_on_start(&mut v, 9_000_000_000_000);
        let [d] = v;
        assert!(d.status.is_terminal(), "write {n}");
        if let Some(rc) = d.recovery {
            assert_ne!(rc.outcome, RecoveryOutcome::InProgress, "write {n}");
        }
    }
}

fn only_the_previous_release_was_stopped_quiet(r: &Rig) {
    for q in r.h.docker.requests() {
        if q.path.contains(&r.prev_container) {
            assert!(
                q.method == "GET" || q.path.contains("/stop"),
                "{} {}",
                q.method,
                q.path
            );
        }
    }
}

#[test]
fn explicit_recovery_of_an_interrupted_cutover_restores_once_through_the_docker_backend() {
    let mut r = rig();
    // the previous release was stopped and the process died before the new one was confirmed
    let (id, name) = (r.prev_container.clone(), r.prev_name.clone());
    // no build happens in an explicit recovery: drop the fixture's "image appears after the build" hook
    r.h.docker.with(|s| s.faults.clear());
    r.h.docker.with(|s| {
        s.containers
            .iter_mut()
            .filter(|c| c.id == id)
            .for_each(|c| c.running = false)
    });
    let mut d = r.h.dep.clone();
    d.transition(DeploymentStatus::Running, 10).unwrap();
    d.start_step(Step::Cutover, 11).unwrap();
    d.begin_cutover(12).unwrap();
    let mut v = [d];
    harpoon_deploy::state::recover_on_start(&mut v, 20);
    let [mut d] = v;
    r.h.dep = d.clone();
    let mut b = backend(&r, transport(Observation::Status(200)));
    let prev = harpoon_deploy::backend::PreviousRelease {
        release: release(&r),
        deployment: r.prev.clone(),
        config: None,
    };
    let mut j = MemoryJournal::new();
    let rep = recover_cutover(
        &mut d,
        prev.clone(),
        &mut b,
        &mut j,
        &harpoon_deploy::time::SystemClock,
        &r.cfg,
    )
    .unwrap();
    assert_eq!(d.status, DeploymentStatus::Interrupted);
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::Restored,
        "{:?}",
        rep.warnings
    );
    assert!(
        r.h.docker
            .container(&format!("hp-{}-restored", d.id))
            .unwrap()
            .running
    );
    assert!(
        !r.h.docker.container(&name).unwrap().running,
        "the old stopped container is left as it was"
    );
    // not twice
    let n = r.h.docker.requests().len();
    assert!(recover_cutover(
        &mut d,
        prev,
        &mut b,
        &mut j,
        &harpoon_deploy::time::SystemClock,
        &r.cfg
    )
    .is_err());
    assert_eq!(
        r.h.docker.requests().len(),
        n,
        "a refused second attempt does not even contact Docker"
    );
}
