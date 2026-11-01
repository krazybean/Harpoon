//! Milestone C through the real CLI against the fake Docker and a fake Keychain: scoped configuration,
//! versioned secrets, BuildKit secret mounts, runtime injection, rotation, snapshots, rollback and
//! restore to the original release's references, the failure modes, and the leakage matrix.
//!
//! Offline. These prove Harpoon's own behaviour; they say nothing about the native Keychain (the
//! `deploy-keychain` crate has its own opt-in test against disposable items) or a real Buildx (image
//! layers and `docker history` cannot be observed here).

use super::*;
use harpoon_deploy::secrets::{FakeMode, MemorySecretStore, SecretRef, SecretStore, SecretValue};
use harpoon_deploy_cli::{run_with, Services};

/// The CLI in process with an explicit secret store and standard input, gated like `T::go`/`run`.
fn crun(t: &T, gate: &Gate, kc: &MemorySecretStore, stdin: &[u8], args: &[String]) -> Outcome {
    let mut a: Vec<String> = args.to_vec();
    if !a.iter().any(|x| x == "--runtime-dir") {
        a.push("--runtime-dir".into());
        a.push(t.rt.display().to_string());
    }
    let spawns = a.iter().any(|x| x == "--lsof-path");
    let mut input: &[u8] = stdin;
    let mut svc = Services {
        secrets: kc,
        stdin: &mut input,
        stdin_is_terminal: false,
    };
    if spawns {
        spawning(|| run_with(&a, gate, &mut svc))
    } else {
        let _h = holding();
        run_with(&a, gate, &mut svc)
    }
}

fn s(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| x.to_string()).collect()
}

fn cfg(t: &T, kc: &MemorySecretStore, a: &[&str]) -> Outcome {
    crun(t, &SHIPPING, kc, b"", &s(a))
}

fn secret(t: &T, kc: &MemorySecretStore, a: &[&str], value: &str) -> Outcome {
    crun(t, &SHIPPING, kc, value.as_bytes(), &s(a))
}

/// A live command: its own arguments plus every flag the acceptance gate demands.
fn live_c(t: &T, kc: &MemorySecretStore, lsof: &str, a: &[&str]) -> Outcome {
    let mut v = s(a);
    v.extend(live_flags(t));
    v.extend(s(&["--forwarder-command", "harpoon", "--lsof-path", lsof]));
    crun(t, &ACCEPTANCE, kc, b"", &v)
}

const FAKE_BUILD: &str = r#"
for a in "$@"; do
  case "$a" in
    id=*,src=*)
      src="${a#*src=}"
      echo "$src" >> "$D/secretpaths.txt"
      echo "BUILD STEP SAW: $(cat "$src")"
      find "$src" -perm 600 | wc -l >> "$D/filemodes.txt"
      ;;
  esac
done
echo built
"#;

fn project_args(t: &T) -> Vec<String> {
    let mut v = s(&[
        "--app",
        APP,
        "--project",
        "shop",
        "--environment",
        "prod",
        "--target",
        "release",
        "--health-timeout-secs",
        "2",
    ]);
    v.extend(t.src());
    v
}

fn running_app_ports(t: &T) -> Vec<u16> {
    containers(t)
        .into_iter()
        .filter(|c| c.1)
        .filter_map(|c| c.2)
        .filter(|p| (38000..39000).contains(p))
        .collect()
}

/// Every file under the runtime directory that contains any of `needles` (SQLite, WAL, scratch...).
fn files_with(t: &T, needles: &[&str]) -> Vec<String> {
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

fn state(t: &T) -> harpoon_deploy_journal::Restored {
    harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir()).unwrap()
}

fn docker_env(t: &T, name_contains: &str) -> Vec<String> {
    let all = containers(t);
    let c = all
        .iter()
        .find(|c| c.0.contains(name_contains))
        .unwrap_or_else(|| panic!("no container {name_contains}"));
    without_marker(&c.4)
}

