//! Webhook verification and the normalized `repository push` event (spec D7, Milestone D).
//!
//! The listener exists to **normalize and verify**; it is bound to loopback only (the API takes no
//! address), caps the header block and the body before reading them, times out slow clients, and passes
//! nothing on until the signature or token has been checked in constant time over the raw body. Real
//! provider deliveries reach a laptop only through a user-supplied tunnel or relay (deferred); the
//! verification here is exercised with signed fixtures.
//!
//! * **GitHub:** `X-Hub-Signature-256: sha256=<hex>` is HMAC-SHA-256 of the raw body with the shared secret.
//! * **GitLab:** `X-Gitlab-Token` is the shared secret, compared in constant time. GitLab's delivery
//!   identifier header (`X-Gitlab-Event-UUID`) is **not confirmed** against a real instance: it is used when
//!   present, and otherwise the delivery id is derived from the pushed commit, so a replay of the same push
//!   is still one delivery.

use crate::sha256::{constant_time_eq, from_hex, hmac_sha256};
use harpoon_deploy::git::is_commit;
use harpoon_deploy_docker::json::Json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

pub const MAX_HEAD: usize = 16 * 1024;
pub const MAX_BODY: usize = 256 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    GitLab,
}

/// One verified push, the only shape that reaches the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushEvent {
    pub provider: Provider,
    pub repository: String,
    /// `refs/heads/main`
    pub ref_name: String,
    /// The pushed commit.
    pub revision: String,
    pub delivery: String,
}

impl PushEvent {
    /// The event source name the queue records.
    pub fn source(&self) -> &'static str {
        match self.provider {
            Provider::GitHub => "github",
            Provider::GitLab => "gitlab",
        }
    }
    pub fn branch(&self) -> Option<&str> {
        self.ref_name.strip_prefix("refs/heads/")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    TooLarge,
    BadSignature,
    Malformed(String),
    NotFound,
    MethodNotAllowed,
    Timeout,
}

impl Reject {
    pub fn status(&self) -> u16 {
        match self {
            Reject::TooLarge => 413,
            Reject::BadSignature => 401,
            Reject::Malformed(_) => 400,
            Reject::NotFound => 404,
            Reject::MethodNotAllowed => 405,
            Reject::Timeout => 408,
        }
    }
}

pub struct Headers(pub Vec<(String, String)>);

