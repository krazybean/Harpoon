//! Milestone D through the real CLI: a selected Git commit becomes the right checkout, Buildx request,
//! deployment record and release identity; polling, signed webhooks, CI triggers, duplicate suppression,
//! newest-wins supersession, a paused application, failed checkouts, credentials and cleanup.
//!
//! Offline: real `git` against local repositories and a loopback smart-HTTP server (Basic auth), the fake
//! Docker and Buildx, a fake Keychain. Attended only: nothing here runs in the background. The SSH path is
//! covered by command-construction tests in `deploy-git` (no SSH server is available offline); real
//! GitHub or GitLab deliveries are not exercised.

use super::milestone_c::running_app_ports;
use super::*;
use crate::gitsupport::{fixture, git, HttpGit, Tmp};
use harpoon_deploy::secrets::MemorySecretStore;
use harpoon_deploy_cli::{run_with, Services};
use harpoon_deploy_git::sha256::{hmac_sha256, to_hex};

/// The CLI in process. Anything that may start `git` runs alone (the fork gate): a child forked while
/// another test holds a journal would keep a copy of its lock.
fn crun(t: &T, gate: &Gate, kc: &MemorySecretStore, stdin: &[u8], args: &[&str]) -> Outcome {
    let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    if !a.iter().any(|x| x == "--runtime-dir") {
        a.extend(["--runtime-dir".to_string(), t.rt.display().to_string()]);
    }
    let spawns = matches!(
        args.first().copied(),
        Some("repo-add" | "git-deploy" | "git-run" | "watch")
    ) || a.iter().any(|x| x == "--lsof-path");
    // The product is fail-fast about the journal lock. A fixture process (the smart-HTTP server's
    // `git http-backend`) that another test forks can briefly hold a copy of this test's lock until it
    // execs, which is operating-system behaviour, so only that refusal is retried, a few times, here.
    let mut attempt = 0;
    loop {
        // Every attempt gets the whole standard input: `repo-secret` and `secret-set` read it before they
        // take the journal lock, so a retry after a lock refusal would otherwise see an empty input.
        let mut input: &[u8] = stdin;
        let mut svc = Services {
            secrets: kc,
            stdin: &mut input,
            stdin_is_terminal: false,
            next: None,
        };
        let o = if spawns {
            spawning(|| run_with(&a, gate, &mut svc))
        } else {
            let _h = holding();
            run_with(&a, gate, &mut svc)
        };
        attempt += 1;
        if o.code == 4
            && o.stderr
                .contains("another Harpoon process holds the journal")
            && attempt < 20
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        }
        return o;
    }
}

fn sim(t: &T, kc: &MemorySecretStore, args: &[&str]) -> Outcome {
    crun(t, &SHIPPING, kc, b"", args)
}

fn add_repo(
    t: &T,
    kc: &MemorySecretStore,
    name: &str,
    url: &str,
    app: &str,
    extra: &[&str],
) -> Outcome {
    let mut a = vec![
        "repo-add",
        "--simulate",
        "--name",
        name,
        "--url",
        url,
        "--app",
        app,
        "--container-port",
        "8080",
    ];
    a.extend(extra);
    sim(t, kc, &a)
}

fn commit(repo: &std::path::Path, file: &str, body: &str) -> String {
    std::fs::write(repo.join(file), body).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", body]);
    git(repo, &["rev-parse", "HEAD"])
}

fn url(p: &std::path::Path) -> String {
    format!("file://{}", p.display())
}

fn scratch_clean(t: &T) -> bool {
    harpoon_deploy_git::leftovers(&t.rt.join("scratch")) == 0
}

fn deployments(t: &T) -> Vec<Deployment> {
    harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir())
        .unwrap()
        .deployments
}

fn restored(t: &T) -> harpoon_deploy_journal::Restored {
    harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir()).unwrap()
}

const SECOND_APP: &str = "app_00000000000000000000000002";

#[test]
fn a_selected_commit_becomes_the_right_checkout_buildx_request_deployment_record_and_release() {
    let _ports = ports();
    let t = T::new();
    let kc = MemorySecretStore::new();
    let lsof = forwarder_lsof(&t);
    let gt = Tmp::new();
    let (repo, first, second) = fixture(&gt.0);
    t.h.ws.install_buildx("ls -A \"$PWD\" > \"$D/ctx_ls.txt\"\ncat \"$PWD/VERSION\" > \"$D/ctx_version.txt\"\n[ -e \"$PWD/.git\" ] && echo yes > \"$D/ctx_git.txt\" || echo no > \"$D/ctx_git.txt\"\necho built");
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"",
            &[
                "repo-add",
                "--name",
                "shop",
                "--url",
                &url(&repo),
                "--app",
                APP,
                "--container-port",
                "8080",
                "--environment",
                "prod",
            ],
        ),
        0,
    );

    let live = |a: &[&str]| -> Outcome {
        let mut v: Vec<String> = a.iter().map(|s| s.to_string()).collect();
        v.extend(live_flags(&t));
        v.extend([
            "--forwarder-command".into(),
            "harpoon".into(),
            "--lsof-path".into(),
            lsof.clone(),
            "--health-timeout-secs".into(),
            "2".into(),
        ]);
        let refs: Vec<&str> = v.iter().map(String::as_str).collect();
        crun(&t, &ACCEPTANCE, &kc, b"", &refs)
    };

    // 1. the first release: exactly the older commit, chosen by id
    let held = occupy_all_but_first();
    let pre = responder(&t, TEST_PORT, true);
    let rel = new_release_responder(&t, vec![]);
    image_appears_after_build(&t);
    let o = live(&["git-deploy", "--repo", "shop", "--commit", &first]);
    drop(held);
    drop((pre, rel));
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains(&format!("source: shop at commit {first}")),
        "{}",
        o.stdout
    );
    // the Buildx request: a private snapshot of that commit's tree, no .git, the Dockerfile Harpoon checked
    assert_eq!(
        t.h.ws.buildx_record("ctx_version.txt"),
        "one\n",
        "the context is the first commit"
    );
    assert_eq!(t.h.ws.buildx_record("ctx_git.txt").trim(), "no");
    assert!(t.h.ws.buildx_record("ctx_ls.txt").contains("Dockerfile"));
    let args = t.h.ws.buildx_record("args.txt");
    let r = restored(&t);
    let dep1 = r.deployments[0].clone();
    assert!(
        args.contains("--iidfile") && args.contains(&format!(":{}", dep1.id)),
        "the image tag carries the deployment id: {args}"
    );
    // the records: the source is the immutable commit; the release identity is the deployment's
    let src = r
        .git
        .source_of(&dep1.id)
        .expect("a source for the deployment");
    assert_eq!(
        (
            src.commit.as_str(),
            src.repo.as_str(),
            src.trigger.as_str(),
            src.delivery.clone()
        ),
        (first.as_str(), "shop", "manual", None)
    );
    assert_eq!(src.selector, harpoon_deploy::git::SelectorKind::Commit);
    assert_eq!(dep1.trigger, Trigger::Manual);
    assert_eq!(dep1.release, Some(ReleaseId::from_deployment(&dep1.id)));
    assert_eq!(dep1.status, DeploymentStatus::Succeeded);
    assert!(
        scratch_clean(&t),
        "the workspace and its credentials are gone"
    );

    // 2. a second release from the branch: the newer commit, a different release identity
    let prior = running_app_ports(&t);
    let held = occupy_all_but_first();
    let pre = responder(&t, TEST_PORT, true);
    let rel = new_release_responder(&t, prior);
    image_appears_after_build(&t);
    let o = live(&[
        "git-deploy",
        "--repo",
        "shop",
        "--branch",
        "main",
        "--confirm-cutover",
    ]);
    drop(held);
    drop((pre, rel));
    assert_exit(&o, 0);
    assert_eq!(t.h.ws.buildx_record("ctx_version.txt"), "two\n");
    let r = restored(&t);
    let dep2 = r
        .deployments
        .iter()
        .find(|d| d.id != dep1.id)
        .unwrap()
        .clone();
    let src2 = r.git.source_of(&dep2.id).unwrap();
    assert_eq!(
        (src2.commit.as_str(), src2.requested.as_str()),
        (second.as_str(), "main")
    );
    assert_ne!(dep2.release, dep1.release);
    // a moved branch does not change what a recorded deployment was
    commit(&repo, "VERSION", "three\n");
    assert_eq!(restored(&t).git.source_of(&dep1.id).unwrap().commit, first);
    let o = sim(&t, &kc, &["repo-list"]);
    assert!(
        o.stdout.contains(&format!(
            "last deployment from it: {} at commit {second}",
            dep2.id
        )),
        "{}",
        o.stdout
    );
    // the request a build with a closed gate would see is refused before a repository is contacted
    let before = t.docker_requests();
    let o = crun(&t, &CLOSED, &kc, b"", &["git-deploy", "--repo", "shop"]);
    assert_exit(&o, 6);
    assert!(
        o.stderr
            .contains("live deployment is disabled in this build"),
        "{}",
        o.stderr
    );
    assert_eq!(t.docker_requests(), before);
}

#[test]
fn a_push_found_by_polling_while_a_new_process_starts_is_reported_late_and_deployed_once() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, second) = fixture(&gt.0);
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    // a first look records a baseline: history that already existed is not deployed
    let o = sim(&t, &kc, &["watch", "--once", "--simulate"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("first seen at")
            && o.stdout.contains("baseline")
            && o.stdout.contains("automatic while Harpoon is running"),
        "{}",
        o.stdout
    );
    assert!(deployments(&t).is_empty());
    // the first release, by hand
    assert_exit(
        &sim(&t, &kc, &["git-deploy", "--repo", "shop", "--simulate"]),
        0,
    );
    // a push lands while no Harpoon is running
    let third = commit(&repo, "VERSION", "three\n");
    let o = sim(
        &t,
        &kc,
        &["watch", "--once", "--simulate", "--confirm-cutover"],
    );
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains(&format!("LATE: shop branch main moved {second} -> {third}"))
            && o.stdout
                .contains(&format!("executing delivery poll:shop:{third}")),
        "{}",
        o.stdout
    );
    let r = restored(&t);
    assert_eq!(r.deployments.len(), 2);
    let dep = r
        .deployments
        .iter()
        .find(|d| r.git.source_of(&d.id).is_some_and(|s| s.commit == third))
        .expect("a deployment of the pushed commit");
    let src = r.git.source_of(&dep.id).unwrap();
    assert_eq!(
        (src.trigger.as_str(), src.delivery.as_deref()),
        ("poll-late", Some(format!("poll:shop:{third}").as_str()))
    );
    assert_eq!(dep.trigger, Trigger::Poll);
    assert_eq!(
        r.control
            .event(&r.git.repos["shop"].app, &format!("poll:shop:{third}"))
            .unwrap()
            .status
            .as_str(),
        "executed"
    );
    // looking again finds nothing new: no second deployment for the same push
    let o = sim(
        &t,
        &kc,
        &["watch", "--once", "--simulate", "--confirm-cutover"],
    );
    assert_exit(&o, 0);
    assert!(
        !o.stdout.contains("executing") && !o.stdout.contains("LATE"),
        "{}",
        o.stdout
    );
    assert_eq!(deployments(&t).len(), 2);
    // a branch the filters do not name is never deployed
    git(&repo, &["checkout", "-q", "-b", "feature/x"]);
    commit(&repo, "VERSION", "feature\n");
    git(&repo, &["checkout", "-q", "main"]);
    let o = sim(
        &t,
        &kc,
        &["watch", "--once", "--simulate", "--confirm-cutover"],
    );
    assert!(
        !o.stdout.contains("feature/x") && deployments(&t).len() == 2,
        "{}",
        o.stdout
    );
    assert!(scratch_clean(&t));
}

#[test]
fn ci_triggered_pushes_are_idempotent_filtered_and_the_newest_wins_without_losing_the_record_of_the_rest(
) {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, first, second) = fixture(&gt.0);
    let third = commit(&repo, "VERSION", "three\n");
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            &url(&repo),
            APP,
            &["--filter", "main", "--filter", "release/*"],
        ),
        0,
    );
    let trig = |delivery: &str, rev: &str, extra: &[&str]| {
        let mut a = vec![
            "git-trigger",
            "--simulate",
            "--repo",
            "shop",
            "--delivery",
            delivery,
            "--revision",
            rev,
        ];
        a.extend(extra);
        sim(&t, &kc, &a)
    };
    assert!(trig("ci-1", &first, &[]).stdout.contains("queued"));
    assert!(trig("ci-2", &second, &[]).stdout.contains("queued"));
    assert!(trig("ci-3", &third, &[]).stdout.contains("queued"));
    // a repeat of a delivery changes nothing; the same id with another commit is refused; bad input is refused
    assert!(trig("ci-3", &third, &[])
        .stdout
        .contains("duplicate delivery"));
    let o = trig("ci-3", &first, &[]);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("different source or revision"),
        "{}",
        o.stderr
    );
    assert_exit(&trig("ci-9", "abc123", &[]), 2);
    assert_exit(&trig("has space", &third, &[]), 6);
    // branches outside the filters and tags are ignored, not queued
    assert!(trig("ci-x", &third, &["--ref", "refs/heads/dev"])
        .stdout
        .contains("ignored"));
    assert!(trig("ci-y", &third, &["--ref", "refs/tags/v9"])
        .stdout
        .contains("ignored"));
    assert!(trig("ci-z", &third, &["--ref", "refs/heads/release/2"])
        .stdout
        .contains("queued"));
    // a release branch push is queued too; execution takes the newest of everything queued
    let o = sim(&t, &kc, &["git-run", "--simulate"]);
    assert_exit(&o, 0);
    assert_eq!(
        o.stdout.matches("executing delivery").count(),
        1,
        "one build for the whole queue: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains("supersedes 3 older queued event(s)"),
        "{}",
        o.stdout
    );
    let r = restored(&t);
    assert_eq!(r.deployments.len(), 1);
    let src = r.git.source_of(&r.deployments[0].id).unwrap();
    assert_eq!(src.commit, third);
    assert_eq!(
        (src.trigger.as_str(), r.deployments[0].trigger),
        ("ci", Trigger::Manual)
    );
    let app = r.git.repos["shop"].app.clone();
    let st = |d: &str| {
        r.control
            .event(&app, d)
            .unwrap()
            .status
            .as_str()
            .to_string()
    };
    let executed: Vec<&str> = ["ci-1", "ci-2", "ci-3", "ci-z"]
        .into_iter()
        .filter(|d| st(d) == "executed")
        .collect();
    assert_eq!(executed.len(), 1);
    assert_eq!(
        executed.len()
            + ["ci-1", "ci-2", "ci-3", "ci-z"]
                .into_iter()
                .filter(|d| st(d) == "superseded")
                .count(),
        4
    );
    // events are kept; running again deploys nothing, and a late duplicate of an executed delivery is still a duplicate
    assert!(sim(&t, &kc, &["git-run", "--simulate"])
        .stdout
        .contains("nothing is queued"));
    assert!(trig(
        executed[0],
        if executed[0] == "ci-3" || executed[0] == "ci-z" {
            &third
        } else {
            &first
        },
        &[]
    )
    .stdout
    .contains("duplicate"));
    assert_eq!(deployments(&t).len(), 1);
    let o = sim(&t, &kc, &["events", "--app", APP]);
    assert!(
        o.stdout.contains("superseded") && o.stdout.contains("executed"),
        "{}",
        o.stdout
    );
}

#[test]
fn a_failed_checkout_is_recorded_with_its_reason_leaves_nothing_behind_and_does_not_block_the_next_event(
) {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, first, _) = fixture(&gt.0);
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    let trig = |d: &str, rev: &str| {
        sim(
            &t,
            &kc,
            &[
                "git-trigger",
                "--simulate",
                "--repo",
                "shop",
                "--delivery",
                d,
                "--revision",
                rev,
            ],
        )
    };
    assert_exit(&trig("bad-1", &"1".repeat(40)), 0);
    let o = sim(&t, &kc, &["git-run", "--simulate"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("delivery bad-1 NOT deployed:"),
        "{}",
        o.stdout
    );
    assert!(
        deployments(&t).is_empty(),
        "no deployment exists for a checkout that failed"
    );
    assert!(
        restored(&t).git.sources.is_empty(),
        "and no source was recorded"
    );
    assert!(scratch_clean(&t));
    let r = restored(&t);
    assert_eq!(
        r.control
            .event(&r.git.repos["shop"].app, "bad-1")
            .unwrap()
            .status
            .as_str(),
        "failed"
    );
    // the repository becomes unavailable: the event fails with the reason and nothing is half-done
    let moved = gt.0.join("origin-moved");
    std::fs::rename(&repo, &moved).unwrap();
    assert_exit(&trig("gone-1", &first), 0);
    let o = sim(&t, &kc, &["git-run", "--simulate"]);
    assert!(
        o.stdout.contains("delivery gone-1 NOT deployed:"),
        "{}",
        o.stdout
    );
    assert!(deployments(&t).is_empty() && scratch_clean(&t));
    let o = sim(&t, &kc, &["watch", "--once", "--simulate"]);
    assert_exit(&o, 7);
    assert!(o.stdout.contains("shop:"), "{}", o.stdout);
    // it comes back; the next event deploys normally
    std::fs::rename(&moved, &repo).unwrap();
    assert_exit(&trig("ok-1", &first), 0);
    assert!(sim(&t, &kc, &["git-run", "--simulate"])
        .stdout
        .contains("executed as deployment"));
    assert_eq!(deployments(&t).len(), 1);
}

