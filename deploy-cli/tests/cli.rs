//! B6: the CLI end to end, in-process, on temporary directories, with the deploy-docker fake Docker
//! (a test-owned Unix socket) and fake Buildx. Nothing contacts a live Docker endpoint.

#[path = "../../deploy-docker/tests/support/mod.rs"]
mod support;

use harpoon_deploy::engine::Journal;
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator};
use harpoon_deploy::ledger::{Disposition, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::state::{Deployment, DeploymentStatus, Trigger};
use harpoon_deploy::time::{ManualClock, SeqEntropy};
use harpoon_deploy_cli::{run as cli_run, Gate, Outcome, AUTH_FLAG, AUTH_PHRASE};
use harpoon_deploy_journal::{Restored, SqliteJournal, SqliteOpenError, SQLITE_FILE};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use support::fake_docker::Action;
use support::Harness;

// ------------------------------------------------------------------ fork gate (test isolation)
//
// `flock` belongs to the open file description. A child that another test thread forks while this
// test's journal is open holds a copy of the lock until it execs (see deploy-journal's test
// `a_child_forked_while_the_journal_is_open_keeps_the_lock_until_it_execs`), so dropping the journal
// does not free it that instant and the next open reports "another Harpoon process holds the journal".
// That is the operating system's behaviour, not a journal defect, and the journal stays fail-fast.
// What the tests control is the overlap: a thread may only spawn a process (fake Buildx, fake lsof, the
// real binary) while no thread has a journal open, and no journal is opened while a spawn is under way.
// Holders never wait for each other; a spawner waits for them and then runs alone.

struct ForkGate {
    holders: usize,
    spawning: bool,
}

static FORK_GATE: (std::sync::Mutex<ForkGate>, std::sync::Condvar) = (
    std::sync::Mutex::new(ForkGate {
        holders: 0,
        spawning: false,
    }),
    std::sync::Condvar::new(),
);

/// A journal may be open for as long as this lives.
struct Holding;

fn holding() -> Holding {
    let (m, cv) = &FORK_GATE;
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    while g.spawning {
        g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
    }
    g.holders += 1;
    Holding
}

impl Drop for Holding {
    fn drop(&mut self) {
        let (m, cv) = &FORK_GATE;
        m.lock().unwrap_or_else(|e| e.into_inner()).holders -= 1;
        cv.notify_all();
    }
}

/// Run `f`, which may spawn a process, with no journal open in any test thread. Must not be called
/// by a thread that is itself holding a journal.
fn spawning<T>(f: impl FnOnce() -> T) -> T {
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            let (m, cv) = &FORK_GATE;
            m.lock().unwrap_or_else(|e| e.into_inner()).spawning = false;
            cv.notify_all();
        }
    }
    {
        let (m, cv) = &FORK_GATE;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        while g.spawning || g.holders > 0 {
            g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        g.spawning = true;
    }
    let _done = Done;
    f()
}

/// A journal handle that keeps the gate's "holding" state for exactly as long as it is open.
struct Held {
    journal: SqliteJournal, // dropped first: the lock is released before the gate is
    _gate: Holding,
}

impl std::ops::Deref for Held {
    type Target = SqliteJournal;
    fn deref(&self) -> &SqliteJournal {
        &self.journal
    }
}

impl std::ops::DerefMut for Held {
    fn deref_mut(&mut self) -> &mut SqliteJournal {
        &mut self.journal
    }
}

fn open_j(dir: &Path) -> Result<(Held, Restored), SqliteOpenError> {
    let gate = holding();
    SqliteJournal::open(dir).map(|(journal, r)| {
        (
            Held {
                journal,
                _gate: gate,
            },
            r,
        )
    })
}

/// The CLI, in process. Commands that start a process (a live-path `deploy`, an inventory with
/// `--lsof-path`) run alone; everything else may overlap with other tests but counts as holding.
fn run(args: &[String], gate: &Gate) -> Outcome {
    let spawns = (args.first().map(String::as_str) == Some("deploy")
        && !args.iter().any(|a| a == "--simulate"))
        || args.iter().any(|a| a == "--lsof-path");
    if spawns {
        spawning(|| cli_run(args, gate))
    } else {
        let _h = holding();
        cli_run(args, gate)
    }
}

fn spawn_binary(args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    spawning(|| {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_harpoon-deploy"));
        c.args(args);
        for (k, v) in envs {
            c.env(k, v);
        }
        c.output().unwrap()
    })
}

const OPEN: Gate = Gate {
    live_deploy_enabled: true,
    live_replacement_enabled: false,
};
/// What the acceptance build opens: live deploy and the replacement family.
const ACCEPTANCE: Gate = Gate {
    live_deploy_enabled: true,
    live_replacement_enabled: true,
};
const SHIPPING: Gate = Gate::shipping();

struct T {
    h: Harness,
    rt: PathBuf,
    /// The fake guest management channel (roomy by default).
    mgmt: support::fake_mgmt::FakeMgmt,
}