impl Headers {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn delivery_ok(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'))
}

fn zeros(rev: &str) -> bool {
    rev.bytes().all(|b| b == b'0')
}

/// Verify a GitHub delivery and normalize it. `Ok(None)` is a verified event that asks for nothing
/// (a ping, a non-push event, a branch deletion).
pub fn verify_github(secret: &[u8], h: &Headers, body: &[u8]) -> Result<Option<PushEvent>, Reject> {
    let sig = h.get("X-Hub-Signature-256").ok_or(Reject::BadSignature)?;
    let want = sig
        .strip_prefix("sha256=")
        .and_then(from_hex)
        .ok_or(Reject::BadSignature)?;
    if !constant_time_eq(&hmac_sha256(secret, body), &want) {
        return Err(Reject::BadSignature);
    }
    // only now is the body looked at
    match h.get("X-GitHub-Event") {
        Some("push") => {}
        Some(_) => return Ok(None),
        None => return Err(Reject::Malformed("no X-GitHub-Event header".into())),
    }
    let delivery = h
        .get("X-GitHub-Delivery")
        .filter(|d| delivery_ok(d))
        .ok_or_else(|| Reject::Malformed("no usable X-GitHub-Delivery header".into()))?;
    let j = Json::parse(std::str::from_utf8(body).map_err(|_| Reject::Malformed("the body is not UTF-8".into()))?)
        .map_err(|e| Reject::Malformed(format!("the body is not JSON: {}", e.0)))?;
    if j.get("deleted").and_then(Json::as_bool) == Some(true) {
        return Ok(None);
    }
    let after = j
        .get("after")
        .and_then(Json::as_str)
        .ok_or_else(|| Reject::Malformed("no 'after' commit".into()))?;
    if !is_commit(after) {
        return Err(Reject::Malformed("'after' is not a commit id".into()));
    }
    if zeros(after) {
        return Ok(None);
    }
    let r = j
        .get("ref")
        .and_then(Json::as_str)
        .ok_or_else(|| Reject::Malformed("no 'ref'".into()))?;
    let repo = j
        .get("repository")
        .and_then(|r| r.get("clone_url").or_else(|| r.get("html_url")))
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    Ok(Some(PushEvent {
        provider: Provider::GitHub,
        repository: repo,
        ref_name: r.to_string(),
        revision: after.to_string(),
        delivery: delivery.to_string(),
    }))
}

/// Verify a GitLab delivery (shared token) and normalize it.
pub fn verify_gitlab(token: &[u8], h: &Headers, body: &[u8]) -> Result<Option<PushEvent>, Reject> {
    let got = h.get("X-Gitlab-Token").ok_or(Reject::BadSignature)?;
    if !constant_time_eq(got.as_bytes(), token) {
        return Err(Reject::BadSignature);
    }
    match h.get("X-Gitlab-Event") {
        Some("Push Hook") => {}
        Some(_) => return Ok(None),
        None => return Err(Reject::Malformed("no X-Gitlab-Event header".into())),
    }
    let j = Json::parse(std::str::from_utf8(body).map_err(|_| Reject::Malformed("the body is not UTF-8".into()))?)
        .map_err(|e| Reject::Malformed(format!("the body is not JSON: {}", e.0)))?;
    let after = j
        .get("checkout_sha")
        .and_then(Json::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| j.get("after").and_then(Json::as_str))
        .ok_or_else(|| Reject::Malformed("no commit in the push".into()))?;
    if !is_commit(after) {
        return Err(Reject::Malformed("the pushed revision is not a commit id".into()));
    }
    if zeros(after) {
        return Ok(None);
    }
    let r = j
        .get("ref")
        .and_then(Json::as_str)
        .ok_or_else(|| Reject::Malformed("no 'ref'".into()))?;
    let delivery = match h.get("X-Gitlab-Event-UUID") {
        Some(u) if delivery_ok(u) => u.to_string(),
        Some(_) => return Err(Reject::Malformed("X-Gitlab-Event-UUID is not usable".into())),
        None => format!("gitlab:{after}"),
    };
    let repo = j
        .get("project")
        .and_then(|p| p.get("git_http_url").or_else(|| p.get("web_url")))
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    Ok(Some(PushEvent {
        provider: Provider::GitLab,
        repository: repo,
        ref_name: r.to_string(),
        revision: after.to_string(),
        delivery,
    }))
}

// ------------------------------------------------------------------ requests

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// Read one HTTP/1.x request with hard limits: the header block at most [`MAX_HEAD`], the declared body
/// at most [`MAX_BODY`] (refused before it is read), a deadline for the whole thing, no chunked bodies.
pub fn read_request(stream: &mut impl Read) -> Result<Request, Reject> {
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(Reject::TooLarge);
        }
        if Instant::now() > deadline {
            return Err(Reject::Timeout);
        }
        match stream.read(&mut chunk) {
            Ok(0) => {
                return Err(Reject::Malformed(
                    "the connection closed before the request was complete".into(),
                ))
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err(Reject::Timeout)
            }
            Err(e) => return Err(Reject::Malformed(format!("read error: {e}"))),
        }
    };
    if head_end > MAX_HEAD {
        return Err(Reject::TooLarge);
    }
    let head =
        std::str::from_utf8(&buf[..head_end]).map_err(|_| Reject::Malformed("the headers are not text".into()))?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, path, version) = (
        first.next().unwrap_or(""),
        first.next().unwrap_or(""),
        first.next().unwrap_or(""),
    );
    if method.is_empty() || !path.starts_with('/') || !version.starts_with("HTTP/1.") {
        return Err(Reject::Malformed("not an HTTP/1.x request".into()));
    }
    let mut headers = Vec::new();
    for l in lines {
        let (k, v) = l
            .split_once(':')
            .ok_or_else(|| Reject::Malformed("a malformed header".into()))?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let headers = Headers(headers);
    if headers.get("Transfer-Encoding").is_some() {
        return Err(Reject::Malformed("chunked bodies are not accepted".into()));
    }
    let len = match headers.get("Content-Length") {
        None => 0,
        Some(v) => v
            .parse::<usize>()
            .map_err(|_| Reject::Malformed("a bad Content-Length".into()))?,
    };
    if len > MAX_BODY {
        return Err(Reject::TooLarge);
    }
    let mut body = buf[head_end + 4..].to_vec();
    if body.len() > len {
        body.truncate(len);
    }
    while body.len() < len {
        if Instant::now() > deadline {
            return Err(Reject::Timeout);
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(Reject::Malformed("the body is shorter than Content-Length".into())),
            Ok(n) => body.extend_from_slice(&chunk[..n.min(len - body.len())]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err(Reject::Timeout)
            }
            Err(e) => return Err(Reject::Malformed(format!("read error: {e}"))),
        }
    }
    Ok(Request {
        method: method.to_string(),
        path: path.to_string(),
        headers,
        body,
    })
}

