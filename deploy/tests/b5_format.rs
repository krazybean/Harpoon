//! B5: the journal line format — snapshot codec, framing, torn-tail and corruption rules, replay.
//! Pure: no files.

use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator, ReleaseId};
use harpoon_deploy::journal_format::*;
use harpoon_deploy::ledger::{
    CleanupOutcome, Disposition, Ledger, LedgerError, LedgerRecord, ResourceKey, ResourceKind,
};
use harpoon_deploy::state::*;
use harpoon_deploy::time::{ManualClock, SeqEntropy};

fn ids(
    seed: u8,
) -> (
    DeploymentId,
    ApplicationId,
    IdGenerator<std::rc::Rc<ManualClock>, SeqEntropy>,
) {
    let mut g = IdGenerator::new(ManualClock::new(1_700_000_000_000), SeqEntropy::new(seed));
    let d = g.deployment().unwrap();
    let a = g.application().unwrap();
    (d, a, g)
}

fn fresh(seed: u8, trigger: Trigger) -> Deployment {
    let (d, a, _) = ids(seed);
    Deployment::new(d, a, trigger, 1_700_000_000_000)
}

/// A deployment that went through a full cutover with a failure, a recovery and an interruption,
/// with awkward text in every free-form field.
fn busy() -> Deployment {
    let mut d = fresh(3, Trigger::Webhook);
    d.transition(DeploymentStatus::Queued, 10).unwrap();
    d.transition(DeploymentStatus::Running, 11).unwrap();
    d.start_step(Step::Source, 12).unwrap();
    d.finish_step(Step::Source, true, "tab\there\nnewline \\ back\r\u{e9}\u{1F680}", 13)
        .unwrap();
    d.start_step(Step::Build, 14).unwrap();
    d.finish_step(Step::Build, true, "", 15).unwrap();
    d.release = Some(ReleaseId::from_deployment(&d.id));
    d.begin_cutover(16).unwrap();
    d.fail_cutover(17).unwrap();
    d.failure = Some(Failure {
        step: Step::Cutover,
        kind: FailureKind::Cutover,
        message: "could not start:\n\tport \\ busy".into(),
        timed_out: true,
    });
    d.start_recovery(60_000, 18).unwrap();
    d.finish_recovery(RecoveryOutcome::RestoreFailed(Reason::StartFailed), 19)
        .unwrap();
    d
}

fn round(d: &Deployment) -> Deployment {
    let line = encode_snapshot(d);
    assert!(!line.contains('\n'), "one line");
    match decode_line(&line).unwrap() {
        Line::Snapshot(x) => *x,
        other => panic!("{other:?}"),
    }
}

#[test]
fn snapshots_round_trip_exactly_including_awkward_text() {
    let d = busy();
    assert_eq!(round(&d), d);
}

#[test]
fn every_status_trigger_cutover_and_recovery_outcome_round_trips() {
    for t in [
        Trigger::Manual,
        Trigger::Cli,
        Trigger::Poll,
        Trigger::Webhook,
        Trigger::Rollback,
        Trigger::Promote,
    ] {
        let d = fresh(1, t);
        assert_eq!(round(&d), d, "{t:?}");
    }
    for s in DeploymentStatus::ALL {
        let mut d = fresh(2, Trigger::Manual);
        d.status = s;
        assert_eq!(round(&d), d, "{s}");
    }
    for c in [
        CutoverState::NotStarted,
        CutoverState::NotRequired,
        CutoverState::Begun { at_ms: 5 },
        CutoverState::Confirmed { at_ms: 6 },
        CutoverState::Failed { at_ms: 7 },
    ] {
        let mut d = fresh(2, Trigger::Manual);
        d.cutover = c;
        assert_eq!(round(&d), d, "{c:?}");
    }
    let reasons = [
        Reason::NoPreviousRelease,
        Reason::ImageMissing,
        Reason::SecretUnavailable,
        Reason::DockerUnreachable,
        Reason::Timeout,
        Reason::StartFailed,
        Reason::Cancelled,
    ];
    let mut outcomes = vec![
        RecoveryOutcome::InProgress,
        RecoveryOutcome::Restored,
        RecoveryOutcome::RestoredUnverified,
    ];
    for r in reasons {
        outcomes.push(RecoveryOutcome::RestoreFailed(r));
        outcomes.push(RecoveryOutcome::NotAttempted(r));
    }
    for o in outcomes {
        let mut d = fresh(2, Trigger::Manual);
        d.recovery = Some(Recovery {
            outcome: o,
            started_ms: 1,
            finished_ms: Some(2),
            budget_ms: 60_000,
        });
        d.interruption = Some(Interruption {
            started_ms: 3,
            ended_ms: None,
        });
        assert_eq!(round(&d), d, "{o:?}");
    }
    for st in [
        StepStatus::Pending,
        StepStatus::Running,
        StepStatus::Succeeded,
        StepStatus::Failed,
        StepStatus::Skipped,
        StepStatus::Interrupted,
    ] {
        let mut d = fresh(2, Trigger::Manual);
        d.steps[2].status = st;
        d.steps[2].started_ms = Some(9);
        assert_eq!(round(&d), d, "{st:?}");
    }
}

