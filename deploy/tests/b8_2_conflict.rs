//! B8.2: the one narrow exception to "an unresolved cleanup blocks the runtime": a retained image
//! of a deployment that did not succeed, whose deletion ended in the **typed** conflict outcome, where
//! everything else of that deployment is released. Pure: no files, no Docker.

use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator};
use harpoon_deploy::journal_format::{encode_header, encode_ledger, recover, replay, FormatError};
use harpoon_deploy::ledger::{
    codec, CleanupOutcome, ConflictCause, Disposition, EndState, EntryState, Ledger, LedgerError, LedgerRecord,
    ResourceKey, ResourceKind,
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
    CleanupOutcome::Conflict(ConflictCause::DockerRefused, "Docker refused (HTTP 409)".into())
}

const NS: Option<EndState> = Some(EndState::NotSucceeded);

// ---------------------------------------------------------------- the typed outcome

#[test]
fn a_recorded_typed_conflict_is_deferred_and_the_image_is_not_released() {
    for cause in [ConflictCause::DockerRefused, ConflictCause::NotExclusive] {
        let (r, dep) = with_image(Some(CleanupOutcome::Conflict(cause, "why".into())));
        assert!(verdict(&r, &dep, NS), "{cause:?}");
        let l = Ledger::replay(&r).unwrap();
        assert!(
            !l.entries()[0].is_released(),
            "a refused deletion never releases the entry"
        );
        assert!(
            l.entries()[0].needs_attention(),
            "it blocks unless the narrow rule says otherwise"
        );
        assert!(matches!(l.entries()[0].state, EntryState::CleanupConflict(c, _) if c == cause));
    }
}

#[test]
fn a_failed_outcome_never_becomes_a_conflict_whatever_its_text_says() {
    for text in [
        "image-conflict: Docker refused to delete the image (HTTP 409)",
        "image-conflict: ",
        "conflict-docker-refused",
        "conflict (docker-refused): x",
        "conflict-not-exclusive\tx",
        "CONFLICT: 409",
        "Docker answered 409 Conflict: image is being used by stopped container",
    ] {
        let (r, dep) = with_image(Some(CleanupOutcome::Failed(text.into())));
        assert!(!verdict(&r, &dep, NS), "{text:?}");
        let l = Ledger::replay(&r).unwrap();
        assert!(matches!(l.entries()[0].state, EntryState::CleanupFailed(_)), "{text:?}");
        // and the same through the wire codec
        let round: Vec<LedgerRecord> = r.iter().map(|x| codec::decode(&codec::encode(x)).unwrap()).collect();
        assert!(!verdict(&round, &dep, NS), "{text:?} after a round trip");
    }
    // a refused (F1) outcome with the same words is the terminal refusal, still blocking
    let (r, dep) = with_image(Some(CleanupOutcome::Refused("image-conflict: not ours".into())));
    assert!(!verdict(&r, &dep, NS));
}

#[test]
fn only_the_typed_outcome_qualifies_every_other_state_keeps_blocking() {
    let (r, dep) = with_image(Some(CleanupOutcome::Failed("removing image: timed out".into())));
    assert!(!verdict(&r, &dep, NS), "timeout");
    let (r, dep) = with_image(None);
    assert!(!verdict(&r, &dep, NS), "never attempted");
    let (r, dep) = with_image(Some(CleanupOutcome::Removed));
    assert!(!verdict(&r, &dep, NS), "released");
    let (r, dep) = with_image(Some(CleanupOutcome::AlreadyAbsent));
    assert!(!verdict(&r, &dep, NS), "released");
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
    assert!(!verdict(&r, &dep, NS), "outstanding attempt");
    r.push(rec_result(&dep, ResourceKind::Image, IMG, conflict()));
    assert!(verdict(&r, &dep, NS), "settled again as a conflict");
    r.push(rec_attempt(&dep, ResourceKind::Image, IMG));
    r.push(rec_result(
        &dep,
        ResourceKind::Image,
        IMG,
        CleanupOutcome::Failed("disconnected".into()),
    ));
    assert!(!verdict(&r, &dep, NS), "an ambiguous failure replaces the conflict");
}

