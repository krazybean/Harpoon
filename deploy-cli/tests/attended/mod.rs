//! What `Gate::shipping()` opens and what it keeps closed.
//!
//! * Attended `deploy`, `replace`, `rollback` and `restore` are open, but each still demands the exact
//!   `--authorize-live-docker-changes` phrase and every explicit path: without it nothing is read, written or contacted.
//! * Execution driven by a trigger (`git-run`, `watch` without `--queue-only`) stays closed: Gate U has not passed.
//!   `watch --queue-only`, `git-trigger` and `git-deploy` (a revision a person names) are not affected by that gate.
//! * No environment variable or flag changes either fact.

use super::*;

const LIVE: [(&str, &[&str]); 4] = [
    ("deploy", &["--container-port", "8080"]),
    (
        "replace",
        &[
            "--app",
            "app_00000000000000000000000000",
            "--container-port",
            "8080",
        ],
    ),
    (
        "rollback",
        &[
            "--app",
            "app_00000000000000000000000000",
            "--to",
            "rel_00000000000000000000000010",
        ],
    ),
    (
        "restore",
        &["--deployment", "dep_00000000000000000000000000", "--apply"],
    ),
];

#[test]
fn the_shipping_gate_opens_attended_operations_and_keeps_trigger_driven_execution_closed() {
    let g = Gate::shipping();
    assert!(g.live_deploy_enabled, "attended initial deployment");
    assert!(
        g.live_replacement_enabled,
        "attended replacement, rollback and restoration"
    );
    assert!(
        !g.live_triggered_enabled,
        "Gate U: trigger-driven execution"
    );
}

#[test]
fn an_open_shipping_gate_still_refuses_every_live_operation_that_lacks_the_exact_authorization() {
    let t = T::new();
    let (sock, res) = (t.socket(), t.resources());
    let src = t.src();
    let wrong = format!("--{AUTH_FLAG}=yes");
    for (cmd, extra) in LIVE {
        for auth in [None, Some(wrong.as_str())] {
            let mut a: Vec<&str> = vec![cmd, "--docker-socket", &sock, "--resources-dir", &res];
            a.extend(extra.iter().copied());
            if cmd == "deploy" || cmd == "replace" {
                a.extend(
                    src.iter()
                        .map(String::as_str)
                        .filter(|s| !s.starts_with("--container-port") && *s != "8080"),
                );
            }
            if let Some(w) = auth {
                a.push(w);
            }
            let o = t.go(&SHIPPING, &a);
            assert_eq!(o.code, 6, "{cmd} {auth:?}: {}{}", o.stdout, o.stderr);
            assert!(
                o.stderr.contains(AUTH_PHRASE) && !o.stderr.contains("not enabled in this build"),
                "{cmd}: the refusal names the missing opt-in, not a closed gate: {}",
                o.stderr
            );
        }
    }
    assert!(!t.rt.exists(), "nothing was created");
    assert_eq!(t.docker_requests(), 0, "the socket was not contacted");
}

#[test]
fn trigger_driven_execution_is_refused_by_the_shipping_gate_before_anything_is_read() {
    let t = T::new();
    let (sock, res) = (t.socket(), t.resources());
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    for cmd in [vec!["git-run"], vec!["watch"], vec!["watch", "--once"]] {
        let mut a: Vec<&str> = cmd.clone();
        a.extend(["--docker-socket", &sock, "--resources-dir", &res, &auth]);
        let o = t.go(&SHIPPING, &a);
        assert_eq!(o.code, 6, "{cmd:?}: {}", o.stderr);
        assert!(
            o.stderr.contains("not enabled in this build") && o.stderr.contains("Gate U"),
            "{cmd:?}: {}",
            o.stderr
        );
        assert!(!t.rt.exists(), "{cmd:?}: nothing was created");
    }
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn queueing_events_needs_no_live_gate_and_deploys_nothing() {
    let t = T::new();
    // `watch --queue-only` passes the gates (it never deploys) and stops only because nothing is registered yet
    let o = t.go(&SHIPPING, &["watch", "--queue-only", "--once"]);
    assert_eq!(o.code, 6, "{}", o.stderr);
    assert!(o.stderr.contains("repo-add"), "{}", o.stderr);
    assert!(
        !o.stderr.contains("not enabled in this build"),
        "{}",
        o.stderr
    );
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn the_real_shipping_binary_keeps_trigger_driven_execution_closed_whatever_the_flags_and_environment(
) {
    let t = T::new();
    let (sock, res) = (t.socket(), t.resources());
    let auth = format!("--{AUTH_FLAG}={AUTH_PHRASE}");
    let rt = t.rt.display().to_string();
    for cmd in [vec!["git-run"], vec!["watch"]] {
        let mut a: Vec<&str> = cmd.clone();
        a.extend([
            "--runtime-dir",
            &rt,
            "--docker-socket",
            &sock,
            "--resources-dir",
            &res,
            &auth,
        ]);
        let out = spawn_binary(
            &a,
            &[
                ("HARPOON_ACCEPTANCE", "1"),
                ("HARPOON_LIVE", "1"),
                ("HARPOON_GATE_U", "open"),
            ],
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(6), "{cmd:?}: {err}");
        assert!(
            err.contains("not enabled in this build") && err.contains("Gate U"),
            "{cmd:?}: {err}"
        );
    }
    assert!(!t.rt.exists());
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn git_deploy_without_the_opt_in_is_refused_before_a_repository_is_contacted() {
    let t = T::new();
    let o = t.go(&SHIPPING, &["git-deploy", "--repo", "shop"]);
    assert_eq!(o.code, 6, "{}", o.stderr);
    assert!(o.stderr.contains(AUTH_PHRASE), "{}", o.stderr);
    assert!(!t.rt.exists(), "nothing was read or created");
    assert_eq!(t.docker_requests(), 0);
}