#[test]
fn a_paused_application_holds_pushed_events_until_a_decision_selects_one() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, first, second) = fixture(&gt.0);
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    // a failed cutover for this application: the pause B14 derives from it
    {
        let app: ApplicationId = APP.parse().unwrap();
        let dep: DeploymentId = "dep_00000000000000000000000005".parse().unwrap();
        let mut d = Deployment::new(dep, app, Trigger::Cli, 50);
        d.transition(DeploymentStatus::Queued, 51).unwrap();
        d.transition(DeploymentStatus::Running, 52).unwrap();
        d.start_step(Step::Cutover, 53).unwrap();
        d.replaces = Some(ReleaseId::from_deployment(&PREV.parse().unwrap()));
        d.begin_cutover(54).unwrap();
        d.fail_cutover(55).unwrap();
        d.start_recovery(60_000, 56).unwrap();
        d.finish_recovery(harpoon_deploy::state::RecoveryOutcome::Restored, 57)
            .unwrap();
        d.transition(DeploymentStatus::Failed, 58).unwrap();
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.persist(&d).unwrap();
    }
    let trig = |d: &str, rev: &str| {
        sim(
            &t,
            &kc,
            &[
                "git-trigger",
                "--simulate",
                "--repo",
                "shop",
                "--delivery",
                d,
                "--revision",
                rev,
            ],
        )
    };
    assert_exit(&trig("p-1", &first), 0);
    assert_exit(&trig("p-2", &second), 0);
    let o = sim(&t, &kc, &["git-run", "--simulate"]);
    assert!(
        o.stdout.contains("nothing is queued to run") && !o.stdout.contains("executing"),
        "{}",
        o.stdout
    );
    assert_eq!(
        deployments(&t).len(),
        1,
        "only the old failed one: nothing deployed by itself"
    );
    let o = sim(&t, &kc, &["events", "--app", APP]);
    assert!(o.stdout.contains("held"), "{}", o.stdout);
    // the developer chooses; only then is it executed, and the pause stays until it is dismissed
    assert_exit(
        &sim(&t, &kc, &["resume", "--app", APP, "--revision", &first]),
        0,
    );
    let o = sim(&t, &kc, &["git-run", "--simulate"]);
    assert!(
        o.stdout
            .contains(&format!("executing delivery p-1 (ci) at {first}")),
        "{}",
        o.stdout
    );
    let r = restored(&t);
    assert_eq!(
        r.control
            .event(&APP.parse().unwrap(), "p-1")
            .unwrap()
            .status
            .as_str(),
        "executed"
    );
    assert_eq!(
        r.control
            .event(&APP.parse().unwrap(), "p-2")
            .unwrap()
            .status
            .as_str(),
        "superseded"
    );
    assert!(
        sim(&t, &kc, &["status"]).stdout.contains("paused"),
        "the pause is not cleared by executing"
    );
}

/// Connect to the listener once it is up. The listener starts only after the CLI call has been through the
/// fork gate (it waits for every open journal in the other test threads), so how long that takes depends on
/// the machine. Wait for the condition, with a deadline that only a genuinely failed start can reach.
fn connect_when_up(port: u16, deadline: std::time::Duration) -> std::net::TcpStream {
    let end = std::time::Instant::now() + deadline;
    loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(c) => return c,
            Err(e) if std::time::Instant::now() >= end => {
                panic!("the listener on port {port} did not come up within {deadline:?}: {e}")
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

const LISTENER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

fn post(port: u16, path: &str, headers: &[(&str, String)], body: &str) -> String {
    let mut c = connect_when_up(port, LISTENER_DEADLINE);
    let mut req = format!("POST {path} HTTP/1.1\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    c.write_all(req.as_bytes()).unwrap();
    c.write_all(body.as_bytes()).unwrap();
    let mut out = String::new();
    // a server that refuses a body without reading it may reset the connection before the reply is read
    if c.read_to_string(&mut out).is_err() && out.is_empty() {
        return "HTTP/1.1 RESET".to_string();
    }
    out.lines().next().unwrap_or("").to_string()
}

/// The fixture's retry after a journal-lock refusal must give the command its whole standard input again.
#[test]
fn crun_retries_a_lock_refusal_with_the_whole_standard_input() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    assert_exit(
        &add_repo(&t, &kc, "shop", &url(&repo), APP, &["--webhook", "github"]),
        0,
    );
    // another writer holds the journal for a moment: the first attempts are refused with exit 4
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let dir = t.journal_dir();
    let holder = std::thread::spawn(move || {
        let (j, _) = open_j(&dir).unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(j);
    });
    locked_rx.recv().unwrap();
    let o = crun(
        &t,
        &SHIPPING,
        &kc,
        b"gh-secret\n",
        &[
            "repo-secret",
            "--simulate",
            "--name",
            "shop",
            "--kind",
            "webhook",
        ],
    );
    holder.join().unwrap();
    assert_exit(&o, 0);
}

/// `post` waits for the listener to come up instead of giving up after a fixed few seconds.
#[test]
fn post_waits_for_a_listener_that_starts_late() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let server = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(3500)); // later than the old fixed window
        let l = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        let (mut c, _) = l.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = std::io::Read::read(&mut c, &mut buf);
        std::io::Write::write_all(
            &mut c,
            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n",
        )
        .unwrap();
    });
    assert_eq!(
        post(port, "/webhook/github/shop", &[], "{}"),
        "HTTP/1.1 202 Accepted"
    );
    server.join().unwrap();
}

#[test]
fn signed_github_and_token_verified_gitlab_deliveries_deploy_once_and_unverifiable_ones_never_reach_the_queue(
) {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (shop, first, second) = fixture(&gt.0);
    let blog_root = Tmp::new();
    let (blog, b1, _b2) = fixture(&blog_root.0);
    assert_exit(
        &add_repo(&t, &kc, "shop", &url(&shop), APP, &["--webhook", "github"]),
        0,
    );
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "blog",
            &url(&blog),
            SECOND_APP,
            &["--webhook", "gitlab"],
        ),
        0,
    );
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"gh-secret\n",
            &[
                "repo-secret",
                "--simulate",
                "--name",
                "shop",
                "--kind",
                "webhook",
            ],
        ),
        0,
    );
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"gl-token\n",
            &[
                "repo-secret",
                "--simulate",
                "--name",
                "blog",
                "--kind",
                "webhook",
            ],
        ),
        0,
    );
    // a repository without a webhook provider takes no webhook secret
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "plain",
            &url(&shop),
            "app_00000000000000000000000003",
            &[],
        ),
        0,
    );
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"x",
            &[
                "repo-secret",
                "--simulate",
                "--name",
                "plain",
                "--kind",
                "webhook",
            ],
        ),
        6,
    );

    // a free loopback port for the listener
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let gh_body = format!(
        r#"{{"ref":"refs/heads/main","after":"{second}","repository":{{"clone_url":"{}"}}}}"#,
        url(&shop)
    );
    let sign = |secret: &str, body: &str| {
        format!(
            "sha256={}",
            to_hex(&hmac_sha256(secret.as_bytes(), body.as_bytes()))
        )
    };
    let gl_body = format!(
        r#"{{"ref":"refs/heads/main","after":"{b1}","checkout_sha":"{b1}","project":{{"git_http_url":"x"}}}}"#
    );
    let (gh_body2, gl_body2) = (gh_body.clone(), gl_body.clone());
    let (secret_ok, sign_ok) = (sign("gh-secret", &gh_body), sign("gh-secret", &gh_body));
    let (sig_tampered, sig_wrong) = (sign("gh-secret", &gh_body), sign("wrong", &gh_body));
    let _ = (secret_ok, sign_ok);
    // the poster talks to the listener from another thread while `watch` runs on this one
    let poster = std::thread::spawn(move || {
        let gh = |delivery: &str, sig: String, body: &str| {
            post(
                port,
                "/webhook/github/shop",
                &[
                    ("X-Hub-Signature-256", sig),
                    ("X-GitHub-Event", "push".into()),
                    ("X-GitHub-Delivery", delivery.into()),
                ],
                body,
            )
        };
        let good = format!(
            "sha256={}",
            to_hex(&hmac_sha256(b"gh-secret", gh_body2.as_bytes()))
        );
        let mut replies: Vec<String> = vec![
            gh("g-1", good.clone(), &gh_body2), // valid
            gh("g-2", sig_tampered, &gh_body2.replace("main", "evil")), // tampered body, original signature
            gh("g-3", sig_wrong, &gh_body2),                            // wrong secret
            gh("g-1", good, &gh_body2),                                 // replayed delivery
        ];
        let gl = |token: &str| {
            post(
                port,
                "/webhook/gitlab/blog",
                &[
                    ("X-Gitlab-Token", token.into()),
                    ("X-Gitlab-Event", "Push Hook".into()),
                ],
                &gl_body2,
            )
        };
        replies.push(gl("gl-token")); // valid, no UUID header
        replies.push(gl("nope"));
        replies.push(gl("gl-token")); // replay
        replies.push(post(port, "/webhook/github/unknown", &[], "{}"));
        replies.push(post(
            port,
            "/webhook/github/shop",
            &[("X-Hub-Signature-256", "sha256=00".into())],
            &"x".repeat(300_000),
        )); // oversized
        replies
    });
    let out = crun(
        &t,
        &SHIPPING,
        &kc,
        b"",
        &[
            "watch",
            "--once",
            "--simulate",
            "--confirm-cutover",
            "--webhook-port",
            &port.to_string(),
            "--webhook-wait-secs",
            "4",
        ],
    );
    let replies = poster.join().unwrap();
    assert_exit(&out, 0);
    let status = |i: usize| replies[i].split(' ').nth(1).unwrap_or("").to_string();
    assert_eq!(
        (0..8).map(status).collect::<Vec<_>>(),
        ["202", "401", "401", "202", "202", "401", "202", "404"],
        "{replies:?}"
    );
    // refused (413), or the connection was reset while the refused body was still being sent
    assert!(
        !status(8).starts_with("20"),
        "an oversized body is never accepted: {replies:?}"
    );
    assert!(
        out.stdout.contains("webhook listener on 127.0.0.1:"),
        "{}",
        out.stdout
    );
    let r = restored(&t);
    let by = |app: &str| -> Vec<&Deployment> {
        r.deployments
            .iter()
            .filter(|d| d.application_id.to_string() == app)
            .collect()
    };
    assert_eq!(
        by(APP).len(),
        1,
        "one deployment for the replayed GitHub delivery"
    );
    assert_eq!(
        by(SECOND_APP).len(),
        1,
        "one deployment for the replayed GitLab push"
    );
    let s = r.git.source_of(&by(APP)[0].id).unwrap();
    assert_eq!(
        (s.commit.as_str(), s.trigger.as_str(), s.delivery.as_deref()),
        (second.as_str(), "github", Some("g-1"))
    );
    assert_eq!(by(APP)[0].trigger, Trigger::Webhook);
    let s = r.git.source_of(&by(SECOND_APP)[0].id).unwrap();
    assert_eq!(
        (s.commit.as_str(), s.trigger.as_str()),
        (b1.as_str(), "gitlab")
    );
    assert_eq!(
        s.delivery.as_deref(),
        Some(format!("gitlab:{b1}").as_str()),
        "derived when the UUID header is absent"
    );
    // the unverified deliveries are nowhere
    let app: ApplicationId = APP.parse().unwrap();
    for d in ["g-2", "g-3"] {
        assert!(r.control.event(&app, d).is_none(), "{d}");
    }
    assert!(out.stdout.contains("duplicate delivery"), "{}", out.stdout);
    assert!(
        files_with_any(&t, &["gh-secret", "gl-token"]).is_empty(),
        "the webhook secrets are in no Harpoon file"
    );
    assert!(scratch_clean(&t));
    let _ = first;
}

fn files_with_any(t: &T, needles: &[&str]) -> Vec<String> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(&t.rt, &mut files);
    files
        .into_iter()
        .filter(|p| {
            let b = std::fs::read(p).unwrap_or_default();
            needles
                .iter()
                .any(|n| b.windows(n.len()).any(|w| w == n.as_bytes()))
        })
        .map(|p| p.display().to_string())
        .collect()
}

#[test]
fn a_private_repository_deploys_with_its_token_a_revoked_token_fails_cleanly_and_nothing_holds_it()
{
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, second) = fixture(&gt.0);
    let served = gt.0.join("served");
    std::fs::create_dir_all(&served).unwrap();
    git(
        &gt.0,
        &[
            "clone",
            "-q",
            "--bare",
            repo.to_str().unwrap(),
            served.join("shop.git").to_str().unwrap(),
        ],
    );
    let server = HttpGit::start(served, "x-access-token", "tok-planted-ONE");
    let u = server.url("shop.git");
    assert_exit(
        &add_repo(&t, &kc, "shop", &u, APP, &["--username", "x-access-token"]),
        0,
    );
    // no credential stored yet: the deployment is refused with the reason, nothing is checked out
    let o = sim(&t, &kc, &["git-deploy", "--repo", "shop", "--simulate"]);
    assert_exit(&o, 7);
    assert!(
        o.stderr.contains("no credential has been saved"),
        "{}",
        o.stderr
    );
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"tok-planted-ONE\n",
            &[
                "repo-secret",
                "--simulate",
                "--name",
                "shop",
                "--kind",
                "credential",
            ],
        ),
        0,
    );
    let o = sim(&t, &kc, &["git-deploy", "--repo", "shop", "--simulate"]);
    assert_exit(&o, 0);
    let r = restored(&t);
    assert_eq!(
        r.git.source_of(&r.deployments[0].id).unwrap().commit,
        second
    );
    for out in [&o.stdout, &o.stderr] {
        assert!(!out.contains("tok-planted"), "{out}");
    }
    // revoked on the server: the next poll and the next deployment fail, naming the credential problem
    *server.token.lock().unwrap() = "tok-planted-TWO".into();
    let o = sim(&t, &kc, &["watch", "--once", "--simulate"]);
    assert_exit(&o, 7);
    assert!(
        o.stdout.contains("credential was refused") && !o.stdout.contains("tok-planted"),
        "{}",
        o.stdout
    );
    let o = sim(&t, &kc, &["git-deploy", "--repo", "shop", "--simulate"]);
    assert_exit(&o, 7);
    assert!(
        o.stderr.contains("credential was refused") && !o.stderr.contains("tok-planted"),
        "{}",
        o.stderr
    );
    assert_eq!(deployments(&t).len(), 1);
    // rotation: a new version is used at once
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"tok-planted-TWO\n",
            &[
                "repo-secret",
                "--simulate",
                "--name",
                "shop",
                "--kind",
                "credential",
            ],
        ),
        0,
    );
    assert_exit(
        &sim(
            &t,
            &kc,
            &[
                "git-deploy",
                "--repo",
                "shop",
                "--simulate",
                "--confirm-cutover",
            ],
        ),
        0,
    );
    assert_eq!(deployments(&t).len(), 2);
    // the token is in no Harpoon file, nothing is left in the workspace area, and the journal still replays
    assert!(files_with_any(&t, &["tok-planted"]).is_empty());
    assert!(scratch_clean(&t));
    drop(open_j(&t.journal_dir()).expect("the journal replays"));
    // SSH repositories must pin their host key, and the credential kind must fit the URL
    let o = add_repo(
        &t,
        &kc,
        "ssh",
        "git@example.com:acme/x.git",
        "app_00000000000000000000000004",
        &[],
    );
    assert_exit(&o, 2);
    assert!(o.stderr.contains("host key"), "{}", o.stderr);
    let o = add_repo(
        &t,
        &kc,
        "ssh",
        "git@example.com:acme/x.git",
        "app_00000000000000000000000004",
        &["--known-host", "example.com ssh-ed25519 AAAAC3Nza"],
    );
    assert_exit(&o, 0);
    assert!(o.stdout.contains("credential: sshkey"), "{}", o.stdout);
    for bad in [
        "-oProxyCommand=x",
        "ext::sh -c x",
        "https://u:pw@example.com/r.git",
        "git://example.com/r.git",
    ] {
        assert_exit(
            &add_repo(&t, &kc, "bad", bad, "app_00000000000000000000000005", &[]),
            2,
        );
    }
}

#[test]
fn checkout_workspaces_left_by_a_hard_kill_are_reported_by_status_and_removed_by_the_next_writer() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    let stale = t.rt.join("scratch").join("git-ws-crashed");
    std::fs::create_dir_all(stale.join("cred")).unwrap();
    std::fs::write(
        stale.join("cred").join("token"),
        "left-by-a-killed-checkout",
    )
    .unwrap();
    let o = sim(&t, &kc, &["status"]);
    assert!(
        o.stdout.contains("WARNING: 1 build-secret director")
            && !o.stdout.contains("left-by-a-killed"),
        "{}",
        o.stdout
    );
    assert!(stale.exists());
    assert_exit(&sim(&t, &kc, &["recover"]), 0);
    assert!(!stale.exists());
}

#[test]
fn the_interval_has_a_floor_and_the_documentation_makes_no_unattended_claim() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    for bad in ["0", "59", "abc"] {
        let o = sim(
            &t,
            &kc,
            &[
                "watch",
                "--simulate",
                "--interval-secs",
                bad,
                "--iterations",
                "1",
            ],
        );
        assert_exit(&o, 2);
    }
    assert_exit(
        &sim(
            &t,
            &kc,
            &["watch", "--simulate", "--once", "--webhook-port", "99999"],
        ),
        2,
    );
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/results/DEPLOY_MILESTONE_D_GIT_INTEGRATION.md"),
    )
    .expect("the Milestone D report exists");
    for must in [
        "while Harpoon is running",
        "Gate U",
        "NOT PASSED",
        "unattended",
        "Not exercised",
    ] {
        assert!(doc.contains(must), "the report must say: {must}");
    }
    assert!(!doc.contains("runs unattended") && !doc.contains("fully automatic"));
    let help = harpoon_deploy_cli::HELP.to_lowercase();
    assert!(
        help.contains("while harpoon is running")
            && !help.contains("unattended operation is supported")
    );
}