#[test]
fn a_refusal_after_a_conflict_is_terminal_and_never_deferred() {
    // F1: ownership refused on a later attempt replaces the conflict and is never deferred.
    let (mut r, dep) = with_image(Some(conflict()));
    r.push(rec_attempt(&dep, ResourceKind::Image, IMG));
    r.push(rec_result(
        &dep,
        ResourceKind::Image,
        IMG,
        CleanupOutcome::Refused("labels changed".into()),
    ));
    assert!(!verdict(&r, &dep, NS));
    let l = Ledger::replay(&r).unwrap();
    assert!(matches!(l.entries()[0].state, EntryState::Refused(_)));
    // and nothing can follow a refusal
    let mut more = r.clone();
    more.push(rec_attempt(&dep, ResourceKind::Image, IMG));
    assert!(Ledger::replay(&more).is_err());
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
        assert!(!verdict(&r, &dep, NS), "{state}");
        r.push(rec_attempt(&dep, kind, name));
        r.push(rec_result(&dep, kind, name, CleanupOutcome::Removed));
        assert!(verdict(&r, &dep, NS), "{state} then released");
    }
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
    assert!(!verdict(&r, &dep, NS), "intent with no receipt");
    r.push(rec_receipt(&dep, ResourceKind::Container, "hp-x", "c1"));
    r.push(rec_attempt(&dep, ResourceKind::Container, "hp-x"));
    r.push(rec_result(
        &dep,
        ResourceKind::Container,
        "hp-x",
        CleanupOutcome::Refused("not ours".into()),
    ));
    assert!(!verdict(&r, &dep, NS), "refused sibling");
}

// ---------------------------------------------------------------- placement is validated

#[test]
fn a_conflict_outcome_anywhere_but_a_receipted_retained_image_is_rejected_by_replay() {
    let (app, dep) = ids(1);
    let attempt_conflict = |kind, disp, receipt: bool| {
        let mut r = vec![rec_intent(&app, &dep, kind, "n", disp)];
        if receipt {
            r.push(rec_receipt(&dep, kind, "n", "id-1"));
        }
        r.push(rec_attempt(&dep, kind, "n"));
        r.push(rec_result(&dep, kind, "n", conflict()));
        Ledger::replay(&r)
    };
    assert!(attempt_conflict(ResourceKind::Image, Disposition::Retain, true).is_ok());
    for (kind, disp, receipt, what) in [
        (ResourceKind::Container, Disposition::Retain, true, "a container"),
        (ResourceKind::PortLease, Disposition::Retain, true, "a lease"),
        (
            ResourceKind::Image,
            Disposition::RemoveOnFailure,
            true,
            "an image removed on failure",
        ),
        (
            ResourceKind::Image,
            Disposition::RemoveAlways,
            true,
            "an image removed always",
        ),
        (
            ResourceKind::Image,
            Disposition::Retain,
            false,
            "an image with no receipt",
        ),
    ] {
        let e = attempt_conflict(kind, disp, receipt).unwrap_err();
        assert!(matches!(e.1, LedgerError::OutOfOrder(..)), "{what}: {e:?}");
    }
    // a result without an attempt is rejected for a conflict as for any other outcome
    let (mut r, dep) = with_image(None);
    r.push(rec_result(&dep, ResourceKind::Image, IMG, conflict()));
    assert!(Ledger::replay(&r).is_err());
}

// ---------------------------------------------------------------- the serialized journal

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
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
}

/// A well-formed (checksummed) line with the given body, as only a different build would write it.
fn framed(body: &str) -> String {
    format!("{:08x}\t{body}", crc32(body.as_bytes()))
}

fn file_of(lines: &[String]) -> Vec<u8> {
    let mut v = Vec::new();
    for l in std::iter::once(&encode_header()).chain(lines) {
        v.extend_from_slice(l.as_bytes());
        v.push(b'\n');
    }
    v
}

