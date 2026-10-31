//! The Docker backend driven through the real engine against a fake Docker on a test-owned
//! socket and a fake Buildx script. Nothing here contacts a live Docker endpoint, builds an
//! image, or starts a container.

mod support;

use harpoon_deploy::backend::{Backend, ResourceOps};
use harpoon_deploy::buildx::BuildxError;
use harpoon_deploy::engine::{EngineError, JournalError, RunConfig};
use harpoon_deploy::health::{Observation, Verdict};
use harpoon_deploy::ledger::{
    Disposition, EntryState, Ledger, LedgerRecord, RemoveRequest, RemoveResult, ResourceKey,
    ResourceKind,
};
use harpoon_deploy::logs::Redactor;
use harpoon_deploy::state::*;
use harpoon_deploy_docker::backend::{
    preliminary_name, replacement_name, serving_name, DockerBackend, DockerConfig,
};
use harpoon_deploy_docker::json::Json;
use harpoon_deploy_docker::probe::{HttpTransport, TcpHttp};
use std::sync::Once;
use std::time::Duration;
use support::fake_docker::{Action, Req};
use support::*;

static ENV: Once = Once::new();

/// Poison the process environment: if the backend or Buildx ever read it, these would leak.
fn poison_env() {
    ENV.call_once(|| {
        std::env::set_var("DOCKER_HOST", "unix:///nonexistent-canary.sock");
        std::env::set_var("DOCKER_CONTEXT", "canary-context");
        std::env::set_var("DOCKER_CONFIG", "/nonexistent-canary-config");
        std::env::set_var("BUILDX_BUILDER", "canary-builder");
        std::env::set_var("HARPOON_CANARY", "leaked");
    });
}

fn ok200() -> Scripted {
    Scripted::always(Observation::Status(200))
}

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
        .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024))
}

fn name_of(h: &Harness, f: fn(&harpoon_deploy::ids::DeploymentId) -> String) -> String {
    f(&h.dep.id)
}

fn no_forbidden_requests(h: &Harness) {
    let forbidden = [
        "prune",
        "/build",
        "/images/create",
        "/networks",
        "/volumes",
        "/exec",
        "/commit",
        "/plugins",
        "/swarm",
    ];
    for r in h.docker.requests() {
        assert!(
            !forbidden.iter().any(|f| r.path.contains(f)),
            "forbidden request: {} {}",
            r.method,
            r.path
        );
        assert!(
            r.method != "DELETE" || !r.path.contains("force=true"),
            "forced delete: {}",
            r.path
        );
    }
    assert!(
        h.docker.with(|s| s.unexpected.clone()).is_empty(),
        "requests outside the allowed set"
    );
}

fn created(h: &Harness, name: &str) -> Option<Req> {
    h.docker.requests().into_iter().find(|r| {
        r.method == "POST"
            && r.path.starts_with("/containers/create")
            && r.path.ends_with(&format!("name={name}"))
    })
}

// ---------------------------------------------------------------- success path

#[test]
fn a_first_deployment_succeeds_and_records_intent_before_and_receipt_after_every_create() {
    poison_env();
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    assert!(
        r.cleanup_problems().is_empty() && r.is_clean(),
        "{:?}",
        r.warnings
    );

    // serving container running with limits; preliminary container gone; image retained
    let serving = h
        .docker
        .container(&name_of(&h, serving_name))
        .expect("serving container exists");
    assert!(serving.running);
    assert!(h.docker.container(&name_of(&h, preliminary_name)).is_none());
    assert!(h.docker.with(|s| s
        .images
        .iter()
        .any(|i| i.tags.contains(&tag_for(&h.dep.application_id, &h.dep.id)))));
    let port = serving.host_port.unwrap();
    assert!((38000..=38999).contains(&port), "application range: {port}");

    // for every container create: an intent stamped earlier, a receipt (with Docker's id) later
    for name in [name_of(&h, preliminary_name), name_of(&h, serving_name)] {
        let req = created(&h, &name).unwrap_or_else(|| panic!("{name} was never created"));
        let key = ResourceKey::new(ResourceKind::Container, &name);
        let intent = j
            .stamps
            .iter()
            .find(|(_, r)| matches!(r, LedgerRecord::Intent { key: k, .. } if *k == key))
            .expect("intent")
            .0;
        let receipt = j
            .stamps
            .iter()
            .find(|(_, r)| matches!(r, LedgerRecord::Receipt { key: k, .. } if *k == key))
            .expect("receipt");
        assert!(
            intent < req.seq,
            "{name}: intent {intent} must precede the create call {}",
            req.seq
        );
        assert!(receipt.0 > req.seq, "{name}: receipt after the call");
        if let LedgerRecord::Receipt { id, .. } = &receipt.1 {
            assert_eq!(id.len(), 64, "the receipt carries the id Docker returned");
        }
    }
    // the image: intent before the build ran, receipt with the inspected image id
    let image_key = ResourceKey::new(
        ResourceKind::Image,
        tag_for(&h.dep.application_id, &h.dep.id),
    );
    let img_intent = j
        .stamps
        .iter()
        .find(|(_, r)| matches!(r, LedgerRecord::Intent { key, .. } if *key == image_key))
        .unwrap()
        .0;
    let img_inspect = h
        .docker
        .requests()
        .into_iter()
        .filter(|r| r.path.starts_with("/images/harpoon/"))
        .nth(1) // the check after the build; the first is the pre-build tag check
        .unwrap();
    assert!(img_intent < img_inspect.seq);
    let e = r.ledger.find(&h.dep.id, &image_key).unwrap();
    assert!(e.id.as_deref().unwrap().starts_with("sha256:"));

    // limits, labels, no HostIp, no restart policy in the create bodies
    for name in [name_of(&h, preliminary_name), name_of(&h, serving_name)] {
        let c = h.docker.container(&name);
        let body = created(&h, &name).unwrap().body;
        let j = Json::parse(&body).unwrap();
        assert_eq!(
            j.get("HostConfig")
                .and_then(|x| x.get("Memory"))
                .and_then(Json::as_i64),
            Some(268_435_456),
            "{name}"
        );
        assert_eq!(
            j.get("HostConfig")
                .and_then(|x| x.get("PidsLimit"))
                .and_then(Json::as_i64),
            Some(64)
        );
        assert!(!body.contains("HostIp"));
        assert!(
            c.is_none()
                || harpoon_deploy::backend::harpoon_labels(&h.dep.application_id, &h.dep.id)
                    .iter()
                    .all(|l| c.as_ref().unwrap().labels.contains(l))
        );
        assert_eq!(
            j.get("Labels").and_then(Json::as_object).map(|l| l.len()),
            Some(3)
        );
    }
    no_forbidden_requests(&h);
}

