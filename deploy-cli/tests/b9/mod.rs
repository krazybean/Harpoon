//! Health-gated replacement through the CLI (simulated only). Live replacement is refused before
//! anything is read; the offline command prepares a candidate, shows the cutover plan, and cuts
//! over only on `--confirm-cutover`, recording the measured (simulated) interruption.

use super::*;

fn app_of(o: &Outcome) -> String {
    let after = o
        .stdout
        .split("(application ")
        .nth(1)
        .expect("an application id");
    after.split(')').next().unwrap().to_string()
}

fn replace(t: &T, app: &str, extra: &[&str]) -> Outcome {
    let src = t.src();
    let mut a: Vec<&str> = vec!["replace", "--simulate", "--app", app];
    a.extend(src.iter().map(String::as_str));
    a.extend(extra);
    t.cli(&a)
}

fn first(t: &T) -> String {
    let o = t.sim_deploy();
    assert_exit(&o, 0);
    app_of(&o)
}

fn deployment_lines(status_out: &str) -> Vec<String> {
    status_out
        .lines()
        .filter(|l| l.starts_with("  dep_"))
        .map(String::from)
        .collect()
}

#[test]
fn live_replacement_is_refused_before_anything_is_read_even_with_every_authorization() {
    let t = T::new();
    let app = "app_00000000000000000000000000";
    let sock = t.socket();
    let res = t.resources();
    let src = t.src();
    let mut a: Vec<&str> = vec![
        "replace",
        "--app",
        app,
        "--docker-socket",
        &sock,
        "--resources-dir",
        &res,
        "--authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE",
    ];
    a.extend(src.iter().map(String::as_str));
    for gate in [&SHIPPING, &OPEN] {
        let o = t.go(gate, &a);
        assert_exit(&o, 6);
        assert!(
            o.stderr.contains("live replacement is not enabled")
                && o.stderr.contains("bounded cutover recovery"),
            "{}",
            o.stderr
        );
    }
    assert_eq!(t.docker_requests(), 0);
    assert!(!t.rt.exists(), "a refused live replacement creates nothing");
}

#[test]
fn replacing_with_no_running_release_is_refused_and_creates_no_journal() {
    let t = T::new();
    let o = replace(&t, "app_00000000000000000000000000", &[]);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("no running release"), "{}", o.stderr);
    assert!(!t.journal_dir().exists());
}

#[test]
fn an_unconfirmed_replacement_prepares_the_candidate_and_leaves_the_release_untouched() {
    let t = T::new();
    let app = first(&t);
    let before = deployment_lines(&t.cli(&["status"]).stdout);
    assert_eq!(before.len(), 1);

    let o = replace(&t, &app, &[]);
    assert_exit(&o, 0);
    // the plan and the anticipated interruption come before the build result
    let plan_at = o.stdout.find("cutover plan").expect("plan printed");
    let result_at = o.stdout.find("result:").expect("result printed");
    assert!(plan_at < result_at, "{}", o.stdout);
    assert!(
        o.stdout
            .contains("anticipated interruption: unmeasured; at most")
            && o.stdout.contains("down from this moment")
            && o.stdout.contains("cutover not confirmed")
            && o.stdout
                .contains("candidate prepared, current release untouched"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("result: cancelled"), "{}", o.stdout);
    assert!(
        o.stdout.contains("evidence preliminary_check")
            || o.stdout.contains("evidence preliminary"),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("measured interruption"), "{}", o.stdout);

    let s = t.cli(&["status"]).stdout;
    let after = deployment_lines(&s);
    assert_eq!(after.len(), 2, "{s}");
    assert!(after.iter().any(|l| l.contains("cancelled")), "{s}");
    assert!(
        after.iter().any(|l| l == &before[0]),
        "the running release's record is unchanged:\n{s}"
    );
    assert!(!s.contains("interruption:"), "no cutover happened: {s}");
    assert!(!s.contains("application may be down"), "{s}");
    assert_eq!(t.docker_requests(), 0);
}

#[test]
fn a_confirmed_replacement_records_the_measured_interruption_durably() {
    let t = T::new();
    let app = first(&t);
    let o = replace(&t, &app, &["--confirm-cutover"]);
    assert_exit(&o, 0);
    assert!(
        o.stdout.contains("result: succeeded")
            && o.stdout
                .contains("measured interruption: 1000 ms (simulated clock"),
        "{}",
        o.stdout
    );
    // a later, separate process sees it: it is in the durable store
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains("interruption: 1000 ms between stopping the previous release"),
        "{s}"
    );
    assert_eq!(
        deployment_lines(&s)
            .iter()
            .filter(|l| l.contains("succeeded"))
            .count(),
        2,
        "{s}"
    );
    assert!(s.contains(&format!("app={app}")), "{s}");
    assert_exit(&t.cli(&["doctor"]), 0);

    // the next replacement replaces the newest release, not the first one
    let newest = deployment_lines(&s)
        .into_iter()
        .max_by_key(|l| {
            l.split("created_ms=")
                .nth(1)
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .unwrap();
    let newest_id = newest.split_whitespace().next().unwrap().to_string();
    let o = replace(&t, &app, &[]);
    assert_exit(&o, 0);
    assert!(
        o.stdout
            .contains(&format!("replaces the release of deployment {newest_id}")),
        "{}",
        o.stdout
    );
}

#[test]
fn an_unfinished_cutover_blocks_replacement_and_status_says_the_application_may_be_down() {
    let t = T::new();
    let app = first(&t);
    // a replacement that died after stopping the healthy release: the cutover began, never confirmed
    {
        let app_id: ApplicationId = app.parse().unwrap();
        let dep: DeploymentId = "dep_00000000000000000000000001".parse().unwrap();
        let mut d = Deployment::new(dep, app_id, Trigger::Cli, 5);
        d.transition(DeploymentStatus::Queued, 6).unwrap();
        d.transition(DeploymentStatus::Running, 7).unwrap();
        d.begin_cutover(8).unwrap();
        let (mut j, _) = open_j(&t.journal_dir()).unwrap();
        j.persist(&d).unwrap();
    }
    let o = replace(&t, &app, &["--confirm-cutover"]);
    assert_exit(&o, 6);
    let o = t.cli(&["recover"]);
    assert_exit(&o, 0);
    let s = t.cli(&["status"]).stdout;
    assert!(
        s.contains("cutover began and was never confirmed: the application may be down"),
        "{s}"
    );
    // recovery marked it interrupted; a new replacement is still not allowed over a possibly-down app
    let before = t.journal_bytes();
    let o = replace(&t, &app, &["--confirm-cutover"]);
    assert_exit(&o, 6);
    assert!(o.stderr.contains("may be down"), "{}", o.stderr);
    assert_eq!(
        t.journal_bytes(),
        before,
        "a refused replacement writes nothing"
    );
}

#[test]
fn replace_needs_the_exact_flags_and_never_reads_the_environment() {
    let t = T::new();
    assert_exit(&t.cli(&["replace", "--simulate"]), 2);
    let o = t.cli(&["replace", "--simulate", "--app", "not-an-id"]);
    assert_exit(&o, 2);
    let o = t.cli(&["deploy", "--simulate", "--confirm-cutover"]);
    assert_exit(&o, 2);
}
