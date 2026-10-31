//! B8.2: `cleanup-image`, end to end through the real CLI, the real journal files and the real
//! `DockerBackend`, against the *simulated* Docker server. No Docker Engine, VM, socket outside the
//! temporary directory, real container or real image is involved.

use super::*;
use harpoon_deploy::ledger::{
    CleanupOutcome, ConflictCause, EndState, Entry, EntryState, Released,
};
use harpoon_deploy::time::Clock as _;
use std::sync::Arc;
use support::fake_docker::{Action, St};

struct C {
    app: ApplicationId,
    dep: DeploymentId,
    tag: String,
    iid: String,
}

/// A journal as a failed (or other) deployment leaves it: the deployment, and its retained image
/// with an intent and a receipt — plus the image in the simulated Docker, labelled for it.
fn craft(t: &T, seed: u8, status: DeploymentStatus) -> C {
    craft_with(t, seed, status, true, |_, _| {})
}

fn craft_with(
    t: &T,
    seed: u8,
    status: DeploymentStatus,
    receipt_it: bool,
    extra: impl FnOnce(&mut Held, &C),
) -> C {
    t.docker_runtime();
    let (app, dep) = ids(seed);
    let c = C {
        tag: format!(
            "harpoon/{}:{}-0123456789abcdef0123456789abcdef",
            app.as_str().to_ascii_lowercase(),
            dep
        ),
        iid: support::fake_iid(1000 + seed as u64),
        app,
        dep,
    };
    let lbl = harpoon_deploy::backend::harpoon_labels(&c.app, &c.dep);
    t.h.docker
        .with(|s| s.add_image_with_id(&c.iid, &[&c.tag], lbl, vec![]));
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&finished(&c.app, &c.dep, status)).unwrap();
    j.record(&intent(
        &c.app,
        &c.dep,
        ResourceKind::Image,
        &c.tag,
        Disposition::Retain,
    ))
    .unwrap();
    if receipt_it {
        j.record(&receipt(&c.dep, ResourceKind::Image, &c.tag, &c.iid))
            .unwrap();
    }
    extra(&mut j, &c);
    c
}

fn attempt(c: &C, kind: ResourceKind, name: &str) -> LedgerRecord {
    LedgerRecord::CleanupAttempt {
        key: ResourceKey::new(kind, name),
        deployment: c.dep.clone(),
        at_ms: 5,
    }
}
fn result(c: &C, kind: ResourceKind, name: &str, o: CleanupOutcome) -> LedgerRecord {
    LedgerRecord::CleanupResult {
        key: ResourceKey::new(kind, name),
        deployment: c.dep.clone(),
        outcome: o,
        at_ms: 6,
    }
}

fn auth() -> String {
    format!("--{AUTH_FLAG}={AUTH_PHRASE}")
}

fn args(t: &T, c: &C, extra: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = vec![
        "cleanup-image".into(),
        "--deployment".into(),
        c.dep.to_string(),
        "--image".into(),
        c.iid.clone(),
        "--docker-socket".into(),
        t.socket(),
        "--resources-dir".into(),
        t.resources(),
    ];
    v.extend(extra.iter().map(|s| s.to_string()));
    v
}

fn dry(t: &T, c: &C) -> Outcome {
    go(t, &SHIPPING, &args(t, c, &[]))
}

fn apply(t: &T, c: &C) -> Outcome {
    go(t, &SHIPPING, &args(t, c, &["--apply", &auth()]))
}

fn entry(t: &T, c: &C) -> Entry {
    read_only(&t.journal_dir())
        .unwrap()
        .ledger
        .entries()
        .iter()
        .find(|e| e.deployment == c.dep && e.key.kind == ResourceKind::Image)
        .expect("the image entry")
        .clone()
}

fn is_conflict(t: &T, c: &C) -> bool {
    let r = read_only(&t.journal_dir()).unwrap();
    let e = r
        .ledger
        .entries()
        .iter()
        .find(|e| e.deployment == c.dep && e.key.kind == ResourceKind::Image)
        .unwrap();
    r.ledger
        .deferred_image_conflict(e, Some(EndState::NotSucceeded))
}

fn image_present(t: &T, c: &C) -> bool {
    t.h.docker.with(|s| s.images.iter().any(|i| i.id == c.iid))
}

fn image_deletes(t: &T) -> Vec<String> {
    t.h.docker
        .requests()
        .into_iter()
        .filter(|r| r.method == "DELETE")
        .map(|r| r.path)
        .collect()
}

fn container_from(t: &T, name: &str, image: &str, running: bool) -> String {
    t.h.docker.with(|s| {
        let id = s.add_container(name, vec![], running);
        s.containers.last_mut().unwrap().image = image.to_string();
        id
    })
}

/// The command refused (exit 6) with this explanation. Callers assert separately that the journal
/// and Docker are unchanged.
fn refused_with(o: Outcome, expect_in_stderr: &str) {
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains(expect_in_stderr),
        "wanted {expect_in_stderr:?} in:\n{}",
        o.stderr
    );
}

