//! B8.2: retained-image cleanup at the Docker-backend level, against the *simulated* Docker server.
//! Nothing here contacts a Docker Engine, builds an image or starts a container.
//!
//! The simulated server's image deletion follows Docker's documented semantics (see
//! `fake_docker.rs`): without `force`, an image used by any container, with dependent child images,
//! or addressed by id while several tags point at it, is a 409 conflict. That behaviour has not been
//! observed on a real Engine in this repository.

mod support;

use harpoon_deploy::backend::{ResourceOps, StepError};
use harpoon_deploy::engine::{execute_actions, EngineError, JournalError};
use harpoon_deploy::health::Observation;
use harpoon_deploy::ledger::{
    Action, ConflictCause, Disposition, EndState, EntryState, Ledger, LedgerRecord, Observed,
    Released, RemoveRequest, RemoveResult, ResourceKey, ResourceKind,
};
use harpoon_deploy_docker::backend::{DockerBackend, ImageVerdict};
use harpoon_deploy_docker::probe::HttpTransport;
use support::fake_docker::{Action as Fault, St};
use support::*;

fn world() -> (Harness, String, String) {
    let h = Harness::new();
    let tag = tag_for(&h.dep.application_id, &h.dep.id);
    let iid = fake_iid(77);
    let l = labels(&h.dep.application_id, &h.dep.id);
    h.docker
        .with(|s| s.add_image_with_id(&iid, &[&tag], l, vec![]));
    (h, tag, iid)
}

fn backend(h: &Harness) -> DockerBackend<Scripted> {
    h.backend(Scripted::always(Observation::Status(200)))
}

fn assess(
    h: &Harness,
    b: &DockerBackend<Scripted>,
    tag: &str,
    iid: &str,
) -> Result<ImageVerdict, StepError> {
    b.assess_retained_image(iid, tag, &h.dep.application_id, &h.dep.id)
}

fn req(h: &Harness, tag: &str, iid: &str) -> RemoveRequest {
    RemoveRequest {
        kind: ResourceKind::Image,
        id: iid.to_string(),
        name: tag.to_string(),
        app: h.dep.application_id.clone(),
        deployment: h.dep.id.clone(),
    }
}

fn deletes(h: &Harness) -> Vec<String> {
    h.docker
        .requests()
        .into_iter()
        .filter(|r| r.method == "DELETE")
        .map(|r| r.path)
        .collect()
}

fn add_container_from(h: &Harness, name: &str, image: &str, running: bool) -> String {
    h.docker.with(|s| {
        let id = s.add_container(name, vec![], running);
        s.containers.last_mut().unwrap().image = image.to_string();
        id
    })
}

// ---------------------------------------------------------------- the simulated Docker is faithful

#[test]
fn the_simulated_docker_refuses_the_deletions_the_real_one_refuses() {
    let (h, tag, iid) = world();
    let api = harpoon_deploy_docker::api::DockerApi::new(
        h.docker.path(),
        std::time::Duration::from_secs(3),
    );
    let t = std::time::Duration::from_secs(3);

    // a running container: 409, and not even force changes that
    let c = add_container_from(&h, "r", &iid, true);
    let e = api.remove_image(&iid, t).unwrap_err();
    assert!(e.is_conflict(), "{e}");
    assert!(h.docker.with(|s| s.images.iter().any(|i| i.id == iid)));
    // a stopped container: 409 without force
    h.docker.with(|s| s.containers.retain(|x| x.id != c));
    add_container_from(&h, "s", &iid, false);
    assert!(api.remove_image(&iid, t).unwrap_err().is_conflict());
    h.docker.with(|s| s.containers.clear());
    // several tags, addressed by id: 409
    h.docker
        .with(|s| s.images.last_mut().unwrap().tags.push("other:1".into()));
    assert!(api.remove_image(&iid, t).unwrap_err().is_conflict());
    h.docker.with(|s| s.images.last_mut().unwrap().tags.pop());
    // a dependent child image: 409
    h.docker.with(|s| {
        s.add_image_with_id(&fake_iid(78), &["child:1"], vec![], vec![]);
        s.images.last_mut().unwrap().parent = Some(iid.clone());
    });
    assert!(api.remove_image(&iid, t).unwrap_err().is_conflict());
    h.docker.with(|s| s.images.retain(|i| i.id != fake_iid(78)));
    // nothing was removed by any of that, and an unconflicted image is removed
    assert!(h.docker.with(|s| s.images.iter().any(|i| i.id == iid)));
    assert!(api.remove_image(&iid, t).unwrap());
    assert!(!api.remove_image(&iid, t).unwrap(), "404 is 'already gone'");
    drop(tag);
}

