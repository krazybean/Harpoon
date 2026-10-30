//! Real health probing of a published port: an HTTP GET over loopback TCP, and the adaptor that
//! feeds `harpoon_deploy::health::run_health`. Evidence built from this is never `simulated`.

use crate::api::DockerApi;
use harpoon_deploy::health::{Attempt, Observation, Probe, Waiter};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

/// One HTTP request against `127.0.0.1:<port>`.
pub trait HttpTransport {
    fn get(&self, port: u16, path: &str, timeout: Duration) -> Observation;
    /// True only for test doubles. A backend built on a simulated transport reports simulated
    /// evidence, which the engine refuses unless the backend says it accepts it.
    fn is_simulated(&self) -> bool {
        false
    }
}

pub struct TcpHttp;

impl HttpTransport for TcpHttp {
    fn get(&self, port: u16, path: &str, timeout: Duration) -> Observation {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut s = match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => s,
            Err(e) => {
                return Observation::ConnectFailed(format!("connect to 127.0.0.1:{port}: {e}"))
            }
        };
        let _ = s.set_read_timeout(Some(timeout));
        let _ = s.set_write_timeout(Some(timeout));
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: harpoon-health\r\nConnection: close\r\n\r\n");
        if let Err(e) = s.write_all(req.as_bytes()) {
            return Observation::ConnectFailed(format!("send: {e}"));
        }
        let mut buf = [0u8; 512];
        let mut got = Vec::new();
        while got.len() < 12 {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e) => return Observation::ConnectFailed(format!("read: {e}")),
            }
        }
        let line = String::from_utf8_lossy(&got);
        let line = line.lines().next().unwrap_or("");
        let mut it = line.split_whitespace();
        match (it.next(), it.next().and_then(|c| c.parse::<u16>().ok())) {
            (Some(v), Some(code)) if v.starts_with("HTTP/1.") && (100..600).contains(&code) => {
                Observation::Status(code)
            }
            _ => Observation::ConnectFailed(format!("not an HTTP response: {line:?}")),
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

/// One attempt = "is the container still running?" then one HTTP request.
pub struct ContainerProbe<'a> {
    pub api: &'a DockerApi,
    pub container_id: &'a str,
    pub port: u16,
    pub path: &'a str,
    pub transport: &'a dyn HttpTransport,
    pub attempt_timeout: Duration,
}

impl Probe for ContainerProbe<'_> {
    fn attempt(&mut self, at_ms: u64) -> Attempt {
        let t = self.attempt_timeout;
        match self.api.container_inspect(self.container_id, t) {
            Ok(Some(c)) if c.running => Attempt {
                at_ms,
                observation: self.transport.get(self.port, self.path, t),
                container_running: true,
            },
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
