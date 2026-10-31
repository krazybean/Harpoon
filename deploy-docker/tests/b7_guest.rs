//! Guest free-space measurement through the existing management channel, against a fake guest
//! (a test-owned Unix socket speaking the `harpoon-mgmt` line protocol) and the fake Docker.
//! Nothing here reaches a real guest: what the real `df`/`hostname` print is an assumption.

mod support;

use harpoon_deploy::health::Observation;
use harpoon_deploy::state::*;
use harpoon_deploy_docker::backend::{DockerBackend, DockerConfig};
use harpoon_deploy_docker::guest::{free_bytes_from_df, GuestDiskProbe, MgmtClient, MgmtDfProbe};
use harpoon_deploy_docker::probe::HttpTransport;
use std::time::Duration;
use support::fake_mgmt::{FakeMgmt, Fault};
use support::*;

const GOOD_DF: &str = "Filesystem           1B-blocks         Used    Available Use% Mounted on\n/dev/vda1      40000000000  10000000000  30000000000  25% /\n";

fn probe(h: &Harness, g: &FakeMgmt) -> MgmtDfProbe {
    MgmtDfProbe::new(h.docker.path(), g.path(), Duration::from_millis(600))
}

fn reqs_of_docker(h: &Harness) -> Vec<String> {
    h.docker
        .requests()
        .into_iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect()
}

// ------------------------------------------------------------------ parsing

#[test]
fn df_output_is_accepted_only_when_it_is_exactly_one_filesystem_that_holds_the_path() {
    assert_eq!(
        free_bytes_from_df(GOOD_DF, "/var/lib/docker").unwrap(),
        30_000_000_000
    );
    // a data root on its own mount
    let own = "Filesystem 1B-blocks Used Available Use% Mounted on\n/dev/vdb 100 40 60 40% /var/lib/docker\n";
    assert_eq!(free_bytes_from_df(own, "/var/lib/docker").unwrap(), 60);
    let var = "Filesystem 1B-blocks Used Available Use% Mounted on\n/dev/vdb 100 40 60 40% /var\n";
    assert_eq!(free_bytes_from_df(var, "/var/lib/docker").unwrap(), 60);
    for (what, out) in [
        ("empty", ""),
        ("header only", "Filesystem 1B-blocks Used Available Use% Mounted on\n"),
        ("no header", "/dev/vda1 100 40 60 40% /\n"),
        ("two data lines", "Filesystem a b c d e\n/dev/a 100 40 60 40% /\n/dev/b 100 40 60 40% /var\n"),
        ("wrapped device name", "Filesystem 1B-blocks Used Available Use% Mounted on\n/dev/mapper/very-long-name\n 100 40 60 40% /\n"),
        ("too few fields", "Filesystem a b\n/dev/a 100 40\n"),
        ("non-numeric total", "Filesystem a b c d e\n/dev/a lots 40 60 40% /\n"),
        ("negative", "Filesystem a b c d e\n/dev/a 100 40 -60 40% /\n"),
        ("hex", "Filesystem a b c d e\n/dev/a 100 40 0x3c 40% /\n"),
        ("decimal", "Filesystem a b c d e\n/dev/a 100 40 60.5 40% /\n"),
        ("zero size", "Filesystem a b c d e\n/dev/a 0 0 0 0% /\n"),
        ("used+avail over total", "Filesystem a b c d e\n/dev/a 100 80 60 40% /\n"),
        ("no percent", "Filesystem a b c d e\n/dev/a 100 40 60 40 /\n"),
        ("mount is elsewhere", "Filesystem a b c d e\n/dev/a 100 40 60 40% /mnt/data\n"),
        ("mount is a sibling prefix", "Filesystem a b c d e\n/dev/a 100 40 60 40% /var/li\n"),
        ("relative mount", "Filesystem a b c d e\n/dev/a 100 40 60 40% var\n"),
        ("overflow", "Filesystem a b c d e\n/dev/a 99999999999999999999999 40 60 40% /\n"),
    ] {
        assert!(free_bytes_from_df(out, "/var/lib/docker").is_err(), "{what}");
    }
}

