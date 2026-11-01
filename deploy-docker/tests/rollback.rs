//! Manual rollback through the Docker backend against a simulated Docker. Replacement (which a
//! rollback is) is enabled only inside these tests' configuration; the CLI never enables it.
//! Nothing here contacts a live endpoint.

mod support;

use harpoon_deploy::engine::{MemoryJournal, PauseReason, RunConfig};
use harpoon_deploy::health::Observation;
use harpoon_deploy::ids::{DeploymentId, ReleaseId};
use harpoon_deploy::ledger::{Disposition, Ledger, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::release::ReleaseConfig;
use harpoon_deploy::rollback::select_target;
use harpoon_deploy::serving::{serving_release, ServedBy};
use harpoon_deploy::state::*;
use harpoon_deploy_docker::backend::DockerBackend;
use harpoon_deploy_docker::probe::HttpTransport;
use std::time::Duration;
use support::*;

const SERVING_PORT: u16 = 38500;
const OLD_PORT: u16 = 38300;

/// Test ports (>= 39000) answer `pre`; the serving release's port answers `restored`; any other
/// application port (the rolled-back-to candidate) answers `candidate`.
struct ByPort {
    pre: Observation,
    candidate: Observation,
    restored: Observation,
}

impl HttpTransport for ByPort {
    fn get(&self, port: u16, _p: &str, _t: Duration) -> Observation {
        if port >= 39000 {
            self.pre.clone()
        } else if port == SERVING_PORT {
            self.restored.clone()
        } else {
            self.candidate.clone()
        }
    }
    fn is_simulated(&self) -> bool {
        true
    }
}

fn ok() -> Observation {
    Observation::Status(200)
}

struct Rig {
    h: Harness,
    old: DeploymentId,
    serving: DeploymentId,
    old_image: String,
    old_pin: ReleaseConfig,
    serving_pin: ReleaseConfig,
    serving_container: String,
    serving_name: String,
    deps: Vec<Deployment>,
    cfg: RunConfig,
    bystander: String,
}

fn pin(image: &str, tag: &str, port: u16, mem: i64) -> ReleaseConfig {
    ReleaseConfig {
        image_id: image.into(),
        image_tag: tag.into(),
        container_port: 8080,
        host_port: port,
        health: {
            let mut x = harpoon_deploy::health::HealthSpec::first_test("/pinned-health");
            x.interval_ms = 5;
            x.startup_deadline_ms = 400;
            x
        },
        memory_bytes: mem,
        nano_cpus: 250_000_000,
        pids: 32,
        env: Vec::new(),
    }
}

fn succeeded(
    app: &harpoon_deploy::ids::ApplicationId,
    id: &DeploymentId,
    at: u64,
    p: &ReleaseConfig,
) -> Deployment {
    let mut d = Deployment::new(id.clone(), app.clone(), Trigger::Manual, at);
    d.transition(DeploymentStatus::Queued, at + 1).unwrap();
    d.transition(DeploymentStatus::Running, at + 2).unwrap();
    d.release = Some(ReleaseId::from_deployment(id));
    d.pinned = Some(p.clone());
    d.transition(DeploymentStatus::Succeeded, at + 3).unwrap();
    d
}

fn rig() -> Rig {
    let mut h = Harness::new();
    let app = h.dep.application_id.clone();
    let old = h.ids.gen.deployment().unwrap();
    let serving = h.ids.gen.deployment().unwrap();
    let old_image = fake_iid(8001);
    let serving_image = fake_iid(8002);
    let serving_name = format!("hp-{serving}");
    let old_pin = pin(&old_image, "harpoon/old:1", OLD_PORT, 64 * 1024 * 1024);
    let serving_pin = pin(
        &serving_image,
        "harpoon/serving:1",
        SERVING_PORT,
        128 * 1024 * 1024,
    );
    let (serving_container, bystander) = h.docker.with(|s| {
        s.add_image_with_id(&old_image, &["harpoon/old:1"], labels(&app, &old), vec![]);
        s.add_image_with_id(
            &serving_image,
            &["harpoon/serving:1"],
            labels(&app, &serving),
            vec![],
        );
        // the old release's container is long stopped; the serving one runs on its port
        s.add_container(&format!("hp-{old}"), labels(&app, &old), false);
        let c = s.add_container(&serving_name, labels(&app, &serving), true);
        s.containers.last_mut().unwrap().host_port = Some(SERVING_PORT);
        let other = {
            let mut g = harpoon_deploy::ids::IdGenerator::new(
                harpoon_deploy::time::SystemClock,
                harpoon_deploy::time::OsEntropy::new(),
            );
            (g.application().unwrap(), g.deployment().unwrap())
        };
        s.add_container("other-app", labels(&other.0, &other.1), true);
        (c, "other-app".to_string())
    });
    let mut ledger = Ledger::new();
    let mut put = |dep: &DeploymentId, kind: ResourceKind, name: &str, id: &str, d: Disposition| {
        for r in [
            LedgerRecord::Intent {
                key: ResourceKey::new(kind, name),
                app: app.clone(),
                deployment: dep.clone(),
                disposition: d,
                at_ms: 1,
            },
            LedgerRecord::Receipt {
                key: ResourceKey::new(kind, name),
                deployment: dep.clone(),
                id: id.into(),
                at_ms: 2,
            },
        ] {
            ledger.apply(&r).unwrap();
        }
    };
    put(
        &old,
        ResourceKind::Image,
        "harpoon/old:1",
        &old_image,
        Disposition::Retain,
    );
    put(
        &serving,
        ResourceKind::Image,
        "harpoon/serving:1",
        &serving_image,
        Disposition::Retain,
    );
    put(
        &old,
        ResourceKind::Container,
        &format!("hp-{old}"),
        "old-cid",
        Disposition::RemoveOnFailure,
    );
    put(
        &serving,
        ResourceKind::Container,
        &serving_name,
        &serving_container,
        Disposition::RemoveOnFailure,
    );
    put(
        &serving,
        ResourceKind::PortLease,
        &SERVING_PORT.to_string(),
        &SERVING_PORT.to_string(),
        Disposition::RemoveOnFailure,
    );
    let deps = vec![
        succeeded(&app, &old, 100, &old_pin),
        succeeded(&app, &serving, 200, &serving_pin),
    ];
    h.dep.rollback_to = Some(ReleaseId::from_deployment(&old));
    let cfg = RunConfig {
        prior_ledger: ledger,
        previous: Some(serving_pin.clone()),
        ..RunConfig::default()
    };
    Rig {
        h,
        old,
        serving,
        old_image,
        old_pin,
        serving_pin,
        serving_container,
        serving_name,
        deps,
        cfg,
        bystander,
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
        .with_rollback(r.old.clone(), r.old_pin.clone())
}

fn run(r: &mut Rig, b: &mut DockerBackend<ByPort>) -> harpoon_deploy::engine::Report {
    let (rel, cfg) = (ReleaseId::from_deployment(&r.serving), r.cfg.clone());
    r.h.run_replacing_with(b, &mut MemoryJournal::new(), &rel, &cfg)
        .expect("the engine itself did not error")
}

fn nothing_unrelated_or_destructive(r: &Rig) {
    for q in r.h.docker.requests() {
        assert!(
            ![
                "prune",
                "/volumes",
                "/networks",
                "/exec",
                "/commit",
                "force=true",
                "/build"
            ]
            .iter()
            .any(|f| q.path.contains(f)),
            "{} {}",
            q.method,
            q.path
        );
        let other = r.h.docker.container(&r.bystander).unwrap().id;
        assert!(
            q.method == "GET" || !q.path.contains(&other),
            "{} {}",
            q.method,
            q.path
        );
        // nothing is ever removed from the retained releases: no image is created or deleted
        assert!(
            !(q.path.starts_with("/images/") && q.method != "GET"),
            "an image was changed: {} {}",
            q.method,
            q.path
        );
    }
    assert!(r.h.docker.container(&r.bystander).unwrap().running);
    assert!(r
        .h
        .docker
        .with(|s| s.images.iter().any(|i| i.id == r.old_image)));
}

fn serving_was_never_changed(r: &Rig) {
    for q in r.h.docker.requests() {
        if q.path.contains(&r.serving_container) {
            assert_eq!(
                q.method, "GET",
                "the serving release was touched: {} {}",
                q.method, q.path
            );
        }
    }
    assert!(r.h.docker.container(&r.serving_name).unwrap().running);
}

fn deps_with(r: &Rig) -> Vec<Deployment> {
    let mut v = r.deps.clone();
    v.push(r.h.dep.clone());
    v
}

#[test]
fn a_rollback_serves_the_exact_retained_image_with_its_pinned_configuration_and_builds_nothing() {
    let mut r = rig();
    let t = ByPort {
        pre: ok(),
        candidate: ok(),
        restored: ok(),
    };
    let mut b = backend(&r, t);
    let imgs_before = r.h.docker.with(|s| s.images.len());
    let rep = run(&mut r, &mut b);
    let d = &r.h.dep;
    assert_eq!(rep.status, DeploymentStatus::Succeeded, "{:?}", d.failure);
    assert!(rep.is_clean(), "{:?}", rep.warnings);
    assert_eq!(d.replaces, Some(ReleaseId::from_deployment(&r.serving)));
    // serving release: stopped, never removed
    let s_ctr = r.h.docker.container(&r.serving_name).unwrap();
    assert!(!s_ctr.running);
    // the rollback container: exact pinned image id, pinned limits, this deployment's labels
    let c =
        r.h.docker
            .container(&format!("hp-{}", d.id))
            .expect("rollback container");
    assert!(c.running);
    assert_eq!(
        c.image, r.old_image,
        "created from the exact retained image id, never a tag"
    );
    for (k, v) in labels(&d.application_id, &d.id) {
        assert!(
            c.labels.iter().any(|(a, b)| *a == k && *b == v),
            "ownership label {k}"
        );
    }
    let body = c.create_body.to_json();
    assert!(
        body.contains("67108864") && body.contains("\"PidsLimit\":32"),
        "{body}"
    );
    // no build: no new image, no buildx call, and the pin is the target's with a fresh port
    assert_eq!(r.h.docker.with(|s| s.images.len()), imgs_before);
    assert!(rep
        .ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == d.id)
        .all(|e| e.key.kind != ResourceKind::Image));
    let pin = d.pinned.clone().unwrap();
    assert_eq!(
        (pin.image_id.as_str(), pin.memory_bytes),
        (r.old_image.as_str(), 64 * 1024 * 1024)
    );
    assert!(d.interruption.unwrap().duration_ms().is_some());
    // derived serving state and the pause
    let deps = deps_with(&r);
    let sv = serving_release(&rep.ledger, &deps, &d.application_id).unwrap();
    assert_eq!(sv.release, ReleaseId::from_deployment(&r.old));
    assert_eq!(sv.container_name, format!("hp-{}", d.id));
    assert_eq!(sv.via, ServedBy::RolledBack { by: d.id.clone() });
    assert_eq!(
        trigger_pauses(&deps)[0].rollback,
        Some(ReleaseId::from_deployment(&r.old))
    );
    nothing_unrelated_or_destructive(&r);
}

#[test]
fn a_missing_or_foreign_target_image_fails_before_anything_is_stopped() {
    // image removed behind Harpoon's back
    let mut r = rig();
    let img = r.old_image.clone();
    r.h.docker.with(|s| s.images.retain(|i| i.id != img));
    let mut b = backend(
        &r,
        ByPort {
            pre: ok(),
            candidate: ok(),
            restored: ok(),
        },
    );
    let rep = run(&mut r, &mut b);
    assert_eq!(rep.status, DeploymentStatus::Failed);
    let f = r.h.dep.failure.as_ref().unwrap();
    assert_eq!(f.step, Step::Source);
    assert!(f.message.contains("no longer exists"), "{}", f.message);
    assert_eq!(r.h.dep.cutover, CutoverState::NotStarted);
    assert_eq!(rep.pause, None);
    serving_was_never_changed(&r);
    assert!(r
        .h
        .docker
        .container(&format!("hp-{}", r.h.dep.id))
        .is_none());

    // an image that carries another application's labels is not the pinned image
    let mut r = rig();
    let (img, other) = (r.old_image.clone(), r.h.ids.gen.application().unwrap());
    let od = r.h.ids.gen.deployment().unwrap();
    r.h.docker
        .with(|s| s.images.iter_mut().find(|i| i.id == img).unwrap().labels = labels(&other, &od));
    let mut b = backend(
        &r,
        ByPort {
            pre: ok(),
            candidate: ok(),
            restored: ok(),
        },
    );
    let rep = run(&mut r, &mut b);
    assert_eq!(rep.status, DeploymentStatus::Failed);
    assert!(r
        .h
        .dep
        .failure
        .as_ref()
        .unwrap()
        .message
        .contains("Harpoon labels"));
    serving_was_never_changed(&r);
}

#[test]
fn a_failed_preliminary_check_leaves_the_serving_release_running() {
    let mut r = rig();
    let mut b = backend(
        &r,
        ByPort {
            pre: Observation::Status(503),
            candidate: ok(),
            restored: ok(),
        },
    );
    let rep = run(&mut r, &mut b);
    assert_eq!(rep.status, DeploymentStatus::Failed);
    assert_eq!(
        r.h.dep.failure.as_ref().unwrap().step,
        Step::PreliminaryCheck
    );
    assert_eq!(r.h.dep.cutover, CutoverState::NotStarted);
    assert_eq!(rep.pause, None);
    assert!(r.h.dep.interruption.is_none());
    serving_was_never_changed(&r);
    assert!(trigger_pauses(&deps_with(&r)).is_empty());
    nothing_unrelated_or_destructive(&r);
}

#[test]
fn failure_after_the_cutover_restores_the_release_that_was_serving_from_its_pin() {
    let mut r = rig();
    let mut b = backend(
        &r,
        ByPort {
            pre: ok(),
            candidate: Observation::Status(503),
            restored: ok(),
        },
    );
    let rep = run(&mut r, &mut b);
    let d = &r.h.dep;
    assert_eq!(rep.status, DeploymentStatus::Failed);
    assert_eq!(d.failure.as_ref().unwrap().kind, FailureKind::FinalCheck);
    assert_eq!(
        d.recovery.unwrap().outcome,
        RecoveryOutcome::Restored,
        "{:?}",
        rep.warnings
    );
    assert_eq!(rep.pause, Some(PauseReason::FailedCutover));
    // the previously serving release is back, from its own pinned image, on its own port
    let c =
        r.h.docker
            .container(&format!("hp-{}-restored", d.id))
            .expect("restored container");
    assert!(c.running);
    assert_eq!(c.image, r.serving_pin.image_id);
    assert_eq!(c.host_port, Some(SERVING_PORT));
    assert!(c.create_body.to_json().contains("134217728"));
    // the failed rollback candidate is kept for inspection, stopped
    assert!(
        !r.h.docker
            .container(&format!("hp-{}", d.id))
            .unwrap()
            .running
    );
    // serving derivation names the restored container of the serving release
    let sv = serving_release(&rep.ledger, &deps_with(&r), &d.application_id).unwrap();
    assert_eq!(sv.release, ReleaseId::from_deployment(&r.serving));
    assert_eq!(sv.via, ServedBy::Restored { by: d.id.clone() });
    assert_eq!(sv.container_name, format!("hp-{}-restored", d.id));
    nothing_unrelated_or_destructive(&r);
}

#[test]
fn selection_from_the_records_matches_what_the_backend_will_serve() {
    let r = rig();
    let t = select_target(
        &r.cfg.prior_ledger,
        &r.deps,
        &r.h.dep.application_id,
        &ReleaseId::from_deployment(&r.old),
    )
    .unwrap();
    assert_eq!(t.config.unwrap().image_id, r.old_image);
}

// ---------------------------------------------------------------- non-secret runtime environment

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
        _m: Option<&harpoon_deploy_docker::probe::ResponseMarker>,
    ) -> Observation {
        self.0.get(port, p, t)
    }
    fn is_simulated(&self) -> bool {
        true
    }
}

