//! Poll event identity across removals. Event de-duplication is per (application, delivery id); the id a poll uses
//! carries the registration's incarnation (how many times it was removed) so a registration claimed again can queue a
//! commit that an earlier incarnation already handled, while nothing recorded earlier is rewritten or replayed.

use harpoon_deploy::control::{self, check_delivery, Choice, ControlState, ExecOutcome, ExecRec, Incoming, Ingress};
use harpoon_deploy::engine::MemoryJournal;
use harpoon_deploy::git::*;
use harpoon_deploy::ids::ApplicationId;

const APP: &str = "app_00000000000000000000000001";
const OTHER: &str = "app_00000000000000000000000002";
const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn app(s: &str) -> ApplicationId {
    s.parse().unwrap()
}

fn registration(name: &str, a: &str, at_ms: u64) -> RepoRec {
    RepoRec {
        name: name.into(),
        url: "https://example.com/acme/shop.git".into(),
        username: None,
        cred: CredKind::None,
        known_host: None,
        webhook: None,
        app: app(a),
        project: None,
        environment: None,
        branch: "main".into(),
        filters: vec![],
        container_port: 8080,
        dockerfile: None,
        context: None,
        target: None,
        health_path: None,
        at_ms,
    }
}

fn register(st: &mut GitState, r: &RepoRec) {
    st.apply(&GitRecord::Repo(Box::new(r.clone()))).unwrap();
}

fn remove(st: &mut GitState, r: &RepoRec, at_ms: u64) {
    st.apply(&GitRecord::Removed(RemovedRec {
        name: r.name.clone(),
        app: r.app.clone(),
        registration: r.at_ms,
        url: r.url.clone(),
        cred: r.cred,
        at_ms,
    }))
    .unwrap();
}

#[test]
fn a_never_removed_registration_keeps_the_original_poll_ids() {
    let mut st = GitState::default();
    let r = registration("shop", APP, 5);
    register(&mut st, &r);
    assert_eq!(st.incarnation(&app(APP)), 0);
    assert_eq!(
        poll_delivery("shop", st.incarnation(&app(APP)), SHA_A),
        format!("poll:shop:{SHA_A}")
    );
    // edits are new versions of the same registration, not new incarnations
    for at in [9, 12, 40] {
        register(
            &mut st,
            &RepoRec {
                branch: format!("b{at}"),
                at_ms: at,
                ..r.clone()
            },
        );
        assert_eq!(st.incarnation(&app(APP)), 0, "an edit at {at} changed the incarnation");
    }
}

#[test]
fn every_removal_gives_the_next_incarnation_and_the_whole_history_is_counted() {
    let mut st = GitState::default();
    let mut r = registration("shop", APP, 5);
    register(&mut st, &r);
    let mut ids = vec![poll_delivery("shop", 0, SHA_A)];
    for n in 1..=3usize {
        remove(&mut st, &r, 100 * n as u64);
        assert_eq!(st.incarnation(&app(APP)), n, "after removal {n}");
        // claimed again by the original name, with the same application
        r = registration("shop", APP, 100 * n as u64 + 10);
        register(&mut st, &r);
        assert_eq!(st.incarnation(&app(APP)), n);
        ids.push(poll_delivery("shop", st.incarnation(&app(APP)), SHA_A));
        // an edit in this incarnation changes nothing
        r = RepoRec {
            branch: "dev".into(),
            at_ms: 100 * n as u64 + 20,
            ..r.clone()
        };
        register(&mut st, &r);
        assert_eq!(st.incarnation(&app(APP)), n);
    }
    assert_eq!(
        ids,
        [
            format!("poll:shop:{SHA_A}"),
            format!("poll:shop:r1:{SHA_A}"),
            format!("poll:shop:r2:{SHA_A}"),
            format!("poll:shop:r3:{SHA_A}"),
        ]
    );
    let distinct: std::collections::BTreeSet<_> = ids.iter().collect();
    assert_eq!(distinct.len(), ids.len(), "each incarnation is its own namespace");
    for id in &ids {
        assert!(check_delivery(id).is_ok(), "{id} must be a valid delivery id");
    }
    assert_eq!(
        st.retired.len(),
        3,
        "the count is the full removal history, not the latest entry"
    );
}

#[test]
fn a_fresh_application_for_a_reused_name_starts_at_incarnation_zero() {
    let mut st = GitState::default();
    let first = registration("shop", APP, 5);
    register(&mut st, &first);
    remove(&mut st, &first, 50);
    register(&mut st, &registration("shop", OTHER, 60));
    assert_eq!(
        st.incarnation(&app(OTHER)),
        0,
        "a fresh application inherits no incarnation"
    );
    assert_eq!(st.incarnation(&app(APP)), 1, "the removed application keeps its own");
}