fn deploy_is_blocked(t: &T) {
    let n = writes(t);
    let o = t.go(&OPEN, &refs(&live_args(t)));
    assert_exit(&o, 6);
    assert_eq!(writes(t), n, "a refused deploy changed Docker");
}

fn deploy_is_permitted(t: &T) {
    image_appears_after_build(t);
    let o = t.go(&OPEN, &refs(&live_args(t)));
    assert_exit(&o, 7); // it ran (nothing listens on the port); it was not refused
    assert!(
        o.stdout.contains("failure at preliminary_check"),
        "{}",
        o.stdout
    );
}

// ================================================================ 1. a failed deployment, end to end

#[test]
fn a_failed_deployments_retained_image_is_removed_by_its_recorded_id() {
    let t = T::new();
    let foreign = add_foreign(&t);
    image_appears_after_build(&t);
    assert_exit(&t.go(&OPEN, &refs(&live_args(&t))), 7); // fails at the preliminary check
    let r = read_only(&t.journal_dir()).unwrap();
    let d = r.deployments[0].clone();
    let e = r
        .ledger
        .entries()
        .iter()
        .find(|e| e.key.kind == ResourceKind::Image)
        .unwrap()
        .clone();
    let c = C {
        app: d.application_id.clone(),
        dep: d.id.clone(),
        tag: e.key.name.clone(),
        iid: e.id.clone().unwrap(),
    };
    assert!(image_present(&t, &c));

    // it is listed, so the command can be found
    let s = t.cli(&["status"]);
    assert!(
        s.stdout
            .contains("retained images of unsuccessful deployments"),
        "{}",
        s.stdout
    );
    assert!(
        s.stdout.contains("unresolved cleanup: none"),
        "{}",
        s.stdout
    );

    // dry run: reads, proposes, writes nothing
    let (journal, n) = (t.journal_bytes(), writes(&t));
    let o = dry(&t, &c);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("would: remove image") && o.stdout.contains("force=false"),
        "{}",
        o.stdout
    );
    assert_eq!((t.journal_bytes(), writes(&t)), (journal, n));

    // apply: one DELETE, by id, without force
    let o = apply(&t, &c);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("cleanup-image: complete"), "{}", o.stdout);
    assert!(!image_present(&t, &c));
    let d = image_deletes(&t);
    let image_delete: Vec<_> = d.iter().filter(|p| p.starts_with("/images/")).collect();
    assert_eq!(image_delete.len(), 1, "{d:?}");
    assert!(
        image_delete[0].ends_with("?force=false&noprune=true"),
        "{}",
        image_delete[0]
    );
    assert!(
        image_delete[0].contains(c.iid.trim_start_matches("sha256:"))
            || image_delete[0].contains(&c.iid)
    );
    assert_foreign_untouched(&t, &foreign);
    assert!(matches!(
        entry(&t, &c).state,
        EntryState::Released(Released::Removed)
    ));
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("unresolved cleanup: none")
            && !s.stdout.contains("retained images of unsuccessful"),
        "{}",
        s.stdout
    );

    // idempotent
    let (journal, n) = (t.journal_bytes(), writes(&t));
    let o = apply(&t, &c);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("nothing to do"), "{}", o.stdout);
    assert_eq!((t.journal_bytes(), writes(&t)), (journal, n));
}

// ================================================================ 2. an interrupted deployment

#[test]
fn an_interrupted_deployments_image_waits_for_reconcile_and_is_then_removed() {
    let t = T::new();
    let foreign = add_foreign(&t);
    interrupt_a_live_deploy(&t);
    assert_exit(&t.cli(&["recover"]), 0);
    let r = read_only(&t.journal_dir()).unwrap();
    let d = r.deployments[0].clone();
    let e = r
        .ledger
        .entries()
        .iter()
        .find(|e| e.key.kind == ResourceKind::Image)
        .unwrap()
        .clone();
    let c = C {
        app: d.application_id.clone(),
        dep: d.id.clone(),
        tag: e.key.name.clone(),
        iid: e.id.clone().unwrap(),
    };

    // its container is not reconciled yet: refused, nothing changed
    let (journal, n) = (t.journal_bytes(), writes(&t));
    refused_with(apply(&t, &c), "reconcile");
    assert_eq!((t.journal_bytes(), writes(&t)), (journal, n));

    let auth = auth();
    assert_exit(
        &go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth])),
        0,
    );
    assert!(image_present(&t, &c), "reconcile keeps a retained image");
    assert_exit(&apply(&t, &c), 0);
    assert!(!image_present(&t, &c));
    assert_foreign_untouched(&t, &foreign);
    deploy_is_permitted(&t);
}

// ================================================================ 3. naming the wrong thing

