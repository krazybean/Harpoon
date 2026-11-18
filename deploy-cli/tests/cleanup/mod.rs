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
        &go(&t2, &SHIPPING, &args(&t2, &["--keep-rollbacks", "1"])),
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
        &go(&t3, &SHIPPING, &args(&t3, &["--keep-rollbacks", "1"])),
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

    // ---- cleanup (the one Clean Up), against the acceptance application: releases A, B, C, an interrupted D, and a build cache
    let t = T::new();
    acceptance_world(&t);
    capture(&t, "cleanup-dry-run", &clean(&t, &[]), &mut all);
    capture(
        &t,
        "cleanup-dry-run-keep-one",
        &clean(&t, &["--keep-rollbacks", "1"]),
        &mut all,
    );
    capture(
        &t,
        "cleanup-apply-refused-unauthorized",
        &clean_noauth(&t),
        &mut all,
    );
    capture(
        &t,
        "cleanup-apply-refused-plan-changed",
        &go(
            &t,
            &SHIPPING,
            &uargs(&t, &["--apply", "--plan", "0000000000000000", &auth()]),
        ),
        &mut all,
    );
    capture(
        &t,
        "cleanup-apply-success",
        &clean(&t, &["--apply"]),
        &mut all,
    );
    capture(
        &t,
        "cleanup-apply-nothing-left",
        &clean(&t, &["--apply"]),
        &mut all,
    );
    let t = T::new();
    acceptance_world(&t);
    t.h.docker.fault(
        Some("POST"),
        "/build/prune",
        1,
        support::fake_docker::Action::Respond(500, r#"{"message":"prune failed"}"#.into()),
    );
    capture(
        &t,
        "cleanup-apply-partial-cache-failure",
        &clean(&t, &["--apply"]),
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

/// The rollback and restoration tests below were written for one previous release kept; retention is now explicit and defaults to
/// 0, so they say so. The retention tests use `dry_keep` / `apply_keep` with the value they mean.
fn dry(t: &T) -> Outcome {
    dry_keep(t, 1)
}
fn apply(t: &T) -> Outcome {
    apply_keep(t, 1)
}
fn dry_keep(t: &T, keep: usize) -> Outcome {
    let o = go(
        t,
        &SHIPPING,
        &args(t, &["--keep-rollbacks", &keep.to_string()]),
    );
    assert_exit(&o, 0);
    o
}
fn apply_keep(t: &T, keep: usize) -> Outcome {
    let o = go(
        t,
        &SHIPPING,
        &args(
            t,
            &["--keep-rollbacks", &keep.to_string(), "--apply", &auth()],
        ),
    );
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
    let o = go(
        &t,
        &SHIPPING,
        &mgmt(&t, args(&t, &["--keep-rollbacks", "1", "--apply", &auth()])),
    );
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
    // the restoration finished after the last release did: its container serves the application, so the container is kept
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let r = restored_interrupted_at(&t, &w, RecoveryOutcome::Restored, true, true, 25_000);
    let o = apply(&t);
    assert!(container_present(&t, &r.container), "{}", o.stdout);
    assert_eq!(
        reason_of(&items(&o.stdout), &r.dep, "container"),
        ("protected", "serving-release"),
        "{}",
        o.stdout
    );
    // what the owner built for itself (its failed candidate's image) is not what serves: it is judged on its own and goes
    assert!(!image_present(&t, &r.image), "{}", o.stdout);
    assert!(
        !items(&o.stdout)
            .iter()
            .any(|i| field(i, "deployment") == r.dep.as_str()
                && field(i, "kind") == "container"
                && field(i, "verdict") == "ready"),
        "{}",
        o.stdout
    );
}

// ---------------------------------------------------------------- retention, recovery protection and the one Clean Up

#[test]
fn historical_rollback_retention_defaults_to_zero_so_a_previous_release_is_disposable() {
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let (old_i, old_c) = w.ids[&w.old.to_string()].clone();
    let dry_run = go(&t, &SHIPPING, &args(&t, &[]));
    assert_exit(&dry_run, 0);
    assert!(
        dry_run
            .stdout
            .contains("historical rollback retention is 0"),
        "{}",
        dry_run.stdout
    );
    assert_eq!(
        reason_of(&items(&dry_run.stdout), &w.old, "image").0,
        "ready",
        "{}",
        dry_run.stdout
    );
    let o = go(&t, &SHIPPING, &args(&t, &["--apply", &auth()]));
    assert_exit(&o, 0);
    assert!(
        !image_present(&t, &old_i) && !container_present(&t, &old_c),
        "{}",
        o.stdout
    );
    let (srv_i, srv_c) = w.ids[&w.serving.to_string()].clone();
    assert!(
        image_present(&t, &srv_i) && container_present(&t, &srv_c),
        "the serving release is never touched"
    );
}

#[test]
fn retention_is_a_whole_number_and_never_governs_recovery() {
    let t = T::new();
    world_full(&t, false, Some(2000));
    for bad in ["-1", "x", "21", ""] {
        let o = go(&t, &SHIPPING, &args(&t, &["--keep-rollbacks", bad]));
        assert_ne!(o.code, 0, "{bad}: {}", o.stdout);
    }
}

#[test]
fn a_release_an_unsettled_recovery_may_need_is_kept_whatever_the_retention() {
    // the restoration is unfinished, unverified, or its pause was never cleared: the release it replaced may have to be restored again
    for (outcome, cleared) in [
        (RecoveryOutcome::InProgress, true),
        (RecoveryOutcome::RestoredUnverified, true),
        (
            RecoveryOutcome::RestoreFailed(harpoon_deploy::state::Reason::ImageMissing),
            true,
        ),
        (RecoveryOutcome::Restored, false),
    ] {
        let t = T::new();
        let w = world_full(&t, false, Some(2000));
        let r = restored_interrupted(&t, &w, outcome, cleared, false);
        let (old_i, old_c) = w.ids[&w.old.to_string()].clone();
        let o = apply_keep(&t, 0);
        assert!(
            image_present(&t, &old_i) && container_present(&t, &old_c),
            "{outcome:?} cleared={cleared}: {}",
            o.stdout
        );
        assert_eq!(
            reason_of(&items(&o.stdout), &w.old, "image"),
            ("protected", "recovery-pending"),
            "{}",
            o.stdout
        );
        assert!(
            container_present(&t, &r.container) && image_present(&t, &r.image),
            "the unsettled deployment's own resources stay too"
        );
        assert!(
            o.stdout.contains("kept for a recovery that is not settled"),
            "{}",
            o.stdout
        );
    }
    // once the recovery is settled the same release is ordinary history and, at retention 0, disposable
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let _ = restored_interrupted(&t, &w, RecoveryOutcome::Restored, true, false);
    let (old_i, _) = w.ids[&w.old.to_string()].clone();
    apply_keep(&t, 0);
    assert!(
        !image_present(&t, &old_i),
        "a settled recovery no longer needs it"
    );
}

fn uargs(t: &T, extra: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = vec![
        "cleanup".into(),
        "--docker-socket".into(),
        t.socket(),
        "--resources-dir".into(),
        t.resources(),
        "--mgmt-socket".into(),
        t.mgmt.path().display().to_string(),
    ];
    v.extend(extra.iter().map(|s| s.to_string()));
    v
}
fn kv(out: &str, key: &str) -> Option<u64> {
    out.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .and_then(|v| v.trim().parse().ok())
}
/// A review (no `--apply`), or an apply that carries the plan id of a review made just before it: that is how the desktop uses it.
fn clean(t: &T, extra: &[&str]) -> Outcome {
    let mut e: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
    if e.iter().any(|a| a == "--apply") {
        if !e.iter().any(|a| a == "--plan") {
            let review: Vec<String> = e.iter().filter(|a| *a != "--apply").cloned().collect();
            let r = go(t, &SHIPPING, &uargs_owned(t, &review));
            if let Some(id) = r
                .stdout
                .lines()
                .find_map(|l| l.strip_prefix("cleanup-plan: "))
            {
                e.push("--plan".into());
                e.push(id.to_string());
            }
        }
        e.push(auth());
    }
    go(t, &SHIPPING, &uargs_owned(t, &e))
}
fn uargs_owned(t: &T, extra: &[String]) -> Vec<String> {
    let refs: Vec<&str> = extra.iter().map(String::as_str).collect();
    uargs(t, &refs)
}
fn plan_of(o: &Outcome) -> String {
    o.stdout
        .lines()
        .find_map(|l| l.strip_prefix("cleanup-plan: "))
        .expect("a review prints its plan")
        .to_string()
}
fn deletes(t: &T) -> usize {
    t.h.docker
        .requests()
        .iter()
        .filter(|r| r.method == "DELETE" || (r.method == "POST" && r.path.contains("/build/prune")))
        .count()
}

const MB: u64 = 1_000_000;

/// A disposable application with the whole history the Clean Up has to make sense of: releases A, B and C (C serves), a deployment D
/// that was interrupted while replacing C and whose restoration completed (C now serves from D's restoration container, which D
/// owns), an unrelated database container, and a build cache. Docker images have layers: a shared Node base and one layer of
/// 400 MB per release, so what removal frees is exactly known. The guest's free space is a model of what Docker holds.
struct Acc {
    a: DeploymentId,
    b: DeploymentId,
    c: DeploymentId,
    d: DeploymentId,
    img: std::collections::BTreeMap<&'static str, String>,
    ctr: std::collections::BTreeMap<&'static str, String>,
}

fn acceptance_world(t: &T) -> Acc {
    t.docker_runtime();
    let (app, _) = ids(30);
    let (_, a) = ids(31);
    let (_, b) = ids(32);
    let (_, c) = ids(33);
    let (_, d) = ids(34);
    let sizes: std::collections::BTreeMap<String, u64> = [
        ("b1", 150),
        ("b2", 100),
        ("a1", 400),
        ("b1x", 400),
        ("c1", 400),
        ("d1", 400),
        ("p1", 500),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v * MB))
    .collect();
    let layers_of = |own: &str| -> Vec<String> { vec!["b1".into(), "b2".into(), own.into()] };
    let total_of = |l: &[String]| l.iter().map(|x| sizes[x]).sum::<u64>();
    let mut acc = Acc {
        a: a.clone(),
        b: b.clone(),
        c: c.clone(),
        d: d.clone(),
        img: Default::default(),
        ctr: Default::default(),
    };

    // the Node base image every release is built on, and an unrelated database with its own image and a running container
    t.h.docker.with(|s| {
        s.add_image_with_id("sha256:base", &["node:22-slim"], Default::default(), vec![]);
        let i = s.images.iter_mut().find(|i| i.id == "sha256:base").unwrap();
        i.layers = vec!["b1".into(), "b2".into()];
        i.size = 250 * MB;
        s.add_image_with_id(
            "sha256:pg",
            &["postgres:15"],
            Default::default(),
            vec!["/var/lib/postgresql/data".into()],
        );
        let i = s.images.iter_mut().find(|i| i.id == "sha256:pg").unwrap();
        i.layers = vec!["p1".into()];
        i.size = 500 * MB;
        let id = s.add_container("collectiv-db", Default::default(), true);
        s.containers.iter_mut().find(|c| c.id == id).unwrap().image = "sha256:pg".into();
    });

    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    let mut release = |name: &'static str, dep: &DeploymentId, own: &str, seed: u64, at: u64| {
        let iid = support::fake_iid(seed);
        let tag = format!(
            "harpoon/{}:{}-0123456789abcdef0123456789abcdef",
            app.as_str().to_ascii_lowercase(),
            dep
        );
        let lbl = harpoon_deploy::backend::harpoon_labels(&app, dep);
        let layers = layers_of(own);
        let size = total_of(&layers);
        t.h.docker.with(|s| {
            s.add_image_with_id(&iid, &[&tag], lbl.clone(), vec![]);
            let i = s.images.iter_mut().find(|i| i.id == iid).unwrap();
            i.layers = layers;
            i.size = size;
        });
        let mut dep_rec = finished(&app, dep, DeploymentStatus::Succeeded);
        dep_rec.release = Some(ReleaseId::from_deployment(dep));
        dep_rec.pinned = Some(pin(iid.clone(), 38000 + seed as u16 % 100));
        dep_rec.finished_ms = Some(at);
        j.persist(&dep_rec).unwrap();
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
        let cname = format!("hp-{dep}");
        let cid = t.h.docker.with(|s| s.add_container(&cname, lbl, false));
        j.record(&intent(
            &app,
            dep,
            ResourceKind::Container,
            &cname,
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
        j.record(&receipt(dep, ResourceKind::Container, &cname, &cid))
            .unwrap();
        (name, iid, cid)
    };
    for (name, dep, own, seed, at) in [
        ("a", &a, "a1", 4001u64, 10_000u64),
        ("b", &b, "b1x", 4002, 20_000),
        ("c", &c, "c1", 4003, 30_000),
    ] {
        let (n, iid, cid) = release(name, dep, own, seed, at);
        acc.img.insert(n, iid);
        acc.ctr.insert(n, cid);
    }

    // D: interrupted while replacing C; C was restored in a container D owns, and that container now serves
    let lbl = harpoon_deploy::backend::harpoon_labels(&app, &d);
    let d_img = support::fake_iid(4004);
    let d_tag = format!(
        "harpoon/{}:{}-0123456789abcdef0123456789abcdef",
        app.as_str().to_ascii_lowercase(),
        d
    );
    t.h.docker.with(|s| {
        s.add_image_with_id(&d_img, &[&d_tag], lbl.clone(), vec![]);
        let i = s.images.iter_mut().find(|i| i.id == d_img).unwrap();
        i.layers = vec!["b1".into(), "b2".into(), "d1".into()];
        i.size = 650 * MB;
    });
    let rname = format!("hp-{d}-restored");
    let c_img = acc.img["c"].clone();
    let rid = t.h.docker.with(|s| {
        let id = s.add_container(&rname, lbl, true);
        s.containers.iter_mut().find(|x| x.id == id).unwrap().image = c_img;
        id
    });
    let mut dd = finished(&app, &d, DeploymentStatus::Interrupted);
    dd.cutover = CutoverState::Begun { at_ms: 35_000 };
    dd.replaces = Some(ReleaseId::from_deployment(&c));
    dd.recovery = Some(Recovery {
        outcome: RecoveryOutcome::Restored,
        started_ms: 36_000,
        finished_ms: Some(40_000),
        budget_ms: 60_000,
    });
    dd.finished_ms = Some(40_000);
    j.persist(&dd).unwrap();
    j.record(&intent(
        &app,
        &d,
        ResourceKind::Image,
        &d_tag,
        Disposition::Retain,
    ))
    .unwrap();
    j.record(&receipt(&d, ResourceKind::Image, &d_tag, &d_img))
        .unwrap();
    j.record(&intent(
        &app,
        &d,
        ResourceKind::Container,
        &rname,
        Disposition::Retain,
    ))
    .unwrap();
    j.record(&receipt(&d, ResourceKind::Container, &rname, &rid))
        .unwrap();
    j.control(&ControlRecord::Resume(ResumeRec {
        app: app.clone(),
        at_ms: 41_000,
        choice: Choice::Dismiss,
        selected: None,
        dismissed: vec![],
        superseded: vec![],
        cleared: vec![d.clone()],
    }))
    .unwrap();
    acc.img.insert("d", d_img);
    acc.ctr.insert("d-restored", rid);

    // a build cache: one unused record nothing shares, one that C's image holds
    t.h.docker.with(|s| {
        s.build_cache = vec![(500 * MB, false), (400 * MB, false)];
        s.build_cache_shared = vec![false, true];
    });
    // the guest's disk: an 8 GB filesystem with 3 GB of system and data, plus every distinct layer Docker holds and the cache
    let st = t.h.docker.st.clone();
    t.mgmt.set(|m| {
        m.total = 8_000 * MB;
        m.avail_fn = Some(std::sync::Arc::new(move || {
            let s = st.lock().unwrap();
            let mut seen = std::collections::BTreeSet::new();
            let mut used = 3_000 * MB;
            for i in &s.images {
                for l in &i.layers {
                    if seen.insert(l.clone()) {
                        used += sizes[l.as_str()];
                    }
                }
            }
            used += s.build_cache.iter().map(|(n, _)| *n).sum::<u64>();
            (8_000 * MB).saturating_sub(used)
        }));
    });
    acc
}

fn present(t: &T, a: &Acc, k: &str) -> bool {
    image_present(t, &a.img[k])
}

#[test]
fn clean_up_removes_everything_disposable_in_one_run_and_measures_what_it_freed() {
    let t = T::new();
    let a = acceptance_world(&t);
    let free0 = {
        let o = go(&t, &SHIPPING, &uargs(&t, &[]));
        kv(&o.stdout, "cleanup-estimated-bytes")
    };
    // review: nothing changes, and the estimate is exact: three 400 MB layers and 500 MB of unshared cache
    let before = deletes(&t);
    let review = clean(&t, &[]);
    assert_exit(&review, 0);
    assert_eq!(deletes(&t), before, "a review changes nothing");
    assert_eq!(
        kv(&review.stdout, "cleanup-estimated-bytes"),
        Some(3 * 400 * MB + 500 * MB),
        "{}",
        review.stdout
    );
    assert_eq!(
        free0,
        kv(&review.stdout, "cleanup-estimated-bytes"),
        "reviewing twice says the same"
    );
    assert!(
        review
            .stdout
            .contains("previous releases kept for rollback: 0"),
        "{}",
        review.stdout
    );
    let its = items(&review.stdout);
    assert_eq!(
        reason_of(&its, &a.c, "image").0,
        "protected",
        "the serving release"
    );
    assert_eq!(reason_of(&its, &a.a, "image").0, "ready");
    assert_eq!(reason_of(&its, &a.b, "image").0, "ready");
    assert_eq!(
        reason_of(&its, &a.d, "container"),
        ("protected", "serving-release"),
        "the restoration container serves"
    );
    assert_eq!(
        reason_of(&its, &a.d, "image").0,
        "ready",
        "the failed candidate's own image is disposable"
    );

    // apply: one run
    let o = clean(&t, &["--apply"]);
    assert_exit(&o, 0);
    // only C and what it needs remain
    assert!(
        present(&t, &a, "c")
            && !present(&t, &a, "a")
            && !present(&t, &a, "b")
            && !present(&t, &a, "d"),
        "{}",
        o.stdout
    );
    assert!(
        container_present(&t, &a.ctr["d-restored"]),
        "the restoration container that serves stays"
    );
    assert!(
        container_present(&t, &a.ctr["c"])
            && !container_present(&t, &a.ctr["a"])
            && !container_present(&t, &a.ctr["b"])
    );
    assert!(
        image_present(&t, "sha256:base") && image_present(&t, "sha256:pg"),
        "the base image and the database image are untouched"
    );
    // persistent data and the unrelated application: the database container is untouched, and no volume was ever addressed
    assert!(t.h.docker.with(|s| s
        .containers
        .iter()
        .any(|c| c.name == "collectiv-db" && c.running)));
    assert!(
        t.h.docker
            .requests()
            .iter()
            .all(|r| !r.path.contains("/volumes")),
        "volumes are never touched"
    );
    // the unshared cache record went; the one an image holds stayed
    assert_eq!(
        t.h.docker.with(|s| s.build_cache.clone()),
        vec![(400 * MB, false)]
    );
    // measured on the modelled disk, and equal to what was estimated: 1.7 GB
    assert_eq!(
        kv(&o.stdout, "measured-reclaimed-bytes"),
        Some(1_700 * MB),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("the measured change matches the estimate"),
        "{}",
        o.stdout
    );
    // the journal: history stays, what was removed is released
    let j = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    assert_eq!(j.deployments.len(), 4, "deployment history is kept");
    assert!(j
        .ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == a.a || e.deployment == a.b)
        .all(|e| e.is_released()));

    // a second Clean Up finds nothing, changes nothing and claims nothing
    let n = deletes(&t);
    let again = clean(&t, &["--apply"]);
    assert_exit(&again, 0);
    assert_eq!(
        deletes(&t),
        n,
        "nothing more was asked of Docker to delete: {}",
        again.stdout
    );
    assert!(
        again.stdout.contains("cleanup: nothing to clean up"),
        "{}",
        again.stdout
    );
    assert_eq!(kv(&again.stdout, "cleanup-eligible"), Some(0));
    assert_eq!(kv(&again.stdout, "measured-reclaimed-bytes"), Some(0));
}

#[test]
fn clean_up_with_retention_one_keeps_the_previous_release_and_frees_only_the_rest() {
    let t = T::new();
    let a = acceptance_world(&t);
    let o = clean(&t, &["--keep-rollbacks", "1", "--apply"]);
    assert_exit(&o, 0);
    assert!(
        present(&t, &a, "b") && container_present(&t, &a.ctr["b"]),
        "B stays: {}",
        o.stdout
    );
    assert!(present(&t, &a, "c") && !present(&t, &a, "a") && !present(&t, &a, "d"));
    assert_eq!(
        reason_of(&items(&o.stdout), &a.b, "image"),
        ("protected", "rollback-target")
    );
    assert_eq!(
        kv(&o.stdout, "measured-reclaimed-bytes"),
        Some(2 * 400 * MB + 500 * MB),
        "A, D's candidate and the cache: {}",
        o.stdout
    );
    let again = clean(&t, &["--keep-rollbacks", "1", "--apply"]);
    assert!(
        again.stdout.contains("cleanup: nothing to clean up"),
        "{}",
        again.stdout
    );
    // raising retention never resurrects anything, and lowering it later frees B
    let lower = clean(&t, &["--keep-rollbacks", "0", "--apply"]);
    assert!(!present(&t, &a, "b"), "{}", lower.stdout);
    assert_eq!(
        kv(&lower.stdout, "measured-reclaimed-bytes"),
        Some(400 * MB)
    );
}

#[test]
fn clean_up_finishes_after_a_partial_failure_and_never_claims_what_it_did_not_free() {
    let t = T::new();
    let a = acceptance_world(&t);
    // Docker drops the connection when asked to remove A's image: the run goes on, A's image stays recorded
    t.h.docker.fault(
        Some("DELETE"),
        &format!("/images/{}", a.img["a"]),
        1,
        support::fake_docker::Action::Disconnect,
    );
    let first = clean(&t, &["--apply"]);
    assert!(present(&t, &a, "a"), "{}", first.stdout);
    assert!(
        !present(&t, &a, "b") && !present(&t, &a, "d"),
        "the others were still done: {}",
        first.stdout
    );
    let freed = kv(&first.stdout, "measured-reclaimed-bytes").unwrap();
    assert_eq!(
        freed,
        400 * MB + 400 * MB + 500 * MB,
        "A's 400 MB is not claimed: {}",
        first.stdout
    );
    // the next run retries exactly that
    let second = clean(&t, &["--apply"]);
    assert_exit(&second, 0);
    assert!(!present(&t, &a, "a"), "{}", second.stdout);
    assert_eq!(
        kv(&second.stdout, "measured-reclaimed-bytes"),
        Some(400 * MB)
    );
    let third = clean(&t, &["--apply"]);
    assert!(
        third.stdout.contains("cleanup: nothing to clean up"),
        "{}",
        third.stdout
    );
}

#[test]
fn clean_up_reports_a_failed_cache_step_as_partial_and_the_rerun_finishes_it() {
    let t = T::new();
    let a = acceptance_world(&t);
    t.h.docker.fault(
        Some("POST"),
        "/build/prune",
        1,
        support::fake_docker::Action::Respond(500, r#"{"message":"prune failed"}"#.into()),
    );
    let first = clean(&t, &["--apply"]);
    assert_ne!(first.code, 0, "{}", first.stdout);
    assert!(
        first.stdout.contains("cleanup: partial"),
        "{}",
        first.stdout
    );
    assert!(
        !present(&t, &a, "a") && !present(&t, &a, "d"),
        "the releases step had finished"
    );
    assert_eq!(
        t.h.docker.with(|s| s.build_cache.len()),
        2,
        "the cache is untouched"
    );
    let second = clean(&t, &["--apply"]);
    assert_exit(&second, 0);
    assert_eq!(
        t.h.docker.with(|s| s.build_cache.len()),
        1,
        "{}",
        second.stdout
    );
}

#[test]
fn clean_up_is_refused_without_authorization_or_while_a_deployment_is_unfinished() {
    let t = T::new();
    let a = acceptance_world(&t);
    let before = deletes(&t);
    let refused = clean_noauth(&t);
    assert_ne!(refused.code, 0, "{}", refused.stdout);
    assert_eq!(deletes(&t), before);
    // a deployment that is still running: nothing, including the cache, is touched
    with_deployment(&t, DeploymentStatus::Running);
    let o = clean(&t, &["--apply"]);
    assert_ne!(o.code, 0, "{}", o.stdout);
    assert_eq!(
        deletes(&t),
        before,
        "no cleanup while a deployment is unfinished"
    );
    assert!(present(&t, &a, "a"));
}

fn clean_noauth(t: &T) -> Outcome {
    go(t, &SHIPPING, &uargs(t, &["--apply"]))
}

#[test]
fn clean_up_after_a_lost_reply_reconciles_what_docker_already_did_and_claims_nothing_for_it() {
    let t = T::new();
    let a = acceptance_world(&t);
    // Docker removes B's container but the reply never arrives: the journal still lists it, Docker no longer has it
    t.h.docker.fault(
        Some("DELETE"),
        &format!("/containers/{}", a.ctr["b"]),
        1,
        support::fake_docker::Action::ApplyThenDrop,
    );
    let first = clean(&t, &["--apply"]);
    assert!(
        !container_present(&t, &a.ctr["b"]),
        "Docker did remove it: {}",
        first.stdout
    );
    // the next run reconciles the record instead of removing again, finishes the rest, and the total is still exact
    let second = clean(&t, &["--apply"]);
    assert_exit(&second, 0);
    assert!(
        !present(&t, &a, "b") && !present(&t, &a, "a") && !present(&t, &a, "d"),
        "{}",
        second.stdout
    );
    let j = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    assert!(
        j.ledger
            .entries()
            .iter()
            .filter(|e| e.deployment == a.b)
            .all(|e| e.is_released()),
        "the journal agrees with Docker"
    );
    let third = clean(&t, &["--apply"]);
    assert!(
        third.stdout.contains("cleanup: nothing to clean up"),
        "{}",
        third.stdout
    );
}

// ---------------------------------------------------------------- an apply acts on the plan that was reviewed

#[test]
fn an_apply_needs_the_reviewed_plan_and_acts_on_nothing_else() {
    let t = T::new();
    let a = acceptance_world(&t);
    let before = deletes(&t);
    // no plan at all
    let none = go(&t, &SHIPPING, &uargs(&t, &["--apply", &auth()]));
    assert_eq!(
        none.code, 2,
        "a usage error: {}{}",
        none.stdout, none.stderr
    );
    // a plan that is not the reviewed one
    let wrong = go(
        &t,
        &SHIPPING,
        &uargs(&t, &["--apply", "--plan", "0000000000000000", &auth()]),
    );
    assert_eq!(wrong.code, 6, "refused: {}{}", wrong.stdout, wrong.stderr);
    assert!(
        wrong
            .stderr
            .contains("the plan changed since it was reviewed")
            && wrong.stderr.contains("Review again"),
        "{}",
        wrong.stderr
    );
    assert_eq!(deletes(&t), before, "nothing was asked of Docker");
    assert!(present(&t, &a, "a") && present(&t, &a, "b"));
    // the plan is stable, and depends on the retention
    let r1 = clean(&t, &[]);
    let r2 = clean(&t, &[]);
    assert_eq!(
        plan_of(&r1),
        plan_of(&r2),
        "reviewing twice names the same plan"
    );
    assert_ne!(
        plan_of(&r1),
        plan_of(&clean(&t, &["--keep-rollbacks", "1"])),
        "another retention is another plan"
    );
    // the right plan applies
    let ok = go(
        &t,
        &SHIPPING,
        &uargs(&t, &["--apply", "--plan", &plan_of(&r1), &auth()]),
    );
    assert_exit(&ok, 0);
    assert!(
        !present(&t, &a, "a") && !present(&t, &a, "b"),
        "{}",
        ok.stdout
    );
    assert_eq!(
        plan_of(&ok),
        plan_of(&r1),
        "what was done is what was reviewed"
    );
}

#[test]
fn a_plan_that_changed_after_the_review_is_refused_and_a_fresh_review_applies() {
    let t = T::new();
    let a = acceptance_world(&t);
    let reviewed = plan_of(&clean(&t, &[]));
    // between the review and the apply, A's image disappears from Docker: what Clean Up would do for it is now different
    t.h.docker.with(|s| s.images.retain(|i| i.id != a.img["a"]));
    let before = deletes(&t);
    let o = go(
        &t,
        &SHIPPING,
        &uargs(&t, &["--apply", "--plan", &reviewed, &auth()]),
    );
    assert_eq!(o.code, 6, "{}{}", o.stdout, o.stderr);
    assert!(o.stderr.contains("Review again"), "{}", o.stderr);
    assert_eq!(deletes(&t), before, "nothing was changed");
    assert!(
        present(&t, &a, "b") && present(&t, &a, "d"),
        "nothing was removed"
    );
    // a fresh review names the new plan, and applying that works
    let applied = clean(&t, &["--apply"]);
    assert_exit(&applied, 0);
    assert!(
        !present(&t, &a, "b") && !present(&t, &a, "d"),
        "{}",
        applied.stdout
    );
}

#[test]
fn an_object_that_becomes_removable_during_an_apply_is_left_alone_and_the_run_says_so() {
    let t = T::new();
    let a = acceptance_world(&t);
    // B's image is kept at review because a container (not Harpoon's) was made from it
    let b_img = a.img["b"].clone();
    t.h.docker.with(|s| {
        let id = s.add_container("scratch-shell", Default::default(), false);
        s.containers.iter_mut().find(|c| c.id == id).unwrap().image = b_img.clone();
    });
    let review = clean(&t, &[]);
    assert_eq!(
        reason_of(&items(&review.stdout), &a.b, "image").0,
        "review",
        "{}",
        review.stdout
    );
    // when the first removal arrives, that container is removed by someone else: B's image would now be removable
    t.h.docker.fault(
        Some("DELETE"),
        &format!("/containers/{}", a.ctr["a"]),
        1,
        support::fake_docker::Action::Race(std::sync::Arc::new(
            |s: &mut support::fake_docker::St| s.containers.retain(|c| c.name != "scratch-shell"),
        )),
    );
    let o = clean(&t, &["--apply"]);
    assert_ne!(o.code, 0, "{}", o.stdout);
    assert!(
        present(&t, &a, "b"),
        "B's image was not in the reviewed plan, so it stays: {}",
        o.stdout
    );
    assert!(o.stdout.contains("cleanup: stopped short"), "{}", o.stdout);
    assert!(!present(&t, &a, "a"), "what was reviewed was done");
    // a fresh review now includes it
    let again = clean(&t, &["--apply"]);
    assert_exit(&again, 0);
    assert!(!present(&t, &a, "b"), "{}", again.stdout);
}

/// Every cache record is shared with an image, as BuildKit leaves it: two held by releases the plan removes, one by the serving release.
fn cache_held_by_images(t: &T, a: &Acc) {
    let (ia, ib, ic, id) = (
        a.img["a"].clone(),
        a.img["b"].clone(),
        a.img["c"].clone(),
        a.img["d"].clone(),
    );
    t.h.docker.with(|s| {
        s.build_cache = vec![(300 * MB, false), (200 * MB, false), (400 * MB, false)];
        s.build_cache_shared = vec![true, true, true];
        s.build_cache_holders = vec![vec![ia, ib], vec![id], vec![ic]];
    });
}

#[test]
fn one_clean_up_removes_the_images_and_then_the_cache_they_held() {
    let t = T::new();
    let a = acceptance_world(&t);
    cache_held_by_images(&t, &a);
    let review = clean(&t, &[]);
    assert_exit(&review, 0);
    // nothing is prunable yet, but the plan says the cache is checked once the images are gone, and does not estimate it
    assert_eq!(kv(&review.stdout, "build-cache-prunable-records"), Some(0));
    assert!(
        review.stdout.contains("cache-stage: after-images"),
        "{}",
        review.stdout
    );
    assert_eq!(
        kv(&review.stdout, "cleanup-estimated-bytes"),
        Some(3 * 400 * MB),
        "{}",
        review.stdout
    );
    let gone: Vec<String> = ["a", "b", "d"].iter().map(|k| a.img[*k].clone()).collect();
    let o = clean(&t, &["--apply"]);
    assert_exit(&o, 0);
    // the records the removed images held were released, re-checked and pruned in the same operation
    assert!(
        o.stdout
            .contains("checked again after the images were removed: 2 record(s) were prunable"),
        "{}",
        o.stdout
    );
    assert_eq!(
        kv(&o.stdout, "build-cache-reclaimed-bytes"),
        Some(500 * MB),
        "{}",
        o.stdout
    );
    assert_eq!(
        t.h.docker.with(|s| s.build_cache.len()),
        1,
        "the record the serving release holds stays"
    );
    for g in &gone {
        assert!(!image_present(&t, g), "reviewed image removed");
    }
    // estimated and measured stay apart: the layers freed 1200 MB, the cache 500 MB, and nothing is claimed beyond the measurement
    assert_eq!(
        kv(&o.stdout, "measured-reclaimed-bytes"),
        Some(3 * 400 * MB + 500 * MB),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("not estimated; only the measured figure is real"),
        "{}",
        o.stdout
    );
    // serving release, its container, the recovery container D owns and the database are untouched
    assert!(present(&t, &a, "c") && !present(&t, &a, "d"));
    assert!(t
        .h
        .docker
        .with(|s| s.containers.iter().any(|c| c.id == a.ctr["d-restored"])));
    assert!(image_present(&t, "sha256:pg"));
    // a second review finds nothing further: no objects, no cache stage
    let again = clean(&t, &[]);
    assert_exit(&again, 0);
    assert_eq!(
        kv(&again.stdout, "cleanup-eligible"),
        Some(0),
        "{}",
        again.stdout
    );
    assert!(
        again.stdout.contains("cache-stage: none"),
        "{}",
        again.stdout
    );
    let before = deletes(&t);
    assert_exit(&clean(&t, &["--apply"]), 0);
    assert_eq!(
        deletes(&t),
        before,
        "a second apply asks Docker for nothing"
    );
}

#[test]
fn the_cache_stage_removes_nothing_the_review_did_not_show() {
    let t = T::new();
    let a = acceptance_world(&t);
    cache_held_by_images(&t, &a);
    let review = clean(&t, &[]);
    let plan = plan_of(&review);
    // after the review a Harpoon-labelled image appears that no journal knows (an orphan) and a new unused container is created
    let (oa, _) = ids(90);
    let (_, od) = ids(91);
    let lbl = harpoon_deploy::backend::harpoon_labels(&oa, &od);
    t.h.docker.with(|s| {
        s.add_image_with_id("sha256:orphan", &["harpoon/other:x"], lbl, vec![]);
    });
    let o = go(
        &t,
        &SHIPPING,
        &uargs(&t, &["--apply", "--plan", &plan, &auth()]),
    );
    assert!(
        image_present(&t, "sha256:orphan"),
        "an object nobody reviewed is never removed\n{}",
        o.stdout
    );
    assert!(
        !image_present(&t, &a.img["a"]),
        "the reviewed removal still ran\n{}",
        o.stdout
    );
    // the reviewed work, if it ran, was exactly the reviewed work
    let removed: Vec<String> =
        t.h.docker
            .requests()
            .iter()
            .filter(|r| r.method == "DELETE")
            .map(|r| r.path.clone())
            .collect();
    assert!(removed.iter().all(|p| !p.contains("orphan")), "{removed:?}");
}

#[test]
fn an_apply_refuses_when_the_cache_stage_is_no_longer_the_reviewed_one() {
    let t = T::new();
    let a = acceptance_world(&t);
    cache_held_by_images(&t, &a);
    let plan = plan_of(&clean(&t, &[]));
    // between review and apply a record stops being shared: the plan's cache stage is now different
    t.h.docker.with(|s| {
        s.build_cache_holders[0].clear();
        s.build_cache_shared[0] = false;
    });
    let before = deletes(&t);
    let o = go(
        &t,
        &SHIPPING,
        &uargs(&t, &["--apply", "--plan", &plan, &auth()]),
    );
    assert_exit(&o, 6);
    assert!(
        o.stdout.contains("plan changed") || o.stderr.contains("plan changed"),
        "{}{}",
        o.stdout,
        o.stderr
    );
    assert_eq!(deletes(&t), before, "nothing was removed or pruned");
}

#[test]
fn a_cache_prune_that_fails_after_the_images_are_gone_is_finished_by_the_next_clean_up() {
    let t = T::new();
    let a = acceptance_world(&t);
    cache_held_by_images(&t, &a);
    t.h.docker.fault(
        Some("POST"),
        "/build/prune",
        1,
        support::fake_docker::Action::Respond(500, "{\"message\":\"boom\"}".into()),
    );
    let o = clean(&t, &["--apply"]);
    assert_ne!(o.code, 0, "{}", o.stdout);
    assert!(
        o.stdout
            .contains("cleanup: partial. The releases step finished"),
        "{}",
        o.stdout
    );
    assert!(
        !image_present(&t, &a.img["a"]),
        "the images were removed and stay removed"
    );
    // the records are released now: the next review shows them as prunable, and nothing else
    let review = clean(&t, &[]);
    assert_eq!(
        kv(&review.stdout, "build-cache-prunable-records"),
        Some(2),
        "{}",
        review.stdout
    );
    assert!(
        review.stdout.contains("cache-stage: now"),
        "{}",
        review.stdout
    );
    let o2 = clean(&t, &["--apply"]);
    assert_exit(&o2, 0);
    assert_eq!(
        kv(&o2.stdout, "build-cache-reclaimed-bytes"),
        Some(500 * MB),
        "{}",
        o2.stdout
    );
    let last = clean(&t, &[]);
    assert_eq!(
        kv(&last.stdout, "cleanup-eligible"),
        Some(0),
        "{}",
        last.stdout
    );
}

#[test]
fn the_build_cache_is_not_pruned_when_the_review_had_no_cache_stage() {
    let t = T::new();
    let a = acceptance_world(&t);
    assert_exit(&clean(&t, &["--apply"]), 0);
    // everything disposable is gone; one record is still shared with the serving release's image
    t.h.docker.with(|s| {
        s.build_cache = vec![(400 * MB, false)];
        s.build_cache_shared = vec![true];
        s.build_cache_holders = vec![vec![a.img["c"].clone()]];
    });
    let review = clean(&t, &[]);
    assert!(
        review.stdout.contains("cache-stage: none"),
        "{}",
        review.stdout
    );
    let before = deletes(&t);
    let o = clean(&t, &["--apply"]);
    assert_exit(&o, 0);
    assert_eq!(t.h.docker.with(|s| s.build_cache.len()), 1);
    assert_eq!(
        deletes(&t),
        before,
        "nothing is prunable, so Docker is not asked"
    );
}

// ---------------------------------------------------------------- failing closed

#[test]
fn clean_up_never_runs_while_a_deployment_is_in_the_middle_of_a_cutover_or_the_journal_is_held() {
    // a deployment that began a cutover and has not finished: the previous release may be the only thing that can restore it
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        let (_, dep) = ids(60);
        let mut d = finished(&w.app, &dep, DeploymentStatus::Running);
        d.cutover = CutoverState::Begun { at_ms: 25_000 };
        d.replaces = Some(ReleaseId::from_deployment(&w.serving));
        j.persist(&d).unwrap();
    }
    let (srv_i, srv_c) = w.ids[&w.serving.to_string()].clone();
    let (old_i, old_c) = w.ids[&w.old.to_string()].clone();
    for keep in ["0", "1"] {
        let o = go(
            &t,
            &SHIPPING,
            &args(&t, &["--keep-rollbacks", keep, "--apply", &auth()]),
        );
        assert_ne!(o.code, 0, "{}", o.stdout);
        assert!(
            image_present(&t, &srv_i)
                && container_present(&t, &srv_c)
                && image_present(&t, &old_i)
                && container_present(&t, &old_c),
            "{}",
            o.stdout
        );
    }
    let review = go(&t, &SHIPPING, &uargs(&t, &[]));
    assert_ne!(review.code, 0, "even the review refuses: {}", review.stdout);
    // another process holds the deployment journal: a deployment may be running, so nothing is applied
    let t = T::new();
    let a = acceptance_world(&t);
    let id = plan_of(&clean(&t, &[]));
    let hold =
        harpoon_deploy_journal::hold_writer_lock(&t.journal_dir()).expect("take the writer lock");
    let before = deletes(&t);
    let o = go(
        &t,
        &SHIPPING,
        &uargs(&t, &["--apply", "--plan", &id, &auth()]),
    );
    assert_ne!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert_eq!(deletes(&t), before);
    assert!(present(&t, &a, "a"));
    drop(hold);
}

#[test]
fn an_unreachable_docker_or_an_unreadable_runtime_fails_a_review_without_a_guess() {
    let t = T::new();
    acceptance_world(&t);
    let o = go(
        &t,
        &SHIPPING,
        &[
            "cleanup".to_string(),
            "--docker-socket".into(),
            "/nonexistent/docker.sock".into(),
            "--resources-dir".into(),
            t.resources(),
        ],
    );
    assert_ne!(o.code, 0, "{}", o.stdout);
    assert!(
        !o.stdout.contains("cleanup-plan:"),
        "no plan is offered when the runtime could not be read: {}",
        o.stdout
    );
}

#[test]
fn a_prepared_candidate_that_was_never_confirmed_leaves_the_serving_release_untouched_at_retention_zero(
) {
    // `replace` without a confirmation only prepares a candidate (status cancelled, cutover not started): the release that
    // serves is still the only thing running, and nothing about it or the candidate may be removed by Clean Up
    let t = T::new();
    let w = world_full(&t, false, Some(2000));
    let (_, cand) = ids(61);
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        let mut d = finished(&w.app, &cand, DeploymentStatus::Cancelled);
        d.replaces = Some(ReleaseId::from_deployment(&w.serving));
        j.persist(&d).unwrap();
    }
    let (srv_i, srv_c) = w.ids[&w.serving.to_string()].clone();
    let o = apply_keep(&t, 0);
    assert!(
        image_present(&t, &srv_i) && container_present(&t, &srv_c),
        "{}",
        o.stdout
    );
    assert_eq!(
        reason_of(&items(&o.stdout), &w.serving, "image"),
        ("protected", "serving-release")
    );
}
