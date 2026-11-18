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
    world_full(t, serving_pins_the_old_image, None)
}

fn pin(image_id: String, host_port: u16) -> harpoon_deploy::release::ReleaseConfig {
    harpoon_deploy::release::ReleaseConfig {
        image_id,
        image_tag: "harpoon/x:y".into(),
        container_port: 8080,
        host_port,
        health: harpoon_deploy::health::HealthSpec::first_test("/"),
        memory_bytes: 0,
        nano_cpus: 0,
        pids: 0,
        env: vec![],
        secrets: vec![],
        snapshot: None,
        build: None,
    }
}

/// `old_pin`: the old release also pins an image (the seed of its id; 2000 is the one the ledger retained), so the journal can
/// prove it is a rollback target.
fn world_full(t: &T, serving_pins_the_old_image: bool, old_pin: Option<u64>) -> W {
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
            if let (Some(seed), true) = (old_pin, dep == &old) {
                d.pinned = Some(pin(support::fake_iid(seed), 38001));
            }
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

#[test]
fn shared_cache_records_are_never_advertised_as_reclaimable_and_the_prune_frees_nothing() {
    let t = T::new();
    t.docker_runtime();
    // two big records shared with images, one unused unshared record, one in use
    t.h.docker.with(|s| {
        s.build_cache = vec![
            (1_450_000_000, false),
            (233_000_000, false),
            (40, false),
            (500, true),
        ];
        s.build_cache_shared = vec![true, true, false, false];
    });
    let o = go(&t, &SHIPPING, &cache_args(&t, &[]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("build-cache-reclaimable-bytes: 40"),
        "only the unshared unused record: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains("build-cache-total-bytes: 1683000540"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("build-cache-shared-bytes: 1683000000"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("build-cache-in-use-bytes: 500"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("build-cache-prunable-records: 1"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("share layers with an image"),
        "{}",
        o.stdout
    );
    // applied: Docker removes what it can; the shared records stay
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("build-cache-reclaimed-bytes: 40"),
        "{}",
        o.stdout
    );
    assert_eq!(t.h.docker.with(|s| s.build_cache.len()), 3);
}

#[test]
fn when_every_record_is_shared_or_in_use_the_prune_says_it_will_free_about_nothing_and_does() {
    let t = T::new();
    t.docker_runtime();
    t.h.docker.with(|s| {
        s.build_cache = vec![(1_450_000_000, false), (233_000_000, false)];
        s.build_cache_shared = vec![true, true];
    });
    let o = go(&t, &SHIPPING, &cache_args(&t, &[]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("build-cache-reclaimable-bytes: 0"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("build-cache-prunable-records: 0"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("expected to free about 0 bytes"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("only removing releases (cleanup-releases) can free it"),
        "{}",
        o.stdout
    );
    let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("build-cache-reclaimed-bytes: 0"),
        "a zero-byte prune is a zero, not an error: {}",
        o.stdout
    );
    assert_eq!(
        t.h.docker.with(|s| s.build_cache.len()),
        2,
        "nothing was removed"
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
fn no_deployment_writer_can_start_between_the_check_and_the_prune_in_either_journal_format() {
    use std::sync::{Arc, Mutex};
    use support::fake_docker::Action;
    for legacy in [false, true] {
        let t = T::new();
        t.docker_runtime();
        seed_cache(&t);
        if legacy {
            legacy_journal(&t, None);
        } else {
            with_deployment(&t, DeploymentStatus::Succeeded);
        }
        let dir = t.journal_dir();
        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let (s2, d2) = (seen.clone(), dir.clone());
        // At the instant Docker is asked to prune, a second Harpoon writer tries to start (a deployment from a terminal, say).
        t.h.docker.fault(
            Some("POST"),
            "/build/prune",
            1,
            Action::Race(Arc::new(move |_| {
                *s2.lock().unwrap() =
                    Some(match harpoon_deploy_journal::SqliteJournal::open(&d2) {
                        Ok(_) => "a second writer started during the prune".to_string(),
                        Err(e) => e.to_string(),
                    });
            })),
        );
        let o = go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()]));
        assert_exit(&o, 0);
        let got = seen
            .lock()
            .unwrap()
            .clone()
            .expect("the prune request reached Docker");
        assert!(
            got.contains("another Harpoon process holds"),
            "legacy={legacy}: {got}"
        );
        assert_eq!(posts(&t), 1);
        // the hold ended with the command: a writer can start now, and the journal itself was never written by the hold
        assert!(
            harpoon_deploy_journal::hold_writer_lock(&dir).is_ok(),
            "legacy={legacy}: the lock was released"
        );
        if legacy {
            assert!(
                !dir.join("deployments.sqlite3").exists(),
                "the legacy journal was not converted"
            );
        }
    }
}

#[test]
fn a_preview_holds_no_lock() {
    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    with_deployment(&t, DeploymentStatus::Succeeded);
    let o = go(&t, &SHIPPING, &cache_args(&t, &[]));
    assert_exit(&o, 0);
    assert!(harpoon_deploy_journal::hold_writer_lock(&t.journal_dir()).is_ok());
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

// ---------------------------------------------------------------- the real output contracts, captured for the desktop

/// What the real commands print and exit with, for each scenario the desktop's result panel has to read. The desktop's tests run its parser
/// on exactly these captures (after the desktop's own response mapping), so its fixtures cannot drift from the engine. Paths that vary
/// from run to run are replaced by placeholders. Regenerate with `WRITE_CLEANUP_FIXTURES=1 cargo test --test cli cleanup_output_contracts`.
fn capture(t: &T, name: &str, o: &Outcome, into: &mut Vec<(String, Json)>) {
    use harpoon_deploy_docker::json::{n, obj, s};
    let scrub = |x: &str| {
        x.replace(&t.socket(), "<docker-socket>")
            .replace(&t.rt.display().to_string(), "<runtime>")
            .replace(&t.resources(), "<resources>")
            .replace(&t.mgmt.path().display().to_string(), "<mgmt-socket>")
    };
    into.push((
        name.to_string(),
        obj(vec![
            ("code", n(o.code as i64)),
            ("stdout", s(scrub(&o.stdout))),
            ("stderr", s(scrub(&o.stderr))),
        ]),
    ));
}

#[test]
fn cleanup_output_contracts_match_the_desktops_fixtures() {
    use harpoon_deploy_docker::json::obj;
    use support::fake_docker::Action;
    let mut all: Vec<(String, Json)> = Vec::new();
    let mgmt = |t: &T| {
        vec![
            "--mgmt-socket".to_string(),
            t.mgmt.path().display().to_string(),
        ]
    };
    let with_mgmt = |t: &T, mut a: Vec<String>| {
        a.extend(mgmt(t));
        a
    };

    // ---- cleanup-releases
    let t = T::new();
    world(&t);
    capture(
        &t,
        "releases-dry-run",
        &go(&t, &SHIPPING, &args(&t, &[])),
        &mut all,
    );
    let t2 = T::new();
    world_full(&t2, false, Some(2000));
    capture(
        &t2,
        "releases-dry-run-rollback-target",
        &go(&t2, &SHIPPING, &args(&t2, &[])),
        &mut all,
    );
    // an interrupted-and-restored deployment whose leftovers share layers with the protected rollback image
    let t3 = T::new();
    let w3 = world_full(&t3, false, Some(2000));
    let r3 = restored_interrupted(&t3, &w3, RecoveryOutcome::Restored, true, false);
    share_layers(&t3, &w3, &r3);
    capture(
        &t3,
        "releases-dry-run-abandoned-restoration",
        &go(&t3, &SHIPPING, &args(&t3, &[])),
        &mut all,
    );
    capture(
        &t3,
        "releases-apply-abandoned-restoration",
        &go(
            &t3,
            &SHIPPING,
            &with_mgmt(&t3, args(&t3, &["--apply", &auth()])),
        ),
        &mut all,
    );
    capture(
        &t,
        "releases-apply-refused-unauthorized",
        &go(&t, &SHIPPING, &args(&t, &["--apply"])),
        &mut all,
    );
    capture(
        &t,
        "releases-apply-success",
        &go(
            &t,
            &SHIPPING,
            &with_mgmt(&t, args(&t, &["--apply", &auth()])),
        ),
        &mut all,
    );
    capture(
        &t,
        "releases-apply-nothing-left",
        &go(
            &t,
            &SHIPPING,
            &with_mgmt(&t, args(&t, &["--apply", &auth()])),
        ),
        &mut all,
    );
    capture(
        &t,
        "releases-dry-run-nothing-left",
        &go(&t, &SHIPPING, &args(&t, &[])),
        &mut all,
    );

    let t = T::new();
    world(&t);
    t.h.docker.fault(
        Some("DELETE"),
        "/images/",
        usize::MAX,
        Action::Respond(409, r#"{"message":"conflict: unable to delete, image is referenced in multiple repositories"}"#.into()),
    );
    capture(
        &t,
        "releases-apply-partial-image-kept",
        &go(
            &t,
            &SHIPPING,
            &with_mgmt(&t, args(&t, &["--apply", &auth()])),
        ),
        &mut all,
    );

    let t = T::new();
    let w = world(&t);
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        let (_, running) = ids(9);
        j.persist(&finished(&w.app, &running, DeploymentStatus::Running))
            .unwrap();
    }
    capture(
        &t,
        "releases-apply-refused-unfinished",
        &go(&t, &SHIPPING, &args(&t, &["--apply", &auth()])),
        &mut all,
    );

    let t = T::new();
    world(&t);
    let mut a = args(&t, &["--apply", &auth()]);
    let i = a.iter().position(|x| x == &t.socket()).unwrap();
    a[i] = "/nonexistent/docker.sock".into();
    capture(
        &t,
        "releases-apply-engine-unreachable",
        &go(&t, &SHIPPING, &a),
        &mut all,
    );

    // ---- prune-build-cache
    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    capture(
        &t,
        "cache-preview-clear",
        &go(&t, &SHIPPING, &with_mgmt(&t, cache_args(&t, &[]))),
        &mut all,
    );
    capture(
        &t,
        "cache-apply-refused-unauthorized",
        &go(&t, &SHIPPING, &cache_args(&t, &["--apply"])),
        &mut all,
    );
    capture(
        &t,
        "cache-apply-success",
        &go(
            &t,
            &SHIPPING,
            &with_mgmt(&t, cache_args(&t, &["--apply", &auth()])),
        ),
        &mut all,
    );
    capture(
        &t,
        "cache-apply-success-guest-unmeasured",
        &{
            let t2 = T::new();
            t2.docker_runtime();
            seed_cache(&t2);
            go(&t2, &SHIPPING, &cache_args(&t2, &["--apply", &auth()]))
        },
        &mut all,
    );

    // the real guest's shape: everything unused, everything shared with an image: a prune frees about nothing
    let t = T::new();
    t.docker_runtime();
    t.h.docker.with(|s| {
        s.build_cache = vec![(1_450_000_000, false), (233_000_000, false)];
        s.build_cache_shared = vec![true, true];
    });
    capture(
        &t,
        "cache-preview-all-shared",
        &go(&t, &SHIPPING, &with_mgmt(&t, cache_args(&t, &[]))),
        &mut all,
    );
    capture(
        &t,
        "cache-apply-all-shared-frees-nothing",
        &go(
            &t,
            &SHIPPING,
            &with_mgmt(&t, cache_args(&t, &["--apply", &auth()])),
        ),
        &mut all,
    );

    let t = T::new();
    t.docker_runtime();
    t.h.docker.with(|s| s.build_cache = vec![(500, true)]);
    capture(
        &t,
        "cache-preview-nothing-unused",
        &go(&t, &SHIPPING, &with_mgmt(&t, cache_args(&t, &[]))),
        &mut all,
    );
    capture(
        &t,
        "cache-apply-nothing-unused",
        &go(
            &t,
            &SHIPPING,
            &with_mgmt(&t, cache_args(&t, &["--apply", &auth()])),
        ),
        &mut all,
    );

    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    {
        // a fixed id: the shared counter in `with_deployment` depends on which tests ran first, and the capture must not
        let (app, dep) = ids(40);
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.persist(&finished(&app, &dep, DeploymentStatus::Running))
            .unwrap();
    }
    capture(
        &t,
        "cache-preview-blocked-unfinished",
        &go(&t, &SHIPPING, &cache_args(&t, &[])),
        &mut all,
    );
    capture(
        &t,
        "cache-apply-refused-unfinished",
        &go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()])),
        &mut all,
    );

    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    t.h.docker.fault(
        Some("POST"),
        "/build/prune",
        1,
        Action::Respond(
            500,
            r#"{"message":"prune failed: buildkit is not available"}"#.into(),
        ),
    );
    capture(
        &t,
        "cache-apply-failed-docker-error",
        &go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()])),
        &mut all,
    );

    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    t.h.docker
        .fault(Some("POST"), "/build/prune", 1, Action::Disconnect);
    capture(
        &t,
        "cache-apply-connection-lost",
        &go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()])),
        &mut all,
    );

    let t = T::new();
    t.docker_runtime();
    seed_cache(&t);
    t.h.docker
        .fault(Some("POST"), "/build/prune", 1, Action::ApplyThenDrop);
    capture(
        &t,
        "cache-apply-reply-lost-after-prune",
        &go(&t, &SHIPPING, &cache_args(&t, &["--apply", &auth()])),
        &mut all,
    );

    let now = obj(all.iter().map(|(k, v)| (k.as_str(), v.clone())).collect()).to_json();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ui/harpoon-desktop/src-tauri/tests/fixtures/cleanup-outputs/raw.json");
    if std::env::var_os("WRITE_CLEANUP_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &now).unwrap();
    }
    let committed =
        std::fs::read_to_string(&path).expect("fixture missing: run with WRITE_CLEANUP_FIXTURES=1");
    assert_eq!(committed, now, "the captured command outputs changed: regenerate with WRITE_CLEANUP_FIXTURES=1 and review the diff");
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

