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
    let mut w = W {
        app: app.clone(),
        old: old.clone(),
        serving: serving.clone(),
        failed: failed.clone(),
        ids: Default::default(),
    };
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    for (n, (dep, status, at)) in [
        (&old, DeploymentStatus::Succeeded, 10_000u64),
        (&serving, DeploymentStatus::Succeeded, 20_000),
        (&failed, DeploymentStatus::Failed, 30_000),
    ]
    .into_iter()
    .enumerate()
    {
        let mut d = finished(&app, dep, status);
        if status == DeploymentStatus::Succeeded {
            d.release = Some(ReleaseId::from_deployment(dep));
            if dep == &serving {
                let pinned = support::fake_iid(if serving_pins_the_old_image {
                    2000
                } else {
                    2001
                });
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
        let tag = format!(
            "harpoon/{}:{}-0123456789abcdef0123456789abcdef",
            app.as_str().to_ascii_lowercase(),
            dep
        );
        let iid = support::fake_iid(2000 + n as u64);
        let lbl = harpoon_deploy::backend::harpoon_labels(&app, dep);
        t.h.docker
            .with(|s| s.add_image_with_id(&iid, &[&tag], lbl.clone(), vec![]));
        j.record(&intent(
            &app,
            dep,
            ResourceKind::Image,
            &tag,
            Disposition::Retain,
        ))
        .unwrap();
        j.record(&receipt(dep, ResourceKind::Image, &tag, &iid))
            .unwrap();
        let mut cid = String::new();
        if status == DeploymentStatus::Succeeded {
            let name = format!("hp-{dep}");
            let running = dep == &serving;
            cid = t.h.docker.with(|s| s.add_container(&name, lbl, running));
            j.record(&intent(
                &app,
                dep,
                ResourceKind::Container,
                &name,
                Disposition::RemoveOnFailure,
            ))
            .unwrap();
            j.record(&receipt(dep, ResourceKind::Container, &name, &cid))
                .unwrap();
        }
        w.ids.insert(dep.to_string(), (iid, cid));
    }
    w
}

fn args(t: &T, extra: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = ["cleanup-releases", "--docker-socket"]
        .iter()
        .map(|s| s.to_string())
        .collect();
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
    t.h.docker
        .with(|s: &mut St| s.containers.iter().any(|c| c.id == id))
}

#[test]
fn a_dry_run_changes_nothing_and_names_what_it_would_remove() {
    let t = T::new();
    let w = world(&t);
    let before = t.journal_bytes();
    let o = go(&t, &SHIPPING, &args(&t, &[]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains(&format!("remove container hp-{}", w.old)),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("would remove 1 container(s)"),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains(&format!("hp-{}", w.serving)) || o.stdout.contains("protected"),
        "{}",
        o.stdout
    );
    assert_eq!(
        t.journal_bytes(),
        before,
        "a dry run never writes the journal"
    );
    assert!(
        t.h.docker.requests().iter().all(|r| r.method != "DELETE"),
        "a dry run deletes nothing"
    );
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
    assert!(
        !container_present(&t, &old_c) && !image_present(&t, &old_i),
        "the superseded release is gone"
    );
    assert!(
        !image_present(&t, &fail_i),
        "the failed deployment's retained image is gone"
    );
    assert!(
        container_present(&t, &srv_c) && image_present(&t, &srv_i),
        "the serving release is untouched"
    );
    // the journal says so, and history stays
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    assert_eq!(r.deployments.len(), 3, "deployment history is kept");
    for e in r
        .ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == w.old || e.deployment == w.failed)
    {
        assert!(e.is_released(), "{:?} is recorded as released", e.key);
    }
    assert!(r
        .ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == w.serving)
        .all(|e| !e.is_released()));
    // an older release is no longer a rollback target; the serving one is unaffected
    let retained = harpoon_deploy::rollback::retained_releases(&r.ledger, &r.deployments, &w.app);
    let old = retained.iter().find(|x| x.deployment == w.old).unwrap();
    assert!(
        old.problems.iter().any(|p| p.contains("retained image")),
        "{:?}",
        old.problems
    );
    // a second run has nothing left to do
    let again = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&again, 0);
    assert!(
        again.stdout.contains("removed 0 container(s)")
            && again.stdout.contains("removed 0 image(s)"),
        "{}",
        again.stdout
    );
}