#[test]
fn the_typed_conflict_survives_encoding_a_restart_and_a_replay() {
    let (r, dep) = with_image(Some(conflict()));
    let lines: Vec<String> = r.iter().map(encode_ledger).collect();
    // the serialized outcome is its own token, not a failure with text
    let last = lines.last().unwrap();
    assert!(last.contains("\tconflict-docker-refused\t"), "{last}");
    assert!(!last.contains("\tfailed\t"), "{last}");
    // restart: bytes -> recover -> replay
    let bytes = file_of(&lines);
    let rec = recover(&bytes).unwrap();
    let state = replay(&rec.lines).unwrap();
    assert_eq!(state.ledger, Ledger::replay(&r).unwrap());
    let e = state.ledger.entries()[0].clone();
    assert!(matches!(
        e.state,
        EntryState::CleanupConflict(ConflictCause::DockerRefused, _)
    ));
    assert!(state.ledger.deferred_image_conflict(&e, NS));
    // every decoded record equals the original, for both causes
    for cause in [ConflictCause::DockerRefused, ConflictCause::NotExclusive] {
        let rec = rec_result(
            &dep,
            ResourceKind::Image,
            IMG,
            CleanupOutcome::Conflict(cause, "a\tb\nc \\ d".into()),
        );
        assert_eq!(codec::decode(&codec::encode(&rec)).unwrap(), rec);
    }
}

#[test]
fn a_typed_conflict_stays_blocking_when_it_is_the_failed_text_that_was_serialized() {
    let (r, dep) = with_image(Some(CleanupOutcome::Failed("image-conflict: x".into())));
    let bytes = file_of(&r.iter().map(encode_ledger).collect::<Vec<_>>());
    let state = replay(&recover(&bytes).unwrap().lines).unwrap();
    let e = state.ledger.entries()[0].clone();
    assert!(!state.ledger.deferred_image_conflict(&e, NS));
    let _ = dep;
}

#[test]
fn unknown_malformed_or_misplaced_outcomes_fail_closed() {
    let (r, _) = with_image(Some(conflict()));
    let good: Vec<String> = r.iter().map(encode_ledger).collect();
    let body_of = |l: &str| l.split_once('\t').unwrap().1.to_string();
    let result_body = body_of(good.last().unwrap());
    // a checksummed line with a token this build does not know (what an older build would face)
    for token in [
        "conflict-docker-refusedX",
        "conflict",
        "conflict-",
        "Conflict-docker-refused",
        "conflict-future-cause",
        "kept",
        "",
    ] {
        let body = result_body.replace("conflict-docker-refused", token);
        let mut lines = good[..good.len() - 1].to_vec();
        lines.push(framed(&body));
        match recover(&file_of(&lines)) {
            Err(FormatError::Corrupt { why, .. }) => assert!(why.contains("outcome"), "{token:?}: {why}"),
            other => panic!("{token:?}: {other:?}"),
        }
    }
    // the wrong number of fields
    for body in [
        format!("{result_body}\textra"),
        result_body.rsplit_once('\t').unwrap().0.to_string(),
    ] {
        let mut lines = good[..good.len() - 1].to_vec();
        lines.push(framed(&body));
        assert!(
            matches!(recover(&file_of(&lines)), Err(FormatError::Corrupt { .. })),
            "{body}"
        );
    }
    // a damaged checksum
    let mut lines = good.clone();
    let last = lines.pop().unwrap();
    lines.push(format!("0{}", &last[1..]));
    assert!(matches!(recover(&file_of(&lines)), Err(FormatError::Corrupt { .. })));
    // a conflict on a container is well-formed text but is rejected as inconsistent on replay
    let (app, dep) = ids(1);
    let misplaced = [
        rec_intent(
            &app,
            &dep,
            ResourceKind::Container,
            "hp-1",
            Disposition::RemoveOnFailure,
        ),
        rec_receipt(&dep, ResourceKind::Container, "hp-1", "c"),
        rec_attempt(&dep, ResourceKind::Container, "hp-1"),
        rec_result(&dep, ResourceKind::Container, "hp-1", conflict()),
    ];
    let bytes = file_of(&misplaced.iter().map(encode_ledger).collect::<Vec<_>>());
    let rec = recover(&bytes).expect("it parses");
    assert!(matches!(replay(&rec.lines), Err(FormatError::Inconsistent(_))));
}

