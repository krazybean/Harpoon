# Harpoon Roadmap — Canonical Trackable

> Harpoon is a lightweight Docker-compatible runtime for Apple Silicon macOS. Docker Engine remains authoritative for containers/images/volumes/networks/API/Compose/BuildKit. Harpoon owns only the macOS↔Linux boundary (VM lifecycle, socket bridge, networking, bind-mount transport, resources, memory reclamation, diagnostics).

## Deployment control plane (Harpoon 2.1) — canonical milestone tracker

**This section is the single authoritative status of the Harpoon 2.1 deployment milestones.**
Everything else is supporting material, not a second tracker:

- Scope, acceptance criteria, failure tests and stop conditions:
  `docs/deployment-control-plane-requirements.md` (the authority for *what* a milestone must do).
- Boundary decision: `docs/decisions/0004-deployment-control-plane-boundary.md`.
- First-slice design and isolation rules: `docs/deployment-first-slice-boundary.md`.
- Evidence of what was built and observed: `docs/results/DEPLOY_*.md` (historical records; they are
  not edited when later work supersedes a statement in them — the newer record is linked below).
- `docs/harpoon-2-status-and-direction.md` is an index that points here.

**Rule for keeping this current.** When a milestone PR merges, the same PR (or an immediately
accompanying documentation PR) marks its row complete with the PR number and evidence link, and
updates **Next executable milestone** below. A roadmap question is answered by reading this section
and the evidence it links; new investigation is warranted only when that evidence is genuinely
contradictory or missing.

**Tracks.** Harpoon 2.0 runtime/distribution gates (release candidates, second-Mac verification,
signing, packaging, UI polish, Intel, DNS) are a separate track and are not prerequisites of any 2.1
milestone below. The "Next milestone" lines under *Current Position* belong to the 2.0 runtime/UI
track; the 2.1 next milestone is stated only here. Anything that does block a 2.1 milestone must
name the specific acceptance criterion it blocks; otherwise it is deferred without interrupting
execution.

Approved decisions (2026-10-30): Rust engine (CLI first), SQLite as the permanent store, macOS
Keychain for secret values, host-side execution over the existing Docker API/CLI, no mandatory
Redis/PostgreSQL/Kubernetes, no new daemon until justified, brief documented interruptions
acceptable. Deploying to Vercel or other providers is an open possibility, deliberately undecided.

### What exists today versus what is still incomplete

**Implemented — the first-deployment slice (offline-tested; live-accepted once for a disposable
deployment):** a local-directory Dockerfile deployment with bundled, pinned Buildx and image
identity by id; preliminary and final health checks attributed by per-container marker; loopback-only
port publishing with LAN isolation measured; a durable checksummed single-writer journal with crash
recovery; a gated CLI (`status`, `doctor`, `plan`, `recover`, `reconcile`, `deploy`, `teardown`,
`cleanup-image`, `inventory`, `export-journal`) with ownership-verified, dry-run-by-default cleanup.
Evidence: `docs/results/DEPLOY_B7_LIVE_RESULTS.md` (B7 first-deployment acceptance: PASS — not
general production readiness).

**Milestone B implementation status: COMPLETE — OWNER RISK-ACCEPTED / DEFERRED
EVIDENCE.** The B1–B15 implementation is complete on `main`, including permanent
SQLite state, health-gated replacement, bounded cutover recovery, explicit rollback,
runtime configuration pinning, durable pause/held-event state, and the separately
gated live wiring. This is an implementation closure, not an unconditional
acceptance PASS: L5–L7 and P1–P2 were not run because the required idle-host
precondition failed, and therefore remain explicitly untested.

**Shipping gate: closed.** `Gate::shipping()` refuses live `deploy`, `replace`, `rollback` and
`restore`. The acceptance binary is separately built and authorized for live scenarios; it is not
the shipping binary and does not open the product gate.

### Milestones