impl T {
    fn new() -> T {
        let h = Harness::new();
        let rt = h.ws.root.join("runtime");
        T {
            h,
            rt,
            mgmt: support::fake_mgmt::FakeMgmt::start(),
        }
    }
    fn go(&self, gate: &Gate, args: &[&str]) -> Outcome {
        let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        if !a.iter().any(|x| x == "--runtime-dir")
            && a.first().map(|s| s != "help").unwrap_or(false)
        {
            a.push("--runtime-dir".into());
            a.push(self.rt.display().to_string());
        }
        run(&a, gate)
    }
    fn cli(&self, args: &[&str]) -> Outcome {
        self.go(&SHIPPING, args)
    }
    fn src(&self) -> Vec<String> {
        vec![
            "--project-dir".into(),
            self.h.ws.context().display().to_string(),
            "--container-port".into(),
            "8080".into(),
        ]
    }
    fn sim_deploy(&self) -> Outcome {
        let src = self.src();
        let mut a: Vec<&str> = vec!["deploy", "--simulate"];
        a.extend(src.iter().map(String::as_str));
        self.cli(&a)
    }
    fn socket(&self) -> String {
        self.h.docker.path().display().to_string()
    }
    fn resources(&self) -> String {
        self.h.ws.resources().display().to_string()
    }
    fn journal_dir(&self) -> PathBuf {
        self.rt.join("deploy-journal")
    }
    fn journal_bytes(&self) -> Vec<u8> {
        std::fs::read(self.journal_dir().join(SQLITE_FILE)).unwrap()
    }
    fn docker_requests(&self) -> usize {
        self.h.docker.requests().len()
    }
    /// A simulated-kind runtime directory prepared by hand (as `deploy --simulate` leaves it).
    fn docker_runtime_sim(&self) {
        self.docker_runtime();
        std::fs::write(self.rt.join("runtime-kind"), "simulated\n").unwrap();
    }
    /// A docker-kind runtime directory prepared by hand (as a previous real deployment would leave it).
    fn docker_runtime(&self) {
        std::fs::create_dir_all(&self.rt).unwrap();
        std::fs::set_permissions(&self.rt, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir_all(self.journal_dir()).unwrap();
        std::fs::set_permissions(self.journal_dir(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::write(self.rt.join("runtime-kind"), "docker\n").unwrap();
    }
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn ids(seed: u8) -> (ApplicationId, DeploymentId) {
    let mut g = IdGenerator::new(ManualClock::new(1_700_000_000_000), SeqEntropy::new(seed));
    (g.application().unwrap(), g.deployment().unwrap())
}

fn finished(app: &ApplicationId, dep: &DeploymentId, to: DeploymentStatus) -> Deployment {
    let mut d = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, 1_000);
    d.transition(DeploymentStatus::Queued, 1_001).unwrap();
    if to != DeploymentStatus::Queued {
        d.transition(DeploymentStatus::Running, 1_002).unwrap();
        if to != DeploymentStatus::Running {
            d.transition(to, 1_003).unwrap();
        }
    }
    d
}

fn intent(
    app: &ApplicationId,
    dep: &DeploymentId,
    kind: ResourceKind,
    name: &str,
    d: Disposition,
) -> LedgerRecord {
    LedgerRecord::Intent {
        key: ResourceKey::new(kind, name),
        app: app.clone(),
        deployment: dep.clone(),
        disposition: d,
        at_ms: 1,
    }
}

fn receipt(dep: &DeploymentId, kind: ResourceKind, name: &str, id: &str) -> LedgerRecord {
    LedgerRecord::Receipt {
        key: ResourceKey::new(kind, name),
        deployment: dep.clone(),
        id: id.into(),
        at_ms: 2,
    }
}

fn assert_exit(o: &Outcome, code: i32) {
    assert_eq!(o.code, code, "stdout:\n{}\nstderr:\n{}", o.stdout, o.stderr);
}

// ------------------------------------------------------------------ usage

#[test]
fn usage_errors_exit_2_and_say_what_is_wrong() {
    let t = T::new();
    for (args, want) in [
        (vec!["frobnicate"], "unknown command"),
        (vec!["status", "--bogus", "x"], "does not take --bogus"),
        (vec!["status", "--runtime-dir"], "needs a value"),
        (vec!["deploy", "--runtime-dir", "relative/dir"], "absolute"),
        (
            vec!["doctor", "--docker-socket", "relative.sock"],
            "absolute",
        ),
        (vec!["status", "stray"], "unexpected argument"),
    ] {
        let o = t.go(&SHIPPING, &args);
        assert_exit(&o, 2);
        assert!(o.stderr.contains(want), "{args:?}: {}", o.stderr);
    }
    // --runtime-dir is mandatory and never guessed
    let o = run(&["status".to_string()], &SHIPPING);
    assert_exit(&o, 2);
    assert!(o.stderr.contains("--runtime-dir"));
    assert_exit(&run(&[], &SHIPPING), 2);
    let h = run(&["help".to_string()], &SHIPPING);
    assert_exit(&h, 0);
    assert!(h.stdout.contains("EXIT CODES") && h.stdout.contains(AUTH_PHRASE));
}

// ------------------------------------------------------------------ first invocation, read-only commands

#[test]
fn read_only_commands_on_a_fresh_runtime_create_nothing() {
    let t = T::new();
    for args in [vec!["status"], vec!["doctor"], vec!["recover"]] {
        let o = t.cli(&args);
        assert_exit(&o, 0);
    }
    let src = t.src();
    let mut plan = vec!["plan"];
    plan.extend(src.iter().map(String::as_str));
    let o = t.cli(&plan);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("nothing is built, created or started"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("host port 39000") && o.stdout.contains("host port 38000"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("live execution: disabled"),
        "{}",
        o.stdout
    );
    assert!(
        !t.rt.exists(),
        "no read-only command (nor recover with nothing to recover) creates the runtime directory"
    );
    assert_eq!(t.docker_requests(), 0);
    let o = t.cli(&["status"]);
    assert!(o.stdout.contains("not initialised"), "{}", o.stdout);
}

#[test]
fn doctor_reports_each_check_and_never_mutates_docker() {
    let t = T::new();
    let src = t.src();
    let (sock, res) = (t.socket(), t.resources());
    let mut a: Vec<&str> = vec!["doctor", "--docker-socket", &sock, "--resources-dir", &res];
    a.extend(src.iter().map(String::as_str));
    let o = t.cli(&a);
    assert_exit(&o, 0);
    for want in [
        "[ok]   source",
        "[ok]   bundled Buildx",
        "[ok]   journal location",
        "answered a ping",
        "alpine:3.22 is present locally",
        "0 failures",
    ] {
        assert!(o.stdout.contains(want), "{want}\n{}", o.stdout);
    }
    assert!(
        t.h.docker.requests().iter().all(|r| r.method == "GET"),
        "doctor only reads"
    );
    assert!(!t.rt.exists());

    // a missing base image, a missing Buildx, an unreachable runtime and a bad Dockerfile are failures with distinct codes
    std::fs::write(t.h.ws.context().join("Dockerfile"), "FROM nowhere:1\n").unwrap();
    assert_exit(&t.cli(&a), 3);
    let o = t.cli(&["doctor", "--resources-dir", "/nonexistent-resources"]);
    assert_exit(&o, 3);
    assert!(o.stdout.contains("never falls back"), "{}", o.stdout);
    let o = t.cli(&["doctor", "--docker-socket", "/nonexistent/docker.sock"]);
    assert_exit(&o, 5);
    assert!(o.stdout.contains("[FAIL] runtime"));
    std::fs::write(
        t.h.ws.context().join("Dockerfile"),
        "# syntax=docker/dockerfile:1\nFROM alpine:3.22\n",
    )
    .unwrap();
    let o = t.cli(&a);
    assert_exit(&o, 3);
    assert!(o.stdout.contains("external frontend"), "{}", o.stdout);
}

#[test]
fn the_environment_and_docker_contexts_are_never_consulted() {
    std::env::set_var("DOCKER_HOST", "unix:///nonexistent-canary.sock");
    std::env::set_var("DOCKER_CONTEXT", "canary");
    let t = T::new();
    let o = t.cli(&["doctor"]);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("Docker was not contacted"));
    assert_eq!(t.docker_requests(), 0);
}

// ------------------------------------------------------------------ simulated deployment through the journal

#[test]
fn a_simulated_deployment_runs_through_the_real_journal_and_is_visible_afterwards() {
    let t = T::new();
    let o = t.sim_deploy();
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("SIMULATED, no Docker") && o.stdout.contains("result: succeeded"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("(simulated)"),
        "evidence is labelled simulated: {}",
        o.stdout
    );

    // privacy
    assert_eq!(mode(&t.rt), 0o700);
    assert_eq!(mode(&t.journal_dir()), 0o700);
    assert_eq!(mode(&t.journal_dir().join(SQLITE_FILE)), 0o600);
    assert_eq!(mode(&t.journal_dir().join("sqlite.lock")), 0o600);
    assert_eq!(
        std::fs::read_to_string(t.rt.join("runtime-kind")).unwrap(),
        "simulated\n"
    );

    let o = t.cli(&["status"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("kind: simulated")
            && o.stdout.contains("deployments: 1")
            && o.stdout.contains("succeeded"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("container hp-") && o.stdout.contains("state=created"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("outstanding leases:\n  port 39000"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("unresolved cleanup: none"),
        "{}",
        o.stdout
    );
    assert_eq!(t.docker_requests(), 0, "a simulation never contacts Docker");
}

#[test]
fn status_is_strictly_read_only() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let bytes = t.journal_bytes();
    let meta = |p: PathBuf| std::fs::metadata(p).unwrap().modified().unwrap();
    let (m1, m2) = (
        meta(t.journal_dir().join(SQLITE_FILE)),
        meta(t.journal_dir().join("sqlite.lock")),
    );
    let listing = || {
        let mut v: Vec<_> = std::fs::read_dir(t.journal_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        v.sort();
        v
    };
    let before = listing();
    for cmd in [vec!["status"], vec!["doctor"]] {
        let o = t.cli(&cmd);
        assert_exit(&o, 0);
    }
    assert_eq!(t.journal_bytes(), bytes);
    assert_eq!(meta(t.journal_dir().join(SQLITE_FILE)), m1);
    assert_eq!(meta(t.journal_dir().join("sqlite.lock")), m2);
    assert_eq!(listing(), before);
}

#[test]
fn leases_from_one_deployment_steer_the_next_and_the_plan() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    assert_exit(&t.sim_deploy(), 0); // a different, newly generated application
    let o = t.cli(&["status"]);
    assert!(
        o.stdout.contains("port 39000") && o.stdout.contains("port 39001"),
        "two distinct leases: {}",
        o.stdout
    );
    assert!(o.stdout.contains("deployments: 2"));
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    let mut ports = r.outstanding_leases();
    ports.sort();
    assert_eq!(ports, vec![39000, 39001]);

    // a docker runtime that already holds 39000/38000 makes the plan pick the next ports
    let t2 = T::new();
    t2.docker_runtime();
    let (app, dep) = ids(3);
    let (mut j, _) = open_j(&t2.journal_dir()).unwrap();
    j.persist(&finished(&app, &dep, DeploymentStatus::Succeeded))
        .unwrap();
    for (p, _) in [("39000", 0), ("38000", 1)] {
        j.record(&intent(
            &app,
            &dep,
            ResourceKind::PortLease,
            p,
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
        j.record(&receipt(&dep, ResourceKind::PortLease, p, p))
            .unwrap();
    }
    drop(j);
    let src = t2.src();
    let mut a = vec!["plan"];
    a.extend(src.iter().map(String::as_str));
    let o = t2.cli(&a);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("host port 39001") && o.stdout.contains("host port 38001"),
        "{}",
        o.stdout
    );
}

// ------------------------------------------------------------------ interrupted deployments and recovery

fn crashed_runtime(t: &T, with_unreceipted_container: bool) -> DeploymentId {
    assert_exit(&t.sim_deploy(), 0); // initialise a (simulated) runtime
    let (app, dep) = ids(9);
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&finished(&app, &dep, DeploymentStatus::Running))
        .unwrap();
    if with_unreceipted_container {
        j.record(&intent(
            &app,
            &dep,
            ResourceKind::Container,
            "hp-crashed",
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
    }
    dep
}

#[test]
fn an_unfinished_deployment_blocks_deploy_until_recover_marks_it_and_recover_is_idempotent() {
    let t = T::new();
    let dep = crashed_runtime(&t, false);
    let o = t.cli(&["status"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("running") && o.stdout.contains("attention:"),
        "{}",
        o.stdout
    );
    let before = t.journal_bytes();
    let o = t.sim_deploy();
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("is running") && o.stderr.contains("recover"),
        "{}",
        o.stderr
    );
    assert_eq!(t.journal_bytes(), before, "a refused deploy writes nothing");

    let o = t.cli(&["recover"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains(&format!("marked interrupted: {dep}")),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("Docker was not contacted"));
    let o = t.cli(&["status"]);
    assert!(o.stdout.contains("interrupted"), "{}", o.stdout);
    let after = t.journal_bytes();
    let o = t.cli(&["recover"]);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("no deployment needed"), "{}", o.stdout);
    assert_eq!(
        t.journal_bytes(),
        after,
        "recovering twice changes nothing the second time"
    );
    // nothing unresolved is left (no ledger entries), so a deployment may proceed
    assert_exit(&t.sim_deploy(), 0);
}

#[test]
fn an_unreceipted_creation_keeps_blocking_deploy_after_recover() {
    let t = T::new();
    crashed_runtime(&t, true);
    assert_exit(&t.cli(&["recover"]), 0);
    let o = t.cli(&["status"]);
    assert!(o.stdout.contains("no receipt exists"), "{}", o.stdout);
    let o = t.sim_deploy();
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("hp-crashed") && o.stderr.contains("reconcile"),
        "{}",
        o.stderr
    );
}

// ------------------------------------------------------------------ B8.1: interruption end to end
//
// These tests run the real CLI and the real `DockerBackend` against the *simulated* Docker server
// of this crate's tests. No Docker Engine, VM or socket outside the temporary directory is
// involved, and no real container or image exists at any point.

const CHILD_ENV: &str = "HARPOON_B81_CHILD_ARGS";

/// Child role: when launched by `interrupt_a_live_deploy` this runs one gate-open `deploy` and is
/// killed by its parent mid-run. In an ordinary test run it does nothing.
#[test]
fn b8_child_live_deploy_to_be_killed() {
    let Ok(raw) = std::env::var(CHILD_ENV) else {
        return;
    };
    let args: Vec<String> = raw.split('\u{1f}').map(String::from).collect();
    let o = cli_run(&args, &OPEN);
    std::process::exit(if o.code == 0 { 99 } else { o.code });
}

/// Start a live deployment in a separate process, hold it inside the start of its preliminary
/// container, and SIGKILL it. What remains is exactly what an interrupted run leaves: a journal
/// with a running deployment, leases and a receipted container, and that container in Docker.
/// Tests that bind or occupy the preliminary-check or application port ranges, or that run a live
/// deployment that needs a free port from them, take this so they never starve each other.
pub(crate) fn port_range() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn interrupt_a_live_deploy(t: &T) {
    use std::time::{Duration, Instant};
    let _ports = port_range();
    image_appears_after_build(t);
    t.h.docker.fault(
        Some("POST"),
        "/start",
        1,
        Action::Delay(Duration::from_millis(2000)),
    );
    let mut args = live_args(t);
    args.push("--runtime-dir".into());
    args.push(t.rt.display().to_string());
    let exe = std::env::current_exe().unwrap();
    let mut child = spawning(|| {
        std::process::Command::new(exe)
            .args([
                "--exact",
                "b8_child_live_deploy_to_be_killed",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, args.join("\u{1f}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    });
    let t0 = Instant::now();
    loop {
        if t.h
            .docker
            .requests()
            .iter()
            .any(|r| r.method == "POST" && r.path.contains("/start"))
        {
            break;
        }
        if let Some(st) = child.try_wait().unwrap() {
            panic!("the deploy ended before it could be interrupted: {st}");
        }
        assert!(
            t0.elapsed() < Duration::from_secs(30),
            "the deploy never reached a start"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        child.try_wait().unwrap().is_none(),
        "still running when interrupted"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    // The start request that was already in flight finishes inside the simulated Docker.
    std::thread::sleep(Duration::from_millis(2600));
}

struct Foreign {
    container: String,
    image: String,
}

fn add_foreign(t: &T) -> Foreign {
    t.h.docker.with(|s| Foreign {
        container: s.add_container("someone-elses", vec![], true),
        image: s.add_image("someone-elses:1", vec![]),
    })
}

fn assert_foreign_untouched(t: &T, f: &Foreign) {
    t.h.docker.with(|s| {
        assert!(
            s.containers
                .iter()
                .any(|c| c.id == f.container && c.running),
            "the foreign container was touched"
        );
        assert!(
            s.images.iter().any(|i| i.id == f.image),
            "the foreign image was touched"
        );
    });
    for r in t.h.docker.requests() {
        assert!(
            !r.path.contains(&f.container) || r.method == "GET",
            "a request other than a read named the foreign container: {} {}",
            r.method,
            r.path
        );
        assert!(
            !r.path.contains(&f.image) || r.method == "GET",
            "a request other than a read named the foreign image: {} {}",
            r.method,
            r.path
        );
    }
}

fn writes(t: &T) -> usize {
    t.h.docker
        .requests()
        .iter()
        .filter(|r| r.method != "GET")
        .count()
}

fn harpoon_containers(t: &T) -> Vec<String> {
    t.h.docker.with(|s| {
        s.containers
            .iter()
            .filter(|c| c.labels.iter().any(|(k, _)| k == "io.harpoon.managed"))
            .map(|c| c.id.clone())
            .collect()
    })
}

fn refs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

#[test]
fn an_interrupted_deployment_is_recovered_reconciled_and_only_then_lets_the_next_one_run() {
    let t = T::new();
    let foreign = add_foreign(&t);
    interrupt_a_live_deploy(&t);
    let ours = harpoon_containers(&t);
    assert!(
        !ours.is_empty(),
        "the interruption left Harpoon's container in Docker"
    );

    // 1. Nothing may deploy while the journal still says a deployment is running.
    let n = writes(&t);
    let o = t.go(&OPEN, &refs(&live_args(&t)));
    assert_exit(&o, 6);
    assert!(o.stderr.contains("recover"), "{}", o.stderr);
    assert_eq!(writes(&t), n, "a refused deploy changed Docker");

    // 2. recover changes the journal only: Docker is not contacted at all.
    let before = t.h.docker.requests().len();
    let o = t.cli(&["recover"]);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("marked interrupted"), "{}", o.stdout);
    assert!(
        o.stdout.contains("Docker was not contacted"),
        "{}",
        o.stdout
    );
    assert_eq!(t.h.docker.requests().len(), before);

    // 3. Recovered is not reconciled: the container and leases are still unresolved.
    let s = t.cli(&["status"]);
    assert!(s.stdout.contains("unresolved cleanup ("), "{}", s.stdout);
    let o = t.go(&OPEN, &refs(&live_args(&t)));
    assert_exit(&o, 6);
    assert!(o.stderr.contains("reconcile"), "{}", o.stderr);
    assert_eq!(harpoon_containers(&t), ours, "still there");

    // 4. The dry run reads and proposes; it writes nothing, to Docker or to the journal.
    let journal = t.journal_bytes();
    let n = writes(&t);
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &[]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("would: remove container hp-"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("dry run"), "{}", o.stdout);
    assert_eq!(writes(&t), n);
    assert_eq!(t.journal_bytes(), journal);
    assert_eq!(harpoon_containers(&t), ours);

    // 5. Apply: only what is provably ours, each by its recorded id, never forced.
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 0);
    assert!(o.stdout.contains("nothing unresolved"), "{}", o.stdout);
    assert!(
        harpoon_containers(&t).is_empty(),
        "Harpoon's container was removed"
    );
    for r in t.h.docker.requests() {
        if r.method == "DELETE" {
            assert!(r.path.contains("force=false"), "{}", r.path);
            assert!(
                r.path.starts_with("/containers/"),
                "only containers were deleted: {}",
                r.path
            );
            assert!(ours.iter().any(|id| r.path.contains(id)), "{}", r.path);
        }
        if r.method == "POST" && r.path.ends_with("/stop") {
            assert!(ours.iter().any(|id| r.path.contains(id)), "{}", r.path);
        }
    }
    assert_foreign_untouched(&t, &foreign);
    // The built image of an interrupted run is retained, not removed and not "unresolved" (F2).
    assert!(t.h.docker.with(|s| s
        .images
        .iter()
        .any(|i| i.tags.iter().any(|g| g.starts_with("harpoon/")))));
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("unresolved cleanup: none"),
        "{}",
        s.stdout
    );
    assert!(s.stdout.contains("interrupted"), "{}", s.stdout);

    // 6. Now, and only now, the next deployment is permitted.
    image_appears_after_build(&t);
    let o = t.go(&OPEN, &refs(&live_args(&t)));
    assert_exit(&o, 7); // it ran (nothing listens on the port); it was not refused
    assert!(
        o.stdout.contains("failure at preliminary_check"),
        "{}",
        o.stdout
    );
    assert_foreign_untouched(&t, &foreign);
    assert!(harpoon_containers(&t).is_empty());
}

#[test]
fn a_reconcile_that_cannot_finish_leaves_the_runtime_blocked_until_it_does() {
    let t = T::new();
    let foreign = add_foreign(&t);
    interrupt_a_live_deploy(&t);
    let ours = harpoon_containers(&t);
    assert_exit(&t.cli(&["recover"]), 0);

    // Docker refuses every removal.
    t.h.docker.fault(
        Some("DELETE"),
        "/containers/",
        10_000,
        Action::Respond(500, "{\"message\":\"device or resource busy\"}".into()),
    );
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 7);
    assert!(o.stdout.contains("still unresolved"), "{}", o.stdout);
    assert_eq!(
        harpoon_containers(&t),
        ours,
        "the container really is still there"
    );
    let s = t.cli(&["status"]);
    assert!(s.stdout.contains("cleanup failed"), "{}", s.stdout);
    let n = writes(&t);
    let o = t.go(&OPEN, &refs(&live_args(&t)));
    assert_exit(&o, 6);
    assert_eq!(writes(&t), n, "a refused deploy changed Docker");

    // Docker recovers; the next reconcile finishes, and the runtime is open again.
    t.h.docker.with(|s| s.faults.clear());
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 0);
    assert!(harpoon_containers(&t).is_empty());
    image_appears_after_build(&t);
    let o = t.go(&OPEN, &refs(&live_args(&t)));
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("failure at preliminary_check"),
        "{}",
        o.stdout
    );
    assert_foreign_untouched(&t, &foreign);
}

#[test]
fn a_refused_ownership_stays_terminal_and_blocks_the_runtime_directory() {
    // F1, documented in DEPLOY_CLI.md: there is deliberately no acknowledgement or override.
    let t = T::new();
    let foreign = add_foreign(&t);
    interrupt_a_live_deploy(&t);
    let ours = harpoon_containers(&t);
    assert_exit(&t.cli(&["recover"]), 0);

    // Something else takes over the recorded container: same id, no Harpoon labels.
    let original: Vec<(String, Vec<(String, String)>)> = t.h.docker.with(|s| {
        s.containers
            .iter_mut()
            .filter(|c| ours.contains(&c.id))
            .map(|c| (c.id.clone(), std::mem::take(&mut c.labels)))
            .collect()
    });
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let n = writes(&t);
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 7);
    assert!(o.stdout.contains("still unresolved"), "{}", o.stdout);
    assert_eq!(writes(&t), n, "the unlabelled container was not touched");
    assert!(t.h.docker.with(|s| ours
        .iter()
        .all(|id| s.containers.iter().any(|c| &c.id == id))));

    // Repeating it, or even putting the labels back, does not reopen the entry; deploy stays refused.
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 7);
    assert_eq!(writes(&t), n);
    t.h.docker.with(|s| {
        for (id, labels) in &original {
            s.containers
                .iter_mut()
                .find(|c| &c.id == id)
                .unwrap()
                .labels = labels.clone();
        }
    });
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 7);
    assert_eq!(
        writes(&t),
        n,
        "restored labels did not reopen a refused entry"
    );
    let o = t.go(&OPEN, &refs(&live_args(&t)));
    assert_exit(&o, 6);
    assert!(o.stderr.contains("ownership not proven"), "{}", o.stderr);
    let s = t.cli(&["status"]);
    assert!(s.stdout.contains("ownership not proven"), "{}", s.stdout);
    assert_eq!(writes(&t), n);
    assert_foreign_untouched(&t, &foreign);
}

// ------------------------------------------------------------------ a runtime that only has the legacy journal

/// Hand-build what a pre-SQLite build left behind: the write-ahead journal and its lock, nothing else.
fn legacy_runtime(t: &T) -> Vec<u8> {
    t.docker_runtime();
    let (mut j, _) = harpoon_deploy_journal::FileJournal::open(&t.journal_dir()).unwrap();
    let app: harpoon_deploy::ids::ApplicationId = "app_00000000000000000000000000".parse().unwrap();
    let dep: harpoon_deploy::ids::DeploymentId = "dep_00000000000000000000000000".parse().unwrap();
    let mut d = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, 10);
    d.transition(DeploymentStatus::Queued, 11).unwrap();
    d.transition(DeploymentStatus::Running, 12).unwrap();
    j.persist(&d).unwrap();
    j.record(&LedgerRecord::Intent {
        key: ResourceKey::new(ResourceKind::PortLease, "38000"),
        app,
        deployment: dep,
        disposition: Disposition::RemoveOnFailure,
        at_ms: 12,
    })
    .unwrap();
    drop(j);
    std::fs::read(t.journal_dir().join("journal.wal")).unwrap()
}

fn listing(t: &T) -> Vec<std::ffi::OsString> {
    let mut v: Vec<_> = std::fs::read_dir(t.journal_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    v.sort();
    v
}

#[test]
fn read_only_commands_on_a_legacy_only_runtime_work_and_change_nothing() {
    let t = T::new();
    let mut wal = legacy_runtime(&t);
    // a torn tail is shown, ignored and left alone
    wal.extend_from_slice(b"deadbeef\tsnap\t1\tpart");
    std::fs::write(t.journal_dir().join("journal.wal"), &wal).unwrap();
    let before = listing(&t);
    for cmd in [vec!["status"], vec!["doctor"]] {
        assert_exit(&t.cli(&cmd), 0);
    }
    let o = t.cli(&["status"]);
    assert!(
        o.stdout.contains("deployments: 1") && o.stdout.contains("port 38000"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("incomplete write"), "{}", o.stdout);
    assert_eq!(
        std::fs::read(t.journal_dir().join("journal.wal")).unwrap(),
        wal
    );
    assert_eq!(listing(&t), before, "no SQLite file or lock was created");
}

#[test]
fn the_first_writer_upgrades_a_legacy_runtime_and_keeps_the_old_journal_exportable() {
    let t = T::new();
    let wal = legacy_runtime(&t);
    let copy = t.h.ws.root.join("legacy-copy.wal");
    assert_exit(
        &t.cli(&["export-journal", "--to", &copy.display().to_string()]),
        0,
    );
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        wal,
        "a legacy runtime exports its journal"
    );

    let o = t.cli(&["recover"]);
    assert_exit(&o, 0);
    assert_eq!(mode(&t.journal_dir().join(SQLITE_FILE)), 0o600);
    assert_eq!(
        std::fs::read(t.journal_dir().join("journal.wal")).unwrap(),
        wal
    );
    let o = t.cli(&["status"]);
    assert!(
        o.stdout.contains("interrupted") && o.stdout.contains("port 38000"),
        "history and lease survived the upgrade: {}",
        o.stdout
    );
}

#[test]
fn a_file_that_is_not_a_sqlite_database_fails_closed_and_is_left_alone() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let junk = b"this is not a database, and must never be overwritten".to_vec();
    std::fs::write(t.journal_dir().join(SQLITE_FILE), &junk).unwrap();
    for args in [vec!["status"], vec!["recover"]] {
        let o = t.cli(&args);
        assert_exit(&o, 4);
        let text = format!("{}{}", o.stdout, o.stderr);
        assert!(
            text.contains("export-journal") && text.contains("Do not delete"),
            "{text}"
        );
    }
    assert_exit(&t.sim_deploy(), 4);
    assert_eq!(
        std::fs::read(t.journal_dir().join(SQLITE_FILE)).unwrap(),
        junk
    );
}

// ------------------------------------------------------------------ corrupt journal, lock contention, permissions

#[test]
fn a_corrupt_journal_fails_closed_with_actionable_output_and_can_be_exported() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let mut bytes = t.journal_bytes();
    // damage one stored record (a deployment id inside a checksummed line), not SQLite's own pages
    let at = bytes.windows(4).position(|w| w == b"dep_").unwrap() + 6;
    bytes[at] ^= 0x01;
    std::fs::write(t.journal_dir().join(SQLITE_FILE), &bytes).unwrap();
    for args in [
        vec!["status"],
        vec!["doctor"],
        vec!["recover"],
        vec!["plan"],
    ] {
        let mut a = args.clone();
        let src = t.src();
        if args[0] == "plan" {
            a.extend(src.iter().map(String::as_str));
        }
        let o = t.cli(&a);
        if args[0] == "doctor" {
            assert!(o.stdout.contains("[FAIL] journal"), "{}", o.stdout);
        }
        assert_exit(&o, 4);
        let text = format!("{}{}", o.stdout, o.stderr);
        assert!(
            text.contains("invalid SQLite record")
                && text.contains("export-journal")
                && text.contains("Do not delete"),
            "{args:?}: {text}"
        );
    }
    let o = t.sim_deploy();
    assert_exit(&o, 4);
    assert_eq!(
        t.journal_bytes(),
        bytes,
        "never repaired, reset or discarded"
    );

    // export a copy
    let copy = t.h.ws.root.join("copy.sqlite3");
    let o = t.cli(&["export-journal", "--to", &copy.display().to_string()]);
    assert_exit(&o, 0);
    assert_eq!(std::fs::read(&copy).unwrap(), bytes);
    assert_eq!(mode(&copy), 0o600);
    assert!(o.stdout.contains("was not modified"));
    // it never overwrites, and never writes into the journal directory
    assert_exit(
        &t.cli(&["export-journal", "--to", &copy.display().to_string()]),
        4,
    );
    let inside = t.journal_dir().join("x.wal");
    assert_exit(
        &t.cli(&["export-journal", "--to", &inside.display().to_string()]),
        2,
    );
    assert_exit(&t.cli(&["export-journal"]), 2);
    assert_eq!(t.journal_bytes(), bytes);
}

#[test]
fn a_held_writer_lock_is_honoured_fail_fast_and_does_not_block_reading() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let (_held, _) = open_j(&t.journal_dir()).unwrap();
    let before = t.journal_bytes();
    for o in [t.sim_deploy(), t.cli(&["recover"])] {
        assert_exit(&o, 4);
        assert!(
            o.stderr.contains("another Harpoon process")
                && o.stderr.contains("second journal directory"),
            "{}",
            o.stderr
        );
    }
    let o = t.cli(&["status"]);
    assert_exit(&o, 0);
    assert!(o.stdout.contains("held by a writer"), "{}", o.stdout);
    let o = t.cli(&["doctor"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("a writer holds it right now"),
        "{}",
        o.stdout
    );
    assert_eq!(t.journal_bytes(), before);
    // no second journal directory appeared to get around the lock
    let mut names: Vec<_> = std::fs::read_dir(&t.rt)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert!(
        names
            .iter()
            .all(|n| n == "deploy-journal" || n == "runtime-kind" || n == "scratch"),
        "{names:?}"
    );
}

#[test]
fn loose_permissions_symlinks_and_mixed_runtime_kinds_are_refused() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    // a real-deploy-flavoured command cannot use a simulated runtime and vice versa
    let sock = t.socket();
    let res = t.resources();
    let o = t.cli(&[
        "reconcile",
        "--docker-socket",
        &sock,
        "--resources-dir",
        &res,
    ]);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("simulated"), "{}", o.stderr);

    std::fs::set_permissions(t.journal_dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = t.sim_deploy();
    assert_exit(&o, 4);
    assert!(o.stderr.contains("chmod 700"), "{}", o.stderr);
    let o = t.cli(&["doctor"]);
    assert_exit(&o, 4);
    std::fs::set_permissions(t.journal_dir(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_exit(&t.sim_deploy(), 0);

    // a symlinked journal directory
    let t2 = T::new();
    std::fs::create_dir_all(&t2.rt).unwrap();
    std::fs::set_permissions(&t2.rt, std::fs::Permissions::from_mode(0o700)).unwrap();
    let elsewhere = t2.h.ws.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, t2.journal_dir()).unwrap();
    let o = t2.sim_deploy();
    assert_exit(&o, 4);
    assert!(o.stderr.contains("symlink"), "{}", o.stderr);
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());

    // a marked docker runtime refuses a simulation, and a marked simulated one refuses a real deploy
    let td = T::new();
    td.docker_runtime();
    let o = td.sim_deploy();
    assert_exit(&o, 6);
    assert!(o.stderr.contains("docker runtime"), "{}", o.stderr);
    let o = go(&t, &OPEN, &live_args(&t));
    assert_exit(&o, 6);
    assert!(o.stderr.contains("simulated runtime"), "{}", o.stderr);

    // a simulation refuses a runtime that holds a real (unmarked) journal
    let t3 = T::new();
    t3.docker_runtime();
    std::fs::remove_file(t3.rt.join("runtime-kind")).unwrap();
    drop(open_j(&t3.journal_dir()).unwrap());
    let o = t3.sim_deploy();
    assert_exit(&o, 6);
    assert!(o.stderr.contains("own runtime directory"), "{}", o.stderr);
}

// ------------------------------------------------------------------ the live gate

#[test]
fn live_deploy_is_refused_before_anything_is_read_written_or_contacted() {
    let t = T::new();
    let (sock, res) = (t.socket(), t.resources());
    let src = t.src();
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let mut full: Vec<&str> = vec![
        "deploy",
        "--docker-socket",
        &sock,
        "--resources-dir",
        &res,
        &auth,
    ];
    full.extend(src.iter().map(String::as_str));
    // shipping build: refused even with every flag and a reachable socket
    let o = t.go(&SHIPPING, &full);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("disabled in this build") && o.stderr.contains("B7"),
        "{}",
        o.stderr
    );
    assert!(!t.rt.exists(), "no directory, journal or lock was created");
    assert_eq!(t.docker_requests(), 0, "the socket was not contacted");

    // build with the gate open still needs the explicit opt-in, spelled exactly, and a socket
    let mut no_auth: Vec<&str> = vec!["deploy", "--docker-socket", &sock, "--resources-dir", &res];
    no_auth.extend(src.iter().map(String::as_str));
    let o = t.go(&OPEN, &no_auth);
    assert_exit(&o, 6);
    assert!(o.stderr.contains(AUTH_PHRASE), "{}", o.stderr);
    let wrong = format!("--{AUTH_FLAG}=yes");
    let mut w = no_auth.clone();
    w.push(&wrong);
    assert_exit(&t.go(&OPEN, &w), 6);
    let mut no_sock: Vec<&str> = vec!["deploy", &auth, "--resources-dir", &res];
    no_sock.extend(src.iter().map(String::as_str));
    assert_exit(&t.go(&OPEN, &no_sock), 5);
    assert!(!t.rt.exists());
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn a_real_binary_invocation_cannot_deploy_live() {
    let t = T::new();
    let (sock, res) = (t.socket(), t.resources());
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let rt = t.rt.display().to_string();
    let ctx = t.h.ws.context().display().to_string();
    let out = spawn_binary(
        &[
            "deploy",
            "--runtime-dir",
            &rt,
            "--docker-socket",
            &sock,
            "--resources-dir",
            &res,
            &auth,
            "--project-dir",
            &ctx,
            "--container-port",
            "8080",
        ],
        &[("DOCKER_HOST", "unix:///nonexistent")],
    );
    assert_eq!(out.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&out.stderr).contains("disabled in this build"));
    assert!(!t.rt.exists());
    assert_eq!(t.docker_requests(), 0);
    assert_eq!(spawn_binary(&["help"], &[]).status.code(), Some(0));
    assert_eq!(spawn_binary(&[], &[]).status.code(), Some(2));
}

// ------------------------------------------------------------------ gate open: the wiring against the fake Docker

/// When the build of `harpoon/<app>:<dep>` is inspected for the second time, make that image exist,
/// labelled for the deployment the CLI chose (taken from the request path).
fn image_appears_after_build(t: &T) {
    t.h.docker.fault_after(
        Some("GET"),
        "/images/sha256:",
        0,
        1,
        Action::Race(std::sync::Arc::new(|s| {
            let id = s
                .requests
                .last()
                .unwrap()
                .path
                .trim_start_matches("/images/")
                .trim_end_matches("/json")
                .to_string();
            let tag = s
                .requests
                .iter()
                .rev()
                .find(|r| r.path.starts_with("/images/harpoon/"))
                .unwrap()
                .path
                .trim_start_matches("/images/")
                .trim_end_matches("/json")
                .to_string();
            let (app_lc, dep) = tag.trim_start_matches("harpoon/").split_once(':').unwrap();
            let dep = dep.rsplit_once('-').unwrap().0; // drop the random suffix
            let app = format!(
                "app_{}",
                app_lc.trim_start_matches("app_").to_ascii_uppercase()
            );
            let (app, dep): (ApplicationId, DeploymentId) =
                (app.parse().unwrap(), dep.parse().unwrap());
            s.add_image_with_id(
                &id,
                &[&tag],
                harpoon_deploy::backend::harpoon_labels(&app, &dep),
                vec![],
            );
        })),
    );
}

fn live_args(t: &T) -> Vec<String> {
    let mut v: Vec<String> = vec![
        "deploy".into(),
        "--docker-socket".into(),
        t.socket(),
        "--resources-dir".into(),
        t.resources(),
        format!("--{AUTH_FLAG}={AUTH_PHRASE}"),
        "--health-timeout-secs".into(),
        "1".into(),
        "--forwarder-pid".into(),
        "4242".into(),
        "--mgmt-socket".into(),
        t.mgmt.path().display().to_string(),
    ];
    v.extend(t.src());
    v
}

#[test]
fn with_the_gate_open_a_deployment_drives_the_backend_and_the_journal_and_fails_cleanly() {
    let t = T::new();
    image_appears_after_build(&t);
    let a = live_args(&t);
    let o = t.go(&OPEN, &a.iter().map(String::as_str).collect::<Vec<_>>());
    // nothing listens on the published port, so the preliminary check cannot pass
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("result: failed") && o.stdout.contains("failure at preliminary_check"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("image tag: harpoon/"), "{}", o.stdout);
    // the build ran through the bundled fake Buildx only
    assert!(!t.h.ws.buildx_record("args.txt").is_empty());
    // the preliminary container was created and removed; nothing foreign was touched
    let reqs = t.h.docker.requests();
    assert!(reqs
        .iter()
        .any(|r| r.path.starts_with("/containers/create")));
    assert!(reqs
        .iter()
        .any(|r| r.method == "DELETE" && r.path.contains("force=false")));
    assert!(
        t.h.docker.with(|s| s.containers.is_empty()),
        "no container remains"
    );
    assert!(reqs.iter().all(|r| !r.path.contains("prune")));
    // the journal remembers all of it
    assert_eq!(mode(&t.journal_dir().join(SQLITE_FILE)), 0o600);
    assert_eq!(
        std::fs::read_to_string(t.rt.join("runtime-kind")).unwrap(),
        "docker\n"
    );
    let s = t.cli(&["status"]);
    assert_exit(&s, 0);
    assert!(
        s.stdout.contains("failed") && s.stdout.contains("image harpoon/"),
        "{}",
        s.stdout
    );
    assert!(
        s.stdout.contains("unresolved cleanup: none"),
        "{}",
        s.stdout
    );
    assert!(s.stdout.contains("kind: docker"));

    // a second deployment for a different application is not blocked by the retained image,
    // and it cannot reuse the first one's tag
    image_appears_after_build(&t);
    let o2 = t.go(&OPEN, &a.iter().map(String::as_str).collect::<Vec<_>>());
    assert_exit(&o2, 7);
    assert!(
        o2.stdout.contains("failure at preliminary_check"),
        "{}",
        o2.stdout
    );
}

#[test]
fn a_build_that_produces_no_image_fails_the_deployment_and_leaves_nothing_unresolved() {
    let t = T::new();
    // no image ever appears: the fake build "succeeds" but produced nothing
    let a = live_args(&t);
    let refs = || a.iter().map(String::as_str).collect::<Vec<_>>();
    let o = t.go(&OPEN, &refs());
    assert_exit(&o, 7);
    assert!(o.stdout.contains("failure at build"), "{}", o.stdout);
    // the engine already reconciled the image intent at the end of the run (nothing existed)
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("unresolved cleanup: none"),
        "{}",
        s.stdout
    );
    let _ = refs;
}

