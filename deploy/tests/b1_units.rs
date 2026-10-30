//! B1 building blocks in isolation: ledger state machine and planning, the durable line
//! codec, log redaction and truncation, port leases, and health evidence.

mod common;

use harpoon_deploy::health::*;
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator};
use harpoon_deploy::ledger::codec::{self, RecoverError, Tail};
use harpoon_deploy::ledger::*;
use harpoon_deploy::logs::*;
use harpoon_deploy::ports::*;
use harpoon_deploy::time::{ManualClock, SeqEntropy};
use std::collections::BTreeSet;

fn ids() -> (ApplicationId, DeploymentId, DeploymentId) {
    let clock = ManualClock::new(5_000_000);
    let mut g = IdGenerator::new(clock, SeqEntropy::new(3));
    (
        g.application().unwrap(),
        g.deployment().unwrap(),
        g.deployment().unwrap(),
    )
}

fn key(kind: ResourceKind, name: &str) -> ResourceKey {
    ResourceKey::new(kind, name)
}

fn intent(app: &ApplicationId, dep: &DeploymentId, k: &ResourceKey, d: Disposition) -> LedgerRecord {
    LedgerRecord::Intent {
        key: k.clone(),
        app: app.clone(),
        deployment: dep.clone(),
        disposition: d,
        at_ms: 1,
    }
}

fn observed(kind: ResourceKind, name: &str, id: &str, app: &ApplicationId, dep: &DeploymentId) -> Observed {
    Observed {
        kind,
        name: name.into(),
        id: id.into(),
        labels: common::labels_of(app, dep),
    }
}

// ---------------------------------------------------------------- ledger state machine

#[test]
fn the_ledger_enforces_intent_then_receipt_then_attempt_then_result() {
    let (app, dep, _) = ids();
    let k = key(ResourceKind::Container, "c1");
    let mut l = Ledger::new();

    // receipt, attempt or result with no intent
    assert!(matches!(
        l.apply(&LedgerRecord::Receipt {
            key: k.clone(),
            deployment: dep.clone(),
            id: "x".into(),
            at_ms: 1
        }),
        Err(LedgerError::ReceiptConflict(_))
    ));
    assert!(matches!(
        l.apply(&LedgerRecord::CleanupAttempt {
            key: k.clone(),
            deployment: dep.clone(),
            at_ms: 1
        }),
        Err(LedgerError::UnknownEntry(_))
    ));

    l.apply(&intent(&app, &dep, &k, Disposition::RemoveAlways)).unwrap();
    assert_eq!(l.find(&dep, &k).unwrap().state, EntryState::Intended);
    assert!(
        matches!(
            l.apply(&intent(&app, &dep, &k, Disposition::Retain)),
            Err(LedgerError::DuplicateIntent(_))
        ),
        "one entry per deployment and key"
    );

    // a result needs an attempt
    let res = LedgerRecord::CleanupResult {
        key: k.clone(),
        deployment: dep.clone(),
        outcome: CleanupOutcome::Removed,
        at_ms: 2,
    };
    assert!(matches!(l.apply(&res), Err(LedgerError::OutOfOrder(..))));

    let rec = LedgerRecord::Receipt {
        key: k.clone(),
        deployment: dep.clone(),
        id: "id-1".into(),
        at_ms: 2,
    };
    l.apply(&rec).unwrap();
    l.apply(&rec).unwrap(); // idempotent replay of the same receipt
    assert!(
        matches!(
            l.apply(&LedgerRecord::Receipt {
                key: k.clone(),
                deployment: dep.clone(),
                id: "id-2".into(),
                at_ms: 3
            }),
            Err(LedgerError::ReceiptConflict(_))
        ),
        "a second, different id for the same object is a conflict, never an overwrite"
    );
    assert_eq!(l.find(&dep, &k).unwrap().id.as_deref(), Some("id-1"));

    l.apply(&LedgerRecord::CleanupAttempt {
        key: k.clone(),
        deployment: dep.clone(),
        at_ms: 3,
    })
    .unwrap();
    l.apply(&LedgerRecord::CleanupResult {
        key: k.clone(),
        deployment: dep.clone(),
        outcome: CleanupOutcome::Failed("boom".into()),
        at_ms: 4,
    })
    .unwrap();
    assert!(l.find(&dep, &k).unwrap().needs_attention());
    // a failed cleanup may be retried; a settled one may not
    l.apply(&LedgerRecord::CleanupAttempt {
        key: k.clone(),
        deployment: dep.clone(),
        at_ms: 5,
    })
    .unwrap();
    l.apply(&LedgerRecord::CleanupResult {
        key: k.clone(),
        deployment: dep.clone(),
        outcome: CleanupOutcome::Removed,
        at_ms: 6,
    })
    .unwrap();
    assert_eq!(l.find(&dep, &k).unwrap().cleanup_attempts, 2);
    assert!(matches!(
        l.apply(&LedgerRecord::CleanupAttempt {
            key: k.clone(),
            deployment: dep.clone(),
            at_ms: 7
        }),
        Err(LedgerError::OutOfOrder(..))
    ));
}

