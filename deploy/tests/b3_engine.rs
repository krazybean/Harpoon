//! B3 engine-side contracts: the preliminary check's resources are gone before cutover, a
//! real backend cannot succeed on simulated evidence, and a collided lease is released.

mod common;

use common::*;
use harpoon_deploy::backend::*;
use harpoon_deploy::engine::{MemoryJournal, Policy, StepIo};
use harpoon_deploy::health::CheckEvidence;
use harpoon_deploy::ledger::*;
use harpoon_deploy::ports::{PortLease, Purpose};
use harpoon_deploy::state::*;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn the_preliminary_container_is_removed_before_the_cutover_begins() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let t: OpTrace = Rc::new(RefCell::new(Vec::new()));
    let mut b = fx.backend().with_ops_trace(t.clone());
    let mut j = MemoryJournal::new().with_ops_trace(t.clone());
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    let pre = format!("hp-{}-pre", d.id);
    let serving = format!("hp-{}", d.id);
    let trace = t.borrow();
    let removed_pre = trace
        .iter()
        .position(|o| matches!(o, Op::Write(LedgerRecord::CleanupResult { key, .. }) if key.name == pre))
        .expect("the preliminary container's cleanup result is recorded");
    let created_serving = trace
        .iter()
        .position(|o| matches!(o, Op::Created { name, .. } if *name == serving))
        .expect("the serving container is created");
    assert!(removed_pre < created_serving, "preliminary resources go before cutover");
    assert!(!fx.world.borrow().has(&pre));
    assert!(fx.world.borrow().has(&serving));
}

#[test]
fn a_failed_preliminary_check_still_removes_its_container() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend().fail_preliminary("probe failed");
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let pre = format!("hp-{}-pre", d.id);
    assert!(!fx.world.borrow().has(&pre));
    assert!(r.cleanup_problems().is_empty());
}

#[test]
fn the_preliminary_cleanup_never_touches_other_deployments_or_unlabelled_objects() {
    let mut fx = Fx::new();
    let bystanders = fx.add_bystanders();
    let mut d = fx.deployment();
    // an unlabelled object that happens to carry the preliminary container's name
    let pre = format!("hp-{}-pre", d.id);
    fx.world.borrow_mut().add_foreign(ResourceKind::Container, &pre, vec![]);
    let mut b = fx.backend();
    let mut j = MemoryJournal::new();
    let _ = run(&fx, &mut d, &Fx::first(), &mut b, &mut j);
    for by in &bystanders {
        assert!(fx.world.borrow().objects.contains(by), "touched {}", by.name);
    }
    assert!(
        fx.world
            .borrow()
            .objects
            .iter()
            .any(|o| o.name == pre && o.labels.is_empty()),
        "the unlabelled namesake survives"
    );
}

/// Delegates to a FakeBackend but, like a real backend, does not accept simulated evidence.
struct RealisticFake(FakeBackend);

impl ResourceOps for RealisticFake {
    fn list_resources(&mut self) -> Result<Vec<Observed>, StepError> {
        self.0.list_resources()
    }
    fn remove_resource(&mut self, req: &RemoveRequest) -> Result<RemoveResult, StepError> {
        self.0.remove_resource(req)
    }
}

impl Backend for RealisticFake {
    fn prepare_source(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        self.0.prepare_source(io)
    }
    fn build(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        self.0.build(io)
    }
    fn preliminary_check(&mut self, io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError> {
        self.0.preliminary_check(io)
    }
    fn stop_current(&mut self, io: &mut StepIo<'_>, p: &PreviousRelease) -> Result<(), StepError> {
        self.0.stop_current(io, p)
    }
    fn start_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        self.0.start_new(io)
    }
    fn final_check(&mut self, io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError> {
        self.0.final_check(io)
    }
    fn stop_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        self.0.stop_new(io)
    }
    fn restore_previous(
        &mut self,
        io: &mut StepIo<'_>,
        p: &PreviousRelease,
        budget_ms: u64,
    ) -> Result<RestoreResult, StepError> {
        self.0.restore_previous(io, p, budget_ms)
    }
    // accepts_simulated_evidence: the trait default (false)
}

