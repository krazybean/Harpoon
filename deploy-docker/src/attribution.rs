//! Marker-free attribution: proving, without any cooperation from the application, that a health response came from
//! the exact container Harpoon created and verified.
//!
//! The marker (`X-Harpoon-Deployment-Marker`) stays the preferred proof: an application that echoes its container's
//! own value proves its own answer. An ordinary application does not, so the same conclusion is reached by proving
//! every hop between this host and the container instead. The chain, each link independent of the others:
//!
//! 1. **The host listener is Harpoon's forwarder.** Exactly one listener on `127.0.0.1:<port>`, owned by the forwarder pid.
//! 2. **That process owns this engine's Docker and management sockets.** So the Docker answers (container id, image,
//!    start time, port mapping) and the management channel both belong to the VM that process runs, and the forwarder
//!    pid is that engine's.
//! 3. **The forwarder dials the machine that is that VM.** The engine publishes the guest address it discovered from
//!    its own VM's serial log. An address proves nothing by itself (it can be stale, reassigned, or a fallback), so the
//!    machine at that address must answer a fresh random challenge that was delivered to the VM only over the
//!    engine's vsock-bound management channel. A machine that is not that VM cannot know it.
//! 4. **Docker says this container, once started, from the pinned image, owns the leased port**, before and after
//!    (checked by the backend around the probe: container id, image id, `StartedAt`, restart count, port mapping).
//!
//! For a published port bound to the guest's loopback (the vsock relay) the forwarder does not dial an address: the
//! connection travels over vsock, which only ever reaches the VM the engine owns, so link 3 is replaced by proof that the
//! engine's guest relay for the port exists.
//!
//! Every link fails closed. Remaining limits are in `docs/results/DEPLOY_MARKER_FREE_ATTRIBUTION.md`.

use crate::api::Published;
use crate::forwarder::HostListenerCheck;
use crate::guest::{ExecResult, MgmtClient};
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The guest TCP port the one-shot identity listener uses. Chosen next to Docker's (2375) and management (2377) ports.
/// Longer than the whole proof can take (10 s management `exec`, 3 s connect, 3 s read).
const LISTENER_LIFETIME_SECS: &str = "20";
pub const CHALLENGE_PORT: u16 = 2378;
/// The vsock port base the engine's guest relays use (`PortForwardManager.GuestRelay.vsockPortBase`).
pub const RELAY_VSOCK_BASE: u32 = 65536;

/// How the forwarder reaches the guest for a published port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Docker published on the guest's wildcard (what every deployment does): the forwarder dials the guest address.
    GuestAddress,
    /// Docker published on the guest's loopback: the forwarder uses the engine's vsock relay.
    GuestLoopbackRelay,
}

/// The route Docker's own report implies for a container port, or `None` when it reports anything else.
pub fn route_of(published: &[Published], container_port: u16) -> Option<Route> {
    let tcp: Vec<&Published> = published
        .iter()
        .filter(|p| p.proto == "tcp" && p.container_port == container_port)
        .collect();
    if tcp.is_empty() {
        return None;
    }
    let mut route = None;
    for p in tcp {
        let r = match p.host_ip.as_str() {
            "" | "0.0.0.0" | "::" => Route::GuestAddress,
            "127.0.0.1" => Route::GuestLoopbackRelay,
            _ => return None,
        };
        if route.is_some_and(|prev| prev != r) {
            return None; // one port published both ways: which path answers is not knowable
        }
        route = Some(r);
    }
    route
}

/// Proof about the path from this host to the guest, for one published host port.
pub trait PathProof {
    fn prove(&self, port: u16, route: Route) -> Result<(), String>;
}

/// The unix sockets a process holds (real: `lsof -U`).
pub trait SocketOwners {
    fn sockets_of(&self, pid: u32) -> Result<Vec<String>, String>;
}

/// The guest management channel (real: [`MgmtClient`]).
pub trait GuestChannel {
    fn exec(&self, argv: &[&str]) -> Result<ExecResult, String>;
}

impl GuestChannel for MgmtClient {
    fn exec(&self, argv: &[&str]) -> Result<ExecResult, String> {
        MgmtClient::exec(self, argv)
    }
}

/// A TCP connection to an address on the guest's network, from this host.
pub trait GuestDial {
    fn read_answer(&self, addr: Ipv4Addr, port: u16) -> Result<String, String>;
}

/// A fresh unpredictable challenge (32 hex digits).
pub trait Challenges {
    fn fresh(&self) -> Result<String, String>;
}

