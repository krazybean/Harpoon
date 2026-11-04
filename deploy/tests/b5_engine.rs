//! B5 engine hooks: a run can start from a restored ledger, and its report is about its own
//! deployment only.

mod common;

use common::*;
use harpoon_deploy::engine::{run_deployment_with, MemoryJournal, RunConfig};
use harpoon_deploy::ledger::{CleanupOutcome, Disposition, Ledger, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::state::DeploymentStatus;
use harpoon_deploy::time::Clock;

/// A ledger as a restart would restore it: another (dead) deployment holds 39000, and its
/// container's removal failed earlier.
fn restored(fx: &mut Fx) -> (Ledger, harpoon_deploy::ids::DeploymentId) {
    let other = fx.gen.deployment().unwrap();
    let mut l = Ledger::new();
    let at = fx.clock.now_ms();
    let lease = ResourceKey::new(ResourceKind::PortLease, "39000");
    let ctr = ResourceKey::new(ResourceKind::Container, "hp-old");
    for r in [
        LedgerRecord::Intent {
            key: lease.clone(),
            app: fx.app.clone(),
            deployment: other.clone(),
            disposition: Disposition::RemoveOnFailure,
            at_ms: at,
        },
        LedgerRecord::Receipt {
            key: lease,
            deployment: other.clone(),
            id: "39000".into(),
            at_ms: at,
        },
        LedgerRecord::Intent {
            key: ctr.clone(),
            app: fx.app.clone(),
            deployment: other.clone(),
            disposition: Disposition::RemoveOnFailure,
            at_ms: at,
        },
        LedgerRecord::Receipt {
            key: ctr.clone(),
            deployment: other.clone(),
            id: "c-old".into(),
            at_ms: at,
        },
        LedgerRecord::CleanupAttempt {
            key: ctr.clone(),
            deployment: other.clone(),
            at_ms: at,
        },
        LedgerRecord::CleanupResult {
            key: ctr,
            deployment: other.clone(),
            outcome: CleanupOutcome::Failed("daemon down".into()),
            at_ms: at,
        },
    ] {
        l.apply(&r).unwrap();
    }
    (l, other)
}

#[test]
fn a_run_starting_from_a_restored_ledger_sees_its_leases_and_leaves_its_entries_alone() {
    let mut fx = Fx::new();
    let (prior, other) = restored(&mut fx);
    let mut d = fx.deployment();
    let mut b = fx.backend();
    let mut j = MemoryJournal::new();
    let cfg = RunConfig {
        prior_ledger: prior.clone(),
        ..RunConfig::default()
    };
    let r = run_deployment_with(&mut d, &Fx::first(), &mut b, &mut j, &fx.clock, &cfg, &|| false).unwrap();
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    assert_eq!(r.deployment, d.id);
    // the run never used the port the dead deployment still holds
    let ports: Vec<u16> = r
        .ledger
        .entries()
        .iter()
        .filter(|e| e.key.kind == ResourceKind::PortLease && e.deployment == d.id)
        .map(|e| e.key.name.parse().unwrap())
        .collect();
    assert!(!ports.is_empty() && !ports.contains(&39000), "{ports:?}");
    assert!(
        r.ledger.leased_ports().contains(&39000),
        "the other lease is still held"
    );
    // the other deployment's entries are exactly as restored: not cleaned, not adopted, not hidden
    for e in prior.entries() {
        assert_eq!(r.ledger.find(&other, &e.key), Some(e));
    }
    // and its failed cleanup is not charged to this deployment
    assert!(r.cleanup_problems().is_empty());
    assert!(r.is_clean());
    assert_eq!(
        r.ledger.needing_attention().len(),
        1,
        "it is still visible in the ledger itself"
    );
    // nothing of the other deployment was written to this run's journal
    assert!(j.ledger_records.iter().all(|x| x.deployment() == &d.id));
}

#[test]
fn without_a_restored_ledger_nothing_changes() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend();
    let mut j = MemoryJournal::new();
    let r = run_deployment_with(
        &mut d,
        &Fx::first(),
        &mut b,
        &mut j,
        &fx.clock,
        &RunConfig::default(),
        &|| false,
    )
    .unwrap();
    assert!(r.ledger.entries().iter().all(|e| e.deployment == d.id));
    assert!(r.is_clean());
}