#[test]
fn the_preliminary_container_is_removed_before_the_application_container_exists() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    let reqs = h.docker.requests();
    let pre = h.dep.id.to_string();
    let pre_id = r
        .ledger
        .find(
            &h.dep.id,
            &ResourceKey::new(ResourceKind::Container, preliminary_name(&h.dep.id)),
        )
        .unwrap()
        .id
        .clone()
        .unwrap();
    let removed = reqs
        .iter()
        .find(|q| q.method == "DELETE" && q.path.contains(&pre_id))
        .expect("preliminary removed");
    let serving_create = created(&h, &serving_name(&h.dep.id)).unwrap();
    assert!(
        removed.seq < serving_create.seq,
        "preliminary removal ({}) precedes cutover ({}) {pre}",
        removed.seq,
        serving_create.seq
    );
    // the test port lease was released too
    let lease = r
        .ledger
        .entries()
        .iter()
        .find(|e| {
            e.key.kind == ResourceKind::PortLease && e.disposition == Disposition::RemoveAlways
        })
        .unwrap();
    assert!(lease.is_released());
}

#[test]
fn every_removal_is_preceded_by_a_read_of_the_same_object_and_stops_a_running_one_first() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, Scripted::always(Observation::Status(503)), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let reqs = h.docker.requests();
    let deletes: Vec<&Req> = reqs
        .iter()
        .filter(|q| q.method == "DELETE" && q.path.starts_with("/containers/"))
        .collect();
    assert!(!deletes.is_empty());
    for d in deletes {
        let id = d
            .path
            .trim_start_matches("/containers/")
            .split('?')
            .next()
            .unwrap();
        let inspect = reqs.iter().rev().find(|q| {
            q.seq < d.seq && q.method == "GET" && q.path == format!("/containers/{id}/json")
        });
        assert!(
            inspect.is_some(),
            "DELETE {id} without a prior inspect of that id"
        );
        let stop = reqs.iter().find(|q| {
            q.seq < d.seq
                && q.method == "POST"
                && q.path.starts_with(&format!("/containers/{id}/stop"))
        });
        assert!(
            stop.is_some(),
            "a running container is stopped before it is removed"
        );
    }
}

// ---------------------------------------------------------------- Buildx

#[test]
fn only_the_bundled_buildx_runs_with_an_isolated_environment() {
    poison_env();
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    h.run(&mut b, &mut j);
    let args = h.ws.buildx_record("args.txt");
    for want in [
        "build",
        "--load",
        "--pull=false",
        "--progress=plain",
        "-t",
        &tag_for(&h.dep.application_id, &h.dep.id),
        "io.harpoon.managed=true",
    ] {
        assert!(args.contains(want), "buildx args lack {want}: {args}");
    }
    assert!(!args.contains("--push") && !args.contains("--secret") && !args.contains("syntax"));
    let env = h.ws.buildx_record("env.txt");
    assert!(
        env.contains(&format!("DOCKER_HOST=unix://{}", h.docker.path().display())),
        "{env}"
    );
    assert!(env.contains(&format!("HOME={}", h.ws.scratch().display())));
    assert!(env.contains(&format!("DOCKER_CONFIG={}", h.ws.scratch().display())));
    for canary in [
        "canary",
        "HARPOON_CANARY",
        "DOCKER_CONTEXT",
        "BUILDX_BUILDER",
        "leaked",
    ] {
        assert!(
            !env.contains(canary),
            "the user's environment leaked into buildx ({canary}): {env}"
        );
    }
    // the snapshot is gone once the build ends, so compare the recorded path's parent
    let rec = h.ws.buildx_record("cwd.txt");
    let cwd = std::path::PathBuf::from(rec.trim());
    assert_eq!(
        std::fs::canonicalize(cwd.parent().unwrap()).unwrap(),
        std::fs::canonicalize(h.ws.scratch()).unwrap(),
        "builds run in a private snapshot inside the scratch directory, not in the user's folder"
    );
    assert!(cwd
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("ctx-"));
    assert!(
        args.contains("--iidfile") && args.contains("/iid-"),
        "{args}"
    );
}

#[test]
fn a_missing_bundled_buildx_fails_explicitly_without_any_fallback() {
    let h = Harness::new();
    std::fs::remove_file(h.ws.buildx_path()).unwrap();
    let e = DockerBackend::new(config(&h.ws, &h.docker), Some(&h.ws.resources()))
        .err()
        .unwrap();
    assert!(matches!(e, BuildxError::Missing(_)));
    let msg = e.to_string();
    assert!(
        msg.contains("never falls back") && msg.contains("fetch-buildx.sh"),
        "{msg}"
    );
    assert!(DockerBackend::new(config(&h.ws, &h.docker), None).is_err());
    assert!(h.docker.requests().is_empty(), "nothing was sent to Docker");
}

#[test]
fn a_non_executable_or_symlinked_buildx_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let h = Harness::new();
    std::fs::set_permissions(h.ws.buildx_path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        DockerBackend::new(config(&h.ws, &h.docker), Some(&h.ws.resources()))
            .err()
            .unwrap(),
        BuildxError::NotExecutable(_)
    ));
    std::fs::remove_file(h.ws.buildx_path()).unwrap();
    std::os::unix::fs::symlink("/usr/bin/true", h.ws.buildx_path()).unwrap();
    assert!(matches!(
        DockerBackend::new(config(&h.ws, &h.docker), Some(&h.ws.resources()))
            .err()
            .unwrap(),
        BuildxError::Symlink(_)
    ));
}

