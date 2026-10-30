# 0004. Harpoon-native deployment control-plane boundary

- **Status:** proposed
- **Date:** 2026-10-29

## Context

Harpoon needs a possible deployment workflow inspired by Vercel and Coolify:
source-triggered deployments, environment variables and secrets, orchestration,
history, health checks, rollback, and observability. Harpoon already has a strict
product boundary: Docker owns Docker state, Linux owns container isolation, Apple
Virtualization.framework owns the macOS VM, and Harpoon owns the host/guest seam.

Coolify is Apache-2.0 licensed and useful as a behavior reference, but its server
model is SSH/SCP-centric and assumes a remotely managed Linux Docker host. Forking
or embedding it would import a larger PaaS/server lifecycle and conflict with
Harpoon's local transport boundary.

## Decision

We propose a small Harpoon-native control plane for one local Harpoon runtime. It
will own deployment intent, source resolution, configuration/secrets, queue state,
deployment history, and promotion decisions. Docker remains authoritative for
images, containers, networks, volumes, Compose, BuildKit, logs, exec, and health.

Deployment operations will use a bounded local Harpoon transport, not SSH or a
wildcard Docker TCP listener. The first implementation will support Dockerfile,
Compose, and existing-image deployments, with deploy-key Git sources, authenticated
webhooks/API/manual triggers, serialized per-application execution, health-gated
replacement, and safe rollback.

## Alternatives considered

- **Fork or embed Coolify:** rejected; Apache-2.0 allows reuse, but the SSH-centric
  server model and broad PaaS scope are a poor fit.
- **Make Harpoon a second container engine:** rejected; Docker ownership is an
  existing architectural invariant.
- **SSH into the guest:** rejected; it adds credential and exposed-service surface
  where Harpoon already has a private host/guest boundary.
- **Generic provider/plugin or multi-server framework first:** deferred until one
  local end-to-end deployment proves a measured need to generalize.
- **Automatic build detection, Kubernetes/Swarm, remote servers, public
  DNS/HTTPS:** deferred; none is required to prove the local path.

## Consequences

- The first prototype is smaller and can reuse Docker's existing semantics.
- Secrets, webhook verification, idempotency, cancellation, and rollback become
  explicit security and correctness obligations.
- Linux and Windows portability can later share the deployment contract through
  runtime adapters, but no adapter framework is justified yet.
- A successful prototype must measure memory, latency, cleanup, and failure behavior
  before broader orchestration or provider Apps are considered.