pub struct LsofSockets(pub PathBuf);

impl SocketOwners for LsofSockets {
    fn sockets_of(&self, pid: u32) -> Result<Vec<String>, String> {
        crate::forwarder::unix_sockets_of(&self.0, pid)
    }
}

pub struct TcpDial(pub Duration);

impl GuestDial for TcpDial {
    fn read_answer(&self, addr: Ipv4Addr, port: u16) -> Result<String, String> {
        let s = TcpStream::connect_timeout(&SocketAddr::from((addr, port)), self.0)
            .map_err(|e| format!("connect to {addr}:{port}: {e}"))?;
        let _ = s.set_read_timeout(Some(self.0));
        let mut buf = Vec::new();
        s.take(128)
            .read_to_end(&mut buf)
            .map_err(|e| format!("read from {addr}:{port}: {e}"))?;
        String::from_utf8(buf)
            .map(|t| t.trim().to_string())
            .map_err(|_| "the answer was not text".to_string())
    }
}

pub struct OsChallenges;

impl Challenges for OsChallenges {
    fn fresh(&self) -> Result<String, String> {
        use harpoon_deploy::time::{Entropy, OsEntropy};
        let mut b = [0u8; 16];
        OsEntropy::new()
            .fill(&mut b)
            .map_err(|e| format!("no randomness for the identity challenge: {e}"))?;
        Ok(b.iter().map(|x| format!("{x:02x}")).collect())
    }
}

/// Starts a single-use listener on the guest that answers one connection with the challenge, then is gone. `$1` is the
/// challenge (32 hex digits, validated by the caller), `$2` the port. Standard input, output and error are detached so the
/// management channel's `exec` returns. `$3` is its lifetime in seconds: a detached watchdog ends exactly this
/// challenge's listener (the pattern names the challenge, so a later probe's listener is never touched) when it has
/// not been used by then, e.g. because the CLI died before it could stop it.
pub const START_SCRIPT: &str = r#"pkill -f "[T]CP4-LISTEN:$2," 2>/dev/null
setsid socat "TCP4-LISTEN:$2,reuseaddr,backlog=1" "SYSTEM:echo $1" </dev/null >/dev/null 2>&1 &
setsid sh -c 'sleep "$3"; pkill -f "[T]CP4-LISTEN:$2,.*$1"' harpoon-attest-expiry "$1" "$2" "$3" </dev/null >/dev/null 2>&1 &
sleep 0.3
pgrep -f "[T]CP4-LISTEN:$2," >/dev/null || { echo "identity listener did not start" >&2; exit 1; }"#;
pub const STOP_SCRIPT: &str = r#"pkill -f "[T]CP4-LISTEN:$1," 2>/dev/null; exit 0"#;
/// `$1` is the vsock port of the relay.
pub const RELAY_PRESENT_SCRIPT: &str =
    r#"pgrep -f "[V]SOCK-LISTEN:$1," >/dev/null && echo present || echo absent"#;

fn is_challenge(t: &str) -> bool {
    t.len() == 32
        && t.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Sockets are compared as paths with the macOS `/private` alias of `/tmp` and `/var` ignored.
fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| -> String {
        let p = p.trim_end_matches('/');
        match p.strip_prefix("/private") {
            Some(rest) if rest.starts_with("/tmp") || rest.starts_with("/var") => rest.to_string(),
            _ => p.to_string(),
        }
    };
    norm(a) == norm(b)
}

pub struct HostGuestProof {
    pub listener: Box<dyn HostListenerCheck>,
    pub forwarder_pid: u32,
    pub sockets: Box<dyn SocketOwners>,
    pub docker_socket: PathBuf,
    pub mgmt_socket: PathBuf,
    pub guest: Box<dyn GuestChannel>,
    /// The address the engine published as the forwarder's target (discovered, never a fallback); `None` when it did not.
    pub forward_address: Option<Ipv4Addr>,
    pub dial: Box<dyn GuestDial>,
    pub challenges: Box<dyn Challenges>,
}

impl HostGuestProof {
    /// The real proof for an engine: `lsof`, the management socket, TCP, and `/dev/urandom`.
    pub fn real(
        listener: impl HostListenerCheck + 'static,
        lsof: &Path,
        forwarder_pid: u32,
        docker_socket: &Path,
        mgmt_socket: &Path,
        forward_address: Option<Ipv4Addr>,
    ) -> HostGuestProof {
        HostGuestProof {
            listener: Box::new(listener),
            forwarder_pid,
            sockets: Box::new(LsofSockets(lsof.to_path_buf())),
            docker_socket: docker_socket.to_path_buf(),
            mgmt_socket: mgmt_socket.to_path_buf(),
            guest: Box::new(MgmtClient::new(mgmt_socket, Duration::from_secs(10))),
            forward_address,
            dial: Box::new(TcpDial(Duration::from_secs(3))),
            challenges: Box::new(OsChallenges),
        }
    }