#[test]
fn an_unfinished_deployment_stops_the_cleanup_before_anything_is_touched() {
    let t = T::new();
    let w = world(&t);
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        let (_, running) = ids(9);
        j.persist(&finished(&w.app, &running, DeploymentStatus::Running))
            .unwrap();
    }
    let o = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("unfinished")
            || o.stderr.contains("not finished")
            || o.stderr.contains("running"),
        "{}",
        o.stderr
    );
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
    assert!(
        o.stdout.contains("the serving release pins this image"),
        "{}",
        o.stdout
    );
}

fn cache_args(t: &T, extra: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = ["prune-build-cache", "--docker-socket"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    v.push(t.socket());
    v.extend(extra.iter().map(|s| s.to_string()));
    v
}

#[test]
fn the_build_cache_is_only_pruned_when_applied_and_confirmed_and_only_the_unused_part_goes() {
    let t = T::new();
    t.docker_runtime();
    t.h.docker
        .with(|s| s.build_cache = vec![(1_000, false), (500, true), (250, false)]);
    // a dry run reports the reclaimable size and sends no prune
    let o = go(&t, &SHIPPING, &cache_args(&t, &[]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("build-cache-reclaimable-bytes: 1250"),
        "{}",
        o.stdout
    );
    assert!(
        t.h.docker.requests().iter().all(|r| r.method != "POST"),
        "a dry run prunes nothing"
    );
    // applying without the authorization is refused before Docker is asked to change anything
    let refused = go(&t, &SHIPPING, &cache_args(&t, &["--apply"]));
    assert_ne!(refused.code, 0, "{}", refused.stdout);
    assert!(t.h.docker.requests().iter().all(|r| r.method != "POST"));
    // applied: Docker prunes, the in-use entry stays, and nothing but the cache is touched
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("build-cache-reclaimed-bytes: 1250"),
        "{}",
        o.stdout
    );
    assert_eq!(
        t.h.docker.with(|s| s.build_cache.clone()),
        vec![(500, true)]
    );
    assert!(
        t.h.docker.requests().iter().all(|r| r.method != "DELETE"),
        "no image or container was removed"
    );
}

// ---------------------------------------------------------------- the build-cache prune guard

fn posts(t: &T) -> usize {
    t.h.docker
        .requests()
        .iter()
        .filter(|r| r.method == "POST")
        .count()
}

fn seed_cache(t: &T) {
    t.h.docker
        .with(|s| s.build_cache = vec![(1_000, false), (500, true), (250, false)]);
}

fn with_deployment(t: &T, status: DeploymentStatus) {
    static N: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(40);
    let (app, dep) = ids(N.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&finished(&app, &dep, status)).unwrap();
}

fn legacy_journal(t: &T, lines: Option<usize>) {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ui/harpoon-desktop/src-tauri/tests/fixtures/b7-legacy-runtime/deploy-journal/journal.wal");
    let text = std::fs::read_to_string(fixture).unwrap();
    let kept: String = text
        .lines()
        .take(lines.unwrap_or(usize::MAX))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(t.journal_dir().join("journal.wal"), kept).unwrap();
}

#[test]
fn an_unfinished_deployment_refuses_the_prune_on_the_apply_path_and_nothing_is_pruned() {
    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    with_deployment(&t, DeploymentStatus::Running);
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("was not pruned") && o.stderr.contains("Nothing was changed"),
        "{}",
        o.stderr
    );
    assert_eq!(posts(&t), 0, "Docker was never asked to prune");
    assert_eq!(t.h.docker.with(|s| s.build_cache.len()), 3);
    // the dry run still answers, and says the apply would be refused
    let o = go(&t, &SHIPPING, &cache_args(&t, &[]));
    assert_exit(&o, 0);
    assert!(o.stdout.contains("prune-guard: blocked"), "{}", o.stdout);
    assert!(
        o.stdout.contains("build-cache-reclaimable-bytes: 1250"),
        "{}",
        o.stdout
    );
}

