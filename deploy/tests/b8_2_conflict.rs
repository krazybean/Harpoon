//! B8.2: the one narrow exception to "an unresolved cleanup blocks the runtime": a retained image
//! of a deployment that did not succeed, whose deletion was refused as a conflict, where everything
//! else of that deployment is released. Pure: no files, no Docker.

use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator};
use harpoon_deploy::ledger::{
    codec, image_conflict_message, CleanupOutcome, Disposition, EndState, Ledger, LedgerRecord, ResourceKey,
    ResourceKind, IMAGE_CONFLICT_MARKER,
};
use harpoon_deploy::time::{ManualClock, SeqEntropy};

fn ids(seed: u8) -> (ApplicationId, DeploymentId) {
    let mut g = IdGenerator::new(ManualClock::new(1_700_000_000_000), SeqEntropy::new(seed));
    (g.application().unwrap(), g.deployment().unwrap())
}

fn rec_intent(app: &ApplicationId, dep: &DeploymentId, k: ResourceKind, n: &str, d: Disposition) -> LedgerRecord {
    LedgerRecord::Intent {
        key: ResourceKey::new(k, n),
        app: app.clone(),
        deployment: dep.clone(),
        disposition: d,
        at_ms: 1,
    }
}
fn rec_receipt(dep: &DeploymentId, k: ResourceKind, n: &str, id: &str) -> LedgerRecord {
    LedgerRecord::Receipt {
        key: ResourceKey::new(k, n),
        deployment: dep.clone(),
        id: id.into(),
        at_ms: 2,
    }
}
fn rec_attempt(dep: &DeploymentId, k: ResourceKind, n: &str) -> LedgerRecord {
    LedgerRecord::CleanupAttempt {
        key: ResourceKey::new(k, n),
        deployment: dep.clone(),
        at_ms: 3,
    }
}
fn rec_result(dep: &DeploymentId, k: ResourceKind, n: &str, o: CleanupOutcome) -> LedgerRecord {
    LedgerRecord::CleanupResult {
        key: ResourceKey::new(k, n),
        deployment: dep.clone(),
        outcome: o,
        at_ms: 4,
    }
}

const IMG: &str = "harpoon/app:dep-1";
const IID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A retained image whose cleanup ended with `outcome`.
fn with_image(outcome: Option<CleanupOutcome>) -> (Vec<LedgerRecord>, DeploymentId) {
    let (app, dep) = ids(1);
    let mut r = vec![
        rec_intent(&app, &dep, ResourceKind::Image, IMG, Disposition::Retain),
        rec_receipt(&dep, ResourceKind::Image, IMG, IID),
    ];
    if let Some(o) = outcome {
        r.push(rec_attempt(&dep, ResourceKind::Image, IMG));
        r.push(rec_result(&dep, ResourceKind::Image, IMG, o));
    }
    (r, dep)
}

fn verdict(records: &[LedgerRecord], dep: &DeploymentId, end: Option<EndState>) -> bool {
    let l = Ledger::replay(records).unwrap();
    let e = l
        .entries()
        .iter()
        .find(|e| &e.deployment == dep && e.key.kind == ResourceKind::Image)
        .unwrap();
    l.deferred_image_conflict(e, end)
}

fn conflict() -> CleanupOutcome {
    CleanupOutcome::Failed(image_conflict_message("Docker refused the deletion (HTTP 409)"))
}

#[test]
fn an_eligible_conflict_is_deferred_and_the_image_is_not_released() {
    let (r, dep) = with_image(Some(conflict()));
    assert!(verdict(&r, &dep, Some(EndState::NotSucceeded)));
    let l = Ledger::replay(&r).unwrap();
    assert!(
        !l.entries()[0].is_released(),
        "a refused deletion never releases the entry"
    );
}

#[test]
fn the_classification_survives_the_wire_codec_and_a_replay() {
    let (r, dep) = with_image(Some(conflict()));
    let round: Vec<LedgerRecord> = r.iter().map(|x| codec::decode(&codec::encode(x)).unwrap()).collect();
    assert_eq!(round, r);
    assert!(verdict(&round, &dep, Some(EndState::NotSucceeded)));
}

#[test]
fn only_a_conflict_marker_qualifies_every_other_outcome_keeps_blocking() {
    for (name, o) in [
        (
            "plain failure",
            CleanupOutcome::Failed("removing image: timed out".into()),
        ),
        ("a 500", CleanupOutcome::Failed("Docker answered 500".into())),
        (
            "marker not at the start",
            CleanupOutcome::Failed(format!("x {IMAGE_CONFLICT_MARKER}y")),
        ),
        (
            "refused is F1 and terminal",
            CleanupOutcome::Refused(image_conflict_message("not ours")),
        ),
    ] {
        let (r, dep) = with_image(Some(o));
        assert!(!verdict(&r, &dep, Some(EndState::NotSucceeded)), "{name}");
    }
    // never attempted, or released
    let (r, dep) = with_image(None);
    assert!(!verdict(&r, &dep, Some(EndState::NotSucceeded)));
    let (r, dep) = with_image(Some(CleanupOutcome::Removed));
    assert!(!verdict(&r, &dep, Some(EndState::NotSucceeded)));
}