// ------------------------------------------------------------------ reconcile: foreign and owned resources, unreachable Docker

fn reconcile_fixture(t: &T, status: DeploymentStatus) -> (ApplicationId, DeploymentId) {
    t.docker_runtime();
    let (app, dep) = ids(21);
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&finished(&app, &dep, status)).unwrap();
    j.record(&intent(
        &app,
        &dep,
        ResourceKind::Container,
        "hp-leftover",
        Disposition::RemoveOnFailure,
    ))
    .unwrap();
    (app, dep)
}

fn reconcile_args(t: &T, extra: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = vec![
        "reconcile".into(),
        "--docker-socket".into(),
        t.socket(),
        "--resources-dir".into(),
        t.resources(),
    ];
    v.extend(extra.iter().map(|s| s.to_string()));
    v
}

fn go(t: &T, gate: &Gate, a: &[String]) -> Outcome {
    t.go(gate, &a.iter().map(String::as_str).collect::<Vec<_>>())
}

#[test]
fn reconcile_never_touches_a_foreign_resource_that_merely_has_the_same_name() {
    let t = T::new();
    reconcile_fixture(&t, DeploymentStatus::Failed);
    let foreign =
        t.h.docker
            .with(|s| s.add_container("hp-leftover", vec![], true));
    let before = t.journal_bytes();

    // dry run: reports the refusal and writes nothing
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &[]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("would: REFUSE container hp-leftover"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("dry run: nothing was changed"));
    assert_eq!(t.journal_bytes(), before);

    // --apply without the exact opt-in is refused before Docker is contacted
    let n = t.docker_requests();
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply"]));
    assert_exit(&o, 6);
    assert_eq!(t.docker_requests(), n);

    // with the opt-in it records the refusal and still removes nothing
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("REFUSE container hp-leftover") && o.stdout.contains("still unresolved"),
        "{}",
        o.stdout
    );
    let c =
        t.h.docker
            .with(|s| s.containers.iter().find(|c| c.id == foreign).cloned())
            .unwrap();
    assert!(
        c.running && c.labels.is_empty(),
        "the foreign container is exactly as it was"
    );
    assert!(
        t.h.docker.requests().iter().all(|r| r.method == "GET"),
        "not a single mutating request"
    );
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("state=refused") && s.stdout.contains("ownership not proven"),
        "{}",
        s.stdout
    );
    // and deployments stay blocked
    let a = live_args(&t);
    let o = go(&t, &OPEN, &a);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("hp-leftover"), "{}", o.stderr);
}

