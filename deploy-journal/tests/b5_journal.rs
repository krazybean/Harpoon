//! B5: the durable journal on a real filesystem (temporary directories). No Docker, VM, Buildx
//! or network. The only fakes are the engine's `FakeBackend`/`FakeRuntime` and a wrapper that
//! makes appends/syncs fail or tear at chosen points on top of a real file.

use harpoon_deploy::backend::{harpoon_labels, FakeBackend, FakeRuntime, World};
use harpoon_deploy::engine::{
    end_state, reconcile, run_deployment, run_deployment_with, Context, EngineError, Journal,
    JournalError, Policy, Report, RunConfig,
};
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator, ReleaseId};
use harpoon_deploy::journal_format::{self as fmt, FormatError, Line};
use harpoon_deploy::ledger::{
    Disposition, EndState, EntryState, Ledger, LedgerError, LedgerRecord, Observed, ResourceKey,
    ResourceKind,
};
use harpoon_deploy::ports::{self, Occupancy, Purpose};
use harpoon_deploy::state::{Deployment, DeploymentStatus, Trigger};
use harpoon_deploy::time::{Clock, ManualClock, SeqEntropy};
use harpoon_deploy_journal::{
    read_only, FileJournal, FileWal, OpenError, Wal, JOURNAL_FILE, LOCK_FILE,
};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------ fixtures

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "hdj-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn journal(&self) -> PathBuf {
        self.0.join(JOURNAL_FILE)
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.journal()).unwrap()
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::set_permissions(self.journal(), std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Opening where success is expected tolerates a brief `Locked`: while another test spawns a child
/// process, the child (between fork and exec) shares this process's open file descriptions, and
/// flock belongs to the description. That window is milliseconds; a lock that is genuinely never
/// released still fails after the bound, so it cannot hide a real leak.
fn retry_locked<T>(mut f: impl FnMut() -> Result<T, OpenError>) -> Result<T, OpenError> {
    let start = std::time::Instant::now();
    loop {
        match f() {
            Err(OpenError::Locked(_)) if start.elapsed() < std::time::Duration::from_secs(3) => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            other => return other,
        }
    }
}

fn open_wait(dir: &Path) -> Result<(FileJournal, harpoon_deploy_journal::Restored), OpenError> {
    retry_locked(|| FileJournal::open(dir))
}

fn open_recovered_wait(
    dir: &Path,
    now_ms: u64,
) -> Result<(FileJournal, harpoon_deploy_journal::Restored), OpenError> {
    retry_locked(|| FileJournal::open_recovered(dir, now_ms))
}

fn running_as_root_like(dir: &Path) -> bool {
    // true if file modes do not stop us (root, or a filesystem that ignores them)
    let p = dir.join("probe-mode");
    std::fs::write(&p, "x").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::File::open(&p).is_ok();
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
    let _ = std::fs::remove_file(&p);
    readable
}

struct Fx {
    clock: Rc<ManualClock>,
    gen: IdGenerator<Rc<ManualClock>, SeqEntropy>,
    app: ApplicationId,
    world: World,
}

impl Fx {
    fn new() -> Fx {
        Fx::with_seed(11)
    }
    fn with_seed(seed: u8) -> Fx {
        let clock = ManualClock::new(1_000_000);
        let mut gen = IdGenerator::new(clock.clone(), SeqEntropy::new(seed));
        let app = gen.application().unwrap();
        Fx {
            clock,
            gen,
            app,
            world: FakeRuntime::world(),
        }
    }
    fn deployment(&mut self) -> Deployment {
        let id = self.gen.deployment().unwrap();
        let mut d = Deployment::new(id, self.app.clone(), Trigger::Manual, self.clock.now_ms());
        d.transition(DeploymentStatus::Queued, self.clock.now_ms())
            .unwrap();
        d
    }
    fn previous(&mut self) -> Context {
        let prev: DeploymentId = self.gen.deployment().unwrap();
        Context {
            previous_release: Some(ReleaseId::from_deployment(&prev)),
        }
    }
    fn backend(&self) -> FakeBackend {
        FakeBackend::new(self.clock.clone()).with_runtime(self.world.clone())
    }
    fn bystanders(&mut self) -> Vec<Observed> {
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
    fn labelled(&self, d: &Deployment) -> Vec<Observed> {
        self.world
            .borrow()
            .objects
            .iter()
            .filter(|o| o.labelled_for(&d.application_id, &d.id))
            .cloned()
            .collect()
    }
}

fn run(
    fx: &Fx,
    d: &mut Deployment,
    ctx: &Context,
    b: &mut FakeBackend,
    j: &mut dyn Journal,
) -> Result<Report, EngineError> {
    run_deployment(d, ctx, b, j, &fx.clock, &Policy::default(), &|| false)
}

// ---- fault injection on top of a real file

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    /// Nothing of the line reaches the file.
    Before,
    /// Only the first byte does.
    One,
    /// Half of it.
    Half,
    /// Everything except the newline.
    AllButNewline,
    /// The whole line is written, then the call still reports failure (died before acknowledging).
    Whole,
}
const HITS: [Hit; 5] = [
    Hit::Before,
    Hit::One,
    Hit::Half,
    Hit::AllButNewline,
    Hit::Whole,
];

#[derive(Default)]
struct PlanInner {
    appends: usize,
    syncs: usize,
    truncates: usize,
    fail_append: Option<(usize, Hit)>,
    fail_sync: Option<usize>,
    fail_truncate: bool,
    fail_dir_sync: bool,
    log: Vec<&'static str>,
}
#[derive(Clone, Default)]
struct Plan(Arc<Mutex<PlanInner>>);

impl Plan {
    /// Fail the `n`-th append from now on (0 = the next one).
    fn fail_append_after(&self, n: usize, hit: Hit) {
        let mut p = self.0.lock().unwrap();
        p.fail_append = Some((p.appends + n, hit));
    }
    fn fail_sync_after(&self, n: usize) {
        let mut p = self.0.lock().unwrap();
        p.fail_sync = Some(p.syncs + n);
    }
    fn fail_dir_sync(&self) {
        self.0.lock().unwrap().fail_dir_sync = true;
    }
    fn fail_truncate(&self) {
        self.0.lock().unwrap().fail_truncate = true;
    }
    fn appends(&self) -> usize {
        self.0.lock().unwrap().appends
    }
    fn syncs(&self) -> usize {
        self.0.lock().unwrap().syncs
    }
    fn log(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().log.clone()
    }
}

struct Faulty {
    inner: FileWal,
    plan: Plan,
}

impl Wal for Faulty {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        let hit = {
            let mut p = self.plan.0.lock().unwrap();
            let n = p.appends;
            p.appends += 1;
            p.log.push("append");
            match p.fail_append {
                Some((at, h)) if at == n => Some(h),
                _ => None,
            }
        };
        match hit {
            None => self.inner.append(bytes),
            Some(h) => {
                let k = match h {
                    Hit::Before => 0,
                    Hit::One => 1,
                    Hit::Half => bytes.len() / 2,
                    Hit::AllButNewline => bytes.len() - 1,
                    Hit::Whole => bytes.len(),
                };
                if k > 0 {
                    self.inner.append(&bytes[..k])?;
                }
                Err(io::Error::other("injected append failure"))
            }
        }
    }
    fn sync(&mut self) -> io::Result<()> {
        let fail = {
            let mut p = self.plan.0.lock().unwrap();
            let n = p.syncs;
            p.syncs += 1;
            p.log.push("sync");
            p.fail_sync == Some(n)
        };
        if fail {
            return Err(io::Error::other("injected sync failure"));
        }
        self.inner.sync()
    }
    fn truncate(&mut self, len: u64) -> io::Result<()> {
        let fail = {
            let mut p = self.plan.0.lock().unwrap();
            p.truncates += 1;
            p.log.push("truncate");
            p.fail_truncate
        };
        if fail {
            return Err(io::Error::other("injected truncate failure"));
        }
        self.inner.truncate(len)
    }
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        let fail = {
            let mut p = self.plan.0.lock().unwrap();
            p.log.push("sync_dir");
            p.fail_dir_sync
        };
        if fail {
            return Err(io::Error::other("injected directory sync failure"));
        }
        self.inner.sync_dir(dir)
    }
}