#[test]
fn replaying_the_records_reproduces_the_ledger_exactly() {
    let (app, dep, _) = ids();
    let recs = vec![
        intent(&app, &dep, &key(ResourceKind::Image, "img"), Disposition::Retain),
        LedgerRecord::Receipt {
            key: key(ResourceKind::Image, "img"),
            deployment: dep.clone(),
            id: "i".into(),
            at_ms: 2,
        },
        intent(
            &app,
            &dep,
            &key(ResourceKind::PortLease, "39001"),
            Disposition::RemoveOnFailure,
        ),
    ];
    let mut live = Ledger::new();
    for r in &recs {
        live.apply(r).unwrap();
    }
    assert_eq!(Ledger::replay(&recs).unwrap(), live);
    assert_eq!(live.leased_ports(), vec![39001]);
    let bad = vec![recs[1].clone()];
    assert_eq!(
        Ledger::replay(&bad).unwrap_err().0,
        0,
        "replay says which record could not apply"
    );
}

// ---------------------------------------------------------------- planning by verified labels

#[test]
fn only_exactly_labelled_objects_are_ever_planned_for_removal() {
    let (app, dep, other_dep) = ids();
    let other_app = {
        let clock = ManualClock::new(9);
        IdGenerator::new(clock, SeqEntropy::new(99)).application().unwrap()
    };
    let k = key(ResourceKind::Container, "web");
    let mut l = Ledger::new();
    l.apply(&intent(&app, &dep, &k, Disposition::RemoveAlways)).unwrap();
    l.apply(&LedgerRecord::Receipt {
        key: k.clone(),
        deployment: dep.clone(),
        id: "real".into(),
        at_ms: 2,
    })
    .unwrap();
    let ended = [(dep.clone(), EndState::NotSucceeded)];

    let ours = observed(ResourceKind::Container, "web", "real", &app, &dep);
    let mut no_managed = ours.clone();
    no_managed.labels.retain(|(k, _)| k != "io.harpoon.managed");
    let mut managed_false = ours.clone();
    managed_false.labels = managed_false
        .labels
        .into_iter()
        .map(|(k, v)| {
            if k == "io.harpoon.managed" {
                (k, "false".into())
            } else {
                (k, v)
            }
        })
        .collect();
    let wrong_app = observed(ResourceKind::Container, "web", "real", &other_app, &dep);
    let wrong_dep = observed(ResourceKind::Container, "web", "real", &app, &other_dep);
    let unlabelled = Observed {
        labels: vec![],
        ..ours.clone()
    };

    assert!(matches!(
        plan(&l, std::slice::from_ref(&ours), &ended)[0],
        Action::Remove { adopt: false, .. }
    ));
    for impostor in [no_managed, managed_false, wrong_app, wrong_dep, unlabelled] {
        let actions = plan(&l, std::slice::from_ref(&impostor), &ended);
        assert_eq!(actions.len(), 1, "{impostor:?}");
        assert!(
            matches!(actions[0], Action::Refuse { .. }),
            "{impostor:?} must be refused, got {:?}",
            actions[0]
        );
    }
}