fn env_pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Environment of every container the run created, by name suffix, without the per-container marker.
fn created_env(r: &Rig) -> Vec<(String, Vec<String>)> {
    r.h.docker
        .requests()
        .iter()
        .filter(|q| q.method == "POST" && q.path.contains("/containers/create"))
        .map(|q| {
            let j = harpoon_deploy_docker::json::Json::parse(&q.body).unwrap();
            let env: Vec<String> = j
                .get("Env")
                .and_then(|e| e.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                env.iter()
                    .filter(|e| e.starts_with("HARPOON_DEPLOYMENT_MARKER="))
                    .count(),
                1,
                "exactly one marker per container: {env:?}"
            );
            (
                q.path.split("name=").nth(1).unwrap_or("").to_string(),
                env.into_iter()
                    .filter(|e| !e.starts_with("HARPOON_DEPLOYMENT_MARKER="))
                    .collect(),
            )
        })
        .collect()
}

fn marked_backend(r: &Rig, t: ByPort, cli_env: &[&str]) -> DockerBackend<Marked> {
    let mut cfg = config(&r.h.ws, &r.h.docker);
    cfg.enable_replacement = true;
    cfg.attribution = harpoon_deploy_docker::backend::Attribution::PerContainer;
    cfg.env = cli_env.iter().map(|s| s.to_string()).collect();
    DockerBackend::with_transport(cfg, Some(&r.h.ws.resources()), Marked(t))
        .unwrap()
        .with_entropy(test_entropy())
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024))
        .with_rollback(r.old.clone(), r.old_pin.clone())
}

