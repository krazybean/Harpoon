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
            env: Vec::new(),
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
    d.replaces = Some(release(&r));
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
        container: None,
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

// ---------------------------------------------------------------- hardening after B11

fn non_get(r: &Rig) -> usize {
    r.h.docker
        .requests()
        .iter()
        .filter(|q| q.method != "GET")
        .count()
}

#[test]
fn the_candidates_pin_is_durable_before_its_container_exists() {
    let mut crashed = 0;
    for n in 0..80 {
        let mut r = rig();
        let mut b = backend(&r, transport(Observation::Status(200)));
        let (rel, cfg) = (release(&r), r.cfg.clone());
        let mut j = StampedJournal::crash_at(n);
        let res = r.h.run_replacing_with(&mut b, &mut j, &rel, &cfg);
        let candidate = format!("hp-{}", r.h.dep.id);
        if r.h.docker.container(&candidate).is_some() {
            let pinned = j.inner.last().and_then(|s| s.pinned.clone());
            let want = pinned.expect("a container exists, so the pin was durable first");
            assert_eq!(want.image_id.len(), 71, "write {n}: an exact image id");
            assert_ne!(want.host_port, PREV_PORT);
            let started = r.h.docker.container(&candidate).unwrap().host_port;
            assert_eq!(
                started,
                Some(want.host_port),
                "write {n}: the pin names the port actually published"
            );
        }
        if res.is_ok() {
            break;
        }
        crashed += 1;
    }
    assert!(crashed > 5);
}

#[test]
fn a_port_collision_moves_the_release_and_its_pin_follows() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker.with(|s| {
        s.taken_ports.insert(38000);
    });
    let mut b = h.backend(Scripted::always(Observation::Status(200)));
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    let pin = h.dep.pinned.clone().expect("pinned");
    assert_eq!(
        pin.host_port, 38001,
        "the pin names the port the release really serves on"
    );
    assert_eq!(
        j.inner.last().unwrap().pinned.as_ref().unwrap().host_port,
        38001
    );
}

#[test]
fn a_pin_that_disagrees_with_the_leased_port_is_not_trusted() {
    let mut r = rig();
    r.cfg.previous.as_mut().unwrap().host_port = PREV_PORT + 1;
    let mut b = backend(&r, transport(Observation::Status(200)));
    let rep = run(&mut r, &mut b);
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::ConfigUnavailable)
    );
    assert!(r
        .h
        .docker
        .container(&format!("hp-{}-restored", r.h.dep.id))
        .is_none());
    assert!(rep
        .logs
        .iter()
        .any(|(s, c)| *s == Step::Recovery && c.text.contains("cannot be trusted")));
    only_the_previous_release_was_stopped(&r);
}