#[test]
fn the_file_version_is_unchanged_so_existing_journals_keep_opening() {
    // The new token is additive: no existing record changes, so the header version stays 1 and
    // journals written before this change replay exactly as before.
    let old = [
        rec_intent(&ids(1).0, &ids(1).1, ResourceKind::Image, IMG, Disposition::Retain),
        rec_receipt(&ids(1).1, ResourceKind::Image, IMG, IID),
        rec_attempt(&ids(1).1, ResourceKind::Image, IMG),
        rec_result(
            &ids(1).1,
            ResourceKind::Image,
            IMG,
            CleanupOutcome::Failed("timed out".into()),
        ),
    ];
    let bytes = file_of(&old.iter().map(encode_ledger).collect::<Vec<_>>());
    assert!(encode_header().contains("\t1"), "{}", encode_header());
    let state = replay(&recover(&bytes).unwrap().lines).unwrap();
    assert!(matches!(state.ledger.entries()[0].state, EntryState::CleanupFailed(_)));
}

#[test]
fn an_image_without_a_receipt_is_never_deferred() {
    let (app, dep) = ids(1);
    let r = vec![
        rec_intent(&app, &dep, ResourceKind::Image, IMG, Disposition::Retain),
        rec_attempt(&dep, ResourceKind::Image, IMG),
    ];
    // a conflict result is not even accepted without a receipt
    let mut with_result = r.clone();
    with_result.push(rec_result(&dep, ResourceKind::Image, IMG, conflict()));
    assert!(Ledger::replay(&with_result).is_err());
    let l = Ledger::replay(&r).unwrap();
    assert!(!l.deferred_image_conflict(&l.entries()[0], NS));
}

// ---------------------------------------------------------------- the engine only types what the ledger accepts

struct Conflicting;
impl harpoon_deploy::backend::ResourceOps for Conflicting {
    fn list_resources(&mut self) -> Result<Vec<harpoon_deploy::ledger::Observed>, harpoon_deploy::backend::StepError> {
        Ok(Vec::new())
    }
    fn remove_resource(
        &mut self,
        _: &harpoon_deploy::ledger::RemoveRequest,
    ) -> Result<harpoon_deploy::ledger::RemoveResult, harpoon_deploy::backend::StepError> {
        Ok(harpoon_deploy::ledger::RemoveResult::Conflict(
            ConflictCause::NotExclusive,
            "in use".into(),
        ))
    }
}

#[test]
fn a_conflict_from_an_operation_on_anything_but_a_retained_image_is_recorded_as_an_ordinary_failure() {
    use harpoon_deploy::engine::{execute_actions, MemoryJournal};
    use harpoon_deploy::ledger::{Action, Observed};
    let (app, dep) = ids(1);
    for (kind, disp, name) in [
        (ResourceKind::Container, Disposition::RemoveOnFailure, "hp-1"),
        (ResourceKind::Image, Disposition::RemoveOnFailure, "img"),
    ] {
        let mut ledger = Ledger::replay(&[
            rec_intent(&app, &dep, kind, name, disp),
            rec_receipt(&dep, kind, name, "id-1"),
        ])
        .unwrap();
        let mut j = MemoryJournal::new();
        let action = Action::Remove {
            entry: (dep.clone(), ResourceKey::new(kind, name)),
            observed: Observed {
                kind,
                name: name.into(),
                id: "id-1".into(),
                labels: vec![],
            },
            adopt: false,
        };
        execute_actions(
            &[action],
            &mut ledger,
            &mut j,
            &mut Conflicting,
            &harpoon_deploy::time::SystemClock,
        )
        .unwrap();
        let e = &ledger.entries()[0];
        assert!(
            matches!(e.state, EntryState::CleanupFailed(_)),
            "{kind:?}/{disp:?}: {:?}",
            e.state
        );
        assert!(!ledger.deferred_image_conflict(e, NS));
    }
    // while a retained image receives the typed outcome
    let mut ledger = Ledger::replay(&with_image(None).0).unwrap();
    let dep = ledger.entries()[0].deployment.clone();
    let mut j = MemoryJournal::new();
    let action = Action::Remove {
        entry: (dep.clone(), ResourceKey::new(ResourceKind::Image, IMG)),
        observed: Observed {
            kind: ResourceKind::Image,
            name: IMG.into(),
            id: IID.into(),
            labels: vec![],
        },
        adopt: false,
    };
    execute_actions(
        &[action],
        &mut ledger,
        &mut j,
        &mut Conflicting,
        &harpoon_deploy::time::SystemClock,
    )
    .unwrap();
    assert!(matches!(
        ledger.entries()[0].state,
        EntryState::CleanupConflict(ConflictCause::NotExclusive, _)
    ));
}