#[test]
fn reconcile_apply_removes_only_what_it_can_prove_is_ours() {
    let t = T::new();
    let (app, dep) = reconcile_fixture(&t, DeploymentStatus::Failed);
    let ours = t.h.docker.with(|s| {
        s.add_container(
            "hp-leftover",
            harpoon_deploy::backend::harpoon_labels(&app, &dep),
            false,
        )
    });
    let bystander =
        t.h.docker
            .with(|s| s.add_container("someone-elses", vec![], true));
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("remove container hp-leftover")
            && o.stdout.contains("nothing unresolved"),
        "{}",
        o.stdout
    );
    t.h.docker.with(|s| {
        assert!(!s.containers.iter().any(|c| c.id == ours));
        assert!(s.containers.iter().any(|c| c.id == bystander && c.running));
    });
    let deletes: Vec<_> =
        t.h.docker
            .requests()
            .into_iter()
            .filter(|r| r.method == "DELETE")
            .collect();
    assert_eq!(deletes.len(), 1);
    assert!(deletes[0].path.contains(&ours) && deletes[0].path.contains("force=false"));
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("unresolved cleanup: none") && s.stdout.contains("state=released"),
        "{}",
        s.stdout
    );
    // idempotent
    let n =
        t.h.docker
            .requests()
            .iter()
            .filter(|r| r.method != "GET")
            .count();
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 0);
    assert_eq!(
        t.h.docker
            .requests()
            .iter()
            .filter(|r| r.method != "GET")
            .count(),
        n
    );
}