#[test]
fn the_developer_workflow_from_scoped_config_to_rotation_rollback_and_a_blocked_restore() {
    let _ports = ports();
    let t = T::new();
    let kc = MemorySecretStore::new();
    let lsof = forwarder_lsof(&t);
    t.h.ws.install_buildx(FAKE_BUILD);
    let (npm1, pw1, pw2) = ("npm-token-ONE", "pw-one-ABCDEF", "pw-two-GHIJKL");

    // 1. scoped, non-secret values at three scopes, one build-time
    let scope_app = format!("app:{APP}@prod");
    for a in [
        vec![
            "config-set",
            "--scope",
            "global",
            "--name",
            "REGION",
            "--value",
            "eu",
        ],
        vec![
            "config-set",
            "--scope",
            "project:shop@prod",
            "--name",
            "LOG_LEVEL",
            "--value",
            "debug",
        ],
        vec![
            "config-set",
            "--scope",
            "project:shop",
            "--name",
            "NODE_ENV",
            "--value",
            "production",
            "--build",
        ],
        vec![
            "config-set",
            "--scope",
            "global",
            "--name",
            "LOG_LEVEL",
            "--value",
            "info",
        ],
    ] {
        assert_exit(&cfg(&t, &kc, &a), 0);
    }
    // 2. versioned secrets: one runtime (application+environment), one build-time (global)
    let o = secret(
        &t,
        &kc,
        &["secret-set", "--scope", &scope_app, "--name", "DB_PASSWORD"],
        &format!("{pw1}\n"),
    );
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("DB_PASSWORD@1") || o.stdout.contains("/DB_PASSWORD@1"),
        "{}",
        o.stdout
    );
    let o = secret(
        &t,
        &kc,
        &[
            "secret-set",
            "--scope",
            "global",
            "--name",
            "NPM_TOKEN",
            "--build",
        ],
        npm1,
    );
    assert_exit(&o, 0);
    for out in [&o.stdout, &o.stderr] {
        assert!(!out.contains(npm1) && !out.contains(pw1));
    }
    assert_eq!(
        kc.accounts().len(),
        2,
        "one Keychain item per saved version"
    );

    // 3. effective configuration with provenance; nothing secret is shown
    let show = |extra: &[&str]| {
        let mut a = s(&[
            "config-show",
            "--app",
            APP,
            "--project",
            "shop",
            "--environment",
            "prod",
        ]);
        a.extend(s(extra));
        cfg(&t, &kc, &a.iter().map(String::as_str).collect::<Vec<_>>())
    };
    let o = show(&["--check-secrets"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains("REGION [runtime] config = \"eu\" (from global)")
            && o.stdout
                .contains("LOG_LEVEL [runtime] config = \"debug\" (from project:shop@prod)")
            && o.stdout
                .contains("NODE_ENV [build] config = \"production\" (from project:shop)")
            && o.stdout.contains(&format!(
                "DB_PASSWORD [runtime] secret {scope_app}/DB_PASSWORD@1"
            ))
            && o.stdout
                .contains("NPM_TOKEN [build] secret global/NPM_TOKEN@1")
            && o.stdout.contains("available"),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains(pw1) && !o.stdout.contains(npm1));

    // 4. build with a non-secret argument and a BuildKit secret mount
    let image = |t: &T| image_appears_after_build(t);
    let held = occupy_all_but_first();
    let _pre = responder(&t, TEST_PORT, true);
    let _rel1 = new_release_responder(&t, vec![]);
    image(&t);
    let mut a = vec!["deploy"];
    let pa = project_args(&t);
    a.extend(pa.iter().map(String::as_str));
    a.extend(["--build-arg", "EXTRA=1"]);
    let o = live_c(&t, &kc, &lsof, &a);
    drop(held);
    drop((_pre, _rel1));
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_exit(&o, 0);
    for out in [&o.stdout, &o.stderr] {
        assert!(!out.contains(npm1) && !out.contains(pw1), "{out}");
    }
    let args = t.h.ws.buildx_record("args.txt");
    for want in [
        "--target release",
        "--build-arg NODE_ENV=production",
        "--build-arg EXTRA=1",
        "--secret id=NPM_TOKEN,src=",
    ] {
        assert!(args.contains(want), "{want} in {args}");
    }
    assert!(!args.contains(npm1) && !args.contains(pw1) && !args.contains("secret-value"));
    assert!(!t.h.ws.buildx_record("env.txt").contains(npm1));
    assert_eq!(
        t.h.ws.buildx_record("filemodes.txt").trim(),
        "1",
        "the secret file was 0600"
    );
    assert!(
        files_with(&t, &[npm1, pw1]).is_empty(),
        "no Harpoon file holds a secret: {:?}",
        files_with(&t, &[npm1, pw1])
    );

    // 5. the container got the resolved runtime configuration: scoped values and the runtime secret,
    //    and not the build-time ones
    let env1 = docker_env(&t, "hp-");
    assert!(
        env1.contains(&"REGION=eu".to_string()) && env1.contains(&"LOG_LEVEL=debug".to_string()),
        "{env1:?}"
    );
    assert!(
        env1.contains(&format!("DB_PASSWORD={pw1}")),
        "documented exposure: docker inspect shows it"
    );
    assert!(
        !env1.iter().any(|e| e.starts_with("NODE_ENV")
            || e.starts_with("NPM_TOKEN")
            || e.starts_with("EXTRA")),
        "{env1:?}"
    );

    let r = state(&t);
    let dep1 = r.deployments[0].clone();
    let rel1 = format!("rel_{}", &dep1.id.to_string()[4..]);
    let pin1 = dep1.pinned.clone().expect("pinned");
    assert_eq!(
        pin1.secrets,
        vec![(
            "DB_PASSWORD".to_string(),
            SecretRef::new(format!("{scope_app}/DB_PASSWORD"), 1)
        )]
    );
    assert_eq!(pin1.snapshot.as_deref(), Some(dep1.id.to_string().as_str()));
    assert!(
        pin1.build.as_deref().unwrap().contains("target=release")
            && pin1.build.as_deref().unwrap().contains("NPM_TOKEN:")
    );
    assert!(pin1.encode().starts_with("v3;") && !pin1.encode().contains(pw1));
    let snap1 = r
        .config
        .snapshot_of(&dep1.id)
        .expect("an immutable snapshot");
    assert!(snap1.entries.iter().any(|e| e.name == "DB_PASSWORD"
        && matches!(&e.value, harpoon_deploy::config::Value::Secret(r) if r.version == 1)));

    // 6. rotate the secret: version 2, an additive change that touches nothing already deployed
    let o = secret(
        &t,
        &kc,
        &["secret-set", "--scope", &scope_app, "--name", "DB_PASSWORD"],
        pw2,
    );
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("DB_PASSWORD@2") && o.stdout.contains("a rotation"),
        "{}",
        o.stdout
    );
    assert_eq!(
        kc.accounts().len(),
        3,
        "version 1 is kept for the release that uses it"
    );

    // 7. replace: a new snapshot with the new version; the original release keeps its reference
    let prior = running_app_ports(&t);
    let held = occupy_all_but_first();
    let _pre2 = responder(&t, TEST_PORT, true);
    let _rel2 = new_release_responder(&t, prior);
    image(&t);
    let mut a = vec!["replace", "--confirm-cutover"];
    a.extend(pa.iter().map(String::as_str));
    let o = live_c(&t, &kc, &lsof, &a);
    drop(held);
    drop((_pre2, _rel2));
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_exit(&o, 0);
    assert!(!o.stdout.contains(pw1) && !o.stdout.contains(pw2) && !o.stderr.contains(pw2));
    let r = state(&t);
    let dep2 = r
        .deployments
        .iter()
        .find(|d| d.id != dep1.id)
        .unwrap()
        .clone();
    assert!(dep2.pinned.as_ref().unwrap().secrets[0].1.version == 2);
    assert!(r
        .config
        .snapshot_of(&dep2.id)
        .unwrap()
        .entries
        .iter()
        .any(|e| e.name == "DB_PASSWORD"
            && matches!(&e.value, harpoon_deploy::config::Value::Secret(r) if r.version == 2)));
    // 8. the original release's snapshot and pin are unchanged: it never inherits the newer version
    let old = r.deployments.iter().find(|d| d.id == dep1.id).unwrap();
    assert_eq!(old.pinned.as_ref().unwrap().secrets[0].1.version, 1);
    let o = cfg(
        &t,
        &kc,
        &["config-show", "--deployment", &dep1.id.to_string()],
    );
    assert!(
        o.stdout.contains("DB_PASSWORD [runtime] secret") && o.stdout.contains("/DB_PASSWORD@1"),
        "{}",
        o.stdout
    );
    // 11. a diff of names, scopes, flags and versions; never a value
    let o = cfg(
        &t,
        &kc,
        &[
            "config-diff",
            "--from",
            &dep1.id.to_string(),
            "--to",
            &dep2.id.to_string(),
        ],
    );
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("~ DB_PASSWORD: secret version 1 -> 2"),
        "{}",
        o.stdout
    );
    for v in [pw1, pw2, npm1, "production", "debug"] {
        assert!(!o.stdout.contains(v), "{v} in {}", o.stdout);
    }
    let o = cfg(&t, &kc, &["config-diff", "--from", &dep2.id.to_string()]);
    assert!(
        o.stdout.contains("no differences"),
        "the configuration that would apply now equals the snapshot:\n{}",
        o.stdout
    );

    // 10. (first) a missing ORIGINAL secret version blocks the rollback before anything changes
    let v1 = SecretRef::new(format!("{scope_app}/DB_PASSWORD"), 1);
    kc.delete(&v1).unwrap();
    let (jb, n) = (t.journal_bytes(), t.docker_requests());
    let o = live_c(
        &t,
        &kc,
        &lsof,
        &[
            "rollback",
            "--app",
            APP,
            "--to",
            &rel1,
            "--confirm-rollback",
        ],
    );
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("DB_PASSWORD")
            && o.stderr.contains("@1")
            && o.stderr
                .contains("Nothing was built, started, stopped or changed"),
        "{}",
        o.stderr
    );
    assert_eq!(
        (t.journal_bytes(), t.docker_requests()),
        (jb, n),
        "refused before the journal or Docker was touched"
    );
    kc.put(&v1, &SecretValue::new(pw1.as_bytes().to_vec()).unwrap())
        .unwrap();

    // 9. roll back to the original release: it is recreated with ITS pinned version (1), not the current one
    let prior = running_app_ports(&t);
    let held = occupy_all_but_first();
    let _pre3 = responder(&t, TEST_PORT, true);
    let _rel1b = new_release_responder(&t, prior);
    let o = live_c(
        &t,
        &kc,
        &lsof,
        &[
            "rollback",
            "--app",
            APP,
            "--to",
            &rel1,
            "--confirm-rollback",
        ],
    );
    drop(held);
    drop((_pre3, _rel1b));
    assert_exit(&o, 0);
    assert!(!o.stdout.contains(pw1) && !o.stdout.contains(pw2));
    let rolled: Vec<_> = containers(&t)
        .into_iter()
        .filter(|c| c.1 && c.3 == pin1.image_id)
        .collect();
    assert_eq!(rolled.len(), 1, "{rolled:?}");
    let env_rb = without_marker(&rolled[0].4);
    assert!(
        env_rb.contains(&format!("DB_PASSWORD={pw1}")),
        "the original version: {env_rb:?}"
    );
    assert!(
        !env_rb.iter().any(|e| e.contains(pw2)),
        "never the newer value: {env_rb:?}"
    );
    let rb = state(&t);
    let dep3 = rb
        .deployments
        .iter()
        .find(|d| d.id != dep1.id && d.id != dep2.id)
        .unwrap();
    assert_eq!(
        dep3.pinned.as_ref().unwrap().snapshot,
        pin1.snapshot,
        "a rollback keeps its target's snapshot"
    );
    assert!(
        control_of(&t)
            .pauses
            .iter()
            .any(|p| p.kind == harpoon_deploy::control::PauseKind::Rollback),
        "B14 pause preserved"
    );

    // the serving (rolled-back) release needs version 1 for its own bounded restoration: without it a
    // replace is refused too, before anything is stopped, built or written
    kc.delete(&v1).unwrap();
    let (jb, n) = (t.journal_bytes(), t.docker_requests());
    let mut a = vec!["replace", "--confirm-cutover"];
    a.extend(pa.iter().map(String::as_str));
    let o = live_c(&t, &kc, &lsof, &a);
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("DB_PASSWORD") && o.stderr.contains("@1"),
        "{}",
        o.stderr
    );
    assert_eq!((t.journal_bytes(), t.docker_requests()), (jb, n));

    // nothing anywhere holds a planted value except the documented container environment
    assert!(
        files_with(&t, &[npm1, pw1, pw2]).is_empty(),
        "{:?}",
        files_with(&t, &[npm1, pw1, pw2])
    );
    drop(open_j(&t.journal_dir()).expect("the journal replays"));
}