#[test]
fn a_failed_build_is_reported_with_its_output_and_leaves_nothing_behind() {
    let mut h = Harness::new();
    h.ws.install_buildx(
        "echo 'ERROR: failed to solve: process \"/bin/false\" did not complete' >&2\nexit 3",
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = h.dep.failure.as_ref().unwrap();
    assert_eq!(f.step, Step::Build);
    assert!(f.message.contains("exit 3"), "{}", f.message);
    let logs = r.logs.iter().find(|(s, _)| *s == Step::Build).unwrap();
    assert!(logs.1.text.contains("failed to solve"));
    assert!(h.docker.with(|s| s.containers.is_empty()));
    assert!(r.cleanup_problems().is_empty());
    let image = r
        .ledger
        .find(
            &h.dep.id,
            &ResourceKey::new(
                ResourceKind::Image,
                tag_for(&h.dep.application_id, &h.dep.id),
            ),
        )
        .unwrap();
    assert!(
        image.is_released(),
        "an intent whose build never produced an image is closed out, not left dangling"
    );
}

#[test]
fn a_hung_build_is_killed_at_its_deadline() {
    let mut h = Harness::new();
    h.ws.install_buildx("echo $$ > \"$D/pid.txt\"\nsleep 30 &\necho $! > \"$D/gpid.txt\"\nwait");
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let mut cfg = RunConfig::default();
    cfg.budgets.build_ms = 400;
    let t0 = std::time::Instant::now();
    let r = h.run_with(&mut b, &mut j, &cfg);
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "returned at the deadline, not after the sleep"
    );
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = h.dep.failure.as_ref().unwrap();
    assert!(f.timed_out && f.step == Step::Build, "{f:?}");
    let pid: i32 = h.ws.buildx_record("pid.txt").trim().parse().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let alive = |p: i32| {
        std::process::Command::new("/bin/kill")
            .args(["-0", &p.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    let gpid: i32 = h.ws.buildx_record("gpid.txt").trim().parse().unwrap();
    assert!(!alive(pid), "the Buildx process was killed");
    assert!(
        !alive(gpid),
        "and so was the child it started (the whole process group)"
    );
}

#[test]
fn build_output_is_redacted_and_bounded() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.ws.install_buildx("echo 'using token=SUPERSECRET123 for registry'\ni=0\nwhile [ $i -lt 5000 ]; do echo \"layer line $i\"; i=$((i+1)); done");
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let mut cfg = RunConfig {
        redactor: Redactor::new(&[("registry_token", "SUPERSECRET123")]),
        ..RunConfig::default()
    };
    cfg.log_limits.head_bytes = 2_000;
    cfg.log_limits.tail_bytes = 2_000;
    let r = h.run_with(&mut b, &mut j, &cfg);
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    let (_, cap) = r.logs.iter().find(|(s, _)| *s == Step::Build).unwrap();
    assert!(!cap.text.contains("SUPERSECRET123"));
    assert!(
        cap.summary.truncated()
            && cap.summary.lines_dropped > 4_000
            && cap.summary.bytes_kept <= 4_200,
        "{:?}",
        cap.summary
    );
    assert!(cap.summary.redactions >= 1);
}

#[test]
fn a_build_that_reports_success_but_produced_no_image_fails() {
    let mut h = Harness::new();
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h
        .dep
        .failure
        .as_ref()
        .unwrap()
        .message
        .contains("has no such image"));
}

#[test]
fn an_image_without_harpoons_labels_is_not_recorded_as_ours() {
    let mut h = Harness::new();
    let tag = tag_for(&h.dep.application_id, &h.dep.id);
    // the build "produces" an image with only one of the three labels
    h.seed_built_image_with(vec![("io.harpoon.managed".into(), "true".into())], vec![]);
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h.dep.failure.as_ref().unwrap().message.contains("labels"));
    let e = r
        .ledger
        .find(&h.dep.id, &ResourceKey::new(ResourceKind::Image, &tag))
        .unwrap();
    assert!(e.id.is_none() || matches!(e.state, EntryState::Refused(_)) || e.is_released());
    assert!(
        h.docker
            .with(|s| s.images.iter().any(|i| i.tags.contains(&tag))),
        "an object we cannot prove ours is not removed"
    );
}

// ---------------------------------------------------------------- source preflight

fn preflight_failure(h: &mut Harness, mut b: DockerBackend<Scripted>) -> (Report, String) {
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = h.dep.failure.clone().unwrap();
    assert_eq!(f.step, Step::Source, "{}", f.message);
    (r, f.message)
}
use harpoon_deploy::engine::Report;

#[test]
fn a_base_image_that_is_not_local_is_refused_and_never_pulled() {
    let mut h = Harness::new();
    h.ws.write_context("FROM node:22\nRUN true\n");
    let b = backend_with(&h, ok200(), |_| {});
    let (_, msg) = preflight_failure(&mut h, b);
    assert!(
        msg.contains("node:22") && msg.contains("does not pull"),
        "{msg}"
    );
    assert!(
        h.ws.buildx_record("args.txt").is_empty(),
        "buildx never ran"
    );
    no_forbidden_requests(&h);
}

#[test]
fn external_frontends_variable_images_and_missing_from_are_refused() {
    for (df, want) in [
        (
            "# syntax=docker/dockerfile:1\nFROM alpine:3.22\n",
            "external frontend",
        ),
        ("ARG B=alpine:3.22\nFROM $B\n", "variable"),
        ("RUN true\n", "no FROM"),
    ] {
        let mut h = Harness::new();
        h.ws.write_context(df);
        let b = backend_with(&h, ok200(), |_| {});
        let (_, msg) = preflight_failure(&mut h, b);
        assert!(msg.contains(want), "{df:?}: {msg}");
        assert!(h.ws.buildx_record("args.txt").is_empty());
    }
}

#[test]
fn multi_stage_builds_check_only_external_images() {
    let mut h = Harness::new();
    h.ws.write_context(
        "FROM alpine:3.22 AS base\nFROM base AS final\nCOPY --from=base /a /b\nFROM scratch\n",
    );
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    assert_eq!(
        h.run(&mut b, &mut j).status,
        DeploymentStatus::Succeeded,
        "{:?}",
        h.dep.failure
    );
}