#[test]
fn deployments_that_may_still_be_running_are_left_alone() {
    let (app, dep, _) = ids();
    let k = key(ResourceKind::Container, "web");
    let mut l = Ledger::new();
    l.apply(&intent(&app, &dep, &k, Disposition::RemoveAlways)).unwrap();
    assert!(plan(&l, &[], &[]).is_empty());
}

#[test]
fn retained_and_succeeded_serving_resources_are_not_cleaned() {
    let (app, dep, _) = ids();
    let img = key(ResourceKind::Image, "img");
    let c = key(ResourceKind::Container, "serving");
    let mut l = Ledger::new();
    for (k, d) in [(&img, Disposition::Retain), (&c, Disposition::RemoveOnFailure)] {
        l.apply(&intent(&app, &dep, k, d)).unwrap();
        l.apply(&LedgerRecord::Receipt {
            key: k.clone(),
            deployment: dep.clone(),
            id: format!("id-{}", k.name),
            at_ms: 2,
        })
        .unwrap();
    }
    assert!(plan(&l, &[], &[(dep.clone(), EndState::Succeeded)]).is_empty());
    let failed = plan(
        &l,
        &[observed(ResourceKind::Container, "serving", "id-serving", &app, &dep)],
        &[(dep.clone(), EndState::NotSucceeded)],
    );
    assert_eq!(
        failed.len(),
        1,
        "on failure only the serving container is removed; the retained image is not"
    );
}

#[test]
fn an_intent_without_receipt_is_adopted_by_name_and_label_and_never_by_name_alone() {
    let (app, dep, _) = ids();
    let k = key(ResourceKind::Container, "web");
    let mut l = Ledger::new();
    l.apply(&intent(&app, &dep, &k, Disposition::RemoveAlways)).unwrap();
    let ended = [(dep.clone(), EndState::NotSucceeded)];
    let mine = observed(ResourceKind::Container, "web", "actual-id", &app, &dep);
    match &plan(&l, &[mine], &ended)[0] {
        Action::Remove {
            adopt: true, observed, ..
        } => assert_eq!(observed.id, "actual-id"),
        a => panic!("{a:?}"),
    }
    let same_name_other_owner = Observed {
        labels: vec![],
        ..observed(ResourceKind::Container, "web", "x", &app, &dep)
    };
    assert!(matches!(
        plan(&l, &[same_name_other_owner], &ended)[0],
        Action::Refuse { .. }
    ));
    assert!(matches!(plan(&l, &[], &ended)[0], Action::MarkAbsent { .. }));
}

#[test]
fn a_retained_intent_without_receipt_is_receipted_not_removed() {
    let (app, dep, _) = ids();
    let k = key(ResourceKind::Image, "img");
    let mut l = Ledger::new();
    l.apply(&intent(&app, &dep, &k, Disposition::Retain)).unwrap();
    let a = plan(
        &l,
        &[observed(ResourceKind::Image, "img", "img-id", &app, &dep)],
        &[(dep.clone(), EndState::NotSucceeded)],
    );
    assert!(matches!(a[0], Action::Adopt { .. }));
}

// ---------------------------------------------------------------- durable line codec

fn sample_records() -> Vec<LedgerRecord> {
    let (app, dep, _) = ids();
    let nasty = key(ResourceKind::Container, "na\tme\nwith \\ odd\r chars ünï");
    vec![
        intent(&app, &dep, &nasty, Disposition::RemoveAlways),
        LedgerRecord::Receipt {
            key: nasty.clone(),
            deployment: dep.clone(),
            id: "id\twith\ttabs".into(),
            at_ms: 2,
        },
        LedgerRecord::CleanupAttempt {
            key: nasty.clone(),
            deployment: dep.clone(),
            at_ms: 3,
        },
        LedgerRecord::CleanupResult {
            key: nasty.clone(),
            deployment: dep.clone(),
            outcome: CleanupOutcome::Failed("boom:\n\tdetails".into()),
            at_ms: 4,
        },
        LedgerRecord::CleanupResult {
            key: nasty.clone(),
            deployment: dep.clone(),
            outcome: CleanupOutcome::Refused("not ours".into()),
            at_ms: 5,
        },
        LedgerRecord::CleanupResult {
            key: nasty.clone(),
            deployment: dep.clone(),
            outcome: CleanupOutcome::Removed,
            at_ms: 6,
        },
        LedgerRecord::CleanupResult {
            key: nasty,
            deployment: dep,
            outcome: CleanupOutcome::AlreadyAbsent,
            at_ms: 7,
        },
        intent(
            &app,
            &ids().1,
            &key(ResourceKind::PortLease, "39007"),
            Disposition::Retain,
        ),
    ]
}