#[test]
fn secret_input_is_stdin_only_and_no_diagnostic_echoes_a_value() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let a = s(&[
        "secret-set",
        "--scope",
        "global",
        "--name",
        "TOKEN",
        "--runtime-dir",
        &t.rt.display().to_string(),
    ]);
    // an interactive terminal is refused (it would echo)
    let mut input: &[u8] = b"hunter2-planted\n";
    let mut svc = Services {
        secrets: &kc,
        stdin: &mut input,
        stdin_is_terminal: true,
    };
    let o = run_with(&a, &SHIPPING, &mut svc);
    assert_exit(&o, 2);
    assert!(!o.stderr.contains("hunter2") && !t.rt.exists() && kc.accounts().is_empty());
    // there is no flag that takes the value
    let mut b = a.clone();
    b.extend(s(&["--value", "hunter2-planted"]));
    let o = crun(&t, &SHIPPING, &kc, b"", &b);
    assert_exit(&o, 2);
    assert!(
        o.stderr.contains("does not take --value") && !o.stderr.contains("hunter2-planted")
            || o.stderr.contains("does not take --value")
    );
    // empty, NUL and oversized values are refused without echoing them
    for bad in [&b""[..], &b"\n"[..], &b"a\0b"[..], &vec![b'x'; 20_000][..]] {
        let o = crun(&t, &SHIPPING, &kc, bad, &a);
        assert_exit(&o, 2);
        assert!(
            !o.stderr.contains("xxxx") && kc.accounts().is_empty(),
            "{}",
            o.stderr
        );
    }
    // one trailing line break is dropped, everything else is kept (CRLF, internal newlines)
    for (input, want) in [
        (&b"abc\n"[..], "abc"),
        (&b"abc\r\n"[..], "abc"),
        (&b"a\nb\n"[..], "a\nb"),
        (&b"no-newline"[..], "no-newline"),
    ] {
        let kc = MemorySecretStore::new();
        assert_exit(&crun(&t, &SHIPPING, &kc, input, &a), 0);
        let got = kc.get(&SecretRef::new("global/TOKEN", 1)).unwrap();
        assert_eq!(got.expose_str(), want);
        std::fs::remove_dir_all(&t.rt).unwrap();
    }
    // a one-character value is accepted, with a warning
    let kc = MemorySecretStore::new();
    let o = crun(&t, &SHIPPING, &kc, b"x", &a);
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains("warning: a value this short will mask many unrelated characters"),
        "{}",
        o.stdout
    );
}

