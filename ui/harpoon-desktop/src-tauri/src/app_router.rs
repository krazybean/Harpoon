//! The application router: `http://<label>.harpoon.localhost:<port>` -> the host port of the release
//! that application serves now.
//!
//! Deliberately small: a TCP-level forwarder that reads the first request head, picks the upstream from
//! its `Host` header and then pipes bytes both ways (so keep-alive, streaming and WebSockets just work).
//! It binds **loopback only** and forwards to `127.0.0.1:<port>` only, where the port comes from the
//! route table (the durable serving release), never from the request. Unknown hosts are not forwarded,
//! so an IP, another domain or a DNS-rebound name reaches nothing.
//!
//! The table is swapped whole, so a route moves exactly when the recorded serving release does. When a
//! connection to the recorded port fails the table is re-read once (a cutover may have just moved it)
//! before answering 503; an application whose container is stopped is reported as such, never as
//! someone else's page.

use harpoon_deploy::route::{Route, RouteTable};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The port the router listens on. `*.localhost` resolves to loopback without any system change, but
/// an unprivileged process cannot bind loopback port 80 on macOS, so the URL carries this port.
pub const DEFAULT_PORT: u16 = 7480;

const HEAD_LIMIT: usize = 16 * 1024;
const TABLE_TTL: Duration = Duration::from_secs(1);
const FORCED_REFRESH_GAP: Duration = Duration::from_millis(250);

static STATUS: Mutex<Option<Result<u16, String>>> = Mutex::new(None);

/// Where the router listens, or why it is not running. `(None, None)` until startup has been tried.
pub fn status() -> (Option<u16>, Option<String>) {
    match &*STATUS.lock().unwrap_or_else(|e| e.into_inner()) {
        Some(Ok(p)) => (Some(*p), None),
        Some(Err(e)) => (None, Some(e.clone())),
        None => (None, None),
    }
}

/// Start the default router (loopback, [`DEFAULT_PORT`]) over the desktop's own journal and record the outcome.
pub fn start_default(refresh: Refresh) {
    let r = start(SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT)), refresh)
        .map(|r| r.addr().port())
        .map_err(|e| format!("cannot listen on 127.0.0.1:{DEFAULT_PORT}: {e}"));
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
}

/// Only Harpoon's own application addresses may be handed to the system opener: `http://<label>.harpoon.localhost:<port>`
/// or the direct `http://127.0.0.1:<port>`; no path, credentials or other host.
pub fn openable(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let Some((host, port)) = rest.trim_end_matches('/').split_once(':') else { return false };
    let label = host.strip_suffix(&format!(".{}", harpoon_deploy::route::DOMAIN));
    let host_ok = host == "127.0.0.1" || label.is_some_and(|l| (1..=32).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
    host_ok && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p >= 1024)
}

pub type Refresh = Box<dyn Fn() -> Result<RouteTable, String> + Send + Sync>;

struct State {
    table: Mutex<(Instant, Arc<RouteTable>)>,
    refresh: Refresh,
}

impl State {
    /// The current table; re-read from the records when it is older than the TTL (or `force`d, but not
    /// more often than the minimum gap). A failed read keeps the previous table: a damaged or busy
    /// store must not blank every route.
    fn table(&self, force: bool) -> Arc<RouteTable> {
        let mut t = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let age = t.0.elapsed();
        if age >= TABLE_TTL || (force && age >= FORCED_REFRESH_GAP) {
            if let Ok(fresh) = (self.refresh)() {
                t.1 = Arc::new(fresh);
            }
            t.0 = Instant::now();
        }
        t.1.clone()
    }
}

pub struct Router {
    addr: SocketAddr,
}

