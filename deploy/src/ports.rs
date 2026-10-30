//! Typed port leases (review gap #4, approved ranges).
//!
//! A lease is a *reservation recorded in the ledger before use*. Allocation considers three
//! independent sources of occupancy — real host listeners, existing Docker port mappings,
//! and leases already in the ledger — because any one of them alone is not enough (a stopped
//! container's mapping, a non-Docker process, a lease whose container has not been created).
//!
//! Collision semantics: if creating or starting the container reports the port busy, the
//! engine may ask for **one** replacement; the contested port is excluded and a second
//! collision is a hard failure, never a loop.

use crate::ledger::Ledger;
use std::collections::BTreeSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Application,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange {
    pub start: u16,
    pub end: u16, // inclusive
}

impl PortRange {
    pub const APPLICATIONS: PortRange = PortRange {
        start: 38000,
        end: 38999,
    };
    pub const TESTS: PortRange = PortRange {
        start: 39000,
        end: 39099,
    };

    pub fn for_purpose(p: Purpose) -> PortRange {
        match p {
            Purpose::Application => PortRange::APPLICATIONS,
            Purpose::Test => PortRange::TESTS,
        }
    }
    pub fn contains(&self, p: u16) -> bool {
        (self.start..=self.end).contains(&p)
    }
}

/// A host port bound for `container_port`, valid for exactly one deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortLease {
    pub host_port: u16,
    pub container_port: u16,
    pub purpose: Purpose,
}

/// Everything known to be taken. Gathered by the caller from the three sources.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Occupancy {
    /// Ports something is listening on at `127.0.0.1` (and the wildcard) on the host.
    pub host_listeners: BTreeSet<u16>,
    /// Host ports in existing Docker mappings, running **or** stopped.
    pub docker_mappings: BTreeSet<u16>,
    /// Ports leased in the ledger and not released.
    pub journaled_leases: BTreeSet<u16>,
}

impl Occupancy {
    pub fn with_ledger(mut self, ledger: &Ledger) -> Occupancy {
        self.journaled_leases.extend(ledger.leased_ports());
        self
    }
    pub fn taken(&self, p: u16) -> Option<Taken> {
        if self.host_listeners.contains(&p) {
            Some(Taken::HostListener)
        } else if self.docker_mappings.contains(&p) {
            Some(Taken::DockerMapping)
        } else if self.journaled_leases.contains(&p) {
            Some(Taken::JournaledLease)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    HostListener,
    DockerMapping,
    JournaledLease,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortError {
    /// Every port in the purpose's range is taken.
    Exhausted(PortRange),
    /// The port is outside the range for this purpose (test and application ranges never mix).
    OutOfRange { port: u16, purpose: Purpose },
    /// A replacement was already used for this deployment.
    CollisionRepeated { first: u16, second: u16 },
    /// A fixed request for a port that is taken.
    Occupied(u16, Taken),
}

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortError::Exhausted(r) => write!(f, "no free port in {}–{}", r.start, r.end),
            PortError::OutOfRange { port, purpose } => write!(f, "port {port} is outside the {purpose:?} range"),
            PortError::CollisionRepeated { first, second } => {
                write!(f, "ports {first} and {second} both collided; not trying again")
            }
            PortError::Occupied(p, t) => write!(f, "port {p} is occupied ({t:?})"),
        }
    }
}

impl std::error::Error for PortError {}

/// Lowest free port at or after `hint` (wrapping), skipping anything in `exclude`.
pub fn allocate(
    purpose: Purpose,
    container_port: u16,
    occ: &Occupancy,
    hint: Option<u16>,
    exclude: &[u16],
) -> Result<PortLease, PortError> {
    let r = PortRange::for_purpose(purpose);
    let size = (r.end - r.start) as u32 + 1;
    let first = hint.filter(|h| r.contains(*h)).unwrap_or(r.start);
    for i in 0..size {
        let p = r.start + ((first - r.start) as u32 + i) as u16 % size as u16;
        if occ.taken(p).is_none() && !exclude.contains(&p) {
            return Ok(PortLease {
                host_port: p,
                container_port,
                purpose,
            });
        }
    }
    Err(PortError::Exhausted(r))
}

/// A specific port was requested (for example by a test): validate rather than search.
pub fn request(purpose: Purpose, container_port: u16, port: u16, occ: &Occupancy) -> Result<PortLease, PortError> {
    if !PortRange::for_purpose(purpose).contains(port) {
        return Err(PortError::OutOfRange { port, purpose });
    }
    if let Some(t) = occ.taken(port) {
        return Err(PortError::Occupied(port, t));
    }
    Ok(PortLease {
        host_port: port,
        container_port,
        purpose,
    })
}

/// One-shot replacement after a reported collision. The contested port is excluded; if a
/// replacement was already issued, the caller must fail instead.
pub fn reallocate_after_collision(
    prev: &PortLease,
    already_replaced: Option<u16>,
    occ: &Occupancy,
) -> Result<PortLease, PortError> {
    if let Some(first) = already_replaced {
        return Err(PortError::CollisionRepeated {
            first,
            second: prev.host_port,
        });
    }
    allocate(
        prev.purpose,
        prev.container_port,
        occ,
        Some(prev.host_port + 1),
        &[prev.host_port],
    )
}
