//! Why `destination` refuses what it refuses: for the odd authorities below, the real Git (libcurl) contacts a
//! different host than a parser that reads "after the last @" would name. The listener is on loopback and sees
//! only the TCP connection; nothing leaves this machine and no credential is involved.

mod support;

use harpoon_deploy::git::destination;
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Whether `git ls-remote <url>` opens a connection to `listener` within a few seconds.
fn git_connects_to(listener: &TcpListener, url: &str) -> bool {
    listener.set_nonblocking(true).unwrap();
    let home = support::Tmp::new();
    let mut child = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home.0)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "credential.helper=", "ls-remote", "--", url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    let mut connected = false;
    while Instant::now() < end {
        if listener.accept().is_ok() {
            connected = true;
            break;
        }
        if child.try_wait().unwrap().is_some() {
            // it may have exited after connecting: look once more
            connected = listener.accept().is_ok();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    connected
}

#[test]
fn git_ends_the_authority_at_a_hash_or_a_question_mark_so_destination_refuses_them() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    // the part before `#`/`?` is where Git goes; the part after the `@` is what a naive parser would name
    for delimiter in ["#", "?"] {
        let url = format!("http://127.0.0.1:{port}{delimiter}@trusted.invalid/r.git");
        assert!(
            git_connects_to(&l, &url),
            "Git did not contact the loopback listener for {url}"
        );
        assert_eq!(destination(&url), None, "{url} must not name a destination");
    }
    // the control: a plain loopback URL connects and is a destination
    let plain = format!("http://127.0.0.1:{port}/r.git");
    assert!(git_connects_to(&l, &plain));
    let d = destination(&plain).expect("a plain URL names a destination");
    assert_eq!((d.host.as_str(), d.port), ("127.0.0.1", port));
}
