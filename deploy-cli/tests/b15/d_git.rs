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

#[test]
fn a_credentialed_registration_keeps_its_host_so_its_secret_is_never_sent_elsewhere() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    assert_exit(
        &add_repo(
            &t,
            &kc,
            "shop",
            "https://git.example.com/acme/shop.git",
            APP,
            &["--username", "ci"],
        ),
        0,
    );
    let at = registered_ms(&t, "shop");
    for other in [
        "https://git.evil.example/acme/shop.git",
        "https://git.example.com.evil.example/acme/shop.git",
        "http://127.0.0.1:1/acme/shop.git",
        "ssh://git@git.example.com/acme/shop.git",
    ] {
        let o = update(&t, &kc, "shop", at, &["--url", other]);
        assert_exit(&o, 6);
        assert!(!o.stdout.contains("updated"), "{other}: {}", o.stdout);
    }
    assert_eq!(
        restored(&t).git.repos["shop"].url,
        "https://git.example.com/acme/shop.git"
    );
    // the same host, another path, is fine, and the credential settings are untouched
    let o = update(
        &t,
        &kc,
        "shop",
        at,
        &["--url", "https://git.example.com/acme/shop2.git"],
    );
    assert_exit(&o, 0);
    assert!(o.stdout.contains("not verified yet"), "{}", o.stdout);
    let r = restored(&t).git.repos["shop"].clone();
    assert_eq!(
        (r.url.as_str(), r.cred, r.username.as_deref()),
        (
            "https://git.example.com/acme/shop2.git",
            harpoon_deploy::git::CredKind::Token,
            Some("ci")
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