// ------------------------------------------------------------------ the listener

/// Per-repository verification secrets. Held in memory only.
pub struct Routes {
    map: BTreeMap<String, (Provider, Vec<u8>)>,
}

impl Routes {
    pub fn new() -> Routes {
        Routes { map: BTreeMap::new() }
    }
    pub fn add(&mut self, repo: &str, provider: Provider, secret: Vec<u8>) {
        self.map.insert(repo.to_string(), (provider, secret));
    }
}

impl Default for Routes {
    fn default() -> Self {
        Routes::new()
    }
}

/// A verified push for a registered repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    pub repo: String,
    pub event: PushEvent,
}

/// Handle one request: route, verify, normalize. Pure over the bytes it is given.
pub fn handle(routes: &Routes, req: &Request) -> Result<Option<Inbound>, Reject> {
    if req.method != "POST" {
        return Err(Reject::MethodNotAllowed);
    }
    let path = req.path.split('?').next().unwrap_or("");
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let ["webhook", provider, repo] = parts[..] else {
        return Err(Reject::NotFound);
    };
    let (kind, secret) = routes.map.get(repo).ok_or(Reject::NotFound)?;
    let ev = match (provider, kind) {
        ("github", Provider::GitHub) => verify_github(secret, &req.headers, &req.body)?,
        ("gitlab", Provider::GitLab) => verify_gitlab(secret, &req.headers, &req.body)?,
        _ => return Err(Reject::NotFound),
    };
    Ok(ev.map(|event| Inbound {
        repo: repo.to_string(),
        event,
    }))
}