#[test]
fn a_tag_that_is_not_the_last_is_only_untagged() {
    let (h, tag, iid) = world();
    h.docker
        .with(|s| s.images.last_mut().unwrap().tags.push("other:1".into()));
    let api = harpoon_deploy_docker::api::DockerApi::new(
        h.docker.path(),
        std::time::Duration::from_secs(3),
    );
    assert!(api
        .remove_image(&tag, std::time::Duration::from_secs(3))
        .unwrap());
    h.docker.with(|s| {
        assert_eq!(s.images.last_mut().unwrap().id, iid);
        assert_eq!(
            s.images.last_mut().unwrap().tags,
            vec!["other:1".to_string()]
        );
    });
}

// ---------------------------------------------------------------- assessment

#[test]
fn an_unshared_labelled_image_with_only_its_tag_is_eligible() {
    let (h, tag, iid) = world();
    let b = backend(&h);
    assert_eq!(assess(&h, &b, &tag, &iid).unwrap(), ImageVerdict::Eligible);
    // an image with no tag at all (the tag was removed) is still eligible: it is addressed by id
    h.docker.with(|s| s.images.last_mut().unwrap().tags.clear());
    assert_eq!(assess(&h, &b, &tag, &iid).unwrap(), ImageVerdict::Eligible);
    // unrelated containers are irrelevant
    add_container_from(&h, "other", &fake_iid(5), true);
    assert_eq!(assess(&h, &b, &tag, &iid).unwrap(), ImageVerdict::Eligible);
    assert!(deletes(&h).is_empty(), "assessment is read-only");
    assert!(h.docker.requests().iter().all(|r| r.method == "GET"));
}

#[test]
fn an_absent_image_is_absent_and_an_unreachable_engine_is_an_error_never_absent() {
    let (h, tag, iid) = world();
    let b = backend(&h);
    h.docker.with(|s| s.images.clear());
    assert_eq!(assess(&h, &b, &tag, &iid).unwrap(), ImageVerdict::Absent);
    let (h, tag, iid) = world();
    let b = backend(&h);
    h.docker.fault(
        Some("GET"),
        "/images/",
        5,
        Fault::Respond(500, "{\"message\":\"x\"}".into()),
    );
    assert!(assess(&h, &b, &tag, &iid).is_err());
    let (h, tag, iid) = world();
    let b = backend(&h);
    h.docker
        .fault(Some("GET"), "/containers/json", 5, Fault::Disconnect);
    assert!(assess(&h, &b, &tag, &iid).is_err());
}

#[test]
fn ownership_failures_are_the_ownership_class() {
    let (h, tag, iid) = world();
    let b = backend(&h);
    for drop_label in 0..3 {
        let (h, tag, iid) = world();
        let b = backend(&h);
        h.docker.with(|s| {
            s.images.last_mut().unwrap().labels.remove(drop_label);
        });
        assert!(
            matches!(
                assess(&h, &b, &tag, &iid).unwrap(),
                ImageVerdict::Ownership(_)
            ),
            "label {drop_label} missing"
        );
    }
    // a label with the wrong value
    h.docker
        .with(|s| s.images.last_mut().unwrap().labels[2].1 = "dep_someone-else".into());
    assert!(matches!(
        assess(&h, &b, &tag, &iid).unwrap(),
        ImageVerdict::Ownership(_)
    ));
    // Docker answering for a different id than asked
    let (h, tag, iid) = world();
    let b = backend(&h);
    let body = format!(
        "{{\"Id\":\"{}\",\"RepoTags\":[],\"RepoDigests\":[],\"Config\":{{\"Labels\":{{}}}}}}",
        fake_iid(1234)
    );
    h.docker
        .fault(Some("GET"), "/images/sha256:", 5, Fault::Respond(200, body));
    assert!(matches!(
        assess(&h, &b, &tag, &iid).unwrap(),
        ImageVerdict::Ownership(_)
    ));
}

