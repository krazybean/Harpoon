//! `cleanup-releases`, end to end through the real CLI, the real journal and the real `DockerBackend`, against the
//! simulated Docker server. No real Docker, VM, container or image is involved.

use super::*;
use harpoon_deploy::ids::ReleaseId;
use support::fake_docker::St;

struct W {
    app: ApplicationId,
    old: DeploymentId,
    serving: DeploymentId,
    failed: DeploymentId,
    ids: std::collections::BTreeMap<String, (String, String)>, // dep -> (image id, container id)
}

fn auth() -> String {
    format!("--{AUTH_FLAG}={AUTH_PHRASE}")
}

/// One application with an old release (stopped container, image), the release serving now (running container,
/// image) and a failed deployment (retained image only), all recorded in the journal and present in Docker.
fn world(t: &T) -> W {
    world_with(t, false)
}

fn world_with(t: &T, serving_pins_the_old_image: bool) -> W {
    t.docker_runtime();
    let (app, old) = ids(1);
    let (_, serving) = ids(2);
    let (_, failed) = ids(3);
    let mut w = W { app: app.clone(), old: old.clone(), serving: serving.clone(), failed: failed.clone(), ids: Default::default() };
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    for (n, (dep, status, at)) in [(&old, DeploymentStatus::Succeeded, 10_000u64), (&serving, DeploymentStatus::Succeeded, 20_000), (&failed, DeploymentStatus::Failed, 30_000)].into_iter().enumerate() {
        let mut d = finished(&app, dep, status);
        if status == DeploymentStatus::Succeeded {
            d.release = Some(ReleaseId::from_deployment(dep));
            if dep == &serving {
                let pinned = support::fake_iid(if serving_pins_the_old_image { 2000 } else { 2001 });
                d.pinned = Some(harpoon_deploy::release::ReleaseConfig {
                    image_id: pinned,
                    image_tag: "harpoon/x:y".into(),
                    container_port: 8080,
                    host_port: 38000,
                    health: harpoon_deploy::health::HealthSpec::first_test("/"),
                    memory_bytes: 0,
                    nano_cpus: 0,
                    pids: 0,
                    env: vec![],
                    secrets: vec![],
                    snapshot: None,
                    build: None,
                });
            }
        }
        d.finished_ms = Some(at);
        j.persist(&d).unwrap();
        let tag = format!("harpoon/{}:{}-0123456789abcdef0123456789abcdef", app.as_str().to_ascii_lowercase(), dep);
        let iid = support::fake_iid(2000 + n as u64);
        let lbl = harpoon_deploy::backend::harpoon_labels(&app, dep);
        t.h.docker.with(|s| s.add_image_with_id(&iid, &[&tag], lbl.clone(), vec![]));
        j.record(&intent(&app, dep, ResourceKind::Image, &tag, Disposition::Retain)).unwrap();
        j.record(&receipt(dep, ResourceKind::Image, &tag, &iid)).unwrap();
        let mut cid = String::new();
        if status == DeploymentStatus::Succeeded {
            let name = format!("hp-{dep}");
            let running = dep == &serving;
            cid = t.h.docker.with(|s| s.add_container(&name, lbl, running));
            j.record(&intent(&app, dep, ResourceKind::Container, &name, Disposition::RemoveOnFailure)).unwrap();
            j.record(&receipt(dep, ResourceKind::Container, &name, &cid)).unwrap();
        }
        w.ids.insert(dep.to_string(), (iid, cid));
    }
    w
}

fn args(t: &T, extra: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = ["cleanup-releases", "--docker-socket"].iter().map(|s| s.to_string()).collect();
    v.push(t.socket());
    v.push("--resources-dir".into());
    v.push(t.resources());
    v.extend(extra.iter().map(|s| s.to_string()));
    v
}

fn image_present(t: &T, id: &str) -> bool {
    t.h.docker.with(|s| s.images.iter().any(|i| i.id == id))
}
fn container_present(t: &T, id: &str) -> bool {
    t.h.docker.with(|s: &mut St| s.containers.iter().any(|c| c.id == id))
}

#[test]
fn a_dry_run_changes_nothing_and_names_what_it_would_remove() {
    let t = T::new();
    let w = world(&t);
    let before = t.journal_bytes();
    let o = go(&t, &SHIPPING, &args(&t, &[]));
    assert_exit(&o, 0);
    assert!(o.stdout.contains(&format!("remove container hp-{}", w.old)), "{}", o.stdout);
    assert!(o.stdout.contains("would remove 1 container(s)"), "{}", o.stdout);
    assert!(!o.stdout.contains(&format!("hp-{}", w.serving)) || o.stdout.contains("protected"), "{}", o.stdout);
    assert_eq!(t.journal_bytes(), before, "a dry run never writes the journal");
    assert!(t.h.docker.requests().iter().all(|r| r.method != "DELETE"), "a dry run deletes nothing");
}

#[test]
fn apply_removes_superseded_and_failed_artifacts_and_never_the_serving_release() {
    let t = T::new();
    let w = world(&t);
    let o = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    let (old_i, old_c) = w.ids[&w.old.to_string()].clone();
    let (srv_i, srv_c) = w.ids[&w.serving.to_string()].clone();
    let (fail_i, _) = w.ids[&w.failed.to_string()].clone();
    assert!(!container_present(&t, &old_c) && !image_present(&t, &old_i), "the superseded release is gone");
    assert!(!image_present(&t, &fail_i), "the failed deployment's retained image is gone");
    assert!(container_present(&t, &srv_c) && image_present(&t, &srv_i), "the serving release is untouched");
    // the journal says so, and history stays
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    assert_eq!(r.deployments.len(), 3, "deployment history is kept");
    for e in r.ledger.entries().iter().filter(|e| e.deployment == w.old || e.deployment == w.failed) {
        assert!(e.is_released(), "{:?} is recorded as released", e.key);
    }
    assert!(r.ledger.entries().iter().filter(|e| e.deployment == w.serving).all(|e| !e.is_released()));
    // an older release is no longer a rollback target; the serving one is unaffected
    let retained = harpoon_deploy::rollback::retained_releases(&r.ledger, &r.deployments, &w.app);
    let old = retained.iter().find(|x| x.deployment == w.old).unwrap();
    assert!(old.problems.iter().any(|p| p.contains("retained image")), "{:?}", old.problems);
    // a second run has nothing left to do
    let again = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&again, 0);
    assert!(again.stdout.contains("removed 0 container(s)") && again.stdout.contains("removed 0 image(s)"), "{}", again.stdout);
}

#[test]
fn an_unfinished_deployment_stops_the_cleanup_before_anything_is_touched() {
    let t = T::new();
    let w = world(&t);
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        let (_, running) = ids(9);
        j.persist(&finished(&w.app, &running, DeploymentStatus::Running)).unwrap();
    }
    let o = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&o, 6);
    assert!(o.stderr.contains("unfinished") || o.stderr.contains("not finished") || o.stderr.contains("running"), "{}", o.stderr);
    let (old_i, old_c) = w.ids[&w.old.to_string()].clone();
    assert!(image_present(&t, &old_i) && container_present(&t, &old_c));
}

#[test]
fn an_image_the_serving_release_pins_is_kept_even_when_an_older_deployment_recorded_it() {
    let t = T::new();
    let w = world_with(&t, true);
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    let o = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    assert!(image_present(&t, &old_i), "{}", o.stdout);
    assert!(o.stdout.contains("the serving release pins this image"), "{}", o.stdout);
}
