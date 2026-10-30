# Harpoon 2 — Status and Direction

Status: canonical consolidation of repository evidence and current direction
Date: 2026-10-29

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

## Proposed native deployment control plane

**Proposed, not implemented:** Harpoon may add a lightweight native control plane
for one local Harpoon runtime. It should own deployment intent, source resolution,
environment/secrets configuration, serialized orchestration state, deployment
history, and health-gated replacement. Docker remains the only authority for
workload state. The requirements are in
[deployment-control-plane-requirements](deployment-control-plane-requirements.md).

The first slice should support:

- projects, environments, applications, and stable resource identity;
- public HTTPS Git and private GitHub/GitLab repositories via read-only deploy keys;
- Dockerfile, Compose, and existing-image deployments;
- global/team, project, environment, and application-scoped variables and secrets;
- distinct build-time and runtime variables;
- manual, API/CI, and authenticated GitHub/GitLab push triggers;
- delivery-id idempotency, branch filtering, serialized per-application queueing,
  structured logs, and deployment history;
- build-before-replace, health checks, last-known-good retention, and safe rollback;
- a local Harpoon transport with bounded argv-preserving commands, file transfer,
  streamed output, cancellation, and exit-status propagation.

The execution seam must not be SSH. It should use the existing private host/guest
boundary and leave room for future Linux/Windows adapters. Do not expose the Docker
API on wildcard TCP to fit an SSH-oriented product.

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

**Proposed Vercel-inspired product patterns:** manual and CI/CD deployment
triggers, GitHub/GitLab auth and webhooks, OCI multi-architecture image metadata,
Dockerfile/Compose/image inputs, scoped variables/secrets, an orchestration queue,
deployment history, health-gated promotion, rollback, and observability. Multi-arch
OCI support is a deployment artifact concern; it does not change the current
macOS ARM64 runtime claim.

## Phase order and stop conditions

1. Keep the runtime MVP and current release evidence stable.
2. Complete second-Mac artifact verification and record gaps honestly.
3. Define the smallest local transport contract needed by one deployment path.
4. Prototype one application with deploy-key Git, Dockerfile/Compose/image,
   persisted history, health gating, and safe replacement.
5. Add signed GitHub/GitLab webhook fixtures and manual/API triggers.
6. Measure idle memory, build overhead, deployment latency, log handling, and
   cleanup before adding provider Apps, previews, notifications, or portability.

Stop and reassess if the prototype needs remote-server management, SSH, a second
container runtime, Kubernetes/Swarm, mandatory Postgres/Redis, mandatory registries,
multi-server scheduling, or automatic public DNS/HTTPS. Those are scope expansion,
not prerequisites for proving the local deployment path.

## Open gaps

- No current second-Mac verification record is present.
- Intel behavior and Windows/Linux adapters are not implemented or validated.
- The control plane has no runtime implementation, persistence choice, secret store,
  API schema, queue worker, webhook verifier, or rollback implementation.
- OCI multi-arch publishing and registry credentials need a concrete acceptance
  contract; the current runtime remains ARM64-only.
- GitHub/GitLab issue state could not be synchronized in this work session because
  the GitHub API was unavailable; no issue status is inferred here.

## Canonical references

- [Roadmap](roadmap.md) — milestone and current-position index.
- [ADR 0001](decisions/0001-virtualization-substrate.md) — macOS virtualization.
- [ADR 0003](decisions/0003-runtime-closure.md) — offline guest/release gate.
- [ADR 0004](decisions/0004-deployment-control-plane-boundary.md) — proposed
  control-plane boundary.
- [Deployment requirements](deployment-control-plane-requirements.md).
- [Coolify spike](results/SPIKE_COOLIFY_DEPLOYMENT.md).
- [Stage 8 result](results/STAGE8.md).
