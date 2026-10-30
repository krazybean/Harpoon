//! Shared fixtures for the B1 tests. No Docker, VM, network, file or process access.
#![allow(dead_code)]

use harpoon_deploy::backend::{harpoon_labels, FakeBackend, FakeRuntime, World};
use harpoon_deploy::engine::{
    end_state, reconcile, run_deployment, Context, EngineError, MemoryJournal, Policy, Report,
};
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator, ReleaseId};
use harpoon_deploy::ledger::{EndState, Ledger, Observed, ResourceKind};
use harpoon_deploy::state::{recover_on_start, Deployment, DeploymentStatus, Trigger};
use harpoon_deploy::time::{ManualClock, SeqEntropy};
use std::rc::Rc;

pub struct Fx {
    pub clock: Rc<ManualClock>,
    pub gen: IdGenerator<Rc<ManualClock>, SeqEntropy>,
    pub app: ApplicationId,
    pub world: World,
}

impl Fx {
    pub fn new() -> Fx {
        let clock = ManualClock::new(1_000_000);
        let mut gen = IdGenerator::new(clock.clone(), SeqEntropy::new(11));
        let app = gen.application().unwrap();
        Fx {
            clock,
            gen,
            app,
            world: FakeRuntime::world(),
        }
    }
    pub fn deployment(&mut self) -> Deployment {
        use harpoon_deploy::time::Clock;
        let id = self.gen.deployment().unwrap();
        let mut d = Deployment::new(id, self.app.clone(), Trigger::Manual, self.clock.now_ms());
        d.transition(DeploymentStatus::Queued, self.clock.now_ms()).unwrap();
        d
    }
    pub fn previous(&mut self) -> Context {
        let prev: DeploymentId = self.gen.deployment().unwrap();
        Context {
            previous_release: Some(ReleaseId::from_deployment(&prev)),
        }
    }
    pub fn first() -> Context {
        Context { previous_release: None }
    }
    pub fn backend(&self) -> FakeBackend {
        FakeBackend::new(self.clock.clone()).with_runtime(self.world.clone())
    }

    /// Objects that are not this deployment's and must survive everything: one with no
    /// labels, one labelled for another application/deployment.
    pub fn add_bystanders(&mut self) -> Vec<Observed> {
        let other_app = self.gen.application().unwrap();
        let other_dep = self.gen.deployment().unwrap();
        let mut w = self.world.borrow_mut();
        w.add_foreign(ResourceKind::Container, "someone-elses-db", vec![]);
        w.add_foreign(ResourceKind::Image, "postgres:16", vec![]);
        w.add_foreign(
            ResourceKind::Container,
            "other-harpoon-app",
            harpoon_labels(&other_app, &other_dep),
        );
        w.objects.clone()
    }
}

pub fn never() -> bool {
    false
}

pub fn run(
    fx: &Fx,
    d: &mut Deployment,
    ctx: &Context,
    b: &mut FakeBackend,
    j: &mut MemoryJournal,
) -> Result<Report, EngineError> {
    run_deployment(d, ctx, b, j, &fx.clock, &Policy::default(), &never)
}

pub fn labelled_for(fx: &Fx, d: &Deployment) -> Vec<Observed> {
    fx.world
        .borrow()
        .objects
        .iter()
        .filter(|o| o.labelled_for(&d.application_id, &d.id))
        .cloned()
        .collect()
}

/// What a restarted process does with a journal that ended in a crash: replay the ledger,
/// mark the in-flight deployment interrupted, then reconcile against the (shared) runtime.
pub fn restart_and_reconcile(
    fx: &Fx,
    old: &MemoryJournal,
    new: &mut MemoryJournal,
) -> Result<(Ledger, EndState, Vec<Observed>), EngineError> {
    let mut ledger = Ledger::replay(&old.ledger_records).expect("the journal's ledger records always replay");
    let Some(last) = old.last() else {
        return Ok((ledger, EndState::NotSucceeded, Vec::new()));
    };
    let mut d = last.clone();
    {
        use harpoon_deploy::time::Clock;
        recover_on_start(std::slice::from_mut(&mut d), fx.clock.now_ms());
    }
    let end = end_state(d.status).expect("recovered deployments are terminal");
    let mut backend = fx.backend();
    let un = reconcile(&mut ledger, &[(d.id.clone(), end)], new, &mut backend, &fx.clock)?;
    Ok((ledger, end, un))
}

pub fn labels_of(app: &ApplicationId, dep: &DeploymentId) -> Vec<(String, String)> {
    harpoon_labels(app, dep)
}