    fn challenge_address(&self) -> Result<(), String> {
        let addr = self.forward_address.ok_or_else(|| {
            "the Harpoon engine did not publish a guest address it discovered, so Harpoon cannot prove which machine its forwarder reaches (update and restart the engine, or have the application echo the deployment marker)".to_string()
        })?;
        let token = self.challenges.fresh()?;
        if !is_challenge(&token) {
            return Err("internal error: the identity challenge is not 32 hex digits".into());
        }
        let port = CHALLENGE_PORT.to_string();
        let started = self.guest.exec(&[
            "sh",
            "-c",
            START_SCRIPT,
            "harpoon-attest",
            &token,
            &port,
            LISTENER_LIFETIME_SECS,
        ]);
        // always stop the listener, whatever happened
        let answer = match &started {
            Ok(r) if r.exit == 0 => self.dial.read_answer(addr, CHALLENGE_PORT),
            Ok(r) => Err(format!(
                "the guest could not start its identity listener: {}",
                r.stderr.trim()
            )),
            Err(e) => Err(e.clone()),
        };
        let _ = self
            .guest
            .exec(&["sh", "-c", STOP_SCRIPT, "harpoon-attest-stop", &port]);
        match answer {
            Ok(a) if a == token => Ok(()),
            Ok(_) => Err(format!("the machine at {addr} did not answer the identity challenge as the Harpoon VM does, so it is not that VM")),
            Err(e) => Err(format!("the machine at {addr} could not be proven to be the Harpoon VM: {e}")),
        }
    }

    fn relay_present(&self, port: u16) -> Result<(), String> {
        let vsock = (RELAY_VSOCK_BASE + u32::from(port)).to_string();
        let r = self.guest.exec(&[
            "sh",
            "-c",
            RELAY_PRESENT_SCRIPT,
            "harpoon-relay-check",
            &vsock,
        ])?;
        if r.exit == 0 && r.stdout.trim() == "present" {
            Ok(())
        } else {
            Err(format!("the engine's guest relay for port {port} is not running, so the loopback-bound port cannot be attributed"))
        }
    }
}