#[test]
fn a_locked_or_denied_keychain_stops_a_deploy_before_any_side_effect_and_read_only_commands_still_work(
) {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let lsof = forwarder_lsof(&t);
    assert_exit(
        &secret(
            &t,
            &kc,
            &["secret-set", "--scope", "global", "--name", "TOKEN"],
            "tok-planted-1",
        ),
        0,
    );
    let (jb, n) = (t.journal_bytes(), t.docker_requests());
    for mode in [FakeMode::Locked, FakeMode::Denied, FakeMode::Unavailable] {
        kc.set_mode(mode);
        let mut a = vec!["deploy"];
        let pa = project_args(&t);
        a.extend(pa.iter().map(String::as_str));
        let o = live_c(&t, &kc, &lsof, &a);
        assert_exit(&o, 6);
        assert!(
            o.stderr.contains("TOKEN")
                && o.stderr.contains("global/TOKEN@1")
                && o.stderr
                    .contains("Nothing was built, started, stopped or changed"),
            "{mode:?}: {}",
            o.stderr
        );
        assert!(!o.stderr.contains("tok-planted"));
        assert_eq!(
            (t.journal_bytes(), t.docker_requests()),
            (jb.clone(), n),
            "{mode:?}: nothing changed"
        );
        assert!(
            t.h.ws.buildx_record("args.txt").is_empty(),
            "{mode:?}: Buildx never started"
        );
        // references are enough for the read-only commands
        for cmd in [
            vec!["status"],
            vec!["config-defs"],
            vec!["config-show", "--app", APP],
            vec!["events"],
        ] {
            let o = cfg(&t, &kc, &cmd);
            assert_exit(&o, 0);
        }
        // an explicit check names what is unavailable, and says so with its exit status
        let o = cfg(&t, &kc, &["config-show", "--app", APP, "--check-secrets"]);
        assert_exit(&o, 6);
        assert!(
            o.stdout.contains("TOKEN")
                && o.stdout.contains("UNAVAILABLE")
                && !o.stdout.contains("tok-planted"),
            "{}",
            o.stdout
        );
    }
    // the same key set back to working: the deploy proceeds past the secret stage (and stops at the
    // preliminary check, because nothing answers in this test)
    kc.set_mode(FakeMode::Working);
    let o = cfg(&t, &kc, &["config-show", "--app", APP, "--check-secrets"]);
    assert_exit(&o, 0);
}