#[test]
fn an_unreachable_runtime_is_never_read_as_everything_being_gone() {
    let t = T::new();
    reconcile_fixture(&t, DeploymentStatus::Failed);
    let before = t.journal_bytes();
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let mut a = reconcile_args(&t, &["--apply", &auth]);
    a[2] = "/nonexistent/docker.sock".into();
    let o = go(&t, &SHIPPING, &a);
    assert_exit(&o, 5);
    assert!(
        o.stderr.contains("Nothing was assumed absent"),
        "{}",
        o.stderr
    );
    assert_eq!(
        t.journal_bytes(),
        before,
        "no record, not even a failed attempt, was written"
    );
    let mut d = reconcile_args(&t, &[]);
    d[2] = "/nonexistent/docker.sock".into();
    assert_exit(&go(&t, &SHIPPING, &d), 5);
    // listing fails mid-way (Docker answers 500)
    t.h.docker.fault(
        Some("GET"),
        "/containers/json",
        1_000,
        Action::Respond(500, "{\"message\":\"boom\"}".into()),
    );
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 5);
    assert_eq!(t.journal_bytes(), before);
    // it needs an explicit socket
    let o = t.cli(&["reconcile", "--resources-dir", &t.resources()]);
    assert_exit(&o, 5);
    assert!(o.stderr.contains("explicit --docker-socket"));
}

#[test]
fn reconcile_apply_refuses_while_a_deployment_is_unfinished() {
    let t = T::new();
    reconcile_fixture(&t, DeploymentStatus::Running);
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let n = t.docker_requests();
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &["--apply", &auth]));
    assert_exit(&o, 6);
    assert!(o.stderr.contains("recover"), "{}", o.stderr);
    assert_eq!(t.docker_requests(), n);
    // the dry run explains instead of acting
    let o = go(&t, &SHIPPING, &reconcile_args(&t, &[]));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("note: deployment") && o.stdout.contains("is running"),
        "{}",
        o.stdout
    );
}

#[test]
fn nothing_in_this_crate_names_the_live_harpoon_socket() {
    let live = ["/tmp/harpoon-docker", ".sock"].concat();
    for dir in ["src", "tests"] {
        for e in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)).unwrap() {
            let p = e.unwrap().path();
            if p.extension().map(|x| x == "rs").unwrap_or(false) {
                let text = std::fs::read_to_string(&p).unwrap();
                assert!(!text.contains(&live), "{}", p.display());
            }
        }
    }
}

#[test]
fn a_recorded_release_blocks_a_second_first_deployment_for_the_same_application() {
    let t = T::new();
    t.docker_runtime();
    let (app, dep) = ids(31);
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&finished(&app, &dep, DeploymentStatus::Succeeded))
        .unwrap();
    j.record(&intent(
        &app,
        &dep,
        ResourceKind::Container,
        "hp-serving",
        Disposition::RemoveOnFailure,
    ))
    .unwrap();
    j.record(&receipt(&dep, ResourceKind::Container, "hp-serving", "c1"))
        .unwrap();
    drop(j);
    // the finished deployment's serving container is rightly kept: nothing is "unresolved"
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("unresolved cleanup: none"),
        "{}",
        s.stdout
    );
    let mut a = live_args(&t);
    a.extend(["--app".to_string(), app.to_string()]);
    let before = t.journal_bytes();
    let o = go(&t, &OPEN, &a);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("replacing a release is not implemented"),
        "{}",
        o.stderr
    );
    assert_eq!(t.journal_bytes(), before);
    assert!(t.h.docker.requests().iter().all(|r| r.method == "GET"));
    // the plan says the same
    let mut p = vec!["plan".to_string(), "--app".into(), app.to_string()];
    p.extend(t.src());
    let o = go(&t, &SHIPPING, &p);
    assert_exit(&o, 6);
    assert!(o.stdout.contains("deploy` would refuse"), "{}", o.stdout);
}

#[test]
fn plan_reports_a_missing_base_image_and_an_unreachable_runtime_distinctly() {
    let t = T::new();
    std::fs::write(t.h.ws.context().join("Dockerfile"), "FROM nowhere:1\n").unwrap();
    let mut a = vec!["plan".to_string(), "--docker-socket".into(), t.socket()];
    a.extend(t.src());
    let o = go(&t, &SHIPPING, &a);
    assert_exit(&o, 6);
    assert!(
        o.stdout
            .contains("base image nowhere:1 is not present locally"),
        "{}",
        o.stdout
    );
    assert!(t.h.docker.requests().iter().all(|r| r.method == "GET"));
    a[2] = "/nonexistent/docker.sock".into();
    assert_exit(&go(&t, &SHIPPING, &a), 5);
    assert!(!t.rt.exists());
}

#[test]
fn an_invalid_source_stops_every_deploy_before_the_journal_is_touched() {
    let t = T::new();
    for (df, want) in [
        ("../Dockerfile", "plain relative path"),
        ("missing.Dockerfile", "does not exist"),
    ] {
        let mut a = vec![
            "deploy".to_string(),
            "--simulate".into(),
            "--dockerfile".into(),
            df.into(),
        ];
        a.extend(t.src());
        let o = go(&t, &SHIPPING, &a);
        assert_exit(&o, 3);
        assert!(o.stderr.contains(want), "{df}: {}", o.stderr);
    }
    let a = vec![
        "deploy".to_string(),
        "--simulate".into(),
        "--container-port".into(),
        "0".into(),
        "--project-dir".into(),
        t.h.ws.context().display().to_string(),
    ];
    assert_exit(&go(&t, &SHIPPING, &a), 3);
    assert!(
        !t.rt.exists(),
        "no runtime directory was created for an invalid request"
    );
}

#[test]
fn health_paths_with_control_characters_are_rejected_and_normal_paths_kept() {
    let t = T::new();
    for (path, what) in [
        ("/ok\r\nHost: evil", "CR LF"),
        ("/ok\n", "LF"),
        ("/ok\r", "CR"),
        ("/a\tb", "tab"),
        ("/a\0b", "NUL"),
        ("/a\u{7f}b", "DEL"),
        ("/a\u{1b}[0m", "ESC"),
        ("/a\u{85}b", "C1 control"),
        ("/a b", "space"),
    ] {
        for cmd in [vec!["plan"], vec!["deploy", "--simulate"]] {
            let mut a: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
            a.extend(["--health-path".to_string(), path.to_string()]);
            a.extend(t.src());
            let o = go(&t, &SHIPPING, &a);
            assert_exit(&o, 3);
            assert!(
                o.stderr.contains("--health-path") && o.stderr.contains("control"),
                "{what}: {}",
                o.stderr
            );
        }
    }
    assert!(!t.rt.exists(), "a rejected request creates nothing");
    for path in [
        "/",
        "/health",
        "/api/v1/ready",
        "/health?x=1&y=2",
        "/a-b_c.d~e/%20ok",
        "/caf%C3%A9",
    ] {
        let mut a: Vec<String> = vec!["plan".to_string(), "--health-path".into(), path.into()];
        a.extend(t.src());
        let o = go(&t, &SHIPPING, &a);
        assert_exit(&o, 0);
        assert!(
            o.stdout.contains(&format!("GET {path} ")),
            "{path}: {}",
            o.stdout
        );
    }
    let mut a: Vec<String> = vec!["plan".to_string(), "--health-path".into(), "health".into()];
    a.extend(t.src());
    let o = go(&t, &SHIPPING, &a);
    assert_exit(&o, 3);
    assert!(o.stderr.contains("must start with '/'"));
}

// ------------------------------------------------------------------ B7 phase 1: live requirements