impl Router {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

/// Start listening. Refuses any non-loopback address.
pub fn start(bind: SocketAddr, refresh: Refresh) -> io::Result<Router> {
    if !bind.ip().is_loopback() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the application router binds loopback only"));
    }
    let listener = TcpListener::bind(bind)?;
    let addr = listener.local_addr()?;
    let initial = refresh().unwrap_or_default();
    let state = Arc::new(State { table: Mutex::new((Instant::now(), Arc::new(initial))), refresh });
    std::thread::Builder::new().name("harpoon-app-router".into()).spawn(move || {
        for conn in listener.incoming().flatten() {
            let state = state.clone();
            let _ = std::thread::Builder::new().name("harpoon-app-router-conn".into()).spawn(move || {
                let _ = serve(conn, &state);
            });
        }
    })?;
    Ok(Router { addr })
}

fn respond(mut c: TcpStream, status: &str, message: &str) -> io::Result<()> {
    let body = format!("<!doctype html><meta charset=utf-8><title>Harpoon</title><body style=\"font:16px system-ui;margin:3rem auto;max-width:34rem\"><h1>{status}</h1><p>{message}</p>");
    write!(c, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}", body.len())
}

/// The `Host` header of a request head.
fn host_of(head: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(head).ok()?;
    text.split("\r\n").skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("host").then(|| v.trim().to_string())
    })
}

fn read_head(c: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        if head.len() > HEAD_LIMIT {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "request head too large"));
        }
        let n = c.read(&mut buf)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.extend_from_slice(&buf[..n]);
    }
    Ok(head)
}

fn connect(port: u16) -> io::Result<TcpStream> {
    TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(2))
}

