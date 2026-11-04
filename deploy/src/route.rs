//! Stable application hostnames and the table that resolves them to the release serving now.
//!
//! The hostname belongs to the **application identity**, not to a deployment or a container: it is
//! derived from the application id (never reused, never derived from a name), so it is the same for
//! every release, replacement, failed deployment, restoration and rollback, survives restarts without
//! a new journal record, and exists for applications registered before this module did.
//!
//! The table is a pure value built from the durable records (the serving release of each application);
//! a router swaps whole tables, so a route moves exactly when the recorded serving release does and
//! never half-way. Nothing here binds a socket.

use crate::ids::ApplicationId;
use crate::ledger::{Ledger, ResourceKind};
use crate::ports::PortRange;
use crate::serving::serving_release;
use crate::state::Deployment;
use std::collections::BTreeMap;

pub const DOMAIN: &str = "harpoon.localhost";
/// Characters of the application id's random part used as the label. 10 Crockford characters are
/// 50 bits: short and URL-safe, and a clash is detected rather than assumed away (see [`RouteTable`]).
const LABEL_LEN: usize = 10;

/// `ABC124XYZ`-style label, lower-case (DNS is case-insensitive; browsers lower-case hosts).
pub fn label(app: &ApplicationId) -> String {
    let id = app.as_str();
    let body = id.split_once('_').map_or(id, |(_, b)| b);
    body[body.len().saturating_sub(LABEL_LEN)..].to_ascii_lowercase()
}

/// `<label>.harpoon.localhost`.
pub fn hostname(app: &ApplicationId) -> String {
    format!("{}.{DOMAIN}", label(app))
}

/// The host port of the release `app` serves now: its pinned configuration (also after a restoration,
/// which serves on the previous release's own lease, and after a rollback, which pins its fresh
/// port), or the recorded port lease of the serving deployment when nothing was pinned.
pub fn serving_port(ledger: &Ledger, deployments: &[Deployment], app: &ApplicationId) -> Option<u16> {
    let s = serving_release(ledger, deployments, app)?;
    if let Some(c) = &s.config {
        return Some(c.host_port);
    }
    ledger
        .entries()
        .iter()
        .filter(|e| e.deployment == s.deployment && e.key.kind == ResourceKind::PortLease && !e.is_released())
        .find_map(|e| e.key.name.parse().ok())
}

