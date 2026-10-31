//! B6: the CLI end to end, in-process, on temporary directories, with the deploy-docker fake Docker
//! (a test-owned Unix socket) and fake Buildx. Nothing contacts a live Docker endpoint.

#[path = "../../deploy-docker/tests/support/mod.rs"]
mod support;

use harpoon_deploy::engine::Journal;
use harpoon_deploy::ids::{ApplicationId, DeploymentId, IdGenerator};
use harpoon_deploy::ledger::{Disposition, LedgerRecord, ResourceKey, ResourceKind};
use harpoon_deploy::state::{Deployment, DeploymentStatus, Trigger};
use harpoon_deploy::time::{ManualClock, SeqEntropy};
use harpoon_deploy_cli::{run, Gate, Outcome, AUTH_FLAG, AUTH_PHRASE};
use harpoon_deploy_journal::{read_only, FileJournal};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use support::fake_docker::Action;
use support::Harness;

const OPEN: Gate = Gate {
    live_deploy_enabled: true,
    guest_free_override: Some(100 << 30),
};
/// The gate open but, as in a real build today, with no guest free-space probe.
const OPEN_NO_PROBE: Gate = Gate {
    live_deploy_enabled: true,
    guest_free_override: None,
};
const SHIPPING: Gate = Gate::shipping();

struct T {
    h: Harness,
    rt: PathBuf,
}

impl T {
    fn new() -> T {
        let h = Harness::new();
        let rt = h.ws.root.join("runtime");
        T { h, rt }
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
        std::fs::read(self.journal_dir().join("journal.wal")).unwrap()
    }
    fn docker_requests(&self) -> usize {
        self.h.docker.requests().len()
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
    assert_eq!(mode(&t.journal_dir().join("journal.wal")), 0o600);
    assert_eq!(mode(&t.journal_dir().join("journal.lock")), 0o600);
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
        meta(t.journal_dir().join("journal.wal")),
        meta(t.journal_dir().join("journal.lock")),
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
    // a torn tail is shown, ignored and left alone
    let mut torn = bytes.clone();
    torn.extend_from_slice(b"deadbeef\tsnap\t1\tpart");
    std::fs::write(t.journal_dir().join("journal.wal"), &torn).unwrap();
    let m1 = if torn != bytes {
        meta(t.journal_dir().join("journal.wal"))
    } else {
        m1
    };
    for cmd in [vec!["status"], vec!["doctor"]] {
        let o = t.cli(&cmd);
        assert_exit(&o, 0);
    }
    let o = t.cli(&["status"]);
    assert!(o.stdout.contains("incomplete write"), "{}", o.stdout);
    assert_eq!(
        std::fs::read(t.journal_dir().join("journal.wal")).unwrap(),
        torn
    );
    assert_eq!(meta(t.journal_dir().join("journal.wal")), m1);
    assert_eq!(meta(t.journal_dir().join("journal.lock")), m2);
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
    let r = read_only(&t.journal_dir()).unwrap();
    let mut ports = r.outstanding_leases();
    ports.sort();
    assert_eq!(ports, vec![39000, 39001]);

    // a docker runtime that already holds 39000/38000 makes the plan pick the next ports
    let t2 = T::new();
    t2.docker_runtime();
    let (app, dep) = ids(3);
    let (mut j, _) = FileJournal::open(&t2.journal_dir()).unwrap();
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
    let (mut j, _) = FileJournal::open(&t.journal_dir()).unwrap();
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

// ------------------------------------------------------------------ corrupt journal, lock contention, permissions

#[test]
fn a_corrupt_journal_fails_closed_with_actionable_output_and_can_be_exported() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let mut bytes = t.journal_bytes();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x01;
    std::fs::write(t.journal_dir().join("journal.wal"), &bytes).unwrap();
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
            text.contains("journal line")
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
    let copy = t.h.ws.root.join("copy.wal");
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
    let (_held, _) = FileJournal::open(&t.journal_dir()).unwrap();
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
    drop(FileJournal::open(&t3.journal_dir()).unwrap());
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
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_harpoon-deploy"))
        .args([
            "deploy",
            "--runtime-dir",
            &t.rt.display().to_string(),
            "--docker-socket",
            &sock,
            "--resources-dir",
            &res,
            &auth,
        ])
        .args([
            "--project-dir",
            &t.h.ws.context().display().to_string(),
            "--container-port",
            "8080",
        ])
        .env("DOCKER_HOST", "unix:///nonexistent")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&out.stderr).contains("disabled in this build"));
    assert!(!t.rt.exists());
    assert_eq!(t.docker_requests(), 0);
    let help = std::process::Command::new(env!("CARGO_BIN_EXE_harpoon-deploy"))
        .arg("help")
        .output()
        .unwrap();
    assert_eq!(help.status.code(), Some(0));
    let none = std::process::Command::new(env!("CARGO_BIN_EXE_harpoon-deploy"))
        .output()
        .unwrap();
    assert_eq!(none.status.code(), Some(2));
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
    assert_eq!(mode(&t.journal_dir().join("journal.wal")), 0o600);
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
    let (mut j, _) = FileJournal::open(&t.journal_dir()).unwrap();
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
    let (mut j, _) = FileJournal::open(&t.journal_dir()).unwrap();
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
fn without_a_guest_free_space_probe_a_live_deployment_stops_at_its_first_step() {
    let t = T::new();
    let a = live_args(&t);
    let o = go(&t, &OPEN_NO_PROBE, &a);
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("failure at source")
            && o.stdout.contains("guest free space could not be verified"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("no guest-side free-space probe is implemented"),
        "{}",
        o.stdout
    );
    assert!(t.h.ws.buildx_record("args.txt").is_empty(), "no build");
    assert!(
        t.h.docker.requests().iter().all(|r| r.method == "GET"),
        "no Docker change of any kind"
    );
    // and the journal says what happened
    let s = t.cli(&["status"]);
    assert!(
        s.stdout.contains("failed") && s.stdout.contains("unresolved cleanup: none"),
        "{}",
        s.stdout
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