// ---------------------------------------------------------------- rollback retention

fn dry(t: &T) -> Outcome {
    let o = go(t, &SHIPPING, &args(t, &[]));
    assert_exit(&o, 0);
    o
}
fn apply(t: &T) -> Outcome {
    let o = go(t, &SHIPPING, &args(t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    o
}
fn reason_of<'a>(its: &'a [Json], dep: &DeploymentId, kind: &str) -> (&'a str, &'a str) {
    let i = find(its, dep, kind);
    (field(i, "verdict"), field(i, "reason"))
}

#[test]
fn the_most_recent_proven_restorable_previous_release_is_protected_and_labelled_apart_from_serving_and_disposable(
) {
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let o = dry(&t);
    let its = items(&o.stdout);
    // three different answers for three kinds of release
    assert_eq!(
        reason_of(&its, &w.serving, "image"),
        ("protected", "serving-release")
    );
    assert_eq!(
        reason_of(&its, &w.old, "image"),
        ("protected", "rollback-target")
    );
    assert_eq!(
        reason_of(&its, &w.old, "container"),
        ("protected", "rollback-target")
    );
    assert_eq!(
        reason_of(&its, &w.failed, "image").0,
        "ready",
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains(&format!("protected: deployment {} (rollback target", w.old)),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains(&format!(
            "serving {}; rollback target {}; 0 other release(s) eligible for cleanup",
            w.serving, w.old
        )),
        "{}",
        o.stdout
    );
    // applying removes only the disposable one
    apply(&t);
    let (old_i, old_c) = w.ids[&w.old.to_string()].clone();
    let (srv_i, srv_c) = w.ids[&w.serving.to_string()].clone();
    let (fail_i, _) = w.ids[&w.failed.to_string()].clone();
    assert!(
        image_present(&t, &old_i) && container_present(&t, &old_c),
        "the rollback target is kept"
    );
    assert!(
        image_present(&t, &srv_i) && container_present(&t, &srv_c),
        "the serving release is kept"
    );
    assert!(!image_present(&t, &fail_i), "the failed attempt is removed");
    // the journal still offers the rollback
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    let kept = harpoon_deploy::rollback::retained_releases(&r.ledger, &r.deployments, &w.app);
    assert!(kept
        .iter()
        .find(|x| x.deployment == w.old)
        .unwrap()
        .eligible());
}

#[test]
fn repeating_the_cleanup_in_any_order_keeps_the_same_protection() {
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    let first = dry(&t);
    apply(&t);
    let again = apply(&t);
    let third = dry(&t);
    for o in [&first, &again, &third] {
        assert!(
            o.stdout
                .contains(&format!("protected: deployment {} (rollback target", w.old)),
            "{}",
            o.stdout
        );
    }
    assert!(
        again.stdout.contains("removed 0 image(s)"),
        "{}",
        again.stdout
    );
    assert!(image_present(&t, &old_i));
    assert_eq!(
        items(&third.stdout).len(),
        items(&again.stdout).len(),
        "a repeat reports the same protected set"
    );
}

#[test]
fn a_release_without_pinned_configuration_is_not_a_rollback_target_and_is_cleaned_as_before() {
    let t = T::new();
    let w = world_full(&t, false, None); // the old release never pinned an image: missing metadata
    let o = dry(&t);
    assert!(!o.stdout.contains("rollback target"), "{}", o.stdout);
    assert!(
        o.stdout
            .contains("no previous release is provably restorable"),
        "{}",
        o.stdout
    );
    assert_eq!(reason_of(&items(&o.stdout), &w.old, "image").0, "ready");
}

#[test]
fn rollback_eligibility_that_cannot_be_shown_keeps_the_image_and_says_why() {
    // Docker's image does not carry the deployment's labels: ownership is not provable
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let (old_i, old_c) = w.ids[&w.old.to_string()].clone();
    t.h.docker.with(|s| {
        s.images.iter_mut().find(|i| i.id == old_i).unwrap().labels = Vec::new();
    });
    let o = apply(&t);
    assert!(
        image_present(&t, &old_i) && container_present(&t, &old_c),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("rollback eligibility could not be verified"),
        "{}",
        o.stdout
    );
    assert_eq!(
        reason_of(&items(&o.stdout), &w.old, "image"),
        ("protected", "rollback-unverified")
    );
}

#[test]
fn a_journal_image_that_is_not_the_pinned_image_is_ambiguous_and_kept() {
    let t = T::new();
    // the release pins one image id while the ledger retained another
    let w = world_full(&t, false, Some(9999));
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    let o = apply(&t);
    assert!(image_present(&t, &old_i), "{}", o.stdout);
    assert!(
        o.stdout.contains("not the image the release pins"),
        "{}",
        o.stdout
    );
}

#[test]
fn an_image_docker_no_longer_has_protects_nothing_and_is_reconciled_not_deleted() {
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    t.h.docker.with(|s| s.images.retain(|i| i.id != old_i));
    let o = apply(&t);
    assert!(!o.stdout.contains("rollback target"), "{}", o.stdout);
    assert_eq!(
        reason_of(&items(&o.stdout), &w.old, "image").1,
        "already-absent"
    );
    let (srv_i, _) = w.ids[&w.serving.to_string()].clone();
    assert!(image_present(&t, &srv_i));
}

#[test]
fn two_restorable_releases_that_cannot_be_ordered_are_both_kept() {
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    // a third release, restorable and finished at the very same moment as the old one
    let app = w.app.clone();
    let (_, extra) = ids(7);
    let tag = format!(
        "harpoon/{}:{}-0123456789abcdef0123456789abcdef",
        app.as_str().to_ascii_lowercase(),
        extra
    );
    let iid = support::fake_iid(2500);
    let lbl = harpoon_deploy::backend::harpoon_labels(&app, &extra);
    t.h.docker
        .with(|s| s.add_image_with_id(&iid, &[&tag], lbl, vec![]));
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    let mut d = finished(&app, &extra, DeploymentStatus::Succeeded);
    d.release = Some(ReleaseId::from_deployment(&extra));
    d.pinned = Some(pin(iid.clone(), 38002));
    d.finished_ms = Some(10_000);
    j.persist(&d).unwrap();
    j.record(&intent(
        &app,
        &extra,
        ResourceKind::Image,
        &tag,
        Disposition::Retain,
    ))
    .unwrap();
    j.record(&receipt(&extra, ResourceKind::Image, &tag, &iid))
        .unwrap();
    drop(j);
    let o = apply(&t);
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    assert!(
        image_present(&t, &old_i) && image_present(&t, &iid),
        "neither is guessed away: {}",
        o.stdout
    );
    assert!(o.stdout.contains("cannot be told"), "{}", o.stdout);
}

#[test]
fn an_interrupted_deployment_that_records_or_uses_the_rollback_image_cannot_cause_its_removal() {
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    // an interrupted deployment of the same application recorded the very same image id and left a container made from it
    let app = w.app.clone();
    let (_, intr) = ids(8);
    let lbl = harpoon_deploy::backend::harpoon_labels(&app, &intr);
    let name = format!("hp-{intr}-restored");
    let cid = t.h.docker.with(|s| {
        let id = s.add_container(&name, lbl, false);
        s.containers.iter_mut().find(|c| c.id == id).unwrap().image = old_i.clone();
        id
    });
    let tag = format!(
        "harpoon/{}:{}-0123456789abcdef0123456789abcdef",
        app.as_str().to_ascii_lowercase(),
        intr
    );
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    let mut d = finished(&app, &intr, DeploymentStatus::Interrupted);
    d.finished_ms = Some(40_000);
    j.persist(&d).unwrap();
    j.record(&intent(
        &app,
        &intr,
        ResourceKind::Image,
        &tag,
        Disposition::Retain,
    ))
    .unwrap();
    j.record(&receipt(&intr, ResourceKind::Image, &tag, &old_i))
        .unwrap();
    drop(j);
    let o = apply(&t);
    assert!(
        image_present(&t, &old_i),
        "the rollback image survives: {}",
        o.stdout
    );
    assert!(container_present(&t, &cid), "{}", o.stdout);
    let (srv_i, _) = w.ids[&w.serving.to_string()].clone();
    assert!(image_present(&t, &srv_i));
}

// ---------------------------------------------------------------- interrupted and restored deployments

use harpoon_deploy::control::{Choice, ControlRecord, ResumeRec};
use harpoon_deploy::state::{CutoverState, Recovery, RecoveryOutcome};

struct Restored {
    dep: DeploymentId,
    image: String,
    container: String,
}

/// A deployment whose cutover began, which restored the old release in a container of its own (made from the OLD release's
/// image, as the real restoration does) and left an image of its own behind: what an interrupted-and-restored deployment leaves.
fn restored_interrupted(
    t: &T,
    w: &W,
    outcome: RecoveryOutcome,
    cleared: bool,
    running: bool,
) -> Restored {
    restored_interrupted_at(t, w, outcome, cleared, running, 15_000)
}

/// `restored_at`: when the restoration finished. After the serving release finished (20_000), the restored container is what
/// serves the application.
fn restored_interrupted_at(
    t: &T,
    w: &W,
    outcome: RecoveryOutcome,
    cleared: bool,
    running: bool,
    restored_at: u64,
) -> Restored {
    let app = w.app.clone();
    let (_, dep) = ids(9);
    let (old_image, _) = w.ids[&w.old.to_string()].clone();
    let lbl = harpoon_deploy::backend::harpoon_labels(&app, &dep);
    let tag = format!(
        "harpoon/{}:{}-0123456789abcdef0123456789abcdef",
        app.as_str().to_ascii_lowercase(),
        dep
    );
    let image = support::fake_iid(3000);
    t.h.docker
        .with(|s| s.add_image_with_id(&image, &[&tag], lbl.clone(), vec![]));
    let name = format!("hp-{dep}-restored");
    let container = t.h.docker.with(|s| {
        let id = s.add_container(&name, lbl, running);
        s.containers.iter_mut().find(|c| c.id == id).unwrap().image = old_image.clone();
        id
    });
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    let mut d = finished(&app, &dep, DeploymentStatus::Interrupted);
    d.cutover = CutoverState::Begun { at_ms: 12_000 };
    d.replaces = Some(ReleaseId::from_deployment(&w.old));
    d.recovery = Some(Recovery {
        outcome,
        started_ms: 12_500,
        finished_ms: Some(restored_at),
        budget_ms: 60_000,
    });
    d.finished_ms = Some(15_000);
    j.persist(&d).unwrap();
    j.record(&intent(
        &app,
        &dep,
        ResourceKind::Image,
        &tag,
        Disposition::Retain,
    ))
    .unwrap();
    j.record(&receipt(&dep, ResourceKind::Image, &tag, &image))
        .unwrap();
    j.record(&intent(
        &app,
        &dep,
        ResourceKind::Container,
        &name,
        Disposition::Retain,
    ))
    .unwrap();
    j.record(&receipt(&dep, ResourceKind::Container, &name, &container))
        .unwrap();
    if cleared {
        j.control(&ControlRecord::Resume(ResumeRec {
            app,
            at_ms: 16_000,
            choice: Choice::Dismiss,
            selected: None,
            dismissed: vec![],
            superseded: vec![],
            cleared: vec![dep.clone()],
        }))
        .unwrap();
    }
    Restored {
        dep,
        image,
        container,
    }
}

fn set_layers(t: &T, id: &str, size: u64, layers: &[&str]) {
    t.h.docker.with(|s| {
        let i = s.images.iter_mut().find(|i| i.id == id).unwrap();
        i.size = size;
        i.layers = layers.iter().map(|l| l.to_string()).collect();
    });
}

fn mgmt(t: &T, mut a: Vec<String>) -> Vec<String> {
    a.extend([
        "--mgmt-socket".to_string(),
        t.mgmt.path().display().to_string(),
    ]);
    a
}

/// Layers as the real guest has them: one base image, and application images built on it. The leftover image of the interrupted
/// deployment has exactly the rollback image's layers.
fn share_layers(t: &T, w: &W, r: &Restored) {
    t.h.docker.with(|s| {
        s.add_image_with_id("sha256:base", &["node:22-slim"], Default::default(), vec![])
    });
    set_layers(t, "sha256:base", 250, &["b1", "b2"]);
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    let (srv_i, _) = w.ids[&w.serving.to_string()].clone();
    let (fail_i, _) = w.ids[&w.failed.to_string()].clone();
    set_layers(t, &old_i, 1_940, &["b1", "b2", "r1", "r2"]);
    set_layers(t, &r.image, 1_940, &["b1", "b2", "r1", "r2"]);
    set_layers(t, &srv_i, 1_950, &["b1", "b2", "s1", "s2"]);
    set_layers(t, &fail_i, 80, &["f1"]);
}

#[test]
fn a_settled_restoration_is_cleaned_while_the_serving_release_and_the_rollback_target_stay() {
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let r = restored_interrupted(&t, &w, RecoveryOutcome::Restored, true, false);
    share_layers(&t, &w, &r);
    let (old_i, old_c) = w.ids[&w.old.to_string()].clone();
    let (srv_i, srv_c) = w.ids[&w.serving.to_string()].clone();
    let o = dry(&t);
    let its = items(&o.stdout);
    assert_eq!(
        reason_of(&its, &r.dep, "container"),
        ("ready", "abandoned-restoration"),
        "{}",
        o.stdout
    );
    assert_eq!(reason_of(&its, &r.dep, "image").0, "ready", "{}", o.stdout);
    assert_eq!(
        reason_of(&its, &w.old, "image"),
        ("protected", "rollback-target")
    );
    assert_eq!(
        reason_of(&its, &w.serving, "image"),
        ("protected", "serving-release")
    );
    // layers: the leftover has the rollback image's layers, so removing it frees nothing by itself, and the failed attempt's
    // own image is the only thing that frees space
    assert!(
        o.stdout.contains(&format!(
            "shared-layers: image {} has the same layers as protected image {old_i}",
            r.image
        )),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("estimated-reclaim-bytes: 80\n"),
        "only the unshared image counts: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains("estimated-reclaim-exact: true"),
        "{}",
        o.stdout
    );
    // nothing was changed by the dry run
    assert!(container_present(&t, &r.container) && image_present(&t, &r.image));
    // applied
    let o = go(&t, &SHIPPING, &mgmt(&t, args(&t, &["--apply", &auth()])));
    assert_exit(&o, 0);
    assert!(
        !container_present(&t, &r.container),
        "the abandoned restoration container is removed: {}",
        o.stdout
    );
    assert!(!image_present(&t, &r.image), "{}", o.stdout);
    assert!(
        image_present(&t, &old_i) && container_present(&t, &old_c),
        "the rollback target stays"
    );
    assert!(
        image_present(&t, &srv_i) && container_present(&t, &srv_c),
        "the serving release stays"
    );
    // removed references are counted apart from space: a measured figure and the reason it can be less than the references
    assert!(
        o.stdout.contains("measured-reclaimed-bytes:"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("reclaim-check:"), "{}", o.stdout);
    // the journal says so and history stays
    let j = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    assert!(j
        .ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == r.dep)
        .all(|e| e.is_released()));
    assert!(
        j.deployments.iter().any(|d| d.id == r.dep),
        "the deployment record is kept"
    );
    let kept = harpoon_deploy::rollback::retained_releases(&j.ledger, &j.deployments, &w.app);
    assert!(
        kept.iter()
            .find(|x| x.deployment == w.old)
            .unwrap()
            .eligible(),
        "rollback still offered"
    );
}

#[test]
fn removing_every_image_that_holds_a_layer_set_is_estimated_by_its_layers_not_by_how_many_references_go(
) {
    // the rollback target is not proven (it pinned no configuration), so it and the leftover are both disposable and the
    // two images with the same layers free their content once, minus the base they share with an image that stays
    let t = T::new();
    let w = world_full(&t, false, None);
    let r = restored_interrupted(&t, &w, RecoveryOutcome::Restored, true, false);
    share_layers(&t, &w, &r);
    let o = dry(&t);
    assert!(
        o.stdout
            .contains(&format!("estimated-reclaim-bytes: {}", 1_940 - 250 + 80)),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("estimated-reclaim-exact: true"),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains("shared-layers:"),
        "nothing protected shares them: {}",
        o.stdout
    );
}

#[test]
fn an_unsettled_restoration_is_never_touched() {
    // not verified, failed, not attempted, still running, or settled but its pause was never cleared
    for (outcome, cleared) in [
        (RecoveryOutcome::RestoredUnverified, true),
        (RecoveryOutcome::InProgress, true),
        (RecoveryOutcome::Restored, false),
    ] {
        let t = T::new();
        let w = world_full(&t, false, Some(2000));
        let r = restored_interrupted(&t, &w, outcome, cleared, false);
        let o = apply(&t);
        assert!(
            container_present(&t, &r.container) && image_present(&t, &r.image),
            "{outcome:?} cleared={cleared}: {}",
            o.stdout
        );
        assert!(!items(&o.stdout).iter().any(|i| field(i, "deployment") == r.dep.as_str() && field(i, "verdict") == "ready"), "{}", o.stdout);
    }
}

#[test]
fn a_restoration_container_that_is_running_or_not_provably_harpoons_is_kept_and_says_why() {
    // running: it may be serving something
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let r = restored_interrupted(&t, &w, RecoveryOutcome::Restored, true, true);
    let o = apply(&t);
    assert!(container_present(&t, &r.container), "{}", o.stdout);
    assert!(
        o.stdout
            .contains(&format!("kept: container hp-{}-restored", r.dep)),
        "{}",
        o.stdout
    );
    assert_eq!(
        reason_of(&items(&o.stdout), &r.dep, "container"),
        ("review", "needs-review")
    );
    // labels that do not match: not Harpoon's by proof
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let r = restored_interrupted(&t, &w, RecoveryOutcome::Restored, true, false);
    t.h.docker.with(|s| {
        s.containers
            .iter_mut()
            .find(|c| c.id == r.container)
            .unwrap()
            .labels = Vec::new()
    });
    let o = apply(&t);
    assert!(container_present(&t, &r.container), "{}", o.stdout);
    assert_eq!(
        reason_of(&items(&o.stdout), &r.dep, "container").0,
        "review"
    );
}

#[test]
fn an_image_the_interrupted_deployment_shares_with_the_rollback_target_is_never_removed() {
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let r = restored_interrupted(&t, &w, RecoveryOutcome::Restored, true, false);
    // the leftover's image is, by id, the rollback image itself (one image, two journal records)
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        let tag = format!(
            "harpoon/{}:{}-0123456789abcdef0123456789abcdef",
            w.app.as_str().to_ascii_lowercase(),
            r.dep
        );
        j.record(&intent(
            &w.app,
            &r.dep,
            ResourceKind::Image,
            &format!("{tag}-twin"),
            Disposition::Retain,
        ))
        .unwrap();
        j.record(&receipt(
            &r.dep,
            ResourceKind::Image,
            &format!("{tag}-twin"),
            &old_i,
        ))
        .unwrap();
    }
    let o = apply(&t);
    assert!(
        image_present(&t, &old_i),
        "the rollback image survives a second record of the same id: {}",
        o.stdout
    );
    assert!(
        !container_present(&t, &r.container),
        "the abandoned container still goes: {}",
        o.stdout
    );
}