#[test]
fn the_guard_is_checked_when_applying_not_when_previewing() {
    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    with_deployment(&t, DeploymentStatus::Succeeded);
    let o = go(&t, &SHIPPING, &cache_args(&t, &[]));
    assert!(o.stdout.contains("prune-guard: clear"), "{}", o.stdout);
    // a deployment starts between the preview and the confirmation
    {
        let (app, dep) = ids(41);
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.persist(&finished(&app, &dep, DeploymentStatus::Queued))
            .unwrap();
    }
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 6);
    assert_eq!(posts(&t), 0);
}

#[test]
fn a_journal_with_only_finished_deployments_allows_the_prune_and_it_stays_all_false() {
    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    for s in [DeploymentStatus::Succeeded, DeploymentStatus::Failed] {
        with_deployment(&t, s);
    }
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    assert_eq!(
        t.h.docker.with(|s| s.build_cache.clone()),
        vec![(500, true)],
        "only the unused part went"
    );
    let reqs = t.h.docker.requests();
    let prunes: Vec<_> = reqs.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(prunes.len(), 1);
    assert!(
        prunes[0].path.starts_with("/build/prune") && prunes[0].path.contains("all=false"),
        "{}",
        prunes[0].path
    );
    assert!(
        reqs.iter().all(|r| r.method != "DELETE"),
        "no image, container or volume was removed"
    );
}

#[test]
fn a_genuinely_absent_journal_does_not_block_the_prune() {
    let t = T::new();
    t.docker_runtime(); // a runtime folder with no journal file of either kind
    seed_cache(&t);
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    assert_eq!(posts(&t), 1);
}

#[test]
fn a_legacy_journal_is_read_where_it_is_and_decides_in_the_same_way() {
    // a finished deployment: allowed
    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    legacy_journal(&t, None);
    let before = std::fs::read(t.journal_dir().join("journal.wal")).unwrap();
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    assert_eq!(posts(&t), 1);
    assert_eq!(
        std::fs::read(t.journal_dir().join("journal.wal")).unwrap(),
        before,
        "not rewritten"
    );
    assert!(
        !t.journal_dir().join("deployments.sqlite3").exists(),
        "not converted"
    );
    // the same journal cut while the deployment was still running: refused
    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    legacy_journal(&t, Some(3));
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("is running")
            || o.stderr.contains("not finished")
            || o.stderr.contains("run `recover`"),
        "{}",
        o.stderr
    );
    assert_eq!(posts(&t), 0);
}

#[test]
fn an_unreadable_or_ambiguous_journal_fails_closed_in_both_formats() {
    for (file, bytes) in [
        ("journal.wal", &b"this is not a journal"[..]),
        ("deployments.sqlite3", &b"not a database at all"[..]),
    ] {
        let t = T::new();
        t.docker_runtime();
        seed_cache(&t);
        std::fs::write(t.journal_dir().join(file), bytes).unwrap();
        let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
        assert_exit(&o, 6);
        assert!(
            o.stderr.contains("could not be read"),
            "{file}: {}",
            o.stderr
        );
        assert_eq!(posts(&t), 0, "{file}");
        assert_eq!(
            std::fs::read(t.journal_dir().join(file)).unwrap(),
            bytes,
            "{file} was not repaired"
        );
        let o = go(&t, &SHIPPING, &cache_args(&t, &[]));
        assert!(
            o.stdout.contains("prune-guard: blocked"),
            "{file}: {}",
            o.stdout
        );
    }
}