#[test]
fn a_rollback_reproduces_the_target_releases_own_environment_not_the_command_line() {
    let mut r = rig();
    r.old_pin.env = env_pairs(&[("E", ""), ("OLD", "1=2")]);
    r.serving_pin.env = env_pairs(&[("SERVING", "x")]);
    r.deps[0].pinned = Some(r.old_pin.clone());
    r.deps[1].pinned = Some(r.serving_pin.clone());
    r.cfg.previous = Some(r.serving_pin.clone());
    let mut b = marked_backend(
        &r,
        ByPort {
            pre: ok(),
            candidate: ok(),
            restored: ok(),
        },
        &["CLI=ignored"],
    );
    let (rel, cfg) = (ReleaseId::from_deployment(&r.serving), r.cfg.clone());
    let rep =
        r.h.run_replacing_with(&mut b, &mut MemoryJournal::new(), &rel, &cfg)
            .unwrap();
    assert_eq!(
        rep.status,
        DeploymentStatus::Succeeded,
        "{:?}",
        r.h.dep.failure
    );
    for (name, env) in created_env(&r) {
        assert_eq!(
            env,
            ["E=", "OLD=1=2"],
            "{name}: the target's pin, nothing from the CLI or the serving release"
        );
    }
    assert_eq!(r.h.dep.pinned.as_ref().unwrap().env, r.old_pin.env);
}