// ------------------------------------------------------------------ the measurement

#[test]
fn the_probe_asks_only_three_fixed_read_only_questions_and_reports_the_data_roots_free_bytes() {
    let h = Harness::new();
    h.docker.with(|s| {
        s.info = (
            "guest-1".into(),
            "/var/lib/docker".into(),
            "6.12.9-harpoon".into(),
        )
    });
    let g = FakeMgmt::start();
    g.set(|m| {
        m.hostname = "guest-1".into();
        m.kernel = "6.12.9-harpoon".into();
        m.total = 200_000_000_000;
        m.used = 1_000;
        m.avail = 123_456_789_012;
    });
    let (free, what) = probe(&h, &g).measure().unwrap();
    assert_eq!(free, 123_456_789_012);
    assert!(
        what.contains("guest-1") && what.contains("/var/lib/docker"),
        "{what}"
    );
    assert_eq!(
        g.requests(),
        ["hostname", "uname -r", "df -B1 /var/lib/docker"],
        "nothing else is ever run in the guest"
    );
    assert_eq!(
        reqs_of_docker(&h),
        ["GET /info"],
        "and Docker is only asked who it is"
    );
    assert!(probe(&h, &g).free_bytes().is_ok());
}

#[test]
fn a_data_root_that_docker_reports_elsewhere_is_what_gets_measured() {
    let h = Harness::new();
    h.docker.with(|s| {
        s.info = (
            "harpoon-guest".into(),
            "/mnt/docker-data".into(),
            "6.12.0-test".into(),
        )
    });
    let g = FakeMgmt::start();
    g.set(|m| {
        m.mount = "/mnt".into();
        m.avail = 7;
        m.used = 1;
        m.total = 8;
    });
    assert_eq!(probe(&h, &g).free_bytes().unwrap(), 7);
    assert_eq!(g.requests().last().unwrap(), "df -B1 /mnt/docker-data");
    // and a df about some other filesystem is refused
    g.set(|m| m.mount = "/srv".into());
    assert!(probe(&h, &g)
        .free_bytes()
        .unwrap_err()
        .contains("does not contain"));
}

#[test]
fn a_channel_that_reaches_a_different_guest_than_docker_is_refused_before_df_is_run() {
    for (what, host, kernel, want) in [
        (
            "other hostname",
            "some-other-vm",
            "6.12.0-test",
            "not the same guest",
        ),
        ("other kernel", "harpoon-guest", "5.15.0-other", "kernel"),
    ] {
        let h = Harness::new();
        let g = FakeMgmt::start();
        g.set(|m| {
            m.hostname = host.into();
            m.kernel = kernel.into();
        });
        let e = probe(&h, &g).free_bytes().unwrap_err();
        assert!(
            e.contains(want) && e.contains("not the same guest"),
            "{what}: {e}"
        );
        assert!(
            !g.requests().iter().any(|r| r.starts_with("df")),
            "{what}: df was never asked"
        );
    }
}