#[test]
fn an_interrupted_cleanup_is_finished_by_the_next_run_and_a_lost_reply_is_reconciled_not_claimed() {
    // the removal request is dropped: Docker never sees it
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let r = restored_interrupted(&t, &w, RecoveryOutcome::Restored, true, false);
    t.h.docker.fault(
        Some("DELETE"),
        &format!("/containers/{}", r.container),
        1,
        support::fake_docker::Action::Disconnect,
    );
    let first = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert!(container_present(&t, &r.container), "{}", first.stdout);
    let again = apply(&t);
    assert!(
        !container_present(&t, &r.container),
        "the next run finishes it: {}",
        again.stdout
    );
    let j = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    assert!(j
        .ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == r.dep && e.key.kind == ResourceKind::Container)
        .all(|e| e.is_released()));

    // Docker removed it but the reply was lost: the next run finds it gone and reconciles the journal, it does not claim a removal
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let r = restored_interrupted(&t, &w, RecoveryOutcome::Restored, true, false);
    t.h.docker.fault(
        Some("DELETE"),
        &format!("/containers/{}", r.container),
        1,
        support::fake_docker::Action::ApplyThenDrop,
    );
    let _ = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert!(!container_present(&t, &r.container), "Docker did remove it");
    let again = apply(&t);
    assert_eq!(
        reason_of(&items(&again.stdout), &r.dep, "container").1,
        "already-absent",
        "{}",
        again.stdout
    );
    assert!(
        again.stdout.contains("reclaimed-containers: 1")
            || again.stdout.contains("removed 0 container"),
        "{}",
        again.stdout
    );
}