#[test]
fn an_unavailable_pinned_secret_refuses_an_explicit_restore_and_a_dry_run_says_so() {
    let _ports = ports();
    let t = T::new();
    let kc = MemorySecretStore::new();
    let lsof = forwarder_lsof(&t);
    let r1 = SecretRef::new("global/DB", 1);
    kc.put(
        &r1,
        &SecretValue::new(b"pinned-planted-1".to_vec()).unwrap(),
    )
    .unwrap();
    // an interrupted cutover whose previous release was pinned with a secret
    let (_, _) = seed_with_secret(&t, &r1);
    dead_cutover(&t);
    assert_exit(&t.cli(&["recover"]), 0);

    let (jb, n) = (t.journal_bytes(), t.docker_requests());
    kc.delete(&r1).unwrap();
    // dry run: informative, changes nothing
    let o = live_c(&t, &kc, &lsof, &["restore", "--deployment", DEAD]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("secret DB (global/DB@1): UNAVAILABLE")
            && o.stdout.contains("--apply would be refused"),
        "{}",
        o.stdout
    );
    // apply: refused before the journal is opened for writing or Docker is contacted
    let o = live_c(
        &t,
        &kc,
        &lsof,
        &["restore", "--deployment", DEAD, "--apply"],
    );
    assert_exit(&o, 6);
    assert!(
        o.stderr.contains("DB") && o.stderr.contains("global/DB@1"),
        "{}",
        o.stderr
    );
    assert_eq!((t.journal_bytes(), t.docker_requests()), (jb, n));
    // the one attempt is not spent: with the version back, the restoration runs and uses the pinned value
    kc.put(
        &r1,
        &SecretValue::new(b"pinned-planted-1".to_vec()).unwrap(),
    )
    .unwrap();
    let _app = responder(&t, PORT, true);
    let o = live_c(
        &t,
        &kc,
        &lsof,
        &["restore", "--deployment", DEAD, "--apply"],
    );
    assert_exit(&o, 0);
    assert!(!o.stdout.contains("pinned-planted-1"));
    let restored = container(&t, &format!("hp-{DEAD}-restored"));
    assert!(
        without_marker(&restored.4).contains(&"DB=pinned-planted-1".to_string()),
        "{:?}",
        restored.4
    );
}