#[test]
fn every_kind_of_channel_failure_is_a_refusal() {
    let faults: Vec<(&str, &str, Fault, &str)> = vec![
        (
            "garbage",
            "hostname",
            Fault::Raw(b"not json\n".to_vec()),
            "not JSON",
        ),
        (
            "no newline",
            "hostname",
            Fault::Raw(br#"{"exit":0,"stdout":"harpoon-guest\n","stderr":""}"#.to_vec()),
            "exactly one line",
        ),
        (
            "two lines",
            "hostname",
            Fault::Raw(
                b"{\"exit\":0,\"stdout\":\"harpoon-guest\\n\",\"stderr\":\"\"}\n{\"exit\":0}\n"
                    .to_vec(),
            ),
            "exactly one line",
        ),
        (
            "empty reply",
            "hostname",
            Fault::Raw(Vec::new()),
            "exactly one line",
        ),
        (
            "missing exit",
            "hostname",
            Fault::Raw(b"{\"stdout\":\"x\",\"stderr\":\"\"}\n".to_vec()),
            "exit status",
        ),
        (
            "fractional exit",
            "hostname",
            Fault::Raw(b"{\"exit\":0.5,\"stdout\":\"x\",\"stderr\":\"\"}\n".to_vec()),
            "exit status",
        ),
        (
            "stdout not text",
            "hostname",
            Fault::Raw(b"{\"exit\":0,\"stdout\":5,\"stderr\":\"\"}\n".to_vec()),
            "stdout",
        ),
        (
            "not utf-8",
            "hostname",
            Fault::Raw(vec![0xff, 0xfe, b'\n']),
            "UTF-8",
        ),
        (
            "disconnect",
            "hostname",
            Fault::Disconnect,
            "exactly one line",
        ),
        ("hang", "hostname", Fault::Hang, "timed out"),
        (
            "non-zero exit",
            "uname -r",
            Fault::Exit(1, "boom".into()),
            "exited 1",
        ),
        (
            "command missing",
            "df -B1 /var/lib/docker",
            Fault::Exit(127, "not found".into()),
            "exited 127",
        ),
        (
            "oversized",
            "hostname",
            Fault::Raw(vec![b'a'; 200_000]),
            "too large",
        ),
    ];
    for (what, argv, fault, want) in faults {
        let h = Harness::new();
        let g = FakeMgmt::start();
        g.set(|m| m.faults.push((argv.to_string(), fault)));
        let e = probe(&h, &g).free_bytes().unwrap_err();
        assert!(e.contains(want), "{what}: {e}");
    }
    // nothing is listening at all
    let h = Harness::new();
    let dead = MgmtDfProbe::new(
        h.docker.path(),
        "/nonexistent/mgmt.sock",
        Duration::from_millis(300),
    );
    assert!(dead
        .free_bytes()
        .unwrap_err()
        .contains("cannot reach the guest management channel"));
}

#[test]
fn empty_multiline_or_control_character_answers_to_hostname_and_uname_are_refused() {
    for (what, host) in [
        ("empty", ""),
        ("two lines", "a\nb"),
        ("control", "gu\u{7}est"),
    ] {
        let h = Harness::new();
        let g = FakeMgmt::start();
        g.set(|m| m.hostname = host.into());
        assert!(probe(&h, &g).free_bytes().is_err(), "{what}");
    }
}

#[test]
fn a_df_that_does_not_make_sense_is_a_refusal_and_never_a_number() {
    let h = Harness::new();
    let g = FakeMgmt::start();
    for out in [
        "Filesystem a b c d e\n/dev/a 100 40 60 40% /\n/dev/b 1 1 1 1% /var\n",
        "garbage\n",
        "",
        "Filesystem a b c d e\n/dev/a 100 90 60 40% /\n",
    ] {
        g.set(|m| m.df_stdout = Some(out.into()));
        assert!(probe(&h, &g).free_bytes().is_err(), "{out:?}");
    }
}

#[test]
fn docker_info_that_is_missing_wrong_or_odd_is_a_refusal() {
    for (what, info) in [
        (
            "relative root",
            ("harpoon-guest", "var/lib/docker", "6.12.0-test"),
        ),
        (
            "dotdot root",
            ("harpoon-guest", "/var/../etc", "6.12.0-test"),
        ),
        ("dot root", ("harpoon-guest", "/var/./lib", "6.12.0-test")),
        (
            "newline in root",
            ("harpoon-guest", "/var/lib/docker\n/etc", "6.12.0-test"),
        ),
        ("empty root", ("harpoon-guest", "", "6.12.0-test")),
    ] {
        let h = Harness::new();
        h.docker
            .with(|s| s.info = (info.0.into(), info.1.into(), info.2.into()));
        let g = FakeMgmt::start();
        assert!(probe(&h, &g).free_bytes().is_err(), "{what}");
        assert!(
            g.requests().is_empty(),
            "{what}: the guest was not even asked"
        );
    }
    // Docker unreachable or answering with an error
    let h = Harness::new();
    h.docker.fault(
        Some("GET"),
        "/info",
        5,
        support::fake_docker::Action::Respond(500, "{\"message\":\"boom\"}".into()),
    );
    let g = FakeMgmt::start();
    assert!(probe(&h, &g)
        .free_bytes()
        .unwrap_err()
        .contains("Docker /info"));
    h.docker.fault(
        Some("GET"),
        "/info",
        5,
        support::fake_docker::Action::Respond(200, "{\"Name\":\"x\"}".into()),
    );
    assert!(probe(&h, &g).free_bytes().is_err());
    let nodocker = MgmtDfProbe::new(
        "/nonexistent/docker.sock",
        g.path(),
        Duration::from_millis(300),
    );
    assert!(nodocker.free_bytes().is_err());
}

#[test]
fn the_client_sends_a_literal_argv_with_no_shell() {
    let g = FakeMgmt::start();
    let c = MgmtClient::new(g.path(), Duration::from_secs(2));
    let r = c.exec(&["df", "-B1", "/weird dir; rm -rf /"]).unwrap();
    assert_eq!(r.exit, 0);
    assert_eq!(
        g.requests(),
        ["df -B1 /weird dir; rm -rf /"],
        "one argv element, untouched"
    );
}

// ------------------------------------------------------------------ through the backend

fn backend<T: HttpTransport>(h: &Harness, g: &FakeMgmt, t: T) -> DockerBackend<T> {
    let cfg: DockerConfig = config(&h.ws, &h.docker);
    DockerBackend::with_transport(cfg, Some(&h.ws.resources()), t)
        .unwrap()
        .with_entropy(test_entropy())
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 << 30))
        .with_guest_disk(probe(h, g))
}