#[test]
fn a_live_deployment_names_the_forwarder_process_it_expects_before_anything_else_happens() {
    let t = T::new();
    let mut a = live_args(&t);
    // drop the --forwarder-pid flag and its value
    let i = a.iter().position(|x| x == "--forwarder-pid").unwrap();
    a.drain(i..i + 2);
    let o = go(&t, &OPEN, &a);
    assert_exit(&o, 3);
    assert!(
        o.stderr.contains("--forwarder-pid") && o.stderr.contains("Harpoon app"),
        "{}",
        o.stderr
    );
    for bad in ["0", "1", "abc", "-5", ""] {
        let mut b = live_args(&t);
        let i = b.iter().position(|x| x == "--forwarder-pid").unwrap();
        b[i + 1] = bad.to_string();
        let o = go(&t, &OPEN, &b);
        assert_exit(&o, 3);
    }
    let mut b = live_args(&t);
    b.extend(["--lsof-path".to_string(), "lsof".to_string()]);
    assert_exit(&go(&t, &OPEN, &b), 3);
    assert!(!t.rt.exists(), "nothing was created");
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn a_live_deployment_names_its_guest_channel_and_stops_when_the_guest_is_short_unsure_or_not_the_same(
) {
    // no --mgmt-socket at all: configuration error before anything happens
    let t = T::new();
    let mut a = live_args(&t);
    let i = a.iter().position(|x| x == "--mgmt-socket").unwrap();
    a.drain(i..i + 2);
    let o = go(&t, &OPEN, &a);
    assert_exit(&o, 3);
    assert!(o.stderr.contains("--mgmt-socket"), "{}", o.stderr);
    assert!(!t.rt.exists() && t.docker_requests() == 0);
    let mut rel = live_args(&t);
    let i = rel.iter().position(|x| x == "--mgmt-socket").unwrap();
    rel[i + 1] = "relative.sock".into();
    assert_exit(&go(&t, &OPEN, &rel), 3);

    // a guest with too little room, an impostor, an unreachable channel: first step fails, nothing changes
    for (what, setup, want) in [
        (
            "short",
            Box::new(|m: &mut support::fake_mgmt::Model| {
                m.total = 3_000_000_000;
                m.used = 2_500_000_000;
                m.avail = 500_000_000;
            }) as Box<dyn Fn(&mut support::fake_mgmt::Model)>,
            "MB free in the guest",
        ),
        (
            "impostor",
            Box::new(|m| m.hostname = "someone-else".into()),
            "not the same guest",
        ),
    ] {
        let t = T::new();
        t.mgmt.set(|m| setup(m));
        let o = go(&t, &OPEN, &live_args(&t));
        assert_exit(&o, 7);
        assert!(
            o.stdout.contains("failure at source") && o.stdout.contains(want),
            "{what}: {}",
            o.stdout
        );
        assert!(
            t.h.ws.buildx_record("args.txt").is_empty(),
            "{what}: no build"
        );
        assert!(
            t.h.docker.requests().iter().all(|r| r.method == "GET"),
            "{what}: no Docker change of any kind"
        );
        let s = t.cli(&["status"]);
        assert!(
            s.stdout.contains("failed") && s.stdout.contains("unresolved cleanup: none"),
            "{what}: {}",
            s.stdout
        );
    }
    let t = T::new();
    let mut a = live_args(&t);
    let i = a.iter().position(|x| x == "--mgmt-socket").unwrap();
    a[i + 1] = "/nonexistent/mgmt.sock".into();
    let o = go(&t, &OPEN, &a);
    assert_exit(&o, 7);
    assert!(
        o.stdout
            .contains("cannot reach the guest management channel"),
        "{}",
        o.stdout
    );
}

#[test]
fn a_live_deployment_gives_its_containers_a_marker_environment() {
    let t = T::new();
    image_appears_after_build(&t);
    let a = live_args(&t);
    let o = go(&t, &OPEN, &a);
    assert_exit(&o, 7); // nothing answers on the published port in this test
    let creates: Vec<_> =
        t.h.docker
            .requests()
            .into_iter()
            .filter(|r| r.path.starts_with("/containers/create"))
            .collect();
    assert!(!creates.is_empty());
    for c in creates {
        assert!(c.body.contains("HARPOON_DEPLOYMENT_MARKER="), "{}", c.body);
    }
}

// ------------------------------------------------------------------ inventory

fn fake_lsof(t: &T, out: &str) -> String {
    let p = t.h.ws.root.join("fake-lsof");
    std::fs::write(&p, format!("#!/bin/sh\nprintf '%s' '{out}'\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.display().to_string()
}

fn inv(t: &T, extra: &[&str]) -> Outcome {
    let mut a: Vec<String> = vec!["inventory".into(), "--docker-socket".into(), t.socket()];
    a.extend(extra.iter().map(|s| s.to_string()));
    run(&a, &SHIPPING)
}

#[test]
fn the_inventory_is_read_only_sorted_and_complete() {
    let t = T::new();
    t.h.docker.with(|s| {
        s.add_container("zz-bystander", vec![], true);
        s.containers.last_mut().unwrap().host_port = Some(8081);
        s.add_container(
            "aa-harpoon",
            harpoon_deploy::backend::harpoon_labels(&ids(5).0, &ids(5).1),
            false,
        );
        s.volumes = vec!["vol-b".into(), "vol-a".into()];
    });
    let lsof = fake_lsof(
        &t,
        "p4242\ncHarpoon\nn127.0.0.1:38000\np9\ncother\nn*:80\np9\ncother\nn*:39001\n",
    );
    let o = inv(&t, &["--lsof-path", &lsof]);
    assert_exit(&o, 0);
    let lines: Vec<&str> = o.stdout.lines().collect();
    assert!(lines[0].starts_with("# harpoon-deploy inventory"));
    let body = &lines[1..];
    let mut sorted = body.to_vec();
    sorted.sort_by_key(|l| (l.split(' ').next().unwrap().to_string(), l.to_string()));
    assert_eq!(body, sorted.as_slice(), "sorted: {}", o.stdout);
    for want in [
        "container ",
        "name=aa-harpoon state=exited harpoon=yes",
        "name=zz-bystander state=running harpoon=no published=[8081]",
        "image ",
        "tag=alpine:3.22 harpoon=no",
        "volume vol-a driver=local",
        "volume vol-b driver=local",
        "docker-port-mapping 8081",
        "host-listener 127.0.0.1:38000 pid=4242 command=Harpoon",
        "host-listener *:39001 pid=9 command=other",
    ] {
        assert!(o.stdout.contains(want), "{want}\n{}", o.stdout);
    }
    assert!(
        !o.stdout.contains(":80 "),
        "listeners outside Harpoon's port ranges are not listed"
    );
    assert!(
        t.h.docker.requests().iter().all(|r| r.method == "GET"),
        "read-only"
    );
    assert!(!t.rt.exists());
    // identical twice: no timestamps, no ordering noise
    assert_eq!(inv(&t, &["--lsof-path", &lsof]).stdout, o.stdout);
}

#[test]
fn the_inventory_can_be_saved_once_and_diffed_around_a_run() {
    let t = T::new();
    let before_path = t.h.ws.root.join("before.txt");
    let after_path = t.h.ws.root.join("after.txt");
    let foreign =
        t.h.docker
            .with(|s| s.add_container("bystander", vec![], true));
    let o = inv(&t, &["--to", &before_path.display().to_string()]);
    assert_exit(&o, 0);
    assert_eq!(mode(&before_path), 0o600);
    // never overwrites
    assert_exit(&inv(&t, &["--to", &before_path.display().to_string()]), 4);

    image_appears_after_build(&t);
    let a = live_args(&t);
    assert_exit(&go(&t, &OPEN, &a), 7);
    assert_exit(&inv(&t, &["--to", &after_path.display().to_string()]), 0);
    let (b, a) = (
        std::fs::read_to_string(&before_path).unwrap(),
        std::fs::read_to_string(&after_path).unwrap(),
    );
    let (b, a): (Vec<&str>, Vec<&str>) = (b.lines().collect(), a.lines().collect());
    let added: Vec<_> = a.iter().filter(|l| !b.contains(l)).collect();
    let removed: Vec<_> = b.iter().filter(|l| !a.contains(l)).collect();
    assert!(
        removed.is_empty(),
        "nothing that existed before is gone or changed: {removed:?}"
    );
    assert_eq!(
        added.len(),
        1,
        "exactly one thing is new — the built image, which Harpoon retains: {added:?}"
    );
    assert!(
        added[0].starts_with("image ") && added[0].contains("harpoon=yes"),
        "{added:?}"
    );
    assert!(a
        .iter()
        .any(|l| l.contains("name=bystander state=running harpoon=no")));
    assert!(t
        .h
        .docker
        .with(|s| s.containers.iter().any(|c| c.id == foreign && c.running)));
}

#[test]
fn the_inventory_needs_an_explicit_socket_and_reports_an_unreachable_one() {
    let t = T::new();
    assert_exit(&run(&["inventory".to_string()], &SHIPPING), 5);
    assert_exit(
        &run(
            &[
                "inventory".to_string(),
                "--docker-socket".into(),
                "/nonexistent/docker.sock".into(),
            ],
            &SHIPPING,
        ),
        5,
    );
    assert_exit(&inv(&t, &["--lsof-path", "lsof"]), 2);
    // a failing lsof is an error, not an empty listener list
    let bad = fake_lsof(&t, "what is this");
    let o = inv(&t, &["--lsof-path", &bad]);
    assert_exit(&o, 5);
    assert!(
        o.stderr.contains("cannot capture host listeners"),
        "{}",
        o.stderr
    );
}

// ------------------------------------------------------------------ B7 final: teardown

struct Done {
    app: ApplicationId,
    dep: DeploymentId,
    cid: String,
    img: String,
    tag: String,
}

/// A docker-kind runtime holding one SUCCEEDED deployment whose serving container is running in the
/// fake Docker, together with its retained image and its application-port lease.
fn succeeded_deployment(t: &T) -> Done {
    t.docker_runtime();
    let (app, dep) = ids(41);
    let tag = format!(
        "harpoon/{}:{}-0123456789abcdef0123456789abcdef",
        app.as_str().to_ascii_lowercase(),
        dep
    );
    let img = support::fake_iid(7);
    let name = format!("hp-{dep}");
    let lbl = harpoon_deploy::backend::harpoon_labels(&app, &dep);
    let (cid, img2) = t.h.docker.with(|s| {
        s.add_image_with_id(&img, &[&tag], lbl.clone(), vec![]);
        let cid = s.add_container(&name, lbl.clone(), true);
        let c = s.containers.last_mut().unwrap();
        c.host_port = Some(38000);
        c.image = img.clone();
        (cid, img.clone())
    });
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&finished(&app, &dep, DeploymentStatus::Succeeded))
        .unwrap();
    j.record(&intent(
        &app,
        &dep,
        ResourceKind::Image,
        &tag,
        Disposition::Retain,
    ))
    .unwrap();
    j.record(&receipt(&dep, ResourceKind::Image, &tag, &img2))
        .unwrap();
    j.record(&intent(
        &app,
        &dep,
        ResourceKind::PortLease,
        "38000",
        Disposition::RemoveOnFailure,
    ))
    .unwrap();
    j.record(&receipt(&dep, ResourceKind::PortLease, "38000", "38000"))
        .unwrap();
    j.record(&intent(
        &app,
        &dep,
        ResourceKind::Container,
        &name,
        Disposition::RemoveOnFailure,
    ))
    .unwrap();
    j.record(&receipt(&dep, ResourceKind::Container, &name, &cid))
        .unwrap();
    Done {
        app,
        dep,
        cid,
        img: img2,
        tag,
    }
}

fn td_args(t: &T, d: &Done, extra: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = vec![
        "teardown".into(),
        "--deployment".into(),
        d.dep.to_string(),
        "--docker-socket".into(),
        t.socket(),
        "--resources-dir".into(),
        t.resources(),
    ];
    v.extend(extra.iter().map(|s| s.to_string()));
    v
}

fn apply_args(t: &T, d: &Done) -> Vec<String> {
    td_args(t, d, &["--apply", &format!("--{AUTH_FLAG}={AUTH_PHRASE}")])
}

fn mutating(t: &T) -> Vec<(String, String)> {
    t.h.docker
        .requests()
        .into_iter()
        .filter(|r| r.method != "GET")
        .map(|r| (r.method, r.path))
        .collect()
}

fn entry_state(t: &T, d: &Done, kind: ResourceKind, name: &str) -> String {
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    let e = r
        .ledger
        .find(&d.dep, &ResourceKey::new(kind, name))
        .unwrap();
    format!("{:?}", e.state)
}

#[test]
fn teardown_is_a_dry_run_by_default_and_needs_the_exact_opt_in_to_change_anything() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    let before = t.journal_bytes();
    let o = go(&t, &SHIPPING, &td_args(&t, &d, &[]));
    assert_exit(&o, 0);
    for want in [
        "dry run: nothing will be changed",
        &format!("would: remove container hp-{} (id {})", d.dep, d.cid),
        "would: remove image",
        "only once its containers are gone and no other container uses it",
        "would: release port lease 38000 only once the containers are verified absent",
        "dry run: nothing was changed",
    ] {
        assert!(o.stdout.contains(want), "{want}\n{}", o.stdout);
    }
    assert!(mutating(&t).is_empty(), "a dry run only reads");
    assert_eq!(t.journal_bytes(), before);

    // --apply without the exact phrase is refused before Docker is contacted
    let n = t.docker_requests();
    let mut a = td_args(&t, &d, &["--apply"]);
    assert_exit(&go(&t, &SHIPPING, &a), 6);
    a.push(format!("--{AUTH_FLAG}=yes"));
    assert_exit(&go(&t, &SHIPPING, &a), 6);
    assert_eq!(t.docker_requests(), n);
    assert_eq!(t.journal_bytes(), before);
}

#[test]
fn teardown_removes_the_container_then_the_image_then_releases_the_lease_each_by_verified_id() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    let foreign_c =
        t.h.docker
            .with(|s| s.add_container("bystander", vec![], true));
    let foreign_i = t.h.docker.with(|s| {
        s.add_image_with_id(&support::fake_iid(99), &["redis:7"], vec![], vec![]);
        support::fake_iid(99)
    });
    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 0);
    assert!(o.stdout.contains("teardown: complete"), "{}", o.stdout);

    let m = mutating(&t);
    let stop = m
        .iter()
        .position(|(me, p)| me == "POST" && p.contains(&format!("{}/stop", d.cid)))
        .expect("stopped first");
    let del_c = m
        .iter()
        .position(|(me, p)| me == "DELETE" && p.starts_with(&format!("/containers/{}", d.cid)))
        .expect("container removed");
    let del_i = m
        .iter()
        .position(|(me, p)| me == "DELETE" && p.starts_with(&format!("/images/{}", d.img)))
        .expect("image removed");
    assert!(
        stop < del_c && del_c < del_i,
        "stop, remove container, remove image: {m:?}"
    );
    assert_eq!(m.len(), 3, "nothing else changed: {m:?}");
    assert!(
        m[del_c].1.contains("force=false") && m[del_c].1.contains("v=false"),
        "{:?}",
        m[del_c]
    );
    assert!(
        m[del_i].1.contains("force=false") && m[del_i].1.contains("noprune=true"),
        "{:?}",
        m[del_i]
    );
    assert!(
        m.iter()
            .all(|(_, p)| !p.contains("prune=") || p.contains("noprune=true"))
            && m.iter()
                .all(|(_, p)| !p.contains("harpoon/") && !p.contains(&d.tag)),
        "never by tag or name: {m:?}"
    );

    // the journal says all three are released, and records an attempt and a result for each
    for (kind, name) in [
        (ResourceKind::Container, format!("hp-{}", d.dep)),
        (ResourceKind::Image, d.tag.clone()),
        (ResourceKind::PortLease, "38000".to_string()),
    ] {
        assert!(
            entry_state(&t, &d, kind, &name).starts_with("Released"),
            "{kind:?}"
        );
    }
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    for e in r.ledger.entries().iter().filter(|e| e.deployment == d.dep) {
        assert_eq!(e.cleanup_attempts, 1, "{}", e.key.name);
        assert!(!e.cleanup_outstanding);
    }
    assert!(r.outstanding_leases().is_empty());
    // foreign things untouched
    t.h.docker.with(|s| {
        assert!(s.containers.iter().any(|c| c.id == foreign_c && c.running));
        assert!(s.images.iter().any(|i| i.id == foreign_i));
        assert!(
            !s.containers.iter().any(|c| c.id == d.cid) && !s.images.iter().any(|i| i.id == d.img)
        );
    });
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("unresolved cleanup: none"),
        "{}",
        s.stdout
    );
}