/// `seed_release`, but the pin also references a runtime secret.
fn seed_with_secret(t: &T, r: &SecretRef) -> (String, String) {
    t.docker_runtime();
    let (app, dep): (ApplicationId, DeploymentId) = (APP.parse().unwrap(), PREV.parse().unwrap());
    let image = support::fake_iid(9001);
    let name = format!("hp-{dep}");
    let lbl = harpoon_deploy::backend::harpoon_labels(&app, &dep);
    let cid = t.h.docker.with(|s| {
        s.add_image_with_id(&image, &[&format!("harpoon/{dep}:1")], lbl.clone(), vec![]);
        let c = s.add_container(&name, lbl.clone(), false);
        s.containers.last_mut().unwrap().host_port = Some(PORT);
        c
    });
    let mut pin = ReleaseConfig::simulated(PORT);
    pin.image_id = image.clone();
    pin.image_tag = format!("harpoon/{dep}:1");
    pin.health.interval_ms = 5;
    pin.health.startup_deadline_ms = 300;
    pin.secrets = vec![("DB".to_string(), r.clone())];
    pin.snapshot = Some(dep.to_string());
    let mut d = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, 100);
    d.transition(DeploymentStatus::Queued, 101).unwrap();
    d.transition(DeploymentStatus::Running, 102).unwrap();
    d.release = Some(ReleaseId::from_deployment(&dep));
    d.pinned = Some(pin);
    d.transition(DeploymentStatus::Succeeded, 103).unwrap();
    let (tag, lease) = (format!("harpoon/{}:1", d.id), PORT.to_string());
    let (mut j, _) = open_j(&t.journal_dir()).unwrap();
    // the records a real deployment writes: the secret version, then the snapshot, then the release
    j.config(&harpoon_deploy::config::ConfigRecord::SecretSaved(
        harpoon_deploy::config::SecretSaved {
            id: r.id.clone(),
            version: r.version,
            at_ms: 1,
        },
    ))
    .unwrap();
    j.config(&harpoon_deploy::config::ConfigRecord::Snapshot(Box::new(
        harpoon_deploy::config::Snapshot {
            deployment: dep.clone(),
            app: app.clone(),
            project: None,
            environment: None,
            at_ms: 2,
            entries: vec![],
        },
    )))
    .unwrap();
    j.persist(&d).unwrap();
    for (kind, n, id, disp) in [
        (
            ResourceKind::Image,
            tag.as_str(),
            image.as_str(),
            Disposition::Retain,
        ),
        (
            ResourceKind::Container,
            name.as_str(),
            cid.as_str(),
            Disposition::RemoveOnFailure,
        ),
        (
            ResourceKind::PortLease,
            lease.as_str(),
            lease.as_str(),
            Disposition::RemoveOnFailure,
        ),
    ] {
        j.record(&intent(&app, &dep, kind, n, disp)).unwrap();
        j.record(&receipt(&dep, kind, n, id)).unwrap();
    }
    (image, cid)
}