fn stream(recs: &[LedgerRecord]) -> Vec<u8> {
    let mut v = Vec::new();
    for r in recs {
        v.extend_from_slice(codec::encode(r).as_bytes());
        v.push(b'\n');
    }
    v
}

#[test]
fn every_record_roundtrips_through_the_line_format() {
    for r in sample_records() {
        let line = codec::encode(&r);
        assert!(!line.contains('\n') && !line.contains('\r'), "one record, one line");
        assert_eq!(codec::decode(&line).unwrap(), r);
    }
}

#[test]
fn a_torn_write_at_any_byte_recovers_the_complete_prefix_and_reports_the_tail() {
    let recs = sample_records();
    let bytes = stream(&recs);
    // boundaries after each record
    let mut ends = Vec::new();
    let mut acc = 0;
    for r in &recs {
        acc += codec::encode(r).len() + 1;
        ends.push(acc);
    }
    for cut in 0..=bytes.len() {
        let got = codec::recover(&bytes[..cut]).unwrap_or_else(|e| panic!("cut {cut}: {e:?}"));
        let complete = ends.iter().filter(|e| **e <= cut).count();
        assert_eq!(got.records, recs[..complete], "cut {cut}");
        let valid = if complete == 0 { 0 } else { ends[complete - 1] };
        assert_eq!(
            got.valid_len, valid,
            "cut {cut}: the writer must truncate to here before appending"
        );
        if cut == valid {
            assert_eq!(got.tail, Tail::Clean, "cut {cut}");
        } else {
            assert_eq!(got.tail, Tail::PartialDiscarded(cut - valid), "cut {cut}");
        }
    }
}

#[test]
fn appending_after_truncating_to_valid_len_never_glues_records_together() {
    let recs = sample_records();
    let mut bytes = stream(&recs[..3]);
    let torn = codec::encode(&recs[3]);
    bytes.extend_from_slice(&torn.as_bytes()[..torn.len() / 2]); // crash mid-write
    let got = codec::recover(&bytes).unwrap();
    assert_eq!(got.records.len(), 3);
    bytes.truncate(got.valid_len);
    bytes.extend_from_slice(codec::encode(&recs[3]).as_bytes());
    bytes.push(b'\n');
    assert_eq!(codec::recover(&bytes).unwrap().records, recs[..4]);
}

#[test]
fn corruption_in_a_terminated_line_is_an_error_not_a_guess() {
    let recs = sample_records();
    let mut bytes = stream(&recs[..4]);
    // flip one bit inside the second line
    let first_end = codec::encode(&recs[0]).len() + 1;
    bytes[first_end + 12] ^= 0x01;
    assert!(matches!(
        codec::recover(&bytes),
        Err(RecoverError::Corrupt { line: 2, .. })
    ));
}

#[test]
fn a_checksum_protects_every_byte_of_every_record() {
    for r in sample_records().iter().take(3) {
        let line = codec::encode(r).into_bytes();
        for i in 0..line.len() {
            let mut bad = line.clone();
            bad[i] ^= 0x01;
            if let Ok(s) = String::from_utf8(bad) {
                assert!(codec::decode(&s).is_err(), "a flipped bit at {i} went undetected");
            }
        }
    }
}

// ---------------------------------------------------------------- logs

