//! Criterion 7: `status` when the deployment store is missing, corrupt or healthy. A fake Docker on
//! a test-owned socket and isolated temporary stores; no real Docker, VM or container is involved.

use super::*;
use support::fake_docker::Action;

const APP: &str = "app_00000000000000000000000001";
const DEP: &str = "dep_00000000000000000000000010";

fn ours(t: &T, name: &str, app: &str, dep: &str, running: bool) -> String {
    let lbl = harpoon_deploy::backend::harpoon_labels(&app.parse().unwrap(), &dep.parse().unwrap());
    t.h.docker.with(|s| s.add_container(name, lbl, running))
}

fn status(t: &T) -> Outcome {
    let sock = t.socket();
    t.cli(&["status", "--docker-socket", &sock])
}

type Seen = Vec<(String, bool, Vec<(String, String)>)>;

fn snapshot(t: &T) -> Seen {
    t.h.docker.with(|s| {
        s.containers
            .iter()
            .map(|c| (c.id.clone(), c.running, c.labels.clone()))
            .collect()
    })
}

fn only_reads(t: &T) {
    let reqs = t.h.docker.requests();
    assert!(!reqs.is_empty());
    assert!(
        reqs.iter()
            .all(|r| r.method == "GET" && r.path.starts_with("/containers/json")),
        "discovery must only list containers: {reqs:?}"
    );
}

#[test]
fn missing_state_lists_labelled_workloads_with_a_history_warning() {
    let t = T::new();
    let a = ours(&t, "hp-one", APP, DEP, true);
    let b = ours(&t, "hp-two", APP, "dep_00000000000000000000000011", false);
    t.h.docker
        .with(|s| s.containers[0].image = support::fake_iid(7));
    let before = snapshot(&t);
    let o = status(&t);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    let s = &o.stdout;
    assert!(
        s.contains("WARNING: deployment history is unavailable"),
        "{s}"
    );
    assert!(
        s.contains("nothing was adopted, recorded or recreated"),
        "{s}"
    );
    assert!(
        s.contains("2 Harpoon-labelled container(s) (UNVERIFIED"),
        "{s}"
    );
    assert!(
        s.contains(&format!(
            "discovered container {a} name=hp-one state=running"
        )),
        "{s}"
    );
    assert!(
        s.contains(&format!(
            "discovered container {b} name=hp-two state=exited"
        )),
        "{s}"
    );
    assert!(
        s.contains(&format!(
            "application={APP} deployment={DEP} image={}",
            support::fake_iid(7)
        )),
        "{s}"
    );
    // discovery is not history: no deployment lines, no serving claim
    assert!(
        !s.contains("deployments:") && !s.contains("serving:"),
        "{s}"
    );
    only_reads(&t);
    assert_eq!(
        snapshot(&t),
        before,
        "nothing was stopped, removed or relabelled"
    );
    assert!(
        !t.rt.exists(),
        "status must not create the runtime directory"
    );
}

