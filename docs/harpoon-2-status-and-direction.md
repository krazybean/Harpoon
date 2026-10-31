# Harpoon 2 — Status and Direction

Status: canonical consolidation of repository evidence and current direction
Date: 2026-10-30 (runtime sections last reviewed 2026-10-29; deployment sections reconciled with the canonical roadmap after PR #121)

**Milestone status lives only in [the roadmap](roadmap.md#deployment-control-plane-harpoon-21--canonical-milestone-tracker).**
This document does not track which deployment milestones are done; where it describes the
deployment direction it records scope and rationale, and defers every status claim to the roadmap.

This document is the short index for the current Harpoon 2 runtime state and the
proposed deployment-control-plane direction. It records what is implemented,
validated, proposed, rejected, or still unknown. It does not promote conversation
ideas to shipped features.

## Status vocabulary

- **Implemented:** present in the checkout or documented product behavior.
- **Validated:** supported by a reproducible result, captured artifact, or named
  acceptance run.
- **Proposed:** direction or requirement; no implementation claim.
- **Rejected/deferred:** intentionally outside the current scope or contradicted
  by evidence.
- **Unknown:** not established by the current checkout or captured evidence.

## Current runtime boundary

**Implemented and validated:** Harpoon 2 is a Docker-compatible development runtime
for Apple Silicon macOS. Apple `Virtualization.framework` owns the Linux VM;
Docker Engine inside the guest remains authoritative for images, containers,
networks, volumes, Compose, BuildKit, logs, exec, and health. Harpoon owns the
macOS/Linux seam: VM lifecycle, the Unix Docker socket bridge, networking and
localhost forwarding, bind-mount translation, resource controls, diagnostics, and
guest-management transport.

The current architecture uses a Linux ARM64 guest, VZNAT, VirtioFS, a private
0600 host socket, and a separate vsock management channel. It does not expose the
Docker API on a wildcard TCP listener and does not introduce a second container
engine. See [architecture](architecture.md), [runtime anatomy](architecture/runtime-anatomy.md),
and [compatibility](compatibility.md).

The guest runtime closure is intended to boot and start Docker and management
without network access; network is for user image pulls and optional maintenance.
The closure decision and its artifact-level checks are recorded in
[ADR 0003](decisions/0003-runtime-closure.md).

## Release and verification state

**Validated:** M17 and M18 have healthy preserved acceptance evidence, including
Docker API/run, persistence, restart, diagnostics, packaging, and signed ARM64
runtime checks. Release tooling and the relocatable tarball are documented in
[distribution](distribution.md) and [M18](results/M18.md). Intermittent
`VZErrorDomain 1` startup failures remain a known host/Virtualization.framework
risk; they were characterized, not fixed.

**Release status:** `v2.0.0-rc.2` is the current testing-prerelease target and is
not a stable release. The new guest initramfs and kernel match the tracked
manifest (`30f5a015ca8594bf55acd546e9a5a9d511f6a538c4696be4d698d95edcb96664`
and `d625d13a08f08ba1befa309edab7fa9360093b7e7c62b03010c0de084ff9ac20`), and
one upgraded-host smoke boot passed. Second-Mac acceptance, 2×30 cold boots,
upgrade/rollback, Developer ID signing, notarization, and Gatekeeper validation
remain open.

**Validated as release preparation, not as a clean-machine release:** the runtime
closure work justifies another RC transfer to the work Mac, but the current
checkout does not contain a named second-Mac acceptance result. The release app
and DMG path also retain environment-specific blockers in the D1/D1.1 records.
Therefore “RC ready for transfer” must not be reported as “verified on a second
Mac.”

The next release verification must record, on the target Mac: host/OS/CPU,
installation from the shipped artifact, first boot, Docker CLI and Compose,
management exec/shell, persistence across stop/start, localhost publishing,
doctor/log output, and clean uninstall data preservation. A failed or unavailable
Virtualization.framework start is a blocked environment result, not evidence that
the runtime architecture passed or failed.

## Intel and non-macOS portability

**Current status: unknown/investigation, not supported.** The current product
contract is Apple Silicon macOS with an ARM64 Linux guest. The repository makes no
Intel or Windows claim; x86_64 images are not an MVP promise and may require
Rosetta or emulation.

**Proposed direction:** preserve the deployment and Docker-facing contract while
placing host-specific work behind runtime adapters. A future adapter boundary may
cover VM/guest lifecycle, socket or API transport, bind-mount transport, port
forwarding, resource observation, and diagnostics. The macOS adapter remains the
only implemented adapter today. Linux and Windows adapters are design targets, not
active compatibility claims.

Do not add a generic adapter framework before a second supported host demonstrates
which seams actually differ. The first portability milestone should be a written
contract plus one measured adapter implementation, not speculative interfaces.

## Stage 8 disk findings

**Validated:** Stage 8B/8D evidence shows that deleting the test-owned Docker image
restores guest free space, and explicit guest `fstrim /` reduces host allocation of
the sparse APFS backing file. The self-hosted result went from 11,054,560 allocated
blocks after deletion to 1,915,784 after trim. See [STAGE8](results/STAGE8.md).

**Rejected/deferred:** this does not justify automatic Docker image, BuildKit, or
volume pruning. Docker owns that state, broad prune commands are not safely scoped,
and no targeted retention policy has been measured. Keep trim diagnostic-only;
manual, explicitly scoped Docker cleanup remains the supported path.

## Native deployment control plane (Harpoon 2.1)

**Partly implemented; the full control plane is not.** Harpoon is building a lightweight native
control plane for one local Harpoon runtime. It should own deployment intent, source resolution,
environment/secrets configuration, serialized orchestration state, deployment
history, and health-gated replacement. Docker remains the only authority for
workload state. The first-deployment slice (local Dockerfile build, health-checked start, durable
journal, gated CLI, ownership-verified cleanup) is implemented and was accepted once on a live
disposable deployment; permanent SQLite state, replacement, bounded cutover recovery, rollback and
Milestones C–E are not. Exactly what is done and what comes next is in
[the roadmap](roadmap.md#deployment-control-plane-harpoon-21--canonical-milestone-tracker). The
requirements, acceptance criteria and stop conditions are in
[deployment-control-plane-requirements](deployment-control-plane-requirements.md).

Decisions approved on 2026-10-30: a standalone Rust engine (CLI first, Tauri
integration later), SQLite for records and configuration, macOS Keychain for secret
values with SQLite holding references only, host-side execution through the existing
Docker API/CLI, no mandatory Redis/PostgreSQL/Kubernetes, no new background daemon until
unattended deployment justifies one, and brief documented interruptions during
replacement. Deploying directly to Vercel or other providers remains an **open future
possibility** that is neither committed nor excluded.

Work is organized into five reviewable milestones (A specification and UX discovery,
B deployment engine and lifecycle, C builds/environments/secrets, D Git integration and
automation, E desktop experience and previews), each with prerequisites, acceptance
tests, failure scenarios, measurable stop conditions and exclusions. They are defined in
the [requirements](deployment-control-plane-requirements.md). The first executable
slice is a local-directory Dockerfile deployment with a preliminary health check,
announced replacement, bounded cutover recovery with a recorded outcome, a retained
previous release, and explicit rollback, with no Git, webhooks, secrets, Compose, UI or
background process. A feasibility decision on unattended execution (Gate U) sits between
milestones C and D.

Corrections adopted in that revision (see the requirements for detail):

- The guest management channel is **not** the deployment transport. It buffers output
  and has no cancellation, timeout or file transfer, and the Docker API already covers
  build, run, logs, exec and copy. The earlier "define a local transport contract"
  phase is dropped.
- Inbound webhooks cannot reach a laptop behind NAT, so the primary trigger is outbound
  polling; webhooks are verified against fixtures and need a user-supplied tunnel for
  real deliveries.
- "Docker is the only authority" is made precise: the store holds intent and history;
  Docker holds actual state; resources carry three labels; recovery only marks
  interrupted deployments and does not reconstruct state.
- Secrets are protected at rest on the host. Once injected they are visible through
  `docker inspect` and to the container; Harpoon does not claim otherwise. Redaction of
  captured output is best effort with no minimum secret length; Harpoon-generated output
  must not deliberately expose secrets, while transformed secrets, application logs and
  Docker inspection remain documented limitations covered by leakage tests.
- Cutover recovery is distinct from rollback: if Harpoon stops a healthy release and the
  replacement fails, it makes one bounded restoration attempt and records the outcome
  without promising zero downtime or guaranteed recovery; retry, promotion and rollback
  stay explicit developer decisions. Triggers pause only after an actual failed cutover,
  with a visible reason, an explicit resume, and held (not discarded) events. Manual
  rollback is the initial 2.1 policy, not a permanent prohibition.
- Unattended Git deployment is gated on Gate U (Keychain and headless-execution
  feasibility). It must establish correct behavior across sleep, logout and shutdown
  (clean stop, recorded catch-up), not execution while the Mac is unavailable. Until it
  passes and is demonstrated, automatic deployment means "while Harpoon is running", and
  the background-agent implementation remains deferred.
- Application rollback never implies database or schema rollback.
- Health checks are evidence about the local container, not a guarantee of behavior on
  Vercel or any other environment.

Execution is host-side and must not use SSH for guest management; Git over SSH from the
host for deploy keys is a separate matter. Do not expose the Docker API on wildcard TCP
to fit an SSH-oriented product.

## Coolify and Vercel lessons

**Validated as external design reference:** Coolify is Apache-2.0 licensed and
provides useful patterns for queued deployments, build-before-replace, health
gating, rollback, scoped variables, provider webhooks, and deploy-only CI tokens.
Its current server model is SSH/SCP-centric and assumes an SSH-reachable Linux
host where it can install or mutate Docker. That is a poor internal boundary for
Harpoon, whose local guest transport and Docker ownership are already defined. See
[the Coolify spike](results/SPIKE_COOLIFY_DEPLOYMENT.md).

**Rejected:** forking or embedding Coolify. Apache-2.0 permits reuse, but the
license does not remove the architectural cost: Harpoon would inherit a large
server/PaaS product, SSH assumptions, provider-specific behavior, and a second
control-plane lifecycle. Reuse the behavior where it fits and implement the
smallest Harpoon-native path.

**Validated as external design reference (documentation only):** the Vercel
developer-experience spike, [SPIKE_VERCEL_UX](results/SPIKE_VERCEL_UX.md), reviewed
Vercel's public documentation on 2026-10-30 without authenticated UI access. Harpoon
targets developer-experience parity where practical, not runtime or infrastructure
parity: Vercel Functions, the Edge runtime, managed networking and domains, and
`vercel.json` semantics do not map to Docker containers and are non-goals.

**Proposed Vercel-inspired product patterns:** import and configure, Development /
Preview / Production environments as configuration scopes, Config versus Secret
variables bound at deploy time, push-triggered deployments with a newest-wins queue,
branch previews, deployment history with filters, build and runtime logs, redeploy,
explicit promotion, and rollback that pauses automatic promotion. OCI multi-arch image
metadata is a deployment artifact concern; it does not change the current macOS ARM64
runtime claim.

## Phase order and stop conditions

This is the intended **order** of the 2.1 work; status is in the roadmap, not here.

1. Keep the runtime MVP and current release evidence stable; the deployment work does
   not touch the runtime, guest assets, or the 2.0 release effort.
2. Second-Mac artifact verification and the other Harpoon 2.0 release gates are a separate
   track. They are not prerequisites of any 2.1 deployment milestone.
3. Milestone A: specification and UX discovery, then owner approval.
4. Baseline measurements (Dockerfile build over the bridge, build under load, publish
   latency, idle cost), gated on an explicitly allowed live VM.
5. Milestone B: engine and lifecycle — first the first-deployment slice, then permanent
   SQLite state, health-gated replacement, bounded cutover recovery and explicit rollback.
6. Milestone C: builds, environments and secrets.
7. Gate U: decide whether credentials and execution can be satisfied unattended
   (decision only; no agent implementation).
8. Milestone D: Git integration, polling, and signed-fixture webhooks (attended unless
   Gate U passed and was demonstrated).
9. Milestone E: desktop experience and previews; a background agent only if Gate U
   passed and the owner approves it.
10. Measure idle memory, build overhead, deployment latency, log handling and cleanup at
   each milestone before adding provider Apps, notifications, portability, or any
   external-provider target.

Stop and reassess if the prototype needs remote-server management, SSH, a second
container runtime, Kubernetes/Swarm, mandatory Postgres/Redis, mandatory registries,
multi-server scheduling, or automatic public DNS/HTTPS. Those are scope expansion,
not prerequisites for proving the local deployment path.

## Open gaps

- No current second-Mac verification record is present (Harpoon 2.0 track; not a 2.1 gate).
- Intel behavior and Windows/Linux adapters are not implemented or validated.
- The full control plane is incomplete: there is no permanent SQLite store (the first-slice
  journal is a stopgap), secret store, API schema, queue worker, webhook verifier, or
  replacement/rollback implementation. The first-deployment slice is implemented; see the roadmap.
- OCI multi-arch publishing and registry credentials need a concrete acceptance
  contract; the current runtime remains ARM64-only.
- GitHub/GitLab issue state could not be synchronized in the 2026-10-29 session
  because the GitHub API was unavailable; no issue status is inferred here.
- The Vercel spike did not use an authenticated dashboard or CLI, did not read the
  GitLab integration page, and did not read Vercel's primary Container Images page.
- Whether Harpoon should ever deploy to Vercel or another provider is undecided by
  design.
- The deployment baseline was measured once, on one host (`results/DEPLOY_BASELINE.md`);
  engine-level idle, latency and cleanup measurements are still owed at each milestone.

## Canonical references

- [Roadmap](roadmap.md) — the single authoritative milestone tracker (deployment milestones: its 2.1 section).
- [ADR 0001](decisions/0001-virtualization-substrate.md) — macOS virtualization.
- [ADR 0003](decisions/0003-runtime-closure.md) — offline guest/release gate.
- [ADR 0004](decisions/0004-deployment-control-plane-boundary.md) — control-plane
  boundary decision.
- [Deployment requirements](deployment-control-plane-requirements.md).
- [Coolify spike](results/SPIKE_COOLIFY_DEPLOYMENT.md).
- [Vercel UX spike](results/SPIKE_VERCEL_UX.md).
- [Stage 8 result](results/STAGE8.md).
