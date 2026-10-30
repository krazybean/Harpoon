# Harpoon Deployment Control Plane — Requirements Draft

Status: DRAFT
Date: 2026-10-29

This initiative adds a small Harpoon-native deployment control plane. It is inspired
by Coolify's useful product patterns, but it does not fork or embed Coolify and does
not create a second container engine.

## Product boundary

Harpoon remains a Docker-compatible local runtime. Docker Engine remains authoritative
for images, containers, networks, volumes, Compose, BuildKit, logs, exec, and health.
Harpoon owns deployment intent, source resolution, orchestration state, secrets and
environment configuration, deployment history, and the macOS/Linux execution seam.

The first target is one Harpoon runtime. Multi-server orchestration is not part of
this initiative's first release.

## MVP requirements

### D1 — Projects and environments

- A project contains one or more environments.
- An environment contains deployable applications and their configuration.
- Each application has one deployment source and one deployment strategy.
- Resource identity is stable across deployments.

### D2 — Deployment sources

MVP sources:

- Public Git repository over HTTPS.
- Private GitHub or GitLab repository through a read-only deploy key.
- Existing Docker image reference.
- Local or repository Docker Compose definition.
- Local or repository Dockerfile.

The source credential is separate from container-registry credentials.

### D3 — Deployment strategies

MVP strategies:

- Dockerfile build.
- Docker Compose build/up.
- Existing image pull/run.

Nixpacks/Railpack-style automatic build detection is deferred until Dockerfile and
Compose workflows are reliable.

### D4 — Deployment lifecycle

Every deployment records:

- requested, queued, running, succeeded, failed, cancelled, or rolled-back status;
- source revision and resolved image identity when available;
- start/end timestamps;
- structured step logs;
- failure reason;
- whether the deployment was manual, webhook-triggered, or API-triggered.

Only one deployment per application runs at a time. A newer request may supersede a
queued request, but must not interrupt a running deployment unless cancellation is
explicitly requested.

### D5 — Safe replacement

- Build before replacing the running version whenever possible.
- Preserve the last known-good version until the new version is healthy.
- Run a configured health check after replacement.
- Roll back automatically only when the replacement was created by the control plane
  and rollback is safe and deterministic.
- Never delete user-owned volumes or unrelated Docker resources as deployment cleanup.

### D6 — Environment and secrets

Support scopes:

- global/team;
- project;
- environment;
- application/resource.

Requirements:

- Secrets are encrypted at rest or delegated to an explicitly configured secret
  provider.
- Secrets never appear in deployment logs.
- Build-time and runtime variables are distinct.
- Changing a runtime-only variable need not rebuild an image.
- Changing a build-time variable requires a new build.

### D7 — GitHub and GitLab automation

MVP automation:

- Authenticated repository webhooks for push events.
- Branch filtering.
- Idempotent event handling using provider delivery IDs.
- Signature or secret-token verification before queueing a deployment.
- A narrow deploy-only API token for external CI.
- Manual deployment remains available when automation is disabled.

GitHub App and GitLab App integrations are SHOULD-level follow-up capabilities. The
first implementation may use repository-scoped deploy keys plus manually configured
webhooks, but the credential model must not prevent adding provider Apps later.

### D8 — Harpoon execution seam

The control plane must target Harpoon through a local Harpoon transport contract, not
SSH. The contract needs:

- bounded command execution with argv preservation;
- deployment-file upload/read/write;
- Docker API or Docker CLI invocation against the active Harpoon Engine;
- streamed stdout/stderr;
- cancellation and exit-status propagation.

The transport must work with the current macOS guest boundary and leave room for a
Linux or Windows Harpoon implementation. It must not expose the Docker API on a
wildcard TCP listener.

## SHOULD requirements

- Pull-request/merge-request preview deployments.
- Monorepo watch paths.
- Deployment tags and grouped deploys.
- Build cache reuse with an explicit force-rebuild option.
- Artifact/image retention policy.
- Deployment notifications.
- API access for CI/CD.
- Per-application concurrency controls.

## Deferred / non-goals

- Multiple remote servers.
- SSH server management and remote Docker installation.
- Swarm or Kubernetes.
- Cloud VM provisioning.
- Dedicated build servers and mandatory registries.
- Provider-specific deployment logic in the core engine.
- A second container or OCI runtime.
- Automatic HTTPS and public DNS before the local deployment path is proven.
- Nixpacks/Railpack before Dockerfile and Compose deployment are stable.

## MVP acceptance slice

The first demonstrable slice is complete when it can:

1. Create a project, environment, and application.
2. Deploy a private GitHub or GitLab repository using a deploy key.
3. Build a Dockerfile image in Harpoon's Docker Engine.
4. Start the application with runtime environment variables.
5. Stream deployment and container logs.
6. Run a health check and retain the prior version on failure.
7. Trigger the same deployment from a verified push webhook.
8. Persist deployment history and environment configuration across control-plane restarts.
9. Demonstrate that Docker remains the only authority for workload state.

## First implementation rule

Do not implement a generic provider/plugin framework or multi-server abstraction until
the single Harpoon target has a working end-to-end deployment and one measured reason
to generalize it.