fn open_faulty_with(dir: &Path, plan: Plan) -> Result<FileJournal<Faulty>, OpenError> {
    FileJournal::open_with(dir, move |w| Faulty { inner: w, plan }).map(|(j, _)| j)
}

fn open_faulty(dir: &Path) -> (FileJournal<Faulty>, Plan) {
    let plan = Plan::default();
    let p2 = plan.clone();
    let (j, r) = FileJournal::open_with(dir, move |w| Faulty { inner: w, plan: p2 }).unwrap();
    assert!(r.deployments.is_empty());
    (j, plan)
}

// ------------------------------------------------------------------ creation, durability calls, ordering

#[test]
fn a_new_journal_is_created_synced_and_its_directory_synced_before_use() {
    let t = Tmp::new();
    let (j, r) = open_wait(&t.0).unwrap();
    assert!(
        r.deployments.is_empty() && r.ledger.entries().is_empty() && r.discarded_tail_bytes == 0
    );
    assert_eq!(
        t.bytes(),
        format!("{}\n", fmt::encode_header()).into_bytes()
    );
    assert!(t.0.join(LOCK_FILE).exists());
    let s = j.stats();
    assert_eq!(s.dir_syncs, 1, "directory synced after creating the file");
    assert_eq!(s.file_syncs, 1, "the header was synced");
    drop(j);
    // reopening an existing journal creates nothing and writes nothing
    let before = t.bytes();
    let (j, _) = open_wait(&t.0).unwrap();
    assert_eq!(j.stats().dir_syncs, 0);
    assert_eq!(j.stats().file_syncs, 0);
    drop(j);
    assert_eq!(t.bytes(), before);
}

#[test]
fn every_record_is_appended_whole_then_synced_before_the_next_one_starts() {
    let t = Tmp::new();
    let (mut j, plan) = open_faulty(&t.0);
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend();
    let r = run(
        &fx,
        &mut d,
        &Context {
            previous_release: None,
        },
        &mut b,
        &mut j,
    )
    .unwrap();
    assert_eq!(r.status, DeploymentStatus::Succeeded);
    let all = plan.log();
    assert_eq!(
        all[0], "sync_dir",
        "the directory is synced before the first byte is written: {all:?}"
    );
    let log: Vec<&str> = all.iter().copied().filter(|l| *l != "sync_dir").collect();
    assert!(log.len() > 20);
    // after the header's own append+sync (before the plan was armed it is counted too)
    for pair in log.chunks(2) {
        assert_eq!(
            pair,
            ["append", "sync"],
            "append and sync strictly alternate: {log:?}"
        );
    }
    assert_eq!(plan.appends(), plan.syncs());
    assert_eq!(j.stats().appends as usize, plan.appends());
    // the file ends on a record boundary and parses
    let rec = fmt::recover(&t.bytes()).unwrap();
    assert_eq!(rec.tail, fmt::Tail::Clean);
    assert_eq!(rec.valid_len, t.bytes().len());
    assert_eq!(j.len() as usize, t.bytes().len());
}

#[test]
fn the_journal_is_written_before_each_side_effect_not_after() {
    // Every object that exists in the runtime when the run is cut short already has its intent
    // in the file — the write-ahead property, observed through the real file.
    let mut fx0 = Fx::new();
    let total = {
        let t = Tmp::new();
        let (mut j, plan) = open_faulty(&t.0);
        let mut d = fx0.deployment();
        let mut b = fx0.backend();
        run(
            &fx0,
            &mut d,
            &Context {
                previous_release: None,
            },
            &mut b,
            &mut j,
        )
        .unwrap();
        plan.appends() - 1
    };
    for n in 0..total {
        let t = Tmp::new();
        let (mut j, plan) = open_faulty(&t.0);
        plan.fail_append_after(n, Hit::Before);
        let mut fx = Fx::new();
        let mut d = fx.deployment();
        let mut b = fx.backend();
        assert!(run(
            &fx,
            &mut d,
            &Context {
                previous_release: None
            },
            &mut b,
            &mut j
        )
        .is_err());
        drop(j);
        let st = read_only(&t.0).unwrap();
        for o in fx.labelled(&d) {
            assert!(
                st.ledger
                    .entries()
                    .iter()
                    .any(|e| e.key.kind == o.kind && e.key.name == o.name),
                "write {n}: {} {} exists in the runtime without a durable intent",
                o.kind.as_str(),
                o.name
            );
        }
    }
}

// ------------------------------------------------------------------ crashes at every persistence boundary

type Script = fn(FakeBackend) -> FakeBackend;

fn scenarios() -> Vec<(&'static str, Script, bool)> {
    vec![
        ("success, first deployment", |b| b, false),
        ("success, with a previous release", |b| b, true),
        ("preliminary check fails", |b| b.fail_preliminary("x"), true),
        (
            "final check fails, restored",
            |b| b.fail_final_check("x"),
            true,
        ),
        (
            "first deployment, final check fails",
            |b| b.fail_final_check("x"),
            false,
        ),
    ]
}