#[test]
fn after_a_restoration_the_next_replacement_stops_the_restored_container_not_the_old_one() {
    let mut r = rig();
    let mut b = backend(&r, transport(Observation::Status(200)));
    let rep = run(&mut r, &mut b);
    assert_eq!(r.h.dep.recovery.unwrap().outcome, RecoveryOutcome::Restored);
    let failed = r.h.dep.clone();
    let restored_name = format!("hp-{}-restored", failed.id);
    let restored = r.h.docker.container(&restored_name).unwrap();

    // the records, as the next invocation would read them
    let mut earlier = Deployment::new(
        r.prev.clone(),
        failed.application_id.clone(),
        Trigger::Cli,
        1,
    );
    earlier.transition(DeploymentStatus::Queued, 2).unwrap();
    earlier.transition(DeploymentStatus::Running, 3).unwrap();
    earlier.release = Some(release(&r));
    earlier.transition(DeploymentStatus::Succeeded, 4).unwrap();
    let deps = [earlier, failed.clone()];
    let sv = harpoon_deploy::serving::serving_release(&rep.ledger, &deps, &failed.application_id)
        .unwrap();
    assert_eq!(sv.container_name, restored_name);
    assert_eq!(sv.container_owner, failed.id);

    // a new candidate replaces it, told where the release really serves
    r.h.dep = new_deployment(&mut r.h.ids);
    r.h.docker.with(|s| s.faults.clear());
    r.h.seed_built_image();
    let mut cfg = r.cfg.clone();
    cfg.prior_ledger = rep.ledger.clone();
    cfg.previous_container = Some(harpoon_deploy::backend::ContainerRef {
        owner: sv.container_owner.clone(),
        name: sv.container_name.clone(),
    });
    let mut b2 = {
        let mut c = config(&r.h.ws, &r.h.docker);
        c.enable_replacement = true;
        DockerBackend::with_transport(c, Some(&r.h.ws.resources()), transport_ok())
            .unwrap()
            .with_entropy(test_entropy())
            .with_port_probe(NoListeners)
            .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024))
    };
    let before = r.h.docker.requests().len();
    let rel = release(&r);
    let rep2 =
        r.h.run_replacing_with(&mut b2, &mut MemoryJournal::new(), &rel, &cfg)
            .expect("engine");
    assert_eq!(
        rep2.status,
        DeploymentStatus::Succeeded,
        "{:?}",
        r.h.dep.failure
    );
    let stops: Vec<String> = r.h.docker.requests()[before..]
        .iter()
        .filter(|q| q.method == "POST" && q.path.contains("/stop"))
        .map(|q| q.path.clone())
        .collect();
    // (the other stop is the new candidate's own preliminary container, removed before the cutover)
    assert_eq!(
        stops.iter().filter(|p| p.contains(&restored.id)).count(),
        1,
        "the restored container was stopped: {stops:?}"
    );
    assert!(
        stops.iter().all(|p| !p.contains(&r.prev_container)),
        "not the old stopped one: {stops:?}"
    );
    assert!(!r.h.docker.container(&restored_name).unwrap().running);
}

fn transport_ok() -> impl HttpTransport {
    Scripted::always(Observation::Status(200))
}

#[test]
fn a_running_container_on_the_port_blocks_the_automatic_restoration_and_is_not_touched() {
    let mut r = rig();
    // while Harpoon stops the old release, something else starts publishing its port
    r.h.docker.fault(
        Some("POST"),
        "/stop",
        1,
        Action::Race(std::sync::Arc::new(|s| {
            s.add_container("squatter", vec![], true);
            s.containers.last_mut().unwrap().host_port = Some(PREV_PORT);
        })),
    );
    let mut b = backend(&r, transport(Observation::Status(200)));
    let rep = run(&mut r, &mut b);
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::NotAttempted(Reason::PortInUse)
    );
    assert!(
        r.h.docker
            .container(&format!("hp-{}-restored", r.h.dep.id))
            .is_none(),
        "nothing was created"
    );
    assert!(
        r.h.docker.container("squatter").unwrap().running,
        "the squatter was not touched"
    );
    assert!(rep.warnings.iter().any(|m| m.contains("squatter")
        && m.contains("no Harpoon labels")
        && m.contains("free the port yourself")));
    assert!(r.h.dep.application_may_be_down());
    let squatter = r.h.docker.container("squatter").unwrap().id;
    assert!(r
        .h
        .docker
        .requests()
        .iter()
        .all(|q| q.method == "GET" || !q.path.contains(&squatter)));
}

