# Harpoon 2.0 — Container Workspace Design Specification

Status: **PROPOSED / DESIGN AGREED IN DISCUSSION; NOT IMPLEMENTED**  
Scope: UI/UX product specification, not a runtime change.  
Related: [Canonical roadmap](../roadmap.md), [UI reference guidance](README.md).

## Purpose and principles

Create a cohesive, native-feeling container management interface without reproducing Docker Desktop. Use **dark graphite**, restrained **Harpoon yellow**, thin outlined surfaces, controlled elevation and selective glass. Borders establish structure; color establishes meaning; translucency establishes depth. Never sacrifice readability or operational clarity for decorative effects.

Keep Harpoon's runtime authoritative only for its macOS↔Linux boundary; Docker Engine remains authoritative for Docker objects. The Tauri/React UI is a client, not a second container state model. No unsolicited telemetry or image/logo network fetches.

### Responsive application shell

- Keep global navigation visible (compact icon rail at smaller widths), and breadcrumbs such as `Containers / Collectiv / api`.
- Main views: **Table** and **Stacks**. View preference persists. Retain selection, filters and gallery scroll position across navigation/view switches when applicable.
- Design for ordinary laptop windows first, not exclusively ultrawide displays. At narrow widths hide less important table columns behind details and use an overlay, not a permanently consuming inspector.
- Toolbars are quiet, commands contextual, typography compact yet readable, and destructive actions never masquerade as navigation.
- Bottom status bar: tiny CPU, memory and network sparklines; click navigates to Resources with metric selected. Metrics must declare their source/scope (host, VM, containers) and never conflate them. Show unavailable state instead of invented zeroes.

## Containers — Table view

- Sort/search/filter, selection, bulk operations, trustworthy status, image, exposed/published ports, and safe per-row actions.
- Group rows where metadata supports it; do not infer group membership from arbitrary name prefixes.
- Selecting one container opens the **same full-workspace details** used by the Stacks view.

## Containers — Stacks gallery

- Responsive, vertically scrollable grid of stack cards (not a horizontal carousel atop a permanent grid). Cards wrap naturally to available width; a partial next row invites scrolling.
- Click stack body to **replace** gallery with that stack's tile grid. Preserve sidebar, breadcrumbs, prior gallery scroll. Back returns to same position.
- Standalone containers appear as standalone objects, not falsely assigned to stacks.
- Stack menu in top-right `…`: explicit Start / Stop / Restart and other supported group actions; menu propagation must not trigger stack navigation.
- Status strip contains **one stable-ordered segment per container**, even when statuses match. Hover/focus reveals name and state; keyboard alternative required; consider grouped accessible summaries/density treatment for very large stacks.
- Show associated **networks and named volumes** in compact edge indicators, with counts. One resource can navigate directly; multiple offer a chooser. Distinguish bind mounts from volumes. Some resources may be shared outside the group: do not label them as exclusively owned.
- Status colors semantically consistent (running green, stopped/exited neutral, restarting amber, failures red); yellow remains brand/selection accent.

### Group semantics and operations

A Compose project, a Podman pod and an ad-hoc set of containers are *not* equivalent. Group display may be shared, but record the discovered group kind and provenance. Docker Engine object metadata is authoritative for attachments; original Compose configuration may not be available merely because Compose labels exist.

Define capabilities per group/engine; route group operations through backend-aware adapters. Prefer native semantics where actually available; only offer explicit per-container fallback when appropriate. Report affected targets, partial failure, unsupported operation and ordering uncertainty clearly. Never describe `compose up` as equivalent to `docker start`, nor `compose down` as equivalent to `docker stop`. Do not invent a broad vendor-independent orchestration platform prematurely.

## Technology-aware icons

- Bundle a licensed, locally available SVG icon set (evaluate Simple Icons and trademark conditions); **no remote fetch**. Default to generic container/image icon when uncertain.
- A deterministic, inspectable resolver takes explicit user overrides plus normalized engine metadata: image name/tag/namespace, OCI labels or annotations, entrypoint/command, exposed ports, optionally inspected base-image metadata. Compose published-port mappings are supplemental evidence, not Dockerfile EXPOSE.
- Distinguish **primary workload** (Postgres, PHP, Node, Python, Ruby, etc.) from **base OS** (Alpine, Debian). Do not assume installed build tools are the workload. Multiple or contradictory clues decrease certainty, not increase it.
- Ports are clues, not proof (80/443/3000 especially ambiguous; 3306 or 6379 shared by related servers). Never execute/modify a container or crawl its filesystem solely to infer an icon.
- Reuse resolved identity across Images, Containers and detail headers. Overrides stored locally, not in container metadata.

## Container workspace — core navigation

Opened identically from a table row or tile. Remain in same application shell with breadcrumb, container identity and technology icon, image/tag, engine/context, clearly labeled status, safe lifecycle actions and an overflow for secondary actions. Tabs planned:

| Tab | Initial responsibility |
|---|---|
| Overview | Identity, lifecycle, ports, health, restart policy, resource/link summaries; jump to related resources |
| Logs | Live stream and history where supported; pause, follow, search/filter, copy; clear distinction between UI buffer and daemon logs |
| Shell | Real interactive TTY-backed exec into the selected **running** container |
| Stats | Container-scoped CPU/memory/network/I/O with clear units/time window and explicit missing-data behavior |
| Inspect | Structured raw engine metadata with search/copy, JSON view; sensitive fields treated carefully |
| Networks | Actual network attachments, addresses and clickable network resources |
| Storage | Named volumes, bind mounts, mount mode and related resource links; never imply ownership |
 
No backend feature becomes a displayed promise without an engine capability and validation. Switching tabs does not silently destroy a running shell.

## Interactive shell and Quake-style quick console

- Primary **Shell** tab plus optional **Quake drop-down** summoned while within a specific container workspace. Both are different views of the **same exec/PTY session**, not two independent shells.
- A drop-down from the top with dark graphite tint, *visually translucent* background (tune opacity for readability, do not hardcode 50% alpha), moderate backdrop blur only behind the panel, crisp opaque text, restrained yellow edge/indicator, rapid motion (~150–200ms) and reduced-motion alternative.
- Header always identifies engine/context, container, executable and effective user. No ambiguous target or last-active-container command execution across workspaces.
- Start with image/container configured user (which can be root); display actual effective identity. Never elevate automatically. Default executable `/bin/sh`, with explicit alternative if absent; no automatic package installation or mutation of container filesystem.
- Use a proven terminal renderer such as xterm.js after dependency/security/license evaluation. Backend must provide real interactive exec with attach streams, stdin, stdout/stderr as protocol permits, TTY resize, bounded startup, disconnect distinction, termination and cleanup.
- Keyboard shortcut configurable; suggested `Ctrl+`\`` / equivalent rather than capturing a literal tilde in the shell. Opening and hiding overlay must not interrupt PTY. Prevent input leaks to underlying controls. Focus restoration and accessibility mandatory.
- Session lifespan when changing containers/leaving workspace must be explicitly specified and testable. No silent command delivery to a stale target. Closing UI must never stop the container or runtime.

## Background context discovery (read-only)

- Nonblocking, bounded discovery at startup and on demand: Docker context files/current selection and relevant environment variables, conventional Unix sockets, Podman system connections and rootless socket candidates. Do not trust config existence as availability.
- Deduplicate endpoints, probe with short timeouts and identify actual engine. States: detected, connected, unavailable, authentication needed. Keep discovery distinct from user **selection** of active context.
- Settings → Contexts prepopulates discovered choices with endpoint and status; explicit custom connection remains possible.
- **Never** start Docker Desktop, boot Podman, switch CLI defaults, change socket ownership/permissions, request unnecessary credentials, or alter another tool's configuration as a side effect of discovery.

## Networks workspace (related design direction)

- Two complementary modes: list/table for network operations; interactive **topology** showing observed network-to-container attachments, not imagined packet flows.
- Clicking network opens a network workspace; clicking container opens same container workspace; links from stack cards lead to same objects.
- Distinguish actual shared network membership from proof of communication. Honor engine contexts and unsupported fields. Topology needs keyboard navigation, pan/zoom and a readable non-graph alternative.

## Accessibility, security, acceptance

- At common laptop window dimensions, no critical primary action is clipped; table and tile views remain useful at reduced width.
- Every segment, icon-only action and topology edge has an accessible name/tooltip where appropriate; use keyboard focus, visible states and reduced motion.
- Tests should cover gallery scroll restoration, group menu click isolation, each-status segment mapping, context-scoped links, generic-icon fallback, conflicting evidence, backend capability absence and partial group-operation failures.
- Terminal tests: shell absent, container stopped/exited, configured root user, resize, ANSI/input, network loss, normal exit, stale container selection, tab/overlay session sharing, teardown without stopping container.
- Discovery tests: missing/stale sockets, engine down, multiple paths to same endpoint, overridden contexts, timeouts and **zero side effects**.
- No runtime implementation or validation is claimed by this document. Validate actual engine/driver capabilities before committing implementation plans.

## Staged design/implementation plan (not started)

1. Existing-UI audit and UI/UX Pro Max review; Ponytail challenge before implementation. Confirm present Rust/React command boundaries and test coverage.
2. Visual tokens/application shell, responsive layouts, keyboard/accessibility baseline.
3. Container table and stack-gallery composition with one detail route.
4. Technology resolver and bundled icons.
5. Container workspace tabs: Overview, Logs, Inspect, Stats, Networks, Storage.
6. Read-only engine/context discovery; capability-aware group actions.
7. Shell transport/session lifecycle and xterm.js presentation; Quake overlay.
8. Network topology and shared relationship navigation.
9. Functional regression, packaging, security audit and screenshot acceptance across sizes.

Sequence can change after repository evidence. No phase is marked PASS from this specification.

## Reference provenance

See existing repository images `docs/ui/01-overview.png` through `07-diagnostics.png` (earlier UI pass). The newer Harpoon multi-screen *design study* and the four user-supplied visual inspirations in the design discussion are **not yet committed as binaries**. They are inspirational only: no imagined text, metrics, features or paths should become requirements without verification. A companion manifest in `docs/ui/harpoon-2-reference-notes.md` records their intended use and remaining image-ingestion task.