fn sink(head: usize, tail: usize, secrets: &[(&str, &str)]) -> LogSink {
    LogSink::new(
        LogLimits {
            head_bytes: head,
            tail_bytes: tail,
        },
        Redactor::new(secrets),
    )
}

#[test]
fn redaction_has_no_length_floor_and_replaces_longest_secrets_first() {
    let r = Redactor::new(&[("SHORT", "a"), ("LONG", "abc-secret"), ("EMPTY", "")]);
    let (out, n) = r.apply("x=abc-secret y=a");
    assert_eq!(out, "x=[REDACTED:LONG] y=[REDACTED:SHORT]");
    assert_eq!(n, 2);
    assert!(!out.contains("abc-secret"));
    // the short secret did not eat part of the long one first
    assert!(!r.apply("abc-secret").0.contains("bc-secret"));
    // an empty secret is ignored, not "match everywhere"
    assert_eq!(Redactor::new(&[("E", "")]).apply("hello").0, "hello");
}

#[test]
fn the_sink_redacts_before_storing_and_counts_replacements() {
    let mut s = sink(1024, 1024, &[("TOKEN", "hunter2")]);
    s.push_line("login with hunter2 and hunter2 again");
    s.push_text("second\nline hunter2\n");
    let c = s.capture();
    assert!(!c.text.contains("hunter2"));
    assert_eq!(c.summary.redactions, 3);
    assert_eq!(c.summary.lines_seen, 3);
    assert!(!c.summary.truncated());
    assert_eq!(s.redact("error: hunter2"), "error: [REDACTED:TOKEN]");
}

#[test]
fn truncation_keeps_head_and_tail_and_says_exactly_what_was_dropped() {
    let mut s = sink(30, 30, &[]);
    for i in 0..100 {
        s.push_line(&format!("line {i:03}")); // 8 bytes + newline = 9
    }
    let c = s.capture();
    assert!(c.summary.truncated());
    assert_eq!(c.summary.lines_seen, 100);
    assert_eq!(
        c.summary.lines_dropped + c.text.lines().filter(|l| l.starts_with("line ")).count() as u64,
        100
    );
    assert!(c.text.contains("line 000"), "the head is kept");
    assert!(
        c.text.contains("line 099"),
        "the tail is kept (that is where failures are)"
    );
    assert!(!c.text.contains("line 050"));
    let marker = c
        .text
        .lines()
        .find(|l| l.contains("truncated by Harpoon"))
        .expect("truncation is announced in the text");
    assert!(marker.contains(&format!("{} lines", c.summary.lines_dropped)));
    assert_eq!(c.summary.bytes_dropped, c.summary.lines_dropped * 9);
    assert!(c.summary.bytes_kept <= 60 && c.summary.bytes_kept + c.summary.bytes_dropped == 900);
}

#[test]
fn an_untruncated_log_has_no_marker_and_a_small_limit_never_panics() {
    let mut s = sink(100, 100, &[]);
    s.push_line("only line");
    assert!(!s.capture().text.contains("truncated"));
    let mut tiny = sink(0, 0, &[]);
    tiny.push_line("anything at all");
    tiny.push_line("more");
    let c = tiny.capture();
    assert!(c.summary.truncated() && c.summary.lines_seen == 2);
}

// ---------------------------------------------------------------- ports

fn set(v: &[u16]) -> BTreeSet<u16> {
    v.iter().copied().collect()
}

#[test]
fn ranges_are_the_approved_ones_and_never_overlap() {
    assert_eq!(
        (PortRange::APPLICATIONS.start, PortRange::APPLICATIONS.end),
        (38000, 38999)
    );
    assert_eq!((PortRange::TESTS.start, PortRange::TESTS.end), (39000, 39099));
    let (a, t) = (PortRange::APPLICATIONS, PortRange::TESTS);
    assert!(a.end < t.start && !a.contains(t.start) && !t.contains(a.end));
}

