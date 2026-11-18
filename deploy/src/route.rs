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

/// The host the router also answers for the one application that owns the alias: `localhost:<router port>`.
/// `127.0.0.1` is deliberately not aliased: browsers keep cookies and storage per host, so it would be a second
/// origin for the same application (and `localhost` cookies are shared with every other localhost port).
pub const ALIAS_HOST: &str = "localhost";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteTable {
    routes: BTreeMap<String, Route>,
    /// What `localhost:<router port>` resolves to; `None` when no application owns the alias.
    alias: Option<Route>,
    owners: Vec<ApplicationId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasState {
    None,
    /// The only application that claims `localhost:<router port>`.
    Owner,
    /// More than one application claims it: it is routed to none of them.
    Contested,
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
        RouteTable {
            routes,
            alias: None,
            owners: Vec::new(),
        }
    }

    /// Give `localhost:<router port>` to the application that owns the alias, from the durable configuration
    /// (`config::alias_owners`). It resolves to that application's own route, so it moves exactly when the
    /// serving release does. More than one owner is a [`Route::Conflict`]: neither is routed through it.
    pub fn with_alias(mut self, owners: &[ApplicationId]) -> RouteTable {
        self.owners = owners.to_vec();
        self.alias = match owners {
            [] => None,
            [one] => Some(self.route_of(one).cloned().unwrap_or(Route::Idle { app: one.clone() })),
            many => Some(Route::Conflict { apps: many.to_vec() }),
        };
        self
    }

    /// What `app` has to do with the alias.
    pub fn alias_state(&self, app: &ApplicationId) -> AliasState {
        match (self.owners.as_slice(), self.owners.contains(app)) {
            ([_], true) => AliasState::Owner,
            (_, true) => AliasState::Contested,
            _ => AliasState::None,
        }
    }

    /// Resolve a request's `Host` for a router listening on `router_port`: an application hostname, or the
    /// alias, which needs the router's own port spelled out (`localhost` alone, or another port, is nothing).
    pub fn resolve_at(&self, host_header: &str, router_port: u16) -> Option<&Route> {
        let (host, port) = split_host(host_header);
        if host.trim_end_matches('.').eq_ignore_ascii_case(ALIAS_HOST) {
            return if port == Some(router_port) {
                self.alias.as_ref()
            } else {
                None
            };
        }
        self.resolve(host_header)
    }

    /// Resolve an HTTP `Host` header value (port and trailing dot allowed). Only exact application
    /// hostnames resolve; anything else (an IP, another domain, a wildcard) is `None`.
    pub fn resolve(&self, host_header: &str) -> Option<&Route> {
        let (host, _) = split_host(host_header);
        self.routes
            .get(host.trim_end_matches('.').to_ascii_lowercase().as_str())
    }

    pub fn route_of(&self, app: &ApplicationId) -> Option<&Route> {
        self.routes.get(&hostname(app))
    }
}

/// `host[:port]`; the port only when it is all digits and fits.
fn split_host(host_header: &str) -> (&str, Option<u16>) {
    match host_header.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (h, p.parse().ok()),
        _ => (host_header, None),
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

    // ---- the localhost alias

    const PORT: u16 = 7480;

    #[test]
    fn the_alias_resolves_to_its_owner_and_moves_with_the_serving_release() {
        let (a, b) = (app("1"), app("2"));
        let table = |port| {
            RouteTable::build([(a.clone(), Some(port)), (b.clone(), Some(38900))]).with_alias(std::slice::from_ref(&a))
        };
        let t = table(38001);
        assert_eq!(
            t.resolve_at("localhost:7480", PORT),
            Some(&Route::Serving {
                app: a.clone(),
                port: 38001
            })
        );
        assert_eq!(t.alias_state(&a), AliasState::Owner);
        assert_eq!(t.alias_state(&b), AliasState::None);
        // Same alias, the owner's new release: replacement, rollback and restoration all only change the port.
        assert_eq!(
            table(38002).resolve_at("LOCALHOST:7480", PORT),
            Some(&Route::Serving {
                app: a.clone(),
                port: 38002
            })
        );
        // The owner keeps its own hostname, and the other application is untouched.
        assert_eq!(t.resolve_at(&hostname(&a), PORT), t.resolve_at("localhost:7480", PORT));
        assert_eq!(
            t.resolve_at(&hostname(&b), PORT),
            Some(&Route::Serving { app: b, port: 38900 })
        );
    }

    #[test]
    fn the_alias_needs_the_routers_own_port_and_never_covers_other_hosts() {
        let a = app("1");
        let t = RouteTable::build([(a.clone(), Some(38001))]).with_alias(std::slice::from_ref(&a));
        for bad in [
            "localhost",
            "localhost:",
            "localhost:80",
            "localhost:7481",
            "localhost:99999",
            "127.0.0.1:7480",
            "[::1]:7480",
            "x.localhost:7480",
            "localhost.evil.test:7480",
            "0.0.0.0:7480",
        ] {
            assert!(t.resolve_at(bad, PORT).is_none(), "{bad:?} must not resolve");
        }
        assert!(
            t.resolve_at("localhost.:7480", PORT).is_some(),
            "a trailing dot is the same host"
        );
    }

    #[test]
    fn without_an_owner_localhost_resolves_to_nothing_and_names_are_unchanged() {
        let a = app("1");
        let t = RouteTable::build([(a.clone(), Some(38001))]);
        assert!(t.resolve_at("localhost:7480", PORT).is_none());
        assert_eq!(t.alias_state(&a), AliasState::None);
        assert_eq!(t.resolve_at(&hostname(&a), PORT), t.resolve(&hostname(&a)));
        assert!(RouteTable::build([(a, Some(38001))])
            .with_alias(&[])
            .resolve_at("localhost:7480", PORT)
            .is_none());
    }

    #[test]
    fn two_claimants_route_neither_through_the_alias_and_keep_their_names() {
        let (a, b) = (app("1"), app("2"));
        let t =
            RouteTable::build([(a.clone(), Some(38001)), (b.clone(), Some(38002))]).with_alias(&[a.clone(), b.clone()]);
        assert_eq!(
            t.resolve_at("localhost:7480", PORT),
            Some(&Route::Conflict {
                apps: vec![a.clone(), b.clone()]
            })
        );
        assert_eq!(
            (t.alias_state(&a), t.alias_state(&b)),
            (AliasState::Contested, AliasState::Contested)
        );
        assert_eq!(
            t.resolve_at(&hostname(&a), PORT),
            Some(&Route::Serving { app: a, port: 38001 })
        );
    }

    #[test]
    fn an_owner_with_nothing_serving_is_idle_and_an_unknown_owner_is_idle_not_a_guess() {
        let (a, ghost) = (app("1"), app("9"));
        let t = RouteTable::build([(a.clone(), None)]).with_alias(std::slice::from_ref(&a));
        assert_eq!(t.resolve_at("localhost:7480", PORT), Some(&Route::Idle { app: a }));
        let t = RouteTable::build([]).with_alias(std::slice::from_ref(&ghost));
        assert_eq!(t.resolve_at("localhost:7480", PORT), Some(&Route::Idle { app: ghost }));
    }
}