fn serve(mut client: TcpStream, state: &State) -> io::Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(10)))?;
    let head = read_head(&mut client)?;
    client.set_read_timeout(None)?;
    let host = host_of(&head).unwrap_or_default();

    let mut table = state.table(false);
    let mut tried: Option<u16> = None;
    let upstream = loop {
        match table.resolve(&host) {
            None => return respond(client, "404 Not Found", "This is not a Harpoon application address."),
            Some(Route::Conflict { .. }) => {
                return respond(client, "409 Conflict", "Two applications were assigned this address. Harpoon refuses to guess; neither is routed.")
            }
            Some(Route::Idle { .. }) => {
                return respond(client, "503 Service Unavailable", "This application has no running release recorded. Deploy it, or restart it from Harpoon.")
            }
            Some(Route::Serving { port, .. }) => match connect(*port) {
                Ok(u) => break u,
                // Once: the release may have just moved. Same port again means it is really down.
                Err(_) if tried.is_none() => {
                    tried = Some(*port);
                    table = state.table(true);
                    if matches!(table.resolve(&host), Some(Route::Serving { port: p, .. }) if Some(*p) == tried) {
                        return respond(client, "503 Service Unavailable", "The release recorded as serving this application is not responding. It may be stopped or restarting; Harpoon has not routed to anything else.");
                    }
                }
                Err(_) => {
                    return respond(client, "503 Service Unavailable", "The release recorded as serving this application is not responding. It may be stopped or restarting; Harpoon has not routed to anything else.")
                }
            },
        }
    };

    let mut up_w = upstream.try_clone()?;
    up_w.write_all(&head)?;
    let (mut up_r, mut cl_w) = (upstream, client.try_clone()?);
    let back = std::thread::spawn(move || {
        let _ = io::copy(&mut up_r, &mut cl_w);
        let _ = cl_w.shutdown(Shutdown::Both);
    });
    let _ = io::copy(&mut client, &mut up_w);
    let _ = up_w.shutdown(Shutdown::Write);
    let _ = back.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harpoon_deploy::ids::ApplicationId;
    use harpoon_deploy::route::hostname;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn app(tail: &str) -> ApplicationId {
        format!("app_{:0>26}", tail).parse().unwrap()
    }

    /// A tiny upstream that answers every request with `body` and counts the connections it saw.
    fn upstream(body: &'static str) -> (u16, Arc<AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                h.fetch_add(1, Ordering::SeqCst);
                let _ = read_head(&mut c);
                let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        (port, hits)
    }

    fn dead_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn get(router: &Router, host: &str) -> String {
        let mut c = TcpStream::connect(router.addr()).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(c, "GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        out
    }

    type Shared = Arc<Mutex<RouteTable>>;
    fn router_for(t: RouteTable) -> (Router, Shared) {
        let shared: Shared = Arc::new(Mutex::new(t));
        let s = shared.clone();
        let r = start("127.0.0.1:0".parse().unwrap(), Box::new(move || Ok(s.lock().unwrap().clone()))).unwrap();
        (r, shared)
    }

    /// Wait out the table TTL so the router re-reads the (test-controlled) records.
    fn settle() {
        std::thread::sleep(TABLE_TTL + Duration::from_millis(100));
    }

    #[test]
    fn only_harpoon_addresses_may_be_opened() {
        for ok in ["http://abc124xyz0.harpoon.localhost:7480", "http://abc124xyz0.harpoon.localhost:7480/", "http://127.0.0.1:38001"] {
            assert!(openable(ok), "{ok}");
        }
        for bad in ["https://abc.harpoon.localhost:7480", "http://evil.test:7480", "http://abc.harpoon.localhost.evil.test:7480", "http://abc.harpoon.localhost:80", "http://abc.harpoon.localhost", "http://u@abc.harpoon.localhost:7480", "http://abc.harpoon.localhost:7480/x", "file:///etc/passwd", "http://.harpoon.localhost:7480", "http://A.harpoon.localhost:7480", ""] {
            assert!(!openable(bad), "{bad}");
        }
    }

    #[test]
    fn it_binds_loopback_only() {
        for addr in ["0.0.0.0:0", "192.168.1.5:0", "[::]:0"] {
            let e = start(addr.parse().unwrap(), Box::new(|| Ok(RouteTable::default()))).err().expect(addr);
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{addr}");
        }
        let (r, _) = router_for(RouteTable::default());
        assert!(r.addr().ip().is_loopback());
        assert_eq!(DEFAULT_PORT, 7480);
    }

    #[test]
    fn it_routes_by_hostname_to_exactly_that_applications_release() {
        let (pa, ha) = upstream("page-a");
        let (pb, hb) = upstream("page-b");
        let (a, b) = (app("1"), app("2"));
        let (r, _) = router_for(RouteTable::build([(a.clone(), Some(pa)), (b.clone(), Some(pb))]));
        assert!(get(&r, &format!("{}:7480", hostname(&a))).ends_with("page-a"));
        assert!(get(&r, &hostname(&b)).ends_with("page-b"));
        assert_eq!((ha.load(Ordering::SeqCst), hb.load(Ordering::SeqCst)), (1, 1), "one request each; nothing crossed over");
    }

    #[test]
    fn the_same_hostname_follows_the_serving_release_when_the_table_is_swapped() {
        let (p1, _) = upstream("release-1");
        let (p2, _) = upstream("release-2");
        let a = app("1");
        let (r, shared) = router_for(RouteTable::build([(a.clone(), Some(p1))]));
        assert!(get(&r, &hostname(&a)).ends_with("release-1"));
        *shared.lock().unwrap() = RouteTable::build([(a.clone(), Some(p2))]);
        settle();
        assert!(get(&r, &hostname(&a)).ends_with("release-2"));
    }

    #[test]
    fn a_cutover_between_refreshes_is_followed_without_waiting_for_the_ttl() {
        let (p2, _) = upstream("release-2");
        let a = app("1");
        // The router still holds the old (now stopped) port; the records already say p2.
        let (r, shared) = router_for(RouteTable::build([(a.clone(), Some(dead_port()))]));
        *shared.lock().unwrap() = RouteTable::build([(a.clone(), Some(p2))]);
        // Past the forced-refresh gap but well inside the TTL: only the failed connect can notice.
        std::thread::sleep(FORCED_REFRESH_GAP + Duration::from_millis(100));
        assert!(get(&r, &hostname(&a)).ends_with("release-2"));
    }

    #[test]
    fn a_stopped_release_is_reported_not_rerouted() {
        let (other, other_hits) = upstream("someone-else");
        let (a, b) = (app("1"), app("2"));
        let (r, _) = router_for(RouteTable::build([(a.clone(), Some(dead_port())), (b.clone(), Some(other))]));
        let out = get(&r, &hostname(&a));
        assert!(out.starts_with("HTTP/1.1 503"), "{out}");
        assert!(out.contains("not responding"), "{out}");
        assert_eq!(other_hits.load(Ordering::SeqCst), 0, "no unrelated application was contacted");
    }

    #[test]
    fn idle_conflicting_and_unknown_hosts_are_refused_without_forwarding() {
        let (p, hits) = upstream("secret");
        let (idle, ok) = (app("1"), app("4"));
        let (r, _) = router_for(RouteTable::build([(idle.clone(), None), (ok.clone(), Some(p))]));
        assert!(get(&r, &hostname(&idle)).starts_with("HTTP/1.1 503"));
        for bad in ["127.0.0.1:7480", "localhost", "harpoon.localhost", &format!("{}.evil.test", hostname(&ok)), "", &format!("x.{}", hostname(&ok))] {
            assert!(get(&r, bad).starts_with("HTTP/1.1 404"), "{bad:?}");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing was forwarded");
    }

    #[test]
    fn a_hostname_collision_answers_409_and_forwards_nothing() {
        let (p, hits) = upstream("secret");
        let a: ApplicationId = "app_0000000000AAAAAAAAAAAAAAAA".parse().unwrap();
        let b: ApplicationId = "app_1111111111AAAAAAAAAAAAAAAA".parse().unwrap();
        let (r, _) = router_for(RouteTable::build([(a.clone(), Some(p)), (b, Some(p))]));
        assert!(get(&r, &hostname(&a)).starts_with("HTTP/1.1 409"));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_good_table() {
        let (p, _) = upstream("still-here");
        let a = app("1");
        let table = RouteTable::build([(a.clone(), Some(p))]);
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let r = start("127.0.0.1:0".parse().unwrap(), Box::new(move || {
            if c.fetch_add(1, Ordering::SeqCst) == 0 { Ok(table.clone()) } else { Err("store busy".into()) }
        })).unwrap();
        settle();
        assert!(get(&r, &hostname(&a)).ends_with("still-here"));
        assert!(calls.load(Ordering::SeqCst) >= 2, "a refresh was attempted and failed");
    }

    /// The whole path through the operating system's resolver: no hosts-file entry, no DNS setting.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_system_resolver_sends_a_harpoon_localhost_name_to_the_router() {
        let (p, _) = upstream("via-resolver");
        let a = app("1");
        let (r, _) = router_for(RouteTable::build([(a.clone(), Some(p))]));
        let mut c = TcpStream::connect((hostname(&a).as_str(), r.addr().port())).expect("resolve and connect");
        assert!(c.peer_addr().unwrap().ip().is_loopback());
        write!(c, "GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", hostname(&a)).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(out.ends_with("via-resolver"), "{out}");
    }

    #[test]
    fn streams_bytes_in_both_directions() {
        // An echo upstream: what the client sends after the head comes back (WebSocket-like traffic).
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                let _ = read_head(&mut c);
                let mut buf = [0u8; 64];
                while let Ok(n) = c.read(&mut buf) {
                    if n == 0 || c.write_all(&buf[..n]).is_err() { break; }
                }
            }
        });
        let a = app("1");
        let (r, _) = router_for(RouteTable::build([(a.clone(), Some(port))]));
        let mut c = TcpStream::connect(r.addr()).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(c, "GET / HTTP/1.1\r\nHost: {}\r\n\r\n", hostname(&a)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        c.write_all(b"ping").unwrap();
        let mut got = [0u8; 4];
        c.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"ping");
    }
}