#[test]
fn an_oversized_context_or_low_disk_stops_before_any_build() {
    let mut h = Harness::new();
    std::fs::write(h.ws.context().join("big.bin"), vec![0u8; 20_000]).unwrap();
    let b = backend_with(&h, ok200(), |c| c.max_context_bytes = 10_000);
    let (_, msg) = preflight_failure(&mut h, b);
    assert!(msg.contains("larger than"), "{msg}");

    let mut h = Harness::new();
    let b = backend_with(&h, ok200(), |_| {}).with_disk_probe(FakeDisk(1_000_000));
    let (_, msg) = preflight_failure(&mut h, b);
    assert!(
        msg.contains("free") && msg.contains("never frees space by pruning"),
        "{msg}"
    );
    no_forbidden_requests(&h);
}

#[test]
fn an_unreachable_docker_socket_is_reported_as_such() {
    let mut h = Harness::new();
    let b = backend_with(&h, ok200(), |c| {
        c.socket = std::env::temp_dir().join("hdd-absent.sock")
    });
    let (_, msg) = preflight_failure(&mut h, b);
    assert!(
        msg.contains("not reachable") && msg.contains("hdd-absent.sock"),
        "{msg}"
    );
    assert!(
        h.docker.requests().is_empty(),
        "the configured socket was the only one used, and the env's DOCKER_HOST was not"
    );
}

// ---------------------------------------------------------------- health

#[test]
fn a_failing_preliminary_check_removes_its_container_and_attaches_the_container_output() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, Scripted::always(Observation::Status(503)), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = h.dep.failure.as_ref().unwrap();
    assert_eq!(f.step, Step::PreliminaryCheck);
    assert!(f.message.contains("HTTP 503"), "{}", f.message);
    assert!(
        h.docker.container(&name_of(&h, preliminary_name)).is_none(),
        "removed"
    );
    assert!(
        h.docker.container(&name_of(&h, serving_name)).is_none(),
        "never cut over"
    );
    assert!(r
        .evidence
        .iter()
        .any(|(s, e)| *s == Step::PreliminaryCheck && !e.is_healthy()));
    assert!(r
        .logs
        .iter()
        .any(|(s, c)| *s == Step::PreliminaryCheck && c.text.contains("container output")));
    assert!(r.cleanup_problems().is_empty());
}

#[test]
fn a_container_that_exits_during_the_check_is_reported_as_exited() {
    let mut h = Harness::new();
    h.seed_built_image();
    let pre = name_of(&h, preliminary_name);
    let pre2 = pre.clone();
    h.docker.fault(
        Some("GET"),
        "/json",
        1_000,
        Action::Race(std::sync::Arc::new(move |s| {
            for c in s.containers.iter_mut().filter(|c| c.name == pre2) {
                if c.running {
                    c.running = false;
                    c.exit_code = 137;
                }
            }
        })),
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let (_, ev) = r
        .evidence
        .iter()
        .find(|(s, _)| *s == Step::PreliminaryCheck)
        .unwrap();
    assert_eq!(ev.verdict, Verdict::Exited);
    assert!(h
        .dep
        .failure
        .as_ref()
        .unwrap()
        .message
        .contains("exited with code 137"));
}

#[test]
fn a_final_check_failure_removes_the_new_container_and_releases_its_port_but_keeps_the_image() {
    let mut h = Harness::new();
    h.seed_built_image();
    let t = Scripted::always(Observation::Status(503));
    *t.answers.borrow_mut() = vec![Observation::Status(200), Observation::Status(200)]; // preliminary passes
    let mut b = backend_with(&h, t, |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert_eq!(h.dep.failure.as_ref().unwrap().step, Step::FinalCheck);
    assert!(
        h.objects_for_dep().is_empty(),
        "no container of this deployment remains"
    );
    assert!(h.docker.with(|s| s
        .images
        .iter()
        .any(|i| i.tags.contains(&tag_for(&h.dep.application_id, &h.dep.id)))));
    assert!(r.ledger.leased_ports().is_empty(), "every lease released");
    assert!(r.cleanup_problems().is_empty());
}

#[test]
fn a_check_that_outlasts_its_step_budget_times_out() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(
        &h,
        Scripted::always(Observation::ConnectFailed("refused".into())),
        |c| c.health.startup_deadline_ms = 60_000,
    );
    let mut j = StampedJournal::new();
    let mut cfg = RunConfig::default();
    cfg.budgets.preliminary_ms = 300;
    let t0 = std::time::Instant::now();
    let r = h.run_with(&mut b, &mut j, &cfg);
    assert!(t0.elapsed() < Duration::from_secs(5));
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h.dep.failure.as_ref().unwrap().timed_out);
}

#[test]
fn simulated_evidence_is_flagged_and_only_a_simulated_transport_can_produce_it() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |_| {});
    assert!(
        b.accepts_simulated_evidence(),
        "a scripted transport is an explicit test seam"
    );
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert!(
        r.evidence.iter().all(|(_, e)| e.simulated),
        "evidence from a scripted transport says so"
    );

    // The real transport: never simulated, so the engine would refuse simulated evidence.
    let real = DockerBackend::new(config(&h.ws, &h.docker), Some(&h.ws.resources())).unwrap();
    assert!(!real.accepts_simulated_evidence());
    assert!(!TcpHttp.is_simulated());
}

#[test]
fn with_the_real_transport_a_closed_port_means_the_deployment_fails() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = DockerBackend::new(
        {
            let mut c = config(&h.ws, &h.docker);
            c.health.startup_deadline_ms = 200;
            c
        },
        Some(&h.ws.resources()),
    )
    .unwrap()
    .with_disk_probe(FakeDisk(10 << 30));
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(
        r.status,
        DeploymentStatus::Failed,
        "nothing listens on the published port"
    );
    let (_, ev) = r
        .evidence
        .iter()
        .find(|(s, _)| *s == Step::PreliminaryCheck)
        .unwrap();
    assert!(!ev.simulated && ev.verdict == Verdict::NotHealthy);
    assert!(matches!(
        ev.last_observation(),
        Some(Observation::ConnectFailed(_))
    ));
    assert!(h.objects_for_dep().is_empty());
}

// ---------------------------------------------------------------- names, ports, collisions

