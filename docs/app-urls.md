# Stable application URLs

`http://<id>.harpoon.localhost:7480` — one name per application, the same for every release.

## Why this shape (checked on macOS 27.0.1, the only version available for this work)

| Question | Result |
|---|---|
| Does `<anything>.harpoon.localhost` resolve with no `/etc/hosts` or DNS change? | Yes: `getaddrinfo` (Python, curl, Rust `std`) returns `::1` and `127.0.0.1` for it. A test (`the_system_resolver_sends_a_harpoon_localhost_name_to_the_router`) connects by name through the system resolver. |
| Can an unprivileged process listen on port 80 of loopback? | **No.** `bind(127.0.0.1:80)` and `bind([::1]:80)` fail with EACCES; only the wildcard `0.0.0.0:80` succeeds, which would expose applications to the LAN. |
| Result | The router listens on loopback port **7480**, so the URL carries the port. A port-less URL would need a privileged helper or a `pf` redirect (admin rights, system change); that is not done. |
| Older macOS (the app targets 12+) | **Not verified.** `.localhost` is reserved (RFC 6761) and the major browsers resolve it themselves, but this was not tested on 12–26. If a name does not resolve, the direct `http://127.0.0.1:<port>` URL still works. |

## Architecture

```
browser ──► 127.0.0.1:7480 (router, desktop app) ──► 127.0.0.1:<release host port> ──► Harpoon forwarder ──► guest container
              Host: <id>.harpoon.localhost                      (unchanged: ledger-leased 38000–38999)
```

* **Identity.** `deploy/src/route.rs`: the label is the last 10 characters of the application id (never reused, 50 bits), lower-case.
  It is derived, not stored, so it is persisted with the application identity by construction, exists for applications registered
  before this change, survives restarts and needs no new journal record. Release, container and port never enter it.
  A fixed test vector pins the derivation (changing it would change every URL).
* **Table.** `RouteTable` is a pure value built from the durable records (`serving_release` + its pinned host port, or the port
  lease): application → `Serving{port}` / `Idle` / `Conflict`. The router swaps whole tables, so a route moves exactly when
  the recorded serving release does, never half-way.
* **When the route moves.** Replacement, rollback and restoration all change what `serving_release` returns, and only at their
  existing success/restoration points. A candidate rejected before the cutover, or a failed rollback that stopped nothing, does not
  change the record, so the route does not move. After a failed post-cutover replacement, the route points at the restored release
  (same name). If restoration failed, the record still names the release that served; the router finds it unreachable and says
  so (503) instead of routing anywhere else.
* **Router** (`ui/harpoon-desktop/src-tauri/src/app_router.rs`). Std-only TCP forwarder: reads the first request head, picks the
  upstream from `Host`, then pipes bytes both ways (keep-alive, streaming, WebSockets work). Loopback bind only (a non-loopback
  address is refused); upstream is always `127.0.0.1:<port from the table>`, never from the request; unknown hosts (IPs, other
  domains, `x.<name>`) get 404 and nothing is forwarded; a hostname collision gets 409 for both applications and nothing is
  forwarded; an application with nothing recorded as serving gets 503. The table is re-read at most once a second from the
  journal (read-only), and once more when a connect to the recorded port fails (a cutover may have just moved it). A failed
  read keeps the last good table.
* **Restart.** The router starts with the desktop app and rebuilds its table from the journal, so nothing needs restoring. A
  container that must be started explicitly is reported as "not observed running" in the UI and 503 by the router until it runs;
  Harpoon starts nothing on its own.
* **Existing behaviour kept.** Direct `127.0.0.1:<port>` URLs, port allocation, ownership labels, journals and leases are
  untouched; this change only reads them. An attached external runtime (the read-only B7 viewer) is never routed.

## UI

The application header shows **Application URL** with Copy and Open acting on exactly the URL shown. The internal port is not in
the primary view; the Recorded-vs-observed panel keeps "Direct URL (diagnostic)". States: running; stopped (URL shown, with
"will not respond until the release is running"); hostname collision (no URL); router unavailable (no URL, reason shown).
Open goes through a Tauri command that accepts only `http://<label>.harpoon.localhost:<port>` or `http://127.0.0.1:<port>`.

## Limits

* The router runs only while the desktop app is open; the direct ports are served by the engine and keep working.
* One router port (7480). If it is taken the UI says so and shows no friendly URL; nothing else is affected.
* Routing is by the first request's `Host` on a connection; a kept-alive connection stays on its upstream until it closes
  (an old container stopping closes it).
* Plain HTTP only; no TLS, no per-application ports in the name, no custom names.
* Hostnames cannot be renamed; a collision (about 1 in 10^15 per pair) is detected and refused, not resolved.