#[test]
fn an_interrupted_cutover_with_the_candidate_still_running_is_reported_and_costs_no_attempt() {
    let r = rig();
    r.h.docker.with(|s| s.faults.clear());
    // the previous release was stopped, and the dead candidate is still running on its port
    let (id, app, dead) = (
        r.prev_container.clone(),
        r.h.dep.application_id.clone(),
        r.h.dep.id.clone(),
    );
    let other = {
        let mut g = harpoon_deploy::ids::IdGenerator::new(
            harpoon_deploy::time::SystemClock,
            harpoon_deploy::time::OsEntropy::new(),
        );
        (g.application().unwrap(), g.deployment().unwrap())
    };
    r.h.docker.with(|s| {
        s.containers
            .iter_mut()
            .filter(|c| c.id == id)
            .for_each(|c| c.running = false);
        for (name, l) in [
            ("dead-candidate", labels(&app, &dead)),
            ("another-apps", labels(&other.0, &other.1)),
            ("unlabelled", vec![]),
        ] {
            s.add_container(name, l, true);
            s.containers.last_mut().unwrap().host_port = Some(PREV_PORT);
        }
        s.add_container("stopped-holder", vec![], false);
        s.containers.last_mut().unwrap().host_port = Some(PREV_PORT);
    });
    let mut d = r.h.dep.clone();
    d.transition(DeploymentStatus::Running, 10).unwrap();
    d.start_step(Step::Cutover, 11).unwrap();
    d.replaces = Some(release(&r));
    d.begin_cutover(12).unwrap();
    let mut v = [d];
    harpoon_deploy::state::recover_on_start(&mut v, 20);
    let [mut d] = v;
    let mut b = backend(&r, transport(Observation::Status(200)));
    let prev = harpoon_deploy::backend::PreviousRelease {
        release: release(&r),
        deployment: r.prev.clone(),
        config: None,
        container: None,
    };
    let mut j = MemoryJournal::new();
    let n = non_get(&r);
    let rep = recover_cutover(
        &mut d,
        prev.clone(),
        &mut b,
        &mut j,
        &harpoon_deploy::time::SystemClock,
        &r.cfg,
    )
    .unwrap();

    // reported, with who owns each holder; the stopped one does not block
    assert_eq!(rep.blocked.len(), 3, "{:?}", rep.blocked);
    let text = rep.blocked.join("\n");
    assert!(
        text.contains("dead-candidate")
            && text.contains("this application's Harpoon labels")
            && text.contains("reconcile")
    );
    assert!(text.contains("another-apps") && text.contains("another application's Harpoon labels"));
    assert!(text.contains("unlabelled") && text.contains("no Harpoon labels"));
    assert!(!text.contains("stopped-holder"));
    // nothing recorded, nothing created, nothing stopped or removed
    assert_eq!(j.writes(), 0);
    assert!(d.can_recover_explicitly() && d.recovery.is_none());
    assert_eq!(non_get(&r), n);
    for name in ["dead-candidate", "another-apps", "unlabelled"] {
        assert!(
            r.h.docker.container(name).unwrap().running,
            "{name} untouched"
        );
    }

    // once the developer clears the port, the one attempt is made and works
    r.h.docker.with(|s| {
        s.containers
            .iter_mut()
            .filter(|c| c.host_port == Some(PREV_PORT))
            .for_each(|c| c.running = false)
    });
    let rep = recover_cutover(
        &mut d,
        prev,
        &mut b,
        &mut j,
        &harpoon_deploy::time::SystemClock,
        &r.cfg,
    )
    .unwrap();
    assert!(rep.blocked.is_empty());
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::Restored,
        "{:?}",
        rep.warnings
    );
    assert!(
        rep.logs
            .iter()
            .any(|(_, c)| c.text.contains("stopped container stopped-holder")),
        "the stopped holder was reported"
    );
}

// ---------------------------------------------------------------- non-secret runtime environment

/// `ByPort` that also accepts a marker-bearing probe (it answers by port, as a simulation does).
struct Marked(ByPort);
impl HttpTransport for Marked {
    fn get(&self, port: u16, p: &str, t: Duration) -> Observation {
        self.0.get(port, p, t)
    }
    fn get_expecting(
        &self,
        port: u16,
        p: &str,
        t: Duration,
        _marker: Option<&harpoon_deploy_docker::probe::ResponseMarker>,
    ) -> Observation {
        self.0.get(port, p, t)
    }
    fn is_simulated(&self) -> bool {
        true
    }
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The exact `Env` array of a create request, in order.
fn env_of(body: &str) -> Vec<String> {
    let j = harpoon_deploy_docker::json::Json::parse(body).unwrap();
    j.get("Env")
        .and_then(|e| e.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn marker_of(env: &[String]) -> String {
    env.iter()
        .find_map(|e| e.strip_prefix("HARPOON_DEPLOYMENT_MARKER="))
        .expect("every container carries its own marker")
        .to_string()
}

fn creates(r: &Rig) -> Vec<(String, Vec<String>)> {
    r.h.docker
        .requests()
        .iter()
        .filter(|q| q.method == "POST" && q.path.contains("/containers/create"))
        .map(|q| {
            let name = q.path.split("name=").nth(1).unwrap_or("").to_string();
            (name, env_of(&q.body))
        })
        .collect()
}

#[test]
fn a_restoration_uses_the_restored_releases_own_pinned_environment_never_the_new_runs() {
    let mut r = rig();
    // the serving (previous) release was pinned with its own variables; this run asks for different ones
    r.cfg.previous.as_mut().unwrap().env = pairs(&[("EMPTY", ""), ("PREV", "a=b=c")]);
    let mut cfg = config(&r.h.ws, &r.h.docker);
    cfg.enable_replacement = true;
    cfg.attribution = harpoon_deploy_docker::backend::Attribution::PerContainer;
    cfg.env = vec!["NEW=2".into(), "ZED=".into()];
    let mut b = DockerBackend::with_transport(
        cfg,
        Some(&r.h.ws.resources()),
        Marked(transport(Observation::Status(200))),
    )
    .unwrap()
    .with_entropy(test_entropy())
    .with_port_probe(NoListeners)
    .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024));
    let (rel, rcfg) = (release(&r), r.cfg.clone());
    let rep =
        r.h.run_replacing_with(&mut b, &mut MemoryJournal::new(), &rel, &rcfg)
            .expect("the engine itself did not error");
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::Restored,
        "{:?}",
        rep.warnings
    );