#[test]
fn the_deployment_and_the_image_id_must_both_match_the_journal() {
    let t = T::new();
    let c = craft(&t, 1, DeploymentStatus::Failed);
    let other = craft(&t, 2, DeploymentStatus::Failed);
    let (journal, n) = (t.journal_bytes(), writes(&t));
    // another deployment's image id under this deployment
    let mut a = args(&t, &c, &["--apply", &auth()]);
    a[4] = other.iid.clone();
    refused_with(go(&t, &SHIPPING, &a), "no recorded image");
    // a well-formed id that nothing records
    a[4] = support::fake_iid(424242);
    refused_with(go(&t, &SHIPPING, &a), "no recorded image");
    // a deployment that is not in the journal
    let mut a = args(&t, &c, &["--apply", &auth()]);
    a[2] = ids(99).1.to_string();
    refused_with(go(&t, &SHIPPING, &a), "not in the journal");
    // not an id: a tag, a prefix, upper case, a bare hex string, nothing
    for bad in [
        c.tag.clone(),
        c.iid[..20].to_string(),
        c.iid.to_uppercase(),
        c.iid.trim_start_matches("sha256:").to_string(),
        String::new(),
    ] {
        let mut a = args(&t, &c, &["--apply", &auth()]);
        a[4] = bad;
        assert_exit(&go(&t, &SHIPPING, &a), 2);
    }
    // required flags and the absence of a force flag
    for (drop, code) in [("--deployment", 2), ("--image", 2), ("--docker-socket", 5)] {
        let a = args(&t, &c, &["--apply", &auth()]);
        let i = a.iter().position(|x| x == drop).unwrap();
        let mut b = a.clone();
        b.drain(i..i + 2);
        assert_exit(&go(&t, &SHIPPING, &b), code);
    }
    assert_exit(&go(&t, &SHIPPING, &args(&t, &c, &["--force"])), 2);
    assert_exit(&go(&t, &SHIPPING, &args(&t, &c, &["--prune"])), 2);
    assert_eq!((t.journal_bytes(), writes(&t)), (journal, n));
    assert!(image_present(&t, &c) && image_present(&t, &other));
}

// ================================================================ 4. what the deployment still owns

#[test]
fn only_a_finished_unsuccessful_deployment_with_everything_else_released_qualifies() {
    for (status, want) in [
        (DeploymentStatus::Succeeded, "teardown"),
        (DeploymentStatus::Running, "recover"),
        (DeploymentStatus::Queued, "recover"),
    ] {
        let t = T::new();
        let c = craft(&t, 3, status);
        let (journal, n) = (t.journal_bytes(), writes(&t));
        refused_with(apply(&t, &c), want);
        assert_eq!((t.journal_bytes(), writes(&t)), (journal, n), "{status}");
        assert!(image_present(&t, &c));
    }
    for status in [
        DeploymentStatus::Failed,
        DeploymentStatus::Interrupted,
        DeploymentStatus::Cancelled,
    ] {
        let t = T::new();
        let c = craft(&t, 3, status);
        assert_exit(&apply(&t, &c), 0);
        assert!(!image_present(&t, &c), "{status}");
    }
    // an unreleased container, an unreleased lease, a container with no receipt
    for (kind, name, with_receipt) in [
        (ResourceKind::Container, "hp-leftover", true),
        (ResourceKind::PortLease, "38000", true),
        (ResourceKind::Container, "hp-unknown", false),
    ] {
        let t = T::new();
        let c = craft_with(&t, 4, DeploymentStatus::Failed, true, |j, c| {
            j.record(&intent(
                &c.app,
                &c.dep,
                kind,
                name,
                Disposition::RemoveOnFailure,
            ))
            .unwrap();
            if with_receipt {
                j.record(&receipt(&c.dep, kind, name, "id-1")).unwrap();
            }
        });
        let (journal, n) = (t.journal_bytes(), writes(&t));
        refused_with(apply(&t, &c), "reconcile");
        assert_eq!((t.journal_bytes(), writes(&t)), (journal, n), "{name}");
    }
    // an image with no receipt cannot be named by id; reconcile resolves it
    let t = T::new();
    let c = craft_with(&t, 5, DeploymentStatus::Failed, false, |_, _| {});
    refused_with(apply(&t, &c), "reconcile");
    assert!(image_present(&t, &c));
}

// ================================================================ 5. containers in any state

#[test]
fn a_container_in_any_state_that_uses_the_image_stops_the_removal() {
    for (what, running, image_of) in [
        ("running", true, true),
        ("stopped", false, true),
        ("image unknown", false, false),
    ] {
        let t = T::new();
        let c = craft(&t, 6, DeploymentStatus::Failed);
        container_from(
            &t,
            "someone-elses-container",
            if image_of { &c.iid } else { "" },
            running,
        );
        let (journal, n) = (t.journal_bytes(), writes(&t));
        refused_with(dry(&t, &c), "someone-elses-container");
        refused_with(apply(&t, &c), "someone-elses-container");
        assert_eq!(
            (t.journal_bytes(), writes(&t)),
            (journal, n),
            "{what}: nothing may be written"
        );
        assert!(
            image_deletes(&t).is_empty(),
            "{what}: Docker was never asked to delete"
        );
        assert!(image_present(&t, &c));
    }
}