#[test]
fn the_backend_builds_only_when_the_measured_guest_has_room() {
    for (what, avail, ok) in [
        ("room", 50_000_000_000u64, true),
        ("short", 1_000_000_000, false),
        ("none", 0, false),
    ] {
        let mut h = Harness::new();
        h.seed_built_image();
        let g = FakeMgmt::start();
        g.set(|m| {
            m.avail = avail;
            m.used = 0;
            m.total = 60_000_000_000;
        });
        let mut b = backend(&h, &g, Scripted::always(Observation::Status(200)));
        let mut j = StampedJournal::new();
        let r = h.run(&mut b, &mut j);
        if ok {
            assert_eq!(
                r.status,
                DeploymentStatus::Succeeded,
                "{what}: {:?}",
                h.dep.failure
            );
        } else {
            assert_eq!(r.status, DeploymentStatus::Failed, "{what}");
            assert_eq!(h.dep.failure.clone().unwrap().step, Step::Source);
            assert!(
                h.ws.buildx_record("args.txt").is_empty()
                    && !h.docker.requests().iter().any(|q| q.method != "GET"),
                "{what}"
            );
        }
    }
}

#[test]
fn an_unreachable_or_mismatched_guest_stops_the_deployment_at_its_first_step() {
    let mut h = Harness::new();
    let g = FakeMgmt::start();
    g.set(|m| m.hostname = "an-impostor".into());
    let mut b = backend(&h, &g, Scripted::always(Observation::Status(200)));
    let mut j = StampedJournal::new();
    assert_eq!(h.run(&mut b, &mut j).status, DeploymentStatus::Failed);
    let f = h.dep.failure.clone().unwrap();
    assert_eq!(f.step, Step::Source);
    assert!(
        f.message.contains("not the same guest") && f.message.contains("could not be verified"),
        "{}",
        f.message
    );
    assert!(h.ws.buildx_record("args.txt").is_empty());
    assert!(!h.docker.requests().iter().any(|q| q.method != "GET"));
}

#[test]
fn nothing_in_the_new_sources_or_tests_names_the_live_management_socket() {
    let live = ["/tmp/harpoon-mgmt", ".sock"].concat();
    for dir in ["src", "tests"] {
        for e in
            std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)).unwrap()
        {
            let p = e.unwrap().path();
            if p.extension().map(|x| x == "rs").unwrap_or(false) {
                assert!(
                    !std::fs::read_to_string(&p).unwrap().contains(&live),
                    "{}",
                    p.display()
                );
            }
        }
    }
}