/// Answers `/health` on every application port that has a running container, marker included, binding each
/// port as its container appears (a release's port is not known in advance).
fn release_responders(t: &T) -> impl Drop {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Stop(std::sync::Arc<AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (flag, docker) = (stop.clone(), t.h.docker.st.clone());
    std::thread::spawn(move || {
        let mut bound: BTreeMap<u16, std::net::TcpListener> = BTreeMap::new();
        while !flag.load(Ordering::SeqCst) {
            let ports: Vec<(u16, Option<String>)> = docker
                .lock()
                .unwrap()
                .containers
                .iter()
                .filter_map(|k| {
                    let p = k.host_port.filter(|p| (38000..39000).contains(p))?;
                    let marker = k.running.then(|| {
                        k.create_body
                            .get("Env")
                            .and_then(harpoon_deploy_docker::json::Json::as_array)
                            .and_then(|a| {
                                a.iter().filter_map(|e| e.as_str()).find_map(|e| {
                                    e.strip_prefix("HARPOON_DEPLOYMENT_MARKER=")
                                        .map(str::to_string)
                                })
                            })
                    });
                    Some((p, marker.flatten()))
                })
                .collect();
            for (p, _) in &ports {
                if !bound.contains_key(p) {
                    if let Ok(l) = std::net::TcpListener::bind(("127.0.0.1", *p)) {
                        l.set_nonblocking(true).unwrap();
                        bound.insert(*p, l);
                    }
                }
            }
            for (p, l) in &bound {
                while let Ok((mut c, _)) = l.accept() {
                    c.set_nonblocking(false).unwrap();
                    let mut buf = [0u8; 1024];
                    let _ = c.set_read_timeout(Some(std::time::Duration::from_secs(1)));
                    let _ = c.read(&mut buf);
                    let marker = ports
                        .iter()
                        .find(|(q, _)| q == p)
                        .and_then(|(_, m)| m.clone());
                    let reply = match marker {
                        Some(m) => format!("HTTP/1.1 200 OK\r\nX-Harpoon-Deployment-Marker: {m}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{m}\n", m.len() + 1),
                        None => "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                    };
                    let _ = c.write_all(reply.as_bytes());
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    });
    Stop(stop)
}

#[test]
fn two_pushes_during_a_running_build_do_not_interrupt_it_and_only_the_newest_builds_next() {
    let _ports = ports();
    let t = T::new();
    let kc = MemorySecretStore::new();
    let lsof = forwarder_lsof(&t);
    let gt = Tmp::new();
    let (repo, first, second) = fixture(&gt.0);
    let third = commit(&repo, "VERSION", "three\n");
    // every build takes two seconds, long enough for pushes to arrive while it runs
    t.h.ws
        .install_buildx("sleep 2\ncat \"$PWD/VERSION\" >> \"$D/versions.txt\"\necho built");
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"",
            &[
                "repo-add",
                "--name",
                "shop",
                "--url",
                &url(&repo),
                "--app",
                APP,
                "--container-port",
                "8080",
                "--webhook",
                "github",
            ],
        ),
        0,
    );
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"hook-secret\n",
            &["repo-secret", "--name", "shop", "--kind", "webhook"],
        ),
        0,
    );
    // the push that starts the first build arrives from CI
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"",
            &[
                "git-trigger",
                "--repo",
                "shop",
                "--delivery",
                "ci-0",
                "--revision",
                &first,
            ],
        ),
        0,
    );

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let started = t.h.ws.buildx_path().parent().unwrap().join("args.txt");
    let body = |rev: &str| {
        format!(r#"{{"ref":"refs/heads/main","after":"{rev}","repository":{{"clone_url":"x"}}}}"#)
    };
    let post_push = move |delivery: &'static str, rev: String| {
        let b = body(&rev);
        let sig = format!(
            "sha256={}",
            to_hex(&hmac_sha256(b"hook-secret", b.as_bytes()))
        );
        post(
            port,
            "/webhook/github/shop",
            &[
                ("X-Hub-Signature-256", sig),
                ("X-GitHub-Event", "push".into()),
                ("X-GitHub-Delivery", delivery.into()),
            ],
            &b,
        )
    };
    let (second2, third2) = (second.clone(), third.clone());
    // wait until Buildx is running the first build, then push twice
    let poster = std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        while !started.exists() && t0.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mid_build = started.exists();
        let a = post_push("hook-1", second2);
        let b = post_push("hook-2", third2);
        (mid_build, a, b)
    });

    let held = occupy_all_but_first();
    let _pre = responder(&t, TEST_PORT, true);
    let _apps = release_responders(&t);
    image_appears_after_build(&t);
    image_appears_after_build(&t);
    let mut a: Vec<String> = [
        "watch",
        "--once",
        "--repo",
        "shop",
        "--confirm-cutover",
        "--webhook-port",
        &port.to_string(),
        "--webhook-wait-secs",
        "12",
        "--health-timeout-secs",
        "2",
    ]
    .map(String::from)
    .to_vec();
    a.extend(live_flags(&t));
    a.extend([
        "--forwarder-command".into(),
        "harpoon".into(),
        "--lsof-path".into(),
        lsof.clone(),
    ]);
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    let o = crun(&t, &ACCEPTANCE, &kc, b"", &refs);
    drop(held);
    let (mid_build, ra, rb) = poster.join().unwrap();
    assert!(
        mid_build && ra.contains("202") && rb.contains("202"),
        "both pushes were accepted while the first build was running: {mid_build} {ra} {rb}"
    );
    assert_exit(&o, 0);

    let r = restored(&t);
    assert_eq!(
        r.deployments.len(),
        2,
        "the running build, then ONE more: {}",
        o.stdout
    );
    let app: ApplicationId = APP.parse().unwrap();
    let by_commit = |c: &str| {
        r.deployments
            .iter()
            .find(|d| r.git.source_of(&d.id).is_some_and(|s| s.commit == c))
    };
    let (d1, d2) = (
        by_commit(&first).expect("the first build ran to completion"),
        by_commit(&third).expect("the newest push built next"),
    );
    assert!(
        by_commit(&second).is_none(),
        "the intermediate push was never built"
    );
    assert_eq!(
        d1.status,
        DeploymentStatus::Succeeded,
        "the running build was not interrupted"
    );
    assert!(
        d1.finished_ms.unwrap() <= d2.created_ms,
        "the second build started after the first finished"
    );
    let st = |d: &str| r.control.event(&app, d).unwrap();
    assert_eq!(st("ci-0").status.as_str(), "executed");
    assert_eq!(
        (
            st("hook-1").status.as_str(),
            st("hook-1").related.as_deref()
        ),
        ("superseded", Some("hook-2"))
    );
    assert_eq!(st("hook-2").status.as_str(), "executed");
    assert_eq!(
        t.h.ws.buildx_record("versions.txt"),
        "one\nthree\n",
        "two builds, the intermediate commit's tree never built"
    );
    assert!(scratch_clean(&t));
}

/// Milestone E.2: the desktop's data directory is `~/Library/Application Support/...`, which has a space.
/// Polling (queue only, as the desktop does under the shipping gate) must work from such a path.
#[test]
fn polling_works_from_a_runtime_directory_whose_path_contains_spaces() {
    use std::os::unix::fs::DirBuilderExt;
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    let rt =
        gt.0.join("Library/Application Support/com.example app/deploy");
    for d in [rt.clone(), rt.join("deploy-journal")] {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&d)
            .unwrap();
        std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    }
    let rts = rt.display().to_string();
    let go = |args: &[&str]| {
        let mut a = args.to_vec();
        a.extend(["--runtime-dir", &rts]);
        crun(&t, &SHIPPING, &kc, b"", &a)
    };
    assert_exit(
        &go(&[
            "repo-add",
            "--name",
            "shop",
            "--url",
            &url(&repo),
            "--app",
            APP,
            "--container-port",
            "8080",
        ]),
        0,
    );
    let before = t.docker_requests();
    let first = go(&["watch", "--once", "--queue-only"]);
    assert_exit(&first, 0);
    assert!(
        first.stdout.contains("first seen at") && first.stdout.contains("baseline"),
        "{}{}",
        first.stdout,
        first.stderr
    );
    let third = commit(&repo, "VERSION", "three\n");
    let second = go(&["watch", "--once", "--queue-only"]);
    assert_exit(&second, 0);
    assert!(
        second.stdout.contains("queued as") && second.stdout.contains(&third),
        "{}{}",
        second.stdout,
        second.stderr
    );
    let ev = go(&["events", "--app", APP]);
    assert!(
        ev.stdout.contains(&third) && ev.stdout.contains("held"),
        "{}",
        ev.stdout
    );
    // reading and queueing only: no deployment, no Docker request
    assert!(go(&["status"]).stdout.contains("deployments: 0"));
    assert_eq!(t.docker_requests(), before);
    // and the workspace it used under the spaced path is gone
    let scratch = std::fs::read_dir(&rt)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .flat_map(|d| {
            std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|n| n.starts_with("git-ws-"))
        .count();
    assert_eq!(scratch, 0);
}

/// Milestone E.3: a local folder with spaces is registered as an encoded file URL, a raw one is refused
/// without leaving a store behind, and polling then works.
#[test]
fn a_local_folder_with_spaces_registers_as_an_encoded_file_url_and_a_raw_one_leaves_nothing() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let home = gt.0.join("Harpoon UI Test");
    std::fs::create_dir_all(&home).unwrap();
    let (repo, _, _) = fixture(&home);
    let raw = url(&repo);
    let enc = raw.replace(' ', "%20");
    assert!(raw.contains(' ') && !enc.contains(' '));
    // the raw URL is refused by URL validation before any store is opened
    let o = add_repo(&t, &kc, "shop", &raw, APP, &[]);
    assert_exit(&o, 2);
    assert!(o.stderr.contains("whitespace"), "{}", o.stderr);
    assert!(
        !t.rt
            .join("deploy-journal")
            .join("deployments.sqlite3")
            .exists(),
        "no store was created by the refusal"
    );
    // malformed / traversing encodings are refused too
    for bad in [
        format!("{enc}/%2e%2e/x"),
        format!("{enc}%00"),
        format!("{enc}%zz"),
    ] {
        assert_exit(&add_repo(&t, &kc, "shop", &bad, APP, &[]), 2);
    }
    // the encoded URL registers and polls
    assert_exit(&add_repo(&t, &kc, "shop", &enc, APP, &[]), 0);
    let list = sim(&t, &kc, &["repo-list"]);
    assert!(
        list.stdout.contains("Harpoon%20UI%20Test"),
        "{}",
        list.stdout
    );
    let o = sim(&t, &kc, &["watch", "--once", "--queue-only"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("first seen at"),
        "{}{}",
        o.stdout,
        o.stderr
    );
}

// ------------------------------------------------------------------ registration checks a local source

/// Every byte of the journal directory, in name order: equal before and after means nothing was written.
fn journal_bytes(t: &T) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(t.journal_dir())
        .map(|d| {
            d.flatten()
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().into(),
                        std::fs::read(e.path()).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn a_local_source_that_is_not_a_usable_repository_is_refused_and_the_journal_is_untouched() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, second) = fixture(&gt.0);
    // an earlier, valid registration, so there is a journal whose bytes can be compared
    let ok = add_repo(&t, &kc, "shop", &url(&repo), APP, &[]);
    assert_exit(&ok, 0);
    assert!(
        ok.stdout
            .contains(&format!("branch main is at {}", &second[..12])),
        "{}",
        ok.stdout
    );
    let before = journal_bytes(&t);
    let plain = gt.0.join("plain");
    std::fs::create_dir(&plain).unwrap();
    let empty = gt.0.join("empty");
    std::fs::create_dir(&empty).unwrap();
    git(&empty, &["init", "-q"]);
    for (name, u, branch, want) in [
        ("a", url(&gt.0.join("missing")), "main", "does not exist"),
        ("b", url(&plain), "main", "is not a Git repository"),
        ("c", url(&empty), "main", "has no commits yet"),
        ("d", url(&repo), "nope", "has no branch \"nope\""),
    ] {
        let o = add_repo(&t, &kc, name, &u, SECOND_APP, &["--branch", branch]);
        assert_exit(&o, 6);
        assert!(o.stderr.contains(want), "{want}: {}", o.stderr);
        assert_eq!(journal_bytes(&t), before, "{name}: the journal changed");
    }
    assert_eq!(
        restored(&t).git.repos.len(),
        1,
        "only the first registration exists"
    );
    assert!(scratch_clean(&t));
}

#[test]
fn a_refused_first_registration_creates_no_journal() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let plain = gt.0.join("plain");
    std::fs::create_dir(&plain).unwrap();
    assert_exit(&add_repo(&t, &kc, "shop", &url(&plain), APP, &[]), 6);
    assert!(
        !t.journal_dir().exists(),
        "no store was created for a refused registration"
    );
}

#[test]
fn registering_an_existing_name_is_refused_with_new_and_replaces_without_it() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    let before = journal_bytes(&t);
    let o = add_repo(
        &t,
        &kc,
        "shop",
        &url(&repo),
        APP,
        &["--new", "--branch", "release/1"],
    );
    assert_exit(&o, 6);
    assert!(o.stderr.contains("already registered"), "{}", o.stderr);
    assert_eq!(journal_bytes(&t), before);
    assert_eq!(restored(&t).git.repos["shop"].branch, "main");
    // the existing command keeps its meaning for scripts that re-register on purpose
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            &url(&repo),
            APP,
            &["--branch", "release/1"],
        ),
        0,
    );
    assert_eq!(restored(&t).git.repos["shop"].branch, "release/1");
}

#[test]
fn a_remote_source_is_registered_unverified_without_contacting_it() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    // .invalid never resolves: if registration contacted it, this would fail or hang
    let o = add_repo(
        &t,
        &kc,
        "shop",
        "https://git.invalid/acme/shop.git",
        APP,
        &[],
    );
    assert_exit(&o, 0);
    assert!(o.stdout.contains("not verified yet"), "{}", o.stdout);
    assert!(!o.stdout.contains("verified: "), "{}", o.stdout);
}

// ------------------------------------------------------------------ editing a registration

fn update(t: &T, kc: &MemorySecretStore, name: &str, expect: u64, extra: &[&str]) -> Outcome {
    let e = expect.to_string();
    let mut a = vec![
        "repo-update",
        "--simulate",
        "--name",
        name,
        "--expect-registered-ms",
        &e,
    ];
    a.extend(extra);
    sim(t, kc, &a)
}

fn registered_ms(t: &T, name: &str) -> u64 {
    restored(t).git.repos[name].at_ms
}

/// A finished deployment of `APP` with a recorded Git source, so the application has history.
fn seed_history(t: &T, repo_name: &str, url: &str) -> (Deployment, harpoon_deploy::git::SourceRec) {
    let app: ApplicationId = APP.parse().unwrap();
    let dep: DeploymentId = "dep_00000000000000000000000009".parse().unwrap();
    let mut d = Deployment::new(dep.clone(), app, Trigger::Cli, 50);
    d.transition(DeploymentStatus::Queued, 51).unwrap();
    d.transition(DeploymentStatus::Running, 52).unwrap();
    d.transition(DeploymentStatus::Failed, 53).unwrap();
    let src = harpoon_deploy::git::SourceRec {
        deployment: dep,
        repo: repo_name.into(),
        url: url.into(),
        selector: harpoon_deploy::git::SelectorKind::Branch,
        requested: "main".into(),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        trigger: "manual".into(),
        delivery: None,
        at_ms: 54,
    };
    let _h = holding();
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.persist(&d).unwrap();
    j.git(&harpoon_deploy::git::GitRecord::Source(src.clone()))
        .unwrap();
    (d, src)
}

#[test]
fn an_edit_appends_a_registration_and_leaves_identity_and_history_alone() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            &url(&repo),
            APP,
            &["--dockerfile", "Dockerfile"],
        ),
        0,
    );
    let (dep, src) = seed_history(&t, "shop", &url(&repo));
    let before = restored(&t);
    let old = before.git.repos["shop"].clone();
    let o = update(
        &t,
        &kc,
        "shop",
        old.at_ms,
        &[
            "--branch",
            "release/1",
            "--dockerfile",
            "Dockerfile.prod",
            "--container-port",
            "9090",
            "--health-path",
            "/healthz",
            "--confirm-change",
        ],
    );
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains("Nothing was deployed, rebuilt, restarted or replaced"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("verified: the Git repository exists and branch release/1"),
        "{}",
        o.stdout
    );
    let after = restored(&t);
    let new = &after.git.repos["shop"];
    assert_eq!(
        (
            new.name.as_str(),
            &new.app,
            new.branch.as_str(),
            new.dockerfile.as_deref(),
            new.container_port,
            new.health_path.as_deref()
        ),
        (
            "shop",
            &old.app,
            "release/1",
            Some("Dockerfile.prod"),
            9090,
            Some("/healthz")
        )
    );
    assert!(
        new.at_ms > old.at_ms,
        "a new version, so a concurrent editor is detected"
    );
    // what the edit must not touch
    assert_eq!(
        (new.url.as_str(), new.cred, &new.username, &new.filters),
        (old.url.as_str(), old.cred, &old.username, &old.filters)
    );
    assert_eq!(
        after.git.sources,
        vec![src],
        "the deployment keeps the source it was built from"
    );
    assert_eq!(after.deployments, vec![dep]);
    assert_eq!(after.control, before.control);
    assert_eq!(t.docker_requests(), 0, "an edit never contacts Docker");
    assert!(sim(&t, &kc, &["repo-list"])
        .stdout
        .contains(&format!("registered-ms {}", new.at_ms)));
}

#[test]
fn a_refused_edit_changes_nothing() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    let plain = gt.0.join("plain");
    std::fs::create_dir(&plain).unwrap();
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    seed_history(&t, "shop", &url(&repo));
    let at = registered_ms(&t, "shop");
    let before = restored(&t);
    let bytes = journal_bytes(&t);
    // (arguments, exit code, words in the reason)
    let cases: Vec<(Vec<String>, i32, &str)> = vec![
        (
            vec![
                "--branch".into(),
                "develop".into(),
                "--confirm-change".into(),
            ],
            6,
            "has no branch",
        ),
        (
            vec!["--url".into(), url(&plain), "--confirm-change".into()],
            6,
            "is not a Git repository",
        ),
        (
            vec![
                "--url".into(),
                url(&gt.0.join("gone")),
                "--confirm-change".into(),
            ],
            6,
            "does not exist",
        ),
        (
            vec!["--branch".into(), "main".into(), "--confirm-change".into()],
            6,
            "nothing to change",
        ),
        (
            vec![
                "--dockerfile".into(),
                String::new(),
                "--confirm-change".into(),
            ],
            2,
            "cannot be empty",
        ),
        (
            vec![
                "--container-port".into(),
                "0".into(),
                "--confirm-change".into(),
            ],
            2,
            "port",
        ),
        (
            vec!["--branch".into(), "release/1".into()],
            6,
            "has deployment history",
        ),
    ];
    for (args, code, want) in cases {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let o = update(&t, &kc, "shop", at, &refs);
        assert_exit(&o, code);
        assert!(o.stderr.contains(want), "{want}: {}", o.stderr);
        let now = restored(&t);
        assert_eq!(
            (&now.git, &now.deployments, &now.control),
            (&before.git, &before.deployments, &before.control),
            "{args:?}"
        );
    }
    // a source refused before the writer opens: not a byte of the journal moved
    assert_exit(
        &update(
            &t,
            &kc,
            "shop",
            at,
            &["--url", &url(&plain), "--confirm-change"],
        ),
        6,
    );
    assert_eq!(journal_bytes(&t), bytes);
    // an unknown name
    assert_exit(&update(&t, &kc, "nope", at, &["--branch", "main"]), 6);
    assert!(scratch_clean(&t));
}