impl PathProof for HostGuestProof {
    fn prove(&self, port: u16, route: Route) -> Result<(), String> {
        self.listener
            .verify(port)
            .map_err(|e| format!("host listener: {e}"))?;
        let owned = self
            .sockets
            .sockets_of(self.forwarder_pid)
            .map_err(|e| format!("socket ownership: {e}"))?;
        for (what, p) in [
            ("Docker", &self.docker_socket),
            ("management", &self.mgmt_socket),
        ] {
            let p = p.to_string_lossy();
            if !owned.iter().any(|o| same_path(o, &p)) {
                return Err(format!("the {what} socket {p} is not owned by the forwarder process {}, so they are not the same engine", self.forwarder_pid));
            }
        }
        match route {
            Route::GuestAddress => self.challenge_address(),
            Route::GuestLoopbackRelay => self.relay_present(port),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn the_route_is_what_docker_reports_and_never_a_guess() {
        let p = |ip: &str, cp: u16| Published {
            container_port: cp,
            proto: "tcp".into(),
            host_ip: ip.into(),
            host_port: 38000,
        };
        for ip in ["", "0.0.0.0", "::"] {
            assert_eq!(
                route_of(&[p(ip, 80)], 80),
                Some(Route::GuestAddress),
                "{ip:?}"
            );
        }
        assert_eq!(
            route_of(&[p("127.0.0.1", 80)], 80),
            Some(Route::GuestLoopbackRelay)
        );
        assert_eq!(
            route_of(&[p("0.0.0.0", 80), p("::", 80)], 80),
            Some(Route::GuestAddress),
            "ipv4 and ipv6 wildcards are one route"
        );
        assert_eq!(
            route_of(&[p("0.0.0.0", 80), p("127.0.0.1", 80)], 80),
            None,
            "one port published two ways has no single path"
        );
        assert_eq!(route_of(&[p("192.168.1.5", 80)], 80), None);
        assert_eq!(route_of(&[], 80), None);
        assert_eq!(route_of(&[p("0.0.0.0", 81)], 80), None);
    }

    #[test]
    fn paths_are_compared_across_the_private_alias_and_nothing_else() {
        assert!(same_path("/tmp/eng/d.sock", "/private/tmp/eng/d.sock"));
        assert!(same_path("/tmp/eng/m.sock", "/tmp/eng/m.sock/"));
        assert!(!same_path("/tmp/eng/d.sock", "/tmp/other/d.sock"));
        assert!(!same_path("/tmp/a", "/private/etc/a"));
    }

    // ---- fakes -----------------------------------------------------------------------------

    struct Listener(Result<(), String>);
    impl HostListenerCheck for Listener {
        fn verify(&self, _p: u16) -> Result<(), String> {
            self.0.clone()
        }
    }
    struct Sockets(Vec<String>);
    impl SocketOwners for Sockets {
        fn sockets_of(&self, _pid: u32) -> Result<Vec<String>, String> {
            Ok(self.0.clone())
        }
    }
    /// A guest that records what it was asked and, like the real one, answers the challenge from `answer_with`.
    struct Guest {
        log: RefCell<Vec<Vec<String>>>,
        start_exit: i64,
        relay: &'static str,
    }
    impl GuestChannel for Guest {
        fn exec(&self, argv: &[&str]) -> Result<ExecResult, String> {
            self.log
                .borrow_mut()
                .push(argv.iter().map(|s| s.to_string()).collect());
            let (exit, out) = match argv.get(3) {
                Some(&"harpoon-attest") => (self.start_exit, String::new()),
                Some(&"harpoon-relay-check") => (0, self.relay.to_string()),
                _ => (0, String::new()),
            };
            Ok(ExecResult {
                exit,
                stdout: out,
                stderr: if exit == 0 {
                    String::new()
                } else {
                    "no socat".into()
                },
            })
        }
    }
    struct Dial(Result<String, String>);
    impl GuestDial for Dial {
        fn read_answer(&self, _a: Ipv4Addr, _p: u16) -> Result<String, String> {
            self.0.clone()
        }
    }
    struct Fixed(&'static str);
    impl Challenges for Fixed {
        fn fresh(&self) -> Result<String, String> {
            Ok(self.0.to_string())
        }
    }
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn proof(
        listener: Result<(), String>,
        sockets: &[&str],
        guest: Guest,
        addr: Option<&str>,
        dial: Result<String, String>,
    ) -> (HostGuestProof, std::rc::Rc<()>) {
        (
            HostGuestProof {
                listener: Box::new(Listener(listener)),
                forwarder_pid: 4242,
                sockets: Box::new(Sockets(sockets.iter().map(|s| s.to_string()).collect())),
                docker_socket: "/tmp/h/d.sock".into(),
                mgmt_socket: "/tmp/h/m.sock".into(),
                guest: Box::new(guest),
                forward_address: addr.map(|a| a.parse().unwrap()),
                dial: Box::new(Dial(dial)),
                challenges: Box::new(Fixed(TOKEN)),
            },
            std::rc::Rc::new(()),
        )
    }
    fn guest() -> Guest {
        Guest {
            log: RefCell::new(vec![]),
            start_exit: 0,
            relay: "present\n",
        }
    }
    const OWNED: [&str; 2] = ["/private/tmp/h/d.sock", "/tmp/h/m.sock"];

    #[test]
    fn every_link_present_proves_the_path_for_a_guest_address() {
        let (p, _) = proof(
            Ok(()),
            &OWNED,
            guest(),
            Some("192.168.64.3"),
            Ok(TOKEN.into()),
        );
        assert_eq!(p.prove(38000, Route::GuestAddress), Ok(()));
    }

    #[test]
    fn the_challenge_is_delivered_only_over_the_management_channel_and_the_listener_is_always_stopped(
    ) {
        let g = guest();
        let (p, _) = proof(
            Ok(()),
            &OWNED,
            g,
            Some("192.168.64.3"),
            Ok("someone else".into()),
        );
        let r = p.prove(38000, Route::GuestAddress);
        assert!(r.unwrap_err().contains("is not that VM"));
    }

    #[test]
    fn a_wrong_answer_a_dead_address_and_a_listener_that_did_not_start_are_all_refusals() {
        for (dial, start_exit, want) in [
            (
                Ok("ffffffffffffffffffffffffffffffff".to_string()),
                0,
                "is not that VM",
            ),
            (Ok(String::new()), 0, "is not that VM"),
            (
                Err("connection refused".to_string()),
                0,
                "could not be proven",
            ),
            (
                Ok(TOKEN.to_string()),
                1,
                "could not start its identity listener",
            ),
        ] {
            let (p, _) = proof(
                Ok(()),
                &OWNED,
                Guest {
                    log: RefCell::new(vec![]),
                    start_exit,
                    relay: "",
                },
                Some("192.168.64.3"),
                dial,
            );
            let e = p.prove(38000, Route::GuestAddress).unwrap_err();
            assert!(e.contains(want), "{e}");
        }
    }

    #[test]
    fn no_published_address_means_nothing_can_be_proven() {
        let (p, _) = proof(Ok(()), &OWNED, guest(), None, Ok(TOKEN.into()));
        let e = p.prove(38000, Route::GuestAddress).unwrap_err();
        assert!(
            e.contains("did not publish a guest address it discovered"),
            "{e}"
        );
    }

    #[test]
    fn a_foreign_or_wrong_forwarder_listener_is_a_refusal_before_anything_reaches_the_guest() {
        let (p, _) = proof(
            Err(
                "pid 99 (nc) listens on 127.0.0.1:38000, but the Harpoon forwarder is pid 4242"
                    .into(),
            ),
            &OWNED,
            guest(),
            Some("192.168.64.3"),
            Ok(TOKEN.into()),
        );
        assert!(p
            .prove(38000, Route::GuestAddress)
            .unwrap_err()
            .starts_with("host listener:"));
    }

    #[test]
    fn the_docker_and_management_sockets_must_both_belong_to_the_forwarder() {
        for owned in [
            vec!["/tmp/h/d.sock"],
            vec!["/tmp/h/m.sock"],
            vec![],
            vec!["/tmp/other/d.sock", "/tmp/other/m.sock"],
        ] {
            let (p, _) = proof(
                Ok(()),
                &owned,
                guest(),
                Some("192.168.64.3"),
                Ok(TOKEN.into()),
            );
            let e = p.prove(38000, Route::GuestAddress).unwrap_err();
            assert!(e.contains("not owned by the forwarder process 4242"), "{e}");
        }
    }

    #[test]
    fn a_loopback_bound_port_is_attributed_through_the_engines_relay_not_an_address() {
        // no published address is needed: the connection travels over vsock
        let (p, _) = proof(
            Ok(()),
            &OWNED,
            guest(),
            None,
            Err("must not be dialed".into()),
        );
        assert_eq!(p.prove(38000, Route::GuestLoopbackRelay), Ok(()));
        let (p, _) = proof(
            Ok(()),
            &OWNED,
            Guest {
                log: RefCell::new(vec![]),
                start_exit: 0,
                relay: "absent\n",
            },
            None,
            Err("x".into()),
        );
        assert!(p
            .prove(38000, Route::GuestLoopbackRelay)
            .unwrap_err()
            .contains("relay for port 38000 is not running"));
        // the same host-side links still apply
        let (p, _) = proof(
            Err("two listeners".into()),
            &OWNED,
            guest(),
            None,
            Err("x".into()),
        );
        assert!(p.prove(38000, Route::GuestLoopbackRelay).is_err());
        let (p, _) = proof(Ok(()), &[], guest(), None, Err("x".into()));
        assert!(p.prove(38000, Route::GuestLoopbackRelay).is_err());
    }

    #[test]
    fn the_guest_scripts_only_ever_receive_validated_arguments() {
        assert!(is_challenge(TOKEN));
        for bad in [
            "",
            "0123",
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef0123456789abcde;",
            "0123456789abcdef0123456789abcdef0",
        ] {
            assert!(!is_challenge(bad), "{bad}");
        }
        struct Bad;
        impl Challenges for Bad {
            fn fresh(&self) -> Result<String, String> {
                Ok("x; rm -rf /".into())
            }
        }
        let (mut p, _) = proof(
            Ok(()),
            &OWNED,
            guest(),
            Some("192.168.64.3"),
            Ok(TOKEN.into()),
        );
        p.challenges = Box::new(Bad);
        assert!(p
            .prove(38000, Route::GuestAddress)
            .unwrap_err()
            .contains("not 32 hex digits"));
        // the real source produces valid challenges and different ones each time
        let (a, b) = (OsChallenges.fresh().unwrap(), OsChallenges.fresh().unwrap());
        assert!(is_challenge(&a) && is_challenge(&b) && a != b);
        assert!(
            START_SCRIPT.contains("setsid socat")
                && START_SCRIPT.contains("</dev/null >/dev/null 2>&1 &")
        );
    }
}
