//! Recovery hardening through the CLI: after a restoration the serving release and its real
//! container are identified from the records, `replace` and `teardown` operate on the right
//! containers, and a release without a pin is flagged before a cutover starts.

use super::*;
use harpoon_deploy::ids::ReleaseId;
use harpoon_deploy::state::{RecoveryOutcome, Step};

const APP: &str = "app_00000000000000000000000001";
const PREV: &str = "dep_00000000000000000000000010";
const FAILED: &str = "dep_00000000000000000000000012";

/// A failed deployment that replaced `replaces` and whose bounded restoration succeeded, leaving
/// the restored container recorded under the failed deployment.
fn restored_by(
    t: &T,
    app: &ApplicationId,
    failed: &DeploymentId,
    replaces: &DeploymentId,
    cid: &str,
) {
    let mut d = Deployment::new(failed.clone(), app.clone(), Trigger::Cli, 500);
    d.transition(DeploymentStatus::Queued, 501).unwrap();
    d.transition(DeploymentStatus::Running, 502).unwrap();
    d.start_step(Step::Cutover, 503).unwrap();
    d.replaces = Some(ReleaseId::from_deployment(replaces));
    d.begin_cutover(504).unwrap();
    d.fail_cutover(505).unwrap();
    d.start_step(Step::Recovery, 506).unwrap();
    d.start_recovery(60_000, 506).unwrap();
    d.finish_recovery(RecoveryOutcome::Restored, 900).unwrap();
    d.transition(DeploymentStatus::Failed, 901).unwrap();
    let name = harpoon_deploy::serving::restored_container_name(failed);
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&d).unwrap();
    j.record(&intent(
        app,
        failed,
        ResourceKind::Container,
        &name,
        Disposition::Retain,
    ))
    .unwrap();
    j.record(&receipt(failed, ResourceKind::Container, &name, cid))
        .unwrap();
}

#[test]
fn status_and_replace_use_the_restored_container_as_the_serving_one() {
    let t = T::new();
    super::b11::earlier_release_for_tests(&t, APP, PREV, true);
    let (app, prev, failed): (ApplicationId, DeploymentId, DeploymentId) = (
        APP.parse().unwrap(),
        PREV.parse().unwrap(),
        FAILED.parse().unwrap(),
    );
    restored_by(&t, &app, &failed, &prev, "cid-restored");

    let s = t.cli(&["status"]).stdout;
    let restored = format!("hp-{FAILED}-restored");
    assert!(
        s.contains(&format!("serving: application {APP} runs release rel_{} (deployment {PREV}) in container {restored} of deployment {FAILED} (restored by {FAILED})", &PREV[4..])),
        "{s}"
    );
    assert!(s.contains("recovery: restored"), "{s}");
    assert!(
        s.contains(&format!("automatic triggers paused for application {APP}")),
        "{s}"
    );
    assert!(!s.contains("the application may be down"), "{s}");

    let src = t.src();
    let mut a: Vec<&str> = vec!["replace", "--simulate", "--app", APP];
    a.extend(src.iter().map(String::as_str));
    let o = t.cli(&a);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains(&format!(
            "served by container {restored} of deployment {FAILED} (restored by {FAILED})"
        )),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains("WARNING"),
        "the release is pinned: {}",
        o.stdout
    );
}

#[test]
fn a_release_without_a_pin_is_flagged_in_the_plan_before_any_cutover() {
    let t = T::new();
    super::b11::earlier_release_for_tests(&t, APP, PREV, false);
    let src = t.src();
    let mut a: Vec<&str> = vec!["replace", "--simulate", "--app", APP];
    a.extend(src.iter().map(String::as_str));
    let o = t.cli(&a);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("WARNING: release")
            && o.stdout.contains("has no pinned configuration")
            && o.stdout.contains("cannot be restored"),
        "{}",
        o.stdout
    );
}