#[test]
fn an_edit_made_from_a_stale_view_is_refused_and_does_not_overwrite_the_newer_one() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    let seen_by_both = registered_ms(&t, "shop");
    assert_exit(
        &update(&t, &kc, "shop", seen_by_both, &["--container-port", "4000"]),
        0,
    );
    let o = update(&t, &kc, "shop", seen_by_both, &["--container-port", "5000"]);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("was changed after you opened it"),
        "{}",
        o.stderr
    );
    assert_eq!(restored(&t).git.repos["shop"].container_port, 4000);
    // a plain re-registration is also a new version
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            &url(&repo),
            APP,
            &["--branch", "release/1"],
        ),
        0,
    );
    assert_exit(
        &update(
            &t,
            &kc,
            "shop",
            registered_ms(&t, "shop") - 1,
            &["--container-port", "1"],
        ),
        6,
    );
}

const PLANTED: &str = "tok-PLANTED-must-never-print";

/// A credentialed https registration whose token has been saved (to the in-memory store).
fn credentialed(t: &T, kc: &MemorySecretStore, url: &str) -> u64 {
    assert_exit(&add_repo(t, kc, "shop", url, APP, &["--username", "ci"]), 0);
    assert_exit(
        &crun(
            t,
            &SHIPPING,
            kc,
            format!("{PLANTED}\n").as_bytes(),
            &[
                "repo-secret",
                "--simulate",
                "--name",
                "shop",
                "--kind",
                "credential",
            ],
        ),
        0,
    );
    registered_ms(t, "shop")
}

#[test]
fn a_saved_credential_follows_only_an_address_with_the_same_destination() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let at = credentialed(&t, &kc, "https://git.example.com/acme/shop.git");
    let before = restored(&t);
    let bytes = journal_bytes(&t);
    // each of these would send the token somewhere other than git.example.com:443 over https, or names a
    // destination that two parsers could read differently
    for other in [
        // another host, or a look-alike
        "https://git.evil.example/acme/shop.git",
        "https://git.example.com.evil.example/acme/shop.git",
        "https://git.example.com./acme/shop.git",
        // the authority a URL parser and libcurl read differently
        "https://git.evil.example#@git.example.com/acme/shop.git",
        "https://git.evil.example?@git.example.com/acme/shop.git",
        "https://git.evil.example:8443#@git.example.com/acme/shop.git",
        "https://git.evil.example\\@git.example.com/acme/shop.git",
        "https://git%2eexample.com/acme/shop.git",
        "https://ci@git.evil.example@git.example.com/acme/shop.git",
        // port and protocol
        "https://git.example.com:8443/acme/shop.git",
        "http://git.example.com/acme/shop.git",
        "http://127.0.0.1:1/acme/shop.git",
        "ssh://git@git.example.com/acme/shop.git",
        "git@git.example.com:acme/shop.git",
        // nothing to send a credential to, or no way to say where
        "file:///tmp/shop",
        "https://",
        "https://git.example.com:/acme/shop.git",
    ] {
        let o = update(&t, &kc, "shop", at, &["--url", other]);
        assert_ne!(o.code, 0, "{other} must be refused: {}", o.stdout);
        assert!(!o.stdout.contains("updated"), "{other}: {}", o.stdout);
        for text in [&o.stdout, &o.stderr] {
            assert!(!text.contains(PLANTED), "{other}: a secret in the output");
        }
        let now = restored(&t);
        assert_eq!(
            (&now.git, &now.control),
            (&before.git, &before.control),
            "{other}"
        );
    }
    assert_eq!(
        restored(&t).git.repos["shop"].url,
        "https://git.example.com/acme/shop.git"
    );
    // every refusal happens before the journal writer opens
    assert_eq!(journal_bytes(&t), bytes, "a refused edit moved the journal");
    // the same destination written differently is fine, and the credential settings are untouched
    for ok in [
        "https://git.example.com/acme/shop2.git",
        "https://GIT.example.com:443/acme/shop3.git",
        "https://ci@git.example.com/acme/shop4.git",
    ] {
        let at = registered_ms(&t, "shop");
        let o = update(&t, &kc, "shop", at, &["--url", ok]);
        assert_exit(&o, 0);
        assert!(o.stdout.contains("not verified yet"), "{ok}: {}", o.stdout);
    }
    let r = restored(&t).git.repos["shop"].clone();
    assert_eq!(
        (r.url.as_str(), r.cred, r.username.as_deref()),
        (
            "https://ci@git.example.com/acme/shop4.git",
            harpoon_deploy::git::CredKind::Token,
            Some("ci")
        )
    );
}

#[test]
fn a_credentialed_registration_with_an_address_that_cannot_be_read_unambiguously_cannot_move() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    // registered before this rule existed (`?` ends this authority for libcurl): its destination is unknowable
    let at = credentialed(&t, &kc, "https://git.example.com?ref=1/acme/shop.git");
    let before = restored(&t);
    for other in [
        "https://git.example.com?ref=1/acme/other.git",
        "https://git.example.com/acme/shop.git",
    ] {
        let o = update(&t, &kc, "shop", at, &["--url", other]);
        assert_exit(&o, 6);
        assert!(o.stderr.contains("unambiguously"), "{other}: {}", o.stderr);
    }
    assert_eq!(restored(&t).git, before.git);
    // an edit that leaves the address alone is not blocked by it
    assert_exit(
        &update(&t, &kc, "shop", at, &["--container-port", "4001"]),
        0,
    );
    assert_eq!(restored(&t).git.repos["shop"].container_port, 4001);
}

#[test]
fn a_registration_without_a_credential_may_still_change_its_source_freely() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            "https://git.example.com/acme/shop.git",
            APP,
            &[],
        ),
        0,
    );
    let at = registered_ms(&t, "shop");
    assert_exit(
        &update(
            &t,
            &kc,
            "shop",
            at,
            &["--url", "https://other.example.org/acme/shop.git"],
        ),
        0,
    );
}

#[test]
fn an_edit_cannot_store_a_deployment_setting_that_deploy_would_refuse() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            "https://git.example.com/acme/shop.git",
            APP,
            &[],
        ),
        0,
    );
    let at = registered_ms(&t, "shop");
    let before = restored(&t);
    let bytes = journal_bytes(&t);
    for (args, want) in [
        (vec!["--health-path", "healthz"], "must start with '/'"),
        (vec!["--health-path", "/health z"], "control or space"),
        (
            vec!["--health-path", "/health\nHost: x"],
            "control or space",
        ),
        (
            vec!["--dockerfile", "/etc/passwd"],
            "relative path inside the build context",
        ),
        (
            vec!["--dockerfile", "../Dockerfile"],
            "relative path inside the build context",
        ),
        (
            vec!["--dockerfile", "a/../../b"],
            "relative path inside the build context",
        ),
        (
            vec!["--dockerfile", "./Dockerfile"],
            "relative path inside the build context",
        ),
        (
            vec!["--dockerfile", "Docker\nfile"],
            "relative path inside the build context",
        ),
        (vec!["--container-port", "0"], "port number"),
        (vec!["--container-port", "65536"], "port number"),
    ] {
        let o = update(&t, &kc, "shop", at, &args);
        assert!(o.code == 6 || o.code == 2, "{args:?}: exit {}", o.code);
        assert!(o.stderr.contains(want), "{args:?}: {}", o.stderr);
    }
    assert_eq!(restored(&t).git, before.git);
    assert_eq!(journal_bytes(&t), bytes);
    // values deploy accepts are stored
    let o = update(
        &t,
        &kc,
        "shop",
        at,
        &[
            "--health-path",
            "/health?probe=1",
            "--dockerfile",
            "docker/Dockerfile.prod",
            "--container-port",
            "65535",
        ],
    );
    assert_exit(&o, 0);
    let r = &restored(&t).git.repos["shop"];
    assert_eq!(
        (
            r.health_path.as_deref(),
            r.dockerfile.as_deref(),
            r.container_port
        ),
        (
            Some("/health?probe=1"),
            Some("docker/Dockerfile.prod"),
            65535
        )
    );
}

#[test]
fn a_source_change_is_refused_while_events_from_the_old_source_are_held() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, second) = fixture(&gt.0);
    let gt2 = Tmp::new();
    let (other, _, _) = fixture(&gt2.0);
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    assert_exit(
        &sim(
            &t,
            &kc,
            &[
                "git-trigger",
                "--simulate",
                "--repo",
                "shop",
                "--delivery",
                "d1",
                "--revision",
                &second,
            ],
        ),
        0,
    );
    let at = registered_ms(&t, "shop");
    let o = update(&t, &kc, "shop", at, &["--url", &url(&other)]);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("held Git events"), "{}", o.stderr);
    assert_eq!(restored(&t).git.repos["shop"].url, url(&repo));
    // the branch can still be corrected: it does not change which source the held revisions belong to
    assert_exit(&update(&t, &kc, "shop", at, &["--branch", "release/1"]), 0);
}

#[test]
fn an_edit_starts_a_fresh_polling_baseline_so_a_new_source_is_not_compared_with_the_old_one() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    assert_exit(&add_repo(&t, &kc, "shop", &url(&repo), APP, &[]), 0);
    let poll = || sim(&t, &kc, &["watch", "--simulate", "--once", "--queue-only"]);
    assert_exit(&poll(), 0);
    commit(&repo, "VERSION", "three\n");
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert_exit(
        &update(
            &t,
            &kc,
            "shop",
            registered_ms(&t, "shop"),
            &["--container-port", "4000"],
        ),
        0,
    );
    let o = poll();
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("first seen"),
        "the observation made before the edit is not a baseline: {}",
        o.stdout
    );
    assert_eq!(
        restored(&t).control.events.len(),
        0,
        "nothing is queued from the old observation"
    );
    // without an edit, the same movement is noticed as before
    commit(&repo, "VERSION", "four\n");
    let o = poll();
    assert!(o.stdout.contains("queued as"), "{}", o.stdout);
}

// ------------------------------------------------------------------ a poll that straddles an edit

/// `repo-update` run directly, without the fork gate's `holding()`. Only the race test uses it: there the poll
/// is already inside `spawning`, which keeps every other test from forking or opening a journal meanwhile.
fn update_direct(
    t: &T,
    kc: &MemorySecretStore,
    name: &str,
    expect: u64,
    extra: &[&str],
) -> Outcome {
    let mut a: Vec<String> = [
        "repo-update",
        "--simulate",
        "--name",
        name,
        "--expect-registered-ms",
        &expect.to_string(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.extend(extra.iter().map(|s| s.to_string()));
    a.extend(["--runtime-dir".to_string(), t.rt.display().to_string()]);
    let mut input: &[u8] = b"";
    let mut svc = Services {
        secrets: kc,
        stdin: &mut input,
        stdin_is_terminal: false,
        next: None,
    };
    run_with(&a, &SHIPPING, &mut svc)
}

/// Releases a held request even when an assertion fails first, so the server thread never waits on a dead test.
struct ReleaseOnDrop<'a>(&'a HttpGit);
impl Drop for ReleaseOnDrop<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[test]
fn a_poll_that_starts_before_an_edit_and_finishes_after_it_discards_what_it_saw() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    let served = gt.0.join("served");
    std::fs::create_dir_all(&served).unwrap();
    let bare = served.join("shop.git");
    git(
        &gt.0,
        &[
            "clone",
            "-q",
            "--bare",
            repo.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let server = HttpGit::start(served, "x-access-token", "tok-race-ONE");
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            &server.url("shop.git"),
            APP,
            &["--username", "x-access-token"],
        ),
        0,
    );
    assert_exit(
        &crun(
            &t,
            &SHIPPING,
            &kc,
            b"tok-race-ONE\n",
            &[
                "repo-secret",
                "--simulate",
                "--name",
                "shop",
                "--kind",
                "credential",
            ],
        ),
        0,
    );
    let poll = || sim(&t, &kc, &["watch", "--simulate", "--once", "--queue-only"]);
    let push = |body: &str| {
        commit(&repo, "VERSION", body);
        git(&repo, &["push", "-q", bare.to_str().unwrap(), "main"]);
    };
    let wait = std::time::Duration::from_secs(30);
    // the same poll on another thread, built from owned values only (the test fixture itself is not Sync); like
    // `crun`, it runs inside the fork gate's `spawning`
    let rt = t.rt.display().to_string();
    let poll_in_background = || {
        let a: Vec<String> = [
            "watch",
            "--simulate",
            "--once",
            "--queue-only",
            "--runtime-dir",
            &rt,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut input: &[u8] = b"";
        let mut svc = Services {
            secrets: &kc,
            stdin: &mut input,
            stdin_is_terminal: false,
            next: None,
        };
        spawning(|| run_with(&a, &SHIPPING, &mut svc))
    };

    // the baseline
    let o = poll();
    assert_exit(&o, 0);
    assert!(o.stdout.contains("first seen"), "{}", o.stdout);

    // control: with the request held and then released and NO edit, the poll records what it saw and queues it
    push("two-b\n");
    server.hold_next_request();
    std::thread::scope(|sc| {
        let _release = ReleaseOnDrop(&server);
        let running = sc.spawn(poll_in_background);
        assert!(
            server.wait_until_held(wait),
            "the poll never reached the server"
        );
        server.release();
        let o = running.join().unwrap();
        assert_exit(&o, 0);
        assert!(o.stdout.contains("queued as"), "{}", o.stdout);
    });
    let seen_after_control = restored(&t).git.seen.clone();
    assert_eq!(restored(&t).control.events.len(), 1);

    // the race: the poll has sent its request when the registration is edited; it finishes afterwards
    push("three\n");
    let at = registered_ms(&t, "shop");
    server.hold_next_request();
    std::thread::scope(|sc| {
        let _release = ReleaseOnDrop(&server);
        let running = sc.spawn(poll_in_background);
        assert!(
            server.wait_until_held(wait),
            "the poll never reached the server"
        );
        assert_exit(
            &update_direct(&t, &kc, "shop", at, &["--container-port", "4242"]),
            0,
        );
        server.release();
        let o = running.join().unwrap();
        assert_exit(&o, 0);
        assert!(
            o.stdout
                .contains("its registration changed while it was being polled"),
            "{}",
            o.stdout
        );
        assert!(!o.stdout.contains("queued as"), "{}", o.stdout);
    });
    let after = restored(&t);
    assert_eq!(
        after.git.repos["shop"].container_port, 4242,
        "the edit stands"
    );
    assert_eq!(
        after.git.seen, seen_after_control,
        "the stale observation was not recorded"
    );
    assert_eq!(
        after.control.events.len(),
        1,
        "the stale observation queued nothing"
    );

    // the next pass starts from the new registration: a fresh baseline, still nothing queued
    let o = poll();
    assert_exit(&o, 0);
    assert!(o.stdout.contains("first seen"), "{}", o.stdout);
    assert_eq!(restored(&t).control.events.len(), 1);
}

// ------------------------------------------------------------------ a deployed release is not touched by an edit

#[test]
fn editing_a_deployed_application_leaves_its_release_ledger_configuration_and_workload_exactly_as_they_were(
) {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _, _) = fixture(&gt.0);
    // a succeeded, serving release: deployment, pinned configuration, image/container/port lease in the ledger,
    // and its running container in the fake Docker
    let (_image, cid) = seed_release(&t, true, &[("MODE", "one")]);
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            &url(&repo),
            APP,
            &["--dockerfile", "Dockerfile"],
        ),
        0,
    );
    let prev: DeploymentId = PREV.parse().unwrap();
    {
        let _h = holding();
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.git(&harpoon_deploy::git::GitRecord::Source(
            harpoon_deploy::git::SourceRec {
                deployment: prev.clone(),
                repo: "shop".into(),
                url: url(&repo),
                selector: harpoon_deploy::git::SelectorKind::Branch,
                requested: "main".into(),
                commit: "0123456789abcdef0123456789abcdef01234567".into(),
                trigger: "manual".into(),
                delivery: None,
                at_ms: 104,
            },
        ))
        .unwrap();
    }
    let app: ApplicationId = APP.parse().unwrap();
    let workload = |t: &T| {
        t.h.docker.with(|s| {
            s.containers
                .iter()
                .map(|c| {
                    (
                        c.id.clone(),
                        c.name.clone(),
                        c.running,
                        c.started_at.clone(),
                        c.restart_count,
                        c.host_port,
                    )
                })
                .collect::<Vec<_>>()
        })
    };
    let before = restored(&t);
    let serving_before =
        harpoon_deploy::serving::serving_release(&before.ledger, &before.deployments, &app);
    assert!(serving_before.is_some(), "the fixture must be serving");
    let (workload_before, requests_before) = (workload(&t), t.docker_requests());
    assert_eq!(workload_before.len(), 1);
    assert!(workload_before[0].2, "the container is running");

    let at = registered_ms(&t, "shop");
    let o = update(
        &t,
        &kc,
        "shop",
        at,
        &[
            "--branch",
            "release/1",
            "--dockerfile",
            "docker/Dockerfile.prod",
            "--container-port",
            "9090",
            "--health-path",
            "/ready",
            "--confirm-change",
        ],
    );
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains("Nothing was deployed, rebuilt, restarted or replaced"),
        "{}",
        o.stdout
    );

    let after = restored(&t);
    // the registration is new; everything a release is made of is as it was
    assert_eq!(after.git.repos["shop"].container_port, 9090);
    assert_eq!(
        after.deployments, before.deployments,
        "deployment records, pinned configuration included"
    );
    assert_eq!(after.deployments[0].pinned, before.deployments[0].pinned);
    assert_eq!(after.ledger, before.ledger, "the Docker ledger");
    assert_eq!(
        after.config, before.config,
        "configuration definitions and snapshots"
    );
    assert_eq!(after.logs, before.logs);
    assert_eq!(after.control, before.control);
    assert_eq!(
        after.git.sources, before.git.sources,
        "the release keeps its recorded source"
    );
    assert_eq!(after.git.seen, before.git.seen);
    assert_eq!(
        harpoon_deploy::serving::serving_release(&after.ledger, &after.deployments, &app),
        serving_before,
        "the serving release"
    );
    assert_eq!(
        workload(&t),
        workload_before,
        "the running container: same id, state, start time and restarts"
    );
    assert_eq!(
        t.docker_requests(),
        requests_before,
        "Docker was not contacted at all"
    );
    untouched(&t, &cid);
}

// ------------------------------------------------------------------ replacing a registration that has a credential