fn appended_in_full_run(script: Script, has_prev: bool) -> usize {
    let t = Tmp::new();
    let (mut j, plan) = open_faulty(&t.0);
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let ctx = if has_prev {
        fx.previous()
    } else {
        Context {
            previous_release: None,
        }
    };
    let mut b = script(fx.backend());
    run(&fx, &mut d, &ctx, &mut b, &mut j).unwrap();
    plan.appends() - 1 // the header
}

#[test]
fn a_crash_or_torn_write_at_every_record_boundary_recovers_conservatively_and_idempotently() {
    let mut cases = 0;
    let mut adopted_somewhere = false;
    for (name, script, has_prev) in scenarios() {
        let total = appended_in_full_run(script, has_prev);
        for n in 0..total {
            for hit in HITS {
                cases += 1;
                let t = Tmp::new();
                let (mut j, plan) = open_faulty(&t.0);
                plan.fail_append_after(n, hit);
                if !matches!(hit, Hit::Before | Hit::Whole) {
                    plan.fail_truncate(); // the process dies; it never gets to tidy its own tail
                }
                let mut fx = Fx::new();
                let bystanders = fx.bystanders();
                let mut d = fx.deployment();
                let ctx = if has_prev {
                    fx.previous()
                } else {
                    Context {
                        previous_release: None,
                    }
                };
                let mut b = script(fx.backend());
                let res = run(&fx, &mut d, &ctx, &mut b, &mut j);
                let tag = format!("{name} @{n} {hit:?}");
                assert!(
                    matches!(res, Err(EngineError::Journal(JournalError::Io(_)))),
                    "{tag}: {res:?}"
                );
                assert!(j.broken().is_some(), "{tag}");
                let world_at_death = fx.world.borrow().objects.clone();
                drop(j); // the process is gone; its lock goes with it

                // ---- restart
                let (mut j2, restored) = open_recovered_wait(&t.0, fx.clock.now_ms()).unwrap();
                if hit == Hit::Before || hit == Hit::Whole {
                    assert_eq!(restored.discarded_tail_bytes, 0, "{tag}");
                } else {
                    assert!(
                        restored.discarded_tail_bytes > 0,
                        "{tag}: a torn tail is cut off"
                    );
                }
                assert_eq!(
                    fx.world.borrow().objects,
                    world_at_death,
                    "{tag}: opening the journal touched the runtime"
                );
                for dd in &restored.deployments {
                    assert!(dd.status.is_terminal(), "{tag}: {} is {}", dd.id, dd.status);
                    assert!(
                        matches!(
                            dd.status,
                            DeploymentStatus::Interrupted
                                | DeploymentStatus::Failed
                                | DeploymentStatus::Succeeded
                        ),
                        "{tag}"
                    );
                }
                // a deployment cut short before its first durable snapshot leaves no ledger entry either
                if restored.deployments.is_empty() {
                    assert!(restored.ledger.entries().is_empty(), "{tag}");
                }
                // nothing was removed or adopted by opening: no cleanup records, no new receipts
                let lines = harpoon_deploy_journal::lines(&t.0).unwrap();
                let ledger_records_before: Vec<_> = lines
                    .iter()
                    .filter_map(|l| {
                        if let Line::Ledger(r) = l {
                            Some(r.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                // ---- explicit, verified reconciliation (a separate step the caller chooses to run)
                let mut ledger = restored.ledger.clone();
                let ended: Vec<(DeploymentId, EndState)> = restored
                    .deployments
                    .iter()
                    .filter_map(|x| end_state(x.status).map(|e| (x.id.clone(), e)))
                    .collect();
                let mut backend = fx.backend();
                reconcile(&mut ledger, &ended, &mut j2, &mut backend, &fx.clock).unwrap();
                adopted_somewhere |= ledger_records_before.len()
                    < harpoon_deploy_journal::lines(&t.0).unwrap().len();

                for e in ledger.entries() {
                    assert!(
                        !matches!(e.state, EntryState::Intended),
                        "{tag}: {} still an unresolved intent",
                        e.key.name
                    );
                    if let Some((_, end)) = ended.iter().find(|(id, _)| *id == e.deployment) {
                        let keep = e.disposition == Disposition::Retain
                            || (e.disposition == Disposition::RemoveOnFailure
                                && *end == EndState::Succeeded);
                        if !keep {
                            assert!(
                                e.is_released(),
                                "{tag}: {} should be released, is {:?}",
                                e.key.name,
                                e.state
                            );
                        }
                    }
                }
                for o in fx.labelled(&d) {
                    let e = ledger
                        .entries()
                        .iter()
                        .find(|e| e.id.as_deref() == Some(o.id.as_str()))
                        .unwrap_or_else(|| {
                            panic!("{tag}: {} survived without a ledger entry", o.name)
                        });
                    assert!(
                        e.disposition == Disposition::Retain
                            || e.disposition == Disposition::RemoveOnFailure,
                        "{tag}"
                    );
                }
                for by in &bystanders {
                    assert!(
                        fx.world.borrow().objects.contains(by),
                        "{tag}: touched {}",
                        by.name
                    );
                }
                // what the file now says is exactly what the process believes
                let on_disk = read_only(&t.0).unwrap();
                assert_eq!(on_disk.ledger, ledger, "{tag}");
                drop(j2);

                // ---- idempotent: another restart changes nothing at all
                let bytes = t.bytes();
                let (mut j3, again) = open_recovered_wait(&t.0, fx.clock.now_ms() + 5_000).unwrap();
                assert!(
                    again.startup.interrupted.is_empty(),
                    "{tag}: interrupted marked twice"
                );
                assert_eq!(again.discarded_tail_bytes, 0, "{tag}");
                let mut l2 = again.ledger.clone();
                reconcile(&mut l2, &ended, &mut j3, &mut fx.backend(), &fx.clock).unwrap();
                drop(j3);
                assert_eq!(
                    t.bytes(),
                    bytes,
                    "{tag}: a repeated recovery wrote something"
                );
            }
        }
    }
    assert!(
        adopted_somewhere,
        "some crash point needed a reconciliation write"
    );
    assert!(cases > 300, "{cases} crash points exercised");
}

#[test]
fn recovery_marks_an_in_flight_deployment_interrupted_once_and_flags_a_begun_cutover() {
    let mut fx = Fx::new();
    let ctx = fx.previous();
    // find a write inside the cutover of a replacement deployment
    let total = appended_in_full_run(|b| b, true);
    let mut seen_begun = false;
    for n in 0..total {
        let t = Tmp::new();
        let (mut j, plan) = open_faulty(&t.0);
        plan.fail_append_after(n, Hit::Before);
        let mut fx2 = Fx::new();
        let mut d = fx2.deployment();
        let ctx2 = fx2.previous();
        let mut b = fx2.backend();
        let _ = run(&fx2, &mut d, &ctx2, &mut b, &mut j);
        drop(j);
        let world = fx2.world.borrow().objects.clone();
        let (j2, r) = open_recovered_wait(&t.0, 9_999_999).unwrap();
        drop(j2);
        if let Some(dd) = r.deployments.first() {
            if dd.status == DeploymentStatus::Interrupted {
                assert!(r.startup.interrupted.contains(&dd.id));
                seen_begun |= r.startup.incomplete_cutovers.contains(&dd.id);
                assert!(dd.finished_ms == Some(9_999_999));
            }
        }
        assert_eq!(fx2.world.borrow().objects, world);
    }
    assert!(
        seen_begun,
        "some crash point left a begun, unconfirmed cutover and said so"
    );
    let _ = ctx;
}

// ------------------------------------------------------------------ restoring what a restart needs

#[test]
fn status_cleanup_failures_ids_and_leases_survive_a_restart_exactly() {
    let t = Tmp::new();
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let serving = format!("hp-{}", d.id);
    let mut b = fx
        .backend()
        .fail_final_check("HTTP 500")
        .fail_remove(&serving);
    let report = {
        let (mut j, _) = open_wait(&t.0).unwrap();
        run(
            &fx,
            &mut d,
            &Context {
                previous_release: None,
            },
            &mut b,
            &mut j,
        )
        .unwrap()
    };
    assert_eq!(report.status, DeploymentStatus::Failed);
    assert!(!report.cleanup_problems().is_empty());

    let r = read_only(&t.0).unwrap();
    assert_eq!(
        r.deployments,
        vec![d.clone()],
        "the final snapshot is restored field for field"
    );
    assert_eq!(r.deployment(d.id.as_str()).unwrap().failure, d.failure);
    assert_eq!(r.ledger, report.ledger);
    let problems = r.cleanup_problems();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].key.name, serving);
    assert!(
        matches!(&problems[0].state, EntryState::CleanupFailed(m) if m.contains("could not remove"))
    );
    let created = r
        .ledger
        .entries()
        .iter()
        .find(|e| e.key.name == serving)
        .unwrap();
    assert!(
        created.id.as_deref().unwrap().starts_with("fake-"),
        "the runtime's own id is kept"
    );
    assert_eq!(r.outstanding_leases(), report.ledger.leased_ports());
    assert!(
        !r.outstanding_leases().is_empty(),
        "the lease remains held while the container cleanup is unresolved"
    );

    // and through a writer's open as well, with nothing newly interrupted
    let (_j, w) = open_recovered_wait(&t.0, fx.clock.now_ms()).unwrap();
    assert_eq!(w.deployments, r.deployments);
    assert!(w.startup.interrupted.is_empty());
}

#[test]
fn outstanding_leases_block_the_same_port_after_a_restart_until_a_verified_release() {
    let t = Tmp::new();
    let mut fx = Fx::new();
    let mut a = fx.deployment();
    let serving = format!("hp-{}", a.id);
    // deployment A dies right after its container and lease exist
    let n = {
        let t2 = Tmp::new();
        let (mut j, plan) = open_faulty(&t2.0);
        let mut fx2 = Fx::new();
        let mut d = fx2.deployment();
        let mut b = fx2.backend();
        run(
            &fx2,
            &mut d,
            &Context {
                previous_release: None,
            },
            &mut b,
            &mut j,
        )
        .unwrap();
        plan.appends() - 1
    };
    let mut leased = None;
    for k in 0..n {
        let dir = Tmp::new();
        let (mut j, plan) = open_faulty(&dir.0);
        plan.fail_append_after(k, Hit::Before);
        let mut fx3 = Fx::new();
        let mut d = fx3.deployment();
        let mut b = fx3.backend();
        let _ = run(
            &fx3,
            &mut d,
            &Context {
                previous_release: None,
            },
            &mut b,
            &mut j,
        );
        drop(j);
        let r = read_only(&dir.0).unwrap();
        if !r.outstanding_leases().is_empty() {
            leased = Some((dir, r, d));
            break;
        }
    }
    let (dir, restored, dead) = leased.expect("a crash point holds a lease");
    let port = restored.outstanding_leases()[0];

    // B starts after the restart, from the restored ledger
    let (mut j, r2) = open_recovered_wait(&dir.0, 5_000_000).unwrap();
    assert_eq!(
        r2.outstanding_leases(),
        vec![port],
        "recovery does not release anything by itself"
    );
    let occ = Occupancy::default().with_ledger(&r2.ledger);
    let other = ports::allocate(Purpose::Test, 8080, &occ, None, &[]).unwrap();
    assert_ne!(other.host_port, port, "allocation sees the journaled lease");

    // even if a backend ignored the occupancy and asked for the same port, the ledger refuses
    let mut fx4 = Fx::with_seed(77);
    let b_dep = fx4.deployment();
    assert_ne!(b_dep.id, dead.id);
    let mut ledger = r2.ledger.clone();
    let intent = LedgerRecord::Intent {
        key: ResourceKey::new(ResourceKind::PortLease, port.to_string()),
        app: b_dep.application_id.clone(),
        deployment: b_dep.id.clone(),
        disposition: Disposition::RemoveOnFailure,
        at_ms: 1,
    };
    assert!(
        matches!(ledger.apply(&intent), Err(LedgerError::LeaseHeld { holder, .. }) if holder == dead.id)
    );

    // a full run that starts from the restored ledger is given a different port
    let mut backend = fx4.backend();
    let mut b_dep2 = b_dep.clone();
    let cfg = RunConfig {
        prior_ledger: r2.ledger.clone(),
        ..RunConfig::default()
    };
    let rep = run_deployment_with(
        &mut b_dep2,
        &Context {
            previous_release: None,
        },
        &mut backend,
        &mut j,
        &fx4.clock,
        &cfg,
        &|| false,
    )
    .unwrap();
    assert_eq!(rep.status, DeploymentStatus::Succeeded);
    let leases_now: Vec<u16> = rep.ledger.leased_ports();
    assert!(
        leases_now.contains(&port),
        "the dead deployment's lease is still held"
    );
    assert!(leases_now.len() >= 2);
    drop(j);

    // the dead deployment's entries are reconciled explicitly; only then is the port free again
    let (mut j, r3) = open_wait(&dir.0).unwrap();
    let mut ledger = r3.ledger.clone();
    let ended = [(dead.id.clone(), EndState::NotSucceeded)];
    reconcile(&mut ledger, &ended, &mut j, &mut fx4.backend(), &fx4.clock).unwrap();
    assert!(!ledger.leased_ports().contains(&port));
    let _ = (t, a.id.clone(), serving, &mut a);
}

// ------------------------------------------------------------------ torn tails and corruption on disk

fn full_journal(t: &Tmp) -> Vec<u8> {
    let (mut j, _) = open_wait(&t.0).unwrap();
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend();
    run(
        &fx,
        &mut d,
        &Context {
            previous_release: None,
        },
        &mut b,
        &mut j,
    )
    .unwrap();
    drop(j);
    t.bytes()
}

fn boundaries(bytes: &[u8]) -> Vec<usize> {
    let mut v = vec![0];
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

#[test]
fn a_journal_cut_at_any_byte_opens_with_exactly_its_whole_records_and_stays_appendable() {
    let source = Tmp::new();
    let full = full_journal(&source);
    let bounds = boundaries(&full);
    let mut cuts: Vec<usize> = Vec::new();
    for b in &bounds {
        cuts.extend([b.saturating_sub(1), *b, b + 1]);
    }
    // every byte inside the last two records, where a real crash is likeliest
    let from = bounds[bounds.len() - 3];
    cuts.extend(from..=full.len());
    cuts.retain(|c| *c <= full.len());
    cuts.sort();
    cuts.dedup();
    for cut in cuts {
        let t = Tmp::new();
        std::fs::write(t.journal(), &full[..cut]).unwrap();
        let whole = bounds.iter().rposition(|b| *b <= cut).unwrap();
        let expect_len = bounds[whole];
        let pure = fmt::recover(&full[..cut]).unwrap();
        let expect = fmt::replay(&pure.lines).unwrap();
        let (mut j, r) = open_wait(&t.0).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
        assert_eq!(r.deployments, expect.deployments, "cut {cut}");
        assert_eq!(r.ledger, expect.ledger, "cut {cut}");
        assert_eq!(r.discarded_tail_bytes, cut - expect_len, "cut {cut}");
        // the torn bytes are gone from the file before anything is appended after them
        let want_len = if expect_len == 0 {
            fmt::encode_header().len() + 1
        } else {
            expect_len
        };
        assert_eq!(t.bytes().len(), want_len, "cut {cut}");
        // and the next write is not glued to a fragment
        let mut fx = Fx::new();
        let d = fx.deployment();
        j.persist(&d).unwrap();
        drop(j);
        let rec = fmt::recover(&t.bytes()).unwrap();
        assert_eq!(rec.tail, fmt::Tail::Clean, "cut {cut}");
        assert_eq!(rec.lines.len(), whole.saturating_sub(1) + 1, "cut {cut}");
    }
}

#[test]
fn read_only_inspection_ignores_a_torn_tail_without_repairing_it() {
    let t = Tmp::new();
    let full = full_journal(&t);
    let cut = full.len() - 7;
    std::fs::write(t.journal(), &full[..cut]).unwrap();
    let r = read_only(&t.0).unwrap();
    assert!(r.discarded_tail_bytes > 0);
    assert_eq!(t.bytes(), full[..cut].to_vec(), "read-only never writes");
}

#[test]
fn a_damaged_committed_record_stops_recovery_and_changes_nothing() {
    let t = Tmp::new();
    let full = full_journal(&t);
    let bounds = boundaries(&full);
    for rec in [1usize, bounds.len() / 2, bounds.len() - 2] {
        let mut f = full.clone();
        f[bounds[rec] + 10] ^= 0x04;
        std::fs::write(t.journal(), &f).unwrap();
        let before = std::fs::read(t.journal()).unwrap();
        for _ in 0..2 {
            // twice: the failed open must not leave the lock held
            match open_wait(&t.0) {
                Err(OpenError::Format(FormatError::Corrupt { line, .. })) => {
                    assert_eq!(line, rec + 1)
                }
                other => panic!("record {rec}: {:?}", other.map(|_| ())),
            }
            assert!(matches!(read_only(&t.0), Err(OpenError::Format(_))));
        }
        assert_eq!(
            std::fs::read(t.journal()).unwrap(),
            before,
            "a corrupt journal is left exactly as found"
        );
    }
    // damage in the very last committed record is corruption too, not a torn write
    let mut f = full.clone();
    let n = f.len();
    f[n - 5] ^= 0x01;
    std::fs::write(t.journal(), &f).unwrap();
    assert!(matches!(
        open_wait(&t.0),
        Err(OpenError::Format(FormatError::Corrupt { .. }))
    ));
}

#[test]
fn files_that_are_not_this_journal_are_never_modified() {
    let t = Tmp::new();
    // an unterminated foreign file must not be "recovered" by truncation
    std::fs::write(t.journal(), b"this is somebody's notes").unwrap();
    assert!(matches!(open_wait(&t.0), Err(OpenError::NotAJournal(_))));
    assert_eq!(t.bytes(), b"this is somebody's notes");
    // a terminated foreign line
    std::fs::write(t.journal(), b"hello\n").unwrap();
    assert!(matches!(
        open_wait(&t.0),
        Err(OpenError::Format(FormatError::BadHeader(_)))
    ));
    assert_eq!(t.bytes(), b"hello\n");
    // an empty file and a torn header are both just a journal that never got its header
    std::fs::write(t.journal(), b"").unwrap();
    open_wait(&t.0).unwrap();
    assert_eq!(
        t.bytes(),
        format!("{}\n", fmt::encode_header()).into_bytes()
    );
    let h = fmt::encode_header();
    std::fs::write(t.journal(), &h.as_bytes()[..7]).unwrap();
    let (_j, r) = open_wait(&t.0).unwrap();
    assert_eq!(r.discarded_tail_bytes, 7);
    assert_eq!(t.bytes(), format!("{h}\n").into_bytes());
}

#[test]
fn a_journal_of_an_unknown_version_is_refused_untouched() {
    let t = Tmp::new();
    let line = format!(
        "{}\n",
        fmt::frame(&format!(
            "header\tharpoon-journal\t{}",
            fmt::FILE_VERSION + 1
        ))
    );
    std::fs::write(t.journal(), &line).unwrap();
    assert!(matches!(
        open_wait(&t.0),
        Err(OpenError::Format(FormatError::UnsupportedVersion(v))) if v == fmt::FILE_VERSION + 1
    ));
    assert_eq!(t.bytes(), line.into_bytes());
}

#[test]
fn contradictory_records_are_refused_untouched() {
    let t = Tmp::new();
    full_journal(&t);
    // append a receipt for something never intended, correctly framed
    let mut fx = Fx::new();
    let d = fx.deployment();
    let r = LedgerRecord::Receipt {
        key: ResourceKey::new(ResourceKind::Container, "ghost"),
        deployment: d.id.clone(),
        id: "x".into(),
        at_ms: 1,
    };
    let mut f = t.bytes();
    f.extend_from_slice(fmt::encode_ledger(&r).as_bytes());
    f.push(b'\n');
    std::fs::write(t.journal(), &f).unwrap();
    assert!(matches!(
        open_wait(&t.0),
        Err(OpenError::Format(FormatError::Inconsistent(_)))
    ));
    assert_eq!(t.bytes(), f);
}

// ------------------------------------------------------------------ failed writes

fn one_deployment_through_the_engine(
    j: &mut dyn Journal,
) -> (Fx, Deployment, Result<Report, EngineError>) {
    let mut fx = Fx::new();
    let mut d = fx.deployment();
    let mut b = fx.backend();
    let r = run(
        &fx,
        &mut d,
        &Context {
            previous_release: None,
        },
        &mut b,
        j,
    );
    (fx, d, r)
}

#[test]
fn a_failed_append_stops_the_engine_and_no_further_write_is_attempted() {
    let t = Tmp::new();
    let (mut j, plan) = open_faulty(&t.0);
    plan.fail_append_after(5, Hit::Before);
    let (fx, _, r) = one_deployment_through_the_engine(&mut j);
    assert!(matches!(r, Err(EngineError::Journal(JournalError::Io(m))) if m.contains("append")));
    let frozen = plan.appends();
    assert_eq!(
        frozen,
        1 + 6,
        "header, five good records, the failing one — then nothing"
    );
    // the journal refuses everything afterwards without touching the file
    let mut other = Fx::new();
    let d2 = other.deployment();
    let e = j.persist(&d2).unwrap_err();
    assert!(
        matches!(&e, JournalError::Io(m) if m.contains("unusable")),
        "{e:?}"
    );
    assert_eq!(plan.appends(), frozen, "a broken journal does not even try");
    assert!(j.broken().unwrap().contains("append failed"));
    // objects created before the failure are intents-first: the world holds at most what the file knows
    let st = read_only(&t.0).unwrap();
    for o in fx.world.borrow().objects.iter() {
        assert!(st.ledger.entries().iter().any(|e| e.key.name == o.name));
    }
}

#[test]
fn a_partial_write_is_cut_back_when_possible_and_by_the_next_open_when_not() {
    for (truncate_works, hit) in [
        (true, Hit::Half),
        (true, Hit::AllButNewline),
        (false, Hit::Half),
        (false, Hit::One),
    ] {
        let t = Tmp::new();
        let (mut j, plan) = open_faulty(&t.0);
        let mut fx = Fx::new();
        let d = fx.deployment();
        j.persist(&d).unwrap();
        let good = t.bytes();
        if !truncate_works {
            plan.fail_truncate();
        }
        plan.fail_append_after(0, hit);
        let mut d2 = d.clone();
        d2.transition(DeploymentStatus::Cancelled, 5).unwrap();
        assert!(j.persist(&d2).is_err());
        if truncate_works {
            assert_eq!(t.bytes(), good, "{hit:?}: the fragment was cut off at once");
            assert!(!j.broken().unwrap().contains("not cleaned"));
        } else {
            assert!(
                t.bytes().len() > good.len(),
                "{hit:?}: the fragment is still there"
            );
            assert!(j.broken().unwrap().contains("tail not cleaned"));
        }
        drop(j);
        let (_j, r) = open_wait(&t.0).unwrap();
        assert_eq!(
            r.deployments,
            vec![d.clone()],
            "{hit:?}: only the acknowledged snapshot"
        );
        assert_eq!(t.bytes(), good, "{hit:?}");
    }
}

#[test]
fn a_failed_sync_poisons_the_journal_is_never_retried_and_the_record_may_or_may_not_be_there() {
    let t = Tmp::new();
    let (mut j, plan) = open_faulty(&t.0);
    let mut fx = Fx::new();
    let d = fx.deployment();
    j.persist(&d).unwrap();
    plan.fail_sync_after(0);
    let mut d2 = d.clone();
    d2.transition(DeploymentStatus::Cancelled, 5).unwrap();
    let before_syncs = plan.syncs();
    assert!(matches!(j.persist(&d2), Err(JournalError::Io(m)) if m.contains("sync")));
    assert_eq!(
        plan.syncs(),
        before_syncs + 1,
        "the failed fsync was not retried"
    );
    assert!(j.persist(&d2).is_err());
    assert_eq!(
        plan.syncs(),
        before_syncs + 1,
        "and nothing else syncs afterwards"
    );
    drop(j);
    // the data did reach the file in this run, so recovery sees it: the caller learned only that
    // it was NOT acknowledged, and the engine had stopped
    let (_j, r) = open_wait(&t.0).unwrap();
    assert_eq!(r.deployments[0].status, DeploymentStatus::Cancelled);
}

#[test]
fn a_record_that_was_written_but_not_acknowledged_is_not_acted_on() {
    // The append reaches the file but reports failure (the process "died" before the ack). The
    // engine must not have used it: nothing that depends on it exists in the runtime.
    let mut stopped = 0;
    for n in 0..60 {
        let t = Tmp::new();
        let (mut j, plan) = open_faulty(&t.0);
        plan.fail_append_after(n, Hit::Whole);
        plan.fail_truncate(); // death: no chance to take the unacknowledged line back
        let (fx, d, r) = one_deployment_through_the_engine(&mut j);
        if r.is_ok() {
            break; // past the last write
        }
        stopped += 1;
        let st = read_only(&t.0).unwrap();
        for o in fx.labelled(&d) {
            assert!(
                st.ledger.entries().iter().any(|e| e.key.name == o.name),
                "write {n}: {}",
                o.name
            );
        }
        let in_file = harpoon_deploy_journal::lines(&t.0).unwrap().len();
        assert_eq!(in_file, plan.appends() - 1, "write {n}");
    }
    assert_eq!(
        stopped,
        appended_in_full_run(|b| b, false),
        "every write point was exercised"
    );
}

// ------------------------------------------------------------------ permissions

#[test]
fn permission_errors_are_reported_with_the_path_and_leave_everything_as_it_was() {
    let probe = Tmp::new();
    if running_as_root_like(&probe.0) {
        eprintln!("file modes are not enforced here: permission cases skipped");
        return;
    }
    // a directory that does not exist
    let missing = probe.0.join("nope");
    assert!(
        matches!(open_wait(&missing), Err(OpenError::Io { op, .. }) if op.contains("lock file"))
    );

    // a read-only directory: nothing can be created, and nothing partial is left behind
    let ro = Tmp::new();
    std::fs::set_permissions(&ro.0, std::fs::Permissions::from_mode(0o555)).unwrap();
    match open_wait(&ro.0) {
        Err(OpenError::Io { error, path, .. }) => {
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert!(path.starts_with(&ro.0));
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert_eq!(std::fs::read_dir(&ro.0).unwrap().count(), 0);
    std::fs::set_permissions(&ro.0, std::fs::Permissions::from_mode(0o755)).unwrap();

    // an existing journal that cannot be written
    let t = Tmp::new();
    full_journal(&t);
    let before = t.bytes();
    std::fs::set_permissions(t.journal(), std::fs::Permissions::from_mode(0o444)).unwrap();
    match open_wait(&t.0) {
        Err(OpenError::Io { op, error, .. }) => {
            assert!(op.contains("appending"), "{op}");
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert_eq!(t.bytes(), before);
    // read-only inspection still works on it
    assert!(read_only(&t.0).is_ok());
    // the failed open did not keep the lock
    std::fs::set_permissions(t.journal(), std::fs::Permissions::from_mode(0o644)).unwrap();
    open_wait(&t.0).unwrap();

    // a journal that cannot be read
    std::fs::set_permissions(t.journal(), std::fs::Permissions::from_mode(0o200)).unwrap();
    assert!(matches!(open_wait(&t.0), Err(OpenError::Io { op, .. }) if op.contains("read")));
    assert!(matches!(read_only(&t.0), Err(OpenError::Io { .. })));
    std::fs::set_permissions(t.journal(), std::fs::Permissions::from_mode(0o644)).unwrap();

    // a torn tail that cannot be cut off (file turned read-only mid-way) is an error, not a silent skip
    let cut = before.len() - 3;
    std::fs::write(t.journal(), &before[..cut]).unwrap();
    std::fs::set_permissions(t.journal(), std::fs::Permissions::from_mode(0o444)).unwrap();
    assert!(open_wait(&t.0).is_err());
    assert_eq!(t.bytes(), before[..cut].to_vec());
}

// ------------------------------------------------------------------ locking and concurrency

#[test]
fn a_second_writer_is_refused_at_once_and_does_not_disturb_the_first() {
    let t = Tmp::new();
    let (mut a, _) = FileJournal::open(&t.0).unwrap();
    let before = t.bytes();
    for _ in 0..3 {
        match FileJournal::open(&t.0) {
            Err(OpenError::Locked(p)) => assert!(p.ends_with(LOCK_FILE)),
            other => panic!("{:?}", other.map(|_| ())),
        }
        assert!(matches!(
            FileJournal::open_recovered(&t.0, 1),
            Err(OpenError::Locked(_))
        ));
    }
    assert_eq!(
        t.bytes(),
        before,
        "a refused open writes nothing, not even a header or a truncation"
    );
    // the holder carries on, and readers can look without a lock
    let mut fx = Fx::new();
    let d = fx.deployment();
    a.persist(&d).unwrap();
    assert_eq!(read_only(&t.0).unwrap().deployments, vec![d.clone()]);
    drop(a);
    // Success is expected here, so the documented fork window is tolerated (see `open_wait`): the
    // refusals above stay strict. Another test's forked-but-not-yet-exec'd child can briefly hold a
    // copy of this lock, notably the parked child of
    // `a_child_forked_while_the_journal_is_open_keeps_the_lock_until_it_execs`.
    let (_b, r) = open_wait(&t.0).unwrap();
    assert_eq!(r.deployments, vec![d]);
}

#[test]
fn many_threads_taking_turns_never_interleave_or_lose_a_record() {
    let t = Tmp::new();
    let dir = Arc::new(t.0.clone());
    let contended = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..6)
        .map(|n| {
            let dir = dir.clone();
            let contended = contended.clone();
            std::thread::spawn(move || {
                let mut gen =
                    IdGenerator::new(ManualClock::new(5_000 + n), SeqEntropy::new(n as u8 + 40));
                let app = gen.application().unwrap();
                let dep = gen.deployment().unwrap();
                for i in 0..15 {
                    loop {
                        match FileJournal::open(&dir) {
                            Ok((mut j, _)) => {
                                let r = LedgerRecord::Intent {
                                    key: ResourceKey::new(
                                        ResourceKind::Container,
                                        format!("t{n}-{i}"),
                                    ),
                                    app: app.clone(),
                                    deployment: dep.clone(),
                                    disposition: Disposition::Retain,
                                    at_ms: i,
                                };
                                j.record(&r).unwrap();
                                break;
                            }
                            Err(OpenError::Locked(_)) => {
                                contended.fetch_add(1, Ordering::SeqCst);
                                std::thread::sleep(std::time::Duration::from_millis(1));
                            }
                            Err(e) => panic!("{e}"),
                        }
                    }
                }
            })
        })
        .collect();
    for th in threads {
        th.join().unwrap();
    }
    let r = read_only(&t.0).unwrap();
    assert_eq!(
        r.ledger.entries().len(),
        6 * 15,
        "every record exactly once"
    );
    let rec = fmt::recover(&t.bytes()).unwrap();
    assert_eq!(rec.tail, fmt::Tail::Clean);
    for n in 0..6 {
        let mine: Vec<u64> = r
            .ledger
            .entries()
            .iter()
            .filter(|e| e.key.name.starts_with(&format!("t{n}-")))
            .map(|e| e.intent_ms)
            .collect();
        assert_eq!(
            mine,
            (0..15).collect::<Vec<_>>(),
            "thread {n}'s records are in order"
        );
    }
    eprintln!(
        "lock contention observed {} times",
        contended.load(Ordering::SeqCst)
    );
}

/// Re-run this test binary as a child process playing one role (see `real_processes_*`).
fn child(role: &str, dir: &Path) -> std::process::ExitStatus {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_role", "--nocapture", "--test-threads=1"])
        .env("HARPOON_B5_ROLE", role)
        .env("HARPOON_B5_DIR", dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
}

/// Not a test of its own: the body that the child processes of `real_processes_*` run.
#[test]
fn child_role() {
    let Ok(role) = std::env::var("HARPOON_B5_ROLE") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("HARPOON_B5_DIR").unwrap());
    match role.as_str() {
        "expect_locked" => match FileJournal::open(&dir) {
            Err(OpenError::Locked(_)) => {}
            _ => std::process::exit(3),
        },
        "write_then_die" => {
            let (mut j, _) = FileJournal::open(&dir).unwrap();
            let mut fx = Fx::new();
            let mut d = fx.deployment();
            let mut b = fx.backend();
            // run until the process kills itself in the middle of the cutover
            let mut dying = Dying {
                inner: &mut j,
                left: 14,
            };
            let _ = run(
                &fx,
                &mut d,
                &Context {
                    previous_release: None,
                },
                &mut b,
                &mut dying,
            );
            std::process::exit(4);
        }
        _ => std::process::exit(5),
    }
}

/// Aborts the process (no destructors, lock still held) after `left` acknowledged writes.
struct Dying<'a> {
    inner: &'a mut FileJournal,
    left: usize,
}
impl Journal for Dying<'_> {
    fn persist(&mut self, d: &Deployment) -> Result<(), JournalError> {
        self.inner.persist(d)?;
        self.tick();
        Ok(())
    }
    fn record(&mut self, r: &LedgerRecord) -> Result<(), JournalError> {
        self.inner.record(r)?;
        self.tick();
        Ok(())
    }
}
impl Dying<'_> {
    fn tick(&mut self) {
        self.left -= 1;
        if self.left == 0 {
            std::process::abort();
        }
    }
}

#[test]
fn real_processes_contend_for_the_lock_and_a_killed_holder_leaves_no_stale_lock() {
    let t = Tmp::new();
    {
        let (_held, _) = FileJournal::open(&t.0).unwrap();
        assert!(
            child("expect_locked", &t.0).success(),
            "another process is refused while we hold it"
        );
    }
    // a process that dies (SIGABRT, destructors not run) while holding the lock after 14 acknowledged writes
    let status = child("write_then_die", &t.0);
    assert!(!status.success());
    // the very next open succeeds: the kernel released the lock, no cleanup step exists
    let (_j, r) = FileJournal::open_recovered(&t.0, 77).unwrap();
    assert_eq!(
        r.discarded_tail_bytes, 0,
        "every record it acknowledged is complete"
    );
    assert_eq!(r.deployments.len(), 1);
    assert_eq!(r.deployments[0].status, DeploymentStatus::Interrupted);
    assert!(
        !r.ledger.entries().is_empty(),
        "its ledger records were durable"
    );
    assert_eq!(r.startup.interrupted.len(), 1);
}

#[test]
fn restoring_from_the_journal_gives_the_same_answer_as_the_pure_replay_of_its_bytes() {
    let t = Tmp::new();
    let bytes = full_journal(&t);
    let pure = fmt::replay(&fmt::recover(&bytes).unwrap().lines).unwrap();
    let r = read_only(&t.0).unwrap();
    assert_eq!(r.deployments, pure.deployments);
    assert_eq!(r.ledger, pure.ledger);
    let _ = Ledger::new();
}

#[test]
fn the_directory_is_synced_before_the_header_and_a_failed_directory_sync_writes_nothing() {
    let t = Tmp::new();
    let plan = Plan::default();
    let _j = open_faulty_with(&t.0, plan.clone()).unwrap();
    assert_eq!(plan.log(), ["sync_dir", "append", "sync"]);

    let t = Tmp::new();
    let plan = Plan::default();
    plan.fail_dir_sync();
    match open_faulty_with(&t.0, plan.clone()) {
        Err(OpenError::Io { op, .. }) => assert!(op.contains("sync the directory")),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert_eq!(
        plan.log(),
        ["sync_dir"],
        "nothing was appended after the failure"
    );
    assert_eq!(t.bytes(), b"", "the file is still empty");
    // a later open tries the directory sync again, because the file never got its header
    let plan = Plan::default();
    let _j = open_faulty_with(&t.0, plan.clone()).unwrap();
    assert_eq!(plan.log(), ["sync_dir", "append", "sync"]);
    assert_eq!(
        t.bytes(),
        format!("{}\n", fmt::encode_header()).into_bytes()
    );
    // an existing journal is not synced again
    drop(_j);
    let plan = Plan::default();
    let _j = open_faulty_with(&t.0, plan.clone()).unwrap();
    assert!(plan.log().is_empty());
}

#[test]
fn journal_and_lock_files_are_created_owner_only_and_named_for_what_they_are() {
    assert_eq!(JOURNAL_FILE, "journal.wal");
    let t = Tmp::new();
    let (_j, _) = open_wait(&t.0).unwrap();
    for name in [JOURNAL_FILE, LOCK_FILE] {
        let mode = std::fs::metadata(t.0.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "{name} is accessible to group/others: {mode:o}"
        );
        assert_eq!(
            mode & 0o600,
            0o600,
            "{name} is not owner read/write: {mode:o}"
        );
    }
    assert!(!t.0.join("journal.jsonl").exists());
}

/// The mechanism behind two CI flakes (B5, B6, #116), made deterministic. `flock` belongs to the open
/// file *description*, so a child that is forked while a journal is open holds a copy of the lock until
/// it execs (the descriptor is close-on-exec). Dropping the journal in the parent therefore does not free
/// the lock while another thread's forked-but-not-yet-exec'd child exists. That is operating-system
/// behaviour, not a journal defect: a single-threaded writer (the CLI) never has a fork in flight while
/// it holds the journal, and `Command::spawn` returns only after the exec.
#[test]
fn a_child_forked_while_the_journal_is_open_keeps_the_lock_until_it_execs() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    let t = Tmp::new();
    let (j, _) = FileJournal::open(&t.0).unwrap();
    let (mut ready_rx, ready_tx) = UnixStream::pair().unwrap();
    let (go_tx, go_rx) = UnixStream::pair().unwrap();
    let spawner = std::thread::spawn(move || {
        let (mut ready_tx, mut go_rx) = (ready_tx, go_rx);
        let mut cmd = std::process::Command::new("true");
        // Runs in the child after fork, before exec: say "I exist", then wait to be released.
        unsafe {
            cmd.pre_exec(move || {
                let _ = ready_tx.write_all(&[1]);
                let mut b = [0u8; 1];
                let _ = go_rx.read(&mut b);
                Ok(())
            });
        }
        cmd.status().unwrap();
    });
    let mut b = [0u8; 1];
    ready_rx.read_exact(&mut b).unwrap(); // the child is forked and parked before exec
    drop(j);
    match FileJournal::open(&t.0) {
        Err(OpenError::Locked(_)) => {}
        other => panic!(
            "a parked child should still hold the lock: {:?}",
            other.map(|_| ())
        ),
    }
    let mut go_tx = go_tx;
    go_tx.write_all(&[1]).unwrap(); // let it exec
    spawner.join().unwrap(); // `status()` returns after the exec and the exit
                             // Free once our child has exec'd. Bounded, like every open that expects success: a lock that is
                             // never released still fails after the bound. The strict check above (a parked child still holds
                             // the lock) is the one that proves the mechanism and is unchanged.
    open_wait(&t.0).expect("the lock is free once the child has exec'd");
}