#[test]
fn a_snapshot_with_an_unknown_version_or_wrong_shape_is_refused_not_guessed() {
    let line = encode_snapshot(&busy());
    // re-frame a modified body with a valid checksum so only the content is wrong
    let reframe = |body: &str| {
        let crc = {
            // the codec's crc is private; build the frame through a real snapshot's prefix trick:
            // decode_line only needs crc(body) == prefix, so compute it with the public ledger codec
            // by encoding a throwaway record is not possible — use the header encoder's algorithm
            // through a tiny local copy
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
        format!("{crc:08x}\t{body}")
    };
    let body = line.split_once('\t').unwrap().1.to_string();
    let v2 = body.replacen("snap\t1\t", "snap\t2\t", 1);
    let e = decode_line(&reframe(&v2)).unwrap_err();
    assert!(e.0.contains("version 2 is not supported"), "{e}");
    let short = body.rsplit_once('\t').unwrap().0.to_string();
    assert!(decode_line(&reframe(&short)).unwrap_err().0.contains("fields"));
    let extra = format!("{body}\textra");
    assert!(decode_line(&reframe(&extra)).unwrap_err().0.contains("fields"));
    let bad_status = body.replacen("\tfailed\t", "\tnonsense\t", 1);
    assert!(decode_line(&reframe(&bad_status)).is_err());
    // a release belonging to some other deployment
    let (other, _, _) = ids(9);
    let rel = ReleaseId::from_deployment(&busy().id).to_string();
    let swapped = body.replacen(&rel, &ReleaseId::from_deployment(&other).to_string(), 1);
    assert!(decode_line(&reframe(&swapped)).unwrap_err().0.contains("release"));
}

#[test]
fn any_single_byte_change_to_a_line_is_detected() {
    let lines = vec![
        encode_snapshot(&busy()),
        encode_header(),
        encode_ledger(&LedgerRecord::Receipt {
            key: ResourceKey::new(ResourceKind::Container, "hp-x"),
            deployment: busy().id,
            id: "abc\tdef".into(),
            at_ms: 5,
        }),
    ];
    for line in lines {
        let original = decode_line(&line).unwrap();
        for i in 0..line.len() {
            let mut b = line.clone().into_bytes();
            b[i] = b[i].wrapping_add(1);
            let Ok(s) = String::from_utf8(b) else { continue };
            match decode_line(&s) {
                Err(_) => {}
                Ok(x) => panic!("byte {i} changed but the line still decodes ({x:?} vs {original:?})"),
            }
        }
    }
}

fn file_of(lines: &[String]) -> Vec<u8> {
    let mut v = Vec::new();
    for l in lines {
        v.extend_from_slice(l.as_bytes());
        v.push(b'\n');
    }
    v
}

fn sample_file() -> (Vec<u8>, Vec<usize>) {
    let d = busy();
    let intent = LedgerRecord::Intent {
        key: ResourceKey::new(ResourceKind::Container, "hp-c"),
        app: d.application_id.clone(),
        deployment: d.id.clone(),
        disposition: Disposition::RemoveAlways,
        at_ms: 1,
    };
    let receipt = LedgerRecord::Receipt {
        key: ResourceKey::new(ResourceKind::Container, "hp-c"),
        deployment: d.id.clone(),
        id: "c0ffee".into(),
        at_ms: 2,
    };
    let lines = vec![
        encode_header(),
        encode_snapshot(&d),
        encode_ledger(&intent),
        encode_ledger(&receipt),
        encode_snapshot(&d),
    ];
    let mut bounds = vec![0];
    let mut acc = 0;
    for l in &lines {
        acc += l.len() + 1;
        bounds.push(acc);
    }
    (file_of(&lines), bounds)
}

#[test]
fn truncating_a_journal_at_every_byte_recovers_exactly_the_whole_records() {
    let (file, bounds) = sample_file();
    for cut in 0..=file.len() {
        let r = recover(&file[..cut]).unwrap_or_else(|e| panic!("cut at {cut}: {e}"));
        let whole = bounds.iter().rposition(|b| *b <= cut).unwrap();
        assert_eq!(r.valid_len, bounds[whole], "cut at {cut}");
        assert_eq!(r.lines.len(), whole.saturating_sub(1), "cut at {cut}");
        assert_eq!(r.has_header, whole >= 1);
        if cut == bounds[whole] {
            assert_eq!(r.tail, Tail::Clean, "cut at {cut}");
        } else {
            assert_eq!(r.tail, Tail::PartialDiscarded(cut - bounds[whole]), "cut at {cut}");
        }
    }
}

#[test]
fn a_damaged_committed_record_is_an_error_naming_its_line_and_nothing_after_it_is_trusted() {
    let (file, bounds) = sample_file();
    // damage one byte inside each record in turn (never the newline)
    for rec in 0..bounds.len() - 1 {
        let mut f = file.clone();
        let i = bounds[rec] + 12.min(bounds[rec + 1] - bounds[rec] - 2);
        f[i] ^= 0x01;
        let e = recover(&f).unwrap_err();
        match (rec, &e) {
            (0, FormatError::BadHeader(_)) => {}
            (n, FormatError::Corrupt { line, .. }) if n > 0 => assert_eq!(*line, rec + 1),
            other => panic!("record {rec}: {other:?}"),
        }
    }
    // damage in the *last* terminated record is corruption too, not a torn tail
    let mut f = file.clone();
    let last = bounds[bounds.len() - 2] + 12;
    f[last] ^= 0x01;
    assert!(matches!(recover(&f), Err(FormatError::Corrupt { .. })));
    // an unterminated tail with garbage is simply discarded
    let mut f = file.clone();
    f.extend_from_slice(b"deadbeef\tsnap\t1\tgarb");
    let r = recover(&f).unwrap();
    assert_eq!(r.valid_len, file.len());
    assert!(matches!(r.tail, Tail::PartialDiscarded(_)));
    // not UTF-8
    let mut f = file.clone();
    f.splice(bounds[2]..bounds[2], [0xff, 0xfe, b'\n']);
    assert!(matches!(recover(&f), Err(FormatError::Corrupt { line: 3, .. })));
}

#[test]
fn headers_are_checked_and_versions_are_refused() {
    assert!(matches!(recover(b"hello world\n"), Err(FormatError::BadHeader(_))));
    let (file, bounds) = sample_file();
    // first line is a valid record but not a header
    assert!(matches!(recover(&file[bounds[1]..]), Err(FormatError::BadHeader(_))));
    // a second header
    let mut f = file.clone();
    f.extend_from_slice(encode_header().as_bytes());
    f.push(b'\n');
    assert!(matches!(recover(&f), Err(FormatError::Corrupt { .. })));
    // a future file version: the checksum is valid, so it is the version that is refused
    let body = format!("header\tharpoon-journal\t{}", FILE_VERSION + 1);
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
    let line = format!("{:08x}\t{body}\n", !crc);
    assert_eq!(
        recover(line.as_bytes()),
        Err(FormatError::UnsupportedVersion(FILE_VERSION + 1))
    );
    // empty and a torn header
    let r = recover(b"").unwrap();
    assert!(!r.has_header && r.tail == Tail::Clean && r.valid_len == 0);
    let h = encode_header();
    let r = recover(&h.as_bytes()[..10]).unwrap();
    assert!(!r.has_header && r.tail == Tail::PartialDiscarded(10));
}

#[test]
fn replay_rebuilds_the_latest_snapshot_and_the_ledger_and_refuses_contradictions() {
    let (file, _) = sample_file();
    let st = replay(&recover(&file).unwrap().lines).unwrap();
    assert_eq!(st.deployments, vec![busy()]);
    assert_eq!(st.ledger.entries().len(), 1);
    assert_eq!(st.ledger.entries()[0].id.as_deref(), Some("c0ffee"));

    // a terminal deployment must not change afterwards
    let mut d = fresh(5, Trigger::Manual);
    d.transition(DeploymentStatus::Queued, 1).unwrap();
    d.transition(DeploymentStatus::Cancelled, 2).unwrap();
    let mut changed = d.clone();
    changed.status = DeploymentStatus::Running;
    let lines = vec![Line::Snapshot(Box::new(d.clone())), Line::Snapshot(Box::new(changed))];
    assert!(matches!(replay(&lines), Err(FormatError::Inconsistent(m)) if m.contains("changed after")));
    // the same terminal snapshot twice is harmless (idempotent writers)
    assert!(replay(&[Line::Snapshot(Box::new(d.clone())), Line::Snapshot(Box::new(d))]).is_ok());

    // a receipt for something never intended
    let r = LedgerRecord::Receipt {
        key: ResourceKey::new(ResourceKind::Container, "x"),
        deployment: busy().id,
        id: "i".into(),
        at_ms: 1,
    };
    assert!(matches!(replay(&[Line::Ledger(r)]), Err(FormatError::Inconsistent(_))));
}

#[test]
fn a_port_lease_cannot_be_held_by_two_deployments_but_can_be_reused_once_released() {
    let (d1, a, mut g) = ids(4);
    let d2 = g.deployment().unwrap();
    let key = ResourceKey::new(ResourceKind::PortLease, "38000");
    let intent = |d: &DeploymentId| LedgerRecord::Intent {
        key: key.clone(),
        app: a.clone(),
        deployment: d.clone(),
        disposition: Disposition::RemoveOnFailure,
        at_ms: 1,
    };
    let mut l = Ledger::new();
    l.apply(&intent(&d1)).unwrap();
    assert!(matches!(l.apply(&intent(&d2)), Err(LedgerError::LeaseHeld { holder, .. }) if holder == d1));
    // the holder gives it back
    l.apply(&LedgerRecord::CleanupAttempt {
        key: key.clone(),
        deployment: d1.clone(),
        at_ms: 2,
    })
    .unwrap();
    l.apply(&LedgerRecord::CleanupResult {
        key: key.clone(),
        deployment: d1.clone(),
        outcome: CleanupOutcome::Removed,
        at_ms: 3,
    })
    .unwrap();
    l.apply(&intent(&d2)).unwrap();
    // a *failed* release does not free it
    let d3 = g.deployment().unwrap();
    l.apply(&LedgerRecord::CleanupAttempt {
        key: key.clone(),
        deployment: d2.clone(),
        at_ms: 4,
    })
    .unwrap();
    l.apply(&LedgerRecord::CleanupResult {
        key: key.clone(),
        deployment: d2.clone(),
        outcome: CleanupOutcome::Failed("x".into()),
        at_ms: 5,
    })
    .unwrap();
    assert!(matches!(l.apply(&intent(&d3)), Err(LedgerError::LeaseHeld { .. })));
    // other resource kinds with the same name are unaffected
    let other = LedgerRecord::Intent {
        key: ResourceKey::new(ResourceKind::Container, "38000"),
        app: a,
        deployment: d3,
        disposition: Disposition::Retain,
        at_ms: 6,
    };
    l.apply(&other).unwrap();
}