#[test]
fn the_deployment_must_be_known_to_have_ended_without_succeeding() {
    let (r, dep) = with_image(Some(conflict()));
    assert!(!verdict(&r, &dep, None), "unfinished or unknown deployment");
    assert!(!verdict(&r, &dep, Some(EndState::Succeeded)), "succeeded deployment");
}

#[test]
fn a_new_attempt_with_no_result_is_ambiguous_and_blocks_again() {
    let (mut r, dep) = with_image(Some(conflict()));
    r.push(rec_attempt(&dep, ResourceKind::Image, IMG));
    assert!(!verdict(&r, &dep, Some(EndState::NotSucceeded)));
    // the retry's own result settles it again, one way or the other
    r.push(rec_result(&dep, ResourceKind::Image, IMG, conflict()));
    assert!(verdict(&r, &dep, Some(EndState::NotSucceeded)));
    r.push(rec_attempt(&dep, ResourceKind::Image, IMG));
    r.push(rec_result(
        &dep,
        ResourceKind::Image,
        IMG,
        CleanupOutcome::Failed("disconnected".into()),
    ));
    assert!(
        !verdict(&r, &dep, Some(EndState::NotSucceeded)),
        "an ambiguous failure replaces the conflict"
    );
}

#[test]
fn anything_else_of_the_deployment_that_is_not_released_keeps_it_blocking() {
    for (kind, name, state) in [
        (ResourceKind::Container, "hp-1", "created, not released"),
        (ResourceKind::PortLease, "38000", "a lease still held"),
    ] {
        let (mut r, dep) = with_image(Some(conflict()));
        let app = ids(1).0;
        r.push(rec_intent(&app, &dep, kind, name, Disposition::RemoveOnFailure));
        r.push(rec_receipt(&dep, kind, name, "id-1"));
        assert!(!verdict(&r, &dep, Some(EndState::NotSucceeded)), "{state}");
        // released: eligible again
        r.push(rec_attempt(&dep, kind, name));
        r.push(rec_result(&dep, kind, name, CleanupOutcome::Removed));
        assert!(verdict(&r, &dep, Some(EndState::NotSucceeded)), "{state} then released");
    }
    // an unreceipted intent, a refused sibling and a failed sibling also block
    let (app, _) = ids(1);
    let mut r = with_image(Some(conflict())).0;
    let dep = r[0].deployment().clone();
    r.push(rec_intent(
        &app,
        &dep,
        ResourceKind::Container,
        "hp-x",
        Disposition::RemoveOnFailure,
    ));
    assert!(
        !verdict(&r, &dep, Some(EndState::NotSucceeded)),
        "intent with no receipt"
    );
    r.push(rec_receipt(&dep, ResourceKind::Container, "hp-x", "c1"));
    r.push(rec_attempt(&dep, ResourceKind::Container, "hp-x"));
    r.push(rec_result(
        &dep,
        ResourceKind::Container,
        "hp-x",
        CleanupOutcome::Refused("not ours".into()),
    ));
    assert!(!verdict(&r, &dep, Some(EndState::NotSucceeded)), "refused sibling");
}

#[test]
fn only_a_retained_image_can_be_deferred_a_forged_marker_elsewhere_changes_nothing() {
    let (app, dep) = ids(1);
    let l = |kind, disp| {
        let r = vec![
            rec_intent(&app, &dep, kind, "n", disp),
            rec_receipt(&dep, kind, "n", "id-1"),
            rec_attempt(&dep, kind, "n"),
            rec_result(&dep, kind, "n", conflict()),
        ];
        let l = Ledger::replay(&r).unwrap();
        let e = l.entries()[0].clone();
        l.deferred_image_conflict(&e, Some(EndState::NotSucceeded))
    };
    assert!(l(ResourceKind::Image, Disposition::Retain));
    assert!(!l(ResourceKind::Container, Disposition::Retain));
    assert!(!l(ResourceKind::PortLease, Disposition::Retain));
    assert!(
        !l(ResourceKind::Image, Disposition::RemoveOnFailure),
        "only a retained image"
    );
    assert!(!l(ResourceKind::Image, Disposition::RemoveAlways));
}

#[test]
fn an_image_without_a_receipt_is_never_deferred() {
    let (app, dep) = ids(1);
    // the codec/ledger do not allow a result before a receipt for cleanup of an unknown identity
    // to be deferred: the entry has no id.
    let r = vec![
        rec_intent(&app, &dep, ResourceKind::Image, IMG, Disposition::Retain),
        rec_attempt(&dep, ResourceKind::Image, IMG),
        rec_result(&dep, ResourceKind::Image, IMG, conflict()),
    ];
    assert!(!verdict(&r, &dep, Some(EndState::NotSucceeded)));
}
