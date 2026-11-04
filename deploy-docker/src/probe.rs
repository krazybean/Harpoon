//! Real health probing of a published port: an HTTP GET over loopback TCP, and the adaptor that
//! feeds `harpoon_deploy::health::run_health`. Evidence built from this is never `simulated`.

use crate::api::{ContainerInfo, DockerApi};
use harpoon_deploy::health::{Attempt, Observation, Probe, Waiter};
use std::cell::{Cell, RefCell};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// What a response must carry to count as coming from *this* deployment's application. A bare
/// "HTTP 200 on the port" cannot tell the application from any other listener that happens to
/// hold the same host port; a marker the deployed application emits (and nothing else on the
/// host does) can. The application has to cooperate: Harpoon cannot inject one into an
/// arbitrary image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseMarker {
    /// A response header with exactly this value (name matched case-insensitively).
    Header { name: String, value: String },
    /// The first 16 KiB of the body contain this text. Chunked framing is not decoded, so use a
    /// marker short enough not to straddle a chunk boundary, or prefer a header.
    BodyContains(String),
}

impl ResponseMarker {
    pub fn header(name: &str, value: &str) -> Result<ResponseMarker, String> {
        if name.trim().is_empty() || value.trim().is_empty() {
            return Err("a marker needs a non-empty name and value".into());
        }
        Ok(ResponseMarker::Header {
            name: name.trim().to_string(),
            value: value.trim().to_string(),
        })
    }
    pub fn body_contains(text: &str) -> Result<ResponseMarker, String> {
        if text.is_empty() {
            return Err("a marker needs non-empty text".into());
        }
        Ok(ResponseMarker::BodyContains(text.to_string()))
    }
    fn describe(&self) -> String {
        match self {
            ResponseMarker::Header { name, .. } => format!("header {name}"),
            ResponseMarker::BodyContains(_) => "body marker".to_string(),
        }
    }
    fn needs_body(&self) -> bool {
        matches!(self, ResponseMarker::BodyContains(_))
    }
}

/// What one response said about who sent it, judged against a header marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkedResponse {
    /// The response carried exactly this container's marker.
    Matched(u16),
    /// The response carried no marker header at all (an ordinary application). Whether it may count is for the caller to
    /// decide from proof about the path, never from the response.
    Unmarked(u16),
    /// A failure, including a marker header with another value (a stale or foreign marker-aware listener).
    Failed(Observation),
}

/// One HTTP request against `127.0.0.1:<port>`.
pub trait HttpTransport {
    fn get(&self, port: u16, path: &str, timeout: Duration) -> Observation;
    /// Like [`get`](HttpTransport::get), but a response only counts if it carries `marker`. A
    /// transport that cannot check a marker must fail closed, which is what the default does.
    fn get_expecting(
        &self,
        port: u16,
        path: &str,
        timeout: Duration,
        marker: Option<&ResponseMarker>,
    ) -> Observation {
        match marker {
            None => self.get(port, path, timeout),
            Some(_) => Observation::ConnectFailed(
                "this transport cannot verify a response marker, so no response is accepted".into(),
            ),
        }
    }
    /// Like [`get_expecting`](HttpTransport::get_expecting) with a header marker, but a response with no marker header is
    /// reported as [`MarkedResponse::Unmarked`] instead of a failure. A transport that cannot tell the two apart never
    /// reports `Unmarked`, so it can only ever accept a response that carries the marker.
    fn get_marked(
        &self,
        port: u16,
        path: &str,
        timeout: Duration,
        marker: &ResponseMarker,
    ) -> MarkedResponse {
        match self.get_expecting(port, path, timeout, Some(marker)) {
            Observation::Status(s) => MarkedResponse::Matched(s),
            other => MarkedResponse::Failed(other),
        }
    }
    /// True only for test doubles. A backend built on a simulated transport reports simulated
    /// evidence, which the engine refuses unless the backend says it accepts it.
    fn is_simulated(&self) -> bool {
        false
    }
}

pub struct TcpHttp;

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 16 * 1024;