#[test]
fn a_foreign_container_with_the_same_name_is_never_touched() {
    let mut h = Harness::new();
    h.seed_built_image();
    let pre = name_of(&h, preliminary_name);
    let foreign = h.docker.with(|s| s.add_container(&pre, vec![], true));
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h
        .dep
        .failure
        .as_ref()
        .unwrap()
        .message
        .contains("already in use"));
    let c = h
        .docker
        .container(&pre)
        .expect("the foreign container survives");
    assert!(c.id == foreign && c.running && c.labels.is_empty());
    assert!(!h
        .docker
        .requests()
        .iter()
        .any(|q| q.method == "DELETE" && q.path.contains(&foreign)));
    assert!(!h
        .docker
        .requests()
        .iter()
        .any(|q| q.path.contains(&format!("{foreign}/stop"))));
    // and the report does not hide it
    let e = r
        .ledger
        .find(&h.dep.id, &ResourceKey::new(ResourceKind::Container, &pre))
        .unwrap();
    assert!(matches!(e.state, EntryState::Refused(_)), "{:?}", e.state);
    assert!(!r.cleanup_problems().is_empty() && !r.is_clean());
}

#[test]
fn a_leftover_container_labelled_for_this_deployment_is_adopted_and_removed() {
    let mut h = Harness::new();
    h.seed_built_image();
    let pre = name_of(&h, preliminary_name);
    let l = labels(&h.dep.application_id, &h.dep.id);
    h.docker.with(|s| s.add_container(&pre, l, false));
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(
        h.docker.container(&pre).is_none(),
        "ours by label, so removed"
    );
    assert!(r.cleanup_problems().is_empty());
}

#[test]
fn listeners_and_docker_mappings_steer_port_allocation() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker.with(|s| {
        let id = s.add_container("someone-elses", vec![], true);
        let _ = id;
        s.containers[0].host_port = Some(38000);
    });
    let mut b = backend_with(&h, ok200(), |_| {})
        .with_port_probe(Listening([39000u16].into_iter().collect()));
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    let pre_body = Json::parse(&created(&h, &name_of(&h, preliminary_name)).unwrap().body)
        .unwrap()
        .to_json();
    assert!(
        pre_body.contains("\"HostPort\":\"39001\""),
        "39000 is listening on the host: {pre_body}"
    );
    assert_eq!(
        h.docker
            .container(&name_of(&h, serving_name))
            .unwrap()
            .host_port,
        Some(38001),
        "38000 is mapped by Docker"
    );
}

#[test]
fn one_port_collision_gets_one_replacement_and_the_first_container_is_removed() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker.with(|s| {
        s.taken_ports.insert(38000);
    });
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    assert!(
        h.docker.container(&name_of(&h, serving_name)).is_none(),
        "the collided container was removed"
    );
    let c = h
        .docker
        .container(&name_of(&h, replacement_name))
        .expect("replacement exists");
    assert_eq!(c.host_port, Some(38001));
    let lease = |p: &str| {
        r.ledger
            .find(&h.dep.id, &ResourceKey::new(ResourceKind::PortLease, p))
            .unwrap()
            .clone()
    };
    assert!(lease("38000").is_released() && !lease("38001").is_released());
    assert!(r.cleanup_problems().is_empty());
}

#[test]
fn a_second_collision_fails_the_deployment_and_cleans_up_everything() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker.with(|s| {
        s.taken_ports.extend([38000, 38001]);
    });
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = h.dep.failure.as_ref().unwrap();
    assert_eq!(f.step, Step::Cutover);
    assert!(
        f.message.contains("38000") && f.message.contains("38001"),
        "{}",
        f.message
    );
    assert!(h.objects_for_dep().is_empty(), "both containers removed");
    assert!(r.ledger.leased_ports().is_empty());
    assert!(r.cleanup_problems().is_empty());
}

// ---------------------------------------------------------------- removal safety and races

fn remove_request(h: &Harness, name: &str, id: &str) -> RemoveRequest {
    RemoveRequest {
        kind: ResourceKind::Container,
        id: id.to_string(),
        name: name.to_string(),
        app: h.dep.application_id.clone(),
        deployment: h.dep.id.clone(),
    }
}

#[test]
fn removal_refuses_when_labels_name_or_id_no_longer_match() {
    let mut h = Harness::new();
    let l = labels(&h.dep.application_id, &h.dep.id);
    let mut b = backend_with(&h, ok200(), |_| {});

    // labels missing one of the three
    let partial: Vec<_> = l.iter().take(2).cloned().collect();
    let id = h.docker.with(|s| s.add_container("c1", partial, false));
    assert!(
        matches!(b.remove_resource(&remove_request(&h, "c1", &id)).unwrap(), RemoveResult::Refused(m) if m.contains("labels"))
    );
    // labels of another deployment
    let other = harpoon_deploy::backend::harpoon_labels(
        &h.dep.application_id,
        &h.ids.gen.deployment().unwrap(),
    );
    let id2 = h.docker.with(|s| s.add_container("c2", other, false));
    assert!(matches!(
        b.remove_resource(&remove_request(&h, "c2", &id2)).unwrap(),
        RemoveResult::Refused(_)
    ));
    // all labels but the container was renamed
    let id3 = h
        .docker
        .with(|s| s.add_container("renamed", l.clone(), false));
    assert!(
        matches!(b.remove_resource(&remove_request(&h, "c3", &id3)).unwrap(), RemoveResult::Refused(m) if m.contains("now named"))
    );
    // a request carrying an id that is a *prefix* of the real one is not an identity match
    let id4 = h.docker.with(|s| s.add_container("c4", l.clone(), false));
    assert!(matches!(
        b.remove_resource(&remove_request(&h, "c4", &id4[..12]))
            .unwrap(),
        RemoveResult::Refused(_) | RemoveResult::AlreadyAbsent
    ));
    assert!(
        h.docker.requests().iter().all(|q| q.method != "DELETE"),
        "nothing was deleted"
    );
    assert_eq!(h.docker.with(|s| s.containers.len()), 4);
}

