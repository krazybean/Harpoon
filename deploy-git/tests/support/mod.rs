//! Shared Git fixtures for tests: temp directories, a `git` helper, a small repository, and a smart-HTTP
//! server with Basic authentication (`git http-backend`). Used by this crate's and the CLI's tests.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Tmp(pub PathBuf);
impl Tmp {
    pub fn new() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "hdg-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Tmp(std::fs::canonicalize(&p).unwrap())
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("/usr/bin/git")
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .args(["-c", "init.defaultBranch=main", "-c", "protocol.file.allow=always"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// A repository with two commits on main, a branch `release/1`, a lightweight tag and an annotated tag.
pub fn fixture(root: &Path) -> (PathBuf, String, String) {
    let repo = root.join("origin");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("Dockerfile"), "FROM alpine:3.22\nCMD [\"true\"]\n").unwrap();
    std::fs::write(repo.join("VERSION"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let first = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["tag", "-a", "v1", "-m", "release one"]);
    git(&repo, &["branch", "release/1"]);
    std::fs::write(repo.join("VERSION"), "two\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "two"]);
    let second = git(&repo, &["rev-parse", "HEAD"]);
    (repo, first, second)
}

/// `git http-backend` behind a server that demands `Authorization: Basic base64(user:token)`.
pub struct HttpGit {
    pub port: u16,
    pub token: Arc<Mutex<String>>,
    pub seen_auth: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

pub fn b64(s: &str) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b = s.as_bytes();
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl HttpGit {
    pub fn start(root: PathBuf, user: &'static str, token: &str) -> HttpGit {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let token = Arc::new(Mutex::new(token.to_string()));
        let (tk, seen, stop) = (
            token.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(AtomicBool::new(false)),
        );
        let (seen2, stop2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                let Ok((mut c, _)) = l.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                c.set_nonblocking(false).unwrap();
                c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let head_end = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i;
                    }
                    match c.read(&mut chunk) {
                        Ok(0) | Err(_) => break usize::MAX,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                };
                if head_end == usize::MAX {
                    continue;
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.split("\r\n");
                let mut first = lines.next().unwrap().split(' ');
                let (method, target) = (first.next().unwrap().to_string(), first.next().unwrap().to_string());
                let hdr = |n: &str| {
                    lines.clone().find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case(n))
                            .map(|(_, v)| v.trim().to_string())
                    })
                };
                let want = format!("Basic {}", b64(&format!("{user}:{}", tk.lock().unwrap())));
                let got = hdr("Authorization");
                seen2.lock().unwrap().push(got.clone().unwrap_or_default());
                if got.as_deref() != Some(want.as_str()) {
                    let _ = c.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"git\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    continue;
                }
                let len: usize = hdr("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
                let mut body = buf[head_end + 4..].to_vec();
                while body.len() < len {
                    match c.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => body.extend_from_slice(&chunk[..n]),
                    }
                }
                let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                let mut cgi = Command::new("/usr/bin/git");
                cgi.arg("http-backend")
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .env("GIT_PROJECT_ROOT", &root)
                    .env("GIT_HTTP_EXPORT_ALL", "1")
                    .env("REQUEST_METHOD", &method)
                    .env("PATH_INFO", path)
                    .env("QUERY_STRING", query)
                    .env("CONTENT_TYPE", hdr("Content-Type").unwrap_or_default())
                    .env("CONTENT_LENGTH", len.to_string())
                    .env("REMOTE_USER", user)
                    .env("HTTP_GIT_PROTOCOL", hdr("Git-Protocol").unwrap_or_default())
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null());
                let mut child = cgi.spawn().unwrap();
                child.stdin.take().unwrap().write_all(&body).ok();
                let out = child.wait_with_output().unwrap().stdout;
                let split = out
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|i| (i, 4))
                    .or_else(|| out.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)));
                let Some((i, sep)) = split else { continue };
                let cgi_head = String::from_utf8_lossy(&out[..i]).to_string();
                let status = cgi_head
                    .lines()
                    .find_map(|l| l.strip_prefix("Status:"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| "200 OK".into());
                let mut resp = format!("HTTP/1.1 {status}\r\n");
                for l in cgi_head.lines().filter(|l| !l.starts_with("Status:")) {
                    resp.push_str(l);
                    resp.push_str("\r\n");
                }
                let payload = &out[i + sep..];
                resp.push_str(&format!(
                    "Content-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                ));
                let _ = c.write_all(resp.as_bytes());
                let _ = c.write_all(payload);
            }
        });
        HttpGit {
            port,
            token,
            seen_auth: seen,
            stop,
        }
    }
    pub fn url(&self, repo: &str) -> String {
        format!("http://127.0.0.1:{}/{repo}", self.port)
    }
}

impl Drop for HttpGit {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