/// Queue a poll-found commit for `application` as the poll does.
fn queue(
    ctl: &mut ControlState,
    j: &mut MemoryJournal,
    application: &ApplicationId,
    id: &str,
    sha: &str,
    at: u64,
) -> Ingress {
    control::queue_event(
        j,
        ctl,
        Incoming {
            app: application,
            delivery_id: id,
            source: "poll",
            revision: sha,
            received_ms: at,
        },
    )
    .unwrap()
}

#[test]
fn a_reclaimed_registration_can_queue_a_commit_an_earlier_incarnation_already_handled() {
    // Each earlier outcome the same commit can have been left in, under the legacy id.
    for outcome in ["held", "dismissed", "failed", "executed"] {
        let a = app(APP);
        let mut ctl = ControlState::default();
        let mut j = MemoryJournal::default();
        let legacy = poll_delivery("shop", 0, SHA_A);
        assert_eq!(queue(&mut ctl, &mut j, &a, &legacy, SHA_A, 10), Ingress::Held);
        match outcome {
            "held" => {}
            "dismissed" => {
                control::resume(&mut j, &[], &mut ctl, &a, Choice::Dismiss, 20).unwrap();
            }
            other => {
                let (kind, dep) = if other == "failed" {
                    (ExecOutcome::Failed, None)
                } else {
                    (
                        ExecOutcome::Deployed,
                        Some("dep_00000000000000000000000001".parse().unwrap()),
                    )
                };
                control::record_exec(
                    &mut j,
                    &mut ctl,
                    ExecRec {
                        app: a.clone(),
                        delivery: legacy.clone(),
                        outcome: kind,
                        deployment: dep,
                        reason: if other == "failed" {
                            "the checkout failed".into()
                        } else {
                            String::new()
                        },
                        superseded: vec![],
                        at_ms: 30,
                    },
                )
                .unwrap();
            }
        }
        let status_before = ctl.event(&a, &legacy).unwrap().status;
        let events_before = ctl.events.clone();

        // the hazard: under the legacy id the new observation is silently dropped as a duplicate
        assert_eq!(
            queue(&mut ctl, &mut j, &a, &legacy, SHA_A, 99),
            Ingress::Duplicate,
            "{outcome}"
        );
        assert_eq!(ctl.events, events_before);

        // under the next incarnation it is a new event, and the earlier one is untouched
        let r1 = poll_delivery("shop", 1, SHA_A);
        assert_eq!(queue(&mut ctl, &mut j, &a, &r1, SHA_A, 100), Ingress::Held, "{outcome}");
        assert_eq!(
            ctl.event(&a, &legacy).unwrap().status,
            status_before,
            "{outcome}: the earlier event was rewritten"
        );
        assert_eq!(ctl.events.len(), events_before.len() + 1);
        assert_eq!(
            &ctl.events[..events_before.len()],
            &events_before[..],
            "{outcome}: history changed"
        );
        // and queuing the same commit again in this incarnation is still de-duplicated
        assert_eq!(queue(&mut ctl, &mut j, &a, &r1, SHA_A, 101), Ingress::Duplicate);
        // a different commit is of course its own event
        assert_eq!(
            queue(&mut ctl, &mut j, &a, &poll_delivery("shop", 1, SHA_B), SHA_B, 102),
            Ingress::Held
        );
    }
}

#[test]
fn nothing_handled_before_a_removal_is_queued_again_by_the_new_ids_alone() {
    // Switching namespace never creates an event by itself: only a poll that sees a commit move does.
    let a = app(APP);
    let mut ctl = ControlState::default();
    let mut j = MemoryJournal::default();
    for (i, sha) in [SHA_A, SHA_B].into_iter().enumerate() {
        let id = poll_delivery("shop", 0, sha);
        assert_eq!(queue(&mut ctl, &mut j, &a, &id, sha, 10 + i as u64), Ingress::Held);
    }
    control::resume(&mut j, &[], &mut ctl, &a, Choice::Dismiss, 40).unwrap();
    let before = ctl.clone();
    // the new incarnation's namespace has no event until something is queued in it
    assert!(ctl.event(&a, &poll_delivery("shop", 1, SHA_A)).is_none());
    assert!(ctl.event(&a, &poll_delivery("shop", 1, SHA_B)).is_none());
    assert_eq!(ctl, before);
    assert!(ctl.held(&a).is_empty());
}