/// A Keychain that fails the test on any access: registering, replacing and editing must never read, write or
/// delete a secret, so none of them can touch an item that belongs to another runtime.
struct ForbiddenStore;
impl harpoon_deploy::secrets::SecretStore for ForbiddenStore {
    fn put(
        &self,
        _: &harpoon_deploy::secrets::SecretRef,
        _: &harpoon_deploy::secrets::SecretValue,
    ) -> Result<(), harpoon_deploy::secrets::SecretError> {
        panic!("the Keychain must not be written")
    }
    fn get(
        &self,
        _: &harpoon_deploy::secrets::SecretRef,
    ) -> Result<harpoon_deploy::secrets::SecretValue, harpoon_deploy::secrets::SecretError> {
        panic!("the Keychain must not be read")
    }
    fn delete(
        &self,
        _: &harpoon_deploy::secrets::SecretRef,
    ) -> Result<(), harpoon_deploy::secrets::SecretError> {
        panic!("the Keychain must not be deleted from")
    }
}

/// A `repo-add` of the registration `shop` with no access to any Keychain.
fn replace_shop(t: &T, url: &str, extra: &[&str]) -> Outcome {
    replace_named(t, "shop", url, extra)
}

fn replace_named(t: &T, name: &str, url: &str, extra: &[&str]) -> Outcome {
    let mut a: Vec<String> = [
        "repo-add",
        "--simulate",
        "--name",
        name,
        "--url",
        url,
        "--container-port",
        "8080",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.extend(extra.iter().map(|s| s.to_string()));
    a.extend(["--runtime-dir".to_string(), t.rt.display().to_string()]);
    let _h = holding();
    let mut input: &[u8] = b"";
    let mut svc = Services {
        secrets: &ForbiddenStore,
        stdin: &mut input,
        stdin_is_terminal: false,
        next: None,
    };
    run_with(&a, &SHIPPING, &mut svc)
}

/// The saved token, read from the in-memory test store (never a real Keychain).
fn saved_token(kc: &MemorySecretStore) -> Vec<u8> {
    use harpoon_deploy::secrets::SecretStore;
    kc.get(&harpoon_deploy::secrets::SecretRef::new(
        "git/shop/credential",
        1,
    ))
    .expect("the saved token is still there")
    .expose()
    .to_vec()
}

#[test]
fn replacing_a_registration_that_has_a_saved_credential_cannot_move_its_destination() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    credentialed(&t, &kc, "https://git.example.com/acme/shop.git");
    seed_history(&t, "shop", "https://git.example.com/acme/shop.git");
    let before = restored(&t);
    let bytes = journal_bytes(&t);
    for other in [
        // another host, or a look-alike
        "https://git.evil.example/acme/shop.git",
        "https://git.example.com.evil.example/acme/shop.git",
        // an authority that a URL parser and libcurl read differently
        "https://git.evil.example#@git.example.com/acme/shop.git",
        "https://git.evil.example?@git.example.com/acme/shop.git",
        "https://git.evil.example\\@git.example.com/acme/shop.git",
        "https://git%2eexample.com/acme/shop.git",
        // another port or protocol
        "https://git.example.com:8443/acme/shop.git",
        "http://git.example.com/acme/shop.git",
        "http://127.0.0.1:9/acme/shop.git",
        "ssh://git@git.example.com/acme/shop.git",
        "git@git.example.com:acme/shop.git",
        // no destination at all
        "file:///tmp/shop",
        "https://git.example.com:/acme/shop.git",
    ] {
        // both ways a replacement can use a credential, and with none: all are refused
        for extra in [&["--username", "ci"][..], &[][..]] {
            let o = replace_shop(&t, other, extra);
            // 2: not an acceptable URL at all (plain http to a real host); 6: refused by the destination rule
            assert!(
                o.code == 6 || o.code == 2,
                "{other} {extra:?}: exit {}: {}",
                o.code,
                o.stderr
            );
            if o.code == 6 {
                assert!(
                    o.stderr.contains("Nothing was changed"),
                    "{other}: {}",
                    o.stderr
                );
            }
            for text in [&o.stdout, &o.stderr] {
                assert!(!text.contains(PLANTED), "{other}: a secret in the output");
            }
            assert_eq!(
                journal_bytes(&t),
                bytes,
                "{other} {extra:?}: the journal changed"
            );
        }
    }
    let now = restored(&t);
    assert_eq!(
        now, before,
        "registration, application, provenance and credential record are as they were"
    );
    assert_eq!(
        saved_token(&kc),
        PLANTED.as_bytes(),
        "the saved token is untouched"
    );
}

#[test]
fn replacing_a_registration_with_the_same_destination_is_still_allowed_and_keeps_its_identity() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    credentialed(&t, &kc, "https://git.example.com/acme/shop.git");
    let (dep, src) = seed_history(&t, "shop", "https://git.example.com/acme/shop.git");
    let app = restored(&t).git.repos["shop"].app.clone();
    for (url, extra) in [
        (
            "https://git.example.com/acme/shop2.git",
            vec!["--username", "ci", "--branch", "release/1"],
        ),
        (
            "https://GIT.example.com:443/acme/shop3.git",
            vec!["--username", "ci"],
        ),
        (
            "https://ci@git.example.com/acme/shop4.git",
            vec!["--username", "ci"],
        ),
        // dropping the credential at the same destination is a replacement too
        ("https://git.example.com/acme/shop5.git", vec![]),
    ] {
        let o = replace_shop(&t, url, &extra);
        assert_exit(&o, 0);
        let r = restored(&t);
        assert_eq!(r.git.repos["shop"].url, url);
        assert_eq!(
            r.git.repos["shop"].app, app,
            "no --app: the application is kept"
        );
        assert_eq!(r.deployments, vec![dep.clone()]);
        assert_eq!(
            r.git.sources,
            vec![src.clone()],
            "the deployment keeps its recorded source"
        );
    }
    assert_eq!(saved_token(&kc), PLANTED.as_bytes());
}

#[test]
fn a_replacement_without_a_credential_on_record_may_still_change_everything() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            "https://git.example.com/acme/shop.git",
            APP,
            &[],
        ),
        0,
    );
    let app = restored(&t).git.repos["shop"].app.clone();
    for url in [
        "https://other.example.org/acme/shop.git",
        "http://127.0.0.1:9/acme/shop.git",
        "https://other.example.org:8443/acme/shop.git",
    ] {
        assert_exit(&replace_shop(&t, url, &[]), 0);
        let r = restored(&t);
        assert_eq!(
            (r.git.repos["shop"].url.as_str(), &r.git.repos["shop"].app),
            (url, &app)
        );
    }
}

#[test]
fn a_credential_stays_bound_to_its_destination_even_after_a_replacement_that_stopped_using_it() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    credentialed(&t, &kc, "https://git.example.com/acme/shop.git");
    // the registration stops using the credential, at the same destination: allowed, but a token is still on record
    assert_exit(
        &replace_shop(&t, "https://git.example.com/acme/shop.git", &[]),
        0,
    );
    assert_eq!(
        restored(&t).git.repos["shop"].cred,
        harpoon_deploy::git::CredKind::None
    );
    let before = restored(&t);
    let bytes = journal_bytes(&t);
    // so neither a replacement nor an edit may move it, or the token could be picked up again elsewhere
    let o = replace_shop(
        &t,
        "https://git.evil.example/acme/shop.git",
        &["--username", "ci"],
    );
    assert_exit(&o, 6);
    let o = update(
        &t,
        &kc,
        "shop",
        registered_ms(&t, "shop"),
        &["--url", "https://git.evil.example/acme/shop.git"],
    );
    assert_exit(&o, 6);
    assert_eq!(journal_bytes(&t), bytes);
    assert_eq!(restored(&t), before);
    // the same destination can use it again
    assert_exit(
        &replace_shop(
            &t,
            "https://git.example.com/acme/shop.git",
            &["--username", "ci"],
        ),
        0,
    );
    assert_eq!(saved_token(&kc), PLANTED.as_bytes());
}

#[test]
fn a_credentialed_registration_whose_address_cannot_be_read_cannot_be_replaced_by_another_address()
{
    let t = T::new();
    let kc = MemorySecretStore::new();
    let legacy = "https://git.example.com?ref=1/acme/shop.git";
    credentialed(&t, &kc, legacy);
    let before = restored(&t);
    let bytes = journal_bytes(&t);
    for other in [
        "https://git.example.com/acme/shop.git",
        "https://git.example.com?ref=2/acme/shop.git",
    ] {
        let o = replace_shop(&t, other, &["--username", "ci"]);
        assert_exit(&o, 6);
        assert!(o.stderr.contains("unambiguously"), "{other}: {}", o.stderr);
    }
    assert_eq!(journal_bytes(&t), bytes);
    assert_eq!(restored(&t), before);
    // re-registering the very same address (for example to change the branch) is not a move
    assert_exit(
        &replace_shop(&t, legacy, &["--username", "ci", "--branch", "release/1"]),
        0,
    );
}

// ------------------------------------------------------------------ an application's identity is fixed

const THIRD_APP: &str = "app_00000000000000000000000003";

/// A serving release of `APP`, its repository `shop` (with or without a saved credential), a recorded source and
/// a held Git event: everything an identity change could disturb.
fn deployed_and_registered(t: &T, kc: &MemorySecretStore, credential: bool) -> String {
    deployed_registered(t, kc, credential, true)
}

fn deployed_registered(
    t: &T,
    kc: &MemorySecretStore,
    credential: bool,
    held_event: bool,
) -> String {
    let second = "0123456789abcdef0123456789abcdef01234567".to_string();
    seed_release(t, true, &[("MODE", "one")]);
    let shop_url = "https://git.example.com/acme/shop.git".to_string();
    if credential {
        credentialed(t, kc, &shop_url);
    } else {
        assert_exit(&add_repo(t, kc, "shop", &shop_url, APP, &[]), 0);
    }
    {
        let _h = holding();
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.git(&harpoon_deploy::git::GitRecord::Source(
            harpoon_deploy::git::SourceRec {
                deployment: PREV.parse().unwrap(),
                repo: "shop".into(),
                url: shop_url.clone(),
                selector: harpoon_deploy::git::SelectorKind::Branch,
                requested: "main".into(),
                commit: second.clone(),
                trigger: "manual".into(),
                delivery: None,
                at_ms: 104,
            },
        ))
        .unwrap();
    }
    if held_event {
        assert_exit(
            &sim(
                t,
                kc,
                &[
                    "git-trigger",
                    "--simulate",
                    "--repo",
                    "shop",
                    "--delivery",
                    "d1",
                    "--revision",
                    &second,
                ],
            ),
            0,
        );
    }
    shop_url
}

fn identity_cannot_change(credential: bool) {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let url = deployed_and_registered(&t, &kc, credential);
    let app: ApplicationId = APP.parse().unwrap();
    let before = restored(&t);
    let serving_before =
        harpoon_deploy::serving::serving_release(&before.ledger, &before.deployments, &app);
    assert!(serving_before.is_some(), "the fixture must be serving");
    assert_eq!(before.control.events.len(), 1, "a held event exists");
    let (bytes, requests) = (journal_bytes(&t), t.docker_requests());
    let extra_cred: &[&str] = if credential {
        &["--username", "ci"]
    } else {
        &[]
    };
    for other in [SECOND_APP, THIRD_APP] {
        let mut extra = vec!["--app", other];
        extra.extend_from_slice(extra_cred);
        let o = replace_shop(&t, &url, &extra);
        assert_exit(&o, 6);
        assert!(o.stderr.contains("identity is fixed"), "{}", o.stderr);
        assert!(o.stderr.contains("Nothing was changed"), "{}", o.stderr);
        // also together with a change of branch, which must not slip through either
        let mut extra = vec!["--app", other, "--branch", "release/1"];
        extra.extend_from_slice(extra_cred);
        assert_exit(&replace_shop(&t, &url, &extra), 6);
        assert_eq!(journal_bytes(&t), bytes, "{other}: the journal changed");
    }
    let after = restored(&t);
    assert_eq!(
        after, before,
        "registration, deployments, sources, held events and credential record"
    );
    assert_eq!(
        harpoon_deploy::serving::serving_release(&after.ledger, &after.deployments, &app),
        serving_before,
        "the serving release"
    );
    assert_eq!(after.ledger, before.ledger, "the Docker ledger");
    assert_eq!(t.docker_requests(), requests, "Docker was not contacted");
    if credential {
        assert_eq!(saved_token(&kc), PLANTED.as_bytes());
    }
}

#[test]
fn an_uncredentialed_registration_cannot_be_moved_to_another_application() {
    identity_cannot_change(false);
}

#[test]
fn a_credentialed_registration_cannot_be_moved_to_another_application() {
    identity_cannot_change(true);
}

#[test]
fn replacing_with_the_same_or_no_application_is_unchanged() {
    for credential in [false, true] {
        let t = T::new();
        let kc = MemorySecretStore::new();
        let url = deployed_and_registered(&t, &kc, credential);
        let extra_cred: &[&str] = if credential {
            &["--username", "ci"]
        } else {
            &[]
        };
        for (tail, branch) in [(vec!["--app", APP], "release/1"), (vec![], "release/2")] {
            let mut extra = tail;
            extra.extend(["--branch", branch]);
            extra.extend_from_slice(extra_cred);
            assert_exit(&replace_shop(&t, &url, &extra), 0);
            let r = restored(&t).git.repos["shop"].clone();
            assert_eq!(
                (r.app.to_string().as_str(), r.branch.as_str()),
                (APP, branch)
            );
        }
    }
}

#[test]
fn a_first_registration_may_claim_an_unclaimed_application_that_already_has_history() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    // PREV is a succeeded, serving deployment of APP made without any repository
    seed_release(&t, true, &[("MODE", "one")]);
    assert!(restored(&t).git.repos.is_empty());
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            "https://git.example.com/acme/shop.git",
            APP,
            &[],
        ),
        0,
    );
    assert_eq!(restored(&t).git.repos["shop"].app.to_string(), APP);
    // and a brand-new application id, never seen before, is just as acceptable for a new name
    assert_exit(
        &replace_named(
            &t,
            "billing",
            "https://git.example.com/acme/billing.git",
            &["--app", THIRD_APP],
        ),
        0,
    );
}

#[test]
fn the_two_command_identity_transfer_is_refused_at_its_first_step() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            "https://git.example.com/acme/shop.git",
            APP,
            &[],
        ),
        0,
    );
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "billing",
            "https://git.example.com/acme/billing.git",
            SECOND_APP,
            &[],
        ),
        0,
    );
    let before = restored(&t);
    let bytes = journal_bytes(&t);
    // step 1 freed billing's application by re-pointing billing to a made-up one
    let o = replace_named(
        &t,
        "billing",
        "https://git.example.com/acme/billing.git",
        &["--app", THIRD_APP],
    );
    assert_exit(&o, 6);
    assert!(o.stderr.contains("identity is fixed"), "{}", o.stderr);
    // step 2, adopting billing's application, is refused as before
    let o = replace_named(
        &t,
        "shop",
        "https://git.example.com/acme/shop.git",
        &["--app", SECOND_APP],
    );
    assert_exit(&o, 6);
    assert_eq!(journal_bytes(&t), bytes);
    let after = restored(&t);
    assert_eq!(after, before);
    assert_eq!(after.git.repos["billing"].app.to_string(), SECOND_APP);
    assert_eq!(after.git.repos["shop"].app.to_string(), APP);
}

// ------------------------------------------------------------------ a removed registration (readable; nothing writes it)

/// The only way a removal gets into a journal in this build: a test appends the record. No command writes it.
fn removal_record(t: &T, name: &str, at_ms: u64) -> harpoon_deploy::git::RemovedRec {
    let live = restored(t).git.repos[name].clone();
    harpoon_deploy::git::RemovedRec {
        name: live.name,
        app: live.app,
        registration: live.at_ms,
        url: live.url,
        cred: live.cred,
        at_ms,
    }
}

fn retire(t: &T, name: &str, at_ms: u64) {
    let rec = removal_record(t, name, at_ms);
    let _h = holding();
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    j.git(&harpoon_deploy::git::GitRecord::Removed(rec))
        .unwrap();
}

/// The same, for the race tests: the poll or deployment is already inside the fork gate's `spawning`, which keeps
/// every other test from forking or opening a journal, so this opens the journal directly.
fn retire_direct(t: &T, name: &str, at_ms: u64) {
    let rec = removal_record(t, name, at_ms);
    let (mut j, _) = harpoon_deploy_journal::SqliteJournal::open(&t.journal_dir()).unwrap();
    j.git(&harpoon_deploy::git::GitRecord::Removed(rec))
        .unwrap();
}

#[test]
fn a_journal_with_a_removed_registration_reads_back_with_the_application_and_its_history_intact() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    deployed_registered(&t, &kc, false, false);
    let app: ApplicationId = APP.parse().unwrap();
    let before = restored(&t);
    let serving =
        harpoon_deploy::serving::serving_release(&before.ledger, &before.deployments, &app);
    assert!(serving.is_some());
    let requests = t.docker_requests();

    retire(&t, "shop", 5_000_000_000_000);
    let after = restored(&t);
    assert!(after.git.repos.is_empty() && after.git.retired.len() == 1);
    assert_eq!(after.git.retired[0].app, app);
    assert_eq!(after.deployments, before.deployments, "deployment records");
    assert_eq!(after.ledger, before.ledger, "the Docker ledger");
    assert_eq!(after.config, before.config);
    assert_eq!(
        after.git.sources, before.git.sources,
        "the release keeps its recorded source"
    );
    assert_eq!(
        harpoon_deploy::serving::serving_release(&after.ledger, &after.deployments, &app),
        serving,
        "the serving release"
    );
    assert_eq!(t.docker_requests(), requests);
    let list = sim(&t, &kc, &["repo-list"]);
    assert_exit(&list, 0);
    assert!(
        list.stdout.contains("shop  (removed: source disconnected)"),
        "{}",
        list.stdout
    );
    assert!(list.stdout.contains(APP), "{}", list.stdout);
}