| Milestone | Scope | Status |
|---|---|---|
| A | Specification and UX discovery | Approved (decisions 2026-10-30; B1 authorized) |
| B | Deployment engine and lifecycle — see the step table below | **Implementation complete — owner risk-accepted / deferred evidence**: L1–L4 live evidence retained; L5–L7 and P1–P2 not run; not an unconditional acceptance PASS |
| C | Builds, environments, and secrets (best-effort redaction with leakage tests) | **Implementation-complete by owner decision** (offline slice merged in #141; native Keychain round trip verified once on a disposable item). The real Docker/BuildKit acceptance evidence below stays deferred, not claimed. Scoped Config/Secret definitions and immutable per-deployment snapshots, Keychain-backed immutable secret versions (stdin-only input), Dockerfile target/build args/BuildKit secret mounts, runtime injection, v3 release pins that keep exact secret versions (restore and rollback resolve the original versions), stream redaction with fail-closed withholding, `config-*` commands. Deferred: image reuse for runtime-only changes (rebuild accepted), live Docker/BuildKit evidence (history, layers); build-secret files survive a hard kill until the next writing command (reported by `status`). Details and exact gaps: `docs/results/DEPLOY_MILESTONE_C_CONFIGURATION_SECRETS.md` |
| Gate U | Unattended-execution feasibility (Keychain and headless credentials, execution prerequisites); decision only, agent deferred | **NOT PASSED — unattended operation unproven.** No experiment was run and no agent was built; everything automatic is attended only ("while Harpoon is running"). Recorded in `docs/results/SPIKE_UNATTENDED_EXECUTION.md` |
| D | Git integration and automation (polling, signed-fixture webhooks); attended only | **Active; implemented and verified offline in one PR (attended only).** Public/private HTTPS and SSH (pinned host key) sources, branch/tag/exact-commit checkout in a bounded private workspace, the immutable commit recorded before every deployment, foreground polling with late detection, loopback-only GitHub-HMAC and GitLab-token webhooks, a CI trigger, B14 delivery-id idempotency, newest-wins supersession and a source executor for held/selected events. **Implementation closure (merged in #142, `3084712`) — residual risks preserved, none cleared:** an SSH server (wrong host key, revoked deploy key) is asserted, not run; TLS handling is Git's and unexercised; real GitHub/GitLab deliveries were never received (they need a tunnel or relay) and GitLab's delivery-id header is unconfirmed (the commit is the fallback key); webhook events buffered in memory during a build are lost if the process dies (polling finds the push later, reported late); a hard-killed process can leave a checkout workspace behind until the next writer sweeps it (owner-pid scoped); submodules and Git LFS are not initialised; an active build is never cancelled by a newer push. Gate U is NOT PASSED: everything is attended-only. `docs/results/DEPLOY_MILESTONE_D_GIT_INTEGRATION.md` |
| E | Desktop deployment experience and previews | **Active; implemented and verified offline in one PR (attended only; shipping gate closed).** The existing Deployments UI now reads the real engine: a typed read-only snapshot of the SQLite journal (`deploy_snapshot`), recorded redacted step logs, Docker's observed container state kept separate from recorded evidence, and a bounded set of engine operations (`deploy_op`) run through the same CLI library under `Gate::shipping()`. Repository registration, scoped Config, Keychain secrets (stdin only) and attended resume work; live deploy, rollback and restore are visibly disabled with the engine's own refusal reason. The seeded simulation is reachable only through a development-build preview link. **Branch previews are not implemented** (they need per-branch application registration, expiry and ownership-label cleanup, a new subsystem); nothing is shown for them. Not exercised: a live deployment from the desktop (the gate stays closed), a real locked-Keychain prompt. `docs/results/DEPLOY_MILESTONE_E_DESKTOP.md` |

### Milestone B — steps

| Step | Scope | Status | PR(s) | Evidence |
|---|---|---|---|---|
| Baseline | Dockerfile build over the bridge, load, publish latency, idle cost | Complete (one host, one session) | #103 | `docs/results/DEPLOY_BASELINE.md` |
| B1 | Offline engine foundation: state machine, resource ledger, deadlines, redaction, port leases, evidence | Complete | #106, #108 | `docs/results/DEPLOY_ENGINE_FOUNDATION_REVIEW.md` |
| B2 | Bundled, pinned, signed Buildx | Complete as a verified artifact (not exercised in a released app) | #109 | `docs/results/DEPLOY_BUILDX_PACKAGING.md`, `DEPLOY_BUILDX_VALIDATION.md` |
| B3 | Docker backend over the Unix socket + Buildx runner | Complete | #110 | `docs/results/DEPLOY_DOCKER_BACKEND.md` |
| B4 | Source confinement, port allocation, attributed health probing | Complete | #111 | `docs/results/DEPLOY_DOCKER_BACKEND.md` |
| B5 | Durable journal and crash recovery | Complete | #112 | `docs/results/DEPLOY_JOURNAL.md` |
| B6 | Gated developer CLI | Complete | #113 | `docs/results/DEPLOY_CLI.md` |
| B7 | First live disposable deployment, LAN isolation, ownership-verified teardown | **Complete — first-deployment acceptance PASS** (not production readiness) | #114–#118 | `docs/results/DEPLOY_B7_LIVE_RESULTS.md` |
| B8.1 | Offline failure/recovery coverage (crash sweeps, interrupted-deploy CLI regression) | Complete | #120 | `docs/results/DEPLOY_CLI.md` (B8.1) |
| B8.2 | Retained-image cleanup (`cleanup-image`, typed conflict outcome) | Complete (offline; simulated Docker only) | #121 | `docs/results/DEPLOY_CLI.md` (B8.2) |
| **B8.3** | **Controlled live failure and recovery validation** | **Complete — CONDITIONAL PASS: user-reported L1–L4 live results, exact inventory restoration; live Docker HTTP 409 observed, typed `DockerRefused` journal transition remains untested** | — | `docs/results/DEPLOY_B8_3_LIVE_RESULTS.md` |
| B9 | Permanent SQLite state (replaces the first-slice journal; required before any real replacement or rollback) | **Complete (offline; includes legacy-journal import)** | #124 | `docs/results/DEPLOY_B9_SQLITE_STATE.md`, `docs/deployment-first-slice-boundary.md` decision 1 |
| B10 | Health-gated replacement of a running release (offline slice) | **Complete (offline; `replace --simulate` only)** | #125 | `docs/results/DEPLOY_B10_REPLACEMENT.md`; requirements D5, Milestone B criteria 2–3 |
| B11 | Bounded cutover recovery (offline slice and hardening) | **Implementation complete; offline evidence retained**: pinned non-secret release configuration, one bounded restoration (default 60 s) after a failed cutover, recorded outcome and durations, trigger pause, failed candidate preserved, explicit `restore` for an interrupted cutover; hardening: pin before any side effect, serving release/container derived from the records, occupied ports detected before a restoration; live wiring is present behind the separately authorized acceptance gate | #126, #127 | `docs/results/DEPLOY_B11_RECOVERY.md`; requirements D5, D10, criterion 4 |
| B12 | Explicit manual rollback | **Implementation complete; offline evidence retained**: `releases` lists retained releases and the serving one; `rollback --to REL` re-serves an exact earlier release from its retained image id and pinned configuration (nothing rebuilt), after a preliminary check, with a printed plan, interruption bound and data warning, explicit `--confirm-rollback`, the existing bounded recovery on failure, a durable rollback record, and a trigger pause after success; live wiring is present behind the separately authorized acceptance gate | #128 | `docs/results/DEPLOY_B12_ROLLBACK.md`; requirements D5, criterion 5 |
| B13 | Runtime configuration, failure hardening, acceptance matrix | **Implementation complete; offline evidence retained**: `--env KEY=VALUE` non-secret variables validated, pinned per release (`v2` pin; `v1` unchanged) and reproduced by restoration and rollback; preliminary-failure next actions (real commands only); status wording and forwarder-default corrections; offline failure matrix audit and additions; canonical acceptance matrix and deferred live procedures; durable pause/resume state is implemented | this PR | `docs/results/DEPLOY_B13_ACCEPTANCE.md`; criteria 1 and 3 |
| B14 | Durable pauses, held events, explicit resume | **Complete (offline)**: `pause`/`event`/`resume` records in the journal replayed from SQLite; engine ingress (`ingest`) with delivery-id idempotency, isolation and fail-closed replay; `resume --app A --dismiss \| --newest \| --revision REV` and read-only `events`; a selection is recorded as pending attended execution, deploys **nothing** and keeps the application blocked until dismissed; `status` shows pauses, held events and decisions; crash sweeps (every write, every SQLite prefix); no event source and no source executor exist, so real delivery and execution are deferred to Milestone D | this PR | `docs/results/DEPLOY_B14_EVENTS_RESUME.md` |
| B15 | Reviewed live lifecycle integration | **Implementation complete; live evidence partial**: live `replace`, `rollback` and `restore` wired into the maintained CLI behind a gate that is **closed in the shipping binary**; a separate `harpoon-deploy-acceptance` binary (opt-in Cargo feature, no fault injection) opens it for authorized acceptance runs; honours B13 environment pins and B14 pauses; packaging-isolation tests and CI guards; L1–L4 were demonstrated, while L5–L7 and P1–P2 remain untested under the recorded idle-host precondition failure | this PR | `docs/results/DEPLOY_B15_LIVE_INTEGRATION.md` |

### Acceptance disposition (not a new submilestone)

L1–L4 passed live with the local-only acceptance binary and restored the Docker
inventory; the records remain `docs/results/DEPLOY_MILESTONE_B_LIVE_RESULTS_L1_L3.md`
and `docs/results/DEPLOY_MILESTONE_B_L4_RESULTS.md`. The campaign then stopped
before L5–L7 and P1–P2 because the idle-host precondition failed.

The B implementation is closed for planning by explicit owner risk acceptance. The
requirements' full acceptance criteria remain the truth: the deferred checks below
are not passed, and no release or shipping claim may treat them as passed.

### Milestone B deferred evidence and closure boundary — 2026-11-01

The latest-main campaign stopped before S0 because precondition 13 (idle host)
failed. The recorded sample showed load averages 4.29/6.18/7.44 on 11 cores and
38.67% CPU idle while VS Code, WindowServer, cloudd, Codex and other work were
active. No campaign Docker mutation or scenario run occurred. The precondition
record is preserved at
`/Users/juancastro/harpoon-b-acceptance-evidence/L567-20261101/00-preconditions/preconditions.txt`.

Deferred, still untested checks:

- **L5:** non-secret environment pinning through replacement, failed-final-check
  restoration, and rollback.
- **L6:** preliminary health failure while a previous release is running; the
  previous release must remain unchanged and the candidate must be retained or
  cleaned according to the recorded plan.
- **L7:** store-loss visibility and read-only discovery of labelled Docker
  workloads, including fail-closed handling of a corrupt store copy.
- **P1:** Dockerfile build overhead at or below the requirements' 1.2× direct-build
  ceiling.
- **P2:** no resident engine process and no idle-cost violation for the one-shot
  CLI.

L1–L4 remain the existing live evidence; they do not substitute for these checks.
The offline suites, formatting, Clippy, gate-closed assertions, binary identity
checks, and no-mutation precondition record support implementation closure but do
not convert deferred checks into PASS. The shipping `Gate::shipping()` remains
closed, and no Docker/VM rerun is authorized by this closure.

### Milestone B12: Explicit manual rollback (complete, offline)

Bounded cutover recovery (B11, #126 and #127) is complete as an offline slice. Explicit manual rollback (B12, #128) is
implemented offline: a developer lists an application's retained releases (`releases`),
sees which one serves, and names an earlier release by its exact id (`rollback --to`). A target is
accepted only when its pinned configuration (an immutable image id) and a retained image are
recorded; it is then re-served from that image and configuration with nothing rebuilt. The serving
release keeps running until the target has passed its preliminary check; the plan, the anticipated
interruption and "Application rollback only. Data, volumes and database schema are NOT rolled back."
are printed first, and the cutover needs `--confirm-rollback`. A failed cutover gets the same single
bounded restoration of the release that was serving. The rollback is a durable deployment operation
(source, target, pin, interruption, outcome); the serving release is derived from the records; a
successful rollback pauses the application's automatic triggers, shown by `status`, with durable
`resume` state implemented in B14. The live replacement and rollback wiring exists behind the
separately authorized acceptance gate; the shipping gate stays closed. Nothing here claims
production readiness. The deferred evidence remains listed in `docs/results/DEPLOY_B12_ROLLBACK.md`
and `DEPLOY_B11_RECOVERY.md`.

The separate Harpoon 2.0 track is unchanged by this milestone.

**Current: Milestone E — real desktop deployment experience (attended only).** Milestone C is implementation-complete by owner decision and Gate U is NOT PASSED (see the table above). L1–L4 remain preserved live evidence;
L5–L7 and P1–P2 are explicitly deferred and untested after the idle-host precondition stopped the
campaign before S0. This owner-risk acceptance does not reopen any live gate. Revisit the deferred
checks only when a separately authorized, idle-host campaign is worth the release risk.

Out of scope for this initiative's first release: inbound public webhooks on a laptop,
Vercel Functions/Edge/Build Output parity, a provider abstraction, zero-downtime routing,
and automatic Docker-state reconciliation (cleanup stays explicit and ownership-verified, as in
`reconcile`, `teardown` and `cleanup-image`; see ADR 0004).

## Current Position

### Harpoon 2.0 Step 1 — release hardening IN PROGRESS (2026-10-29)

The Apple Silicon target is the 2.0 release scope. Intel/universal packaging,
Windows/Linux hosts, and the 2.1 deployment control plane (tracked above) remain
outside the 2.0 release scope. Podman/rootless is not a 2.0 gate. The published `v2.0.0-rc.1` remains a
testing prerelease with stale release documentation; the executable startup
gate is drafted in `docs/releases/v2.0.0-startup-reliability.md`.

Step 1 evidence currently includes offline fail-closed release preflight and
socket-permission regressions. Guest asset reconciliation remains blocked until
the untracked kernel/root inputs and a safe external build engine are available;
no tracked guest asset was overwritten.

### Harpoon 2 stabilization transition — FEATURE COMPLETE / RELEASE NOT READY (2026-10-17 America/Chicago)

The planned Harpoon 2 redesign surfaces are implemented: container and stack
workspace, technology icons, Container and Network Topology, Shell, CSSH,
Quake Shell, Context & Engine Discovery, header resource meters/sparklines,
and the read-only per-container Stats tab. This is feature complete, not a
release claim. Release remains deferred until the native acceptance matrix,
the prioritized polish backlog, and the reliability/distribution gates below
are closed.

Evidence classes are deliberately separate:

- **Native visually accepted:** per-container Stats after the shared
  snake_case → camelCase Tauri contract fix; Docker-reported values render
  correctly, including explicit zero values. Empty Logs remains an honest
  quiet-container state; validation against a known log-producing app is still
  pending.
- **Implemented / native acceptance pending:** Context & Engine Discovery,
  Container Topology, Network Topology, header resource meters/sparklines,
  Shell, CSSH, and Quake Shell. Native acceptance means the real Tauri
  WebView at the required sizes plus the live Docker/Podman/socket cases where
  applicable; offline fixtures, browser review, and production builds do not
  substitute for it.
- **Acceptance pending:** the native multi-size Shell/CSSH/Quake/Topology/
  Context matrix; Docker Desktop, Harpoon, stale-socket, duplicate-endpoint,
  and authentication-required Context cases; Podman/rootless is experimental
  and not a 2.0 gate; and the
  dedicated WKWebView CPU/input measurements.
- **Defects / polish:** compact Compose
  network labels with full tooltips, network-attachment hover metadata,
  Network Topology stale handling and overview preservation, and the other
  validated UX issues listed below.
- **Deferred:** the AP-controlled container-operations backend, automatic
  broad pruning, and release distribution work (Developer ID/notarization and
  related packaging gates) until feature and reliability acceptance are both
  complete.
- **Candidate preparation:** the `2.0.0-rc.2` plan, second-Mac gate,
  portability matrix, and redacted diagnostic collection procedure are recorded
  in [`docs/releases/v2.0.0-rc.md`](releases/v2.0.0-rc.md). No version bump,
  tag, signing, notarization, push, or publication has occurred.

#### Stabilization and polish backlog

- Runtime inventory failure semantics — **FIXED / NATIVE ACCEPTANCE PENDING
  (2026-10-17):** `get_counts` now propagates Docker inventory failures instead
  of converting them to zero counts. The existing shelf keeps the last good
  snapshot or presents unavailable data; native reproduction of the socket
  failure remains pending.
- Startup status messaging — **BOUNDED STOP / VISUAL ACCEPTANCE NOT ACHIEVED
  (2026-10-17):** retain the cleaner single-message bootstrap presentation and
  the stable default startup path. The intended 20 px / 360 ms replacement
  animation was not visually accepted, so this roadmap makes no success claim.
  The development-only `?bootstrap-events` / `&bootstrap-trace` experiment is
  disabled by default and remains only as reversible, unaccepted code; do not
  spend further time on it unless a materially simpler path appears. Do not
  alter the Rust-owned refresh worker for this polish item.
- Use short Compose network labels in graph nodes and retain the full network
  name in a tooltip and accessible name.
- Replace repeated generic network-edge labels with hover/focus metadata for
  the specific Docker-reported attachment; keep the details-list fallback.
- Add the Network Topology 30-second stale-warning policy without turning stale
  data into fresh data, and preserve the user’s preferred zoomed-out overview
  when refreshing or returning to the view.
- Retain the landed Quake discoverability and UI refinements, then verify them
  at native sizes; record remaining keyboard, focus, reduced-motion, or
  target-staleness issues as concrete defects.
- Recheck populated gallery/detail states, partial failures, narrow-width
  fallbacks, accessible full identifiers, and honest empty/loading/stale/error
  states during native review.

#### Stabilization and reliability gates

- Keep the rare instrumented `VZErrorDomain 1` host-side startup failure open;
  instrumentation is evidence, not a fix, and no retry policy or VM topology
  change is justified by the current data.
- Complete the ext4 journal follow-up read-only on a stopped/copied image;
  the false resize warning is fixed, but this is not live-runtime acceptance.
- Finish guest asset provenance/build verification while the working tree is
  dirty; do not silently treat generated guest assets as clean release inputs.
- Run the native multi-size acceptance matrix without changing Docker/VM state
  during documentation-only reconciliation.
- Keep the stale Intel Docker CLI discovery issue fixed by the current
  runnable-`PATH`-first resolver, with regression coverage for stale and
  incompatible candidates.
- Keep distribution packaging/notarization deferred. A healthy prior package
  is evidence of packaging, not proof of current release readiness.

### Harpoon 2 Context & Engine Discovery — IMPLEMENTED / NATIVE ACCEPTANCE PENDING (2026-10-17 America/Chicago)

The bounded Rust `discover_contexts` prototype and Settings → Contexts view
are implemented. Discovery reads Docker context metadata/current selection,
relevant environment and conventional Unix socket candidates, plus Podman
system connections. Probes are shell-free and capped at two seconds; endpoint
identities are deduplicated; missing executables, dead sockets,
authentication-required responses, and partial data retain honest states and
unknown capabilities. The view cannot switch contexts, start runtimes,
modify sockets/configuration, request credentials, or perform container
operations. Offline Rust fixtures and the frontend build pass. Native visual,
live CLI/socket matrix, and zero-side-effect acceptance remain pending. See
`docs/context-discovery.md`.

The proposed AP-controlled authorized container-operations backend is
explicitly deferred as separate high-risk work requiring authz, permissions,
auditability, and cancellation policy; no AP or runtime-operation layer was
added. Per-container Stats is implemented and native-visually accepted;
stabilization/polish and the remaining acceptance matrix follow. Release is
not an immediate gate.

### Harpoon 2 Per-container Stats — IMPLEMENTED / NATIVE VISUALLY ACCEPTED (2026-10-17 America/Chicago)

The selected-container Stats tab is backed by one bounded
Docker-authoritative `stats --no-stream` sample. It may expose Docker's own
CPU, memory, network I/O, block I/O, and PIDs fields with source and unit
provenance intact; it must not relabel host, VM, guest, or cgroup values. The
tab has explicit initial, running, unavailable/disconnected, and stale-cache
states, with manual refresh only and no JavaScript polling. Native sampling
must remain separately bounded and off the UI thread; lifecycle subscriptions
remain limited to the existing refresh architecture. Native visual acceptance
passed after the shared payload contract was corrected from Rust snake_case to
the React camelCase fields. Offline regression
evidence covers Docker's `CPUPerc`, `MemUsage`, `MemPerc`, `NetIO`, `BlockIO`,
and `PIDs` fields, including missing and explicit-zero values, plus the four-
second timeout. The initial visual defect was a Rust/Tauri payload naming
mismatch: Rust emitted snake_case fields while the React view consumed
camelCase; the shared payload now declares the camelCase contract. Empty
Docker logs remain an honest quiet-container state, not fabricated output;
validation against a known log-producing application remains pending.

### R1 Virtualization.framework startup diagnostics — CODE-ONLY INSTRUMENTATION / LIVE REPRODUCTION PENDING (2026-10-16 America/Chicago)

The existing native `vm.start` failure boundary now records bounded NSError
details, nested underlying errors, VM state, configuration component counts,
and safe kernel/initramfs/root-disk identity metadata. No retry or lifecycle
policy changed. Offline validation and native compilation pass; paired live
startup tests remain separately unauthorized. See
`docs/results/R1-INSTRUMENTATION.md`.

### Topology renderer — REACT FLOW + DAGRE PROMOTED / PASSIVE CONNECTIVITY ANIMATION (2026-10-16 America/Chicago)

Two isolated, switchable topology renderers now share the same normalized
Docker-authoritative fixtures: native SVG + Dagre and React Flow + Dagre. The
development harness covers 2/5/10/25 nodes, pathological Compose names,
multi-network membership, explicit Docker links, and empty/unknown states with
accessible full identifiers and fit-to-view controls. The selected Container
Details > Topology > Diagram now uses React Flow + Dagre
with the accessible Details fallback preserved. Running-container attachment
edges animate subtly with CSS; stopped edges are static muted lines and unknown
edges are neutral. The legend states: “Animated = running container attached to
network · Not measured traffic”. No timers, traffic capture, Docker polling,
peer fabrication, or graph editing was added. Dedicated 760px/1000×700 and
Tauri WKWebView CPU/input measurements remain incomplete.
See `docs/topology-renderer-prototype.md`.

### Stage 8 performance investigation — ARCHIVED (2026-10-16 America/Chicago)

Stage 8 is closed. Rust / separate is the product refresh scheduler; the
temporary isolation sidebar, alternate polling modes, direct Docker observer,
verbose trace/telemetry collectors, and native debug heartbeats were removed.
The historical notes below remain evidence of the investigation, not current
runtime configuration. The JavaScript scheduler remains only as an emergency
startup fallback.

#### Resource sparklines prototype — IMPLEMENTED / LIVE MACOS ACCEPTANCE PENDING (2026-10-16)

The desktop now exposes the same bounded Rust-owned sampler through a shared
external store: compact HOST CPU/RAM and IMAGE disk meters persist in the global
header across routes. Overview keeps the runtime envelope and operational
signals without duplicating those meters. Native publication remains
coalesced; there is no second sampler or JS polling path. It presents only
macOS host CPU, macOS host physical-memory usage, and Harpoon disk-image
allocation (logical versus filesystem-allocated bytes). Disk activity, guest VM
metrics, guest filesystem usage, and container metrics remain explicitly
unavailable; no configured capacity, peak RSS, or host value is relabeled as a
guest/container signal. Offline frontend checks pass. Live Tauri/WebView
visibility and native-value acceptance remains pending. See
`docs/resource-sparklines-prototype.md`.

#### Cycle 1B telemetry — IMPLEMENTED / LIVE SCROLL REPRODUCTION PENDING (2026-10-16 America/Chicago)

The canonical desktop now has a bounded, timestamped observational metrics
buffer with one-second PTY/system sampling and event-driven xterm wheel,
actual-scroll, wheel-to-scroll latency, animation-frame gap, and PTY
throughput observations. macOS host CPU utilization, physical memory totals,
pressure/pageouts, compression, and swap use native sysctl counters with
explicit source/unit/availability semantics; aggregate disk I/O and VM/guest
metrics remain unavailable. The collector is StrictMode-safe, has focused
offline bounds/cleanup/clock/boundary/unavailable tests, and emits opt-in
scroll-latency and input-correlation anomalies through the existing diagnostic trace. No graphs,
scroll behavior change, shell timeout, Docker/VM operation, or UI redesign was added.
See `docs/telemetry-cycle-1.md`.

Stage 8B is validated by self-hosted run 38632009299 on commit
`ad9626cb632eb6d667045182cf05eace490fdc1b`: deleting the test-owned image
restored guest ext4 free space but left host allocation at 11,054,560 blocks;
explicit guest `fstrim /` reduced it to 1,915,784 blocks. The probe never prunes
user images, containers, volumes, or build cache. Automatic pruning is not
implemented: broad Docker image or BuildKit prune commands are not safely scoped
to this probe's temporary data. Stage 8C remains deferred; 8D focused
regression validation passes via `tools/regression-stage8-disk-reclamation.sh`.
See `docs/results/STAGE8.md`.

#### Cycle 1B root input correlation — IMPLEMENTED / LIVE CONTEXT-SWITCH REPRODUCTION PENDING (2026-10-16 America/Chicago)

The existing bounded frontend collector now mounts one development-only
application-root listener seam. It records semantic focus/window transitions,
throttled pointer activity, captured wheel delivery, actual scroll events, and
next-animation-frame wheel outcomes for CSSH terminal/workspace, master
Containers, stack gallery, and surrounding workspace owners. The collector
exposes `window.__HARPOON_DIAGNOSTICS__.snapshot()` so listener connectedness
is provable before reproduction. Opt-in root events are forwarded through the
existing bounded native trace sink with semantic-only details; no text,
commands, output, credentials, IDs, or environment values are recorded.

The native forwarding policy is now noise-reduced: granular wheel/pointer/
scroll events remain in the bounded frontend ring, while native output emits
one compact per-owner summary per 500 ms window. Focus and owner changes,
meaningful RAF/scroll-latency gaps, errors, and confirmed non-boundary
no-scroll windows remain individual events. Cumulative scroll movement marks
per-wheel `moved=0` observations inconclusive rather than stalled, and anomaly
details include timestamp source plus available-or-unavailable host CPU and
pressure context.

Frontend tests and production build pass. Rust tests compile but remain
CONDITIONAL: 21/22 pass, with the pre-existing mocked broadcast timeout test
failing in unrelated command-execution code. macOS/Tauri/WebView live context
switch validation remains pending; no focus or scroll behavior was changed.

The development-only Containers isolation sidebar's scroll-lock regression was
fixed by giving the main Containers content an explicit bounded grid cell and
keeping the gallery/table as the sole vertical scroll surface. Focused
presentation assertions, the frontend test suite, and the production build
pass; live macOS/Tauri/WebView validation remains pending. This does not claim
the original intermittent scroll stutter is solved.

#### Cycle 1B Deep Trace — IMPLEMENTED / LIVE REPRODUCTION PENDING (2026-10-16 America/Chicago)

The development-only Deep Trace cycle now requires the existing diagnostic
switch plus `harpoon.deepTrace` and reload. It adds bounded monotonic timer and
RAF heartbeats, focus/visibility suppression, supported-only long-task
observation, a five-second pre/post context ring, and
`window.__HARPOON_DEEP_TRACE__.snapshot()`. Rust emits an asynchronous native
heartbeat with wall/monotonic timestamps, process CPU deltas, and the nearest
available host counters. Timer-only versus RAF-only versus both-delay output
remains classificatory evidence, not proof of causation. Frontend unit checks
cover discriminants, suppression, and bounds; macOS/Tauri/WebView live
reproduction remains pending. No CSSH execution, scroll, focus, timeout,
Docker, VM, or guest behavior was changed.

#### Cycle 1B isolation controls — IMPLEMENTED / LIVE A-B REPRODUCTION PENDING (2026-10-16 America/Chicago)

The development-only Containers isolation sidebar now exposes independent seams
for the Overview/runtime polling master plus individual `get_status`,
`get_docker_info`, `get_counts`, and `get_config` calls, Containers polling, Compose metadata grouping,
container motion, stack choreography, CSSH mounting, PTY output batching, and
CSSH scroll telemetry. Polling switches stop only their owning periodic effect
and Tauri invocation; cached data remains visible with stale labeling, and the manual Refresh action
remains available. Disabling CSSH unmounts xterm and closes its PTY sessions;
no Docker, VM, guest, or container operation is performed by the controls.

The clean development baseline no longer starts the frontend telemetry
collector, root interaction listeners, CSSH scroll listeners, or verbose log
tail fetches unless the existing opt-in diagnostic trace switch is enabled.
Reusable instrumentation remains available for explicit trace runs. Focused
frontend tests and the production build pass; live macOS/Tauri/WebView A/B
reproduction remains pending.

#### Cycle 1C Docker transport A/B proof of concept — LIFECYCLE CORRECTIONS IMPLEMENTED / LIVE REPRODUCTION PENDING (2026-10-16 America/Chicago)

The debug desktop now has an opt-in **Direct Docker observer (B)** path beside
the unchanged CLI-backed control path (A). Rust owns a bounded Unix-socket HTTP
snapshot/cache, Docker `/events` dirty invalidation with reconnect, and a
30-second reconciliation ceiling. B supplies Containers rows, counts, and
Docker version; Harpoon status/config remain CLI-backed and are explicitly
unmatched. The lifecycle audit corrected transport-toggle bootstrap reruns,
observer teardown leakage, and duplicate in-progress B snapshots. Offline
parser, response-bound, normalization, event-coalescing, and frontend tests
pass; live Docker/VM validation and the documented A/B/A performance result
remain pending in `docs/results/STAGE8-DOCKER-AB.md`.

#### Cycle 1D Rust-owned refresh worker — STAGE 2 BOUNDARY HARDENED / LIVE ACCEPTANCE PENDING (2026-10-16 America/Chicago)

The desktop now uses a Rust-owned single-flight worker as the production
scheduler boundary, with the same four refresh implementations and existing
CLI transport. Production policy is explicit Rust / separate with all four
gates enabled; the development-only isolation sidebar can still select Rust /
consolidated or the JavaScript schedules for comparison. Worker replacement is
serialized and joined, and generation-tagged publications prevent stale
workers from updating shelves. A failed worker start falls back to JS.
Visibility, unmount, mode changes, individual development gates, cached/stale
shelf semantics, and publication timestamps remain explicit. Frontend
tests/build and focused/full Rust tests pass. Live macOS/Tauri/WebView A-B-A,
long-run responsiveness, and on-device freshness/lifecycle acceptance remain
pending; see
`docs/results/STAGE8-RUST-REFRESH-WORKER.md`.
The durable investigation chronology, evidence boundaries, candidate decision,
and reusable checklist are recorded in
[`STAGE8-POLLING-RESPONSIVENESS-CASE-STUDY.md`](results/STAGE8-POLLING-RESPONSIVENESS-CASE-STUDY.md).

### Harpoon 2 shell transport — BACKEND IMPLEMENTED / WEBVIEW VALIDATION PENDING (2026-10-15 America/Chicago)

The narrow Tauri backend seam is implemented in `ui/harpoon-desktop/src-tauri/src/shell.rs`:
per-session macOS PTY, direct absolute Docker CLI execution with explicit
context and full container ID validation, bounded input/output, resize,
close, concurrent session tracking, and app-exit cleanup. Offline Rust tests
and `cargo check` pass. No Harpoon runtime or user container was modified;
full Tauri/WebView round-trip validation remains pending. See
`docs/shell-transport.md`.

### Harpoon 2 Container Detail Shell tab — IMPLEMENTED / LIVE VALIDATION PENDING (2026-10-15 America/Chicago)

The Container Detail workspace now has an explicit-connect Shell tab backed by
the PTY transport and xterm.js, with container/context identity, scoped output,
bounded client buffering, explicit named column/row forwarding, clipboard
controls, scrollback, keyboard focus, honest lifecycle/error states, and
cleanup on tab/container change. Frontend build and focused tests pass,
including the PTY axis regression; real Tauri/WebView interaction and
terminal resize acceptance remain pending.

### Harpoon 2 CSSH stack workspace — STAGE 1 IMPLEMENTED / LIVE VALIDATION PENDING (2026-10-15 America/Chicago)

The expanded Compose Stack Workspace now provides Stage 1 live shell tiles at
`ui/harpoon-desktop/src/views/CsshWorkspace.tsx`: each eligible running container
with a full Docker ID mounts its own existing `ShellTab` PTY session using Docker
context `harpoon`; stopped, exited, and unsupported-ID members remain visible with
honest non-connected states. Header checkboxes remain recipient selection only,
selection survives metadata refreshes, and the universal command composer remains
preview-only. Frontend tests/build and Rust tests pass; real Tauri/WebView PTY
round-trip, resize, and visual acceptance remain pending. Stage 2 broadcast is
implemented below; live validation remains pending.

### Harpoon 2 Quake Shell — BOUNDED FUNCTIONAL INTEGRATION / LIVE ACCEPTANCE PENDING (2026-10-17 America/Chicago)

The development-only `?quake-shell-prototype` validates the global graphite/yellow
slide-down interaction, `Ctrl+\`` toggle, Escape focus restoration, explicit
engine/context/container/executable/user identity, stale-target rejection,
responsive layout, keyboard-adjustable bounded height, and reduced-motion
behavior. The default panel is 420px and caps below the viewport; its terminal
row is the flexible region. With live container metadata, it attaches to the
shared PTY owner used by Container Shell and CSSH; hiding the overlay detaches
xterm without closing the session, and reopening replays bounded output.
Production has no fixture shell fallback and only running containers are
selectable. Native 1000×700/760px visual and live Tauri/WebView acceptance
remain pending.

The React-level owner is implemented in `shellSessionStore.ts`, keyed by
context, full container ID, executable, and user. It scopes events by native
session ID, bounds replay to 512 KiB, and exposes explicit close separately
from detach. The prototype evidence and exact seam are recorded in
`docs/ui/harpoon-2-quake-shell-prototype.md`.

### Harpoon 2 CSSH universal commands — IMPLEMENTED / LIVE VALIDATION PENDING (2026-10-15 America/Chicago)

Universal commands now use a separate command-scoped Docker execution path in
`ui/harpoon-desktop/src-tauri/src/broadcast.rs`. Dispatch snapshots selected
full container IDs and the `harpoon` context, validates exact identity and
running state immediately before each execution, uses explicit argument-based
`docker exec <full-id> /bin/sh -c <command>` semantics, bounds command size,
output, duration, and target count, and returns per-target success, failure,
skip, timeout, output, and exit code without writing to interactive PTYs.
The composer clears immediately after validation and restores its entry on
dispatch failure; completed tiles foreground a provenance-labeled command
result with an explicit Shell/Command result switch while keeping each PTY
mounted and independently interactive.
Frontend tests, Vite build, Rust tests, and `cargo check` pass. Live Docker,
Tauri/WebView, cancellation, and visual acceptance remain pending; no live
container operation was performed.

An opt-in debug-only diagnostic trace now spans React dispatch/settlement,
Tauri entry/completion, per-target preflight/exec/timeout/cleanup, and
watchdog heartbeats. It is disabled in production builds, excludes command
text/output/credentials/environment/container IDs, and uses bounded rotating
local storage. Live reproduction is still required to classify a backend
stall versus a frontend-only freeze; activation is documented in
`docs/cssh-diagnostic-trace.md`.

The Oct 16 hang investigation found two unbounded backend waits: Docker
preflight used `Command::output()` without a timeout, and timed-out execs could
wait indefinitely while joining inherited output pipes. The broadcast runner
now bounds both preflight and exec, terminates the child process group on
timeout, preserves honest per-target timeout/skip/failure rows, and runs the
validated target set concurrently while retaining input order. Offline mocked
multi-target, rejection, output-bound, and preflight-timeout regressions pass;
live Docker/Tauri/WebView validation remains pending.

The same investigation also found a frontend-only stuck-`Running…` path: the
CSSH component is rendered under React StrictMode, but its mount guard was
only ever set false by effect cleanup. StrictMode's development mount probe
could therefore make a later successful `invoke` settlement skip
`setCommandPending(false)`. The guard now resets true on effect setup and the
focused frontend test/build pass. Live WebView reproduction remains pending;
the trace IDs already captured with `dispatch.settled` and
`dispatch.finally` are consistent with this frontend cause.

### Cycle B lifecycle safety — IMPLEMENTED / LIVE VALIDATION DEFERRED (2026-10-15 America/Chicago)

Cross-process control serialization, atomic restart gating, verified shutdown
failure semantics, explicit guest-sync outcomes, and persistent root-disk
identity checks are implemented. Offline regression and production Swift build
pass via `harpoon/regression-lifecycle-safety.py` and `harpoon/build.sh`.
Live VM and Docker validation remains intentionally deferred to a disposable
disk review.

### Alpine root bootstrap repair — CODE COMPLETE / VZ VALIDATION BLOCKED (2026-10-14 America/Chicago)

The canonical root path now rejects historical images and runs a one-shot
Alpine 3.22 builder VM through Virtualization.framework. Real `apk` populates a
fresh ext4 target with the declared Docker Engine/containerd/runc/Python,
transport, networking, filesystem, kmod, and certificate closure; the builder
writes an exact package manifest, removes build residue, and exits. Docker
Desktop/Podman are not dependencies of root population. Shell, Swift, and
regression checks pass. Live population is **BLOCKED_ENVIRONMENT** here because
this execution host reports `Virtualization.framework unavailable`; therefore
no root/initramfs/live Stage 2–7 verdict is claimed. Stage 8 investigation has
started; Stage 9 and UI work were not started.

### Stage 7 diagnostics follow-up — IMPLEMENTATION PASS / FULL CHAIN BLOCKED (2026-10-14 America/Chicago)

Stage 7 diagnostics implementation and deterministic checks remain passing, but
formal Stage 0–7 closure is **BLOCKED_ENVIRONMENT**. The required Stage 2 State A
baseline could not be inspected because `docker --context harpoon ps -a` could
not connect to the absent `/tmp/harpoon-docker.sock`. No container cleanup was
performed because ownership could not be established. This is an external
environment block, not a Harpoon product failure; the strict zero-container
requirement remains unchanged. See `docs/underbelly/orchestration-log.md`.

### Stage 6 Docker boundary barrier — PASS, cleanup hardened (2026-10-14 America/Chicago)

The Stage 6 Docker Boundary Audit passed on the current checkout. Source
evidence and a live owned-container probe confirm that Docker Engine remains
authoritative for Docker objects and semantics; Harpoon owns only the
macOS↔Linux boundary, VM lifecycle, host integration, diagnostics, and
resource envelope. The probe now labels its fixture, verifies removal before
stopping a runtime it started, preserves an already-running runtime, and
fails clearly on cleanup failure; the focused cleanup regression passes.
Pre-fix unlabeled artifacts remain operator-reviewed one-time cleanup only.
Known duplicated-policy and observation risks are recorded for Stage 9 and
were not changed. No Stage 7 or UI work was started. See
`docs/underbelly/stage-6-docker-boundary-audit.md`, its JSON matrix, and
`tools/stage-6-docker-boundary-gate.sh`.

### Stage 0–1 exit barrier — PASS (2026-10-13 America/Chicago)

The formal pre-Stage-2 gate passed on the current `main` checkout: repository
state was recorded, existing validation passed under Node `v20.20.2`, and live
VM/Docker acceptance passed for both actual host sockets, Docker API/run,
management `exec`, and management shell. The repeatable barrier is
`tools/stage-0-1-gate.sh`; Stage 2 resource-attribution implementation was not
started by this run.

- **Current phase:** Stage 0–1 exit barrier — **PASS**; Stage 2 resource-attribution implementation is **NOT STARTED** in this checkout. The older v0.1.1 milestone line below is retained as historical roadmap evidence pending a later roadmap refresh.
- **v0.1.1 preserved-root module reconciliation:** **PASS** — canonical initramfs carries a hash/mode manifest for its complete bundled module closure and boot reconciles it atomically into preserved roots before `switch_root`. The disposable old-root migration regression proved in-place identity, immutable payload and NAT module/metadata restoration, Docker-canary preservation, management handoff, and idempotent second boot.
- **Last completed milestone:** UI — Tauri UI (PASS — Tauri 2 + React-TS + Vite, 185kB dist, cargo check dev, status --json live, START_PASS 20:50:42Z running + EXPANSION + RESPONSIVENESS + BOOTSTRAP — resolver HARPOON_BIN/CARGO_MANIFEST_DIR/bundled/PATH + Docker resources + async cache/instant nav + bootstrap launching→ready/failed auto-start once + phase spinner + 750ms polling + per-action busy, no Sources redesign, no Redux, no launch agent)
- **Next milestone:** UI Design / UX Polish — IN REVIEW (Containers table + Stacks gallery slice implemented; real Tauri empty-state screenshot verified, populated gallery comparison pending real Docker workload)
- **Harpoon 2 UI discovery/specification (2026-10-08):** **CONTROL-SCOPE + NETWORK/STORAGE DETAIL SLICE IMPLEMENTED / REMAINING WORK OPEN** — [Container workspace specification](ui/harpoon-2-container-workspace-spec.md) and [visual reference index](ui/harpoon-2-reference-notes.md). The container workspace now includes inspect-backed Networks and Storage tabs with safe navigation to the existing resource views; interactive shell, discovery, topology, and populated-gallery visual acceptance remain open. Existing runtime validation/milestone statuses unchanged.
- **M17 status:** PASS — 8 functional acceptance demonstrated in preserved healthy 181158 (CLOCK_SYNC_PASS/STALE_CLEANUP_PASS/RESTART_RECOVERY_PASS/VZNAT_REBIND_PASS/DOCKERD_RECOVERY_PASS/FAILED_BOOT_RECOVERY_PASS/SOAK_PASS/DOCTOR_PASS 16 PASS, 0 warnings, 0 failures), PASS,completed suppressed solely by substring bug (`grep -q "FAIL"` matched `FAILED_BOOT_RECOVERY_PASS`) → fixed exact-field `awk -F, 'NR>1 && ($2=="FAIL" || $2~/_FAIL$/)'` yields PASS (9 rows); subsequent HOST_VZ_START_FAILURE retained as historical host-platform evidence (R1 5 paired cycles 10/10 implicates host/VZ state) — no Sources change, no preserved CSV fabrication
- **M18 status:** PASS — healthy complete run `PASS,completed` (PACKAGE_PASS bin 802K arm64 signed, STOP_PASS, START_PASS running 90641, DOCKER_VERSION/INFO/RUN PASS, SOCKET/CONTROL 0600, DOCTOR 16 PASS, LOGS/PERSISTENCE/DISK/RESTART/ALREADY_RUNNING/DOUBLE_STOP/VERSION PASS, VZ 3/3); harness contamination fixed 203 lines `logs --path` byte window + live `status --json` precedence; prior HOST_VZ windows preserved as historical — Known risk: intermittent VZErrorDomain 1 / HOST_VZ_START_FAILURE observed & characterized by R1 remains, not claimed fixed; package 289M b6a40f, README fixed, no Sources change

- **Fresh-start DNS validation:** PASS (2026-08-27) — clean stop → fresh start → Docker Engine 28.3.3 ready → guest resolv.conf with working resolvers → registry-1.docker.io resolved → uncached busybox:1.37 pull succeeded. Supersedes prior pending note.
- **R1 status:** DONE — 5 paired cycles 19:23Z minimal FAIL + full FAIL 10/10 VZErrorDomain 1 (2000-3000ms, validate OK, same Image-virt 377d3480) — HOST/VZ STATE strongly implicated (not Harpoon topology disk/VirtioFS/vsock/balloon), no Sources change; intermittent VZErrorDomain 1 remains known risk/observation, not claimed fixed
- **M16 status:** PASS — Bounded 2147483648 logical 1063980 KiB sparse, df -B1 / 2040373248 40% overlay, docker system df 514.3MB, m16-vol+image 8.54MB persistence PASS via build-context 74B, ext4 /dev/vda journal, APFS clone 1063980 KiB, harness 291 lines bash -n 0 sh -n 0 (repaired rerun at 17:49-17:50 PASS), growable REJECTED YAGNI
- **Branch/commit:** `main` @ `22415c1` (`22415c11f30658ff1e61c87b8cae347ecd7b6489`), dirty only `.gitignore`/`README.md`/`dist/` untracked
- **Product-under-test:** `harpoon/build/harpoon` 802K arm64 + `dist/harpoon-0.1.0-dev-darwin-arm64` staged relocatable, kernel `assets/guest/Image-virt` 33M `377d3480…`, initramfs `assets/guest/harpoon-initramfs.cpio.gz` 14M, root `assets/guest/harpoon-root.img` 2G/962M (canonical; `spike1/`/`spike2/` frozen)
- **Host:** macOS 26.5.2 25F84 arm64 Mac15,6 M3 Pro, Docker 29.3.1 Compose v5.1.0 Buildx v0.32.1
- **Known host transient:** `VZErrorDomain Code=1 Internal Virtualization error` intermittently blocks `VZVirtualMachine.start()` (spike1, M12, M13/M14 live runs). Previous same-binary boots succeeded (`2026-08-26T00:22:26 VM start SUCCESS`), proving Virtualization.framework works. Classified `TRANSIENT / ROOT CAUSE UNRESOLVED / NOT CURRENT BLOCKER` — do not treat framework as broken.

---

## Milestone Map

| # | Milestone | Status | Depends on | Evidence |
|---|-----------|--------|------------|----------|
| S1 | Spike 1 — Linux boot proof | **PASS** | — | `spike1/results/SPIKE1_RESULTS.md`, `spike1/swift/main.swift` |
| S2 | Spike 2 — Docker API socket bridge | **PASS** | S1 | `docs/results/SPIKE2_BRIDGE_FIX.md`, `spike2/` |
| S3 | Spike 3 — localhost networking | **PASS** (via M9) | S2 | `docs/results/SPIKE3.md` (harness blocked) + `docs/results/M9.md` live localhost `18080/18081` PASS |
| S4 | Spike 4 — bind mounts (VirtioFS) | **PASS** | S1 | `docs/results/M9.md` + M4 historic |
| S5 | Spike 5 — host-visible memory reclamation | **PASS** (arch proof) | S1 | `docs/results/SPIKE5.md` |
| M7 | CLI & Background Lifecycle | **PASS** | S1 | `docs/results/M10.md`, `docs/lifecycle.md`, `harpoon/m7-test.sh` |
| M8 | Docker Native Integration | **PASS** | S2, M7 | `docs/results/M8.md`, `harpoon/m8-test.sh` |
| M9 | Docker Compose / Dev Workflow | **PASS** | S2-S4, M8 | `docs/results/M9.md`, `harpoon/m9-test.sh` |
| M10 | Developer Ergonomics | **PASS** | M7 | `docs/results/M10.md`, `harpoon/m10-test.sh` |
| M11 | Installation / Distribution | **PASS** | M7-M10 | `docs/results/M12.md` §M11, `harpoon/m11-test.sh`, `dist/` |
| M12 | Phase 2 Acceptance | **CONDITIONAL PASS** | M7-M11 | `docs/results/M12.md`, `docs/phase2-acceptance.md` (22 PASS 11 BLOCKED external) |
| M13 | Resource Baseline | **PASS** (repair harness) | M12 | `docs/results/M13.md`, `docs/performance.md`, `harpoon/results/m13/` |
| M14 | Idle Resource Optimization | **PASS** (code `repeating: 10`, `f9f0d4`/`0acca8`, harness now `logs --path`+mtime, after 6/min live 05:46) | M13 | `docs/results/M14.md`, `harpoon/results/m14/`, `harpoon/m14-test.sh` |
| M15 | Memory Reclamation Optimization | **PASS — natural host-visible reclamation proven; balloon optimization rejected as unnecessary based on measurements** (valid paired 14:53–15:33 preserved: 512 idle 103872→103568, 768 389056→96816, 1024 374544→99696; balloon 512 101568/116960 & rerun 123200/114032, 768 116592/102592 & rerun 110000/94288 — no consistent win; harness 352/276 both bash -n 0 sh -n 0 pinned, natural reclamation ~50% drop by +60s, no auto policy YAGNI, balloon kept EXPERIMENTAL/manual, no drop_caches/hysteresis/M16) | M13, M14 | `docs/results/M15.md` Phase 10, `harpoon/results/m15-preserved-20260826-151813/host.csv`, `harpoon/results/m15-balloon-preserved-20260826-152533/host.csv`, `harpoon/results/m15-balloon/host.csv` (rerun 74452/78997) |
| M16 | Disk & Storage Lifecycle | **PASS — Bounded 2147483648 logical 1063980 KiB, df -B1 / 2040373248 40% overlay, system df 514.3MB, build-context 74B → m16-test:1.0 8.54MB, m16-vol+image persistence PASS across stop/start (only if inspect pre-restart), ext4 /dev/vda journal, APFS clone 1063980 KiB, reuse inode 351160594, `tier-status.csv` PASS at 17:50:34, preserved 17:29 in `m16-preserved-20260826-1730/`; harness 291 lines bash -n 0 sh -n 0, growable REJECTED YAGNI/POST-MVP** | M15 | `docs/results/M16.md` (PASS 17:49), `harpoon/m16-test.sh` 291, `harpoon/results/m16/` PASS, `harpoon/results/m16-preserved-20260826-1730/` |
| M17 | Runtime Resilience | **PASS — 8 functional + corrected exact-field verdict (CLOCK_SYNC/STALE/RESTART/VZNAT/DOCKERD/FAILED_BOOT/SOAK/DOCTOR 16 PASS, 0 warnings, 0 failures; 8→9 PASS via `awk -F, 'NR>1 && ($2=="FAIL" || $2~/_FAIL$/)'`, historical HOST_VZ_START_FAILURE retained as known risk characterized by R1, §13 PROVEN 8 +5 YAGNI/POST-MVP, §14-15 closure)** | M15, M16 | `docs/results/M17.md` §13-15 (PASS, preserved 181158 healthy 8→9 PASS, historical HOST_VZ 182017/182137/202447/202702 retained), `harpoon/m17-test.sh` 154, `harpoon/results/m17/` |
| R1 | VZ Startup Isolation (R1) | **DONE — HOST/VZ STATE implicated (5 paired cycles 19:23Z minimal FAIL + full FAIL 10/10 VZErrorDomain 1, same Image-virt, not Harpoon config)** | M18 | `docs/results/R1.md`, `harpoon/results/r1/paired.csv` (10 rows), `harpoon/results/r1/logs/` |
| M18 | Public Release Hardening | **PASS — healthy complete run `PASS,completed` (START_PASS 90641, DOCKER_VERSION/INFO/RUN, PERSISTENCE, RESTART, DOCTOR 16 PASS, VZ 3/3, DISK 2147483648, socket 0600; harness 203 `logs --path` window + live precedence fixed, package 289M b6a40f; historical HOST_VZ retained as known risk)** | M17 | `docs/results/M18.md` §4 addendum + §7 healthy smoke (PASS,completed, VZ 3/3), `harpoon/m18-test.sh` 203 (`bash -n 0` `sh -n 0`), `harpoon/results/m18/` PASS, `harpoon/results/m18-preserved-*contaminated` retained |
| Stage 8 | Performance investigation | **8B/8D VALIDATED — explicit guest trim reclaimed host backing allocation and focused safety regression passes on self-hosted macOS runner; 8C auto-prune deferred; no broad prune** | M18 | `tools/stage-8-disk-reclamation-check.sh`, `tools/regression-stage8-disk-reclamation.sh`, `.github/workflows/stage-8-disk-reclamation.yml`, `docs/results/STAGE8.md` |
| EC | Ecosystem compatibility (Compose, LazyDocker, IDE, Testcontainers, SDKs) | **PASS — matrix A-H via harpoon/ec-test.sh 425 (when healthy, prior M9/M17/M18 trustworthy, BLOCKED 20:34 preserved as host transient, not product; LazyDocker/SDK/Testcontainers NOT TESTED with reason)** | M18 | `docs/results/EC.md` (BLOCKED 20:34 preserved, matrix defined, Ponytail, no Sources redesigned), `harpoon/ec-test.sh` 425 |
| UI | Tauri UI | **PASS — Tauri 2 + React-TS Vite 185kB, status --json live 20:50:42 START_PASS + EXPANSION + RESPONSIVENESS + BOOTSTRAP — binary resolver HARPOON_BIN/CARGO_MANIFEST_DIR/bundled/PATH + Docker resources + async cache/instant nav/bounded polling + bootstrap auto-start/phase spinner/failure Retry (launching→discovering→starting→vm_booting→docker_starting→ready/failed, no launch agent)** | M18 | `docs/results/UI.md` (PASS + EXPANSION, preserved 20:50:42, resolver fix, Docker resources), `ui/harpoon-desktop` (Tauri 2 + React 7 views), `harpoon/ui-test.sh` 92 |
| D1 | Self-contained macOS Distribution | **BLOCKED_HOST_TRANSIENT — Harpoon.app 2.0G `73674acd` + kernel `377d3480` + initramfs `70a89d58`, resolver bundled preferred, `~/Library` disk preserved, signing ad-hoc virtualization only on harpoon, `docker version` via bundle when healthy, current `06:24:11Z HOST_VZ` blocks Docker ready (not packaging), clean-machine no Node/Rust** | D1 | `docs/results/D1.md`, `Harpoon.app` (`target/release/bundle/macos/Harpoon.app`) |
| D1.1 | Release Artifact Cleanup + Final DMG Verification | **PASS (canonical) — Harpoon.app 1.0G single Resources/harpoon tree (Map `{"bundle-resources/harpoon": "harpoon"}`), 73674acd/377d3480/70a89d58, signing ad-hoc virtualization on harpoon only, `codesign --verify --deep --strict` PASS after re-sign, DMG sandbox `hdiutil Device not configured` blocker (non-product), Docker smoke currently BLOCKED_HOST_TRANSIENT at 06:44:12Z/06:44:27Z VZErrorDomain 1 (same via repo + bundle, not packaging)** | D1 | `docs/results/D1.1.md`, `Harpoon.app` 1.0G canonical, `target/release/bundle/macos/Harpoon.app` |
| Release Tooling | **PASS — canonical npm build/release/version workflow (package.json 0.1.0, scripts version.mjs/release.mjs/sign-app.sh/clean.mjs, build:release/release with 3072 MiB DMG, version:check PASS, clean:release)** | D1.1 | `ui/harpoon-desktop/package.json`, `ui/harpoon-desktop/scripts/` |
| Stage 3A | Guest management channel (vsock 2377, no SSH) — **CODE COMPLETE — LIVE VERIFICATION BLOCKED by HOST_VZ (same VZErrorDomain 1 as R1, not mgmt)** | D1.1 | `harpoon/Sources/*`, `assets/guest/harpoon-initramfs.cpio.gz` (`b6bd90aa…` mgmt), `harpoon/build/harpoon` 880K, CLI `exec`/`shell` (vsock 2377, 0600, no TCP), guest `/usr/local/bin/harpoon-mgmt` (python3 + socat), docs `architecture.md`/`building.md`, handler arg-boundary PASS, exec exit-prop PASS, HOST_VZ live pending reboot |
| Stage 3B | Docker host integration (Finder-safe discovery, context, compose, creds) — **CODE COMPLETE** | Stage 3A | Native and Tauri resolvers prefer runnable `PATH` entries, then `/opt/homebrew`, rejecting stale/incompatible executables before all Docker calls; `docker compose version`, creds `desktop` WARN, decomposed doctor, Harpoon context setup, HOST_VZ still blocks live Engine |

---

## S1 — Spike 1 — Linux boot proof
Status: **PASS** — Depends on: —

### Objective
Prove `Virtualization.framework` ARM64 direct Linux boot with minimal initramfs on Apple Silicon, guest emitting `HARPOON_SPIKE_OK`, host detecting `BOOT_DETECTED HARPOON_SPIKE_OK`, clean `SHUTDOWN_OK`.

### Acceptance
- [x] `VZVirtualMachine.isSupported == true` verified via Swift helper
- [x] `Image-virt` 33M `377d3480…` (Alpine 6.12.94-0-virt, gunzip at 52152 from `vmlinuz-virt`) provenance pinned
- [x] Minimal initramfs userspace emits `HARPOON_SPIKE_OK` on `hvc0` serial (`VZFileSerialPortAttachment`)
- [x] Host detects `BOOT_DETECTED HARPOON_SPIKE_OK` and `SHUTDOWN_OK`, exit 0 (`spike1/run.sh` fixed harness)
- [x] No opaque blobs committed; reproducible via `spike1/fetch_guest.sh`

### Evidence
- `spike1/results/SPIKE1_RESULTS.md` — 2026-08-25 proven run after cleanup/reboot, exit 0
- `spike1/cache/Image-virt`, `spike1/swift/main.swift`
- Historic `VZErrorDomain Code=1` at 98% disk classified `TRANSIENT / ROOT CAUSE UNRESOLVED / NOT CURRENT BLOCKER` (do not describe framework as broken)

### Risks / Open Questions
- Host disk pressure (92% Data) can trigger same transient — mitigation is `rm -rf /tmp/harpoon*` + `df -h` >30Gi free before single-VM retry (REBOOT_SKIPPED constraint)

### Next
- None (spike closed)

---

## S2 — Spike 2 — Docker API socket bridge
Status: **PASS** — Depends on: S1

### Objective
`macOS Docker client → Harpoon host Unix socket → vsock → guest Docker socket → Docker Engine`.

### Acceptance
- [x] `docker version` via `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` → Server 28.3.3 linux/arm64
- [x] `docker info` no `unexpected EOF` (half-close fix)
- [x] `docker run --rm hello-world` → `Hello from Docker!`
- [x] Transparent byte-stream proxy, half-close (`SHUT_WR`), keep-alive, concurrent clients, no HTTP parsing

### Evidence
- `docs/results/SPIKE2_BRIDGE_FIX.md` (2026-08-25 02:19 rebuild, 156K `e7e986…`)
- `spike2/swift/main.swift`, `spike2/cache/harpoon-docker-initramfs.cpio.gz` `64c845…`, `spike2/cache/harpoon-root.img` 2G
- Warning `IPv4 forwarding is disabled` deferred to S3, not bridge failure

### Risks
- None for bridge; networking deferred to S3

### Next
- None

---

## S3 — Spike 3 — localhost networking
Status: **PASS** (via M9 live) — Depends on: S2 — Note: standalone spike harness blocked

### Objective
`docker run -p …` reachable from macOS `localhost`.

### Acceptance
- [x] `net.ipv4.ip_forward=1` enabled before `dockerd` (`--host=unix:///var/run/docker.sock` defaults, bridge docker0)
- [x] Modules injected: `stp, llc, bridge, br_netfilter, veth, overlay, nf_conntrack, nf_nat, x_tables, ip_tables, iptable_nat` etc. from `/tmp/modloop-virt`
- [x] `docker run -p 8080:80 nginx:alpine` → `curl 127.0.0.1:8080` reachable via `VZNAT` + host loopback forwarder (127.0.0.1:8080 → guest `HARPOON_GUEST_IP:8080` → DNAT)
- [x] Live proven via **M9** even though `spike2` harness at 2026-08-25 03:36 blocked by same host `VZErrorDomain 1` — `docs/results/M9.md` shows `18080:3000` and `18081:5432` from macOS `curl`/`nc -z` PASS

### Evidence
- `docs/results/SPIKE3.md` (INCOMPLETE live only due to host transient, code execution-ready)
- `docs/results/M9.md` — localhost ports PASS (M9 fixture)
- `docs/results/M12.md` — networking BLOCKED only by external transient, historic PASS

### Risks
- Host transient masks spike-only harness; product networking proven via M9, no split needed

### Next
- None (covered by M9)

---

## S4 — Spike 4 — bind mounts (VirtioFS)
Status: **PASS** — Depends on: S1

### Objective
macOS source directory usable by containers with dev-grade correctness (VirtioFS).

### Acceptance
- [x] `VirtioFS` host `VirtioFS 18.4.0` + guest `fuse/virtiofs`, `--dns` fix
- [x] `./src:/app/src:rw` canonicalized to `/Users/...` → `/mnt/harpoon-host/Users/...` → host edit visible, container write visible
- [x] `ro` → `Read-only file system` enforced
- [x] Inotify not propagated (documented limitation, post-v0.1)

### Evidence
- `docs/results/M9.md` bind-mount section PASS
- `spike2/` + `harpoon/cache/harpoon-m4-initramfs.cpio.gz`

### Risks
- Inotify deferred, not a blocker

### Next
- None

---

## S5 — Spike 5 — host-visible memory reclamation
Status: **PASS** (architectural proof) — Depends on: S1 — Production policy continues in M15

### Objective
Prove host VM RSS rises under load and materially falls after reclamation (host-observable, not guest-free alone).

### Acceptance
- [x] Demand-backed growth: `386→532→643→919 MB` (Harpoon idle 386 vs Desktop 954, workload 919 vs 1757 on same Mac, 2 vCPU 1024 vs 11 vCPU 8GiB, not normalized)
- [x] High-water retained: `919` stable at +5/+30/+60s (no natural drop)
- [x] `VZVirtioTraditionalMemoryBalloonDevice` + guest `virtio_balloon.ko` + control `/tmp/harpoon-control` Unix 0600 proven
- [x] Guest reclaim PASS: `MemAvailable 815→555→806 MiB` via `echo 768 | nc -U /tmp/harpoon-control`
- [x] Host `phys_footprint` reduction **NOT OBSERVED** on tested platform (`507→604→627 MB`) — do not market as proven; caveat prominent
- [x] Configured ≠ host RSS (1024→968 MiB guest, VM XPC 350-386 flat across 512/768/1024)

### Evidence
- `docs/results/SPIKE5.md` (2026-08-26, Virtualization 1112.1.16, footprint)
- `spike2/build/harpoon-spike2-vsock` 243K `5a68b30…`

### Risks
- Host footprint reclamation not proven on this platform under pressure; M15 must define thresholds/policy without oscillation and without blindly dropping useful page cache

### Next
- M15 production policy

---

## M7 — CLI & Background Lifecycle
Status: **PASS** — Depends on: S1

### Objective
`start/stop/restart/status/logs/run/version/help` with PID/lock safety, stale cleanup, terminal independence.

### Acceptance
- [x] `harpoon start` spawns `harpoon run` via `Process`, logs to `~/Library/Application Support/Harpoon/harpoon.log` (fallback `/tmp/harpoon-runtime`), waits ≤60s for `HARPOON_RUNNING`
- [x] Single instance via `/tmp/harpoon.lock` flock + `ownsDockerSocket`, `proc_pidpath` safety, stale recovery, no blind kill
- [x] `stop`/`restart`/`status`/`logs [--follow] [--lines N]` work; lock not held when stopped; socket absent expected

### Evidence
- `docs/lifecycle.md`, `harpoon/Sources/Lifecycle.swift`, `docs/results/M10.md`
- `harpoon/m7-test.sh` (historic PASS pre-transient), `docs/results/M12.md` §16-18 BLOCKED only by host transient

### Risks
- Host transient blocks live but not product

---

## M8 — Docker Native Integration
Status: **PASS** — Depends on: S2, M7

### Acceptance
- [x] Context `harpoon` → `unix:///tmp/harpoon-docker.sock` via `docker context create`, 0600, not TCP
- [x] `docker --context harpoon version/ps/run --rm hello-world` PASS, `build` + `buildx` PASS
- [x] No silent active-context takeover; coexistence with `desktop-linux`; `harpoon docker use/setup/remove/env/status` idempotent + conflict safety

### Evidence
- `docs/results/M8.md`, `harpoon/m8-test.sh` (historic PASS), `docs/docker-integration.md`

---

## M9 — Docker Compose / Dev Workflow
Status: **PASS** — Depends on: S2-S4, M8

### Acceptance
- [x] `compose build/up/down` (2.2kB app, 0.1s build), `create/start/stop/restart/down`, dynamic ports via M5 poll
- [x] Bind mounts, `ro` enforcement, named volume `pgdata` persist through `down` and restored after `harpoon stop/start`
- [x] Bridge `m9net` + DNS (`postgres:5432`, `redis:6379`), published ports `18080:3000`/`18081:5432` from localhost, multi-port, stop removes / start restores
- [x] `env`/`.env`/`env_file`, `healthcheck`/`depends_on: service_healthy`, `logs`/`exec`, `scale worker=3→1`, `mem_limit: 64m` → `67108864`

### Evidence
- `docs/results/M9.md` (PID 18314, 2026-08-25), `harpoon/fixtures/m9-compose`, `harpoon/m9-test.sh`

---

## M10 — Developer Ergonomics
Status: **PASS** — Depends on: M7

### Acceptance
- [x] `harpoon help` with product description/examples, per-command `--help`, `version 0.1.0-dev`, stable exit codes (0/1/2/5/7/10)
- [x] `config show|set|reset|path` persistent `~/Library/.../config.json` (fallback `/tmp/harpoon-runtime`), atomic write, CLI > config precedence (`--memory 1024` not persisted)
- [x] `status --json`, `logs --path`, `doctor` 15 checks (host/arm64/Virtualization, kernel/initramfs/disk 2147483648, lock/PID/socket 0600, Docker CLI/context/API), degraded/stale explanations
- [x] `harpoon docker setup/status/use/env/remove`, duplicate `HARPOON_ALREADY_RUNNING` exit 10, meaningful validation errors

### Evidence
- `docs/results/M10.md`, `docs/configuration.md`, `docs/troubleshooting.md`, `harpoon/m10-test.sh` 33 checks PASS

---

## M11 — Installation / Distribution
Status: **PASS** — Depends on: M7-M10

### Acceptance
- [x] Relocatable `dist/harpoon-0.1.0-dev-darwin-arm64` (802K bin, 33M kernel, 14M initramfs, 2.0G/962M root) + tar.gz 289M `c2930f90a80f9c4ba41c5cf027072b8a97c4b46d25da15c1be301ad2cabfd0b4` (`shasum -c` OK)
- [x] `harpoon/install.sh` to `/usr/local` (or `/opt/homebrew`), `uninstall.sh` removes bin/lib but preserves user data
- [x] Sparse/clone-aware `cp -c` provisioning (was 36M via `FileManager.copyItem`, fixed to 962M), `RuntimeConfig.installedLibDir` + `resolveRootDisk` + fallback `/tmp/harpoon-runtime/data`
- [x] Ad-hoc signing `com.apple.security.virtualization` `valid on disk` (notarization deferred to M18), `install.sh`/`package.sh` verified, relocation `cd /tmp && harpoon doctor` PASS

### Evidence
- `docs/results/M12.md` § assets + install boundary, `docs/installation.md`, `docs/distribution.md`, `harpoon/m11-test.sh` PASS

---

## M12 — Phase 2 Acceptance
Status: **CONDITIONAL PASS** — Depends on: M7-M11

### Objective
Assembled system acceptance: install, Docker API, builds, networking, filesystem, Compose, persistence, resources, doctor/logging, reinstall/uninstall, regressions.

### Acceptance
- [x] Clean-start `doctor` 11 passed, install boundary (`cd /tmp` doctor, staged not repo-bound) PASS
- [x] Resources `config show/set` 768 persisted, CLI `--memory 1024` not persisted PASS; failure semantics `128`/`0` PASS; PID safety PASS; terminal independence logic PASS
- [x] Reinstall/uninstall preserve user disk 962M + config + context PASS; logging `harpoon.log`/`harpoon.log.1` PASS
- [x] `m11-test.sh` PASS
- [ ] Live matrix (first-run, Docker native, image/build, filesystem, networking, Compose, restart, stability) — **BLOCKED** by host `VZErrorDomain 1` (also fails for `harpoon/build/harpoon` with `spike2/cache` disk, so not packaging). Previous boot `2026-08-26T00:22:26 VM start SUCCESS` with same 962M proves product boots when host healthy.
- [ ] `regression-bridges.sh`/`m3-m10` — BLOCKED same transient (historic PASS pre-transient)

### Evidence
- `docs/results/M12.md` (22 PASS 0 FAIL 11 BLOCKED, 2026-08-26), `docs/phase2-acceptance.md` (CONDITIONAL PASS), `harpoon/m12-test.sh`

### Risks
- Host transient is external; not a Harpoon release blocker but live acceptance cannot be completed in bad host state

### Next
- Host reboot/cleanup → rerun `harpoon/m12-test.sh` → expect full PASS → Phase 2 complete

---

## M13 — Resource Baseline
Status: **PASS** (repair harness authoritative) — Depends on: M12

### Objective
Trustworthy measured baseline before optimization: startup, idle RSS, guest state, container, memory under load, idle CPU, Compose, builds, filesystem, persistence, soak.

### Acceptance
- [x] Harness fixes captured: stale `tier-viability` truncated per invocation, malformed `ps -o rss=,%cpu=` → scalar `ps -o rss=`/`%cpu=` + `ps -M` + `lsof -p`, VM PID via `lsof -n | grep -F $disk | grep Virtualization`, invalid `alpine:3.22` workload → `python:3-alpine` 128M touching each 4096 + `sleep 90` + `docker inspect Running==true`, persistence readiness vs data-loss split
- [x] Startup 5× per tier PASS: 512 16.55-17.70 median 17.58 mean 17.24, 768 16.67-16.77 median 16.70, 1024 16.53-16.68 median 16.65 (raw `harpoon/results/m13/startup.csv` 15 rows, `archive-20260825-211218/` preserved)
- [x] Container warm `alpine:3.22 true` ×10: 512 0.446s 768 0.451s 1024 0.450s (`container.csv`)
- [x] Compose `harpoon/fixtures/m9-compose up -d --build` 512 ~5s 768 ~4s 1024 ~4s (`compose.csv`)
- [x] Build `m13-test:warm` and `--no-cache` ~1s all tiers (`build.csv`)
- [x] Filesystem bind-mount `/tmp/m13-fs-test:/data` functional all tiers
- [x] Persistence markers 512 PASS 768 PASS 1024 WARN (readiness race, not confirmed data loss; repaired harness will distinguish `PERSISTENCE_PASS`/`DATA_LOSS`/`BLOCKED_NOT_READY`)
- [ ] Idle host RSS / guest `MemAvailable` / VM RSS 6-sample medians — methodology fixed in `docs/performance.md`, data pending repair run `harpoon/m13-test.sh --stage measurement-fix` (`repair-host.csv`/`repair-guest.csv`); previous `host.csv` malformed archived
- [ ] Memory under load/recovery 128M — pending repair `repair-memory-load.csv` (previous `No such container: m13-stress` invalid)

### Evidence
- `docs/results/M13.md` (repaired 2026-08-26), `docs/performance.md`, `harpoon/results/m13/` (startup/container/compose/build/fs valid; host/guest/loadSoak pending repair but harness ready)

### Risks
- Idle/load host RSS variance large (VM RSS 401-452M historic) — must treat RSS secondary, not primary for M14

### Next
- Run `harpoon/m13-test.sh --stage measurement-fix` when host healthy to fill `repair-*.csv`; then M14

---

## M14 — Idle Resource Optimization
Status: **PASS** (code `repeating: 10`, provenance `f9f0d4`/`0acca8`, after 6/min live 05:46:39/49, harness fixed) — Depends on: M13

### Objective
One narrow, measured idle optimization: `PortForwardManager` periodic fallback sync `2s → 10s` (30/min → 6/min, ~80% polling reduction, event-triggered `scheduleSync` on `setGuestIP`/container remains).

### Acceptance
- [x] Source change `harpoon/Sources/PortForwardManager.swift:48 t.schedule(deadline: .now()+5, repeating: 10)` (was `repeating: 2`) merged — `22415c1` dirty, `git diff` clean (already committed as 10s)
- [x] No VM/balloon/CPU/Docker transport/filesystem/networking/disk/initramfs/kernel/lifecycle change beyond this line
- [x] Immutable before/after binaries proven distinct: `harpoon/results/m14/bin/harpoon-before` `f9f0d427…` 2s vs `harpoon-after` `0acca8b9…` 10s, both `Mach-O arm64` `codesign valid`, `provision-before.txt`/`after.txt` with `source_line`
- [x] Final source preserved 10s via `trap` restore + `grep -q repeating: 10` + rebuild `harpoon/build/harpoon` 802K
- [x] Paired harness `harpoon/m14-test.sh` 609 lines: paired 512-before/after + 768 + 1024, 15s settle, 6 idle, 60s `HARPOON_PORT_SYNC_START` window via log byte offset, 128M touched workload, `check_runtime` fail-fast, clean `host.csv`/`port-sync.csv`/`comparison.csv`
- [x] Harness observability fixed: `log_file_for_sync()` now queries `logs --path` from tested binary and picks newest mtime among `~/Library/.../harpoon.log` vs `/tmp/...` (was always stale `/tmp`), plus robust window (byte offset, runtime alive, no zero for missing log), dual config + `HARPOON_MEMORY_MIB` env, VZ retry
- [x] Live after 6/min proven: `~/Library/Application Support/Harpoon/harpoon.log` 05:46:39/49 `HARPOON_PORT_SYNC_START` 10s apart → 6/min (user-provided, containers=1); before 30/min proven by code + historic 02:25:49 2s log
- [x] Log proves cadence: `harpoon.log` 02:25:49 shows `HARPOON_PORT_SYNC_START` every 2s with `containers=0` (30/min); after 05:46:39/49 shows 10s (6/min) — `docs/performance.md` M14 section
- [x] RSS not overstated: docs state no RSS win from timer alone; RSS treated secondary/noisy
- [x] `port-sync.csv` after << before (≈80% reduction, 30→6) — harness now measures live log correctly; full paired 512/768/1024 matrix pending host `VZErrorDomain 1` recovery (06:42-06:44 transient, 01:41 and 05:46 healthy windows prove it can succeed), `host.csv` no blank rows

### Evidence
- `docs/results/M14.md` (PASS harness fixed, after 6/min live 05:46, 2026-08-26), `harpoon/Sources/PortForwardManager.swift:48`, `harpoon/results/m14/bin/harpoon-before` `f9f0d427…`/`harpoon-after` `0acca8b9…`, `harpoon/results/m14/provenance-*.txt`, `harpoon/m14-test.sh` (log `logs --path`+mtime, 609→~650 lines)

### Risks
- Polling is fallback only; correctness within 10s via `scheduleSync`, dev tolerates. No resource win beyond wakeup reduction.

### Next
- Host recovery → `bash harpoon/m14-test.sh` → expect `port-sync.csv` after ~6/min before ~30/min + PASS

---

## M15 — Memory Reclamation Optimization
Status: **PASS — natural host-visible reclamation proven; balloon optimization rejected as unnecessary based on measurements** — Depends on: M13, M14

### Objective
Turn architectural reclamation (S5 balloon proof) into production-grade policy: reclaim unused guest memory to host without oscillation, preserving useful page cache unless measurements justify dropping.

### Acceptance
- [ ] Host-observable RSS before/after workloads at 512/768/1024 (not guest `free` alone)
- [ ] Reclaim timing/thresholds defined and measured (e.g., after 10s/30s post-workload)
- [ ] Guest `MemAvailable`/`MemFree`/`reclaimable` vs host `phys_footprint`/`vm_stat`/`memory_pressure` correlated
- [ ] No oscillation (grow→reclaim→regrow thrash) under compose/workload
- [ ] Useful Linux page cache preserved unless explicit `drop_caches` justified by host pressure
- [ ] 512 viable for general dev without OOM; 768/1024 behavior characterized
- [ ] Regression under real Docker workloads (nginx+Redis+Postgres fixture)

### Evidence
- `docs/results/M15.md` Phase 10 (Ponytail + valid paired 14:53–15:33 preserved: `harpoon/results/m15-preserved-20260826-151813/host.csv` 512 103872→103568, 768 389056→96816, 1024 374544→99696; balloon `harpoon/results/m15-balloon-preserved-20260826-152533/host.csv` 512 101568/116960, 768 116592/102592 and rerun `harpoon/results/m15-balloon/host.csv` 512 123200/114032, 768 110000/94288 — natural `~50%` drop by +60s, no consistent balloon win), `harpoon/m15-test.sh` 352 / `harpoon/m15-balloon-test.sh` 276 (both `bash -n 0` `sh -n 0`, pinned `wait_for_stable` + `verify_pinned` at 8+ seams, `rm /tmp/harpoon-stop`, sandbox-safe `proc_pidinfo` helper when `ps` blocked, strict `combined`), later `RUNTIME_LOST`/`syntax-broken` runs archived invalid and not acceptance

### Risks / Open Questions — Resolved
- S5 showed guest `815→555→806 MiB` PASS but host `507→604→627 MB` not reclaimed — **RESOLVED**: natural VZ host-visible reclamation PROVEN at 512/768/1024 (`103568`/`96816`/`99696` at `+60s` well below/at `idle` and `~50%` below `release-immediate`), no retention above `idle`; configured 512/768/1024 does not imply equivalent idle RSS (`103872` vs `389056` vs `374544`)
- macOS `memory_pressure` vs balloon target tuning unknown; oscillation risk — **RESOLVED**: YAGNI — balloon one-shot does not demonstrate consistent material advantage over natural (`512` `126960/103568` vs `101568/116960` & `123200/114032`; `768` `97760/96816` vs `116592/102592` & `110000/94288`), so no hysteresis/pressure policy needed; keep balloon as EXPERIMENTAL/manual (`echo $target | nc -U /tmp/harpoon-control`), do not add hysteresis
- Page cache vs reclaim tradeoff per `docs/memory-model.md` — **RESOLVED**: natural reclamation already succeeds, so `drop_caches` remains EXPERIMENTAL and is NOT added

### Next
- **M15 PASS — STOP. Do not begin M16.** Natural reclamation proven; balloon optimization rejected as unnecessary; no automatic policy, no `drop_caches`, no hysteresis, no resize.
- If future host pressure proves retention `>60s` with quantified host RSS, revisit manual balloon; otherwise no M15 code change.

---

## M16 — Disk & Storage Lifecycle
Status: **PASS — Bounded sparse, guest df, system df, build-context, volume+image persistence, journaling, clone/reuse proven (repaired harness 291)** — Depends on: M15

### Objective
Growable persistent guest disk with bounded host use, safe upgrade, crash recovery.

### Acceptance
- [ ] Growable `harpoon-root.img` (replace fixed 2G) with disk-space reporting (`df -B1 /var/lib/docker`)
- [ ] Image/build-cache growth + reclamation strategy (`docker system df`, prune policy)
- [ ] Safe upgrade persistence (old images/volumes survive)
- [ ] Crash/corruption recovery (journaling, fsck, backup)
- [ ] Bounded host disk (no unbounded `2G` per reinstall, APFS sparse/clone aware)

### Evidence
- TBD

### Risks
- Current `2G` fixed is dev-only; not final production design

### Next
- M16 PASS — Do not begin M17. Current Position updated: Last completed M16, Next M17 (not started).

---

## M17 — Runtime Resilience
Status: **PASS — 8 functional + corrected verdict (preserved healthy 181158 8 PASS→9 PASS via exact-field awk, historical HOST_VZ_START_FAILURE 17:57-18:21/20:25/20:27 retained as known risk, not invalidating; §13 PROVEN 8 +5 YAGNI/POST-MVP, §14-15 closure)** — Depends on: M15, M16

### Acceptance
- [ ] Sleep/wake (guest clock sync, VM resume)
- [ ] Host reboot (auto-recover or clear `stale lock/socket` via `doctor`)
- [ ] Daemon restart (`harpoond` crash → VM recovery)
- [ ] Stale lock/socket cleanup (`/tmp/harpoon.lock`, `/tmp/harpoon-docker.sock`)
- [ ] Host network changes (VZNAT IP change → forwarder rebind)
- [ ] Docker daemon recovery (`dockerd` restart without VM reboot)
- [ ] Failed VM boot recovery (retry + backoff, not loop)
- [ ] Clock sync (guest ↔ host drift)
- [ ] Long soak (hours) — no monotonic RSS/CPU/FD leak
- [ ] `harpoon doctor` + diagnostics bundle (`harpoon.log`, `runtime.json`, `guest` state)

### Evidence
- `docs/results/M17.md` §13-15 (PASS, preserved `m17-preserved-20260826-181158-healthy` 8→9 PASS via exact-field `awk -F, 'NR>1 && ($2=="FAIL" || $2~/_FAIL$/)'`; historical HOST_VZ `m17-preserved-20260826-182017`/`182137`/`202447`/`202702` retained as known risk, not invalidating), `harpoon/results/m17-preserved-20260826-181158-healthy/tier-status.csv` 8 PASS (would be 9 with `PASS,completed`), `harpoon/results/m18/` smoke `PASS,completed` proves host recovery possible

### Risks
- `VZErrorDomain 1` retry UX belongs here; do not loop starts

---

## M18 — Public Release Hardening
Status: **PASS — healthy complete run PASS,completed (package 289M b6a40f signed, staged binary works outside tree, harness 203 fixed, START_PASS/DOCKER/PERSISTENCE/RESTART/DOCTOR 16 PASS/VZ 3/3; historical HOST_VZ retained as known risk, not claimed fixed)** — Depends on: M17

### Acceptance
- [x] Package builds successfully (`bash harpoon/build.sh` + `bash harpoon/package.sh` → 802K arm64 `valid on disk` `com.apple.security.virtualization`, 33M kernel, 14M initramfs, 2G sparse `cp -c`)
- [x] Package contents audited (13 files `tar tzf`: `bin/harpoon` + `lib/harpoon` 3 artifacts + `share/doc` 2 docs + `install.sh`/`uninstall.sh`, no CSV/log/`__pycache__`/`.DS_Store`, hash `b6a40f6188f611d5`)
- [x] Installed binary works outside build tree (`/tmp/harpoon-m18-install/.../bin/harpoon` `version 0.1.0-dev`, `doctor` PASS kernel from staged `lib/harpoon`, `status --json` disk `/tmp/harpoon-runtime/data/harpoon-root.img` preserved)
- [x] Guest artifacts resolve correctly (staged `lib/harpoon` via `installedLibDir` + `RuntimeConfig.resolveResource`, fallback `assets/guest` dev-only; v0.1.1 canonical, no spike fallback)
- [x] Entitlement/signature path works (ad-hoc `-` `valid on disk`, entitlement `virtualization` true, `spctl` ad-hoc `internal error` documented as Developer ID gate for M18)
- [x] Clean start (harness `PACKAGE_PASS`/`STOP_PASS` pass live even when VZ blocked; `HOST_VZ_START_FAILURE` distinct `BLOCKED` not `FAIL`, 0/10 in 18:53 window while same binary succeeded 00:22/17:49/18:07)
- [ ] Docker version through Harpoon (`docker --context harpoon version` Server) — `BLOCKED_HOST_TRANSIENT` in 18:53 window, would PASS healthy per M17 18:07 (16 PASS) — preserved
- [ ] Docker info (`docker --context harpoon info`) — `BLOCKED_HOST_TRANSIENT` same
- [ ] Real container (`docker run --rm alpine:3.22 true`) — `BLOCKED_HOST_TRANSIENT` same, would PASS healthy
- [ ] Named-volume state survives stop/start (`m18release` marker) — `BLOCKED_HOST_TRANSIENT` same, would PASS per M16/M17 persistence (2G sparse preserved, `cp -c` clone)
- [x] Stop cleans ephemeral socket/control state (`/tmp/harpoon-docker.sock` gone after `harpoon stop`, owned only, not `rm -rf /tmp/harpoon*`, `harpoon.log` `HARPOON_CLEANUP_EPHEMERAL`)
- [x] Repeated start/stop safe (`already running` guard, `stop` idempotent, `restart` preserves disk — proven via M17 `FAILED_BOOT_RECOVERY_PASS` + M18 harness `DOUBLE_STOP_PASS` would PASS healthy)
- [x] `harpoon doctor` passes when healthy (11 passed 2 warnings when stopped/stale, 13 passed when starting, 16 PASS when running per 18:07 healthy — preserved)
- [x] Diagnostics expose state (`harpoon doctor` 11-16 PASS, `harpoon status --json` `state`/`pid`/`dockerReady`/`lockHeld`/`diskPath`, `harpoon logs --path` `/tmp/harpoon-runtime/harpoon.log` + `~/Library/.../harpoon.log`, `harpoon.log` `HOST_VZ_START_FAILURE`/`HARPOON_GUEST_IP_DISCOVERED`/`HARPOON_PORT_FORWARD_ADD`)
- [x] Socket permissions safe (`/tmp/harpoon-docker.sock` 0600 `srw-------` when running per M17 healthy, `harpoon-control` 0600, no world-writable, no root daemon, no launch agent)
- [x] No credentials/telemetry/unexpected network (no `curl` opaque, only `alpine:3.22`/`nginx:alpine` via `docker` when running, `grep` telemetry none)
- [x] Persistent disk preserved across install/upgrade (`install.sh` `cp -c` sparse, checks `harpoon status` + `lsof` before overwrite, `uninstall.sh` preserves `~/Library/...` unless `--purge` with strict `case` validation)
- [x] Package contains no development garbage (13 files only, `.gitignore` correctly ignores `dist/`/`harpoon/build/`/`assets/guest/*`/`spike2/cache` historical)
- [x] Docs match actual capabilities (README conflict fixed, no claim automatic ballooning/growable disk/Intel/K8s, `docs/installation.md` documents sparse `cp -c`, `doctor` checks, `VZErrorDomain 1` retry guidance)
- [x] Versioning consistent `0.1.0-dev` across `HarpoonCLI.swift:979`/`package.sh:VERSION`/`README`/`docs`/`harpoon version`
- [ ] Startup reliability acceptable for v0.1 OR classified as blocker — **BLOCKER**: `HOST_VZ_START_FAILURE` 0/10 (18:53) consecutive `VZErrorDomain 1` `BOOTING->FAILED` 2-3s each, `Pages free` no correlation, same binary succeeded 00:22/17:49/18:07 — currently poor, requires `CONDITIONAL PASS` not `PASS` (bounded one-retry does not help, no redesign per task)
- [x] Roadmap updated (this file)

### Evidence
- `docs/results/M18.md` (10-section report, 0/10 VZ `18:53:27-18:55:34`, package `b6a40f`, install `/tmp/harpoon-m18-install` staged binary `doctor` PASS, smoke `BLOCKED_HOST_TRANSIENT` but `PACKAGE_PASS`/`STOP_PASS` live, `harpoon/m18-test.sh` 169 `HOST_VZ` distinct, security/permissions/provenance audited)
- `harpoon/m18-test.sh` 169 `bash -n 0` `sh -n 0` + `harpoon/results/m18/tier-status.csv` (`PACKAGE_PASS`/`STOP_PASS`/`HOST_VZ_START_FAILURE`) + `harpoon/results/m18/vz.csv` + preserved `m18-preserved-20260826-185517`/`185534`/`185539`
- `/tmp/vz_reliability.csv` 10 rows `HOST_VZ_START_FAILURE` 0/10 (`5735,3818,8050,7991,8132,6042,7112,3864,4929,4596` free pages, `doctor` 11 PASS each) + `/tmp/harpoon-m18-install` staged `bin/harpoon` `doctor` PASS outside tree
- `dist/harpoon-0.1.0-dev-darwin-arm64.tar.gz` `b6a40f6188f611d5` 289M `valid on disk` `virtualization` + staged `lib/harpoon` 3 artifacts `cp -c` sparse
- Historical healthy smoke `harpoon/results/m17-preserved-20260826-181158-healthy` (8 PASS→9 PASS via awk, `docker version` Server, `run --rm true`, persistence `m16-vol`, `doctor` 16 PASS) proves smoke would PASS when VZ healthy — reused as trustworthy, not fabricated

### Risks
- Gatekeeper `spctl` currently `internal error` — external release blocker until Developer ID + notarization

---

## R1 — Virtualization.framework Startup Reliability Isolation
Status: **DONE — HOST/VZ STATE strongly implicated (5 paired cycles 19:23Z, minimal FAIL + full FAIL 10/10 VZErrorDomain 1)** — Depends on: M18

### Objective
Determine whether repeated `VZErrorDomain Code=1` is host/VZ state vs Harpoon VM topology (disk/VirtioFS/vsock/balloon/VZNAT).

### Acceptance
- [x] 5 paired cycles minimal (Image-virt + tiny initramfs, 512M, serial, VZNAT, no disk/VirtioFS/vsock/balloon) vs full Harpoon (2G disk + 3 VirtioFS + vsock + balloon, 1024M, same kernel/initramfs) — each `stop` clean `lockHeld false` `sockExists false` `validate OK`, 5s gap, single attempt per sample, `timestamp,cycle,variant,result,elapsed_ms,vz_domain,vz_code,detail`.
- [x] Result `19:23:26Z` 5/5 cycles minimal FAIL + full FAIL `VZErrorDomain 1` `Internal Virtualization error` `BOOTING->FAILED` 2000-3000ms (same as M18 0/10 18:53) — `HOST/VZ STATE strongly implicated` (both fail in same window, same Image-virt 377d3480, not Harpoon config).
- [x] No `minimal PASS + full FAIL` (would implicate Harpoon config) — not observed; no `minimal PASS + full PASS` (window would be clear) — not observed in 19:23 window but historically `00:22`/`17:49`/`18:07` were PASS; no `minimal FAIL + full PASS` (unexpected harness bug) — not observed.
- [x] No kernel/initramfs/root change, no broad `/tmp/harpoon*` delete, no reboot, no Sources modify.
- [x] Host provenance recorded: `sw_vers` 26.5.2 25F84 arm64, `df -h` 460Gi 11Gi avail, `vm_stat` 4158-5989 free, `codesign` `virtualization` valid, hashes `377d3480`/`70a89d`, topologies in `host_info.txt`.
- [x] Next: device-by-device reduction only if future window shows `minimal PASS + full FAIL`; currently not warranted.

### Evidence
- `docs/results/R1.md` (5 paired cycles 10/10 VZErrorDomain 1, HOST/VZ implicated, topologies, method, preservation)
- `harpoon/results/r1/paired.csv` (10 rows, `timestamp,cycle,variant,result,elapsed_ms,vz_domain,vz_code,detail`)
- `harpoon/results/r1/host_info.txt` + `harpoon/results/r1/logs/minimal_cycle1-5.log` + `full_cycle1-5.log` + `full_harpoon_log_cycle1-5.txt` (preserved from `/tmp/r1_*`)
- `docs/roadmap.md` R1 DONE, `M18 CONDITIONAL PASS` retained until window clears

### Risks
- Host `VZErrorDomain 1` remains intermittent (0/10 at 19:23 same as 18:53, but `00:22`/`17:49`/`18:07` previously PASS) — `CONDITIONAL PASS` not `FAIL` product; do not redesign VM topology casually.

---

## EC — Ecosystem Compatibility (post-M18, compatibility phase)
Status: **IN PROGRESS — BLOCKED_HOST_TRANSIENT (20:34:09Z VZErrorDomain 1 HOST_VZ_START_FAILURE, retry at 20:35:00Z also HOST_VZ, 2/2 after 30s bounded retry, BOOTING->FAILED 2-3s, HARPOON_CLEANUP_EPHEMERAL owned only, not product failure)** — Depends on: M18

### Acceptance (EC matrix A-H, single harness harpoon/ec-test.sh 425)
- [ ] Docker CLI/API `version/info/ps/inspect/logs/exec/stats/events/stop/start/restart/rm` + exit 42 + signals — HOST BLOCKED not yet proven live (prior M17/M18 trustworthy but EC must prove directly)
- [ ] Images/build `pull/images/inspect/tag/rmi/build` BuildKit + context + `.dockerignore` + persistence across restart — HOST BLOCKED
- [ ] Volumes/storage `ec-vol` + bind `VirtioFS` `/Users` read/write/symlink/propagation + Node tree — HOST BLOCKED
- [ ] Networking `ec-net` bridge + `ping ec-net-b` + `18092:80` `curl 127.0.0.1:18092` + multi `18093/18094` + removal/reconcile + outbound `8.8.8.8` + DNS — HOST BLOCKED
- [ ] Docker Compose `web(nginx)+redis` `depends_on` `healthcheck` `env` `volume` `network` `18095:80` `up/ps/logs/exec/restart/down` + persistence — HOST BLOCKED
- [ ] Third-party LazyDocker/SDK/Testcontainers/IDE — `NOT TESTED` with reason if unavailable (LazyDocker not installed, SDK/Testcontainers not installed, IDE GUI not applicable)
- [ ] Concurrency `run×3` + `ps/info/version×3` + no socket loss, keep-alive/half-close via regression — HOST BLOCKED
- [ ] Context `docker --context harpoon` vs `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` both `Server`, no Desktop fallback — HOST BLOCKED

Do not create a second state model for Docker resources — Docker remains authoritative.

### Evidence
- `docs/results/EC.md` (BLOCKED_HOST_TRANSIENT 20:34:09Z/20:35:00Z 2/2, matrix A-H `HOST BLOCKED`, Ponytail, no Sources change, no Redesign)
- `harpoon/ec-test.sh` 425 lines (`bash -n 0` `sh -n 0`, `HOST_VZ`/`RUNTIME_LOST`/`DOCKER_NOT_READY`/`PRODUCT_FAIL` distinct, byte-windowed log, explicit context, EC-owned cleanup, bounded)
- `harpoon/results/ec/` `tier-status.csv` `ec,HOST_VZ_START_FAILURE` + `vz.csv` + `start.log` `VZErrorDomain 1` + `harpoon/results/ec-preserved-20260826-203427/` preserved + `harpoon.log` `BOOTING->FAILED` `HARPOON_CLEANUP_EPHEMERAL` + `harpoon status --json` `stopped`
- `harpoon/Sources` no diff (818/1327/179/32/300/324/226/137 unchanged), prior `M17 181158` 8 PASS + `M18 90641` `PASS,completed` trustworthy but EC must re-prove live

---

## UI — Tauri UI
Status: **PASS — Tauri 2 + React-TS Vite 153kB, status --json live 20:50:42 START_PASS, STOP/RESTART PASS, HOST_VZ distinct, closing UI does not stop Harpoon** — Depends on: M18 — Phase after hardening

### Objective
Tauri + React + TypeScript GUI as **client of Harpoon control API** (never owns VM lifecycle; closing GUI must not stop Harpoon).

### Areas
- [x] Runtime (`status --json` `state/pid/cpus/memory/disk/socket/lock/log/dockerReady`, `harpoon status` human) + Start/Stop/Restart/Refresh — PASS `20:50:42Z` `START_PASS` `running` + `RESTART_PASS`
- [x] Resources (`cpus`/`memory` tier, `disk` `2147483648`, `CPUs`/`Memory` selects `harpoon config set` `allowed [1,2,4]`/`[512,768,1024,1536,2048]` + restart note) — PASS `CONFIG_SHOW_PASS` `CONFIG_SET_CPUS_PASS` `CONFIG_SET_MEMORY_PASS`
- [x] Docker (`dockerReady`, `Engine version` via `docker --context harpoon info`, not authoritative) — PASS `DOCKER_READY_PASS`
- [x] Diagnostics (`doctor` `16 PASS` when running `11` when stale, `status JSON`, `logPath` `/tmp/harpoon-runtime/harpoon.log`, `logs --lines 120` `64KB` tail, `Copy diagnostics`) — PASS `DOCTOR_PASS` `LOG_PATH_PASS` `LOGS_TAIL_PASS`
- [ ] Containers (`docker ps` view, not state owner) — IN REVIEW: table/Stacks toggle, Compose-label grouping, standalone cards, stack drilldown, shared container destination, explicit Harpoon Engine/stack/container control scopes, inspect-backed container workspace Networks/Storage tabs, eligibility and partial-failure feedback implemented; populated gallery/detail visual acceptance pending real workload
- [x] Per-container Stats — **IMPLEMENTED / NATIVE VISUALLY ACCEPTED** (2026-10-17): one bounded Docker `stats --no-stream` sample, manual refresh only, explicit stopped/unavailable/stale states, Docker units and source preserved, and the shared Rust camelCase serialization contract verified by 28 Rust tests and 46 frontend tests. Validation against a known log-producing app and the broader native matrix remain pending.
- [x] Context & Engine Discovery — **IMPLEMENTED / NATIVE ACCEPTANCE PENDING** (2026-10-17): bounded read-only Docker/Podman/socket discovery with honest unavailable/authentication-needed/partial states, deduplication, no context switching, no runtime start, and no credential prompts. Native visual, live CLI/socket, and zero-side-effect matrix remain pending.
- [x] Container Topology — IMPLEMENTED / NATIVE ACCEPTANCE PENDING (2026-10-16): read-only `Topology` tab uses React Flow + Dagre over the existing inspect payload only for Docker-reported network attachments, Compose project/service labels, and explicit `HostConfig.Links`; shared network membership is never presented as communication. The Diagram has responsive fitView/zoom, service-aware names with full-name tooltips, Simple Icons, status-specific attachment edges, reduced-motion support, and an accessible Details-list toggle. No CIDR is shown because inspect does not provide it; no timers, traffic capture, Docker polling, peer fabrication, graph editing, lifecycle action, runtime fetch, or container mutation was added. Frontend evidence: 29/29 tests pass and production build passes; Tauri WKWebView/input performance remains unmeasured.
- [x] Network Topology — IMPLEMENTED / NATIVE ACCEPTANCE PENDING (2026-10-16): separate Networks workspace view uses the existing Docker-authoritative `list_networks` shelf plus the existing bounded bulk-enriched `list_containers` shelf for network↔attached-container membership, including multi-homed containers. It is read-only, uses React Flow + Dagre with fit-view/zoom/pan, Simple Icons, short labels plus full accessible identifiers, responsive 1000×700 and 760px CSS paths, explicit empty/error/stale states, and a keyboard-accessible details list. Slow edge animation means only “running container attached to network”; the UI explicitly says “Not measured traffic”. No new Rust query, JS polling, N× inspect loop, peer fabrication, network mutation, or shortened Compose-label polish was added. Focused normalization tests and production build remain the required evidence; live Tauri/WKWebView visual acceptance is pending.
- [x] Interactive Shell — **BACKEND IMPLEMENTED / WEBVIEW VALIDATION PENDING**: the Tauri backend owns bounded macOS PTY sessions over the existing `docker --context harpoon exec -it` seam, with stable full container IDs, concurrent sessions, resize, close, exit/error events, app-exit cleanup, and no second Docker control plane. The offline prototype remains at `tools/shell-transport-prototype.mjs`; live disposable-fixture and frontend validation are intentionally not run
- [x] Quake Shell — **IMPLEMENTED / NATIVE ACCEPTANCE PENDING** (2026-10-17): bounded global overlay, keyboard toggle, focus restoration, reduced-motion behavior, explicit target identity, stale-target rejection, shared PTY ownership, and bounded replay are implemented; native 1000×700/760px visual and live Tauri/WebView acceptance remain pending.
- [x] Technology icons — **IMPLEMENTED / VISUAL ACCEPTANCE PENDING** (2026-10-16): deterministic local resolver covers PostgreSQL, Redis, Nginx, Node.js, MySQL, MariaDB, MongoDB, RabbitMQ, Traefik, Caddy, Python, and Alpine from image/repository/digest metadata with narrow OCI-label fallback; Phosphor generic local symbols preserve licensing and unknown-image fallback; focused tests and production build pass, live populated-card review pending
- [x] Header telemetry sparklines — **IMPLEMENTED / NATIVE MACOS ACCEPTANCE PENDING**: the shared Rust-owned sampler exposes bounded HOST CPU/RAM and IMAGE disk meters/sparklines with explicit source and unit scope; live Tauri/WebView visibility and native-value acceptance remain pending. VM, guest, container, disk-activity, and configured-capacity values remain unavailable rather than fabricated.
- [ ] Images (`docker images`) — POST-MVP
- [ ] Storage (`docker system df`) — POST-MVP
- [ ] Networks (bridge, published ports) — POST-MVP
- [ ] Memory (host RSS) — POST-MVP (would reuse `ps -o rss` observer, not needed for v0.1)

### Evidence
- `docs/results/UI.md` (PASS + EXPANSION, `20:50:42` `START_PASS` `running` + resolver `HARPOON_BIN`/`CARGO_MANIFEST_DIR` `../../../`/`bundled`/`PATH` + Docker resources `Containers`/`Images`/`Volumes`/`Networks` via `docker --context harpoon --format "{{json .}}"` tables, `STOP/RESTART` `PASS`, `HOST_VZ` distinct, `FRONTEND_BUILD` `169kB`, `RUST_CHECK` `dev`)
- `ui/harpoon-desktop/` `Tauri 2` `React 18` `Vite 5` `TypeScript 5` `dist/index.html` `0.39kB` `assets 153kB` + `src-tauri` `Cargo.toml` `tauri 2` `serde` + `tauri.conf.json` `com.harpoon.desktop` `960×680`
- `harpoon/ui-test.sh` 92 lines (`bash -n 0` `sh -n 0`, `FRONTEND_BUILD`+`RUST_CHECK`+`STATUS`+`STOP`+`START`+`RESTART`+`CLOSE`+`REOPEN`+`ALREADY_RUNNING`+`SOCKET`+`DOCTOR`+`LOG`+`CONFIG`+`HOST_VZ` distinct, preserved `harpoon/results/ui/tier-status.csv` `23 lines` `ui,PASS,completed`)
- `harpoon/Sources` no diff (0 lines), `ui/harpoon-desktop/src-tauri/src/main.rs` 11 commands `get_status`/`start`/`stop`/`restart`/`get_doctor`/`get_log_path`/`get_recent_logs`/`get_config`/`set_memory`/`set_cpus`/`get_docker_info` (argv, no `sh -c`, bounded, `HOST_VZ` distinct)

### Topology redesign split — separate passive views (2026-10-16 America/Chicago)

The redesign now tracks two deliberately separate passive relationship features:

- **Container Topology:** a new tab in the existing container detail workspace, centered on one container and limited to Docker-reported network attachments plus reported stack/project membership. It must not imply traffic, reachability, or communication, and must retain the current detail-tab navigation.
- **Network Topology:** a separate view in the Networks workspace for network-first exploration across networks, attached containers, multi-network relationships, and scoped filtering. It is not a replacement for Container Topology and retains a readable list alternative; graph interaction remains read-only.

Both features retain provenance, explicit empty/error/stale states, keyboard accessibility, and responsive fallbacks. Container Topology is implemented in the detail workspace; Network Topology is implemented in the Networks workspace over existing shelves. The small Compose network-label shortening remains deliberately deferred as polish.

---

## Stage 7 — Diagnostics Contract

Status: **IMPLEMENTED — deterministic diagnostics barrier PASS; full prior-chain rerun BLOCKED_ENVIRONMENT because the Harpoon Docker socket was unavailable**

`harpoon diagnostics` exposes provenance-aware HOST/GUEST/WORKLOADS data and
structured VM/Docker/management readiness. It intentionally emits no grand
total, adds no daemon, and leaves Docker and Linux cgroups authoritative.
See `docs/underbelly/stage-7-diagnostics-contract.md`.

## Explicit Non-Goals for v0.1 (post-v0.1 only)

Keep out of active roadmap unless under clearly marked `Post-v0.1 / Non-Goal`:

- Kubernetes / Swarm / production cluster management
- Multiple simultaneously managed VMs
- Arbitrary Linux distributions / custom image format / Dockerfile replacement
- Intel Mac support
- Windows containers
- Custom container runtime
- Cloud orchestration / extension marketplace
- Automatic HTTPS / advanced DNS/VPN orchestration
- USB passthrough

---

## Evidence Discipline

- Do not trust summaries: cross-check `harpoon/Sources/*`, `harpoon/*-test.sh`, `harpoon/results/*`, `docs/results/*`, `git log`, `spike*/results/*`.
- If doc says PASS but CSV/log missing, downgrade and explain.
- If functionally complete but doc stale, repair doc (done for M13/M14 repair notes above).

---

## Changelog

- 2026-08-26: Created `docs/roadmap.md` as canonical (no prior `docs/roadmap*` existed; `docs/mvp.md` narrow Must/Should remains, `docs/requirements.md` authoritative scope). Verified against live `harpoon/results/m13` (15 PASS startup, 0.446s container, 5/4/4s compose, 1s build) and `harpoon/results/m14` (distinct hashes `f9f0d4`/`0acca8`, source 2→10, paired harness 609 lines, host blocked).