impl TcpHttp {
    /// One request with a single deadline for the whole exchange: a server that answers one byte
    /// at a time cannot stretch it past `timeout`. Redirects are never followed.
    fn fetch(
        port: u16,
        path: &str,
        timeout: Duration,
        want_head: bool,
        want_body: bool,
    ) -> Result<Response, String> {
        let deadline = Instant::now() + timeout;
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut s = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| format!("connect to 127.0.0.1:{port}: {e}"))?;
        let left = |d: Instant| {
            d.checked_duration_since(Instant::now())
                .filter(|l| !l.is_zero())
                .ok_or_else(|| "timed out".to_string())
        };
        let _ = s.set_write_timeout(Some(left(deadline)?));
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: harpoon-health\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes())
            .map_err(|e| format!("send: {e}"))?;
        let mut got: Vec<u8> = Vec::new();
        let mut buf = [0u8; 2048];
        let head_end = loop {
            if let Some(i) = find(&got, b"\r\n\r\n") {
                break Some(i);
            }
            // Without a marker only the status line matters.
            if !want_head && got.contains(&b'\n') && got.len() >= 12 {
                break None;
            }
            if got.len() > MAX_HEAD {
                return Err("response headers are too large".into());
            }
            let _ = s.set_read_timeout(Some(left(deadline)?));
            match s.read(&mut buf) {
                Ok(0) => break find(&got, b"\r\n\r\n"),
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return Err("timed out".into())
                }
                Err(e) => return Err(format!("read: {e}")),
            }
        };
        if want_head && head_end.is_none() {
            return Err("the response ended before its headers were complete".into());
        }
        let text_end = head_end.unwrap_or(got.len());
        let head = String::from_utf8_lossy(&got[..text_end]).into_owned();
        let mut lines = head.lines();
        let first = lines.next().unwrap_or("");
        let mut it = first.split_whitespace();
        let status = match (it.next(), it.next().and_then(|c| c.parse::<u16>().ok())) {
            (Some(v), Some(code)) if v.starts_with("HTTP/1.") && (100..600).contains(&code) => code,
            _ => return Err(format!("not an HTTP response: {first:?}")),
        };
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        let mut body = Vec::new();
        if let (true, Some(i)) = (want_body, head_end) {
            body.extend_from_slice(&got[i + 4..]);
            while body.len() < MAX_BODY {
                let _ = s.set_read_timeout(Some(left(deadline)?));
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => body.extend_from_slice(&buf[..n]),
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        return Err("timed out".into())
                    }
                    Err(e) => return Err(format!("read: {e}")),
                }
            }
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl HttpTransport for TcpHttp {
    fn get(&self, port: u16, path: &str, timeout: Duration) -> Observation {
        match TcpHttp::fetch(port, path, timeout, false, false) {
            Ok(r) => Observation::Status(r.status),
            Err(e) => Observation::ConnectFailed(e),
        }
    }

    fn get_marked(
        &self,
        port: u16,
        path: &str,
        timeout: Duration,
        marker: &ResponseMarker,
    ) -> MarkedResponse {
        let ResponseMarker::Header { name, value } = marker else {
            return match self.get_expecting(port, path, timeout, Some(marker)) {
                Observation::Status(s) => MarkedResponse::Matched(s),
                other => MarkedResponse::Failed(other),
            };
        };
        let r = match TcpHttp::fetch(port, path, timeout, true, false) {
            Ok(r) => r,
            Err(e) => return MarkedResponse::Failed(Observation::ConnectFailed(e)),
        };
        let seen: Vec<&str> = r
            .headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect();
        if seen.is_empty() {
            MarkedResponse::Unmarked(r.status)
        } else if !value.is_empty() && seen.iter().all(|v| *v == value) {
            MarkedResponse::Matched(r.status)
        } else {
            MarkedResponse::Failed(Observation::ConnectFailed(format!(
                "HTTP {} response carried a different {name} than this container's; it comes from another listener or a stale one",
                r.status
            )))
        }
    }

    fn get_expecting(
        &self,
        port: u16,
        path: &str,
        timeout: Duration,
        marker: Option<&ResponseMarker>,
    ) -> Observation {
        let Some(m) = marker else {
            return self.get(port, path, timeout);
        };
        let r = match TcpHttp::fetch(port, path, timeout, true, m.needs_body()) {
            Ok(r) => r,
            Err(e) => return Observation::ConnectFailed(e),
        };
        let found = match m {
            ResponseMarker::Header { name, value } => {
                !value.is_empty()
                    && r.headers
                        .iter()
                        .any(|(n, v)| n.eq_ignore_ascii_case(name) && v == value)
            }
            ResponseMarker::BodyContains(t) => {
                !t.is_empty() && find(&r.body, t.as_bytes()).is_some()
            }
        };
        if found {
            Observation::Status(r.status)
        } else {
            // A response without the marker is *not* a success, whatever its status.
            Observation::ConnectFailed(format!(
                "HTTP {} response did not carry this container's deployment marker ({}), so Harpoon cannot prove it came from the container it started (it may come from a different listener). Update and restart the Harpoon engine (it must publish the guest address it discovered), or have the application echo that header with the value of its HARPOON_DEPLOYMENT_MARKER environment variable",
                r.status,
                m.describe()
            ))
        }
    }
}

/// Is something listening on this port on the host? Used for port allocation.
pub trait PortProbe {
    fn is_listening(&self, port: u16) -> bool;
}

/// Tries to bind the port on the wildcard and on loopback; a failure to bind means taken.
pub struct BindProbe;

impl PortProbe for BindProbe {
    fn is_listening(&self, port: u16) -> bool {
        TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).is_err()
            || TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err()
    }
}

pub struct SleepWaiter;