#[test]
fn removal_of_an_absent_object_or_one_that_vanishes_mid_way_is_already_absent() {
    let h = Harness::new();
    let l = labels(&h.dep.application_id, &h.dep.id);
    let mut b = backend_with(&h, ok200(), |_| {});
    assert_eq!(
        b.remove_resource(&remove_request(&h, "ghost", &"a".repeat(64)))
            .unwrap(),
        RemoveResult::AlreadyAbsent
    );

    // the container disappears between our inspect and our DELETE
    let id = h.docker.with(|s| s.add_container("racer", l, false));
    let idc = id.clone();
    h.docker.fault(
        Some("DELETE"),
        &id,
        1,
        Action::Race(std::sync::Arc::new(move |s| {
            s.containers.retain(|c| c.id != idc)
        })),
    );
    assert_eq!(
        b.remove_resource(&remove_request(&h, "racer", &id))
            .unwrap(),
        RemoveResult::AlreadyAbsent
    );
}

#[test]
fn failed_stops_and_conflicting_deletes_are_errors_not_successes() {
    let h = Harness::new();
    let l = labels(&h.dep.application_id, &h.dep.id);
    let mut b = backend_with(&h, ok200(), |_| {});
    let id = h.docker.with(|s| s.add_container("busy", l.clone(), true));
    h.docker.fault(
        Some("POST"),
        "/stop",
        1,
        Action::Respond(
            500,
            "{\"message\":\"cannot stop container: permission denied\"}".into(),
        ),
    );
    let e = b
        .remove_resource(&remove_request(&h, "busy", &id))
        .unwrap_err();
    assert!(e.message.contains("permission denied"), "{}", e.message);
    assert!(h.docker.container("busy").is_some());

    let id2 = h.docker.with(|s| s.add_container("stuck", l, false));
    h.docker.fault(
        Some("DELETE"),
        &id2,
        1,
        Action::Respond(
            409,
            "{\"message\":\"removal of container is already in progress\"}".into(),
        ),
    );
    let e = b
        .remove_resource(&remove_request(&h, "stuck", &id2))
        .unwrap_err();
    assert!(e.message.contains("already in progress"));
}

#[test]
fn a_removal_that_times_out_is_a_timeout_error() {
    let h = Harness::new();
    let l = labels(&h.dep.application_id, &h.dep.id);
    let mut b = backend_with(&h, ok200(), |c| {
        c.cleanup_timeout = Duration::from_millis(150)
    });
    let id = h.docker.with(|s| s.add_container("slow", l, false));
    h.docker.fault(
        Some("GET"),
        &format!("/containers/{id}/json"),
        1,
        Action::Delay(Duration::from_millis(600)),
    );
    let e = b
        .remove_resource(&remove_request(&h, "slow", &id))
        .unwrap_err();
    assert!(e.timed_out, "{}", e.message);
}

#[test]
fn images_are_removed_only_with_all_labels_and_without_force_or_prune() {
    let h = Harness::new();
    let l = labels(&h.dep.application_id, &h.dep.id);
    let mut b = backend_with(&h, ok200(), |_| {});
    let req = |id: &str, name: &str| RemoveRequest {
        kind: ResourceKind::Image,
        id: id.into(),
        name: name.into(),
        app: h.dep.application_id.clone(),
        deployment: h.dep.id.clone(),
    };
    let foreign = h.docker.with(|s| s.add_image("postgres:16", vec![]));
    assert!(matches!(
        b.remove_resource(&req(&foreign, "postgres:16")).unwrap(),
        RemoveResult::Refused(_)
    ));
    let ours = h.docker.with(|s| s.add_image("harpoon/x:y", l));
    assert_eq!(
        b.remove_resource(&req(&ours, "harpoon/x:y")).unwrap(),
        RemoveResult::Removed
    );
    assert!(h.docker.with(|s| s
        .images
        .iter()
        .any(|i| i.tags == vec!["postgres:16".to_string()])));
    let del = h
        .docker
        .requests()
        .into_iter()
        .find(|q| q.method == "DELETE")
        .unwrap();
    assert!(
        del.path.contains("force=false") && del.path.contains("noprune=true"),
        "{}",
        del.path
    );
    assert!(matches!(
        b.remove_resource(&RemoveRequest {
            kind: ResourceKind::PortLease,
            ..req("1", "38000")
        })
        .unwrap(),
        RemoveResult::Refused(_)
    ));
}

// ---------------------------------------------------------------- partial operations and cleanup failure

#[test]
fn a_create_whose_reply_is_lost_is_adopted_by_label_and_removed() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker
        .fault(Some("POST"), "/containers/create", 1, Action::ApplyThenDrop);
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(
        h.dep
            .failure
            .as_ref()
            .unwrap()
            .message
            .contains("closed the connection"),
        "{}",
        h.dep.failure.as_ref().unwrap().message
    );
    let pre = ResourceKey::new(ResourceKind::Container, preliminary_name(&h.dep.id));
    let e = r.ledger.find(&h.dep.id, &pre).unwrap();
    assert!(
        e.id.is_some() && e.is_released(),
        "adopted (receipt from the listing), then removed: {e:?}"
    );
    assert!(h.docker.container(&preliminary_name(&h.dep.id)).is_none());
    assert!(r.cleanup_problems().is_empty());
}

#[test]
fn a_failed_removal_stays_in_the_outcome_and_the_object_stays_put() {
    let mut h = Harness::new();
    h.seed_built_image();
    let t = Scripted::always(Observation::Status(503));
    *t.answers.borrow_mut() = vec![Observation::Status(200), Observation::Status(200)];
    let mut b = backend_with(&h, t, |_| {});
    // every DELETE of a container fails
    h.docker.fault(
        Some("DELETE"),
        "/containers/",
        100,
        Action::Respond(500, "{\"message\":\"device or resource busy\"}".into()),
    );
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let problems = r.cleanup_problems();
    assert!(!problems.is_empty());
    assert!(problems.iter().all(|e| matches!(&e.state, EntryState::CleanupFailed(m) if m.contains("device or resource busy"))));
    assert!(!r.is_clean());
    assert!(
        !h.objects_for_dep().is_empty(),
        "the objects really are still there"
    );
    // the journal alone reproduces the same accounting
    assert_eq!(Ledger::replay(&j.inner.ledger_records).unwrap(), r.ledger);
}