    let made = creates(&r);
    let by = |suffix: &str| {
        made.iter()
            .find(|(n, _)| n.ends_with(suffix))
            .unwrap_or_else(|| panic!("no create for {suffix}: {made:?}"))
            .1
            .clone()
    };
    let (pre, serving, restored) = (by("-pre"), by(&r.h.dep.id.to_string()), by("-restored"));
    let strip = |e: &[String]| {
        e.iter()
            .filter(|x| !x.starts_with("HARPOON_DEPLOYMENT_MARKER="))
            .cloned()
            .collect::<Vec<_>>()
    };
    // preliminary and serving candidate: this run's variables, exactly, plus their own marker
    assert_eq!(strip(&pre), ["NEW=2", "ZED="]);
    assert_eq!(strip(&serving), ["NEW=2", "ZED="]);
    // the restored release: its own pin, exactly, and none of this run's
    assert_eq!(strip(&restored), ["EMPTY=", "PREV=a=b=c"]);
    assert_eq!(restored.len(), 3);
    // three containers, three different markers
    let m = [marker_of(&pre), marker_of(&serving), marker_of(&restored)];
    assert!(m[0] != m[1] && m[1] != m[2] && m[0] != m[2], "{m:?}");
    // the candidate's pin carries this run's environment (it failed, so it is never a restore source)
    assert_eq!(
        r.h.dep.pinned.as_ref().unwrap().env,
        pairs(&[("NEW", "2"), ("ZED", "")])
    );
}

#[test]
fn a_run_without_variables_keeps_the_legacy_behaviour_and_pin() {
    let mut r = rig();
    let mut b = backend(&r, transport(Observation::Status(200)));
    let _ = run(&mut r, &mut b);
    for (name, env) in creates(&r) {
        assert!(env.is_empty(), "{name}: {env:?}");
    }
    let pin = r.h.dep.pinned.as_ref().unwrap();
    assert!(pin.env.is_empty() && pin.encode().starts_with("v1;"));
}

#[test]
fn a_reserved_or_malformed_variable_never_reaches_docker() {
    for bad in [
        "HARPOON_DEPLOYMENT_MARKER=x",
        "HARPOON_X=1",
        "BAD NAME=1",
        "A=1\nB=2",
    ] {
        let mut r = rig();
        let mut cfg = config(&r.h.ws, &r.h.docker);
        cfg.enable_replacement = true;
        cfg.env = vec![bad.into()];
        let mut b = DockerBackend::with_transport(
            cfg,
            Some(&r.h.ws.resources()),
            transport(Observation::Status(200)),
        )
        .unwrap()
        .with_entropy(test_entropy())
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024));
        let _ = run(&mut r, &mut b);
        assert!(creates(&r).is_empty(), "{bad:?}: a container was created");
        assert!(
            r.h.docker.container(&r.prev_name).unwrap().running,
            "{bad:?}: the serving release must be untouched"
        );
        assert!(r.h.dep.failure.is_some(), "{bad:?}");
    }
}