// ================================================================ 6. tags, digests, shared references

#[test]
fn another_tag_a_digest_or_a_shared_ledger_entry_means_not_exclusive() {
    // another tag
    let t = T::new();
    let c = craft(&t, 7, DeploymentStatus::Failed);
    t.h.docker
        .with(|s| s.images.last_mut().unwrap().tags.push("foo:latest".into()));
    let (journal, n) = (t.journal_bytes(), writes(&t));
    refused_with(apply(&t, &c), "foo:latest");
    assert_eq!((t.journal_bytes(), writes(&t)), (journal, n));
    // when the other tag is removed it works
    t.h.docker.with(|s| {
        s.images
            .last_mut()
            .unwrap()
            .tags
            .retain(|x| x != "foo:latest")
    });
    assert_exit(&apply(&t, &c), 0);

    // a registry digest
    let t = T::new();
    let c = craft(&t, 7, DeploymentStatus::Failed);
    t.h.docker.with(|s| {
        s.images
            .last_mut()
            .unwrap()
            .digests
            .push("registry.example/x@sha256:abc".into())
    });
    refused_with(apply(&t, &c), "digest");
    assert!(image_deletes(&t).is_empty() && image_present(&t, &c));

    // another deployment's unreleased ledger entry for the same image id
    let t = T::new();
    let c = craft(&t, 7, DeploymentStatus::Failed);
    let (app2, dep2) = ids(8);
    let tag2 = "harpoon/other:dep-2".to_string();
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&finished(&app2, &dep2, DeploymentStatus::Failed))
        .unwrap();
    j.record(&intent(
        &app2,
        &dep2,
        ResourceKind::Image,
        &tag2,
        Disposition::Retain,
    ))
    .unwrap();
    j.record(&receipt(&dep2, ResourceKind::Image, &tag2, &c.iid))
        .unwrap();
    drop(j);
    refused_with(apply(&t, &c), "shared");
    assert!(image_deletes(&t).is_empty() && image_present(&t, &c));
}

// ================================================================ 7. F1: ownership refusal stays terminal and blocking

#[test]
fn a_label_mismatch_is_the_terminal_f1_refusal_and_keeps_blocking_deployments() {
    let t = T::new();
    let c = craft(&t, 9, DeploymentStatus::Failed);
    let saved = t.h.docker.with(|s| s.images.last().unwrap().labels.clone());
    t.h.docker
        .with(|s| s.images.last_mut().unwrap().labels.remove(2)); // the deployment label

    // the dry run says so and writes nothing
    let (journal, n) = (t.journal_bytes(), writes(&t));
    let o = dry(&t, &c);
    assert_exit(&o, 6);
    assert!(o.stdout.contains("REFUSE"), "{}", o.stdout);
    assert_eq!((t.journal_bytes(), writes(&t)), (journal, n));

    // apply records the same terminal refusal reconcile and teardown record, and deletes nothing
    let o = apply(&t, &c);
    assert_exit(&o, 7);
    assert!(o.stdout.contains("refused"), "{}", o.stdout);
    assert!(matches!(entry(&t, &c).state, EntryState::Refused(_)));
    assert!(image_deletes(&t).is_empty() && image_present(&t, &c));
    let s = t.cli(&["status"]);
    assert!(s.stdout.contains("ownership not proven"), "{}", s.stdout);
    deploy_is_blocked(&t);

    // terminal: repeating the command, or restoring the labels, reopens nothing
    let (journal, n) = (t.journal_bytes(), writes(&t));
    refused_with(apply(&t, &c), "refused earlier");
    t.h.docker
        .with(|s| s.images.last_mut().unwrap().labels = saved.clone());
    refused_with(apply(&t, &c), "refused earlier");
    assert_eq!((t.journal_bytes(), writes(&t)), (journal, n));
    assert!(!is_conflict(&t, &c));
    deploy_is_blocked(&t);
}

#[test]
fn an_image_id_that_docker_does_not_confirm_is_also_the_terminal_refusal() {
    let t = T::new();
    let c = craft(&t, 9, DeploymentStatus::Failed);
    let body = format!(
        "{{\"Id\":\"{}\",\"RepoTags\":[],\"RepoDigests\":[],\"Config\":{{\"Labels\":{{}}}}}}",
        support::fake_iid(31337)
    );
    t.h.docker.fault(
        Some("GET"),
        "/images/sha256:",
        50,
        Action::Respond(200, body),
    );
    assert_exit(&dry(&t, &c), 6);
    assert_exit(&apply(&t, &c), 7);
    assert!(matches!(entry(&t, &c).state, EntryState::Refused(_)));
    t.h.docker.with(|s| s.faults.clear());
    assert!(image_deletes(&t).is_empty() && image_present(&t, &c));
    deploy_is_blocked(&t);
}

