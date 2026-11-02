//! The Git runner against real `git`: local repositories for revision handling, a smart-HTTP server
//! with Basic authentication (`git http-backend`) for the credential path, and fake `git` scripts to
//! observe exactly what a process is given. No remote network, Docker or Keychain is involved.

mod support;

use harpoon_deploy::logs::Redactor;
use harpoon_deploy::secrets::SecretValue;
use harpoon_deploy_git::*;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use support::*;

fn url(p: &Path) -> String {
    format!("file://{}", p.display())
}

fn none() -> Redactor {
    Redactor::none()
}

fn ws(root: &Path, n: &str) -> Workspace {
    Workspace::create(&root.join("scratch"), n).unwrap()
}

#[test]
fn a_branch_a_tag_and_an_exact_commit_each_check_out_exactly_that_revision() {
    let t = Tmp::new();
    let (repo, first, second) = fixture(&t.0);
    let u = url(&repo);
    let s = Settings::default();
    let read = |c: &Checkout, f: &str| std::fs::read_to_string(c.src.join(f)).unwrap();
    let w = ws(&t.0, "a");
    let c = checkout(&w, &u, &Credential::None, &Selector::Branch("main".into()), &s, &none()).unwrap();
    assert_eq!(
        (c.commit.as_str(), read(&c, "VERSION").as_str()),
        (second.as_str(), "two\n")
    );
    assert!(!c.src.join(".git").exists(), "the tree handed to the build has no .git");
    assert!(c.src.join("Dockerfile").exists());
    let w2 = ws(&t.0, "b");
    // an annotated tag resolves to the commit it points at
    let c = checkout(&w2, &u, &Credential::None, &Selector::Tag("v1".into()), &s, &none()).unwrap();
    assert_eq!(
        (c.commit.as_str(), read(&c, "VERSION").as_str()),
        (first.as_str(), "one\n")
    );
    let w3 = ws(&t.0, "c");
    let c = checkout(
        &w3,
        &u,
        &Credential::None,
        &Selector::Branch("release/1".into()),
        &s,
        &none(),
    )
    .unwrap();
    assert_eq!(c.commit, first);
    // an exact commit that is no longer the tip of anything but main's history is still reachable
    let w4 = ws(&t.0, "d");
    let c = checkout(
        &w4,
        &u,
        &Credential::None,
        &Selector::Commit(first.clone()),
        &s,
        &none(),
    )
    .unwrap();
    assert_eq!(
        (c.commit.as_str(), read(&c, "VERSION").as_str()),
        (first.as_str(), "one\n")
    );
    // the branch moves on; the pinned commit does not
    std::fs::write(repo.join("VERSION"), "three\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "three"]);
    let w5 = ws(&t.0, "e");
    let c = checkout(
        &w5,
        &u,
        &Credential::None,
        &Selector::Commit(second.clone()),
        &s,
        &none(),
    )
    .unwrap();
    assert_eq!(
        (c.commit.as_str(), read(&c, "VERSION").as_str()),
        (second.as_str(), "two\n")
    );
}

#[test]
fn polling_sees_where_every_branch_points() {
    let t = Tmp::new();
    let (repo, first, second) = fixture(&t.0);
    let w = ws(&t.0, "p");
    let refs = ls_remote(&w, &url(&repo), &Credential::None, &Settings::default(), &none()).unwrap();
    let heads = branch_heads(&refs);
    assert!(
        heads.contains(&("main".to_string(), second.clone()))
            && heads.contains(&("release/1".to_string(), first.clone())),
        "{heads:?}"
    );
    assert!(
        refs.iter().any(|r| r.name == "refs/tags/v1^{}" && r.commit == first),
        "the annotated tag is peeled: {refs:?}"
    );
    // the remote moves
    std::fs::write(repo.join("VERSION"), "three\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "three"]);
    let third = git(&repo, &["rev-parse", "HEAD"]);
    let heads = branch_heads(&ls_remote(&w, &url(&repo), &Credential::None, &Settings::default(), &none()).unwrap());
    assert!(heads.contains(&("main".to_string(), third)));
}

#[test]
fn bad_requests_are_refused_before_git_runs_and_failed_checkouts_leave_nothing_behind() {
    let t = Tmp::new();
    let (repo, _, _) = fixture(&t.0);
    let u = url(&repo);
    let s = Settings::default();
    let scratch = t.0.join("scratch");
    let try_it = |url: &str, sel: Selector| {
        let w = Workspace::create(&scratch, "f").unwrap();
        let r = checkout(&w, url, &Credential::None, &sel, &s, &none());
        drop(w);
        r
    };
    for bad in [
        "-oProxyCommand=x",
        "ext::sh -c x",
        "git://example.com/r",
        "https://u:p@example.com/r",
        "relative/path",
    ] {
        assert!(
            matches!(try_it(bad, Selector::Branch("main".into())), Err(GitError::Invalid(_))),
            "{bad}"
        );
    }
    for sel in [
        Selector::Branch("-x".into()),
        Selector::Branch("a..b".into()),
        Selector::Tag("v1 x".into()),
        Selector::Commit("abc123".into()),
        Selector::Commit("A".repeat(40)),
    ] {
        assert!(matches!(try_it(&u, sel.clone()), Err(GitError::Invalid(_))), "{sel:?}");
    }
    // a repository that does not exist, a branch that does not exist, a commit nobody has
    assert!(matches!(
        try_it(&url(&t.0.join("nope")), Selector::Branch("main".into())),
        Err(GitError::NotFound(_) | GitError::Unavailable(_) | GitError::Failed(_))
    ));
    assert!(
        matches!(try_it(&u, Selector::Branch("nope".into())), Err(GitError::NotFound(_))),
        "no such branch"
    );
    let e = try_it(&u, Selector::Commit("1".repeat(40))).unwrap_err();
    assert!(matches!(e, GitError::NotFound(_) | GitError::Failed(_)), "{e:?}");
    // every failure removed its workspace; a hard kill's leftovers are found and swept
    assert_eq!(leftovers(&scratch), 0);
    // a live process's workspace (this one's) is never swept; a dead one's is
    let live = Workspace::create(&scratch, &format!("{}-live", std::process::id())).unwrap();
    std::fs::create_dir_all(scratch.join("git-ws-crashed/cred")).unwrap();
    std::fs::write(scratch.join("git-ws-crashed/cred/token"), "left").unwrap();
    assert_eq!(leftovers(&scratch), 1);
    assert_eq!(sweep(&scratch), 1);
    assert_eq!(leftovers(&scratch), 0);
    assert!(live.root().exists(), "a workspace of a live process survives the sweep");
}

#[test]
fn an_oversized_repository_is_stopped_and_leaves_nothing() {
    let t = Tmp::new();
    let repo = t.0.join("big");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    // incompressible data so the fetched pack is as large as the file
    let mut x: u64 = 0x9e3779b97f4a7c15;
    let blob: Vec<u8> = (0..4_000_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    std::fs::write(repo.join("blob.bin"), blob).unwrap();
    std::fs::write(repo.join("Dockerfile"), "FROM alpine:3.22\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "big"]);
    let s = Settings {
        max_bytes: 1_000_000,
        ..Settings::default()
    };
    let w = ws(&t.0, "big");
    let started = std::time::Instant::now();
    let e = checkout(
        &w,
        &url(&repo),
        &Credential::None,
        &Selector::Branch("main".into()),
        &s,
        &none(),
    )
    .unwrap_err();
    assert!(matches!(e, GitError::TooLarge { limit: 1_000_000 }), "{e:?}");
    assert!(started.elapsed() < Duration::from_secs(30));
    drop(w);
    assert_eq!(leftovers(&t.0.join("scratch")), 0);
    // within the limit the same repository is fine
    let w = ws(&t.0, "ok");
    let c = checkout(
        &w,
        &url(&repo),
        &Credential::None,
        &Selector::Branch("main".into()),
        &Settings::default(),
        &none(),
    )
    .unwrap();
    assert!(c.bytes >= 4_000_000);
}

// ---------------------------------------------------------------- fake git: what a process is given

fn fake_git(root: &Path, body: &str) -> PathBuf {
    let p = root.join("fakegit");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn a_token_reaches_git_only_through_a_private_file_and_is_gone_afterwards() {
    let t = Tmp::new();
    let token = SecretValue::new(b"ghp_planted-token-value-1234".to_vec()).unwrap();
    let rec = t.0.join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let g = fake_git(
        &t.0,
        &format!(
            "echo \"$@\" > {r}/args\nenv > {r}/env\n\"$GIT_ASKPASS\" \"Username for 'https://example.com': \" > {r}/user\n\"$GIT_ASKPASS\" \"Password for 'https://x@example.com': \" > {r}/pass\nls -l \"$HARPOON_GIT_TOKEN_FILE\" > {r}/mode\nls -ld \"$(dirname \"$HARPOON_GIT_TOKEN_FILE\")\" > {r}/dirmode\nexit 1",
            r = rec.display()
        ),
    );
    let s = Settings {
        git: g,
        ..Settings::default()
    };
    let w = ws(&t.0, "tok");
    let cred = Credential::Token {
        username: "x-access-token",
        token: &token,
    };
    let redactor = Redactor::new(&[("TOKEN", "ghp_planted-token-value-1234")]);
    let _ = ls_remote(&w, "https://example.com/acme/shop.git", &cred, &s, &redactor);
    let cred_dir = w.root().join("cred");
    assert!(cred_dir.exists(), "the credential files exist while the workspace does");
    let read = |f: &str| std::fs::read_to_string(rec.join(f)).unwrap();
    assert_eq!(read("user"), "x-access-token");
    assert_eq!(
        read("pass"),
        "ghp_planted-token-value-1234",
        "the askpass script hands Git the token"
    );
    for sink in [read("args"), read("env")] {
        assert!(
            !sink.contains("ghp_planted"),
            "the token is in neither argv nor the environment:\n{sink}"
        );
    }
    assert!(read("mode").starts_with("-rw-------"), "{}", read("mode"));
    assert!(read("dirmode").starts_with("drwx------"), "{}", read("dirmode"));
    let env = read("env");
    for must in [
        "GIT_TERMINAL_PROMPT=0",
        "GIT_CONFIG_NOSYSTEM=1",
        "GIT_CONFIG_GLOBAL=/dev/null",
        "GIT_ASKPASS=",
    ] {
        assert!(env.contains(must), "{must}");
    }
    assert!(!env.contains("SSH_AUTH_SOCK") && !env.contains("DOCKER_HOST"));
    let args = read("args");
    assert!(
        args.contains("core.hooksPath=/dev/null") && args.contains("credential.helper="),
        "{args}"
    );
    drop(w);
    assert_eq!(leftovers(&t.0.join("scratch")), 0, "removed on drop");
    // the wrong credential kind for the URL never reaches Git
    let w = ws(&t.0, "tok2");
    assert!(matches!(
        ls_remote(&w, "ssh://git@example.com/r.git", &cred, &s, &none()),
        Err(GitError::Invalid(_))
    ));
    let key = SecretValue::new(b"-----BEGIN KEY-----\nplanted-key-line\n-----END KEY-----".to_vec()).unwrap();
    let ssh = Credential::SshKey {
        key: &key,
        known_host: "example.com ssh-ed25519 AAAAC3Nza",
    };
    assert!(matches!(
        ls_remote(&w, "https://example.com/r.git", &ssh, &s, &none()),
        Err(GitError::Invalid(_))
    ));
    assert!(matches!(
        ls_remote(&w, "ssh://git@example.com/r.git", &Credential::None, &s, &none()),
        Err(GitError::Invalid(_))
    ));
}

#[test]
fn an_ssh_key_and_the_pinned_host_key_are_files_and_the_command_demands_a_match() {
    let t = Tmp::new();
    let rec = t.0.join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let g = fake_git(
        &t.0,
        &format!(
            "echo \"$@\" > {r}/args\nenv > {r}/env\ncat \"$(dirname \"$GIT_SSH_COMMAND\" 2>/dev/null)\" >/dev/null 2>&1\nd=$(echo \"$GIT_SSH_COMMAND\" | sed 's/.*UserKnownHostsFile=\\([^ ]*\\)\\/known_hosts.*/\\1/')\ncat \"$d/known_hosts\" > {r}/kh\nls -l \"$d/id\" > {r}/idmode\nexit 1",
            r = rec.display()
        ),
    );
    let s = Settings {
        git: g,
        ..Settings::default()
    };
    let key = SecretValue::new(b"-----BEGIN KEY-----\nplanted-key-line\n-----END KEY-----".to_vec()).unwrap();
    let cred = Credential::SshKey {
        key: &key,
        known_host: "example.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA",
    };
    let w = ws(&t.0, "ssh");
    let _ = ls_remote(
        &w,
        "git@example.com:acme/shop.git",
        &cred,
        &s,
        &Redactor::new(&[("KEY", "planted-key-line")]),
    );
    let read = |f: &str| std::fs::read_to_string(rec.join(f)).unwrap();
    let env = read("env");
    assert!(
        env.contains("StrictHostKeyChecking=yes")
            && env.contains("GlobalKnownHostsFile=/dev/null")
            && env.contains("IdentitiesOnly=yes"),
        "{env}"
    );
    assert!(!env.contains("planted-key") && !read("args").contains("planted-key"));
    assert_eq!(
        read("kh"),
        "example.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA\n",
        "only the pinned key is trusted"
    );
    assert!(read("idmode").starts_with("-rw-------"), "{}", read("idmode"));
    drop(w);
    assert_eq!(leftovers(&t.0.join("scratch")), 0);
}

#[test]
fn a_hung_git_is_killed_at_its_deadline_with_its_children() {
    let t = Tmp::new();
    let g = fake_git(
        &t.0,
        &format!(
            "echo $$ > {r}/pid\nsleep 30 &\necho $! > {r}/gpid\nwait",
            r = t.0.display()
        ),
    );
    let s = Settings {
        git: g,
        timeout: Duration::from_millis(500),
        ..Settings::default()
    };
    let w = ws(&t.0, "hang");
    let started = std::time::Instant::now();
    let e = ls_remote(&w, "https://example.com/r.git", &Credential::None, &s, &none()).unwrap_err();
    assert_eq!(e, GitError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(150));
    let alive = |p: &str| {
        Command::new("/bin/kill")
            .args(["-0", p.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(!alive(&std::fs::read_to_string(t.0.join("pid")).unwrap()));
    assert!(
        !alive(&std::fs::read_to_string(t.0.join("gpid")).unwrap()),
        "the process group, not just the leader"
    );
}

// ---------------------------------------------------------------- smart HTTP with Basic auth

#[test]
fn a_private_repository_needs_its_token_and_a_revoked_one_is_refused_without_leaving_anything() {
    let t = Tmp::new();
    let (repo, _, second) = fixture(&t.0);
    let served = t.0.join("served");
    std::fs::create_dir_all(&served).unwrap();
    let bare = served.join("shop.git");
    git(
        &t.0,
        &["clone", "-q", "--bare", repo.to_str().unwrap(), bare.to_str().unwrap()],
    );
    let server = HttpGit::start(served, "x-access-token", "good-token-1");
    let u = server.url("shop.git");
    let s = Settings::default();
    let scratch = t.0.join("scratch");

    // no credential: refused (git cannot prompt), and the failure says authentication
    let w = Workspace::create(&scratch, "n").unwrap();
    let e = checkout(&w, &u, &Credential::None, &Selector::Branch("main".into()), &s, &none()).unwrap_err();
    assert!(matches!(e, GitError::AuthFailed(_)), "{e:?}");
    drop(w);

    // the right token: the checkout works and is exactly the branch tip
    let good = SecretValue::new(b"good-token-1".to_vec()).unwrap();
    let cred = Credential::Token {
        username: "x-access-token",
        token: &good,
    };
    let w = Workspace::create(&scratch, "g").unwrap();
    let c = checkout(
        &w,
        &u,
        &cred,
        &Selector::Branch("main".into()),
        &s,
        &Redactor::new(&[("TOKEN", "good-token-1")]),
    )
    .unwrap();
    assert_eq!(c.commit, second);
    assert!(c.src.join("Dockerfile").exists());
    assert!(
        server.seen_auth.lock().unwrap().iter().any(|a| a.starts_with("Basic ")),
        "Basic credentials reached the server"
    );
    drop(w);

    // the token is revoked on the server: the same request now fails, and nothing is left on disk
    *server.token.lock().unwrap() = "rotated-token-2".into();
    let w = Workspace::create(&scratch, "r").unwrap();
    let e = checkout(
        &w,
        &u,
        &cred,
        &Selector::Branch("main".into()),
        &s,
        &Redactor::new(&[("TOKEN", "good-token-1")]),
    )
    .unwrap_err();
    assert!(matches!(e, GitError::AuthFailed(_)), "{e:?}");
    assert!(!e.to_string().contains("good-token-1"));
    drop(w);
    assert_eq!(leftovers(&scratch), 0);
    // polling uses the same path
    let w = Workspace::create(&scratch, "l").unwrap();
    assert!(matches!(
        ls_remote(&w, &u, &cred, &s, &none()),
        Err(GitError::AuthFailed(_))
    ));
    // and the new token works
    let new = SecretValue::new(b"rotated-token-2".to_vec()).unwrap();
    let refs = ls_remote(
        &w,
        &u,
        &Credential::Token {
            username: "x-access-token",
            token: &new,
        },
        &s,
        &none(),
    )
    .unwrap();
    assert!(branch_heads(&refs).iter().any(|(b, c)| b == "main" && *c == second));
}

#[test]
fn an_unreachable_remote_is_reported_as_unavailable_and_bounded() {
    let t = Tmp::new();
    // a port nobody listens on
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let w = ws(&t.0, "u");
    let started = std::time::Instant::now();
    let e = ls_remote(
        &w,
        &format!("http://127.0.0.1:{port}/r.git"),
        &Credential::None,
        &Settings::default(),
        &none(),
    )
    .unwrap_err();
    assert!(matches!(e, GitError::Unavailable(_)), "{e:?}");
    assert!(started.elapsed() < Duration::from_secs(20));
}

// ---------------------------------------------------------------- ordinary macOS paths (Milestone E.2)

/// `<tmp>/Library/Application Support/com.example app's/deploy/scratch`: spaces, a dot and an apostrophe,
/// like the desktop's data directory. The repository itself stays elsewhere.
fn spaced(t: &Tmp) -> PathBuf {
    t.0.join("Library/Application Support/com.example app's/deploy/scratch")
}

#[test]
fn a_workspace_under_a_path_with_spaces_polls_and_checks_out_with_real_git() {
    let t = Tmp::new();
    let (repo, first, second) = fixture(&t.0);
    let (u, s) = (url(&repo), Settings::default());
    let scratch = spaced(&t);
    let w = Workspace::create(&scratch, "poll").unwrap();
    assert!(w.root().to_str().unwrap().contains("Application Support"));
    // polling
    let refs = ls_remote(&w, &u, &Credential::None, &s, &none()).unwrap();
    assert!(
        branch_heads(&refs).iter().any(|(b, c)| b == "main" && *c == second),
        "{refs:?}"
    );
    // checkout of a branch and of an exact commit
    let c = checkout(&w, &u, &Credential::None, &Selector::Branch("main".into()), &s, &none()).unwrap();
    assert_eq!(c.commit, second);
    assert_eq!(std::fs::read_to_string(c.src.join("VERSION")).unwrap(), "two\n");
    assert!(c.src.starts_with(&scratch) && !c.src.join(".git").exists());
    let w2 = Workspace::create(&scratch, "exact").unwrap();
    let c = checkout(
        &w2,
        &u,
        &Credential::None,
        &Selector::Commit(first.clone()),
        &s,
        &none(),
    )
    .unwrap();
    assert_eq!(c.commit, first);
    drop((w, w2));
    assert_eq!(leftovers(&scratch), 0, "the workspaces were removed");
}

#[test]
fn the_ssh_command_survives_the_shell_and_sshs_own_parser_with_spaces_and_an_apostrophe() {
    let t = Tmp::new();
    let w = Workspace::create(&spaced(&t), "ssh").unwrap();
    let cred = w.root().join("cred");
    let cmd = ssh_command(&cred);
    // ssh -G leaves out a -i file that does not exist
    std::fs::write(cred.join("id"), b"k").unwrap();
    // Git hands GIT_SSH_COMMAND to `sh -c`; here `ssh` is a stand-in that asks the real ssh to print the
    // configuration it would use (-G never connects).
    let bin = t.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("ssh");
    std::fs::write(&fake, "#!/bin/sh\nexec /usr/bin/ssh -G \"$@\" dummyhost\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", &t.0)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    let line = |k: &str| text.lines().find(|l| l.starts_with(k)).unwrap_or("").to_string();
    assert_eq!(
        line("userknownhostsfile "),
        format!("userknownhostsfile {}/known_hosts", cred.display()),
        "{text}"
    );
    assert!(
        text.lines().any(|l| l == format!("identityfile {}/id", cred.display())),
        "{text}"
    );
    for fixed in [
        "stricthostkeychecking true",
        "globalknownhostsfile /dev/null",
        "identitiesonly yes",
        "batchmode yes",
    ] {
        assert!(text.lines().any(|l| l == fixed), "{fixed} in {text}");
    }
    // an ordinary path still produces the unquoted command it always did
    assert!(ssh_command(Path::new("/w/cred")).contains("-i /w/cred/id -o"));
}

#[test]
fn credential_files_in_a_spaced_workspace_reach_an_askpass_run_the_way_git_runs_it() {
    let t = Tmp::new();
    let rec = t.0.join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    // Git executes GIT_ASKPASS directly (no shell); the fake git does the same and records the answer.
    let g = fake_git(
        &t.0,
        &format!(
            "\"$GIT_ASKPASS\" 'Username for x' > {r}/user\n\"$GIT_ASKPASS\" 'Password for x' > {r}/pass\nexit 1",
            r = rec.display()
        ),
    );
    let s = Settings {
        git: g,
        ..Settings::default()
    };
    let token = SecretValue::new(b"planted-token-value".to_vec()).unwrap();
    let w = Workspace::create(&spaced(&t), "tok").unwrap();
    let _ = ls_remote(
        &w,
        "https://example.com/acme/shop.git",
        &Credential::Token {
            username: "bot",
            token: &token,
        },
        &s,
        &Redactor::new(&[("TOKEN", "planted-token-value")]),
    );
    assert_eq!(std::fs::read_to_string(rec.join("user")).unwrap(), "bot");
    assert_eq!(
        std::fs::read_to_string(rec.join("pass")).unwrap(),
        "planted-token-value"
    );
}

#[test]
fn unsafe_workspace_names_and_paths_are_still_refused_and_create_nothing() {
    let t = Tmp::new();
    let base = t.0.join("scratch");
    // the name is one path component of plain characters
    for nonce in ["", "..", "../x", "a/b", "a b", "a;b", "a\nb", "$(x)"] {
        assert!(
            matches!(Workspace::create(&base, nonce), Err(GitError::Invalid(_))),
            "nonce {nonce:?}"
        );
    }
    // characters no consumer can treat safely are refused even though spaces are not
    for bad in ["we\"ird", "we\\ird", "we$ird", "we%ird", "we\nird", "we\x07ird"] {
        let parent = t.0.join(bad);
        assert!(
            matches!(Workspace::create(&parent, "n"), Err(GitError::Invalid(_))),
            "{bad:?}"
        );
        assert!(!parent.exists(), "refused before anything was created: {bad:?}");
    }
    assert!(!base.exists());
    // a refused request never leaves a workspace behind
    assert_eq!(leftovers(&base), 0);
}