#[test]
fn allocation_avoids_listeners_docker_mappings_and_journaled_leases() {
    let occ = Occupancy {
        host_listeners: set(&[39000]),
        docker_mappings: set(&[39001]),
        journaled_leases: set(&[39002]),
    };
    let l = allocate(Purpose::Test, 8080, &occ, None, &[]).unwrap();
    assert_eq!(l.host_port, 39003);
    assert_eq!(occ.taken(39000), Some(Taken::HostListener));
    assert_eq!(occ.taken(39001), Some(Taken::DockerMapping));
    assert_eq!(occ.taken(39002), Some(Taken::JournaledLease));
    // each source alone is enough to exclude a port
    for taken in [
        Occupancy {
            host_listeners: set(&[39000]),
            ..Default::default()
        },
        Occupancy {
            docker_mappings: set(&[39000]),
            ..Default::default()
        },
        Occupancy {
            journaled_leases: set(&[39000]),
            ..Default::default()
        },
    ] {
        assert_ne!(allocate(Purpose::Test, 80, &taken, None, &[]).unwrap().host_port, 39000);
    }
}

#[test]
fn leases_in_the_ledger_count_until_released() {
    let (app, dep, _) = ids();
    let k = key(ResourceKind::PortLease, "39000");
    let mut l = Ledger::new();
    l.apply(&intent(&app, &dep, &k, Disposition::RemoveOnFailure)).unwrap();
    let occ = Occupancy::default().with_ledger(&l);
    assert_eq!(allocate(Purpose::Test, 80, &occ, None, &[]).unwrap().host_port, 39001);
    l.apply(&LedgerRecord::CleanupAttempt {
        key: k.clone(),
        deployment: dep.clone(),
        at_ms: 2,
    })
    .unwrap();
    l.apply(&LedgerRecord::CleanupResult {
        key: k,
        deployment: dep,
        outcome: CleanupOutcome::Removed,
        at_ms: 3,
    })
    .unwrap();
    let occ = Occupancy::default().with_ledger(&l);
    assert_eq!(
        allocate(Purpose::Test, 80, &occ, None, &[]).unwrap().host_port,
        39000,
        "a released lease frees its port"
    );
}

#[test]
fn allocation_stays_in_its_range_wraps_and_reports_exhaustion() {
    let app = allocate(Purpose::Application, 80, &Occupancy::default(), None, &[]).unwrap();
    assert!(PortRange::APPLICATIONS.contains(app.host_port));
    let mut occ = Occupancy {
        host_listeners: (39000..=39099).filter(|p| *p != 39050).collect(),
        ..Default::default()
    };
    assert_eq!(
        allocate(Purpose::Test, 80, &occ, Some(39090), &[]).unwrap().host_port,
        39050,
        "wraps around"
    );
    occ.host_listeners.insert(39050);
    assert_eq!(
        allocate(Purpose::Test, 80, &occ, None, &[]),
        Err(PortError::Exhausted(PortRange::TESTS))
    );
    // a hint outside the range is ignored rather than trusted
    assert!(PortRange::TESTS.contains(
        allocate(Purpose::Test, 80, &Occupancy::default(), Some(1), &[])
            .unwrap()
            .host_port
    ));
}

#[test]
fn requesting_a_specific_port_validates_range_and_occupancy() {
    let occ = Occupancy {
        docker_mappings: set(&[39010]),
        ..Default::default()
    };
    assert_eq!(request(Purpose::Test, 80, 39011, &occ).unwrap().host_port, 39011);
    assert_eq!(
        request(Purpose::Test, 80, 38500, &occ),
        Err(PortError::OutOfRange {
            port: 38500,
            purpose: Purpose::Test
        }),
        "test deployments never use the application range"
    );
    assert_eq!(
        request(Purpose::Application, 80, 39011, &occ),
        Err(PortError::OutOfRange {
            port: 39011,
            purpose: Purpose::Application
        })
    );
    assert_eq!(
        request(Purpose::Test, 80, 39010, &occ),
        Err(PortError::Occupied(39010, Taken::DockerMapping))
    );
}