// ================================================================ 8. absent, unreachable

#[test]
fn an_absent_image_is_recorded_as_absent_and_an_unreachable_docker_changes_nothing() {
    let t = T::new();
    let c = craft(&t, 10, DeploymentStatus::Failed);
    t.h.docker.with(|s| s.images.retain(|i| i.id != c.iid));
    let o = dry(&t, &c);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("already absent"), "{}", o.stdout);
    assert_exit(&apply(&t, &c), 0);
    assert!(matches!(
        entry(&t, &c).state,
        EntryState::Released(Released::AlreadyAbsent)
    ));
    assert!(image_deletes(&t).is_empty(), "nothing to delete");

    for (what, path) in [
        ("container list", "/containers/json"),
        ("image inspect", "/images/sha256:"),
    ] {
        let t = T::new();
        let c = craft(&t, 10, DeploymentStatus::Failed);
        t.h.docker.fault(
            Some("GET"),
            path,
            50,
            Action::Respond(500, "{\"message\":\"boom\"}".into()),
        );
        let (journal, n) = (t.journal_bytes(), writes(&t));
        for o in [dry(&t, &c), apply(&t, &c)] {
            assert_exit(&o, 5);
            assert!(
                o.stderr.contains("Nothing was assumed"),
                "{what}: {}",
                o.stderr
            );
        }
        assert_eq!((t.journal_bytes(), writes(&t)), (journal, n), "{what}");
        assert!(image_present(&t, &c));
    }
}

// ================================================================ 9. Docker 409: the narrow exception

#[test]
fn a_conflict_is_recorded_does_not_release_the_image_and_does_not_block_unrelated_deployments() {
    let t = T::new();
    let foreign = add_foreign(&t);
    let c = craft(&t, 11, DeploymentStatus::Failed);
    // a container appears between the checks and the deletion
    let iid = c.iid.clone();
    t.h.docker.fault(
        Some("DELETE"),
        "/images/",
        1,
        Action::Race(Arc::new(move |s: &mut St| {
            s.add_container("sneaky", vec![], false);
            s.containers.last_mut().unwrap().image = iid.clone();
        })),
    );
    let o = apply(&t, &c);
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("kept:") && o.stdout.contains("does not block other deployments"),
        "{}",
        o.stdout
    );
    // recorded, not released, and the image is intact
    assert!(matches!(
        entry(&t, &c).state,
        EntryState::CleanupConflict(..)
    ));
    assert!(
        is_conflict(&t, &c),
        "the classification is read back from the journal"
    );
    assert!(image_present(&t, &c));
    for d in image_deletes(&t) {
        assert!(
            d.contains("force=false") && !d.contains("force=true"),
            "{d}"
        );
    }
    // visible, but not a blocker; the command to retry is named
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("unresolved cleanup: none"),
        "{}",
        s.stdout
    );
    assert!(
        s.stdout.contains("cleanup refused as a conflict") && s.stdout.contains("cleanup-image"),
        "{}",
        s.stdout
    );
    // an unrelated deployment can run
    deploy_is_permitted(&t);
    assert_foreign_untouched(&t, &foreign);
    // explicit retry: while the cause stays, it is refused before Docker is asked
    let n = image_deletes(&t).len();
    refused_with(apply(&t, &c), "sneaky");
    assert_eq!(image_deletes(&t).len(), n);
    // the cause goes away; the retry releases the image
    t.h.docker
        .with(|s| s.containers.retain(|x| x.name != "sneaky"));
    assert_exit(&apply(&t, &c), 0);
    assert!(matches!(
        entry(&t, &c).state,
        EntryState::Released(Released::Removed)
    ));
    assert!(!image_present(&t, &c));
}

#[test]
fn docker_itself_refusing_the_deletion_is_a_conflict_by_status_code() {
    // The assessment finds nothing wrong (no container, one tag); Docker still answers 409.
    let t = T::new();
    let c = craft(&t, 12, DeploymentStatus::Failed);
    t.h.docker.fault(
        Some("DELETE"),
        "/images/",
        1,
        Action::Respond(409, "{\"message\":\"anything at all\"}".into()),
    );
    assert_exit(&apply(&t, &c), 7);
    assert!(is_conflict(&t, &c));
    assert!(image_present(&t, &c));
    deploy_is_permitted(&t);
    // it survives a process restart (every command re-reads the journal) and a retry releases it
    assert_exit(&apply(&t, &c), 0);
    assert!(!image_present(&t, &c));
}