#[test]
fn a_repeated_teardown_changes_nothing_further() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    assert_exit(&go(&t, &SHIPPING, &apply_args(&t, &d)), 0);
    let (bytes, n) = (t.journal_bytes(), mutating(&t).len());
    for args in [apply_args(&t, &d), td_args(&t, &d, &[])] {
        let o = go(&t, &SHIPPING, &args);
        assert_exit(&o, 0);
        assert!(o.stdout.contains("nothing to do"), "{}", o.stdout);
    }
    assert_eq!(t.journal_bytes(), bytes);
    assert_eq!(mutating(&t).len(), n);
}

#[test]
fn a_failed_removal_is_recorded_leaves_everything_else_alone_and_is_retried_by_the_next_run() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    t.h.docker.fault(
        Some("DELETE"),
        "/containers/",
        1,
        Action::Respond(500, "{\"message\":\"driver failed\"}".into()),
    );
    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("teardown: incomplete") && o.stdout.contains("driver failed"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("the image and the port leases were left alone"),
        "{}",
        o.stdout
    );
    // the container was stopped but not removed; the image and the lease are exactly as they were
    t.h.docker.with(|s| {
        assert!(s.containers.iter().any(|c| c.id == d.cid), "still there");
        assert!(s.images.iter().any(|i| i.id == d.img));
    });
    assert!(!mutating(&t)
        .iter()
        .any(|(me, p)| me == "DELETE" && p.starts_with("/images/")));
    assert!(
        entry_state(&t, &d, ResourceKind::Container, &format!("hp-{}", d.dep))
            .contains("CleanupFailed")
    );
    assert!(entry_state(&t, &d, ResourceKind::PortLease, "38000").contains("Created"));
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("state=cleanup-failed") && s.stdout.contains("unresolved cleanup (1)"),
        "{}",
        s.stdout
    );

    // the next run retries only what is left, and finishes
    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 0);
    assert!(o.stdout.contains("teardown: complete"), "{}", o.stdout);
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    let c = r
        .ledger
        .find(
            &d.dep,
            &ResourceKey::new(ResourceKind::Container, format!("hp-{}", d.dep)),
        )
        .unwrap();
    assert_eq!(
        c.cleanup_attempts, 2,
        "the failed attempt is kept in the record"
    );
    assert!(r
        .ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == d.dep)
        .all(|e| e.is_released()));
}

#[test]
fn an_interrupted_removal_attempt_is_retried_and_not_forgotten() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    // a previous run recorded its intent to remove the container and died before the result
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.record(&LedgerRecord::CleanupAttempt {
        key: ResourceKey::new(ResourceKind::Container, format!("hp-{}", d.dep)),
        deployment: d.dep.clone(),
        at_ms: 9,
    })
    .unwrap();
    drop(j);
    let s = t.cli(&["status"]);
    assert!(
        s.stdout
            .contains("attempted and its result was never recorded"),
        "{}",
        s.stdout
    );
    assert_exit(&go(&t, &SHIPPING, &apply_args(&t, &d)), 0);
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    assert!(r.cleanup_problems().is_empty());
}

#[test]
fn a_container_that_is_not_provably_the_recorded_one_is_never_touched_and_stays_refused() {
    for (what, relabel) in [
        (
            "labels differ",
            Box::new(|c: &mut support::fake_docker::Ctr| {
                c.labels = vec![("io.harpoon.managed".into(), "true".into())]
            }) as Box<dyn Fn(&mut support::fake_docker::Ctr)>,
        ),
        ("no labels at all", Box::new(|c| c.labels = vec![])),
    ] {
        let t = T::new();
        let d = succeeded_deployment(&t);
        t.h.docker
            .with(|s| relabel(s.containers.iter_mut().find(|c| c.id == d.cid).unwrap()));
        let o = go(&t, &SHIPPING, &apply_args(&t, &d));
        assert_exit(&o, 7);
        assert!(
            o.stdout.contains("REFUSE container") && o.stdout.contains("not touched"),
            "{what}: {}",
            o.stdout
        );
        assert!(
            mutating(&t).is_empty(),
            "{what}: not a single change to Docker: {:?}",
            mutating(&t)
        );
        assert!(
            entry_state(&t, &d, ResourceKind::Container, &format!("hp-{}", d.dep))
                .contains("Refused")
        );
        // image and lease untouched; and a second run refuses to go on
        assert!(
            entry_state(&t, &d, ResourceKind::Image, &d.tag).contains("Created"),
            "{what}"
        );
        let before = t.journal_bytes();
        let o = go(&t, &SHIPPING, &apply_args(&t, &d));
        assert_exit(&o, 6);
        assert!(
            o.stderr.contains("refused earlier") && o.stderr.contains("will not touch it"),
            "{what}: {}",
            o.stderr
        );
        assert_eq!(t.journal_bytes(), before);
        assert!(mutating(&t).is_empty());
    }
}

#[test]
fn a_same_named_container_with_a_different_id_is_refused_not_adopted() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    // the recorded container vanished and something else took its name
    t.h.docker.with(|s| {
        s.containers.retain(|c| c.id != d.cid);
        s.add_container(
            &format!("hp-{}", d.dep),
            harpoon_deploy::backend::harpoon_labels(&d.app, &d.dep),
            true,
        );
    });
    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("its id is not the recorded"),
        "{}",
        o.stdout
    );
    assert!(mutating(&t).is_empty());
    assert!(t.h.docker.with(|s| s
        .containers
        .iter()
        .any(|c| c.name == format!("hp-{}", d.dep) && c.running)));
}

#[test]
fn a_container_already_gone_is_recorded_absent_and_the_rest_proceeds() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    t.h.docker.with(|s| s.containers.retain(|c| c.id != d.cid));
    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains("is not in the runtime: record it as absent"),
        "{}",
        o.stdout
    );
    assert!(
        entry_state(&t, &d, ResourceKind::Container, &format!("hp-{}", d.dep))
            .contains("AlreadyAbsent")
    );
    assert!(entry_state(&t, &d, ResourceKind::Image, &d.tag).starts_with("Released"));
    assert!(entry_state(&t, &d, ResourceKind::PortLease, "38000").starts_with("Released"));
}

#[test]
fn an_unreachable_or_failing_runtime_changes_nothing_and_assumes_nothing() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    let before = t.journal_bytes();
    for apply in [false, true] {
        let mut a = if apply {
            apply_args(&t, &d)
        } else {
            td_args(&t, &d, &[])
        };
        a[4] = "/nonexistent/docker.sock".into();
        let o = go(&t, &SHIPPING, &a);
        assert_exit(&o, 5);
        assert!(
            o.stderr.contains("Nothing was assumed absent"),
            "{}",
            o.stderr
        );
    }
    t.h.docker.fault(
        Some("GET"),
        "/containers/json",
        1_000,
        Action::Respond(500, "{\"message\":\"boom\"}".into()),
    );
    assert_exit(&go(&t, &SHIPPING, &apply_args(&t, &d)), 5);
    assert_eq!(
        t.journal_bytes(),
        before,
        "not even a failed attempt was recorded"
    );
    assert!(mutating(&t).is_empty());
    // a socket is mandatory
    let mut a = apply_args(&t, &d);
    a.drain(3..5);
    assert_exit(&go(&t, &SHIPPING, &a), 5);
}

#[test]
fn only_a_succeeded_recorded_deployment_can_be_torn_down() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    // unknown id
    let (_, other) = ids(77);
    let mut a = apply_args(&t, &d);
    a[2] = other.to_string();
    let o = go(&t, &SHIPPING, &a);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("is not in the journal"));
    // not an id at all
    a[2] = "hp-whatever".into();
    assert_exit(&go(&t, &SHIPPING, &a), 2);
    assert_exit(&go(&t, &SHIPPING, &["teardown".to_string()]), 2);
    // failed and running deployments are for reconcile/recover
    for (st, want) in [
        (DeploymentStatus::Failed, "failed"),
        (DeploymentStatus::Running, "running"),
    ] {
        let t2 = T::new();
        t2.docker_runtime();
        let (app, dep) = ids(55);
        let (mut j, _) = open_j(&t2.journal_dir()).unwrap();
        j.persist(&finished(&app, &dep, st)).unwrap();
        drop(j);
        let dd = Done {
            app,
            dep,
            cid: String::new(),
            img: String::new(),
            tag: String::new(),
        };
        let o = go(&t2, &SHIPPING, &apply_args(&t2, &dd));
        assert_exit(&o, 6);
        assert!(
            o.stderr.contains(want) && o.stderr.contains("succeeded"),
            "{}",
            o.stderr
        );
        assert!(mutating(&t2).is_empty());
    }
    // a simulated runtime has nothing real to tear down; a runtime with no journal has nothing recorded
    let t3 = T::new();
    assert_exit(&t3.sim_deploy(), 0);
    let dd = Done {
        app: d.app.clone(),
        dep: d.dep.clone(),
        cid: String::new(),
        img: String::new(),
        tag: String::new(),
    };
    assert_exit(&go(&t3, &SHIPPING, &apply_args(&t3, &dd)), 6);
    let t4 = T::new();
    assert_exit(&go(&t4, &SHIPPING, &apply_args(&t4, &dd)), 6);
}