#[test]
fn a_journal_another_process_is_writing_refuses_the_prune() {
    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    with_deployment(&t, DeploymentStatus::Succeeded);
    let _writer = open_j(&t.journal_dir()).unwrap(); // a writer holds the store
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("holds the deployment journal"),
        "{}",
        o.stderr
    );
    assert_eq!(posts(&t), 0);
}

#[test]
fn the_confirmation_requirement_is_unchanged_by_the_guard() {
    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply"]));
    assert_ne!(o.code, 0);
    assert_eq!(posts(&t), 0);
}

// ---------------------------------------------------------------- ownership and classification

use harpoon_deploy::git::{CredKind, GitRecord, RepoRec, SelectorKind, SourceRec};
use harpoon_deploy_docker::json::Json;

/// The application's registration and a recorded source (commit) for each of its deployments.
fn register(t: &T, w: &W, name: &str) {
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    // the registration must precede the source records that name it, but the journal already holds the deployments
    j.git(&GitRecord::Repo(Box::new(RepoRec {
        name: name.into(),
        url: "https://git.example.com/acme/shop.git".into(),
        username: None,
        cred: CredKind::None,
        known_host: None,
        webhook: None,
        app: w.app.clone(),
        project: None,
        environment: None,
        branch: "main".into(),
        filters: vec![],
        container_port: 8080,
        dockerfile: None,
        context: None,
        target: None,
        health_path: None,
        settings: None,
        at_ms: 5,
    })))
    .unwrap();
    for (dep, sha) in [
        (&w.old, "aaaaaaa1111111111111111111111111111111aa"),
        (&w.serving, "bbbbbbb2222222222222222222222222222222bb"),
    ] {
        j.git(&GitRecord::Source(SourceRec {
            deployment: dep.clone(),
            repo: name.into(),
            url: "https://git.example.com/acme/shop.git".into(),
            selector: SelectorKind::Branch,
            requested: "main".into(),
            commit: sha.into(),
            trigger: "manual".into(),
            delivery: None,
            at_ms: 6,
        }))
        .unwrap();
    }
}

fn items(stdout: &str) -> Vec<Json> {
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix("item: "))
        .map(|l| Json::parse(l).expect("an item line is JSON"))
        .collect()
}
fn field<'a>(j: &'a Json, k: &str) -> &'a str {
    j.get(k).and_then(Json::as_str).unwrap_or("")
}
fn find<'a>(all: &'a [Json], dep: &DeploymentId, kind: &str) -> &'a Json {
    all.iter()
        .find(|i| field(i, "deployment") == dep.as_str() && field(i, "kind") == kind)
        .unwrap_or_else(|| panic!("no {kind} item for {dep}: {all:?}"))
}