#[test]
fn anything_that_makes_removal_non_exclusive_is_not_eligible() {
    // another tag
    let (h, tag, iid) = world();
    let b = backend(&h);
    h.docker
        .with(|s| s.images.last_mut().unwrap().tags.push("foo:latest".into()));
    let ImageVerdict::NotExclusive(w) = assess(&h, &b, &tag, &iid).unwrap() else {
        panic!()
    };
    assert!(w.contains("foo:latest"), "{w}");
    // a registry digest
    let (h, tag, iid) = world();
    let b = backend(&h);
    h.docker.with(|s| {
        s.images
            .last_mut()
            .unwrap()
            .digests
            .push("r.example/x@sha256:abc".into())
    });
    assert!(matches!(
        assess(&h, &b, &tag, &iid).unwrap(),
        ImageVerdict::NotExclusive(_)
    ));
    // running, stopped, and unknown-image containers
    for (running, image, what) in [
        (true, true, "running"),
        (false, true, "stopped"),
        (false, false, "unknown"),
    ] {
        let (h, tag, iid) = world();
        let b = backend(&h);
        add_container_from(&h, "user", if image { &iid } else { "" }, running);
        let ImageVerdict::NotExclusive(w) = assess(&h, &b, &tag, &iid).unwrap() else {
            panic!("{what}")
        };
        assert!(w.contains("user"), "{what}: {w}");
    }
    // ownership is judged before exclusivity
    let (h, tag, iid) = world();
    let b = backend(&h);
    h.docker.with(|s| {
        s.images.last_mut().unwrap().labels.clear();
        s.images.last_mut().unwrap().tags.push("foo:latest".into());
    });
    assert!(matches!(
        assess(&h, &b, &tag, &iid).unwrap(),
        ImageVerdict::Ownership(_)
    ));
}

// ---------------------------------------------------------------- the deletion

#[test]
fn the_deletion_is_one_request_by_id_without_force_and_with_noprune() {
    let (h, tag, iid) = world();
    let mut b = backend(&h);
    let mut ops = b.retained_image_ops();
    assert_eq!(
        ops.remove_resource(&req(&h, &tag, &iid)).unwrap(),
        RemoveResult::Removed
    );
    let d = deletes(&h);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].starts_with("/images/sha256"), "{}", d[0]);
    assert!(d[0].contains(iid.trim_start_matches("sha256:")) || d[0].contains(&iid));
    assert!(d[0].ends_with("?force=false&noprune=true"), "{}", d[0]);
    assert!(!h.docker.with(|s| s.images.iter().any(|i| i.id == iid)));
    // idempotent: now it is simply gone
    assert_eq!(
        ops.remove_resource(&req(&h, &tag, &iid)).unwrap(),
        RemoveResult::AlreadyAbsent
    );
    assert_eq!(deletes(&h).len(), 1);
}

#[test]
fn a_conflict_at_delete_time_is_recorded_as_a_conflict_and_nothing_is_deleted_or_forced() {
    let (h, tag, iid) = world();
    let mut b = backend(&h);
    // a container appears after the pre-delete assessment and before Docker is asked
    let image = iid.clone();
    h.docker.fault(
        Some("DELETE"),
        "/images/",
        1,
        Fault::Race(std::sync::Arc::new(move |s: &mut St| {
            s.add_container("sneaky", vec![], false);
            s.containers.last_mut().unwrap().image = image.clone();
        })),
    );
    let r = b
        .retained_image_ops()
        .remove_resource(&req(&h, &tag, &iid))
        .unwrap();
    assert!(
        matches!(r, RemoveResult::Conflict(ConflictCause::DockerRefused, _)),
        "{r:?}"
    );
    assert!(
        h.docker.with(|s| s.images.iter().any(|i| i.id == iid)),
        "not deleted"
    );
    for d in deletes(&h) {
        assert!(
            d.contains("force=false") && !d.contains("force=true"),
            "{d}"
        );
    }
}