#[test]
fn planted_secrets_of_every_shape_never_reach_a_harpoon_sink_through_a_failing_build() {
    let t = T::new();
    let kc = MemorySecretStore::new();
    let lsof = forwarder_lsof(&t);
    let v1 = "§";
    let v3 = "Q#7";
    let v8 = "Zk4_mW9p";
    let v40 = "0123456789abcdefghijklmnopqrstuvwxyzABCD";
    let vm = "-----BEGIN KEY-----\nAAAA-line-one\nBBBB-line-two\n-----END KEY-----";
    let vlong = "OVERLAP-secret-long-form";
    let vinner = "secret-long";
    let planted = [
        ("S1", v1),
        ("S3", v3),
        ("S8", v8),
        ("S40", v40),
        ("SPEM", vm),
        ("SLONG", vlong),
        ("SINNER", vinner),
    ];
    for (name, v) in planted {
        assert_exit(
            &secret(
                &t,
                &kc,
                &["secret-set", "--scope", "global", "--name", name, "--build"],
                v,
            ),
            0,
        );
    }
    t.h.ws.install_buildx(
        r#"
for a in "$@"; do
  case "$a" in id=*,src=*) src="${a#*src=}"; echo "dump: $(cat "$src")"; printf 'chunked: '; head -c 20 "$src"; sleep 0.2; tail -c +21 "$src"; echo;; esac
done
echo "base64 of a secret is NOT detected: $(printf %s 'Zk4_mW9p' | base64)"
exit 5
"#,
    );
    let mut a = vec!["deploy"];
    let pa = project_args(&t);
    a.extend(pa.iter().map(String::as_str));
    let o = live_c(&t, &kc, &lsof, &a);
    assert_exit(&o, 7);
    // build failure evidence is printed: masked, never raw
    for out in [&o.stdout, &o.stderr] {
        for v in [
            v3,
            v8,
            v40,
            "AAAA-line-one",
            "BBBB-line-two",
            "BEGIN KEY",
            vlong,
            vinner,
        ] {
            assert!(!out.contains(v), "{v:?} leaked:\n{out}");
        }
    }
    assert!(o.stdout.contains("[REDACTED:"), "{}", o.stdout);
    // the documented limit: a transformed secret is not detected
    assert!(
        o.stdout.contains("Wms0X21XOXA="),
        "base64 of the 8-character value passes through unmasked (documented): {}",
        o.stdout
    );
    // sinks: SQLite and WAL byte scans, scratch files, process argv and environment
    let leaks = files_with(
        &t,
        &[v3, v8, v40, "AAAA-line-one", "BEGIN KEY", vlong, vinner],
    );
    assert!(leaks.is_empty(), "{leaks:?}");
    let args = t.h.ws.buildx_record("args.txt");
    let env = t.h.ws.buildx_record("env.txt");
    for v in [v3, v8, v40, "AAAA-line-one", vlong, vinner] {
        assert!(!args.contains(v) && !env.contains(v), "{v}");
    }
    let scratch = t.rt.join("scratch");
    let dirs: Vec<_> = std::fs::read_dir(&scratch)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("buildsecrets-"))
                .collect()
        })
        .unwrap_or_default();
    assert!(dirs.is_empty(), "{dirs:?}");
    // the read-only commands and the diff are clean too
    for cmd in [
        vec!["status"],
        vec!["config-defs"],
        vec!["config-show", "--app", APP],
    ] {
        let o = cfg(&t, &kc, &cmd);
        for v in [v3, v8, v40, "AAAA-line-one", vlong, vinner] {
            assert!(
                !o.stdout.contains(v) && !o.stderr.contains(v),
                "{cmd:?} {v}"
            );
        }
    }
    // Debug formatting of a secret-bearing value never prints it
    let mut b = harpoon_deploy::secrets::SecretBundle::new();
    b.insert(
        SecretRef::new("x", 1),
        SecretValue::new(v40.as_bytes().to_vec()).unwrap(),
    );
    assert!(
        !format!("{b:?}").contains(v40)
            && !format!("{:?}", b.get(&SecretRef::new("x", 1))).contains(v40)
    );
}

#[test]
fn the_report_documents_what_cannot_be_detected_or_hidden() {
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/results/DEPLOY_MILESTONE_C_CONFIGURATION_SECRETS.md"),
    )
    .expect("the Milestone C report exists");
    for must in [
        "base64",
        "not detected",
        "docker inspect",
        "docker history",
        "image layer",
        "Docker's own log store",
        "not live",
    ] {
        assert!(doc.contains(must), "the report must say: {must}");
    }
}