#[test]
fn teardown_of_a_restored_release_removes_both_of_its_containers_and_nothing_else() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    let failed = ids(43).1;
    // the old release's container was stopped by the cutover; a restoration made a new one
    let cid_restored = {
        let lbl = harpoon_deploy::backend::harpoon_labels(&d.app, &failed);
        let name = harpoon_deploy::serving::restored_container_name(&failed);
        t.h.docker.with(|s| {
            for c in s.containers.iter_mut().filter(|c| c.id == d.cid) {
                c.running = false;
            }
            let id = s.add_container(&name, lbl.clone(), true);
            s.containers.last_mut().unwrap().host_port = Some(38000);
            // the failed candidate: stopped, kept for inspection, and not part of the release
            s.add_container(&format!("hp-{failed}"), lbl, false);
            id
        })
    };
    restored_by(&t, &d.app, &failed, &d.dep, &cid_restored);
    let candidate = format!("hp-{failed}");
    let cand_id = t.h.docker.container(&candidate).unwrap().id;
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.record(&intent(
            &d.app,
            &failed,
            ResourceKind::Container,
            &candidate,
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
        j.record(&receipt(
            &failed,
            ResourceKind::Container,
            &candidate,
            &cand_id,
        ))
        .unwrap();
    }
    let bystander =
        t.h.docker
            .with(|s| s.add_container("bystander", vec![], true));

    // dry run names both containers and changes nothing
    let o = go(&t, &SHIPPING, &td_args(&t, &d, &[]));
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains(&harpoon_deploy::serving::restored_container_name(&failed)),
        "{}",
        o.stdout
    );
    assert!(mutating(&t).is_empty());

    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 0);
    assert!(o.stdout.contains("teardown: complete"), "{}", o.stdout);
    let m = mutating(&t);
    let touched = |id: &str| m.iter().any(|(_, p)| p.contains(id));
    assert!(
        touched(&cid_restored) && touched(&d.cid) && touched(&d.img),
        "{m:?}"
    );
    assert!(
        m.iter()
            .any(|(me, p)| me == "POST" && p.contains(&format!("{cid_restored}/stop"))),
        "the running restored container is stopped first: {m:?}"
    );
    assert!(
        !m.iter()
            .any(|(_, p)| p.contains(&format!("{}/stop", d.cid))),
        "the already stopped original is not stopped again: {m:?}"
    );
    assert!(
        !touched(&cand_id) && !touched(&bystander),
        "the failed candidate and bystanders are untouched: {m:?}"
    );
    assert!(m.iter().all(|(_, p)| !p.contains("force=true")));
    assert!(
        t.h.docker.container(&candidate).is_some() && t.h.docker.container("bystander").is_some()
    );
    assert!(t
        .h
        .docker
        .container(&harpoon_deploy::serving::restored_container_name(&failed))
        .is_none());

    // both containers and the release's lease are released in the journal
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    let state = |dep: &DeploymentId, kind, name: &str| {
        format!(
            "{:?}",
            r.ledger
                .find(dep, &ResourceKey::new(kind, name))
                .unwrap()
                .state
        )
    };
    assert!(state(
        &failed,
        ResourceKind::Container,
        &harpoon_deploy::serving::restored_container_name(&failed)
    )
    .starts_with("Released"));
    assert!(
        state(&d.dep, ResourceKind::Container, &format!("hp-{}", d.dep)).starts_with("Released")
    );
    assert!(state(&d.dep, ResourceKind::PortLease, "38000").starts_with("Released"));
    assert!(
        !state(&failed, ResourceKind::Container, &candidate).starts_with("Released"),
        "the candidate stays recorded"
    );
}

#[test]
fn reconcile_removes_the_failed_candidate_and_keeps_the_restored_serving_container() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    let failed = ids(43).1;
    let lbl = harpoon_deploy::backend::harpoon_labels(&d.app, &failed);
    let restored_name = harpoon_deploy::serving::restored_container_name(&failed);
    let candidate = format!("hp-{failed}");
    let (cid_restored, cand_id) = t.h.docker.with(|s| {
        for c in s.containers.iter_mut().filter(|c| c.id == d.cid) {
            c.running = false;
        }
        let r = s.add_container(&restored_name, lbl.clone(), true);
        let c = s.add_container(&candidate, lbl, false);
        (r, c)
    });
    restored_by(&t, &d.app, &failed, &d.dep, &cid_restored);
    {
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.record(&intent(
            &d.app,
            &failed,
            ResourceKind::Container,
            &candidate,
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
        j.record(&receipt(
            &failed,
            ResourceKind::Container,
            &candidate,
            &cand_id,
        ))
        .unwrap();
    }
    let bystander =
        t.h.docker
            .with(|s| s.add_container("bystander", vec![], true));
    let (sock, res) = (t.socket(), t.resources());
    let o = t.cli(&[
        "reconcile",
        "--docker-socket",
        &sock,
        "--resources-dir",
        &res,
        "--apply",
        &format!("--{AUTH_FLAG}={AUTH_PHRASE}"),
    ]);
    assert_exit(&o, 0);
    let m = mutating(&t);
    assert!(
        m.iter()
            .any(|(me, p)| me == "DELETE" && p.contains(&cand_id)),
        "the failed candidate is removed: {m:?}"
    );
    assert!(
        !m.iter().any(|(_, p)| p.contains(&cid_restored)
            || p.contains(&d.cid)
            || p.contains(&bystander)),
        "the restored serving container, the original and bystanders are untouched: {m:?}"
    );
    assert!(t.h.docker.container(&restored_name).unwrap().running);
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains(&format!(
            "in container {restored_name} of deployment {failed}"
        )),
        "{s}"
    );
}