impl Waiter for SleepWaiter {
    fn wait_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// What must stay the same, for the whole check, about the container being probed. A response is only attributed to a
/// container that is the same one, started once, from the same image, from the first attempt to the last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerBaseline {
    pub id: String,
    pub image_id: String,
    pub started_at: String,
    pub restart_count: u64,
}

impl ContainerBaseline {
    /// `None` unless every identity fact is known: a fact Docker did not report cannot be compared, so it cannot attribute.
    pub fn of(c: &ContainerInfo) -> Option<ContainerBaseline> {
        (c.running && !c.id.is_empty() && !c.image_id.is_empty() && !c.started_at.is_empty()).then(
            || ContainerBaseline {
                id: c.id.clone(),
                image_id: c.image_id.clone(),
                started_at: c.started_at.clone(),
                restart_count: c.restart_count,
            },
        )
    }

    /// Why `c` is not the container this baseline was taken from, if it is not.
    pub fn differs(&self, c: &ContainerInfo) -> Option<String> {
        if c.id != self.id {
            Some("the container was replaced while it was being checked".to_string())
        } else if c.image_id != self.image_id {
            Some("the container's image changed while it was being checked".to_string())
        } else if c.started_at != self.started_at || c.restart_count != self.restart_count {
            Some("the container restarted while it was being checked".to_string())
        } else {
            None
        }
    }
}

/// Decides whether a 2xx response that carried no marker may count. `Ok` only when the path from this host to the
/// container has been proven; `Err` is the reason, in words fit for the person deploying.
pub trait UnmarkedGate {
    fn accept(&self, port: u16) -> Result<(), String>;
}

/// One attempt = "is the container still running?" then one HTTP request.
pub struct ContainerProbe<'a> {
    pub api: &'a DockerApi,
    pub container_id: &'a str,
    pub port: u16,
    pub path: &'a str,
    pub transport: &'a dyn HttpTransport,
    pub marker: Option<&'a ResponseMarker>,
    pub attempt_timeout: Duration,
    /// When set, a 2xx response without the marker header may count if the gate proves the path.
    pub unmarked: Option<&'a dyn UnmarkedGate>,
    /// Set when an unmarked response was accepted: the evidence is then `path-verified`, not `marker-verified`.
    pub used_unmarked: &'a Cell<bool>,
    /// The container's identity, taken at the first attempt that finds it running (only when `unmarked` is set); any
    /// later difference ends the check as a failure.
    pub baseline: &'a RefCell<Option<ContainerBaseline>>,
    /// Why the container changed, if it did.
    pub changed: &'a RefCell<Option<String>>,
}

impl Probe for ContainerProbe<'_> {
    fn attempt(&mut self, at_ms: u64) -> Attempt {
        let t = self.attempt_timeout;
        match self.api.container_inspect(self.container_id, t) {
            Ok(Some(c)) if c.running => {
                if self.unmarked.is_some() {
                    let mut base = self.baseline.borrow_mut();
                    match base.as_ref() {
                        Some(b) => {
                            if let Some(why) = b.differs(&c) {
                                *self.changed.borrow_mut() = Some(why.clone());
                                return Attempt {
                                    at_ms,
                                    observation: Observation::ConnectFailed(why),
                                    container_running: true,
                                };
                            }
                        }
                        None => *base = ContainerBaseline::of(&c),
                    }
                }
                let observation = match (self.marker, self.unmarked) {
                    (Some(m), Some(gate)) => {
                        match self.transport.get_marked(self.port, self.path, t, m) {
                            MarkedResponse::Matched(s) => Observation::Status(s),
                            MarkedResponse::Unmarked(s) if (200..300).contains(&s) => {
                                if self.baseline.borrow().is_none() {
                                    Observation::ConnectFailed("Docker did not report this container's image and start time, so a response without the deployment marker cannot be attributed to it".into())
                                } else {
                                    match gate.accept(self.port) {
                                        Ok(()) => {
                                            self.used_unmarked.set(true);
                                            Observation::Status(s)
                                        }
                                        Err(why) => Observation::ConnectFailed(why),
                                    }
                                }
                            }
                            MarkedResponse::Unmarked(s) => Observation::Status(s),
                            MarkedResponse::Failed(o) => o,
                        }
                    }
                    _ => self
                        .transport
                        .get_expecting(self.port, self.path, t, self.marker),
                };
                Attempt {
                    at_ms,
                    observation,
                    container_running: true,
                }
            }
            Ok(Some(c)) => Attempt {
                at_ms,
                observation: Observation::ContainerExited(c.exit_code),
                container_running: false,
            },
            Ok(None) => Attempt {
                at_ms,
                observation: Observation::ContainerExited(None),
                container_running: false,
            },
            // Cannot tell: record the failure; it can never count as a success.
            Err(e) => Attempt {
                at_ms,
                observation: Observation::ConnectFailed(format!(
                    "could not inspect the container: {e}"
                )),
                container_running: true,
            },
        }
    }
}