#[test]
fn commands_aimed_at_a_removed_name_say_it_was_removed_and_write_nothing() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    deployed_registered(&t, &kc, false, false);
    retire(&t, "shop", 5_000_000_000_000);
    let before = restored(&t);
    let sha = "0123456789abcdef0123456789abcdef01234567";
    for args in [
        vec!["git-deploy", "--simulate", "--repo", "shop"],
        vec![
            "git-trigger",
            "--simulate",
            "--repo",
            "shop",
            "--delivery",
            "d9",
            "--revision",
            sha,
        ],
        vec![
            "repo-update",
            "--simulate",
            "--name",
            "shop",
            "--expect-registered-ms",
            "1",
            "--branch",
            "x",
        ],
    ] {
        let o = sim(&t, &kc, &args);
        assert_exit(&o, 6);
        assert!(
            o.stderr.contains("was removed") && o.stderr.contains("keeps its history"),
            "{args:?}: {}",
            o.stderr
        );
    }
    let o = crun(
        &t,
        &SHIPPING,
        &kc,
        b"x\n",
        &[
            "repo-secret",
            "--simulate",
            "--name",
            "shop",
            "--kind",
            "credential",
        ],
    );
    assert_exit(&o, 6);
    assert!(o.stderr.contains("was removed"), "{}", o.stderr);
    assert_eq!(restored(&t), before);
}

#[test]
fn only_the_original_name_can_claim_a_removed_application_and_only_explicitly() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let url = deployed_registered(&t, &kc, false, false);
    retire(&t, "shop", 5_000_000_000_000);
    let before = restored(&t);
    let bytes = journal_bytes(&t);
    // another name: refused, with or without confirmation
    for extra in [vec!["--app", APP], vec!["--app", APP, "--confirm-reclaim"]] {
        let o = replace_named(&t, "blog", "https://git.example.com/acme/blog.git", &extra);
        assert_exit(&o, 6);
        assert!(
            o.stderr.contains("only that name can claim it again"),
            "{}",
            o.stderr
        );
    }
    // the original name: naming the application is not enough
    let o = replace_shop(&t, &url, &["--app", APP]);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("--confirm-reclaim"), "{}", o.stderr);
    assert_eq!(journal_bytes(&t), bytes, "refusals wrote nothing");
    assert_eq!(restored(&t), before);
    // with the explicit identity and confirmation it connects a source to the same application again
    assert_exit(
        &replace_shop(&t, &url, &["--app", APP, "--confirm-reclaim"]),
        0,
    );
    let after = restored(&t);
    assert_eq!(after.git.repos["shop"].app.to_string(), APP);
    assert_eq!(after.git.retired.len(), 1, "the removal stays on record");
    assert_eq!(
        (&after.deployments, &after.ledger, &after.git.sources),
        (&before.deployments, &before.ledger, &before.git.sources)
    );
}

#[test]
fn a_removed_application_with_held_events_cannot_be_claimed_again_until_they_are_resolved() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let url = deployed_registered(&t, &kc, false, true); // a held event from before the removal
    retire(&t, "shop", 5_000_000_000_000);
    assert_eq!(restored(&t).control.events.len(), 1);
    let bytes = journal_bytes(&t);
    let o = replace_shop(&t, &url, &["--app", APP, "--confirm-reclaim"]);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("held Git events"), "{}", o.stderr);
    assert_eq!(journal_bytes(&t), bytes);
    assert!(restored(&t).git.repos.is_empty());
}

#[test]
fn a_reused_name_gets_a_fresh_application_and_inherits_no_polling_state_or_held_events() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let url = deployed_registered(&t, &kc, false, true);
    // an observation made under the old registration
    {
        let _h = holding();
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        let live = restored(&t).git.repos["shop"].clone();
        j.git(&harpoon_deploy::git::GitRecord::Seen(
            harpoon_deploy::git::SeenRec {
                repo: "shop".into(),
                ref_name: "refs/heads/main".into(),
                commit: "a".repeat(40),
                at_ms: live.at_ms + 1,
            },
        ))
        .unwrap();
    }
    retire(&t, "shop", 5_000_000_000_000);
    assert!(
        restored(&t).git.seen.is_empty(),
        "polling state does not outlive the registration"
    );
    assert_exit(&replace_shop(&t, &url, &[]), 0);
    let after = restored(&t);
    let fresh = after.git.repos["shop"].app.clone();
    assert_ne!(
        fresh.to_string(),
        APP,
        "a fresh application, not the removed one"
    );
    assert!(after.git.seen.is_empty());
    assert!(
        after.control.events_of(&fresh).next().is_none(),
        "no held event of the old application"
    );
    // the old application is untouched, still registered to nobody, and still the original name's
    assert_eq!(after.git.retired_owner(&APP.parse().unwrap()), Some("shop"));
    assert!(after
        .deployments
        .iter()
        .all(|d| d.application_id.to_string() == APP));
    assert_eq!(after.control.events.len(), 1);
}

#[test]
fn a_removed_registrations_credential_destination_survives_every_removal_and_registration_cycle() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    credentialed(&t, &kc, "https://git.example.com/acme/shop.git");
    retire(&t, "shop", 5_000_000_000_000);
    let before = restored(&t);
    let bytes = journal_bytes(&t);
    let elsewhere = [
        "https://git.evil.example/acme/shop.git",
        "https://git.evil.example#@git.example.com/acme/shop.git",
        "https://git.example.com:8443/acme/shop.git",
        "ssh://git@git.example.com/acme/shop.git",
        "file:///tmp/shop",
    ];
    for url in elsewhere {
        for extra in [
            &["--username", "ci"][..],
            &[][..],
            &["--app", APP, "--confirm-reclaim"][..],
        ] {
            let o = replace_shop(&t, url, extra);
            assert!(
                o.code == 6 || o.code == 2,
                "{url} {extra:?}: exit {}: {}",
                o.code,
                o.stderr
            );
            for text in [&o.stdout, &o.stderr] {
                assert!(!text.contains(PLANTED), "a secret in the output");
            }
        }
    }
    assert_eq!(journal_bytes(&t), bytes, "refusals wrote nothing");
    assert_eq!(restored(&t), before);
    assert_eq!(
        saved_token(&kc),
        PLANTED.as_bytes(),
        "the saved token is untouched"
    );
    // the same destination is fine, with a fresh application; then a second and a third cycle change nothing
    for (i, url) in [
        "https://git.example.com/acme/a.git",
        "https://GIT.example.com:443/acme/b.git",
    ]
    .iter()
    .enumerate()
    {
        assert_exit(&replace_shop(&t, url, &["--username", "ci"]), 0);
        retire(&t, "shop", 5_000_000_000_100 + i as u64);
        for url in elsewhere {
            let o = replace_shop(&t, url, &[]);
            assert!(
                o.code == 6 || o.code == 2,
                "cycle {i}: {url}: exit {}",
                o.code
            );
        }
    }
    assert_eq!(restored(&t).git.retired.len(), 3);
    assert_eq!(saved_token(&kc), PLANTED.as_bytes());
}

// ------------------------------------------------------------------ races with a removal (the request is held mid-flight)

/// `shop` registered with a saved token against a local smart-HTTP server that can hold a request.
fn served_shop(
    t: &T,
    kc: &MemorySecretStore,
    gt: &Tmp,
) -> (HttpGit, std::path::PathBuf, std::path::PathBuf) {
    let (repo, _, _) = fixture(&gt.0);
    let served = gt.0.join("served");
    std::fs::create_dir_all(&served).unwrap();
    let bare = served.join("shop.git");
    git(
        &gt.0,
        &[
            "clone",
            "-q",
            "--bare",
            repo.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let server = HttpGit::start(served, "x-access-token", "tok-race-ONE");
    assert_exit(
        &add_repo(
            t,
            kc,
            "shop",
            &server.url("shop.git"),
            APP,
            &["--username", "x-access-token"],
        ),
        0,
    );
    assert_exit(
        &crun(
            t,
            &SHIPPING,
            kc,
            b"tok-race-ONE\n",
            &[
                "repo-secret",
                "--simulate",
                "--name",
                "shop",
                "--kind",
                "credential",
            ],
        ),
        0,
    );
    (server, repo, bare)
}

/// Run `args` on another thread (inside the fork gate like `crun`), with only owned values captured.
fn in_background(rt: &str, kc: &MemorySecretStore, args: &[&str]) -> Outcome {
    let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    a.extend(["--runtime-dir".to_string(), rt.to_string()]);
    let mut input: &[u8] = b"";
    let mut svc = Services {
        secrets: kc,
        stdin: &mut input,
        stdin_is_terminal: false,
        next: None,
    };
    spawning(|| run_with(&a, &SHIPPING, &mut svc))
}

#[test]
fn a_poll_that_straddles_a_removal_records_nothing_and_leaves_a_readable_journal() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (server, repo, bare) = served_shop(&t, &kc, &gt);
    let rt = t.rt.display().to_string();
    let wait = std::time::Duration::from_secs(30);
    assert_exit(
        &sim(&t, &kc, &["watch", "--simulate", "--once", "--queue-only"]),
        0,
    ); // the baseline
    let seen_before = restored(&t).git.seen.clone();
    assert!(!seen_before.is_empty());
    commit(&repo, "VERSION", "three\n");
    git(&repo, &["push", "-q", bare.to_str().unwrap(), "main"]);

    server.hold_next_request();
    std::thread::scope(|sc| {
        let _release = ReleaseOnDrop(&server);
        let running = sc
            .spawn(|| in_background(&rt, &kc, &["watch", "--simulate", "--once", "--queue-only"]));
        assert!(
            server.wait_until_held(wait),
            "the poll never reached the server"
        );
        retire_direct(&t, "shop", 5_000_000_000_000);
        server.release();
        let o = running.join().unwrap();
        assert_exit(&o, 0);
        assert!(
            o.stdout
                .contains("its registration was removed while it was being polled"),
            "{}",
            o.stdout
        );
        assert!(!o.stdout.contains("queued as"), "{}", o.stdout);
    });
    // an observation recorded after the removal would make this restore fail
    let after = restored(&t);
    assert!(after.git.repos.is_empty() && after.git.retired.len() == 1);
    assert!(
        after.git.seen.is_empty(),
        "the removal dropped the baseline and the stale poll added none"
    );
    assert!(after.control.events.is_empty(), "nothing was queued");
}

#[test]
fn a_deployment_that_straddles_a_removal_or_an_edit_records_no_source_and_deploys_nothing() {
    for removal in [true, false] {
        let t = T::new();
        let kc = MemorySecretStore::new();
        let gt = Tmp::new();
        let (server, _repo, _bare) = served_shop(&t, &kc, &gt);
        let rt = t.rt.display().to_string();
        let wait = std::time::Duration::from_secs(30);
        let at = registered_ms(&t, "shop");
        let before = restored(&t);
        server.hold_next_request();
        let o = std::thread::scope(|sc| {
            let _release = ReleaseOnDrop(&server);
            let running = sc
                .spawn(|| in_background(&rt, &kc, &["git-deploy", "--simulate", "--repo", "shop"]));
            assert!(
                server.wait_until_held(wait),
                "the deployment never reached the server"
            );
            if removal {
                retire_direct(&t, "shop", 5_000_000_000_000);
            } else {
                assert_exit(
                    &update_direct(&t, &kc, "shop", at, &["--container-port", "4242"]),
                    0,
                );
            }
            server.release();
            running.join().unwrap()
        });
        assert_exit(&o, 7);
        let want = if removal {
            "was removed while its source was being fetched"
        } else {
            "was changed"
        };
        assert!(
            o.stderr.contains(want) && o.stderr.contains("nothing was deployed"),
            "removal={removal}: {}",
            o.stderr
        );
        // a source record for a removed registration would make this restore fail
        let after = restored(&t);
        assert!(after.git.sources.is_empty(), "no source was recorded");
        assert_eq!(
            after.deployments, before.deployments,
            "no deployment was started"
        );
        assert_eq!(after.ledger, before.ledger);
        assert_eq!(t.docker_requests(), 0);
        assert!(scratch_clean(&t), "the checkout was cleaned up");
        assert_eq!(after.git.repos.is_empty(), removal);
    }
}

// ------------------------------------------------------------------ repo-remove

fn marker(t: &T) -> std::path::PathBuf {
    std::fs::canonicalize(&t.rt)
        .unwrap()
        .join("git-removal.enabled")
}

/// What the person authorizing removal for this runtime does by hand: a mode-600 file naming the runtime.
fn authorize_removal(t: &T) {
    use std::os::unix::fs::PermissionsExt;
    let canon = std::fs::canonicalize(&t.rt).unwrap();
    std::fs::write(
        marker(t),
        format!("harpoon-git-removal 1\nruntime={}\n", canon.display()),
    )
    .unwrap();
    std::fs::set_permissions(marker(t), std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn run_removal(t: &T, gate: &Gate, args: &[&str]) -> Outcome {
    run_removal_as("repo-remove", t, gate, args)
}

fn run_removal_as(command: &str, t: &T, gate: &Gate, args: &[&str]) -> Outcome {
    let mut a: Vec<String> = [command, "--simulate"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(args.iter().map(|s| s.to_string()));
    a.extend(["--runtime-dir".to_string(), t.rt.display().to_string()]);
    let mut input: &[u8] = b"";
    // a Keychain that fails the test on any access: removal must never touch a secret
    let mut svc = Services {
        secrets: &ForbiddenStore,
        stdin: &mut input,
        stdin_is_terminal: false,
        next: None,
    };
    run_with(&a, gate, &mut svc)
}

/// `repo-remove` as a person runs it, in the fork gate like every other command.
fn remove_with(t: &T, gate: &Gate, name: &str, expect: u64, extra: &[&str]) -> Outcome {
    let e = expect.to_string();
    let mut args = vec!["--name", name, "--expect-registered-ms", &e];
    args.extend_from_slice(extra);
    let _h = holding();
    run_removal(t, gate, &args)
}

fn remove_shop(t: &T) -> Outcome {
    remove_with(
        t,
        &ACCEPTANCE,
        "shop",
        registered_ms(t, "shop"),
        &["--confirm-removal"],
    )
}

/// The same without the fork gate, for the race tests (the poll or deployment is inside `spawning`).
fn remove_direct(t: &T, name: &str) -> Outcome {
    let e = registered_ms(t, name).to_string();
    run_removal(
        t,
        &ACCEPTANCE,
        &[
            "--name",
            name,
            "--expect-registered-ms",
            &e,
            "--confirm-removal",
        ],
    )
}

fn db_bytes(t: &T) -> Vec<u8> {
    std::fs::read(t.journal_dir().join(harpoon_deploy_journal::SQLITE_FILE)).unwrap()
}

fn backup_files(t: &T) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(t.journal_dir().join("backups"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn schema_of(t: &T) -> i64 {
    harpoon_deploy_journal::inspect_file(&t.journal_dir().join(harpoon_deploy_journal::SQLITE_FILE))
        .unwrap()
        .schema
}

/// Nothing about the journal changed, and no backup was made.
fn assert_untouched(
    t: &T,
    bytes: &[(String, Vec<u8>)],
    before: &harpoon_deploy_journal::Restored,
    what: &str,
) {
    assert_eq!(&journal_bytes(t), bytes, "{what}: the journal changed");
    assert_eq!(&restored(t), before, "{what}: the state changed");
    assert!(backup_files(t).is_empty(), "{what}: a backup was made");
    assert!(
        restored(t).git.retired.is_empty(),
        "{what}: a removal was recorded"
    );
}

#[test]
fn removal_is_closed_without_the_gate_the_marker_and_an_isolated_runtime_and_nothing_is_opened_to_find_out(
) {
    use std::os::unix::fs::PermissionsExt;
    let t = T::new();
    let kc = MemorySecretStore::new();
    deployed_registered(&t, &kc, false, false);
    let at = registered_ms(&t, "shop");
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    let go = |gate: &Gate| remove_with(&t, gate, "shop", at, &["--confirm-removal"]);

    // the shipping gate, the desktop's gate and a closed gate: refused, however well the runtime is authorized
    authorize_removal(&t);
    for (label, gate) in [
        ("shipping", &SHIPPING),
        ("closed", &CLOSED),
        ("trigger-open", &OPEN),
    ] {
        let o = go(gate);
        assert_exit(&o, 6);
        assert!(
            o.stderr.contains("not enabled in this build"),
            "{label}: {}",
            o.stderr
        );
        assert_untouched(&t, &bytes, &before, label);
    }
    std::fs::remove_file(marker(&t)).unwrap();

    // a capable build without a marker
    let o = go(&ACCEPTANCE);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("marker") && o.stderr.contains("missing"),
        "{}",
        o.stderr
    );
    assert_untouched(&t, &bytes, &before, "no marker");

    let canon = std::fs::canonicalize(&t.rt).unwrap();
    let good = format!("harpoon-git-removal 1\nruntime={}\n", canon.display());
    let write = |text: &str, mode: u32| {
        let _ = std::fs::remove_file(marker(&t));
        std::fs::write(marker(&t), text).unwrap();
        std::fs::set_permissions(marker(&t), std::fs::Permissions::from_mode(mode)).unwrap();
    };
    for (label, text, mode, want) in [
        ("group-readable", good.clone(), 0o640, "mode 600"),
        ("world-writable", good.clone(), 0o666, "mode 600"),
        (
            "another runtime",
            "harpoon-git-removal 1\nruntime=/private/tmp/some-other-runtime\n".to_string(),
            0o600,
            "does not name this runtime",
        ),
        (
            "another version",
            format!("harpoon-git-removal 2\nruntime={}\n", canon.display()),
            0o600,
            "does not name this runtime",
        ),
        ("empty", String::new(), 0o600, "does not name this runtime"),
        (
            "path only",
            format!("runtime={}\n", canon.display()),
            0o600,
            "does not name this runtime",
        ),
        (
            "too large",
            format!("{good}{}", "#".repeat(5000)),
            0o600,
            "too large",
        ),
    ] {
        write(&text, mode);
        let o = go(&ACCEPTANCE);
        assert_exit(&o, 6);
        assert!(o.stderr.contains(want), "{label}: {}", o.stderr);
        assert_untouched(&t, &bytes, &before, label);
    }
    // a link to a good marker is not a marker
    std::fs::remove_file(marker(&t)).unwrap();
    let elsewhere = t.rt.join("elsewhere.txt");
    std::fs::write(&elsewhere, &good).unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&elsewhere, marker(&t)).unwrap();
    let o = go(&ACCEPTANCE);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("not a regular file"), "{}", o.stderr);
    assert_untouched(&t, &bytes, &before, "a link");

    // and with a good marker it works (the same call, so the refusals above were about authorization alone)
    std::fs::remove_file(marker(&t)).unwrap();
    authorize_removal(&t);
    assert_exit(&go(&ACCEPTANCE), 0);
}

#[test]
fn the_first_removal_backs_up_the_journal_stamps_schema_2_and_changes_nothing_else() {
    for credential in [false, true] {
        let t = T::new();
        let kc = MemorySecretStore::new();
        deployed_registered(&t, &kc, credential, false);
        authorize_removal(&t);
        let app: ApplicationId = APP.parse().unwrap();
        let before = restored(&t);
        let rows_before = harpoon_deploy_journal::inspect_file(
            &t.journal_dir().join(harpoon_deploy_journal::SQLITE_FILE),
        )
        .unwrap()
        .rows;
        let serving =
            harpoon_deploy::serving::serving_release(&before.ledger, &before.deployments, &app);
        assert!(serving.is_some());
        assert_eq!(schema_of(&t), 1);
        let requests = t.docker_requests();

        let o = remove_shop(&t);
        assert_exit(&o, 0);
        assert!(
            o.stdout
                .contains("removed the registration of repository shop"),
            "{}",
            o.stdout
        );
        assert!(
            o.stdout
                .contains("Nothing was built, deployed, stopped or restarted"),
            "{}",
            o.stdout
        );
        assert!(
            o.stdout.contains("backup:") && o.stdout.contains("sha256:"),
            "{}",
            o.stdout
        );

        // the verified backup: private, faithful, with its digest beside it
        use std::os::unix::fs::PermissionsExt;
        let files = backup_files(&t);
        assert_eq!(files.len(), 2, "{files:?}");
        let (db, sidecar) = (&files[0], &files[1]);
        assert!(
            db.ends_with(".sqlite3") && sidecar.ends_with(".sqlite3.sha256"),
            "{files:?}"
        );
        let dir = t.journal_dir().join("backups");
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for f in &files {
            assert_eq!(
                std::fs::metadata(dir.join(f)).unwrap().permissions().mode() & 0o777,
                0o600,
                "{f}"
            );
        }
        let digest = harpoon_deploy_git::sha256::to_hex(&harpoon_deploy_git::sha256::sha256(
            &std::fs::read(dir.join(db)).unwrap(),
        ));
        assert!(std::fs::read_to_string(dir.join(sidecar))
            .unwrap()
            .starts_with(&format!("{digest}  {db}\n")));
        assert!(o.stdout.contains(&digest), "the digest is reported");
        let facts = harpoon_deploy_journal::inspect_file(&dir.join(db)).unwrap();
        assert_eq!((facts.integrity.as_str(), facts.schema), ("ok", 1));
        assert_eq!(
            facts.rows, rows_before,
            "record for record the journal as it was"
        );
        assert_eq!(
            harpoon_deploy_journal::replay_file(&dir.join(db)).unwrap(),
            before,
            "and it replays to the same state"
        );

        // the journal: one record and the stamp, nothing else
        assert_eq!(schema_of(&t), 2);
        let after = restored(&t);
        assert!(after.git.repos.is_empty() && after.git.retired.len() == 1);
        assert_eq!(after.git.retired[0].app, app);
        assert_eq!(after.deployments, before.deployments, "deployment records");
        assert_eq!(after.ledger, before.ledger, "the Docker ledger");
        assert_eq!(
            after.config, before.config,
            "configuration and the saved-credential record"
        );
        assert_eq!(after.git.sources, before.git.sources, "provenance");
        assert_eq!(after.control, before.control);
        assert_eq!(
            harpoon_deploy::serving::serving_release(&after.ledger, &after.deployments, &app),
            serving,
            "the serving release"
        );
        let rows_after = harpoon_deploy_journal::inspect_file(
            &t.journal_dir().join(harpoon_deploy_journal::SQLITE_FILE),
        )
        .unwrap()
        .rows;
        assert_eq!(
            &rows_after[..rows_before.len()],
            &rows_before[..],
            "append-only: every earlier record is as it was"
        );
        assert_eq!(rows_after.len(), rows_before.len() + 1);
        assert_eq!(t.docker_requests(), requests, "Docker was not contacted");
        if credential {
            assert_eq!(
                saved_token(&kc),
                PLANTED.as_bytes(),
                "the stored credential is untouched"
            );
        }
        let list = sim(&t, &kc, &["repo-list"]);
        assert!(
            list.stdout.contains("shop  (removed: source disconnected)"),
            "{}",
            list.stdout
        );
    }
}

#[test]
fn a_removal_that_may_not_happen_is_refused_before_anything_is_written() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    deployed_registered(&t, &kc, false, false);
    authorize_removal(&t);
    let at = registered_ms(&t, "shop");
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    for (label, args, code, want) in [
        (
            "no confirmation",
            vec!["--name", "shop"],
            2,
            "--expect-registered-ms is required",
        ),
        (
            "not confirmed",
            vec!["--name", "shop", "--expect-registered-ms", "1"],
            6,
            "--confirm-removal",
        ),
        (
            "no name",
            vec!["--expect-registered-ms", "1", "--confirm-removal"],
            2,
            "--name is required",
        ),
        (
            "a bad version",
            vec![
                "--name",
                "shop",
                "--expect-registered-ms",
                "yesterday",
                "--confirm-removal",
            ],
            2,
            "registration time",
        ),
        (
            "a stale version",
            vec![
                "--name",
                "shop",
                "--expect-registered-ms",
                "1",
                "--confirm-removal",
            ],
            6,
            "was changed after you opened it",
        ),
        (
            "an unknown name",
            vec![
                "--name",
                "nope",
                "--expect-registered-ms",
                "1",
                "--confirm-removal",
            ],
            6,
            "is not registered",
        ),
    ] {
        let o = {
            let _h = holding();
            run_removal(&t, &ACCEPTANCE, &args)
        };
        assert_exit(&o, code);
        assert!(o.stderr.contains(want), "{label}: {}", o.stderr);
        assert_untouched(&t, &bytes, &before, label);
    }
    // an edit since the person looked: the version they were shown is gone
    assert_exit(
        &update(
            &t,
            &kc,
            "shop",
            at,
            &["--container-port", "4001", "--confirm-change"],
        ),
        0,
    );
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    let o = remove_with(&t, &ACCEPTANCE, "shop", at, &["--confirm-removal"]);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("was changed after you opened it"),
        "{}",
        o.stderr
    );
    assert_untouched(&t, &bytes, &before, "an edit");
    assert!(restored(&t).git.repos.contains_key("shop"));
}

#[test]
fn removal_is_refused_while_held_or_selected_events_exist_and_never_dismisses_them() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    deployed_registered(&t, &kc, false, true); // a held event
    authorize_removal(&t);
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    assert_eq!(before.control.events.len(), 1);
    let o = remove_shop(&t);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("held or selected Git events")
            && o.stderr.contains("never dismisses or replays"),
        "{}",
        o.stderr
    );
    assert_untouched(&t, &bytes, &before, "a held event");
    // selecting it is not resolving it
    let rev = before.control.events[0].rec.revision.clone();
    assert_exit(
        &sim(&t, &kc, &["resume", "--app", APP, "--revision", &rev]),
        0,
    );
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    assert!(before
        .control
        .pending_selection(&APP.parse().unwrap())
        .is_some());
    let o = remove_shop(&t);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("held or selected Git events"),
        "{}",
        o.stderr
    );
    assert_untouched(&t, &bytes, &before, "a selected event");
    // resolved by the person (dismissed), it no longer blocks, and the event record is kept as history
    assert_exit(&sim(&t, &kc, &["resume", "--app", APP, "--dismiss"]), 0);
    let events = restored(&t).control.events.clone();
    assert_exit(&remove_shop(&t), 0);
    assert_eq!(
        restored(&t).control.events,
        events,
        "removal neither dismissed nor rewrote any event"
    );
}