#[test]
fn every_item_names_its_application_release_and_a_grounded_verdict() {
    let t = T::new();
    let w = world(&t);
    register(&t, &w, "shop");
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.record(&intent(
            &w.app,
            &w.old,
            ResourceKind::PortLease,
            "38000",
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
        j.record(&receipt(&w.old, ResourceKind::PortLease, "38000", "38000"))
            .unwrap();
    }
    let o = go(&t, &SHIPPING, &args(&t, &[]));
    assert_exit(&o, 0);
    let all = items(&o.stdout);
    let old_c = find(&all, &w.old, "container");
    assert_eq!(
        (
            field(old_c, "verdict"),
            field(old_c, "reason"),
            field(old_c, "appName"),
            field(old_c, "commit")
        ),
        ("ready", "superseded", "shop", "aaaaaaa")
    );
    assert_eq!(field(old_c, "environment"), "production");
    assert_eq!(old_c.get("physical"), Some(&Json::Bool(true)));
    let lease = find(&all, &w.old, "port-lease");
    assert_eq!(
        (field(lease, "verdict"), field(lease, "reason")),
        ("reconcile", "stale-port-lease")
    );
    assert_eq!(
        lease.get("physical"),
        Some(&Json::Null),
        "a lease has no Docker object, and is not said to"
    );
    let failed = find(&all, &w.failed, "image");
    assert_eq!(
        (field(failed, "verdict"), field(failed, "reason")),
        ("ready", "failed-attempt")
    );
    // the serving release is listed as protected, with the reason
    let srv = find(&all, &w.serving, "container");
    assert_eq!(
        (
            field(srv, "verdict"),
            field(srv, "reason"),
            field(srv, "commit")
        ),
        ("protected", "serving-release", "bbbbbbb")
    );
    assert_eq!(
        find(&all, &w.serving, "image")
            .get("verdict")
            .and_then(Json::as_str),
        Some("protected")
    );
}

#[test]
fn an_application_without_metadata_is_unassociated_never_guessed() {
    let t = T::new();
    let w = world(&t); // no registration at all
    let all = items(&go(&t, &SHIPPING, &args(&t, &[])).stdout);
    let c = find(&all, &w.old, "container");
    assert_eq!(c.get("appName"), Some(&Json::Null));
    assert_eq!(c.get("environment"), Some(&Json::Null));
    assert_eq!(c.get("commit"), Some(&Json::Null));
    assert_eq!(
        field(c, "app"),
        w.app.as_str(),
        "the journal's own application id is still given"
    );
}

#[test]
fn a_dry_run_judges_images_as_the_apply_will_after_the_containers_and_leases_are_released() {
    let t = T::new();
    let w = world(&t);
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.record(&intent(
            &w.app,
            &w.old,
            ResourceKind::PortLease,
            "38000",
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
        j.record(&receipt(&w.old, ResourceKind::PortLease, "38000", "38000"))
            .unwrap();
    }
    let dry = go(&t, &SHIPPING, &args(&t, &[]));
    assert!(
        !dry.stdout.contains("still has unreleased resources"),
        "the dry run must not hold an image back for a record the same run releases:\n{}",
        dry.stdout
    );
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    assert!(
        dry.stdout.contains(&format!("(id {old_i}) of {}", w.old)),
        "{}",
        dry.stdout
    );
    // one apply does all of it, and a second has nothing left (idempotent)
    let applied = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&applied, 0);
    assert!(!image_present(&t, &old_i));
    let again = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&again, 0);
    assert!(
        items(&again.stdout)
            .iter()
            .all(|i| field(i, "verdict") == "protected"),
        "{}",
        again.stdout
    );
}

#[test]
fn objects_docker_no_longer_has_are_reconciliation_not_removal() {
    let t = T::new();
    let w = world(&t);
    let (old_i, old_c) = w.ids[&w.old.to_string()].clone();
    t.h.docker.with(|s| {
        s.containers.retain(|c| c.id != old_c);
        s.images.retain(|i| i.id != old_i);
    });
    let dry = go(&t, &SHIPPING, &args(&t, &[]));
    let all = items(&dry.stdout);
    for kind in ["container", "image"] {
        let i = find(&all, &w.old, kind);
        assert_eq!(
            (field(i, "verdict"), field(i, "reason")),
            ("reconcile", "already-absent"),
            "{kind}"
        );
        assert_eq!(i.get("physical"), Some(&Json::Bool(false)));
    }
    // only the failed deployment's image is physically present; the absent container and image claim nothing
    assert!(
        dry.stdout.contains("reclaimed-containers: 0")
            && dry.stdout.contains("reclaimed-images: 1"),
        "{}",
        dry.stdout
    );
    let applied = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&applied, 0);
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    assert!(
        r.ledger
            .entries()
            .iter()
            .filter(|e| e.deployment == w.old)
            .all(|e| e.is_released()),
        "the journal no longer lists them"
    );
    let deletes =
        t.h.docker
            .requests()
            .iter()
            .filter(|r| r.method == "DELETE")
            .count();
    assert_eq!(
        deletes, 1,
        "only the failed deployment's present image is deleted; nothing absent is"
    );
}