#[test]
fn a_rollback_that_fails_restores_the_serving_releases_environment_not_the_rollback_targets() {
    let mut r = rig();
    r.old_pin.env = env_pairs(&[("OLD", "1")]);
    r.serving_pin.env = env_pairs(&[("SERVING", "x"), ("S2", "")]);
    r.deps[1].pinned = Some(r.serving_pin.clone());
    r.cfg.previous = Some(r.serving_pin.clone());
    let mut b = marked_backend(
        &r,
        ByPort {
            pre: ok(),
            candidate: Observation::Status(503),
            restored: ok(),
        },
        &[],
    );
    let (rel, cfg) = (ReleaseId::from_deployment(&r.serving), r.cfg.clone());
    let rep =
        r.h.run_replacing_with(&mut b, &mut MemoryJournal::new(), &rel, &cfg)
            .unwrap();
    assert_eq!(rep.status, DeploymentStatus::Failed);
    assert_eq!(
        r.h.dep.recovery.unwrap().outcome,
        RecoveryOutcome::Restored,
        "{:?}",
        rep.warnings
    );
    let made = created_env(&r);
    let restored = made
        .iter()
        .find(|(n, _)| n.ends_with("-restored"))
        .expect("a restoration");
    assert_eq!(restored.1, ["S2=", "SERVING=x"]);
    let candidate = made
        .iter()
        .find(|(n, _)| !n.ends_with("-restored") && !n.ends_with("-pre"))
        .unwrap();
    assert_eq!(candidate.1, ["OLD=1"]);
}