#[test]
fn a_repeated_or_stale_removal_is_refused_and_adds_no_record_or_backup() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    deployed_registered(&t, &kc, false, false);
    authorize_removal(&t);
    let at = registered_ms(&t, "shop");
    assert_exit(
        &remove_with(&t, &ACCEPTANCE, "shop", at, &["--confirm-removal"]),
        0,
    );
    let (bytes, after, files) = (journal_bytes(&t), restored(&t), backup_files(&t));
    for expect in [at, at + 1, 1] {
        let o = remove_with(&t, &ACCEPTANCE, "shop", expect, &["--confirm-removal"]);
        assert_exit(&o, 6);
        assert!(
            o.stderr.contains("was already removed") && o.stderr.contains("keeps its history"),
            "{}",
            o.stderr
        );
    }
    assert_eq!(
        (journal_bytes(&t), restored(&t), backup_files(&t)),
        (bytes, after, files)
    );
    assert_eq!(restored(&t).git.retired.len(), 1);
}

#[test]
fn a_backup_that_cannot_be_made_stops_the_removal_before_it_is_written() {
    use std::os::unix::fs::PermissionsExt;
    for (label, make) in [
        ("a file in the way", 0),
        ("a link in the way", 1),
        ("a directory open to others", 2),
    ] {
        let t = T::new();
        let kc = MemorySecretStore::new();
        deployed_registered(&t, &kc, false, false);
        authorize_removal(&t);
        let b = t.journal_dir().join("backups");
        match make {
            0 => std::fs::write(&b, "x").unwrap(),
            1 => std::os::unix::fs::symlink(&t.rt, &b).unwrap(),
            _ => {
                std::fs::create_dir(&b).unwrap();
                std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let (bytes, before) = (db_bytes(&t), restored(&t));
        let o = remove_shop(&t);
        assert!(o.code != 0, "{label}");
        assert!(
            o.stderr.contains("No removal was written"),
            "{label}: {}",
            o.stderr
        );
        assert_eq!(db_bytes(&t), bytes, "{label}: the journal changed");
        assert_eq!(restored(&t), before, "{label}");
        assert_eq!(
            schema_of(&t),
            1,
            "{label}: schema 2 was stamped without a removal"
        );
    }
}

#[test]
fn a_removal_cannot_start_while_another_writer_holds_the_journal() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    deployed_registered(&t, &kc, false, false);
    authorize_removal(&t);
    let at = registered_ms(&t, "shop");
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    {
        // another process is writing (this holds the journal's exclusive writer lock)
        let _writer = harpoon_deploy_journal::SqliteJournal::open(&t.journal_dir()).unwrap();
        let o = run_removal(
            &t,
            &ACCEPTANCE,
            &[
                "--name",
                "shop",
                "--expect-registered-ms",
                &at.to_string(),
                "--confirm-removal",
            ],
        );
        assert_ne!(o.code, 0);
        assert!(
            o.stderr.contains("another Harpoon process holds"),
            "{}",
            o.stderr
        );
    }
    assert_untouched(&t, &bytes, &before, "a busy journal");
    // and once it is free, the same request succeeds
    assert_exit(
        &remove_with(&t, &ACCEPTANCE, "shop", at, &["--confirm-removal"]),
        0,
    );
}

#[test]
fn restoring_the_backup_offline_returns_the_journal_to_before_the_removal_and_loses_what_came_after(
) {
    let t = T::new();
    let kc = MemorySecretStore::new();
    deployed_registered(&t, &kc, false, false);
    authorize_removal(&t);
    let before = restored(&t);
    assert_exit(&remove_shop(&t), 0);
    let files = backup_files(&t);
    let backup = t.journal_dir().join("backups").join(&files[0]);
    // something happens after the removal: it will be lost by the restore, as documented
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "late",
            "https://git.example.com/acme/late.git",
            SECOND_APP,
            &[],
        ),
        0,
    );
    // the person's procedure: the writer is gone; verify the digest; copy the backup over the live file
    let side = std::fs::read_to_string(format!("{}.sha256", backup.display())).unwrap();
    let digest = side.split_whitespace().next().unwrap().to_string();
    assert_eq!(
        harpoon_deploy_git::sha256::to_hex(&harpoon_deploy_git::sha256::sha256(
            &std::fs::read(&backup).unwrap()
        )),
        digest
    );
    let live = t.journal_dir().join(harpoon_deploy_journal::SQLITE_FILE);
    std::fs::copy(&backup, &live).unwrap();
    assert_eq!(
        schema_of(&t),
        1,
        "schema 1 again, with nothing edited by hand"
    );
    let back = restored(&t);
    assert_eq!(back, before, "exactly the journal from before the removal");
    assert!(
        back.git.repos.contains_key("shop") && !back.git.repos.contains_key("late"),
        "what came after the backup is gone"
    );
    // the backup itself was never altered, and the journal can be upgraded again from here
    assert_eq!(
        harpoon_deploy_git::sha256::to_hex(&harpoon_deploy_git::sha256::sha256(
            &std::fs::read(&backup).unwrap()
        )),
        digest
    );
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert_exit(&remove_shop(&t), 0);
    assert_eq!(
        backup_files(&t).len(),
        4,
        "a second upgrade makes its own backup: {:?}",
        backup_files(&t)
    );
}

#[test]
fn removal_exists_only_as_repo_remove() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert!(harpoon_deploy_cli::command_names().contains(&"repo-remove"));
    for cmd in ["repo-drop", "repo-delete", "repo-retire", "repo-unregister"] {
        assert!(!harpoon_deploy_cli::command_names().contains(&cmd), "{cmd}");
        assert_exit(&sim(&t, &kc, &[cmd, "--name", "shop"]), 2);
    }
}

// ------------------------------------------------------------------ a real removal racing a poll or a deployment

#[test]
fn a_poll_that_straddles_a_real_removal_records_nothing_and_leaves_a_readable_upgraded_journal() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (server, repo, bare) = served_shop(&t, &kc, &gt);
    authorize_removal(&t);
    let rt = t.rt.display().to_string();
    let wait = std::time::Duration::from_secs(30);
    assert_exit(
        &sim(&t, &kc, &["watch", "--simulate", "--once", "--queue-only"]),
        0,
    );
    commit(&repo, "VERSION", "three\n");
    git(&repo, &["push", "-q", bare.to_str().unwrap(), "main"]);
    server.hold_next_request();
    std::thread::scope(|sc| {
        let _release = ReleaseOnDrop(&server);
        let running = sc
            .spawn(|| in_background(&rt, &kc, &["watch", "--simulate", "--once", "--queue-only"]));
        assert!(
            server.wait_until_held(wait),
            "the poll never reached the server"
        );
        // the real command, while the poll waits on the network
        assert_exit(&remove_direct(&t, "shop"), 0);
        server.release();
        let o = running.join().unwrap();
        assert_exit(&o, 0);
        assert!(
            o.stdout
                .contains("its registration was removed while it was being polled"),
            "{}",
            o.stdout
        );
    });
    let after = restored(&t); // an observation recorded after the removal would make this fail
    assert!(
        after.git.repos.is_empty() && after.git.retired.len() == 1 && after.git.seen.is_empty()
    );
    assert!(
        after.control.events.is_empty(),
        "the stale poll queued nothing"
    );
    assert_eq!(schema_of(&t), 2);
    assert_eq!(backup_files(&t).len(), 2);
}

#[test]
fn a_deployment_that_straddles_a_real_removal_records_no_source_and_deploys_nothing() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (server, _repo, _bare) = served_shop(&t, &kc, &gt);
    authorize_removal(&t);
    let rt = t.rt.display().to_string();
    let wait = std::time::Duration::from_secs(30);
    let before = restored(&t);
    server.hold_next_request();
    let o = std::thread::scope(|sc| {
        let _release = ReleaseOnDrop(&server);
        let running =
            sc.spawn(|| in_background(&rt, &kc, &["git-deploy", "--simulate", "--repo", "shop"]));
        assert!(
            server.wait_until_held(wait),
            "the deployment never reached the server"
        );
        assert_exit(&remove_direct(&t, "shop"), 0);
        server.release();
        running.join().unwrap()
    });
    assert_exit(&o, 7);
    assert!(
        o.stderr
            .contains("was removed while its source was being fetched")
            && o.stderr.contains("nothing was deployed"),
        "{}",
        o.stderr
    );
    let after = restored(&t); // a source recorded for a removed registration would make this fail
    assert!(after.git.sources.is_empty());
    assert_eq!(
        (after.deployments, after.ledger),
        (before.deployments, before.ledger)
    );
    assert_eq!(t.docker_requests(), 0);
    assert!(scratch_clean(&t));
}

#[test]
fn two_removals_racing_for_the_same_registration_record_exactly_one() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    deployed_registered(&t, &kc, false, false);
    authorize_removal(&t);
    let at = registered_ms(&t, "shop");
    let attempt = |gate: &Gate| {
        let e = at.to_string();
        run_removal(
            &t,
            gate,
            &[
                "--name",
                "shop",
                "--expect-registered-ms",
                &e,
                "--confirm-removal",
            ],
        )
    };
    let _g = holding();
    // the journal's writer lock makes them take turns; the second finds the registration already gone
    let (a, b) = (attempt(&ACCEPTANCE), attempt(&ACCEPTANCE));
    let codes = (a.code, b.code);
    assert_eq!(codes, (0, 6), "{codes:?}\n{}\n{}", a.stderr, b.stderr);
    assert!(b.stderr.contains("was already removed"), "{}", b.stderr);
    assert_eq!(restored(&t).git.retired.len(), 1);
    assert_eq!(backup_files(&t).len(), 2);
}

// ------------------------------------------------------------------ identity after a real removal