#[test]
fn an_unlistable_engine_during_cleanup_is_a_visible_failed_cleanup() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, Scripted::always(Observation::Status(503)), |_| {});
    // the first listing (port occupancy) works; every listing after the container exists fails
    h.docker.fault_after(
        Some("GET"),
        "/containers/json",
        1,
        1_000,
        Action::Respond(500, "{\"message\":\"daemon is shutting down\"}".into()),
    );
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let problems = r.cleanup_problems();
    assert!(!problems.is_empty() && !r.is_clean());
    assert!(problems.iter().all(|e| matches!(&e.state, EntryState::CleanupFailed(m) if m.contains("could not list") && m.contains("shutting down"))), "{problems:?}");
    assert!(
        !h.objects_for_dep().is_empty(),
        "and the containers are still there, truthfully reported"
    );
}

#[test]
fn a_docker_that_dies_mid_deployment_fails_the_step_not_the_process() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker
        .fault(Some("POST"), "/start", 1, Action::Disconnect);
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h
        .dep
        .failure
        .as_ref()
        .unwrap()
        .message
        .contains("closed the connection"));
    assert!(
        h.objects_for_dep().is_empty(),
        "cleanup still ran and removed what was created"
    );
}

#[test]
fn a_slow_start_is_cut_off_by_the_step_budget() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker.fault(
        Some("POST"),
        "/start",
        1,
        Action::Delay(Duration::from_millis(900)),
    );
    let mut b = backend_with(&h, ok200(), |c| c.api_timeout = Duration::from_millis(200));
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(h.dep.failure.as_ref().unwrap().timed_out);
    assert!(
        r.cleanup_problems().is_empty(),
        "{:?}",
        r.cleanup_problems()
    );
}

// ---------------------------------------------------------------- crash injection

#[test]
fn a_crash_at_every_write_is_recovered_by_label_with_the_real_backend() {
    // measure the writes of a clean run first
    let total = {
        let mut h = Harness::new();
        h.seed_built_image();
        let mut b = backend_with(&h, ok200(), |c| c.health.startup_deadline_ms = 300);
        let mut j = StampedJournal::new();
        assert_eq!(h.run(&mut b, &mut j).status, DeploymentStatus::Succeeded);
        j.inner.writes()
    };
    assert!(total > 20);
    let mut adopted = false;
    for crash_at in 0..total {
        let ctx = format!("crash @{crash_at}");
        let mut h = Harness::new();
        h.seed_built_image();
        // an unrelated object that must survive anything
        let by = h
            .docker
            .with(|s| s.add_container("bystander", vec![], true));
        let mut b = backend_with(&h, ok200(), |c| c.health.startup_deadline_ms = 300);
        let mut first = StampedJournal::crash_at(crash_at);
        let res = h.try_run(&mut b, &mut first, &RunConfig::default());
        assert!(
            matches!(res, Err(EngineError::Journal(JournalError::Crash))),
            "{ctx}"
        );
        if first.inner.last().is_none() {
            continue;
        }

        // restart: replay the ledger, mark the deployment interrupted, reconcile with a new backend
        let mut ledger = Ledger::replay(&first.inner.ledger_records).expect(&ctx);
        let mut d = first.inner.last().unwrap().clone();
        recover_on_start(std::slice::from_mut(&mut d), 1);
        let end = harpoon_deploy::engine::end_state(d.status).unwrap();
        let mut b2 = backend_with(&h, ok200(), |_| {});
        let mut second = StampedJournal::new();
        harpoon_deploy::engine::reconcile(
            &mut ledger,
            &[(d.id.clone(), end)],
            &mut second,
            &mut b2,
            &harpoon_deploy::time::SystemClock,
        )
        .expect(&ctx);
        adopted |= second.stamps.iter().any(|(_, r)| matches!(r, LedgerRecord::Receipt { key, .. } if key.kind == ResourceKind::Container));

        assert!(
            h.objects_for_dep().is_empty(),
            "{ctx}: containers of the crashed deployment remain"
        );
        for e in ledger.entries() {
            assert!(!matches!(e.state, EntryState::Intended), "{ctx}: {e:?}");
            if e.disposition != Disposition::Retain {
                assert!(e.is_released(), "{ctx}: {e:?}");
            }
        }
        assert!(
            h.docker
                .container("bystander")
                .map(|c| c.id == by && c.running)
                .unwrap_or(false),
            "{ctx}: bystander touched"
        );
        assert!(
            h.docker.with(|s| s
                .images
                .iter()
                .any(|i| i.tags.contains(&"alpine:3.22".to_string()))),
            "{ctx}: base image touched"
        );
        no_forbidden_requests(&h);
    }
    assert!(
        adopted,
        "some crash point left a created-but-unreceipted container that recovery adopted by label"
    );
}

// ---------------------------------------------------------------- scope

#[test]
fn release_replacement_is_explicitly_not_implemented() {
    let h = Harness::new();
    let mut b = backend_with(&h, ok200(), |_| {});
    assert_eq!(
        b.restore_previous(1_000),
        harpoon_deploy::backend::RestoreResult::NotAttempted(Reason::NoPreviousRelease)
    );
}

#[test]
fn tests_never_name_the_live_harpoon_socket() {
    let live = concat!("/tmp/harpoon-docker", ".sock");
    for dir in ["tests", "src"] {
        for entry in
            std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)).unwrap()
        {
            let p = entry.unwrap().path();
            if p.extension().map(|e| e == "rs").unwrap_or(false) {
                let text = std::fs::read_to_string(&p).unwrap();
                assert!(
                    !text.contains(live),
                    "{} names the live socket",
                    p.display()
                );
            }
        }
    }
    assert!(support::fake_docker::FakeDocker::start()
        .path()
        .starts_with(std::env::temp_dir()));
}

// ---------------------------------------------------------------- image-tag isolation