#[test]
fn the_classification_comes_from_the_status_code_not_from_docker_text() {
    // Same text, different statuses: only HTTP 409 is a conflict.
    for (status, conflict) in [
        (409u16, true),
        (500, false),
        (400, false),
        (403, false),
        (404, false),
    ] {
        let (h, tag, iid) = world();
        let mut b = backend(&h);
        h.docker.fault(
            Some("DELETE"),
            "/images/",
            1,
            Fault::Respond(
                status,
                "{\"message\":\"conflict: unable to delete: image is being used\"}".into(),
            ),
        );
        let r = b.retained_image_ops().remove_resource(&req(&h, &tag, &iid));
        match (status, r) {
            (404, Ok(RemoveResult::AlreadyAbsent)) => {}
            (409, Ok(RemoveResult::Conflict(ConflictCause::DockerRefused, _))) => assert!(conflict),
            // every other status is an error, never a typed conflict, whatever the text says
            (_, Err(_)) => assert!(!conflict, "{status}"),
            (s, other) => panic!("{s}: {other:?}"),
        }
    }
    // and a 409 whose text says nothing about conflicts is still a conflict
    let (h, tag, iid) = world();
    let mut b = backend(&h);
    h.docker.fault(
        Some("DELETE"),
        "/images/",
        1,
        Fault::Respond(409, "{\"message\":\"anything\"}".into()),
    );
    let r = b
        .retained_image_ops()
        .remove_resource(&req(&h, &tag, &iid))
        .unwrap();
    assert!(matches!(
        r,
        RemoveResult::Conflict(ConflictCause::DockerRefused, _)
    ));
}

#[test]
fn ambiguous_failures_are_errors_never_a_typed_conflict() {
    for (name, f) in [
        ("disconnect", Fault::Disconnect),
        ("reply lost", Fault::ApplyThenDrop),
        ("garbage", Fault::Raw(b"nonsense".to_vec())),
        (
            "truncated",
            Fault::Truncate {
                claimed: 100,
                body: "{".into(),
            },
        ),
    ] {
        let (h, tag, iid) = world();
        let mut b = backend(&h);
        h.docker.fault(Some("DELETE"), "/images/", 1, f);
        let r = b.retained_image_ops().remove_resource(&req(&h, &tag, &iid));
        assert!(r.is_err(), "{name}: {r:?}");
    }
}

#[test]
fn only_an_image_is_ever_removed_here_and_an_ownership_change_is_refused_not_deleted() {
    let (h, tag, iid) = world();
    let mut b = backend(&h);
    let mut r = req(&h, &tag, &iid);
    r.kind = ResourceKind::Container;
    assert!(matches!(
        b.retained_image_ops().remove_resource(&r),
        Ok(RemoveResult::Refused(_))
    ));
    h.docker
        .with(|s| s.images.last_mut().unwrap().labels.clear());
    assert!(matches!(
        b.retained_image_ops().remove_resource(&req(&h, &tag, &iid)),
        Ok(RemoveResult::Refused(_))
    ));
    assert!(deletes(&h).is_empty());
}

// ---------------------------------------------------------------- crash at every journal write

fn ledger_with_image(h: &Harness, tag: &str, iid: &str) -> Vec<LedgerRecord> {
    vec![
        LedgerRecord::Intent {
            key: ResourceKey::new(ResourceKind::Image, tag),
            app: h.dep.application_id.clone(),
            deployment: h.dep.id.clone(),
            disposition: Disposition::Retain,
            at_ms: 1,
        },
        LedgerRecord::Receipt {
            key: ResourceKey::new(ResourceKind::Image, tag),
            deployment: h.dep.id.clone(),
            id: iid.to_string(),
            at_ms: 2,
        },
    ]
}

fn remove_action(h: &Harness, tag: &str, iid: &str) -> Action {
    Action::Remove {
        entry: (h.dep.id.clone(), ResourceKey::new(ResourceKind::Image, tag)),
        observed: Observed {
            kind: ResourceKind::Image,
            name: tag.to_string(),
            id: iid.to_string(),
            labels: vec![],
        },
        adopt: false,
    }
}