#[test]
fn a_cause_found_after_the_attempt_was_journaled_is_a_conflict_and_no_delete_is_sent() {
    let t = T::new();
    let c = craft(&t, 13, DeploymentStatus::Failed);
    // the first container listing is the command's own check; the second is the pre-delete one
    let iid = c.iid.clone();
    t.h.docker.fault_after(
        Some("GET"),
        "/containers/json",
        1,
        1,
        Action::Race(Arc::new(move |s: &mut St| {
            s.add_container("late", vec![], true);
            s.containers.last_mut().unwrap().image = iid.clone();
        })),
    );
    assert_exit(&apply(&t, &c), 7);
    assert!(
        image_deletes(&t).is_empty(),
        "Docker was never asked to delete"
    );
    assert!(is_conflict(&t, &c) && image_present(&t, &c));
    deploy_is_permitted(&t);
}

#[test]
fn a_conflict_does_not_block_only_while_everything_else_of_the_deployment_is_released() {
    let conflict = |c: &C| {
        result(
            c,
            ResourceKind::Image,
            &c.tag,
            CleanupOutcome::Conflict(ConflictCause::DockerRefused, "x".into()),
        )
    };
    // a conflict alone: not blocking
    let t = T::new();
    let _ = craft_with(&t, 14, DeploymentStatus::Failed, true, |j, c| {
        j.record(&attempt(c, ResourceKind::Image, &c.tag)).unwrap();
        j.record(&conflict(c)).unwrap();
    });
    deploy_is_permitted(&t);
    // the same with an active container of that deployment: blocking
    let t = T::new();
    let _ = craft_with(&t, 14, DeploymentStatus::Failed, true, |j, c| {
        j.record(&intent(
            &c.app,
            &c.dep,
            ResourceKind::Container,
            "hp-x",
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
        j.record(&receipt(&c.dep, ResourceKind::Container, "hp-x", "cid"))
            .unwrap();
        j.record(&attempt(c, ResourceKind::Image, &c.tag)).unwrap();
        j.record(&conflict(c)).unwrap();
    });
    assert!(t.cli(&["status"]).stdout.contains("unresolved cleanup ("));
    deploy_is_blocked(&t);
    // the same for a deployment that succeeded (a forged or misplaced marker relaxes nothing)
    let t = T::new();
    let _ = craft_with(&t, 14, DeploymentStatus::Succeeded, true, |j, c| {
        j.record(&attempt(c, ResourceKind::Image, &c.tag)).unwrap();
        j.record(&conflict(c)).unwrap();
    });
    deploy_is_blocked(&t);
    // and for a marker on a container entry
    let t = T::new();
    let _ = craft_with(&t, 14, DeploymentStatus::Failed, true, |j, c| {
        j.record(&intent(
            &c.app,
            &c.dep,
            ResourceKind::Container,
            "hp-y",
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
        j.record(&receipt(&c.dep, ResourceKind::Container, "hp-y", "cid"))
            .unwrap();
        j.record(&attempt(c, ResourceKind::Container, "hp-y"))
            .unwrap();
        j.record(&result(
            c,
            ResourceKind::Container,
            "hp-y",
            CleanupOutcome::Failed("image-conflict: x".into()),
        ))
        .unwrap();
    });
    deploy_is_blocked(&t);
}

// ================================================================ 10. ambiguous outcomes stay blocking

#[test]
fn ambiguous_failures_are_recorded_keep_the_runtime_blocked_and_settle_on_retry() {
    for (name, f, deleted) in [
        (
            "http 500",
            Action::Respond(500, "{\"message\":\"boom\"}".into()),
            false,
        ),
        ("disconnect", Action::Disconnect, false),
        ("reply lost after the delete", Action::ApplyThenDrop, true),
    ] {
        let t = T::new();
        let c = craft(&t, 15, DeploymentStatus::Failed);
        t.h.docker.fault(Some("DELETE"), "/images/", 1, f);
        let o = apply(&t, &c);
        assert_exit(&o, 7);
        assert!(o.stdout.contains("not certain"), "{name}: {}", o.stdout);
        assert!(
            matches!(entry(&t, &c).state, EntryState::CleanupFailed(_)),
            "{name}"
        );
        assert!(
            !is_conflict(&t, &c),
            "{name}: not a typed conflict, so it blocks"
        );
        assert_eq!(image_present(&t, &c), !deleted, "{name}");
        let s = t.cli(&["status"]);
        assert!(
            s.stdout.contains("unresolved cleanup ("),
            "{name}: {}",
            s.stdout
        );
        deploy_is_blocked(&t);
        // a repeat settles it (removed, or found already gone) and the runtime opens again
        assert_exit(&apply(&t, &c), 0);
        assert!(!image_present(&t, &c), "{name}");
        deploy_is_permitted(&t);
    }
}

// ================================================================ 11. restart after interruption

#[test]
fn an_attempt_with_no_recorded_result_blocks_until_it_is_retried_and_then_settles() {
    for image_still_there in [true, false] {
        let t = T::new();
        let c = craft_with(&t, 16, DeploymentStatus::Failed, true, |j, c| {
            j.record(&attempt(c, ResourceKind::Image, &c.tag)).unwrap();
        });
        if !image_still_there {
            t.h.docker.with(|s| s.images.retain(|i| i.id != c.iid));
        }
        assert!(t.cli(&["status"]).stdout.contains("never recorded"));
        deploy_is_blocked(&t);
        assert_exit(&apply(&t, &c), 0);
        let want = if image_still_there {
            Released::Removed
        } else {
            Released::AlreadyAbsent
        };
        assert!(matches!(entry(&t, &c).state, EntryState::Released(r) if r == want));
        assert_eq!(entry(&t, &c).cleanup_attempts, 2);
        deploy_is_permitted(&t);
    }
}

// ================================================================ 12. concurrency

#[test]
fn apply_takes_the_single_writer_lock_and_fails_fast_while_a_dry_run_still_reads() {
    let t = T::new();
    let c = craft(&t, 17, DeploymentStatus::Failed);
    let (_held, _) = open_j(&t.journal_dir()).unwrap();
    let n = writes(&t);
    assert_exit(&dry(&t, &c), 0);
    let o = apply(&t, &c);
    assert_exit(&o, 4);
    assert_eq!(writes(&t), n);
    assert!(image_present(&t, &c));
}

// ================================================================ 13. authorization and runtime kind

#[test]
fn mutation_needs_the_exact_phrase_and_a_real_runtime_and_is_decided_before_docker_is_contacted() {
    let t = T::new();
    let c = craft(&t, 18, DeploymentStatus::Failed);
    let before = t.h.docker.requests().len();
    assert_exit(&go(&t, &SHIPPING, &args(&t, &c, &["--apply"])), 6);
    assert_exit(
        &go(
            &t,
            &SHIPPING,
            &args(&t, &c, &["--apply", &format!("--{AUTH_FLAG}=yes")]),
        ),
        6,
    );
    assert_eq!(
        t.h.docker.requests().len(),
        before,
        "Docker was not contacted"
    );
    // no journal at all
    let t2 = T::new();
    let c2 = C {
        app: c.app.clone(),
        dep: c.dep.clone(),
        tag: c.tag.clone(),
        iid: c.iid.clone(),
    };
    assert_exit(&dry(&t2, &c2), 6);
    // a simulated runtime has nothing real to clean
    let t3 = T::new();
    assert_exit(&t3.sim_deploy(), 0);
    let o = go(&t3, &SHIPPING, &args(&t3, &c, &[]));
    assert_exit(&o, 6);
    assert!(o.stderr.contains("simulated"), "{}", o.stderr);
    assert_eq!(t3.h.docker.requests().len(), 0);
    // the dry run needs an explicit socket like every Docker command
    let a = args(&t, &c, &[]);
    let i = a.iter().position(|x| x == "--docker-socket").unwrap();
    let mut b = a.clone();
    b.drain(i..i + 2);
    assert_exit(&go(&t, &SHIPPING, &b), 5);
}

// ================================================================ 14. only the named deployment

#[test]
fn only_the_named_deployments_image_is_touched() {
    let t = T::new();
    let a = craft(&t, 19, DeploymentStatus::Failed);
    let b = craft(&t, 20, DeploymentStatus::Failed);
    let foreign = add_foreign(&t);
    assert_exit(&apply(&t, &a), 0);
    assert!(!image_present(&t, &a) && image_present(&t, &b));
    assert!(matches!(entry(&t, &b).state, EntryState::Created));
    let d = image_deletes(&t);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].contains(a.iid.trim_start_matches("sha256:")) || d[0].contains(&a.iid));
    assert_foreign_untouched(&t, &foreign);
    // and the other one is still individually removable
    assert_exit(&apply(&t, &b), 0);
    assert!(!image_present(&t, &b));
}

// ================================================================ 15. no force, no prune

#[test]
fn the_command_has_no_force_or_prune_surface_and_the_help_names_it() {
    let h = harpoon_deploy_cli::HELP;
    assert!(h.contains("cleanup-image"));
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands.rs"),
    )
    .unwrap();
    let start = src.find("cleanup-image (B8.2)").expect("section");
    let section = &src[start..];
    for bad in [
        "prune",
        "force=true",
        "\"force\"",
        "\"all\"",
        "starts_with(\"harpoon",
        "contains(\"harpoon",
    ] {
        assert!(
            !section
                .lines()
                .any(|l| l.contains(bad) && !l.trim_start().starts_with("//")),
            "{bad}"
        );
    }
    let _ = harpoon_deploy::time::SystemClock.now_ms();
}

// ================================================================ the classification cannot be forged from text

#[test]
fn a_failed_cleanup_whose_text_looks_like_a_conflict_still_blocks_and_a_repeat_settles_it() {
    for text in [
        "image-conflict: Docker refused to delete image (HTTP 409)",
        "conflict-docker-refused",
        "Docker answered 409 Conflict",
    ] {
        let t = T::new();
        let c = craft_with(&t, 21, DeploymentStatus::Failed, true, |j, c| {
            j.record(&attempt(c, ResourceKind::Image, &c.tag)).unwrap();
            j.record(&result(
                c,
                ResourceKind::Image,
                &c.tag,
                CleanupOutcome::Failed(text.into()),
            ))
            .unwrap();
        });
        assert!(matches!(entry(&t, &c).state, EntryState::CleanupFailed(_)));
        assert!(!is_conflict(&t, &c), "{text:?}");
        let s = t.cli(&["status"]);
        assert!(
            s.stdout.contains("unresolved cleanup ("),
            "{text:?}: {}",
            s.stdout
        );
        deploy_is_blocked(&t);
        // only an explicit retry that ends in a typed outcome (or a release) changes that
        assert_exit(&apply(&t, &c), 0);
        deploy_is_permitted(&t);
    }
}

#[test]
fn the_typed_conflict_is_a_journal_token_of_its_own_and_survives_a_restart() {
    let t = T::new();
    let c = craft(&t, 22, DeploymentStatus::Failed);
    t.h.docker.fault(
        Some("DELETE"),
        "/images/",
        1,
        Action::Respond(409, "{\"message\":\"x\"}".into()),
    );
    assert_exit(&apply(&t, &c), 7);
    let journal = String::from_utf8(t.journal_bytes()).unwrap();
    let result_line = journal
        .lines()
        .rfind(|l| l.contains("\tresult\t"))
        .unwrap()
        .to_string();
    assert!(
        result_line.contains("\tconflict-docker-refused\t"),
        "{result_line}"
    );
    assert!(!result_line.contains("\tfailed\t"), "{result_line}");
    // every command re-reads the file: that is a restart
    assert!(is_conflict(&t, &c));
    assert!(t
        .cli(&["status"])
        .stdout
        .contains("cleanup refused as a conflict"));
    deploy_is_permitted(&t);
    assert!(is_conflict(&t, &c), "still the same after another run");
}

#[test]
fn a_misplaced_unknown_or_damaged_conflict_record_makes_the_whole_journal_unusable() {
    use harpoon_deploy::ledger::codec;
    let append = |t: &T, line: &str| {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(t.journal_dir().join("journal.wal"))
            .unwrap();
        f.write_all(line.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
    };
    let setup = |seed: u8| {
        let t = T::new();
        let c = craft_with(&t, seed, DeploymentStatus::Failed, true, |j, c| {
            j.record(&intent(
                &c.app,
                &c.dep,
                ResourceKind::Container,
                "hp-z",
                Disposition::RemoveOnFailure,
            ))
            .unwrap();
            j.record(&receipt(&c.dep, ResourceKind::Container, "hp-z", "cid"))
                .unwrap();
            j.record(&attempt(c, ResourceKind::Container, "hp-z"))
                .unwrap();
        });
        (t, c)
    };
    // a typed conflict on a container: well-formed, checksummed, and impossible
    let (t, c) = setup(23);
    append(
        &t,
        &codec::encode(&result(
            &c,
            ResourceKind::Container,
            "hp-z",
            CleanupOutcome::Conflict(ConflictCause::NotExclusive, "x".into()),
        )),
    );
    // an outcome token this build does not know, with a valid checksum
    let (t2, c2) = setup(24);
    let good = codec::encode(&result(
        &c2,
        ResourceKind::Container,
        "hp-z",
        CleanupOutcome::Conflict(ConflictCause::NotExclusive, "x".into()),
    ));
    let body = good
        .split_once('\t')
        .unwrap()
        .1
        .replace("conflict-not-exclusive", "conflict-from-the-future");
    let crc = {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in body.as_bytes() {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    };
    append(&t2, &format!("{crc:08x}\t{body}"));
    // a damaged checksum
    let (t3, c3) = setup(25);
    let good = codec::encode(&result(
        &c3,
        ResourceKind::Container,
        "hp-z",
        CleanupOutcome::Conflict(ConflictCause::NotExclusive, "x".into()),
    ));
    append(&t3, &format!("0{}", &good[1..]));
    for (name, t) in [
        ("misplaced", &t),
        ("unknown token", &t2),
        ("bad checksum", &t3),
    ] {
        let before = t.journal_bytes();
        for a in [vec!["status"], vec!["recover"], vec!["doctor"]] {
            let o = t.cli(&a);
            assert_exit(&o, 4);
            let _ = name;
        }
        deploy_is_refused_by_the_journal(t);
        assert_eq!(
            t.journal_bytes(),
            before,
            "{name}: the journal was left untouched"
        );
    }
}

fn deploy_is_refused_by_the_journal(t: &T) {
    let n = writes(t);
    let o = t.go(&OPEN, &refs(&live_args(t)));
    assert_exit(&o, 4);
    assert_eq!(writes(t), n);
}