#[test]
fn a_backend_that_does_not_accept_simulated_evidence_cannot_succeed_on_it() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = RealisticFake(fx.backend());
    let mut j = MemoryJournal::new();
    let r = harpoon_deploy::engine::run_deployment(
        &mut d,
        &Fx::first(),
        &mut b,
        &mut j,
        &fx.clock,
        &Policy::default(),
        &never,
    )
    .unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = d.failure.as_ref().unwrap();
    assert_eq!(f.step, Step::PreliminaryCheck);
    assert!(f.message.contains("simulated evidence"), "{}", f.message);
    assert!(
        r.evidence.iter().any(|(_, e)| e.simulated),
        "the rejected evidence is kept"
    );
}

/// A backend whose start_new collides once and takes a replacement port.
struct Collider(FakeBackend);

impl ResourceOps for Collider {
    fn list_resources(&mut self) -> Result<Vec<Observed>, StepError> {
        self.0.list_resources()
    }
    fn remove_resource(&mut self, req: &RemoveRequest) -> Result<RemoveResult, StepError> {
        self.0.remove_resource(req)
    }
}

impl Backend for Collider {
    fn prepare_source(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        self.0.prepare_source(io)
    }
    fn build(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        self.0.build(io)
    }
    fn preliminary_check(&mut self, io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError> {
        self.0.preliminary_check(io)
    }
    fn stop_current(&mut self, io: &mut StepIo<'_>, p: &PreviousRelease) -> Result<(), StepError> {
        self.0.stop_current(io, p)
    }
    fn start_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        let lease = |p| PortLease {
            host_port: p,
            container_port: 8080,
            purpose: Purpose::Application,
        };
        io.lease_port(lease(38000))?;
        io.release_lease(38000)?; // collided: give it back
        io.lease_port(lease(38001))?;
        self.0.start_new(io)
    }
    fn final_check(&mut self, io: &mut StepIo<'_>) -> Result<CheckEvidence, StepError> {
        self.0.final_check(io)
    }
    fn stop_new(&mut self, io: &mut StepIo<'_>) -> Result<(), StepError> {
        self.0.stop_new(io)
    }
    fn restore_previous(
        &mut self,
        io: &mut StepIo<'_>,
        p: &PreviousRelease,
        budget_ms: u64,
    ) -> Result<RestoreResult, StepError> {
        self.0.restore_previous(io, p, budget_ms)
    }
    fn accepts_simulated_evidence(&self) -> bool {
        true
    }
}

#[test]
fn a_released_lease_is_recorded_as_released_and_the_replacement_stays_leased() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = Collider(fx.backend());
    let mut j = MemoryJournal::new();
    let r = harpoon_deploy::engine::run_deployment(
        &mut d,
        &Fx::first(),
        &mut b,
        &mut j,
        &fx.clock,
        &Policy::default(),
        &never,
    )
    .unwrap();
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", d.failure);
    let lease = |p: &str| {
        r.ledger
            .entries()
            .iter()
            .find(|e| e.key.kind == ResourceKind::PortLease && e.key.name == p)
            .unwrap()
            .clone()
    };
    assert!(lease("38000").is_released() && lease("38000").cleanup_attempts == 1);
    assert!(!lease("38001").is_released());
    let held = r.ledger.leased_ports();
    assert!(held.contains(&38001) && !held.contains(&38000), "{held:?}");
    let _ = std::mem::size_of::<Reason>();
}

#[test]
fn an_unlistable_runtime_never_turns_the_preliminary_cleanup_into_already_absent() {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend().fail_list();
    let mut j = MemoryJournal::new();
    let r = run(&fx, &mut d, &Fx::first(), &mut b, &mut j).unwrap();
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = d.failure.as_ref().unwrap();
    assert_eq!(f.step, Step::PreliminaryCheck);
    let pre = format!("hp-{}-pre", d.id);
    let e = r.ledger.entries().iter().find(|e| e.key.name == pre).unwrap();
    assert!(
        matches!(&e.state, EntryState::CleanupFailed(m) if m.contains("could not list")),
        "{:?}",
        e.state
    );
    assert!(fx.world.borrow().has(&pre), "the container really is still there");
}