fn respond(stream: &mut TcpStream, status: u16, text: &str) {
    let reason = match status {
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let _ = write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
}

pub struct Listener {
    inner: TcpListener,
}

impl Listener {
    /// Bind `127.0.0.1:port` (0 chooses a free port). There is no way to ask for another address.
    pub fn bind_loopback(port: u16) -> std::io::Result<Listener> {
        let inner = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        inner.set_nonblocking(true)?;
        Ok(Listener { inner })
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr().expect("a bound listener has an address")
    }

    /// Serve connections until `stop` is set, sending each verified push to `tx`. One connection at a
    /// time, each bounded in size and time; a bad request costs the sender a refusal and nothing else.
    pub fn serve(&self, routes: &Routes, tx: &Sender<Inbound>, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            match self.inner.accept() {
                Ok((mut s, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    let _ = s.set_nonblocking(false);
                    let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                    match read_request(&mut s).and_then(|r| handle(routes, &r)) {
                        Ok(Some(i)) => {
                            let _ = tx.send(i);
                            respond(&mut s, 202, "accepted\n");
                        }
                        Ok(None) => respond(&mut s, 202, "ignored\n"),
                        Err(r) => respond(&mut s, r.status(), "refused\n"),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256::to_hex;

    const SECRET: &[u8] = b"It's a Secret to Everybody";
    const BODY: &str = r#"{"ref":"refs/heads/main","after":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","deleted":false,"repository":{"clone_url":"https://github.com/acme/shop.git"}}"#;

    fn gh(secret: &[u8], body: &str, event: &str, delivery: Option<&str>) -> Headers {
        let mut h = vec![
            (
                "X-Hub-Signature-256".to_string(),
                format!("sha256={}", to_hex(&hmac_sha256(secret, body.as_bytes()))),
            ),
            ("X-GitHub-Event".to_string(), event.to_string()),
        ];
        if let Some(d) = delivery {
            h.push(("X-GitHub-Delivery".to_string(), d.to_string()));
        }
        Headers(h)
    }

    #[test]
    fn a_correctly_signed_github_push_is_normalized() {
        let e = verify_github(
            SECRET,
            &gh(SECRET, BODY, "push", Some("72d3162e-cc78-11e3-81ab-4c9367dc0958")),
            BODY.as_bytes(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(e.provider, Provider::GitHub);
        assert_eq!(e.ref_name, "refs/heads/main");
        assert_eq!(e.branch(), Some("main"));
        assert_eq!(e.revision, "a".repeat(40));
        assert_eq!(e.delivery, "72d3162e-cc78-11e3-81ab-4c9367dc0958");
        assert_eq!(e.repository, "https://github.com/acme/shop.git");
        assert_eq!(e.source(), "github");
    }

    #[test]
    fn tampering_a_wrong_secret_and_a_missing_signature_are_all_refused_before_the_body_is_read() {
        let h = gh(SECRET, BODY, "push", Some("d1"));
        let tampered = BODY.replace("main", "evil");
        assert_eq!(
            verify_github(SECRET, &h, tampered.as_bytes()),
            Err(Reject::BadSignature)
        );
        assert_eq!(
            verify_github(b"another secret", &h, BODY.as_bytes()),
            Err(Reject::BadSignature)
        );
        assert_eq!(
            verify_github(SECRET, &Headers(vec![]), BODY.as_bytes()),
            Err(Reject::BadSignature)
        );
        for bad in ["sha256=zz", "sha1=abcd", "sha256=", "abcd"] {
            let h = Headers(vec![("x-hub-signature-256".into(), bad.into())]);
            assert_eq!(
                verify_github(SECRET, &h, BODY.as_bytes()),
                Err(Reject::BadSignature),
                "{bad}"
            );
        }
        // a signed but non-JSON body is malformed, not a crash
        let junk = "not json";
        assert!(matches!(
            verify_github(SECRET, &gh(SECRET, junk, "push", Some("d")), junk.as_bytes()),
            Err(Reject::Malformed(_))
        ));
    }

    #[test]
    fn verified_events_that_ask_for_nothing_are_ignored() {
        let h = gh(SECRET, BODY, "ping", Some("d1"));
        assert_eq!(verify_github(SECRET, &h, BODY.as_bytes()), Ok(None));
        let deleted = BODY.replace("\"deleted\":false", "\"deleted\":true");
        assert_eq!(
            verify_github(SECRET, &gh(SECRET, &deleted, "push", Some("d1")), deleted.as_bytes()),
            Ok(None)
        );
        let zero = BODY.replace(&"a".repeat(40), &"0".repeat(40));
        assert_eq!(
            verify_github(SECRET, &gh(SECRET, &zero, "push", Some("d1")), zero.as_bytes()),
            Ok(None)
        );
        assert!(matches!(
            verify_github(SECRET, &gh(SECRET, BODY, "push", None), BODY.as_bytes()),
            Err(Reject::Malformed(_))
        ));
        assert!(matches!(
            verify_github(SECRET, &gh(SECRET, BODY, "push", Some("has space")), BODY.as_bytes()),
            Err(Reject::Malformed(_))
        ));
    }

    const GL: &str = r#"{"ref":"refs/heads/main","after":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","checkout_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","project":{"git_http_url":"https://gitlab.com/acme/shop.git"}}"#;

    fn gl(token: &str, event: &str, uuid: Option<&str>) -> Headers {
        let mut h = vec![
            ("X-Gitlab-Token".to_string(), token.to_string()),
            ("X-Gitlab-Event".to_string(), event.to_string()),
        ];
        if let Some(u) = uuid {
            h.push(("X-Gitlab-Event-UUID".to_string(), u.to_string()));
        }
        Headers(h)
    }

    #[test]
    fn a_gitlab_push_is_verified_by_token_and_gets_a_stable_delivery_id() {
        let e = verify_gitlab(b"tok", &gl("tok", "Push Hook", Some("uuid-1")), GL.as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(
            (e.provider, e.delivery.as_str(), e.revision.as_str()),
            (Provider::GitLab, "uuid-1", "b".repeat(40).as_str())
        );
        // without the (unconfirmed) UUID header the id is derived from the commit, so a replay is one delivery
        let a = verify_gitlab(b"tok", &gl("tok", "Push Hook", None), GL.as_bytes())
            .unwrap()
            .unwrap();
        let b = verify_gitlab(b"tok", &gl("tok", "Push Hook", None), GL.as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(a.delivery, format!("gitlab:{}", "b".repeat(40)));
        assert_eq!(a, b);
        assert_eq!(
            verify_gitlab(b"tok", &gl("wrong", "Push Hook", None), GL.as_bytes()),
            Err(Reject::BadSignature)
        );
        assert_eq!(
            verify_gitlab(b"tok", &gl("toK", "Push Hook", None), GL.as_bytes()),
            Err(Reject::BadSignature)
        );
        assert_eq!(
            verify_gitlab(b"tok", &Headers(vec![]), GL.as_bytes()),
            Err(Reject::BadSignature)
        );
        assert_eq!(
            verify_gitlab(b"tok", &gl("tok", "Tag Push Hook", None), GL.as_bytes()),
            Ok(None)
        );
    }

    #[test]
    fn requests_are_bounded_before_they_are_read() {
        let ok = format!(
            "POST /webhook/github/shop HTTP/1.1\r\nContent-Length: {}\r\nX-A: b\r\n\r\n{BODY}",
            BODY.len()
        );
        let r = read_request(&mut ok.as_bytes()).unwrap();
        assert_eq!(
            (r.method.as_str(), r.path.as_str(), r.body.len()),
            ("POST", "/webhook/github/shop", BODY.len())
        );
        let huge = format!("POST /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert_eq!(read_request(&mut huge.as_bytes()).err(), Some(Reject::TooLarge));
        let long_head = format!("POST /x HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD + 100));
        assert_eq!(read_request(&mut long_head.as_bytes()).err(), Some(Reject::TooLarge));
        for bad in [
            "garbage\r\n\r\n",
            "POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            "POST /x HTTP/1.1\r\nContent-Length: nope\r\n\r\n",
            "POST x HTTP/1.1\r\n\r\n",
            "POST /x HTTP/1.1\r\nContent-Length: 10\r\n\r\nshort",
        ] {
            assert!(
                matches!(read_request(&mut bad.as_bytes()), Err(Reject::Malformed(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn routing_needs_a_registered_repository_the_right_provider_and_post() {
        let mut routes = Routes::new();
        routes.add("shop", Provider::GitHub, SECRET.to_vec());
        let req = |method: &str, path: &str| Request {
            method: method.into(),
            path: path.into(),
            headers: gh(SECRET, BODY, "push", Some("d1")),
            body: BODY.as_bytes().to_vec(),
        };
        assert!(handle(&routes, &req("POST", "/webhook/github/shop")).unwrap().is_some());
        assert_eq!(
            handle(&routes, &req("GET", "/webhook/github/shop")).err(),
            Some(Reject::MethodNotAllowed)
        );
        assert_eq!(
            handle(&routes, &req("POST", "/webhook/github/other")).err(),
            Some(Reject::NotFound)
        );
        assert_eq!(
            handle(&routes, &req("POST", "/webhook/gitlab/shop")).err(),
            Some(Reject::NotFound)
        );
        assert_eq!(handle(&routes, &req("POST", "/x")).err(), Some(Reject::NotFound));
    }

    #[test]
    fn the_listener_is_loopback_only_and_forwards_only_verified_pushes() {
        let l = Listener::bind_loopback(0).unwrap();
        assert!(l.local_addr().ip().is_loopback());
        let addr = l.local_addr();
        let mut routes = Routes::new();
        routes.add("shop", Provider::GitHub, SECRET.to_vec());
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let t = std::thread::spawn(move || l.serve(&routes, &tx, &flag));
        let post = |sig_secret: &[u8], body: &str, delivery: &str| -> String {
            let mut c = TcpStream::connect(addr).unwrap();
            let sig = to_hex(&hmac_sha256(sig_secret, body.as_bytes()));
            write!(c, "POST /webhook/github/shop HTTP/1.1\r\nX-Hub-Signature-256: sha256={sig}\r\nX-GitHub-Event: push\r\nX-GitHub-Delivery: {delivery}\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            let mut out = String::new();
            c.read_to_string(&mut out).unwrap();
            out.lines().next().unwrap_or("").to_string()
        };
        assert!(post(SECRET, BODY, "d1").contains("202"));
        assert!(post(b"wrong", BODY, "d2").contains("401"));
        assert!(
            post(SECRET, &BODY.replace("main", "evil"), "d3").contains("202"),
            "a correctly re-signed different body is a different, valid push"
        );
        // the same delivery again is accepted by the listener and deduplicated by the queue, not here
        assert!(post(SECRET, BODY, "d1").contains("202"));
        stop.store(true, Ordering::SeqCst);
        t.join().unwrap();
        let got: Vec<Inbound> = rx.try_iter().collect();
        assert_eq!(got.len(), 3, "the unverified request never reached the queue");
        assert!(got.iter().all(|i| i.repo == "shop"));
        assert_eq!(got.iter().filter(|i| i.event.delivery == "d1").count(), 2);
    }
}
