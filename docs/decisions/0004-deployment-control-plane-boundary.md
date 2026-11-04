# 0004. Harpoon-native deployment control-plane boundary

- **Status:** proposed (the decisions marked *approved* below were accepted by the
  product owner on 2026-10-30; the milestone plan awaits final approval)
- **Date:** 2026-10-29, revised 2026-10-30

## Context

Harpoon needs a possible deployment workflow inspired by Vercel's developer experience
and Coolify's orchestration lifecycle: source-triggered deployments, environment
variables and secrets, history, health checks, rollback, and observability. Harpoon
already has a strict product boundary: Docker owns Docker state, Linux owns container
isolation, Apple Virtualization.framework owns the macOS VM, and Harpoon owns the
host/guest seam.

Coolify is Apache-2.0 licensed and useful as a behavior reference, but its server model
is SSH/SCP-centric and assumes a remotely managed Linux Docker host. Forking or
embedding it would import a larger PaaS/server lifecycle and conflict with Harpoon's
local transport boundary. Vercel is a developer-experience reference only; its
serverless, Edge and networking infrastructure does not map to Docker containers
(see [the Vercel UX spike](../results/SPIKE_VERCEL_UX.md)).

## Decision

We will build a small Harpoon-native control plane for **one local Harpoon runtime**. It
owns deployment intent, source resolution, configuration, secret references, execution
records, deployment history, and promotion decisions. Docker remains authoritative for
images, containers, networks, volumes, Compose, BuildKit, logs, exec and health.

Approved on 2026-10-30:

1. **Language:** Rust, as a standalone engine (library plus CLI), independently
   testable, with Tauri integration later.
2. **Store:** SQLite for deployment metadata, history and configuration.
3. **Secrets:** macOS Keychain holds values; SQLite holds references only.
4. **Placement:** host-side execution using Harpoon's existing Docker API/CLI. The
   control plane does **not** run as a workload in the guest: that would require
   mounting the Docker socket into a container (root-equivalent), would stop with the
   VM, and would put secrets on the guest disk.
5. **Transport:** no new transport. Deployments use the Docker API/CLI through the
   private Docker socket. The guest management channel is excluded from the deployment
   path (it buffers output and has no cancellation, timeout or file transfer, and it
   exposes privileged exec). New guest capabilities need measured justification.
6. **No additional services:** no mandatory Redis, PostgreSQL, Kubernetes or second
   container runtime, and no additional background daemon until unattended deployment
   requirements justify one.
7. **Interruptions:** brief, documented deployment interruptions are acceptable
   initially; zero-downtime routing is a measured later decision.

Behavioral decisions:

- Retry, rollback, promotion and redeploy are explicit developer operations. The
  initial 2.1 policy is that a completed deployment is not rolled back automatically;
  this is a policy for this release, not a permanent prohibition.
- **Cutover recovery is distinct from rollback.** If Harpoon stops a healthy release to
  switch versions and the replacement fails, it makes one bounded attempt to restore the
  previous release and records and displays the outcome (`restored`,
  `restored_unverified`, `restore_failed`, `not_attempted`). It promises neither zero
  downtime nor successful recovery, and afterwards leaves retry, promotion and rollback
  to the developer. Automatic triggers for that application are paused only after an
  actual failed cutover (not after an ordinary build failure); the pause and its reason
  are visible, lifted by an explicit resume, and trigger events received meanwhile are
  held and listed, not discarded.
- Application rollback never implies database or schema rollback.
- **Redaction is best effort and not a security boundary.** Harpoon-generated output must
  not deliberately expose configured secrets; captured external output is redacted by
  exact match regardless of secret length. Transformed secrets, application-generated
  logs, and Docker inspection remain documented limitations, backed by leakage tests.
- **Unattended Git deployment is gated.** A feasibility decision on Keychain and
  headless execution (Gate U) precedes any milestone that claims unattended deployments.
  The background-agent implementation stays deferred. Gate U establishes *correct
  behavior* during sleep, logout and shutdown (clean stop, recorded catch-up), not
  execution while the machine is unavailable. Until Gate U passes and is demonstrated,
  automatic deployment means "while Harpoon is running".
- Health checks are evidence about the local container, not a guarantee of production
  behavior.
- The first trigger is outbound polling plus manual/CLI triggers; inbound webhooks are
  verified against fixtures and require a user-supplied tunnel for real delivery,
  because a laptop is not publicly reachable.
- Docker reconciliation is minimal: records written before side effects, three Docker
  labels, `interrupted` marking after a restart, and explicit retry/cleanup. No state
  reconstruction, drift remediation or adoption of arbitrary containers.

"No SSH" means SSH is not used to manage the guest or Docker. Git over SSH from the host,
using read-only deploy keys with pinned host keys, is permitted for source access.

## Open future possibility (not decided)

Deploying directly to Vercel or other external providers may be supported later. This
ADR neither commits to nor rules out that outcome. We deliberately do **not** build a
provider abstraction now: a Vercel target would be a different pipeline (source →
provider API) rather than an extension of the container pipeline, and a generalized
interface designed without a second concrete case would likely be wrong. Records use
neutral naming (an artifact reference, a `target` defaulting to `local`) so adding one
later does not require a schema rewrite.

## Alternatives considered

- **Fork or embed Coolify:** rejected; Apache-2.0 allows reuse, but the SSH-centric
  server model and broad PaaS scope are a poor fit.
- **Make Harpoon a second container engine:** rejected; Docker ownership is an existing
  architectural invariant.
- **SSH into the guest:** rejected; it adds credential and exposed-service surface where
  Harpoon already has a private host/guest boundary.
- **Run the control plane in the guest as a container:** rejected (see Placement).
- **Extend the guest management channel into a deployment transport:** rejected; see
  Transport.
- **Swift implementation:** rejected in favor of Rust (reuse of the desktop app's Rust
  ecosystem and independent testability). Packaging a new executable is an open item.
- **Generic provider/plugin or multi-server framework first:** deferred until one local
  end-to-end deployment proves a measured need to generalize.
- **Automatic build detection, Kubernetes/Swarm, remote servers, public DNS/HTTPS:**
  deferred; none is required to prove the local path.

## Consequences

- The first prototype is smaller and reuses Docker's existing semantics.
- Secrets, webhook verification, idempotency, cancellation and rollback become explicit
  security and correctness obligations. Keychain protects stored values only; values
  injected into a container are visible through `docker inspect` and to the container.
- Unattended deployment is not supported until Gate U establishes that credentials and
  execution requirements can be met without the developer present. Whether a
  background agent is built is decided after that, not before.
- Redaction reduces accidental exposure in what Harpoon stores and shows; it does not
  protect values from `docker inspect`, from the container itself, or from the
  application's own logs.
- The engine is a new executable to bundle and sign, like the bundled Docker CLI.
- Linux and Windows portability can later share the deployment contract through
  runtime adapters, but no adapter framework is justified yet.
- A successful prototype must measure memory, latency, cleanup and failure behavior
  before broader orchestration or provider Apps are considered.