#[test]
fn a_crash_at_every_cleanup_write_is_retried_to_a_released_image_without_touching_anything_else() {
    // a clean run first: how many journal writes does a cleanup make?
    let total = {
        let (h, tag, iid) = world();
        let mut ledger = Ledger::replay(&ledger_with_image(&h, &tag, &iid)).unwrap();
        let mut j = StampedJournal::new();
        let mut b = backend(&h);
        let mut ops = b.retained_image_ops();
        execute_actions(
            &[remove_action(&h, &tag, &iid)],
            &mut ledger,
            &mut j,
            &mut ops,
            &harpoon_deploy::time::SystemClock,
        )
        .unwrap();
        j.inner.writes()
    };
    assert_eq!(total, 2, "one attempt, one result");

    // Also: the reply to the delete can be lost, so the image is gone when the retry looks.
    // The reply can be lost (the image is gone when the retry looks), or Docker can refuse with 409
    // (a typed conflict that a crash must not turn into anything non-blocking).
    for (variant, lose_reply, conflict_409) in [
        ("clean", false, false),
        ("lost reply", true, false),
        ("409", false, true),
    ] {
        for crash_at in 0..total {
            let ctx = format!("crash @{crash_at}, {variant}");
            let (h, tag, iid) = world();
            let by_c = add_container_from(&h, "bystander", &fake_iid(5), true);
            let by_i = h.docker.with(|s| s.add_image("bystander:1", vec![]));
            let base = ledger_with_image(&h, &tag, &iid);
            let mut ledger = Ledger::replay(&base).unwrap();
            if lose_reply {
                h.docker
                    .fault(Some("DELETE"), "/images/", 1, Fault::ApplyThenDrop);
            }
            if conflict_409 {
                h.docker.fault(
                    Some("DELETE"),
                    "/images/",
                    1,
                    Fault::Respond(409, "{\"message\":\"x\"}".into()),
                );
            }
            let mut first = StampedJournal::crash_at(crash_at);
            let mut b = backend(&h);
            let res = {
                let mut ops = b.retained_image_ops();
                execute_actions(
                    &[remove_action(&h, &tag, &iid)],
                    &mut ledger,
                    &mut first,
                    &mut ops,
                    &harpoon_deploy::time::SystemClock,
                )
            };
            // either the journal crashed (a crash point) or the failed delete was recorded
            if (lose_reply || conflict_409) && crash_at == 1 {
                assert!(
                    matches!(res, Err(EngineError::Journal(JournalError::Crash))),
                    "{ctx}: {res:?}"
                );
            }
            let mut records = base.clone();
            records.extend(first.inner.ledger_records.clone());

            // restart: replay what is durable, then repeat the same operation
            h.docker.with(|s| s.faults.clear());
            let mut ledger = Ledger::replay(&records).expect(&ctx);
            // Whatever was durable at the crash, an attempt without its result blocks; and a typed
            // conflict is recorded only when its result was.
            let e0 = ledger.entries()[0].clone();
            if crash_at < total {
                assert!(
                    e0.cleanup_outstanding || e0.cleanup_attempts == 0,
                    "{ctx}: {e0:?}"
                );
                assert!(
                    !ledger.deferred_image_conflict(&e0, Some(EndState::NotSucceeded)),
                    "{ctx}: a crashed cleanup is never deferred"
                );
            }
            let still_open = !ledger.entries()[0].is_released();
            assert!(
                still_open,
                "{ctx}: nothing releases the image before a result is recorded as removed"
            );
            let mut second = StampedJournal::new();
            let mut b2 = backend(&h);
            {
                let mut ops = b2.retained_image_ops();
                execute_actions(
                    &[remove_action(&h, &tag, &iid)],
                    &mut ledger,
                    &mut second,
                    &mut ops,
                    &harpoon_deploy::time::SystemClock,
                )
                .expect(&ctx);
            }
            records.extend(second.inner.ledger_records.clone());
            let e = &ledger.entries()[0];
            assert!(
                matches!(
                    e.state,
                    EntryState::Released(Released::Removed)
                        | EntryState::Released(Released::AlreadyAbsent)
                ),
                "{ctx}: {:?}",
                e.state
            );
            assert!(
                !h.docker.with(|s| s.images.iter().any(|i| i.id == iid)),
                "{ctx}: image still there"
            );
            // the journal reproduces the accounting, and nothing else moved
            assert_eq!(Ledger::replay(&records).expect(&ctx), ledger, "{ctx}");
            assert!(
                h.docker
                    .with(|s| s.containers.iter().any(|c| c.id == by_c && c.running)),
                "{ctx}"
            );
            assert!(
                h.docker.with(|s| s.images.iter().any(|i| i.id == by_i)),
                "{ctx}"
            );
            for d in deletes(&h) {
                assert!(
                    d.contains("force=false") && d.starts_with("/images/sha256"),
                    "{ctx}: {d}"
                );
            }
            // a third run has nothing left to do
            assert!(ledger.entries()[0].is_released());
        }
    }
}