#[test]
fn poll_event_ids_follow_the_registrations_incarnation_through_repeated_removal_and_reclaim() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let gt = Tmp::new();
    let (repo, _first, _second) = fixture(&gt.0);
    let url = format!("file://{}", repo.display());
    assert_exit(&add_repo(&t, &kc, "shop", &url, APP, &[]), 0);
    authorize_removal(&t);
    let poll = || {
        let o = sim(&t, &kc, &["watch", "--simulate", "--once", "--queue-only"]);
        assert_exit(&o, 0);
        o.stdout
    };
    let ids = |t: &T| -> Vec<(String, String)> {
        restored(t)
            .control
            .events
            .iter()
            .map(|e| (e.rec.delivery.clone(), e.status.as_str().to_string()))
            .collect()
    };
    let reclaim = || {
        let o = sim(
            &t,
            &kc,
            &[
                "repo-add",
                "--simulate",
                "--name",
                "shop",
                "--url",
                &url,
                "--app",
                APP,
                "--container-port",
                "8080",
                "--confirm-reclaim",
            ],
        );
        assert_exit(&o, 0);
    };
    assert!(poll().contains("first seen"));

    // incarnation 0: the original ids
    let c1 = commit(&repo, "VERSION", "one\n");
    assert!(poll().contains("queued as"));
    assert_eq!(
        ids(&t),
        vec![(format!("poll:shop:{c1}"), "held".to_string())]
    );
    assert_exit(&sim(&t, &kc, &["resume", "--app", APP, "--dismiss"]), 0);
    let legacy = ids(&t);
    assert_eq!(
        legacy,
        vec![(format!("poll:shop:{c1}"), "dismissed".to_string())]
    );

    // remove, claim it again: incarnation 1, a fresh baseline, and nothing queued by the switch itself
    assert_exit(&remove_shop(&t), 0);
    assert!(poll_after_removal_is_not_polled(&t, &kc));
    reclaim();
    assert!(
        poll().contains("first seen"),
        "the reclaimed registration starts from a baseline"
    );
    assert_eq!(ids(&t), legacy, "nothing was replayed or rewritten");
    let c2 = commit(&repo, "VERSION", "two\n");
    poll();
    assert_eq!(
        ids(&t)[1..],
        [(format!("poll:shop:r1:{c2}"), "held".to_string())]
    );
    // an ordinary edit does not move the incarnation
    assert_exit(
        &update(
            &t,
            &kc,
            "shop",
            registered_ms(&t, "shop"),
            &["--container-port", "4001", "--confirm-change"],
        ),
        0,
    );
    poll(); // an edit re-baselines polling (see the edit tests), so this pass queues nothing
    let c3 = commit(&repo, "VERSION", "three\n");
    poll();
    assert_eq!(ids(&t).last().unwrap().0, format!("poll:shop:r1:{c3}"));
    // the branch goes back to a commit an earlier incarnation already dismissed: it is queued, not suppressed
    git(&repo, &["reset", "-q", "--hard", &c1]);
    poll();
    assert_eq!(
        ids(&t).last().unwrap(),
        &(format!("poll:shop:r1:{c1}"), "held".to_string())
    );
    assert_eq!(
        ids(&t)[0],
        (format!("poll:shop:{c1}"), "dismissed".to_string()),
        "the earlier event is exactly as it was"
    );

    // a second cycle is a new namespace again
    assert_exit(&sim(&t, &kc, &["resume", "--app", APP, "--dismiss"]), 0);
    assert_exit(&remove_shop(&t), 0);
    assert_eq!(
        backup_files(&t).len(),
        2,
        "only the first removal upgrades the journal and backs it up"
    );
    reclaim();
    poll();
    let c4 = commit(&repo, "VERSION", "four\n");
    poll();
    assert_eq!(ids(&t).last().unwrap().0, format!("poll:shop:r2:{c4}"));
    assert_eq!(restored(&t).git.retired.len(), 2);
}

/// After a removal the name is not polled at all: a pass finds nothing registered to watch.
fn poll_after_removal_is_not_polled(t: &T, kc: &MemorySecretStore) -> bool {
    let o = sim(t, kc, &["watch", "--simulate", "--once", "--queue-only"]);
    o.code == 6 && o.stderr.contains("no repository is registered to watch")
}

/// What `repo-list` says about `shop`'s last deployment, or `None` when it names none.
fn listed_deployment(t: &T, kc: &MemorySecretStore) -> Option<String> {
    let o = sim(t, kc, &["repo-list"]);
    assert_exit(&o, 0);
    o.stdout.lines().find_map(|l| {
        l.trim()
            .strip_prefix("last deployment from it: ")
            .map(String::from)
    })
}

#[test]
fn repo_list_names_a_registrations_last_deployment_only_when_its_own_application_made_it() {
    // an ordinary registration shows its deployment
    let t = T::new();
    let kc = MemorySecretStore::new();
    let url = deployed_registered(&t, &kc, false, false);
    let shown = listed_deployment(&t, &kc).expect("an ordinary registration shows its deployment");
    assert!(shown.starts_with(PREV), "{shown}");

    // removed: the retired application is listed as disconnected, with no deployment line of a registration
    authorize_removal(&t);
    assert_exit(&remove_shop(&t), 0);
    assert_eq!(listed_deployment(&t, &kc), None);

    // the name reused by a fresh application does not show the retired application's deployment
    assert_exit(&replace_shop(&t, &url, &[]), 0);
    assert_ne!(restored(&t).git.repos["shop"].app.to_string(), APP);
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    assert_eq!(listed_deployment(&t, &kc), None);
    // listing is display only
    assert_eq!(journal_bytes(&t), bytes);
    assert_eq!(restored(&t), before);
}

#[test]
fn repo_list_shows_the_history_of_an_explicitly_reclaimed_application() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let url = deployed_registered(&t, &kc, false, false);
    authorize_removal(&t);
    assert_exit(&remove_shop(&t), 0);
    assert_exit(
        &replace_shop(&t, &url, &["--app", APP, "--confirm-reclaim"]),
        0,
    );
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    assert_eq!(before.git.repos["shop"].app.to_string(), APP);
    let shown = listed_deployment(&t, &kc).expect("a reclaimed application keeps its history");
    assert!(shown.starts_with(PREV), "{shown}");
    assert_eq!(journal_bytes(&t), bytes);
    assert_eq!(restored(&t), before);
}

#[test]
fn a_fresh_registration_of_a_removed_name_inherits_no_event_identity_polling_state_or_credential() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let url = deployed_registered(&t, &kc, true, false);
    authorize_removal(&t);
    assert_exit(&remove_shop(&t), 0);
    let removed = restored(&t);
    // a fresh registration: a new application, incarnation 0, no baseline, no events, and the same credential destination
    assert_exit(&replace_shop(&t, &url, &["--username", "ci"]), 0);
    let now = restored(&t);
    let fresh = now.git.repos["shop"].app.clone();
    assert_ne!(fresh.to_string(), APP);
    assert_eq!(now.git.incarnation(&fresh), 0);
    assert!(now.git.seen.is_empty() && now.control.events_of(&fresh).next().is_none());
    assert_eq!(
        now.git.retired, removed.git.retired,
        "the history of removals is untouched"
    );
    // but the credential still belongs to its destination, through the removal
    let bytes = journal_bytes(&t);
    let o = replace_shop(
        &t,
        "https://git.evil.example/acme/shop.git",
        &["--username", "ci"],
    );
    assert_exit(&o, 6);
    assert_eq!(journal_bytes(&t), bytes);
    assert_eq!(saved_token(&kc), PLANTED.as_bytes());
    // a second removal and registration cycle cannot release it either
    assert_exit(&remove_shop(&t), 0);
    assert_exit(
        &replace_shop(&t, "https://git.evil.example/acme/shop.git", &[]),
        6,
    );
    assert_eq!(restored(&t).git.retired.len(), 2);
}

// ------------------------------------------------------------------ app-remove

/// `app-remove` as a person runs it.
fn remove_app_with(t: &T, gate: &Gate, name: &str, expect: u64, extra: &[&str]) -> Outcome {
    let e = expect.to_string();
    let mut args = vec!["--name", name, "--expect-registered-ms", &e];
    args.extend_from_slice(extra);
    let _h = holding();
    run_removal_as("app-remove", t, gate, &args)
}

fn remove_app(t: &T) -> Outcome {
    remove_app_with(
        t,
        &ACCEPTANCE,
        "shop",
        registered_ms(t, "shop"),
        &["--confirm-application-removal"],
    )
}

const SHOP_URL: &str = "https://git.example.com/acme/shop.git";

#[test]
fn a_never_deployed_application_is_removed_with_one_record_a_backup_and_nothing_purged() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(&add_repo(&t, &kc, "shop", SHOP_URL, APP, &[]), 0);
    assert_exit(
        &sim(
            &t,
            &kc,
            &[
                "config-set",
                "--scope",
                &format!("app:{APP}"),
                "--name",
                "MODE",
                "--value",
                "one",
            ],
        ),
        0,
    );
    authorize_removal(&t);
    let before = restored(&t);
    assert_eq!(schema_of(&t), 1);

    let o = remove_app(&t);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("removed application") && o.stdout.contains("not purged"),
        "{}",
        o.stdout
    );
    let after = restored(&t);
    assert_eq!(schema_of(&t), 2);
    assert_eq!(backup_files(&t).len(), 2, "the backup and its digest");
    assert_eq!(after.git.retired.len(), 1);
    assert_eq!(after.git.retired_owner(&APP.parse().unwrap()), Some("shop"));
    assert!(after.git.repos.is_empty());
    // only the registration changed: configuration, ledger and control history are as they were
    assert_eq!(after.config, before.config);
    assert_eq!(after.ledger, before.ledger);
    assert_eq!(after.control, before.control);
    assert_eq!(after.deployments, before.deployments);
    assert_eq!(t.docker_requests(), 0);

    // it is gone for good: removing again says so and writes nothing
    let (bytes, state) = (journal_bytes(&t), restored(&t));
    let again = remove_app_with(
        &t,
        &ACCEPTANCE,
        "shop",
        1,
        &["--confirm-application-removal"],
    );
    assert_exit(&again, 6);
    assert!(again.stderr.contains("already removed"), "{}", again.stderr);
    assert_eq!(journal_bytes(&t), bytes);
    assert_eq!(restored(&t), state);
}

#[test]
fn application_removal_refuses_whatever_it_cannot_leave_behind_and_changes_nothing() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    // a deployed application with a serving release, a recorded source and a held event
    deployed_registered(&t, &kc, false, true);
    authorize_removal(&t);
    let at = registered_ms(&t, "shop");
    let (bytes, before) = (journal_bytes(&t), restored(&t));

    // unconfirmed, and with the shipping gate: refused before the journal is consulted
    let o = remove_app_with(&t, &ACCEPTANCE, "shop", at, &[]);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("--confirm-application-removal"),
        "{}",
        o.stderr
    );
    let o = remove_app_with(
        &t,
        &SHIPPING,
        "shop",
        at,
        &["--confirm-application-removal"],
    );
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("not enabled in this build"),
        "{}",
        o.stderr
    );
    assert_untouched(&t, &bytes, &before, "gate and confirmation");

    // the held event is reported first (shared with `repo-remove`); once it is gone the deployment still refuses
    let o = remove_app_with(
        &t,
        &ACCEPTANCE,
        "shop",
        at,
        &["--confirm-application-removal"],
    );
    assert_exit(&o, 6);
    assert!(o.stderr.contains("held or selected"), "{}", o.stderr);
    assert_untouched(&t, &bytes, &before, "held event");

    let t = T::new();
    deployed_registered(&t, &kc, false, false);
    authorize_removal(&t);
    let at = registered_ms(&t, "shop");
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    let o = remove_app_with(
        &t,
        &ACCEPTANCE,
        "shop",
        at,
        &["--confirm-application-removal"],
    );
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("deployment record(s)") && o.stderr.contains("repo-remove"),
        "{}",
        o.stderr
    );
    assert_untouched(&t, &bytes, &before, "deployed application");
    assert_eq!(t.docker_requests(), 0);
    // the registration-only operation is still available for it, with its own confirmation
    assert_exit(&remove_shop(&t), 0);
}

#[test]
fn a_stale_view_cannot_remove_an_application() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(&add_repo(&t, &kc, "shop", SHOP_URL, APP, &[]), 0);
    authorize_removal(&t);
    let at = registered_ms(&t, "shop");
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    let o = remove_app_with(
        &t,
        &ACCEPTANCE,
        "shop",
        at + 1,
        &["--confirm-application-removal"],
    );
    assert_exit(&o, 6);
    assert_untouched(&t, &bytes, &before, "stale version");
}

#[test]
fn a_fresh_registration_of_the_same_name_inherits_nothing_from_a_removed_application() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(&add_repo(&t, &kc, "shop", SHOP_URL, APP, &[]), 0);
    assert_exit(
        &sim(
            &t,
            &kc,
            &[
                "config-set",
                "--scope",
                &format!("app:{APP}"),
                "--name",
                "OLD_ONLY",
                "--value",
                "x",
            ],
        ),
        0,
    );
    authorize_removal(&t);
    assert_exit(&remove_app(&t), 0);

    assert_exit(
        &sim(
            &t,
            &kc,
            &[
                "repo-add",
                "--simulate",
                "--name",
                "shop",
                "--url",
                SHOP_URL,
                "--container-port",
                "8080",
            ],
        ),
        0,
    );
    let after = restored(&t);
    let fresh = after.git.repos["shop"].app.clone();
    assert_ne!(
        fresh.to_string(),
        APP,
        "a fresh application, not the removed one"
    );
    assert_eq!(after.git.incarnation(&fresh), 0);
    assert!(
        after.git.sources.iter().all(|s| after
            .deployments
            .iter()
            .any(|d| d.id == s.deployment && d.application_id != fresh))
            || after.git.sources.is_empty()
    );
    assert!(after.control.events_of(&fresh).next().is_none());
    assert!(!after.deployments.iter().any(|d| d.application_id == fresh));
    let shown = sim(&t, &kc, &["config-show", "--app", &fresh.to_string()]);
    assert!(!shown.stdout.contains("OLD_ONLY"), "{}", shown.stdout);
    // the removed application keeps its name and is not claimable without the explicit, confirmed reclaim
    let o = add_repo(&t, &kc, "shop", SHOP_URL, APP, &["--new"]);
    assert_exit(&o, 6);
    assert_eq!(after.git.retired_owner(&APP.parse().unwrap()), Some("shop"));
}

fn project_folder(t: &T) -> String {
    let dir = t.rt.join("project");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Dockerfile"), "FROM scratch\n").unwrap();
    dir.display().to_string()
}

#[test]
fn a_plain_deploy_cannot_bring_back_a_removed_never_deployed_application() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(&add_repo(&t, &kc, "shop", SHOP_URL, APP, &[]), 0);
    authorize_removal(&t);
    assert_exit(&remove_app(&t), 0);
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    let ctx = project_folder(&t);
    let o = sim(
        &t,
        &kc,
        &[
            "deploy",
            "--simulate",
            "--app",
            APP,
            "--project-dir",
            &ctx,
            "--container-port",
            "8080",
        ],
    );
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("will not bring it back") && o.stderr.contains("--confirm-reclaim"),
        "{}",
        o.stderr
    );
    assert_eq!(journal_bytes(&t), bytes);
    assert_eq!(restored(&t), before);
    assert_eq!(t.docker_requests(), 0);

    // an application that was deployed before its registration was removed is not retired by that
    let t = T::new();
    deployed_registered(&t, &kc, false, false);
    authorize_removal(&t);
    assert_exit(&remove_shop(&t), 0);
    let ctx = project_folder(&t);
    let o = sim(
        &t,
        &kc,
        &[
            "deploy",
            "--simulate",
            "--app",
            APP,
            "--project-dir",
            &ctx,
            "--container-port",
            "8080",
        ],
    );
    assert!(!o.stderr.contains("will not bring it back"), "{}", o.stderr);
}

#[test]
fn capability_refusals_name_the_operation_that_was_refused() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(&add_repo(&t, &kc, "shop", SHOP_URL, APP, &[]), 0);
    let at = registered_ms(&t, "shop");
    let (bytes, before) = (journal_bytes(&t), restored(&t));
    // the shipping gate, then a capable build whose runtime has no marker
    for gate in [&SHIPPING, &ACCEPTANCE] {
        let reg = remove_with(&t, gate, "shop", at, &["--confirm-removal"]);
        let app = remove_app_with(&t, gate, "shop", at, &["--confirm-application-removal"]);
        for (o, what, other) in [
            (&reg, "a Git registration", "an application"),
            (&app, "an application", "a Git registration"),
        ] {
            assert_exit(o, 6);
            assert!(
                o.stderr.contains(&format!("removing {what} is not ")) && !o.stderr.contains(other),
                "{}",
                o.stderr
            );
        }
    }
    assert_untouched(&t, &bytes, &before, "refusals");
}

/// What every build that can open a journal must do with each schema, through the command line (the reader the desktop
/// links is the same code): read schema 1 without writing, read a schema-2 journal that holds a removal, and refuse a
/// newer schema by name. In each case the journal's bytes are unchanged afterwards. (Older builds meeting schema 2
/// fail closed; that is measured against an old binary in docs/results/DEPLOY_GIT_REGISTRATION_REMOVAL.md.)
#[test]
fn a_journal_of_schema_1_and_2_is_read_without_a_write_and_a_newer_schema_is_refused_by_name() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(&add_repo(&t, &kc, "shop", SHOP_URL, APP, &[]), 0);
    assert_eq!(schema_of(&t), 1);
    let reads = |t: &T| -> Vec<Outcome> {
        ["repo-list", "status", "events"]
            .iter()
            .map(|c| sim(t, &kc, &[c]))
            .collect()
    };
    let bytes = journal_bytes(&t);
    for o in reads(&t) {
        assert_exit(&o, 0);
    }
    assert!(sim(&t, &kc, &["repo-list"]).stdout.contains("shop"));
    assert_eq!(journal_bytes(&t), bytes, "reading schema 1 wrote nothing");

    // schema 2: the first removal upgrades it, and every later read sees the removal and writes nothing
    authorize_removal(&t);
    assert_exit(&remove_shop(&t), 0);
    assert_eq!(schema_of(&t), 2);
    let bytes = journal_bytes(&t);
    for o in reads(&t) {
        assert_exit(&o, 0);
    }
    let listed = sim(&t, &kc, &["repo-list"]);
    assert!(
        listed.stdout.contains("removed: source disconnected"),
        "{}",
        listed.stdout
    );
    assert_eq!(restored(&t).git.retired.len(), 1);
    assert_eq!(journal_bytes(&t), bytes, "reading schema 2 wrote nothing");

    // schema 3 (a journal from a newer Harpoon): refused by name, nothing repaired or discarded
    let db = t.journal_dir().join(harpoon_deploy_journal::SQLITE_FILE);
    let done = std::process::Command::new("sqlite3")
        .arg(&db)
        .arg("INSERT INTO schema_migrations(version) VALUES (3)")
        .status()
        .expect("the sqlite3 command-line tool is needed to make a schema-3 journal");
    assert!(done.success());
    let bytes = journal_bytes(&t);
    for c in ["repo-list", "status"] {
        let o = sim(&t, &kc, &[c]);
        let said = format!("{}{}", o.stdout, o.stderr);
        assert!(
            said.contains("written by a newer Harpoon")
                && said.contains("journal schema 3")
                && said.contains("reads up to schema 2")
                && said.contains("Nothing was changed, repaired or discarded"),
            "{c}: {said}"
        );
    }
    assert_eq!(
        journal_bytes(&t),
        bytes,
        "a refused journal is left byte-identical"
    );
}
