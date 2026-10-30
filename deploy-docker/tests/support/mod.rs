//! Shared fixtures: ids, a fake Buildx executable, a temp workspace and the harness that runs a
//! whole deployment through the engine. Nothing here touches a real Docker endpoint.
#![allow(dead_code)]

pub mod fake_docker;

use fake_docker::FakeDocker;
use harpoon_deploy::engine::{
    run_deployment_with, EngineError, Journal, JournalError, MemoryJournal, Report, RunConfig,
};
use harpoon_deploy::health::Observation;
use harpoon_deploy::ids::{ApplicationId, IdGenerator};
use harpoon_deploy::ledger::LedgerRecord;
use harpoon_deploy::ledger::Observed;
use harpoon_deploy::state::{Deployment, Trigger};
use harpoon_deploy::time::{Clock, ManualClock, SeqEntropy, SystemClock};
use harpoon_deploy_docker::backend::{image_tag, DockerBackend, DockerConfig};
use harpoon_deploy_docker::preflight::DiskProbe;
use harpoon_deploy_docker::probe::{HttpTransport, PortProbe};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A journal that stamps every ledger write with the shared counter, so a test can prove the
/// order "intent durable → engine call → receipt durable" against the fake engine's requests.
pub struct StampedJournal {
    pub inner: MemoryJournal,
    pub stamps: Vec<(usize, LedgerRecord)>,
}

impl StampedJournal {
    pub fn new() -> StampedJournal {
        StampedJournal {
            inner: MemoryJournal::new(),
            stamps: Vec::new(),
        }
    }
    pub fn crash_at(n: usize) -> StampedJournal {
        StampedJournal {
            inner: MemoryJournal::crash_at(n),
            stamps: Vec::new(),
        }
    }
}

impl Journal for StampedJournal {
    fn persist(&mut self, d: &Deployment) -> Result<(), JournalError> {
        self.inner.persist(d)
    }
    fn record(&mut self, r: &LedgerRecord) -> Result<(), JournalError> {
        self.inner.record(r)?;
        self.stamps.push((fake_docker::tick(), r.clone()));
        Ok(())
    }
}

pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    pub fn new() -> Workspace {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("hdd-ws-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Workspace { root }
    }
    pub fn resources(&self) -> PathBuf {
        self.root.join("Resources")
    }
    pub fn buildx_path(&self) -> PathBuf {
        self.resources().join("harpoon/bin/docker-buildx")
    }
    pub fn context(&self) -> PathBuf {
        self.root.join("src")
    }
    pub fn scratch(&self) -> PathBuf {
        self.root.join("scratch")
    }
    pub fn write_context(&self, dockerfile: &str) {
        std::fs::create_dir_all(self.context()).unwrap();
        std::fs::write(self.context().join("Dockerfile"), dockerfile).unwrap();
    }
    /// Install a fake `docker-buildx` shell script. `body` runs after it records its arguments,
    /// working directory and environment next to itself.
    pub fn install_buildx(&self, body: &str) {
        let p = self.buildx_path();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let script = format!(
            "#!/bin/sh\nD=\"$(dirname \"$0\")\"\necho \"$@\" > \"$D/args.txt\"\nenv | sort > \"$D/env.txt\"\npwd > \"$D/cwd.txt\"\n{body}\n"
        );
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    pub fn buildx_record(&self, name: &str) -> String {
        std::fs::read_to_string(self.resources().join("harpoon/bin").join(name)).unwrap_or_default()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub struct FakeDisk(pub u64);
impl DiskProbe for FakeDisk {
    fn free_bytes(&self, _: &Path) -> Result<u64, String> {
        Ok(self.0)
    }
}

pub struct NoListeners;
impl PortProbe for NoListeners {
    fn is_listening(&self, _: u16) -> bool {
        false
    }
}

pub struct Listening(pub BTreeSet<u16>);
impl PortProbe for Listening {
    fn is_listening(&self, p: u16) -> bool {
        self.0.contains(&p)
    }
}

/// A test transport: answers from a script. Marked simulated, so a backend built on it can only
/// ever be accepted through the engine's explicit simulated-evidence rule.
pub struct Scripted {
    pub answers: RefCell<Vec<Observation>>,
    pub fallback: Observation,
    pub seen: RefCell<Vec<(u16, String)>>,
}

impl Scripted {
    pub fn always(o: Observation) -> Scripted {
        Scripted {
            answers: RefCell::new(Vec::new()),
            fallback: o,
            seen: RefCell::new(Vec::new()),
        }
    }
}

impl HttpTransport for Scripted {
    fn get(&self, port: u16, path: &str, _t: Duration) -> Observation {
        self.seen.borrow_mut().push((port, path.to_string()));
        let mut a = self.answers.borrow_mut();
        if a.is_empty() {
            self.fallback.clone()
        } else {
            a.remove(0)
        }
    }
    fn is_simulated(&self) -> bool {
        true
    }
}

pub struct Ids {
    pub gen: IdGenerator<std::rc::Rc<ManualClock>, SeqEntropy>,
    pub clock: std::rc::Rc<ManualClock>,
    pub app: ApplicationId,
}

pub fn ids() -> Ids {
    let clock = ManualClock::new(1_700_000_000_000);
    let mut gen = IdGenerator::new(clock.clone(), SeqEntropy::new(5));
    let app = gen.application().unwrap();
    Ids { gen, clock, app }
}

pub fn new_deployment(i: &mut Ids) -> Deployment {
    let id = i.gen.deployment().unwrap();
    let mut d = Deployment::new(id, i.app.clone(), Trigger::Manual, i.clock.now_ms());
    d.transition(
        harpoon_deploy::state::DeploymentStatus::Queued,
        i.clock.now_ms(),
    )
    .unwrap();
    d
}

pub fn labels(
    app: &ApplicationId,
    dep: &harpoon_deploy::ids::DeploymentId,
) -> Vec<(String, String)> {
    harpoon_deploy::backend::harpoon_labels(app, dep)
}

pub fn config(ws: &Workspace, docker: &FakeDocker) -> DockerConfig {
    let mut c = DockerConfig::first_test(docker.path(), ws.context(), ws.scratch(), 8080);
    c.health.interval_ms = 5;
    c.health.startup_deadline_ms = 400;
    c.api_timeout = Duration::from_secs(3);
    c.cleanup_timeout = Duration::from_secs(3);
    c
}

/// Everything wired: workspace with a good Dockerfile and a succeeding fake Buildx, a fake engine
/// holding `alpine:3.22` and (when `seed_image` is called) the image the "build" produces.
pub struct Harness {
    pub ws: Workspace,
    pub docker: FakeDocker,
    pub ids: Ids,
    pub dep: Deployment,
}

impl Harness {
    pub fn new() -> Harness {
        let ws = Workspace::new();
        ws.write_context("FROM alpine:3.22\nCMD [\"true\"]\n");
        ws.install_buildx("echo 'step 1/1 : FROM alpine:3.22'\necho done");
        let docker = FakeDocker::start();
        docker.with(|s| {
            s.add_image("alpine:3.22", vec![]);
        });
        let mut ids = ids();
        let dep = new_deployment(&mut ids);
        Harness {
            ws,
            docker,
            ids,
            dep,
        }
    }

    /// What a successful `buildx build --load` leaves in the engine: the image appears when the
    /// backend inspects the tag the *second* time (the first inspect, before the build, must find
    /// nothing).
    pub fn seed_built_image(&self) {
        let l = labels(&self.dep.application_id, &self.dep.id);
        self.seed_built_image_with(l, Vec::new());
    }

    pub fn seed_built_image_with(&self, image_labels: Vec<(String, String)>, volumes: Vec<String>) {
        let tag = image_tag(&self.dep.application_id, &self.dep.id);
        self.docker.fault_after(
            Some("GET"),
            "/images/harpoon/",
            1,
            1,
            fake_docker::Action::Race(std::sync::Arc::new(move |s| {
                let id = s.add_image(&tag, image_labels.clone());
                if let Some(i) = s.images.iter_mut().find(|i| i.id == id) {
                    i.volumes = volumes.clone();
                }
            })),
        );
    }

    /// An image that already holds the deployment's tag before the backend runs.
    pub fn preexisting_image(&self, image_labels: Vec<(String, String)>) -> String {
        let tag = image_tag(&self.dep.application_id, &self.dep.id);
        self.docker.with(|s| s.add_image(&tag, image_labels))
    }

    pub fn backend<T: HttpTransport>(&self, t: T) -> DockerBackend<T> {
        DockerBackend::with_transport(
            config(&self.ws, &self.docker),
            Some(&self.ws.resources()),
            t,
        )
        .expect("bundled buildx resolves")
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024))
    }

    pub fn run<T: HttpTransport>(
        &mut self,
        b: &mut DockerBackend<T>,
        j: &mut dyn Journal,
    ) -> Report {
        self.run_with(b, j, &RunConfig::default())
    }

    pub fn run_with<T: HttpTransport>(
        &mut self,
        b: &mut DockerBackend<T>,
        j: &mut dyn Journal,
        cfg: &RunConfig,
    ) -> Report {
        self.try_run(b, j, cfg)
            .expect("the engine itself did not error")
    }

    pub fn try_run<T: HttpTransport>(
        &mut self,
        b: &mut DockerBackend<T>,
        j: &mut dyn Journal,
        cfg: &RunConfig,
    ) -> Result<Report, EngineError> {
        run_deployment_with(
            &mut self.dep,
            &harpoon_deploy::engine::Context {
                previous_release: None,
            },
            b,
            j,
            &SystemClock,
            cfg,
            &|| false,
        )
    }

    pub fn objects_for_dep(&self) -> Vec<Observed> {
        let (app, dep) = (self.dep.application_id.clone(), self.dep.id.clone());
        self.docker.with(|s| {
            s.containers
                .iter()
                .filter(|c| labels(&app, &dep).iter().all(|l| c.labels.contains(l)))
                .map(|c| Observed {
                    kind: harpoon_deploy::ledger::ResourceKind::Container,
                    name: c.name.clone(),
                    id: c.id.clone(),
                    labels: c.labels.clone(),
                })
                .collect()
        })
    }
}
