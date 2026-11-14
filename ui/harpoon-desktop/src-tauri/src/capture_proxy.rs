//! A loopback-only, anonymous, GET-only relay for the private preview window.
//!
//! WebKit applies `upgrade-insecure-requests` to an app's own http subresources even on loopback, so an app that sends
//! that directive cannot load its scripts and styles in a WKWebView (Chrome exempts localhost). The preview window
//! therefore loads such an app through this `hpcapture://` scheme instead: each request is fetched over plain http from
//! the same loopback host, and only that one directive is removed from the response's Content-Security-Policy, so the
//! window never tries to upgrade. Nothing else about the app's policy changes, nothing here is served to any other
//! client (ordinary visitors never touch this path), and the relay never forwards cookies or credentials, never follows
//! a redirect off the app's own host, and answers nothing but GET and HEAD for loopback hosts.

use std::borrow::Cow;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub const SCHEME: &str = "hpcapture";
const MAX_BODY: usize = 32 * 1024 * 1024;
const TOTAL: Duration = Duration::from_secs(20);

pub fn handle(request: tauri::http::Request<Vec<u8>>, responder: tauri::UriSchemeResponder) {
    // a blocking fetch must never run on the main thread that serves the web view
    std::thread::spawn(move || responder.respond(serve(&request)));
}

fn status(code: u16, why: &str) -> tauri::http::Response<Cow<'static, [u8]>> {
    tauri::http::Response::builder().status(code).header("content-type", "text/plain; charset=utf-8").body(Cow::Owned(why.as_bytes().to_vec())).expect("static response")
}

fn serve(req: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Cow<'static, [u8]>> {
    let head = req.method() == tauri::http::Method::HEAD;
    if req.method() != tauri::http::Method::GET && !head {
        return status(405, "the preview relay answers GET and HEAD only");
    }
    let uri = req.uri();
    let (Some(host), Some(port)) = (uri.host(), uri.port_u16()) else { return status(400, "the preview relay needs a host and port") };
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    // the same boundary as the capture itself: an app on this Mac, plain http, no credentials in the address
    if crate::preview::loopback_target(&format!("http://{host}:{port}{path}")).is_err() {
        return status(403, "the preview relay only reaches apps on this Mac");
    }
    let header = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let raw = match fetch(host, port, path, head, header("accept").as_deref(), header("user-agent").as_deref()) {
        Ok(r) => r,
        Err(e) => return status(502, &format!("the app did not answer: {e}")),
    };
    let Some(parsed) = parse_response(&raw) else { return status(502, "the app's answer could not be read") };
    let mut out = tauri::http::Response::builder().status(parsed.status);
    for (k, v) in sanitize(&parsed.headers) {
        if k == "location" {
            match relocate(&v, host, port) {
                Some(l) => out = out.header("location", l),
                None => return status(403, "the app redirected somewhere other than itself; the preview does not follow it"),
            }
        } else {
            out = out.header(k, v);
        }
    }
    out.body(Cow::Owned(parsed.body)).unwrap_or_else(|_| status(502, "the app's headers could not be relayed"))
}

fn fetch(host: &str, port: u16, path: &str, head: bool, accept: Option<&str>, agent: Option<&str>) -> Result<Vec<u8>, String> {
    // `*.localhost` is loopback by definition; the system resolver does not always say so
    let target = if host == "localhost" || host.ends_with(".localhost") { "127.0.0.1" } else { host };
    let addr = (target, port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let started = Instant::now();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let req = format!(
        "{} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: {}\r\nUser-Agent: {}\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        if head { "HEAD" } else { "GET" },
        accept.unwrap_or("*/*"),
        agent.unwrap_or("Harpoon-preview"),
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        if started.elapsed() > TOTAL || buf.len() > MAX_BODY {
            return Err("too slow or too large".into());
        }
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                if buf.is_empty() {
                    return Err("no answer".into());
                }
                break;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(buf)
}

pub(crate) struct Parsed {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// An HTTP/1.1 response read to the end of the connection: status, headers (names lower-cased) and the de-chunked body.
pub(crate) fn parse_response(raw: &[u8]) -> Option<Parsed> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))).collect();
    let rest = &raw[split + 4..];
    let chunked = headers.iter().any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let body = if chunked { dechunk(rest)? } else { rest.to_vec() };
    Some(Parsed { status, headers, body })
}

fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n")?;
        let size = usize::from_str_radix(std::str::from_utf8(&b[..eol]).ok()?.split(';').next()?.trim(), 16).ok()?;
        b = &b[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(b.get(..size)?);
        b = b.get(size + 2..)?;
    }
}

/// `value` of a Content-Security-Policy without `upgrade-insecure-requests`; every other directive is kept as sent.
pub(crate) fn strip_upgrade(value: &str) -> String {
    value.split(';').map(str::trim).filter(|d| !d.is_empty() && !d.eq_ignore_ascii_case("upgrade-insecure-requests")).collect::<Vec<_>>().join("; ")
}

/// The headers to relay: hop-by-hop and per-client headers dropped (cookies are never set in the private window),
/// framing left to the relay, and the one CSP directive removed.
pub(crate) fn sanitize(headers: &[(String, String)]) -> Vec<(String, String)> {
    const DROP: [&str; 8] = ["connection", "transfer-encoding", "keep-alive", "content-length", "content-encoding", "set-cookie", "strict-transport-security", "upgrade"];
    headers
        .iter()
        .filter(|(k, _)| !DROP.contains(&k.as_str()))
        .map(|(k, v)| if k == "content-security-policy" { (k.clone(), strip_upgrade(v)) } else { (k.clone(), v.clone()) })
        .collect()
}

/// A redirect target on the app's own host, as an `hpcapture` address; `None` for anywhere else.
pub(crate) fn relocate(location: &str, host: &str, port: u16) -> Option<String> {
    if location.starts_with('/') && !location.starts_with("//") {
        return Some(location.to_string());
    }
    let rest = location.strip_prefix(&format!("http://{host}:{port}")).or_else(|| location.strip_prefix(&format!("{SCHEME}://{host}:{port}")))?;
    (rest.is_empty() || rest.starts_with('/') || rest.starts_with('?')).then(|| format!("{SCHEME}://{host}:{port}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_upgrade_directive_leaves_the_policy() {
        assert_eq!(strip_upgrade("default-src 'self'; script-src 'self' 'unsafe-inline'; upgrade-insecure-requests"), "default-src 'self'; script-src 'self' 'unsafe-inline'");
        assert_eq!(strip_upgrade("upgrade-insecure-requests"), "");
        assert_eq!(strip_upgrade("img-src 'self' https://cms.juancastro.me; frame-ancestors 'none'"), "img-src 'self' https://cms.juancastro.me; frame-ancestors 'none'");
    }

    #[test]
    fn headers_are_relayed_without_cookies_or_framing_and_with_the_one_change() {
        let h = vec![
            ("content-security-policy".to_string(), "default-src 'self'; upgrade-insecure-requests".to_string()),
            ("set-cookie".to_string(), "a=b".to_string()),
            ("transfer-encoding".to_string(), "chunked".to_string()),
            ("content-type".to_string(), "text/html".to_string()),
        ];
        let out = sanitize(&h);
        assert_eq!(out, vec![("content-security-policy".to_string(), "default-src 'self'".to_string()), ("content-type".to_string(), "text/html".to_string())]);
    }

    #[test]
    fn a_chunked_and_a_plain_response_are_both_read() {
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nX-A: b\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let p = parse_response(chunked).unwrap();
        assert_eq!((p.status, p.body.as_slice()), (200, b"hello world".as_slice()));
        let plain = parse_response(b"HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\n\r\nnop").unwrap();
        assert_eq!((plain.status, plain.body.as_slice()), (404, b"nop".as_slice()));
        assert!(parse_response(b"garbage").is_none());
    }

    #[test]
    fn a_redirect_stays_on_the_apps_own_host() {
        assert_eq!(relocate("/about", "a.harpoon.localhost", 7480).as_deref(), Some("/about"));
        assert_eq!(relocate("http://a.harpoon.localhost:7480/x?y=1", "a.harpoon.localhost", 7480).as_deref(), Some("hpcapture://a.harpoon.localhost:7480/x?y=1"));
        for off in ["https://evil.example/", "//evil.example/", "http://a.harpoon.localhost:7480.evil.example/", "http://other.harpoon.localhost:7480/", "http://a.harpoon.localhost:9999/"] {
            assert!(relocate(off, "a.harpoon.localhost", 7480).is_none(), "{off}");
        }
    }
}
