//! B8.3 regressions: live-configured health budgets and safe reconciliation ordering.

mod common;

use common::Fx;
use harpoon_deploy::backend::harpoon_labels;
use harpoon_deploy::engine::{reconcile, MemoryJournal};
use harpoon_deploy::ledger::{Disposition, EndState, Ledger, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::time::Clock;

#[test]
fn reconciliation_keeps_the_lease_when_its_container_removal_fails() {
    let mut fx = Fx::new();
    let d = fx.deployment();
    let lease = ResourceKey::new(ResourceKind::PortLease, "39000");
    let container = ResourceKey::new(ResourceKind::Container, "hp-b83");
    let labels = harpoon_labels(&d.application_id, &d.id);
    fx.world
        .borrow_mut()
        .create(ResourceKind::Container, &container.name, labels);

    let at = fx.clock.now_ms();
    let mut ledger = Ledger::new();
    for record in [
        LedgerRecord::Intent {
            key: lease.clone(),
            app: d.application_id.clone(),
            deployment: d.id.clone(),
            disposition: Disposition::RemoveOnFailure,
            at_ms: at,
        },
        LedgerRecord::Receipt {
            key: lease.clone(),
            deployment: d.id.clone(),
            id: lease.name.clone(),
            at_ms: at,
        },
        LedgerRecord::Intent {
            key: container.clone(),
            app: d.application_id.clone(),
            deployment: d.id.clone(),
            disposition: Disposition::RemoveOnFailure,
            at_ms: at,
        },
        LedgerRecord::Receipt {
            key: container.clone(),
            deployment: d.id.clone(),
            id: "fake-0001".into(),
            at_ms: at,
        },
    ] {
        ledger.apply(&record).unwrap();
    }

    let mut backend = fx.backend().fail_remove(&container.name);
    let mut journal = MemoryJournal::new();
    reconcile(
        &mut ledger,
        &[(d.id.clone(), EndState::NotSucceeded)],
        &mut journal,
        &mut backend,
        &fx.clock,
    )
    .unwrap();

    assert!(!ledger.find(&d.id, &container).unwrap().is_released());
    assert!(!ledger.find(&d.id, &lease).unwrap().is_released());
    assert!(matches!(
        ledger.find(&d.id, &container).unwrap().state,
        harpoon_deploy::ledger::EntryState::CleanupFailed(_)
    ));
    assert!(fx.world.borrow().has(&container.name));
}