#[test]
fn a_recorded_conflict_is_not_a_release_and_a_later_clean_attempt_releases_it() {
    let (h, tag, iid) = world();
    let base = ledger_with_image(&h, &tag, &iid);
    let mut ledger = Ledger::replay(&base).unwrap();
    let mut j = StampedJournal::new();
    let image = iid.clone();
    h.docker.fault(
        Some("DELETE"),
        "/images/",
        1,
        Fault::Race(std::sync::Arc::new(move |s: &mut St| {
            s.add_container("sneaky", vec![], false);
            s.containers.last_mut().unwrap().image = image.clone();
        })),
    );
    let mut b = backend(&h);
    {
        let mut ops = b.retained_image_ops();
        execute_actions(
            &[remove_action(&h, &tag, &iid)],
            &mut ledger,
            &mut j,
            &mut ops,
            &harpoon_deploy::time::SystemClock,
        )
        .unwrap();
    }
    let e = ledger.entries()[0].clone();
    assert!(matches!(
        &e.state,
        EntryState::CleanupConflict(ConflictCause::DockerRefused, _)
    ));
    assert!(!e.is_released());
    assert!(ledger.deferred_image_conflict(&e, Some(EndState::NotSucceeded)));
    // the cause goes away; the retry (a real second attempt) succeeds
    h.docker.with(|s| s.containers.clear());
    let mut b = backend(&h);
    {
        let mut ops = b.retained_image_ops();
        execute_actions(
            &[remove_action(&h, &tag, &iid)],
            &mut ledger,
            &mut j,
            &mut ops,
            &harpoon_deploy::time::SystemClock,
        )
        .unwrap();
    }
    assert!(ledger.entries()[0].is_released());
    assert_eq!(ledger.entries()[0].cleanup_attempts, 2);
}

#[test]
fn nothing_in_the_new_backend_code_prunes_forces_or_matches_by_name() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/backend.rs"),
    )
    .unwrap();
    let start = src.find("B8.2: retained-image cleanup").expect("section");
    let section = &src[start..];
    for bad in [
        "prune",
        "force=true",
        "/images/create",
        "starts_with(\"harpoon",
        "contains(\"harpoon",
    ] {
        let hits: Vec<_> = section
            .lines()
            .filter(|l| l.contains(bad) && !l.trim_start().starts_with("//"))
            .collect();
        assert!(hits.is_empty(), "{bad}: {hits:?}");
    }
    let _: Option<&dyn HttpTransport> = None;
}

// ---------------------------------------------------------------- tag and digest lists are strict

/// Docker's inspect answer for the image, with the given raw `RepoTags` and `RepoDigests` JSON.
fn inspect_body(
    iid: &str,
    labels: &[(String, String)],
    tags: Option<&str>,
    digests: Option<&str>,
) -> String {
    let l = labels
        .iter()
        .map(|(k, v)| format!("\"{k}\":\"{v}\""))
        .collect::<Vec<_>>()
        .join(",");
    let mut f = vec![format!("\"Id\":\"{iid}\"")];
    if let Some(t) = tags {
        f.push(format!("\"RepoTags\":{t}"));
    }
    if let Some(d) = digests {
        f.push(format!("\"RepoDigests\":{d}"));
    }
    f.push(format!("\"Config\":{{\"Labels\":{{{l}}}}}"));
    format!("{{{}}}", f.join(","))
}

