//! B13: non-secret runtime variables through the real CLI (simulated runtime and fake Docker),
//! and the failure/next-action output of a failed candidate. Nothing here contacts a live endpoint.

use super::*;


fn app_of(o: &Outcome) -> String {
    o.stdout
        .split("(application ")
        .nth(1)
        .expect("an application id")
        .split(')')
        .next()
        .unwrap()
        .to_string()
}

fn sim_deploy_env(t: &T, env: &[&str]) -> Outcome {
    let src = t.src();
    let mut a: Vec<&str> = vec!["deploy", "--simulate"];
    a.extend(src.iter().map(String::as_str));
    for e in env {
        a.extend(["--env", e]);
    }
    t.cli(&a)
}

fn sim_replace_env(t: &T, app: &str, env: &[&str], extra: &[&str]) -> Outcome {
    let src = t.src();
    let mut a: Vec<&str> = vec!["replace", "--simulate", "--app", app];
    a.extend(src.iter().map(String::as_str));
    for e in env {
        a.extend(["--env", e]);
    }
    a.extend(extra);
    t.cli(&a)
}

/// Every pinned environment in the journal, by deployment order of creation.
fn pins(t: &T) -> Vec<Vec<(String, String)>> {
    let r = harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir()).unwrap();
    let mut d = r.deployments;
    d.sort_by_key(|d| d.created_ms);
    d.iter()
        .filter_map(|d| d.pinned.as_ref().map(|p| p.env.clone()))
        .collect()
}

fn kv(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn variables_are_validated_pinned_and_only_their_names_are_printed() {
    let t = T::new();
    let o = sim_deploy_env(&t, &["B=x=y", "A=", "TOKEN_LIKE=hunter2"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("environment (from --env, pinned with the release): A, B, TOKEN_LIKE — NON-SECRET values"),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains("hunter2") && !o.stdout.contains("x=y"),
        "values are not printed:\n{}",
        o.stdout
    );
    assert_eq!(
        pins(&t),
        vec![kv(&[("A", ""), ("B", "x=y"), ("TOKEN_LIKE", "hunter2")])]
    );
    // the status of a later process shows no values either
    let s = t.cli(&["status"]).stdout;
    assert!(!s.contains("hunter2"), "{s}");
}

#[test]
fn bad_variables_are_refused_before_anything_is_written_or_contacted() {
    for bad in [
        vec!["NOEQUALS"],
        vec!["=v"],
        vec!["1A=v"],
        vec!["A-B=v"],
        vec!["A=1", "A=2"],
        vec!["HARPOON_DEPLOYMENT_MARKER=abc"],
        vec!["HARPOON_ANYTHING=1"],
        vec!["A=line\nbreak"],
        vec!["A=nul\u{1}"],
    ] {
        let t = T::new();
        let o = sim_deploy_env(&t, &bad);
        assert_exit(&o, 3);
        assert!(
            !t.rt.exists(),
            "{bad:?}: no journal or runtime directory was created"
        );
        assert_eq!(t.docker_requests(), 0);
        // plan validates the same way and is read-only
        let src = t.src();
        let mut a: Vec<&str> = vec!["plan"];
        a.extend(src.iter().map(String::as_str));
        for e in &bad {
            a.extend(["--env", e]);
        }
        assert_exit(&t.cli(&a), 3);
        assert!(!t.rt.exists());
    }
    // the marker refusal says why
    let t = T::new();
    let o = sim_deploy_env(&t, &["HARPOON_DEPLOYMENT_MARKER=abc"]);
    assert!(
        o.stderr
            .contains("generated for every container and cannot be set"),
        "{}",
        o.stderr
    );
    // other value flags still reject a repeat
    let src = t.src();
    let mut a: Vec<&str> = vec![
        "deploy",
        "--simulate",
        "--health-path",
        "/a",
        "--health-path",
        "/b",
    ];
    a.extend(src.iter().map(String::as_str));
    assert_exit(&t.cli(&a), 2);
}

#[test]
fn plan_reports_names_and_is_read_only() {
    let t = T::new();
    let src = t.src();
    let mut a: Vec<&str> = vec!["plan", "--env", "SECRETISH=abc", "--env", "MODE=prod"];
    a.extend(src.iter().map(String::as_str));
    let o = t.cli(&a);
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains("environment (from --env): MODE, SECRETISH — NON-SECRET values"),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains("abc") && !o.stdout.contains("prod\n"),
        "{}",
        o.stdout
    );
    assert!(!t.rt.exists());
}

#[test]
fn no_variables_keeps_the_legacy_pin_and_a_replacement_changes_them_intentionally() {
    let t = T::new();
    let o = sim_deploy_env(&t, &[]);
    assert_exit(&o, 0);
    let app = app_of(&o);
    assert!(
        o.stdout
            .contains("environment (from --env, pinned with the release): none"),
        "{}",
        o.stdout
    );
    assert!(
        !t.journal_bytes().windows(7).any(|w| w == b"v2;img="),
        "no variables, no v2 pin"
    );
    let o = sim_replace_env(&t, &app, &["MODE=two"], &["--confirm-cutover"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains("environment differs from the serving release's pinned environment (none)"),
        "{}",
        o.stdout
    );
    assert_eq!(pins(&t), vec![vec![], kv(&[("MODE", "two")])]);
}

#[test]
fn a_manual_rollback_reproduces_the_target_releases_own_pinned_environment() {
    let t = T::new();
    let o = sim_deploy_env(&t, &["MODE=one", "ONLY_V1="]);
    assert_exit(&o, 0);
    let app = app_of(&o);
    assert_exit(
        &sim_replace_env(&t, &app, &["MODE=two"], &["--confirm-cutover"]),
        0,
    );
    let rel = {
        let r = harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir()).unwrap();
        let mut d = r.deployments;
        d.sort_by_key(|d| d.created_ms);
        d[0].release.clone().unwrap()
    };
    let before = pins(&t);
    let o = t.cli(&[
        "rollback",
        "--simulate",
        "--app",
        &app,
        "--to",
        rel.as_str(),
        "--confirm-rollback",
    ]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains(&format!(
            "environment (pinned by release {rel}; not taken from this command): MODE, ONLY_V1"
        )),
        "{}",
        o.stdout
    );
    let after = pins(&t);
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(
        after[2], before[0],
        "the rolled-back release serves with release one's own environment"
    );
    assert_eq!(
        before[1],
        kv(&[("MODE", "two")]),
        "release two was not altered"
    );
}

#[test]
fn a_damaged_stored_environment_fails_closed() {
    let t = T::new();
    assert_exit(&sim_deploy_env(&t, &["MODE=one"]), 0);
    let mut bytes = t.journal_bytes();
    let at = bytes.windows(5).position(|w| w == b";env=").unwrap() + 8;
    bytes[at] = if bytes[at] == b'0' { b'1' } else { b'0' };
    std::fs::write(t.journal_dir().join(SQLITE_FILE), &bytes).unwrap();
    let damaged = t.journal_bytes();
    for cmd in [vec!["status"], vec!["recover"]] {
        let o = t.cli(&cmd);
        assert_exit(&o, 4);
        assert!(
            format!("{}{}", o.stdout, o.stderr).contains("Do not delete"),
            "{}{}",
            o.stdout,
            o.stderr
        );
    }
    assert_eq!(t.journal_bytes(), damaged);
}