/// The table for every application in `apps`, from the durable records. A recorded port outside the ranges Harpoon
/// leases for applications is not a forwarding target (the router never trusts an arbitrary port).
pub fn table_of(ledger: &Ledger, deployments: &[Deployment], apps: &[ApplicationId]) -> RouteTable {
    let leased = |p: u16| PortRange::APPLICATIONS.contains(p) || PortRange::TESTS.contains(p);
    RouteTable::build(
        apps.iter()
            .map(|a| (a.clone(), serving_port(ledger, deployments, a).filter(|p| leased(*p)))),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Forward to `127.0.0.1:<port>`: the host port of the serving release.
    Serving { app: ApplicationId, port: u16 },
    /// The application is registered but nothing is recorded as serving (or no port is known).
    Idle { app: ApplicationId },
    /// Two applications derive the same hostname. Neither is routed.
    Conflict { apps: Vec<ApplicationId> },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteTable {
    routes: BTreeMap<String, Route>,
}

impl RouteTable {
    /// One entry per application: its serving host port, if any. A hostname claimed by more than one
    /// distinct application is a [`Route::Conflict`] for all of them.
    pub fn build(apps: impl IntoIterator<Item = (ApplicationId, Option<u16>)>) -> RouteTable {
        let mut by_host: BTreeMap<String, Vec<(ApplicationId, Option<u16>)>> = BTreeMap::new();
        for (app, port) in apps {
            let claims = by_host.entry(hostname(&app)).or_default();
            if !claims.iter().any(|(a, _)| *a == app) {
                claims.push((app, port));
            }
        }
        let routes = by_host
            .into_iter()
            .map(|(host, mut claims)| {
                let route = if claims.len() > 1 {
                    Route::Conflict {
                        apps: claims.into_iter().map(|(a, _)| a).collect(),
                    }
                } else {
                    let (app, port) = claims.remove(0);
                    match port {
                        Some(port) if port >= 1024 => Route::Serving { app, port },
                        _ => Route::Idle { app },
                    }
                };
                (host, route)
            })
            .collect();
        RouteTable { routes }
    }

    /// Resolve an HTTP `Host` header value (port and trailing dot allowed). Only exact application
    /// hostnames resolve; anything else (an IP, another domain, a wildcard) is `None`.
    pub fn resolve(&self, host_header: &str) -> Option<&Route> {
        let host = host_header
            .rsplit_once(':')
            .filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit()))
            .map_or(host_header, |(h, _)| h);
        self.routes
            .get(host.trim_end_matches('.').to_ascii_lowercase().as_str())
    }

    pub fn route_of(&self, app: &ApplicationId) -> Option<&Route> {
        self.routes.get(&hostname(app))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(tail: &str) -> ApplicationId {
        format!("app_{:0>26}", tail).parse().unwrap()
    }

    #[test]
    fn the_hostname_is_short_lowercase_and_pinned_to_the_application_id() {
        // A fixed vector: changing the derivation would change every user's URL.
        let a: ApplicationId = "app_01JABCDEFGHJKMNPQRSTVWXYZ0".parse().unwrap();
        assert_eq!(hostname(&a), "qrstvwxyz0.harpoon.localhost");
        assert_eq!(label(&a), "qrstvwxyz0");
        assert!(label(&a).bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
    }

    #[test]
    fn the_hostname_ignores_everything_but_the_application() {
        let a = app("5");
        let t1 = RouteTable::build([(a.clone(), Some(38001))]);
        let t2 = RouteTable::build([(a.clone(), Some(38002))]);
        let t3 = RouteTable::build([(a.clone(), None)]);
        for t in [&t1, &t2, &t3] {
            assert!(t.route_of(&a).is_some());
            assert!(t.resolve(&hostname(&a)).is_some());
        }
    }

    #[test]
    fn a_route_follows_the_serving_port_and_goes_idle_without_one() {
        let a = app("5");
        let t = RouteTable::build([(a.clone(), Some(38001))]);
        assert_eq!(
            t.resolve(&hostname(&a)),
            Some(&Route::Serving {
                app: a.clone(),
                port: 38001
            })
        );
        let t = RouteTable::build([(a.clone(), None)]);
        assert_eq!(t.resolve(&hostname(&a)), Some(&Route::Idle { app: a.clone() }));
        // A privileged or zero port is never a forwarding target.
        for p in [0u16, 80, 1023] {
            let t = RouteTable::build([(a.clone(), Some(p))]);
            assert_eq!(t.route_of(&a), Some(&Route::Idle { app: a.clone() }));
        }
    }

    #[test]
    fn host_headers_resolve_only_for_exact_application_hostnames() {
        let a = app("5");
        let h = hostname(&a);
        let t = RouteTable::build([(a.clone(), Some(38001))]);
        assert!(t.resolve(&format!("{h}:7480")).is_some());
        assert!(t.resolve(&h.to_ascii_uppercase()).is_some());
        assert!(t.resolve(&format!("{h}.")).is_some());
        for bad in [
            "127.0.0.1:7480",
            "localhost",
            "harpoon.localhost",
            &format!("x.{h}"),
            &format!("{h}.evil.test"),
            &format!("{h}@evil.test"),
            "",
        ] {
            assert!(t.resolve(bad).is_none(), "{bad:?} must not resolve");
        }
    }

    #[test]
    fn a_hostname_collision_routes_neither_application() {
        // Same last 10 characters, different applications.
        let a: ApplicationId = "app_0000000000AAAAAAAAAAAAAAAA".parse().unwrap();
        let b: ApplicationId = "app_1111111111AAAAAAAAAAAAAAAA".parse().unwrap();
        let c = app("7");
        assert_eq!(hostname(&a), hostname(&b));
        let t = RouteTable::build([
            (a.clone(), Some(38001)),
            (b.clone(), Some(38002)),
            (c.clone(), Some(38003)),
        ]);
        assert_eq!(
            t.route_of(&a),
            Some(&Route::Conflict {
                apps: vec![a.clone(), b.clone()]
            })
        );
        assert_eq!(t.route_of(&a), t.route_of(&b));
        // The unrelated application is unaffected.
        assert_eq!(t.route_of(&c), Some(&Route::Serving { app: c, port: 38003 }));
        // The same application listed twice is not a collision.
        let t = RouteTable::build([(a.clone(), Some(38001)), (a.clone(), Some(38001))]);
        assert_eq!(t.route_of(&a), Some(&Route::Serving { app: a, port: 38001 }));
    }
}
