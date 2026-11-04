//! Writer contention across every command that changes the journal, and two simultaneous deploys
//! for one application. Offline; no Docker.

use super::*;

#[test]
fn every_mutating_command_is_refused_fail_fast_while_a_writer_holds_the_journal() {
    let t = T::new();
    let o = sim_deploy_env(&t, &[]);
    assert_exit(&o, 0);
    let app = app_of(&o);
    let rel = {
        let r = harpoon_deploy_journal::SqliteJournal::read_only(&t.journal_dir()).unwrap();
        r.deployments[0].release.clone().unwrap()
    };
    let (_held, _) = open_j(&t.journal_dir()).unwrap();
    let before = t.journal_bytes();
    let src = t.src();
    let with_src = |base: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = base.iter().map(|s| s.to_string()).collect();
        v.extend(src.iter().cloned());
        v
    };
    let dep = "dep_00000000000000000000000099";
    let cmds: Vec<Vec<String>> = vec![
        with_src(&["deploy", "--simulate"]),
        with_src(&["replace", "--simulate", "--app", &app, "--confirm-cutover"]),
        [
            "rollback",
            "--simulate",
            "--app",
            &app,
            "--to",
            rel.as_str(),
            "--confirm-rollback",
        ]
        .map(String::from)
        .to_vec(),
        ["restore", "--simulate", "--deployment", dep, "--apply"]
            .map(String::from)
            .to_vec(),
        ["recover"].map(String::from).to_vec(),
    ];
    for c in cmds {
        let refs: Vec<&str> = c.iter().map(String::as_str).collect();
        let o = t.cli(&refs);
        assert!(
            o.code != 0,
            "{c:?} must not succeed while a writer holds the journal: {}",
            o.stdout
        );
        // a command may also refuse earlier for its own reasons (an unknown deployment); it must
        // never have changed anything
        assert_eq!(t.journal_bytes(), before, "{c:?} changed the journal");
        if o.code == 4 {
            assert!(
                o.stderr.contains("another Harpoon process"),
                "{c:?}: {}",
                o.stderr
            );
        }
    }
    // reading stays possible
    assert_exit(&t.cli(&["status"]), 0);
    assert_exit(&t.cli(&["releases", "--app", &app]), 0);
}

#[test]
fn two_simultaneous_deploys_for_one_application_cannot_both_proceed() {
    // the first writer wins the lock; the other is refused at once and writes nothing
    let t = T::new();
    let first = sim_deploy_env(&t, &[]);
    assert_exit(&first, 0);
    let app = app_of(&first);
    let src = t.src();
    let mut a: Vec<&str> = vec!["deploy", "--simulate", "--app", &app];
    a.extend(src.iter().map(String::as_str));
    let (held, _) = open_j(&t.journal_dir()).unwrap();
    let before = t.journal_bytes();
    let o = t.cli(&a);
    assert_exit(&o, 4);
    assert!(
        o.stderr
            .contains("another Harpoon process holds the journal"),
        "{}",
        o.stderr
    );
    assert_eq!(t.journal_bytes(), before);
    drop(held);
    // with the lock free, a second first-deployment for the same application is refused, not run
    let again = t.cli(&a);
    assert_exit(&again, 6);
    assert!(again.stderr.contains("deploy refused"), "{}", again.stderr);
    assert_eq!(t.journal_bytes(), before);
}