#[test]
fn missing_state_with_no_resources_says_so_and_still_warns() {
    let t = T::new();
    let o = status(&t);
    assert_eq!(o.code, 0, "{}", o.stdout);
    assert!(
        o.stdout
            .contains("WARNING: deployment history is unavailable"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("0 Harpoon-labelled container(s)"),
        "{}",
        o.stdout
    );
    assert!(!t.rt.exists());
}

#[test]
fn foreign_and_unlabelled_containers_are_never_attributed_to_harpoon() {
    let t = T::new();
    // named like ours, but carrying no labels / only another vendor's labels / a non-"true" managed label
    t.h.docker.with(|s| {
        s.add_container(&format!("hp-{DEP}"), vec![], true);
        s.add_container(
            "hp-dep_00000000000000000000000099",
            vec![("com.example".into(), "x".into())],
            true,
        );
        s.add_container(
            "almost",
            vec![
                ("io.harpoon.managed".into(), "false".into()),
                ("io.harpoon.app".into(), APP.into()),
            ],
            true,
        );
    });
    let real = ours(&t, "plain-name", APP, DEP, true);
    let o = status(&t);
    assert_eq!(o.code, 0);
    let s = &o.stdout;
    assert!(
        s.contains("1 Harpoon-labelled container(s)") && s.contains("3 other container(s) ignored"),
        "{s}"
    );
    assert!(
        s.contains(&format!("discovered container {real} name=plain-name")),
        "{s}"
    );
    assert_eq!(s.matches("discovered container").count(), 1, "{s}");
    only_reads(&t);
}

#[test]
fn a_harpoon_label_without_usable_identity_is_reported_as_unknown_not_guessed() {
    let t = T::new();
    t.h.docker.with(|s| {
        s.add_container(
            &format!("hp-{DEP}"),
            vec![("io.harpoon.managed".into(), "true".into())],
            true,
        );
        s.add_container(
            "bad",
            vec![
                ("io.harpoon.managed".into(), "true".into()),
                ("io.harpoon.app".into(), "not-an-id".into()),
            ],
            true,
        );
    });
    let o = status(&t);
    let s = &o.stdout;
    assert_eq!(
        s.matches("unknown (label missing or malformed)").count(),
        4,
        "{s}"
    );
    assert!(
        !s.contains("deployment=dep_"),
        "the id in a name must not become the identity:\n{s}"
    );
}

#[test]
fn unavailable_docker_is_reported_as_unknown_not_as_an_empty_inventory() {
    let t = T::new();
    // no socket at that path
    let gone = t.h.ws.root.join("no-such.sock").display().to_string();
    let o = t.cli(&["status", "--docker-socket", &gone]);
    assert_eq!(o.code, 5, "{}{}", o.stdout, o.stderr);
    assert!(
        o.stdout.contains("docker inspection FAILED"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("UNKNOWN, not empty"), "{}", o.stdout);
    assert!(!o.stdout.contains("0 Harpoon-labelled"), "{}", o.stdout);
    // and an engine that answers with an error
    ours(&t, "hp-one", APP, DEP, true);
    t.h.docker.fault(
        Some("GET"),
        "/containers/json",
        1,
        Action::Respond(500, "{\"message\":\"boom\"}".into()),
    );
    let o = status(&t);
    assert_eq!(o.code, 5, "{}", o.stdout);
    assert!(
        o.stdout.contains("UNKNOWN, not empty") && !o.stdout.contains("discovered container"),
        "{}",
        o.stdout
    );
}

#[test]
fn corrupt_state_fails_closed_and_never_falls_back_to_docker() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let mut bytes = t.journal_bytes();
    let at = bytes.windows(4).position(|w| w == b"dep_").unwrap() + 6;
    bytes[at] ^= 0x01;
    std::fs::write(t.journal_dir().join(SQLITE_FILE), &bytes).unwrap();
    ours(&t, "hp-one", APP, DEP, true);
    let damaged = t.journal_bytes();
    let o = status(&t);
    assert_eq!(o.code, 4, "{}{}", o.stdout, o.stderr);
    let text = format!("{}{}", o.stdout, o.stderr);
    assert!(
        text.contains("the journal is not usable") && text.contains("Do not delete"),
        "{text}"
    );
    assert!(
        !text.contains("discovered container") && !text.contains("history is unavailable"),
        "{text}"
    );
    assert_eq!(
        t.docker_requests(),
        0,
        "Docker must not be consulted when the store is corrupt"
    );
    assert_eq!(
        t.journal_bytes(),
        damaged,
        "the damaged store is left exactly as found"
    );
}

#[test]
fn healthy_state_keeps_the_normal_status_and_does_not_consult_docker() {
    let t = T::new();
    assert_exit(&t.sim_deploy(), 0);
    let plain = t.cli(&["status"]).stdout;
    ours(&t, "hp-one", APP, DEP, true);
    let o = status(&t);
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains("deployments: 1"), "{}", o.stdout);
    assert!(!o.stdout.contains("discovered container"), "{}", o.stdout);
    assert_eq!(t.docker_requests(), 0);
    let body = o.stdout.replace(
        "note: deployment history is available, so Docker was not consulted\n",
        "",
    );
    assert_eq!(body, plain);
}

#[test]
fn missing_state_without_a_socket_never_contacts_docker_and_says_how_to_look() {
    let t = T::new();
    ours(&t, "hp-one", APP, DEP, true);
    let o = t.cli(&["status"]);
    assert_eq!(o.code, 0);
    assert!(
        o.stdout.contains("not initialised") && o.stdout.contains("--docker-socket PATH"),
        "{}",
        o.stdout
    );
    assert_eq!(t.docker_requests(), 0);
}