#[test]
fn a_collision_gets_exactly_one_replacement_then_fails() {
    let occ = Occupancy::default();
    let first = allocate(Purpose::Test, 80, &occ, None, &[]).unwrap();
    let second = reallocate_after_collision(&first, None, &occ).unwrap();
    assert_ne!(second.host_port, first.host_port);
    assert_eq!(
        reallocate_after_collision(&second, Some(first.host_port), &occ),
        Err(PortError::CollisionRepeated {
            first: first.host_port,
            second: second.host_port
        })
    );
}

// ---------------------------------------------------------------- health

struct Scripted {
    script: Vec<(Observation, bool)>,
    i: usize,
}

impl Probe for Scripted {
    fn attempt(&mut self, at_ms: u64) -> Attempt {
        let (o, running) = self.script[self.i.min(self.script.len() - 1)].clone();
        self.i += 1;
        Attempt {
            at_ms,
            observation: o,
            container_running: running,
        }
    }
}

fn health(script: Vec<(Observation, bool)>, spec: HealthSpec) -> CheckEvidence {
    let clock = ManualClock::new(0);
    let mut p = Scripted { script, i: 0 };
    run_health(&spec, "http://127.0.0.1:39001/", &mut p, &clock, &clock)
}

fn ok() -> (Observation, bool) {
    (Observation::Status(200), true)
}
fn refused() -> (Observation, bool) {
    (Observation::ConnectFailed("connection refused".into()), true)
}

#[test]
fn healthy_means_consecutive_successes_while_the_container_runs() {
    let e = health(vec![refused(), refused(), ok(), ok()], HealthSpec::first_test("/"));
    assert_eq!(e.verdict, Verdict::Healthy);
    assert_eq!(
        (e.attempts.len(), e.consecutive_reached, e.consecutive_required),
        (4, 2, 2)
    );
    assert_eq!(e.elapsed_ms, 1_500, "3 waits of 500 ms between 4 attempts");
    assert_eq!(e.scope_note, SCOPE_NOTE);
    assert!(e.scope_note.contains("not a guarantee"));
}

#[test]
fn a_failure_resets_the_streak() {
    let e = health(
        vec![ok(), refused(), ok(), refused(), ok(), ok()],
        HealthSpec::first_test("/"),
    );
    assert_eq!(e.verdict, Verdict::Healthy);
    assert_eq!(e.attempts.len(), 6, "ok,ok never happened in a row until the end");
}

#[test]
fn not_healthy_at_the_deadline_records_what_was_seen() {
    let spec = HealthSpec {
        startup_deadline_ms: 2_000,
        ..HealthSpec::first_test("/health")
    };
    let e = health(vec![(Observation::Status(503), true)], spec);
    assert_eq!(e.verdict, Verdict::NotHealthy);
    assert!(e.attempts.len() >= 4 && e.elapsed_ms <= 2_000);
    assert_eq!(e.consecutive_reached, 0);
    assert!(e.summary().contains("HTTP 503"));
    assert!(!e.is_healthy());
}

#[test]
fn a_server_error_is_not_healthy_even_when_the_class_would_otherwise_pass() {
    let spec = HealthSpec {
        expect: StatusClass::NotServerError,
        startup_deadline_ms: 1_000,
        ..HealthSpec::first_test("/")
    };
    assert_eq!(
        health(
            vec![(Observation::Status(404), true), (Observation::Status(404), true)],
            spec.clone()
        )
        .verdict,
        Verdict::Healthy
    );
    assert_eq!(
        health(vec![(Observation::Status(500), true)], spec).verdict,
        Verdict::NotHealthy
    );
    assert!(!StatusClass::Success2xx.accepts(301) && StatusClass::Success2xx.accepts(204));
}

#[test]
fn a_container_that_exits_can_never_be_healthy() {
    let e = health(
        vec![ok(), (Observation::ContainerExited(Some(1)), false)],
        HealthSpec::first_test("/"),
    );
    assert_eq!(e.verdict, Verdict::Exited);
    assert!(e.summary().contains("exited with code 1"));
    // a good HTTP answer from a container that is no longer running does not count either
    let e = health(vec![(Observation::Status(200), false)], HealthSpec::first_test("/"));
    assert_eq!(e.verdict, Verdict::Exited);
}
