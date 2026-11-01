//! A candidate that fails before any cutover: the real CLI against the fake Docker (live `deploy`
//! with the gate opened for tests). The output says what stayed untouched and offers only commands
//! that exist, and the suggested ones are actually run.

use super::*;

fn args(t: &T) -> Vec<String> {
    live_args(t)
}

fn go_open(t: &T) -> Outcome {
    let a = args(t);
    t.go(&OPEN, &a.iter().map(String::as_str).collect::<Vec<_>>())
}

fn bystander(t: &T) -> String {
    let (app, dep) = ids(77);
    t.h.docker.with(|s| {
        let id = s.add_container(
            "other-app",
            harpoon_deploy::backend::harpoon_labels(&app, &dep),
            true,
        );
        s.containers.last_mut().unwrap().host_port = Some(38900);
        id
    })
}

fn untouched(t: &T, id: &str) {
    for r in t.h.docker.requests() {
        assert!(
            r.method == "GET" || !r.path.contains(id),
            "{} {}",
            r.method,
            r.path
        );
    }
    assert!(t
        .h
        .docker
        .with(|s| s.containers.iter().find(|c| c.id == id).unwrap().running));
}

#[test]
fn a_candidate_that_never_becomes_healthy_is_reported_with_accurate_next_actions() {
    let t = T::new();
    let other = bystander(&t);
    image_appears_after_build(&t);
    let o = go_open(&t);
    assert_exit(&o, 7);
    let s = &o.stdout;
    assert!(
        s.contains("failure at preliminary_check") && s.contains("evidence preliminary_check:"),
        "{s}"
    );
    assert!(
        s.contains("the candidate failed at preliminary_check before any cutover began"),
        "{s}"
    );
    assert!(
        s.contains("no release was serving") && s.contains("no restoration was attempted"),
        "{s}"
    );
    assert!(
        s.contains("automatic triggers are not paused by this failure"),
        "{s}"
    );
    assert!(
        s.contains("the candidate's image is retained for inspection: harpoon/"),
        "{s}"
    );
    assert!(s.contains("nothing is retried automatically"), "{s}");
    assert!(
        s.contains(&format!("status --runtime-dir {}", t.rt.display())),
        "{s}"
    );
    // every `harpoon-deploy <command>` the output mentions is a command that exists
    let known = harpoon_deploy_cli::command_names();
    for l in s.lines() {
        let mut rest = l;
        while let Some(i) = rest.find("harpoon-deploy ") {
            rest = &rest[i + "harpoon-deploy ".len()..];
            let word = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches('`');
            assert!(known.contains(&word), "invented command {word:?} in: {l}");
        }
    }
    // nothing paused, nothing unresolved, history durable across a fresh process
    let st = t.cli(&["status"]).stdout;
    assert!(!st.contains("automatic triggers paused"), "{st}");
    assert!(
        st.contains("failed at preliminary_check") && st.contains("unresolved cleanup: none"),
        "{st}"
    );
    assert!(
        st.contains("retained images of unsuccessful deployments"),
        "{st}"
    );
    untouched(&t, &other);
    // the suggested removal is real: run exactly what was printed (as a dry run)
    let dep = s
        .lines()
        .find_map(|l| l.split("--deployment ").nth(1))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let img = s
        .lines()
        .find_map(|l| l.split("--image ").nth(1))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let sock = t.socket();
    let res = t.resources();
    let o = t.cli(&[
        "cleanup-image",
        "--deployment",
        &dep,
        "--image",
        &img,
        "--docker-socket",
        &sock,
        "--resources-dir",
        &res,
    ]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("dry run: nothing was changed")
            && o.stdout.contains("would: remove image"),
        "{}",
        o.stdout
    );
    assert!(
        t.h.docker.with(|s| s.images.iter().any(|i| i.id == img)),
        "the dry run removed nothing"
    );
}

#[test]
fn a_failed_build_reports_no_retained_image_and_leaves_everything_alone() {
    let t = T::new();
    let other = bystander(&t);
    // the fake build "succeeds" but produces no image
    let o = go_open(&t);
    assert_exit(&o, 7);
    let s = &o.stdout;
    assert!(s.contains("failure at build"), "{s}");
    assert!(
        s.contains("the candidate failed at build before any cutover began"),
        "{s}"
    );
    assert!(s.contains("the build produced no retained image"), "{s}");
    assert!(!s.contains("cleanup-image"), "nothing to remove: {s}");
    assert!(t
        .cli(&["status"])
        .stdout
        .contains("unresolved cleanup: none"));
    untouched(&t, &other);
}

#[test]
fn a_failed_first_attempt_can_be_retried_deliberately_as_a_new_deployment_without_blocking() {
    let t = T::new();
    image_appears_after_build(&t);
    assert_exit(&go_open(&t), 7);
    let before = t.journal_bytes();
    // status is read-only; nothing retries on its own
    let _ = t.cli(&["status"]);
    assert_eq!(t.journal_bytes(), before);
    // a deliberate retry is a new deployment (the retained image of the failed one does not block it)
    image_appears_after_build(&t);
    let o = go_open(&t);
    assert_exit(&o, 7);
    let st = t.cli(&["status"]).stdout;
    assert_eq!(st.matches("failed at preliminary_check").count(), 2, "{st}");
}