#[test]
fn an_image_something_else_still_uses_is_kept_and_a_port_docker_still_maps_keeps_its_lease() {
    // image in use by a foreign container
    let t = T::new();
    let d = succeeded_deployment(&t);
    let user = t.h.docker.with(|s| {
        let id = s.add_container("someone-elses-app", vec![], true);
        s.containers.last_mut().unwrap().image = support::fake_iid(7);
        id
    });
    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("kept: image") && o.stdout.contains("someone-elses-app"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("an image was kept"), "{}", o.stdout);
    t.h.docker.with(|s| {
        assert!(
            s.images.iter().any(|i| i.id == d.img),
            "the shared image stays"
        );
        assert!(
            s.containers.iter().any(|c| c.id == user && c.running),
            "and so does its user"
        );
    });
    assert!(entry_state(&t, &d, ResourceKind::Image, &d.tag).contains("Created"));
    assert!(entry_state(&t, &d, ResourceKind::PortLease, "38000").starts_with("Released"));
    // repeating it is stable
    let bytes = t.journal_bytes();
    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 0);
    assert!(o.stdout.contains("kept: image"));
    assert_eq!(
        mutating(&t).len(),
        2,
        "stop + container removal only; no further change"
    );
    let _ = bytes;

    // an image whose users cannot be identified is kept too
    let t2 = T::new();
    let d2 = succeeded_deployment(&t2);
    t2.h.docker.with(|s| {
        s.add_container("mystery", vec![], false);
        s.containers.last_mut().unwrap().image = String::new();
    });
    let o = go(&t2, &SHIPPING, &apply_args(&t2, &d2));
    assert_exit(&o, 0);
    assert!(o.stdout.contains("mystery (image unknown)"), "{}", o.stdout);
    assert!(t2
        .h
        .docker
        .with(|s| s.images.iter().any(|i| i.id == d2.img)));

    // a port that Docker still maps (a foreign container publishes it) is not released
    let t3 = T::new();
    let d3 = succeeded_deployment(&t3);
    t3.h.docker
        .fault(Some("DELETE"), "/containers/", 1, Action::Disconnect);
    let o = go(&t3, &SHIPPING, &apply_args(&t3, &d3));
    assert_exit(&o, 7);
    assert!(
        entry_state(&t3, &d3, ResourceKind::PortLease, "38000").contains("Created"),
        "the lease stays while the container is not verified gone"
    );
}

#[test]
fn teardown_does_not_touch_other_deployments_of_the_same_application() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    // a second deployment of the same app, also succeeded, with its own container
    let (_, dep2) = ids(88);
    let name2 = format!("hp-{dep2}");
    let lbl2 = harpoon_deploy::backend::harpoon_labels(&d.app, &dep2);
    let cid2 = t.h.docker.with(|s| {
        let id = s.add_container(&name2, lbl2, true);
        s.containers.last_mut().unwrap().host_port = Some(38001);
        id
    });
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&finished(&d.app, &dep2, DeploymentStatus::Succeeded))
        .unwrap();
    j.record(&intent(
        &d.app,
        &dep2,
        ResourceKind::Container,
        &name2,
        Disposition::RemoveOnFailure,
    ))
    .unwrap();
    j.record(&receipt(&dep2, ResourceKind::Container, &name2, &cid2))
        .unwrap();
    drop(j);
    assert_exit(&go(&t, &SHIPPING, &apply_args(&t, &d)), 0);
    assert!(
        t.h.docker
            .with(|s| s.containers.iter().any(|c| c.id == cid2 && c.running)),
        "the other deployment's container is untouched"
    );
    assert!(mutating(&t).iter().all(|(_, p)| !p.contains(&cid2)));
    let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
    assert!(!r
        .ledger
        .find(&dep2, &ResourceKey::new(ResourceKind::Container, &name2))
        .unwrap()
        .is_released());
}

#[test]
fn doctor_reports_guest_free_space_through_the_management_channel_read_only() {
    let t = T::new();
    let src = t.src();
    let mut a: Vec<String> = vec![
        "doctor".into(),
        "--docker-socket".into(),
        t.socket(),
        "--mgmt-socket".into(),
        t.mgmt.path().display().to_string(),
    ];
    a.extend(src);
    let o = go(&t, &SHIPPING, &a);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("[ok]   guest free space")
            && o.stdout.contains("read-only: hostname, uname -r, df"),
        "{}",
        o.stdout
    );
    assert_eq!(
        t.mgmt.requests(),
        ["hostname", "uname -r", "df -B1 /var/lib/docker"]
    );
    assert!(t.h.docker.requests().iter().all(|r| r.method == "GET"));
    t.mgmt.set(|m| {
        m.total = 1_000_000_000;
        m.used = 900_000_000;
        m.avail = 100_000_000;
    });
    let o = go(&t, &SHIPPING, &a);
    assert_exit(&o, 5);
    assert!(
        o.stdout.contains("[FAIL] guest free space") && o.stdout.contains("MB required"),
        "{}",
        o.stdout
    );
}

#[test]
fn a_lease_is_kept_while_docker_still_maps_its_port() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    // after our container is gone, something else is found publishing the same host port
    t.h.docker.fault(
        Some("DELETE"),
        "/containers/",
        1,
        Action::Race(std::sync::Arc::new(|s| {
            s.add_container("squatter", vec![], true);
            s.containers.last_mut().unwrap().host_port = Some(38000);
        })),
    );
    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 7);
    assert!(
        o.stdout
            .contains("port 38000 is still mapped by a container; lease kept"),
        "{}",
        o.stdout
    );
    assert!(
        entry_state(&t, &d, ResourceKind::Container, &format!("hp-{}", d.dep))
            .starts_with("Released")
    );
    assert!(entry_state(&t, &d, ResourceKind::PortLease, "38000").contains("Created"));
    assert!(
        t.h.docker.with(|s| s
            .containers
            .iter()
            .any(|c| c.name == "squatter" && c.running)),
        "and the squatter is untouched"
    );
    // once the port is free, the next run releases the lease
    t.h.docker
        .with(|s| s.containers.retain(|c| c.name != "squatter"));
    assert_exit(&go(&t, &SHIPPING, &apply_args(&t, &d)), 0);
    assert!(entry_state(&t, &d, ResourceKind::PortLease, "38000").starts_with("Released"));
}

#[test]
fn a_lease_stays_while_its_container_is_not_verified_gone_even_if_docker_shows_no_mapping() {
    let t = T::new();
    let d = succeeded_deployment(&t);
    // Docker reports nothing published for the container, so only the container's own state can keep the lease
    t.h.docker.with(|s| {
        s.containers
            .iter_mut()
            .find(|c| c.id == d.cid)
            .unwrap()
            .host_port = None
    });
    t.h.docker.fault(
        Some("DELETE"),
        "/containers/",
        1,
        Action::Respond(500, "{\"message\":\"busy\"}".into()),
    );
    let o = go(&t, &SHIPPING, &apply_args(&t, &d));
    assert_exit(&o, 7);
    assert!(
        entry_state(&t, &d, ResourceKind::Container, &format!("hp-{}", d.dep))
            .contains("CleanupFailed")
    );
    assert!(
        entry_state(&t, &d, ResourceKind::PortLease, "38000").contains("Created"),
        "the lease is not released on a guess"
    );
    assert!(entry_state(&t, &d, ResourceKind::Image, &d.tag).contains("Created"));
}

#[test]
fn the_first_container_that_does_not_come_out_clean_stops_the_rest() {
    for (what, trouble) in [("a failed removal", 0), ("a refusal", 1)] {
        let t = T::new();
        let d = succeeded_deployment(&t);
        // a second recorded container of the same deployment (a replacement), journaled after the first
        let name2 = format!("hp-{}-r2", d.dep);
        let lbl = harpoon_deploy::backend::harpoon_labels(&d.app, &d.dep);
        let cid2 = t.h.docker.with(|s| {
            let id = s.add_container(&name2, lbl, true);
            s.containers.last_mut().unwrap().image = d.img.clone();
            id
        });
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.record(&intent(
            &d.app,
            &d.dep,
            ResourceKind::Container,
            &name2,
            Disposition::RemoveOnFailure,
        ))
        .unwrap();
        j.record(&receipt(&d.dep, ResourceKind::Container, &name2, &cid2))
            .unwrap();
        drop(j);
        if trouble == 0 {
            t.h.docker.fault(
                Some("DELETE"),
                "/containers/",
                1,
                Action::Respond(500, "{\"message\":\"driver failed\"}".into()),
            );
        } else {
            t.h.docker.with(|s| {
                s.containers
                    .iter_mut()
                    .find(|c| c.id == d.cid)
                    .unwrap()
                    .labels = vec![]
            });
        }
        let o = go(&t, &SHIPPING, &apply_args(&t, &d));
        assert_exit(&o, 7);
        assert!(
            o.stdout
                .contains(&format!("not attempted: container {name2}")),
            "{what}: {}",
            o.stdout
        );
        // the second container: not stopped, not removed, not even looked at by a removal — and no journal attempt
        assert!(
            mutating(&t).iter().all(|(_, p)| !p.contains(&cid2)),
            "{what}: {:?}",
            mutating(&t)
        );
        assert!(
            t.h.docker
                .with(|s| s.containers.iter().any(|c| c.id == cid2 && c.running)),
            "{what}"
        );
        let r = SqliteJournal::read_only(&t.journal_dir()).unwrap();
        let e2 = r
            .ledger
            .find(&d.dep, &ResourceKey::new(ResourceKind::Container, &name2))
            .unwrap();
        assert_eq!(
            e2.cleanup_attempts, 0,
            "{what}: no attempt was recorded for it"
        );
        assert!(!e2.is_released());
        // image and lease untouched as before
        assert!(
            entry_state(&t, &d, ResourceKind::Image, &d.tag).contains("Created"),
            "{what}"
        );
        assert!(
            entry_state(&t, &d, ResourceKind::PortLease, "38000").contains("Created"),
            "{what}"
        );
        if trouble == 0 {
            // the next run retries from the failed one and then continues
            let o = go(&t, &SHIPPING, &apply_args(&t, &d));
            assert_exit(&o, 0);
            assert!(t
                .h
                .docker
                .with(|s| !s.containers.iter().any(|c| c.id == cid2 || c.id == d.cid)));
        }
    }
}

#[test]
fn the_cli_sources_never_call_a_prune_endpoint_or_force_a_removal() {
    // the same prohibition the CI scan makes, as a test: operations, not the word in a message
    for e in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src")).unwrap() {
        let p = e.unwrap().path();
        let text = std::fs::read_to_string(&p).unwrap();
        for bad in ["/prune", "force=true", "Command::new", "env::var"] {
            assert!(!text.contains(bad), "{} contains {bad}", p.display());
        }
    }
}

#[test]
fn the_fork_gate_never_lets_a_spawn_overlap_an_open_journal() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static OPEN_NOW: AtomicUsize = AtomicUsize::new(0);
    static SPAWNING_NOW: AtomicUsize = AtomicUsize::new(0);
    let threads: Vec<_> = (0..6)
        .map(|n| {
            std::thread::spawn(move || {
                for i in 0..200 {
                    if (n + i) % 5 == 0 {
                        spawning(|| {
                            SPAWNING_NOW.fetch_add(1, Ordering::SeqCst);
                            assert_eq!(
                                OPEN_NOW.load(Ordering::SeqCst),
                                0,
                                "a spawn ran while a journal was open"
                            );
                            assert_eq!(
                                SPAWNING_NOW.load(Ordering::SeqCst),
                                1,
                                "two spawns overlapped"
                            );
                            std::thread::yield_now();
                            SPAWNING_NOW.fetch_sub(1, Ordering::SeqCst);
                        });
                    } else {
                        let _h = holding();
                        let _nested = holding(); // a holder may hold again without waiting
                        OPEN_NOW.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(
                            SPAWNING_NOW.load(Ordering::SeqCst),
                            0,
                            "a journal was opened during a spawn"
                        );
                        std::thread::yield_now();
                        OPEN_NOW.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

// B8.2: `cleanup-image` (offline; simulated Docker only).
#[path = "b8_2/mod.rs"]
mod b8_2;

// Health-gated replacement (offline; simulated only).
#[path = "b9/mod.rs"]
mod b9;

// Bounded cutover recovery (offline; simulated only).
#[path = "b11/mod.rs"]
mod b11;

// Recovery hardening (offline; simulated Docker only).
#[path = "b12/mod.rs"]
mod b12;

// Explicit manual rollback (offline; simulated Docker only).
#[path = "rollback/mod.rs"]
mod rollback;

// Criterion 7: state-loss visibility (offline; fake Docker only).
#[path = "state_loss/mod.rs"]
mod state_loss;

// B13: non-secret runtime variables and preliminary-failure output (offline; fake Docker only).
#[path = "b13/mod.rs"]
mod b13;

// B14: durable pauses, held events and explicit resume (offline; fake Docker only to prove it is untouched).
#[path = "b14/mod.rs"]
mod b14;

// B15: live replace, rollback and restore through the acceptance gate against the fake Docker, and
// the packaging boundary (offline; fake Docker only).
#[path = "b15/mod.rs"]
mod b15;