#[test]
fn a_malformed_tag_or_digest_list_is_an_error_never_an_empty_set() {
    let (h0, _, _) = world();
    let l = labels(&h0.dep.application_id, &h0.dep.id);
    let bad_lists: [(&str, Option<&str>); 7] = [
        ("RepoTags is a string", Some("\"foo:latest\"")),
        ("RepoTags is an object", Some("{\"a\":1}")),
        ("RepoTags holds a number", Some("[\"ok:1\",7]")),
        ("RepoTags holds null", Some("[null]")),
        ("RepoTags holds an array", Some("[[\"x\"]]")),
        ("RepoTags is missing", None),
        ("RepoTags is a boolean", Some("true")),
    ];
    for (what, tags) in bad_lists {
        let (h, tag, iid) = world();
        let b = backend(&h);
        h.docker.fault(
            Some("GET"),
            "/images/sha256:",
            50,
            Fault::Respond(200, inspect_body(&iid, &l, tags, Some("[]"))),
        );
        let r = assess(&h, &b, &tag, &iid);
        assert!(r.is_err(), "{what}: {r:?}");
        assert!(
            r.unwrap_err().message.contains("RepoTags"),
            "{what}: names the field"
        );
    }
    for (what, digests) in [
        ("RepoDigests is a string", Some("\"x@sha256:1\"")),
        ("RepoDigests holds a number", Some("[1]")),
        ("RepoDigests is missing", None),
    ] {
        let (h, tag, iid) = world();
        let b = backend(&h);
        h.docker.fault(
            Some("GET"),
            "/images/sha256:",
            50,
            Fault::Respond(200, inspect_body(&iid, &l, Some("[]"), digests)),
        );
        let r = assess(&h, &b, &tag, &iid);
        assert!(r.is_err(), "{what}: {r:?}");
    }
    // Docker's own spelling of "none" (null) and the well-formed lists are accepted
    for (tags, digests, want) in [
        ("null", "null", true),
        ("[]", "[]", true),
        ("[\"{TAG}\"]", "[]", true),
        ("[\"{TAG}\",\"foo:1\"]", "[]", false),
        ("[]", "[\"r/x@sha256:1\"]", false),
    ] {
        let (h, tag, iid) = world();
        let b = backend(&h);
        let tags = tags.replace("{TAG}", &tag);
        h.docker.fault(
            Some("GET"),
            "/images/sha256:",
            50,
            Fault::Respond(200, inspect_body(&iid, &l, Some(&tags), Some(digests))),
        );
        let v = assess(&h, &b, &tag, &iid).unwrap();
        assert_eq!(v == ImageVerdict::Eligible, want, "{tags} {digests}: {v:?}");
    }
}

#[test]
fn a_malformed_tag_list_makes_the_deletion_an_error_and_sends_no_delete() {
    let (h0, _, _) = world();
    let l = labels(&h0.dep.application_id, &h0.dep.id);
    let (h, tag, iid) = world();
    let mut b = backend(&h);
    h.docker.fault(
        Some("GET"),
        "/images/sha256:",
        50,
        Fault::Respond(
            200,
            inspect_body(&iid, &l, Some("[\"ok:1\",5]"), Some("[]")),
        ),
    );
    let r = b.retained_image_ops().remove_resource(&req(&h, &tag, &iid));
    assert!(r.is_err(), "{r:?}");
    assert!(deletes(&h).is_empty());
}

#[test]
fn the_image_list_is_strict_too_so_a_malformed_tag_list_is_never_read_as_untagged() {
    for (what, tags) in [
        ("a string", "\"x\""),
        ("a number inside", "[\"a:1\",3]"),
        ("an object", "{}"),
    ] {
        let h = Harness::new();
        h.docker.fault(
            Some("GET"),
            "/images/json",
            50,
            Fault::Respond(
                200,
                format!("[{{\"Id\":\"sha256:aa\",\"RepoTags\":{tags},\"Labels\":null}}]"),
            ),
        );
        let api = harpoon_deploy_docker::api::DockerApi::new(
            h.docker.path(),
            std::time::Duration::from_secs(3),
        );
        assert!(
            api.list_images(std::time::Duration::from_secs(3)).is_err(),
            "{what}"
        );
    }
    // null (a dangling image) is an untagged image; a missing key is malformed
    let h = Harness::new();
    h.docker.fault(
        Some("GET"),
        "/images/json",
        1,
        Fault::Respond(
            200,
            "[{\"Id\":\"sha256:aa\",\"RepoTags\":null,\"Labels\":null}]".into(),
        ),
    );
    let api = harpoon_deploy_docker::api::DockerApi::new(
        h.docker.path(),
        std::time::Duration::from_secs(3),
    );
    let v = api.list_images(std::time::Duration::from_secs(3)).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].0, v[0].1, "an untagged image is named by its id");
    h.docker.fault(
        Some("GET"),
        "/images/json",
        1,
        Fault::Respond(200, "[{\"Id\":\"sha256:aa\",\"Labels\":null}]".into()),
    );
    assert!(api.list_images(std::time::Duration::from_secs(3)).is_err());
}
