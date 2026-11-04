# Harpoon Deployment Control Plane — Requirements

Status: DRAFT v2 (reviewed 2026-10-30; supersedes the 2026-10-29 draft)
Date: 2026-10-30

This document is the authority for milestone **scope, acceptance criteria and stop conditions**.
Which milestones are complete, and which is next, is tracked only in
[the roadmap](roadmap.md#deployment-control-plane-harpoon-21--canonical-milestone-tracker).

This initiative adds a small Harpoon-native deployment control plane to one local
Harpoon runtime. It takes **developer-experience** ideas from Vercel and
**orchestration lifecycle** ideas from Coolify. It does not fork or embed either, does
not create a second container engine, and does not claim Vercel runtime or
infrastructure parity. See [the Vercel UX spike](results/SPIKE_VERCEL_UX.md) and
[the Coolify spike](results/SPIKE_COOLIFY_DEPLOYMENT.md). The architectural decisions
are in [ADR 0004](decisions/0004-deployment-control-plane-boundary.md).

Product statement: a portable, local-first, Vercel-like developer experience built on
Harpoon's Docker-compatible runtime. Harpoon helps developers build, deploy, observe,
troubleshoot and manage applications locally **without hiding meaningful differences
between local and production environments.**

## Principles

1. **Docker is authoritative** for containers, images, networks, volumes and health.
   The control plane owns deployment *intent*, configuration, execution records and
   history, and nothing else.
2. **Evidence, not guarantees.** A health check is evidence that a local container
   responded. It does not show the application will behave the same on Vercel or any
   other environment, and the UI must say so.
3. **Explicit operations.** Retry, rollback, promotion and redeploy are named,
   developer-initiated operations. Nothing completed is rolled back, deleted or promoted
   silently. The one automatic action is the bounded cutover recovery of D5, which only
   undoes Harpoon's own interruption and always reports its outcome.
4. **Report interruptions.** Any downtime required for replacement is announced before
   confirmation and recorded afterwards.
5. **Smallest thing first.** No daemon, provider framework, registry, queue service or
   adapter layer until a measured need exists.

## Approved decisions (2026-10-30)

- Standalone deployment engine in **Rust**, CLI first; Tauri integration later.
- **SQLite** for deployment metadata, history and configuration.
- **macOS Keychain** for secret values; SQLite holds references only.
- **Host-side** execution using Harpoon's Docker API/CLI capabilities.
- No mandatory Redis, PostgreSQL, Kubernetes or additional container runtime.
- No additional background daemon until unattended deployment requirements justify one.
- Brief, documented deployment interruptions are acceptable in the first implementation.
- Deploying *to* Vercel or other providers is an **open future possibility**; it is
  neither committed nor excluded (see Deferred decisions).

## Product boundary

Harpoon remains a Docker-compatible local runtime. The control plane targets **one
Harpoon runtime** on the same Mac. Multi-server orchestration is out of scope.

## Terminology

- **Project** — a named group of environments and applications.
- **Environment** — a configuration scope inside a project: `development`,
  `preview`, `production` (see D1). All environments run on the same local runtime.
- **Application** — one deployable unit with one source and one strategy.
- **Deployment** — one attempt to build/start/replace an application (a record).
- **Release** — the immutable result of a *successful build*: an image reference plus
  a configuration snapshot (variable names, scopes, secret versions, runtime
  settings). Rollback and promotion operate on releases.
- **Current release** — the release a running application instance was started from.
- **Cutover** — the window in which Harpoon stops a healthy release to start its
  replacement. **Cutover recovery** is the bounded attempt to restore the previous
  release when that replacement fails (D5). It is distinct from **rollback**, which is
  a developer-initiated return to a retained release after a deployment has completed.
- **Attended** — a deployment run while a Harpoon process the developer started is
  present: a CLI command, a foreground `watch` process, or the open desktop app.
- **Unattended** — a deployment started with no developer-run process present, which
  requires a background agent and credentials that remain available without the
  developer (see Gate U).

## Milestones

Each milestone below lists prerequisites, acceptance tests, failure scenarios,
measurable stop conditions and exclusions. A milestone is complete only when its
acceptance tests pass and no stop condition has triggered. Milestones are reviewed
before the next begins.

### Milestone A — Specification and UX discovery (this document set)

- **Prerequisites:** none.
- **Deliverables:** the Vercel UX spike, this requirements revision, ADR 0004 update,
  roadmap and status updates.
- **Acceptance:** the documents agree with each other; every contradiction found in the
  2026-10-30 review is resolved or listed under *Deferred decisions and open
  questions*; the owner approves.
- **Failure scenarios:** a requirement depends on infrastructure Harpoon lacks (for
  example inbound webhooks on a laptop) without a stated alternative.
- **Stop condition:** an unresolved disagreement about the boundary, language, store or
  secret model blocks Milestone B.
- **Exclusions:** no code, no runtime or guest changes, no change to the 2.0 release
  work.

### Milestone B — Deployment engine and lifecycle

- **Scope:** Rust crate (library + CLI): state store, deployment state machine,
  stable identifiers and minimal Docker labels, per-application serialization,
  interrupted-operation marking, image deployment (pull/run), health checks, release
  records, explicit retry/rollback/promote, history queries, and local-directory
  Dockerfile builds *without secrets* (the first vertical slice).
- **Prerequisites:** Milestone A approved; baseline measurements (below).
- **Acceptance:**
  1. A local directory with a Dockerfile builds in Harpoon's Docker Engine, starts with
     non-secret runtime variables, and passes a preliminary health check **before** the
     previous release is replaced.
  2. Replacement reports the interruption beforehand and records its measured
     duration.
  3. If the new release fails its preliminary check, nothing was stopped: the previous
     release keeps running, the failed release is retained for inspection, and the
     output lists explicit actions (retry, logs).
  4. If the cutover has already begun and the replacement fails to start or fails its
     final check, Harpoon makes one bounded attempt to restore the previous release
     (D5), records and displays the outcome (`restored`, `restored_unverified`,
     `restore_failed` or `not_attempted`, with the reason), retains the failed release,
     and leaves retry, promotion and rollback decisions to the developer.
  5. `rollback` restarts a named earlier release with its pinned configuration and
     states that data and schema are not rolled back.
  6. History, logs and statuses survive a restart of the CLI and a `kill -9` at any
     step; an interrupted deployment is shown as `interrupted`, never as `running`.
     If the engine was killed during a cutover, `status` says the cutover is
     incomplete and the application may be down, and offers the same bounded recovery
     as an explicit command (D5, D10).
  7. Deleting the store does not affect running workloads; `status` still reports
     labelled Docker resources.
- **Failure tests:** Docker unreachable; VM stopped mid-deploy; port already in use;
  health check never succeeds; image build fails; disk full during build; two
  concurrent deploys of one application; deploy of application A while application B
  is running (B must be untouched). Cutover recovery: replacement fails to start;
  replacement fails its final check; the restoration itself fails; the previous
  release's image or pinned configuration is unavailable; Docker becomes unreachable
  during restoration; the restoration exceeds its time budget; the engine is killed
  mid-cutover and mid-restoration. Pause behavior: an ordinary build failure and a
  failed preliminary check do **not** pause triggers; a failed cutover and a rollback
  do; the pause and reason appear in `status`; events arriving while paused are listed
  as `held` and none is lost; `resume` is explicit and offers deploy-newest, deploy a
  chosen revision, or dismiss.
- **Stop conditions (measurable):**
  - A healthy release is removed or stopped before the replacement's preliminary
    check has passed and before the plan was shown to the developer.
  - A failed cutover with no restoration attempt and no recorded reason, an
    unbounded restoration, or an outcome recorded as `restored` without the previous
    release having been started and checked.
  - A subsequent retry, promotion or rollback performed without a developer action.
  - An unrecoverable state after injected crashes at each state transition.
  - Any resident engine process, or idle cost above 30 MiB RSS or 0.5 % CPU, once
    idle. As a one-shot CLI the engine should have no resident process.
  - Dockerfile build overhead above 1.2× a direct `docker build` of the same context.
- **Exclusions:** Git, webhooks, secrets, Compose, previews, UI, background process,
  zero-downtime routing.

### Milestone C — Builds, environments and secrets

- **Scope:** scoped variables, Config/Secret types, Keychain-backed secret storage,
  secret versioning, build-time versus runtime handling, BuildKit secrets, log
  redaction, environment diff, sensitive-value display rules, Dockerfile build
  overrides (context directory, Dockerfile path, target stage, build args).
- **Prerequisites:** Milestone B accepted.
- **Acceptance:** see D6, including its leakage test matrix. In addition: a planted
  secret never appears in Harpoon-generated logs, records or output, nor in
  `docker history`, image layers, the image's `docker inspect`, SQLite, or crash output.
  It is expected to appear in `docker inspect` of the *running container*, and in any
  log line the application itself writes to Docker's own log store; those exposures are
  documented, not hidden.
- **Failure tests:** Keychain locked or access denied before and during a deploy;
  secret deleted between deploy and rollback; value printed by the application during
  build; secrets of every length including one character; a secret split across output
  chunks and across lines; a multi-line secret; one secret that is a substring of
  another; the redaction component failing; a transformed secret (documented as an
  undetected limitation); a very large variable set.
- **Stop conditions:** any leak of a configured secret into Harpoon-generated output
  or storage; a deployment that proceeds with a missing secret; redaction that
  corrupts the structure of a log (it may mask text, never drop or reorder it); raw
  output stored or shown after the redaction component fails.
- **Exclusions:** automatic framework builds, external secret providers, team
  sharing, Compose.
- **Follow-on gate:** Gate U (below) runs once Milestone C's Keychain work exists, and
  Milestone D cannot begin until Gate U has a recorded decision.

### Gate U — Unattended-execution feasibility (decision, not an implementation)

No milestone may describe Git deployments as "automatic" or "unattended" unless Gate U
has passed *and* the demonstrated behavior below holds. Gate U is a feasibility
investigation with a written result (to be recorded in a new
`docs/results/SPIKE_UNATTENDED_EXECUTION.md`); the background-agent **implementation**
stays deferred until the result justifies it.

- **Prerequisites:** Milestone C's Keychain work exercised with the signing identities
  that will ship (ad-hoc and Developer ID).
- **Questions the investigation must answer with evidence:**
  1. **Credentials.** Can a non-interactive process read the Keychain items it needs
     (secret versions, the deploy key) when the screen is locked, after display and
     system sleep, after logout, after reboot before login, and when the login keychain
     is locked? Which keychain type and access-control settings are required, and do
     they survive rebuilds and re-signing?
  2. **Execution.** Who starts the process, and what must already be running for a
     deployment to proceed: the Harpoon VM and Docker, an awake machine, network
     access, sufficient disk and memory for a build?
  3. **Failure behavior.** What happens when any prerequisite is missing, and in
     particular during sleep, logout, and shutdown or reboot? The requirement is
     **correct behavior, not execution while the machine is unavailable**: Harpoon
     cannot deploy while the Mac is off or asleep. A correct outcome is that nothing
     unsafe happens meanwhile (no credential misuse, no half-applied cutover, no
     deployment with a missing secret), and that on return the missed push is
     detected, recorded with the reason it was not deployed, reported as late, and
     handled according to the developer's policy.
- **Outcomes:**
  - **Pass:** unattended execution is feasible with the stated constraints. A
    background-agent proposal may follow, with its own review and acceptance tests.
  - **Partial or fail:** automatic deployments are documented and presented as
    **attended only** ("while Harpoon is running"), and the UI and documentation must
    not imply otherwise.
- **Demonstration required before any "unattended" claim:** with the agent installed,
  in each of the following the observed behavior must be correct — a deployment that
  proceeds when its prerequisites are met, or is deferred or failed with a recorded
  reason when they are not: screen locked, display asleep, system sleep and wake,
  login keychain locked, logged out, shutdown and restart (including before login), VM
  stopped, Docker not ready, network down. Where the machine is off, asleep or logged
  out, the correct behavior is a clean stop and a correct catch-up, not a deployment.
  A deployment must never proceed with a missing secret, never use plaintext
  fallbacks, and every deferred, failed or late attempt must be visible on next
  attendance.
- **Stop condition:** any scenario in which an unattended run uses a credential it
  should not have been able to read, leaves a cutover half-applied across a sleep,
  logout or shutdown, or misses a push without recording it on return.

### Milestone D — Git integration and automation

- **Scope:** public HTTPS and private deploy-key Git sources (host-side checkout),
  revision recording, host-key pinning, outbound polling trigger, manual and
  CI-invoked triggers, one normalized `repository push` event, a loopback-only webhook
  listener verified against signed fixtures, delivery-ID idempotency, branch filters,
  newest-wins supersede of *queued* requests. **Automatic deployments in this milestone
  are attended only** (a developer-started foreground `watch` process or the open
  app) unless Gate U has passed and its demonstration is complete.
- **Prerequisites:** Milestone C accepted **and a recorded Gate U decision.**
- **Acceptance:** a push to a private repository, detected by polling while a Harpoon
  process is running, produces a deployment with the source revision recorded; the same
  deployment is produced by a signed GitHub fixture and a token-verified GitLab
  fixture; duplicate deliveries do not duplicate deployments; failed clones leave no
  key material or workspace behind; documentation and CLI output describe automatic
  deployment as "while Harpoon is running" and make no unattended claim. A push that
  arrives while no Harpoon process is running is detected on the next run and is
  reported as late, not silently dropped.
- **Failure tests:** wrong host key; revoked deploy key; remote unreachable; clone of a
  very large repository; replayed and tampered webhook bodies; oversized body; two
  pushes while a build is running (the running build finishes, the newest queued
  request builds next, intermediate ones are cancelled — the behavior Vercel
  documents).
- **Stop conditions:** any unsigned or unverifiable event reaching the queue; key
  material remaining on disk after a failure; polling that raises idle cost above the
  Milestone B budget; any need for an always-on public listener; any documentation or
  UI wording that claims unattended operation without a passed Gate U.
- **Exclusions:** GitHub/GitLab Apps, PR comments or commit statuses (they need
  write-scoped provider tokens), relay or tunnel service, any listener bound beyond
  loopback.

### Milestone E — Desktop deployment experience and previews

- **Scope:** Tauri views (deployment timeline, logs, release list, environment diff,
  parity notes, explicit retry/rollback/promote), branch preview deployments discovered
  by polling with TTL and label-scoped cleanup. A background agent is in scope only if
  Gate U passed and the owner approves its implementation; otherwise this milestone
  remains attended-only.
- **Prerequisites:** Milestone D accepted. A background agent additionally requires a
  passed Gate U and a separate owner approval.
- **Acceptance:** every lifecycle operation available in the UI behaves identically to
  the CLI; previews are created per branch, reachable on distinct loopback URLs, and
  removed on TTL expiry without touching unlabelled resources or volumes; the engine
  remains independently testable and unaware of the UI.
- **Failure tests:** preview limit exceeded; branch deleted while its preview is
  building; app closed mid-deployment (the deployment must complete or be marked
  interrupted); Docker restarts mid-preview.
- **Stop conditions:** the UI becomes required for a deployment to proceed; cleanup
  removes a resource lacking Harpoon labels; unattended operation is claimed or built
  without a passed Gate U and the demonstration it requires.
- **Exclusions:** zero-downtime routing unless the baseline measurements justify it,
  PR comments, any multi-user or remote feature.

## Requirements

### D1 — Projects, environments and applications

- A project contains environments; an environment contains applications. Each
  application has one source and one strategy. Identity is a stable generated ID,
  never a Docker name (names may change; IDs are written to labels).
- **Environments are configuration scopes, not infrastructure tiers.**
  - `production`: tracks one configured branch (default: the repository default
    branch); one stable instance per application.
  - `preview`: one instance per non-production branch, with a TTL; may carry
    branch-specific variable overrides.
  - `development`: manual deployments from a local directory; no automatic triggers.
- Unlike Vercel, the first deployment is **not** forced into Production; the developer
  chooses the environment.
- The UI and CLI must label all environments as running locally under Harpoon.

### D2 — Deployment sources

Public Git over HTTPS; private GitHub/GitLab via a read-only deploy key; local
directory; existing image reference. Compose definitions (local or in a repository)
follow in a later milestone (see D3). Source credentials are separate from registry
credentials. Git over SSH from the **host** is allowed for deploy keys; the "no SSH"
rule (ADR 0004) applies to guest management only.

- Deploy keys: the host key is **pinned** on first use and verified afterwards; a
  mismatch fails the deployment. Key material is passed to `git` without being
  written into the repository or logged, and removed after use.
- Checkout happens on the host and the build context is uploaded through the Docker
  API; cloning inside the guest is not required.

### D3 — Deployment strategies

- **Dockerfile build** (Milestones B and C) and **existing image** (Milestone B).
- **Compose** is a *best-effort* strategy after Milestone D. `compose up` recreates
  services in place, so build-before-replace and last-known-good do not hold in the
  same way; rollback is offered only for resources the control plane created, with
  weaker guarantees that must be documented when the strategy lands.
- Automatic framework detection (Nixpacks/Railpack-style) is deferred. Vercel's
  Build Output format and function or edge runtimes do not map to containers and are
  non-goals.

### D4 — Deployment lifecycle and records

Every deployment records: status (`requested`, `queued`, `running`, `succeeded`,
`failed`, `cancelled`, `interrupted`), trigger (manual, CLI/CI, poll, webhook), source
revision, resolved image identity, timestamps, structured step logs, failure reason,
the interruption (if any) with measured duration, and the release it produced.

- One deployment runs per application at a time. A newer queued request supersedes an
  older **queued** one; a running deployment is never interrupted except by explicit
  cancellation.
- **Global build limit:** one build at a time across the runtime until measurements
  show otherwise (builds share the VM's CPU and memory with running applications).
- Operations: `deploy`, `retry` (re-run a failed deployment's request), `redeploy`
  (new build of the same revision, with or without cache), `promote` (below),
  `rollback`, `cancel`.
- `promote` of a preview to production is a **new deployment of the same revision
  under production configuration**; the record states whether the image was reused
  (build-time configuration identical) or rebuilt.

### D5 — Safe, explicit replacement and rollback

- Build the new image without disturbing the running release.
- Start the new release on a non-conflicting, non-published address and run its
  **preliminary health check** before replacing a healthy release. Probe options, to
  be chosen by Milestone B measurement: Docker `HEALTHCHECK` status, an in-guest probe
  via `docker exec`, or an HTTP/TCP probe through a published loopback port (which can
  lag up to 10 s because the port forwarder syncs on a 10-second interval).
- Replacement may require a brief interruption (published host ports cannot be held by
  two containers). The plan displays the interruption before confirmation and the
  record stores it afterwards.
- Run a **final** health check after replacement and report it as evidence.
- Two different mechanisms exist and must not be conflated:
  **cutover recovery** (below) is a bounded, automatic attempt to undo *Harpoon's own
  interruption* when a cutover fails. **Rollback** is a developer decision about which
  retained release should run after a deployment has completed. Under the **initial
  2.1 policy**, Harpoon does not roll back a completed deployment on its own, for
  example because of a later health degradation; manual rollback is the policy for this
  release, not a permanent architectural prohibition, and an automatic policy could be
  proposed later with evidence.
- **Failure before cutover.** If the build fails or the preliminary check fails,
  nothing has been stopped. Retain the failed release for inspection, keep the running
  release, and print the explicit next actions (retry, logs).
- **Cutover recovery.** If Harpoon has stopped a healthy release and the replacement
  then fails to start or fails its final check, Harpoon makes **one bounded attempt**
  to restore the previous release from its retained image and pinned configuration
  snapshot. The attempt has a configurable time budget (default 60 s), is not repeated,
  and does not alter the failed release, volumes, or any other application.
  - The outcome is recorded and shown: `restored` (the previous release started and
    passed its own health check), `restored_unverified` (it started but its check did
    not pass or was inconclusive), `restore_failed`, or `not_attempted` (with the
    reason, for example the image or a pinned secret version is unavailable, Docker is
    unreachable, or the developer cancelled). The record includes the measured
    interruption and the restoration duration.
  - Harpoon does **not** promise zero downtime and does **not** promise recovery:
    a restored release may be incompatible with data the replacement touched, and the
    attempt can fail. The wording of every message must say what happened, not what is
    guaranteed.
  - The failed release is retained, the deployment is `failed` with its recovery
    outcome, and the application is flagged as needing attention.
  - **Pause after a failed cutover.** Automatic triggers for that application are
    paused (so a broken push cannot cause repeated interruptions) **only after an
    actual failed cutover** — a cutover that began and whose replacement failed. An
    ordinary build failure or a failed preliminary check does *not* pause triggers,
    because nothing was stopped. The paused state is visible in `status`, the history
    and the UI, with its reason (the deployment, the failure and the recovery
    outcome), and is cleared only by an explicit `resume` action. See *Paused
    triggers* below.
  - Retry, promotion and rollback remain explicit developer actions; none runs
    automatically after a recovery.
  - If the engine is killed during a cutover there is no process to recover. The next
    invocation marks the deployment `interrupted`, `status` states that the cutover is
    incomplete and the application may be down, and the same bounded restoration is
    offered as an explicit command (D10). This is a consequence of having no resident
    process.
- `rollback` restarts a named earlier release with its pinned configuration snapshot.
  Before confirmation it states: **application rollback only — data, volumes and schema
  are not rolled back and may be incompatible with the earlier release.**
- After a rollback, automatic promotion of newer pushes for that application is
  **paused** until the developer resumes it (adapted from Vercel's documented behavior
  of disabling production auto-assignment after a rollback).
- **Paused triggers.** A pause (after a failed cutover or a rollback) records its
  reason, time and originating deployment, is shown wherever the application's status
  appears, and is lifted only by an explicit `resume`. While paused, trigger events
  (poll results, webhooks, CI requests) are **not discarded**: each is recorded as a
  `held` event with its source revision and delivery ID, listed in `status`, and on
  `resume` the developer chooses to deploy the newest held revision, deploy a specific
  one, or dismiss them explicitly. Manual deployments remain possible while paused.
  Newest-wins supersede of queued requests (D4) likewise records each superseded
  request as `superseded` rather than deleting it.
- Cleanup is label-scoped: only containers and images carrying Harpoon labels for the
  affected application are candidates. Retention keeps the current release plus a
  configurable number of earlier releases (default 2). **Volumes, networks and
  unlabelled resources are never deleted by deployment cleanup.** Build cache is not
  pruned automatically (consistent with the Stage 8 finding).

### D6 — Environment variables and secrets

**Scopes.** `global` (all projects on this Harpoon install; the local analogue of
Vercel's team scope), `project`, `environment`, `application`. Preview environments may
add a `branch` override.

**Resolution.** At deploy time the engine resolves each variable by specificity, most
specific wins:

```
branch override (preview only)
  > application + environment > application
  > project + environment     > project
  > global + environment      > global
```

An explicit *unset* at a narrower scope blocks inheritance. The result is stored as an
immutable **configuration snapshot** with, for each key, its source scope, type,
build/runtime flags and (for secrets) the secret *version*. The UI shows the source
scope of every value and never recomputes a past snapshot from current settings.

**Types.** *Config* values are readable after saving. *Secret* values are write-only
after saving (the split Vercel documents). Secret values are never printed by the CLI,
shown in the UI, written to logs, or exported.

**Build time versus runtime.** Each variable is flagged `build`, `runtime` or both.
Changing a runtime-only variable creates a new release with the same image (no
rebuild). Changing a build-time variable requires a new build. Non-secret build values
use `--build-arg`. **Secrets are never passed as build args** (they persist in image
history); build-time secrets use BuildKit `--secret` mounts, available only to the
`RUN` steps that request them.

**Storage.** Secret values live in the macOS Keychain, one item per secret *version*
(service `io.harpoon.deploy`, account `<secret-id>@<version>`). SQLite stores
references (IDs and versions), key names, scopes and flags — never values.

**Versioning and rotation.** Saving a new value creates version N+1. A running or
retained release keeps the version it was built with. Rotation is: create the new
credential at its provider, save the new secret version, redeploy, verify, then revoke
the old credential — the sequence Vercel documents. Old versions are kept until no
retained release references them. Deleting a version that a retained release needs
makes that release unrollbackable, and the UI says so beforehand.

**Unavailable secrets.** If the Keychain is locked, denied, or an item is missing, the
deployment **fails before any side effect** (no build, no stop, no container change)
with the missing key named. There is no fallback to plaintext. Read-only commands
(`history`, `status`) keep working because they need only references. Whether any
deployment can run when the login keychain may be locked is not assumed: it is the
subject of Gate U, and until Gate U passes every deployment is attended.

**Exposure — what Keychain does and does not do.** The Keychain protects stored values
on the host. It does **not** protect a value after injection: runtime variables become
part of the container's configuration, are visible to `docker inspect` and to processes
in the container, and persist in Docker's configuration on the guest disk. The engine
passes runtime values to `docker` through the process environment (`-e NAME`) or a
short-lived `0600` env file, **never on the command line**, where `ps` would show them;
the choice is validated in Milestone C. Build secrets mounted with BuildKit do not
persist in image layers, but a build step can still print or copy them.

**Redaction.** Redaction is a best-effort control, not a security boundary, and it has
no minimum secret length.

- *Harpoon-generated output must not deliberately expose configured secrets.* The
  engine's own step logs, error messages, CLI and JSON output, records and crash
  reports are built so that resolved secret values never enter them. This is an
  invariant to be enforced by construction and verified by the leakage tests below,
  not by scrubbing alone.
- *Captured external output* (build output, `git` and `docker` stderr/stdout, and
  container logs that Harpoon captures or streams for display) is passed through
  redaction before it is stored or shown. Every secret value resolved for the
  deployment is masked wherever it appears **regardless of length**, matching across
  read-chunk boundaries, across the lines of multi-line values, and with the longest
  value taking precedence when one secret contains another. The mask names the key,
  never the value.
- *Short and low-entropy values* (for example a one-character value) will mask many
  unrelated characters. That over-masking is accepted and Harpoon warns when such a
  secret is saved, because the alternative is silently leaving it unprotected. Masking
  may hide text, but it must never drop, reorder or corrupt the log structure.
- *Failure behavior:* if the redaction step fails, the raw output is **withheld**
  rather than stored or shown, and the record says so.
- *Comparison to Vercel:* Vercel's documentation describes redaction of Secret values
  in build logs only at 32 characters or longer. Harpoon applies no floor, which
  trades readability for coverage; neither is a guarantee.

*Documented limitations (not defects):*

- **Transformed secrets are not detected:** base64, URL or JSON escaping, hashes,
  case changes, truncation, interleaving with other text, or a value an application
  splits across log records. Common encodings may be added later as best-effort
  extras; they are not promised.
- **Application-generated logs.** What an application writes to its own output is the
  application's responsibility. Harpoon masks exact known values in what it captures or
  streams, but Docker's own log store on the guest disk keeps the raw output, and
  `docker logs` run directly bypasses Harpoon entirely.
- **Docker inspection.** Runtime variables are visible in `docker inspect` of the
  running container, and via the container's process environment. Build arguments
  are visible in `docker history`; secrets must never be passed as build arguments.
  BuildKit secret mounts avoid image layers, but a build step can still print or copy a
  secret into a layer or log.
- **Display tools outside Harpoon** (a terminal's scrollback, a shell history, a
  copied command) are outside its control.

*Leakage and failure tests (Milestone C).* For each, plant a known secret of varied
lengths (1, 3, 8, 40 characters, a multi-line value, and two secrets where one contains
the other) and assert it is absent from the Harpoon-generated or Harpoon-stored sink:

| Sink or situation | Expectation |
|---|---|
| Build stdout/stderr, including `RUN` output echoing the secret | Masked in stored and displayed output |
| `git` and `docker` error messages that echo a value | Masked |
| Secret split across output chunks and across lines | Masked |
| Engine step logs, error messages, panic/crash output | Never contains the value |
| CLI human output and `--json` output, environment diff | Never contains the value |
| SQLite database and WAL (byte scan) | Never contains the value |
| Temporary files, env files, workspace after success and after failure | Removed or never contain the value |
| Host `ps` argv during a deployment | Never contains the value |
| Image `docker history` and layers | Never contain the value when BuildKit secrets are used |
| Redaction component forced to fail | Raw output withheld; recorded |
| Transformed secret (base64) printed by the build | Documented as not detected; the test asserts the limitation is documented, not that it is caught |
| Running container `docker inspect`, application's own log line | Value present; documented exposure |

**Display.** Environment diff shows names, scopes, flags and secret *version changes*,
never values. Harpoon does not store hashes of secret values, to avoid offline
guessing.

### D7 — Git automation and triggers

- **Primary trigger: outbound polling** (`git ls-remote`, interval at least 60 s with
  jitter, paused while the Mac sleeps or the VM is stopped). A laptop behind NAT cannot
  receive provider webhooks, so inbound delivery is *not* assumed.
- **Attended versus unattended.** Polling runs only inside a developer-started Harpoon
  process (a foreground `watch` process or the open app) and is described as
  "automatic *while Harpoon is running*". Pushes made while nothing is running are
  detected on the next run and reported as late. **Unattended automatic deployment is
  not claimed** until Gate U has passed and its demonstration is complete. Both the
  Git deploy key and the secret versions need the same credential availability that
  Gate U investigates; a deployment that cannot read them fails before any side effect.
- **Manual and CLI/CI triggers** are available regardless. A CI-invoked trigger works
  only when the CI job can reach the developer's machine; this is documented rather
  than hidden.
- **Webhook handling** exists to normalize and verify events. It is bound to loopback,
  size-capped, and verified against signed fixtures in Milestone D. Real provider
  deliveries require a user-supplied tunnel or relay (deferred). GitHub verification is
  HMAC-SHA-256 over the raw body; GitLab sends a shared secret token, compared in
  constant time. GitLab's idempotency header is unconfirmed and must be checked.
- One normalized event shape (`repository push`: provider, repository, ref, revision,
  delivery ID) feeds the queue. Delivery IDs are recorded for idempotency.
- Branch filters use patterns; the production branch is configurable.
- GitHub/GitLab **Apps**, PR comments and commit statuses require write-scoped provider
  credentials and are deferred.

### D8 — Execution seam

- Deployment operations use the **Docker API / CLI through the existing private Docker
  socket**: build (the CLI uploads the context), run, logs, exec, copy. The guest
  management channel is **not** part of the deployment path: it buffers output, has no
  cancellation, timeout, stdin or file transfer, and holds privileged exec, so it is a
  higher-risk boundary than a byte-proxied Docker socket.
- Any new guest capability requires measured justification first.
- The Docker API must never be exposed on a wildcard TCP listener.
- Docker CLI use matches the rest of the desktop app today; direct API use is a
  possible later optimization tracked by existing issue #54, not a prerequisite.

### D9 — Observability and transparency

The developer can see, per application and release: build and deployment status; build
log; container logs; Docker-reported health and resource use (through the existing
Harpoon resource views); the configuration snapshot and the environment diff between
releases; failed releases and the recovery actions available; and a versioned **parity
notes** panel listing known differences between Harpoon execution and Vercel-hosted
execution (functions and edge runtimes, routing and CDN, TLS and domains, regions,
caching, filesystem and process lifetime, CPU architecture).

### D10 — Persistence and recovery

- Deployment records, steps and logs are written to SQLite **before** each external
  side effect; each step is idempotent or marked non-resumable.
- Docker objects created by Harpoon carry three labels: `io.harpoon.managed=true`,
  `io.harpoon.app=<id>`, `io.harpoon.deployment=<id>`. Release identity equals the
  deployment ID of the successful build.
- **Minimal recovery:** when the engine starts, any deployment left `running` or
  `queued` is marked `interrupted`. It is never resumed automatically. `status` shows
  the interrupted deployment next to what Docker currently has for its labels. The
  developer chooses `retry`, `cleanup` (removes only that deployment's labelled
  leftovers) or `ignore`.
- **Interrupted cutover.** Before stopping a healthy release the engine records that
  the cutover has begun, and afterwards that the new release was confirmed. If it finds
  a cutover that began and was not confirmed, `status` reports "cutover incomplete —
  application may be down" and offers an explicit `recover` command that runs the same
  bounded restoration as D5 and records its outcome. The engine does not run it on its
  own, because it has no resident process and the developer must stay in control.
- **Not implemented for the MVP:** reconstructing the database from Docker, drift
  remediation, automatic adoption of arbitrary containers, background reconciliation
  loops. These stay deferred until evidence shows a need.
- A further simplification was considered: drop automatic `interrupted` marking and
  require an explicit `status --repair`. It is rejected for now because a record
  claiming `running` after a crash would mislead; Milestone B may revisit it with
  evidence.
- The store has a schema version and forward-only migrations; uninstall never deletes
  it without an explicit purge.

## Baseline measurements required before Milestone B code

Recorded in a new results file, and gated on the owner allowing a live VM:

1. Time and guest memory for a reference Dockerfile build through the Harpoon socket
   versus its direct baseline.
2. Behavior of a build while another application runs in the same VM (CPU and memory
   pressure; any out-of-memory kill).
3. Loopback publish latency for a newly started container (port forwarder sync).
4. Idle cost of the engine as a one-shot CLI (expected: zero resident process).

## SHOULD requirements (not committed)

Monorepo root-directory and watch paths; deployment tags; build cache controls; image
retention policy beyond the default; notifications; per-application resource limits;
PR/merge-request previews (need provider API access).

## Deferred / non-goals

- Multiple remote servers, SSH server management, remote Docker installation.
- Swarm, Kubernetes, cloud VM provisioning, dedicated build servers, mandatory
  registries.
- A generic provider/plugin or adapter framework. Records use neutral naming (an
  *artifact reference* rather than only a container ID, and a `target` column
  defaulting to `local`) so a later provider can be added without a schema rewrite, but
  no interface is built.
- **Deploying to Vercel or any external provider** — undecided (below).
- Vercel Functions, Edge runtime, Build Output API, `vercel.json` semantics, managed
  domains and TLS, global routing.
- Automatic public DNS/HTTPS, and automatic framework builds, until Dockerfile and
  image deployments are reliable.
- Zero-downtime traffic switching, unless measurements show the interruption is
  unacceptable.
- Docker-state reconciliation beyond D10.

## Deferred decisions and open questions

1. **External providers (Vercel and others).** Whether Harpoon should ever deploy
   directly to Vercel is deliberately undecided. A Vercel target would be a different
   pipeline (source → provider API), not an extension of the container pipeline, so no
   shared abstraction is built now.
2. **Background agent.** The *feasibility* question (can credentials and execution
   requirements be satisfied without the developer present?) is Gate U and precedes any
   milestone that claims unattended Git deployments. The agent's *implementation* stays
   deferred until Gate U passes and the owner approves it. Until then all automatic
   deployment is attended.
3. **Automatic rollback of a completed deployment.** Not part of the initial 2.1 policy,
   which is manual rollback; it is not a permanent prohibition and may be proposed later
   with evidence. Cutover recovery (D5) is the only automatic restoration and is bounded
   to a failed cutover. The restoration time budget and the pause behavior are tuned in
   Milestone B.
4. **Health probe mechanism.** Choose after the baseline measurements.
5. **Zero-downtime routing.** Only if the measured interruption is unacceptable.
6. **GitLab idempotency header and merge-request behavior.** Verify when D7 lands.
7. **Rust binary packaging.** The engine is a new executable that must be bundled and
   signed like the bundled Docker CLI.
8. **Secret injection mechanism.** `-e NAME` versus a short-lived env file; validate in
   Milestone C.
9. **Redaction extras.** Whether to add best-effort detection of common encodings
   (base64, URL-encoding) beyond exact-value matching, and whether the over-masking
   caused by very short secrets needs a save-time confirmation. Neither is promised.
10. **Keychain access across builds (unverified hypothesis).** Keychain items are tied to
    the creating application's identity. A rebuilt or re-signed engine binary, or one
    shared between the CLI and the desktop app, may trigger repeated access prompts or be
    denied. Test with ad-hoc and Developer ID signatures in Milestone C before relying on
    Keychain-backed secrets; this is part of Gate U's credential question.

## First vertical slice (Milestone B)

`harpoon deploy` on a local directory containing a Dockerfile: build, preliminary
health check, announced replacement, bounded cutover recovery with a recorded outcome,
retained previous release, failure inspection, explicit `rollback`, and history that
survives restart and crash. No Git, webhooks,
secrets, Compose, UI or background process. It exercises D4, D5, D9 and D10 on existing
Harpoon capabilities and settles the store, label and release model.
