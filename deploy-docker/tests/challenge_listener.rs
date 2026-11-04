//! The guest identity listener's scripts, run for real against shimmed guest tools (macOS has no `setsid`, and no
//! `socat` is installed): the process management (`sh`, `sleep`, `pkill`, `pgrep`) is real, so the real patterns must
//! find, expire and kill the fake listener, which carries the real command line.
use harpoon_deploy_docker::attribution::{START_SCRIPT, STOP_SCRIPT};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210";

struct Guest {
    dir: PathBuf,
    port: &'static str,
}

impl Guest {
    fn new(name: &str, port: &'static str) -> Guest {
        let dir = std::env::temp_dir().join(format!("hp-listener-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shim = |n: &str, body: &str| {
            let p = dir.join(n);
            std::fs::write(&p, format!("#!/bin/bash\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
        };
        shim("setsid", "exec \"$@\"");
        // a port that is already taken: the real socat exits at once
        shim(
            "socat",
            "[ -n \"$SHIM_SOCAT_FAIL\" ] && exit 1\nexec -a \"socat $1 $2\" /bin/sleep 60",
        );
        Guest { dir, port }
    }
    fn sh(&self, script: &str, args: &[&str]) -> (i32, String) {
        let o = Command::new("/usr/bin/env")
            .env("PATH", format!("{}:/usr/bin:/bin", self.dir.display()))
            .args(["sh", "-c", script, "harpoon-test"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stderr).into(),
        )
    }
    fn start(&self, token: &str, port: &str, secs: &str) -> (i32, String) {
        self.sh(START_SCRIPT, &[token, port, secs])
    }
    fn stop(&self, port: &str) {
        self.sh(STOP_SCRIPT, &[port]);
    }
    fn alive(&self, token: &str, port: &str) -> bool {
        Command::new("/usr/bin/pgrep")
            .args(["-f", &format!("[T]CP4-LISTEN:{port},.*{token}")])
            .output()
            .unwrap()
            .status
            .success()
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        // only this test's own port: the tests run in parallel
        let _ = Command::new("/usr/bin/pkill")
            .args(["-f", &format!("[T]CP4-LISTEN:{},", self.port)])
            .status();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_listener_that_is_started_and_stopped_leaves_nothing_behind() {
    let g = Guest::new("normal", "45201");
    assert_eq!(g.start(TOKEN, "45201", "30").0, 0);
    assert!(g.alive(TOKEN, "45201"));
    g.stop("45201");
    assert!(!g.alive(TOKEN, "45201"));
    g.stop("45201"); // stopping an absent listener is not an error
}

#[test]
fn a_listener_nobody_stops_expires_by_itself() {
    let g = Guest::new("expiry", "45202");
    assert_eq!(g.start(TOKEN, "45202", "1").0, 0);
    assert!(g.alive(TOKEN, "45202"));
    let t = Instant::now();
    while g.alive(TOKEN, "45202") {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "the listener outlived its lifetime"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn an_expiry_ends_only_its_own_challenge_and_never_a_later_probes_listener() {
    let g = Guest::new("later", "45203");
    assert_eq!(g.start(TOKEN, "45203", "1").0, 0);
    // a later probe replaces the first listener with its own, long-lived one
    assert_eq!(g.start(OTHER, "45203", "30").0, 0);
    assert!(
        !g.alive(TOKEN, "45203"),
        "starting replaces the earlier listener"
    );
    std::thread::sleep(Duration::from_millis(1600)); // the first watchdog fires now
    assert!(
        g.alive(OTHER, "45203"),
        "the first challenge's watchdog must not kill the second listener"
    );
    g.stop("45203");
    assert!(!g.alive(OTHER, "45203"));
}

#[test]
fn an_occupied_port_is_refused_and_nothing_is_left_running() {
    let g = Guest::new("busy", "45204");
    let o = Command::new("/usr/bin/env")
        .env("PATH", format!("{}:/usr/bin:/bin", g.dir.display()))
        .env("SHIM_SOCAT_FAIL", "1")
        .args([
            "sh",
            "-c",
            START_SCRIPT,
            "harpoon-test",
            TOKEN,
            "45204",
            "30",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_ne!(o.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&o.stderr).contains("identity listener did not start"));
    assert!(!g.alive(TOKEN, "45204"));
}