fn tag_collision_run(h: &mut Harness) -> (Report, StampedJournal) {
    let mut b = backend_with(h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    (r, j)
}

fn image_id_of(h: &Harness, tag: &str) -> Option<String> {
    h.docker.with(|s| {
        s.images
            .iter()
            .find(|i| i.tags.contains(&tag.to_string()))
            .map(|i| i.id.clone())
    })
}

#[test]
fn a_foreign_image_already_holding_the_tag_stops_the_build_before_buildx_runs() {
    let mut h = Harness::new();
    let tag = tag_for(&h.dep.application_id, &h.dep.id);
    let foreign = h.preexisting_image(vec![]);
    let (r, j) = tag_collision_run(&mut h);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = h.dep.failure.as_ref().unwrap();
    assert_eq!(f.step, Step::Build);
    assert!(
        f.message.contains("already exists") && f.message.contains("did not create"),
        "{}",
        f.message
    );
    assert!(
        h.ws.buildx_record("args.txt").is_empty(),
        "Buildx never ran"
    );
    assert_eq!(
        image_id_of(&h, &tag),
        Some(foreign),
        "the existing image keeps its tag"
    );
    assert!(
        !j.stamps.iter().any(|(_, r)| matches!(r, LedgerRecord::Intent { key, .. } if key.kind == ResourceKind::Image)),
        "no intent is recorded for an image this deployment will not create"
    );
    assert!(h.docker.with(|s| s.containers.is_empty()));
    assert!(r.cleanup_problems().is_empty());
    no_forbidden_requests(&h);
}

#[test]
fn an_existing_harpoon_owned_tag_is_not_silently_overwritten() {
    for (what, l) in [
        ("this deployment's labels", None),
        ("another deployment's labels", Some(())),
        ("only the managed label", Some(())),
    ] {
        let mut h = Harness::new();
        let tag = tag_for(&h.dep.application_id, &h.dep.id);
        let existing_labels = match what {
            "this deployment's labels" => labels(&h.dep.application_id, &h.dep.id),
            "another deployment's labels" => {
                let other = h.ids.gen.deployment().unwrap();
                labels(&h.dep.application_id, &other)
            }
            _ => vec![("io.harpoon.managed".to_string(), "true".to_string())],
        };
        let _ = l;
        let existing = h.preexisting_image(existing_labels);
        let (r, j) = tag_collision_run(&mut h);
        assert_eq!(r.status, DeploymentStatus::Failed, "{what}");
        assert!(
            h.dep
                .failure
                .as_ref()
                .unwrap()
                .message
                .contains("was not overwritten"),
            "{what}"
        );
        assert!(
            h.ws.buildx_record("args.txt").is_empty(),
            "{what}: Buildx never ran"
        );
        assert_eq!(
            image_id_of(&h, &tag),
            Some(existing),
            "{what}: image and tag untouched"
        );
        assert!(!j.stamps.iter().any(|(_, r)| matches!(r, LedgerRecord::Intent { key, .. } if key.kind == ResourceKind::Image)), "{what}");
        assert!(
            !h.docker.requests().iter().any(|q| q.method == "DELETE"),
            "{what}: nothing was removed"
        );
    }
}

#[test]
fn the_tag_check_precedes_the_build_and_the_intent_precedes_the_build() {
    let mut h = Harness::new();
    h.seed_built_image();
    let (r, j) = tag_collision_run(&mut h);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    let inspects: Vec<Req> = h
        .docker
        .requests()
        .into_iter()
        .filter(|q| q.path.starts_with("/images/harpoon/"))
        .collect();
    assert!(inspects.len() >= 2, "once before the build, once after");
    let key = ResourceKey::new(
        ResourceKind::Image,
        tag_for(&h.dep.application_id, &h.dep.id),
    );
    let intent = j
        .stamps
        .iter()
        .find(|(_, r)| matches!(r, LedgerRecord::Intent { key: k, .. } if *k == key))
        .unwrap()
        .0;
    assert!(
        inspects[0].seq < intent,
        "the tag is checked before the intent is recorded"
    );
    assert!(
        intent < inspects[1].seq,
        "and the intent is durable before the build's result is looked at"
    );
}

// ---------------------------------------------------------------- volumes

#[test]
fn an_image_that_declares_volumes_is_rejected_before_any_container_exists() {
    for vols in [
        vec!["/data".to_string()],
        vec!["/var/lib/db".to_string(), "/cache".to_string()],
    ] {
        let mut h = Harness::new();
        let existing_container = h
            .docker
            .with(|s| s.add_container("bystander", vec![], true));
        h.seed_built_image_with(labels(&h.dep.application_id, &h.dep.id), vols.clone());
        let mut b = backend_with(&h, ok200(), |_| {});
        let mut j = StampedJournal::new();
        let r = h.run(&mut b, &mut j);
        assert_eq!(r.status, DeploymentStatus::Failed);
        let f = h.dep.failure.as_ref().unwrap();
        assert_eq!(f.step, Step::PreliminaryCheck);
        assert!(
            f.message.contains("declares volumes")
                && f.message.contains(&vols[0])
                && f.message.contains("no container was created"),
            "{}",
            f.message
        );
        assert!(
            !h.docker
                .requests()
                .iter()
                .any(|q| q.path.starts_with("/containers/create")),
            "no container create was even attempted"
        );
        assert!(
            !h.docker
                .requests()
                .iter()
                .any(|q| q.path.contains("/volumes")),
            "no volume was read or changed"
        );
        assert!(
            !j.stamps.iter().any(|(_, r)| matches!(r, LedgerRecord::Intent { key, .. } if key.kind == ResourceKind::Container)),
            "no container intent either"
        );
        assert_eq!(h.docker.with(|s| s.containers.len()), 1);
        assert_eq!(
            h.docker.container("bystander").unwrap().id,
            existing_container
        );
        assert!(r.cleanup_problems().is_empty());
    }
}

#[test]
fn an_image_without_volumes_still_deploys() {
    let mut h = Harness::new();
    h.seed_built_image_with(labels(&h.dep.application_id, &h.dep.id), vec![]);
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    assert_eq!(h.run(&mut b, &mut j).status, DeploymentStatus::Succeeded);
}

#[test]
fn a_malformed_volumes_field_is_an_error_not_a_pass() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker.fault_after(
        Some("GET"),
        "/images/harpoon/",
        1,
        1,
        Action::Respond(
            200,
            "{\"Id\":\"sha256:aa\",\"Config\":{\"Labels\":null,\"Volumes\":[\"/data\"]}}".into(),
        ),
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    assert!(
        h.dep.failure.as_ref().unwrap().message.contains("Volumes"),
        "{}",
        h.dep.failure.as_ref().unwrap().message
    );
    assert!(!h
        .docker
        .requests()
        .iter()
        .any(|q| q.path.starts_with("/containers/create")));
}