#[test]
fn repeating_the_cleanup_changes_nothing_after_the_first_run() {
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let r = restored_interrupted(&t, &w, RecoveryOutcome::Restored, true, false);
    apply(&t);
    let again = apply(&t);
    assert!(
        again.stdout.contains("removed 0 container(s)")
            && again.stdout.contains("removed 0 image(s)"),
        "{}",
        again.stdout
    );
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    assert!(image_present(&t, &old_i) && !container_present(&t, &r.container));
}

#[test]
fn a_restoration_container_that_is_what_serves_now_is_never_removed() {
    // the restoration finished after the last release did: its container serves the application, so the whole deployment is protected
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let r = restored_interrupted_at(&t, &w, RecoveryOutcome::Restored, true, true, 25_000);
    let o = apply(&t);
    assert!(
        container_present(&t, &r.container) && image_present(&t, &r.image),
        "{}",
        o.stdout
    );
    assert_eq!(
        reason_of(&items(&o.stdout), &r.dep, "container"),
        ("protected", "serving-release"),
        "{}",
        o.stdout
    );
    assert!(
        !items(&o.stdout)
            .iter()
            .any(|i| field(i, "deployment") == r.dep.as_str() && field(i, "verdict") == "ready"),
        "{}",
        o.stdout
    );
}
