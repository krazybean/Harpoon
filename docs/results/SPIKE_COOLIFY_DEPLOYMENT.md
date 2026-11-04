# Spike — Coolify Deployment, Orchestration, and Git Integrations

Status: COMPLETE — source/documentation investigation
Date: 2026-10-29
Scope: Coolify current upstream behavior as a design reference for Harpoon's own
deployment control plane. No runtime or application code was modified by the spike;
the requirements and this report are the documentation outputs.

## Executive conclusion

Coolify's strongest reusable ideas are its deployment lifecycle, queued execution,
last-known-good replacement model, scoped environment variables, provider webhooks,
deploy-only API tokens, and separation between repository credentials and registry
credentials.

Its weakest fit for Harpoon is the server abstraction: Coolify assumes an SSH-reachable
Linux host and performs shell/file operations there. Harpoon should retain the
control-plane behavior while replacing the transport with a Harpoon-local execution
contract.

Coolify's upstream repository is Apache-2.0 licensed. That permits reuse, but does
not make a fork the right architecture: the SSH/SCP server model, Docker installation
actions, and larger PaaS lifecycle would still be the wrong boundary for Harpoon.
See the [upstream LICENSE](https://github.com/coollabsio/coolify/blob/v4.x/LICENSE).

## Findings

### 1. Deployment orchestration

Coolify treats a deployment as a queued operation that resolves source/configuration,
builds or pulls images, applies runtime configuration, and replaces workloads. Builds
normally happen on the deployment server; a separate build server is optional and
requires a registry for image transfer.

Useful patterns to adopt:

- explicit deployment records and logs;
- queueing and per-resource serialization;
- build-before-replace;
- health-gated replacement;
- rollback that preserves the current version when the new build fails;
- force-deploy as an explicit cache bypass;
- source revision and image identity recorded as deployment metadata.

Source: [Coolify build and deployment model](https://coolify.io/docs/core/build-deployment-model),
[advanced build settings](https://coolify.io/docs/applications/configuration/advanced).

Decision: ADOPT the lifecycle semantics; REJECT build-server/registry requirements for
the first Harpoon target.

### 2. Remote execution model

Coolify's deployment job generates SSH commands and uses SCP/SSH multiplexing for
remote execution and file transfer. Its server actions can install Docker, modify
`daemon.json`, restart Docker, and create networks.

Sources: [`ExecuteRemoteCommand.php`](https://github.com/coollabsio/coolify/blob/main/app/Traits/ExecuteRemoteCommand.php),
[`SshMultiplexingHelper.php`](https://github.com/coollabsio/coolify/blob/main/app/Helpers/SshMultiplexingHelper.php),
[`InstallDocker.php`](https://github.com/coollabsio/coolify/blob/main/app/Actions/Server/InstallDocker.php).

Decision: REJECT SSH as Harpoon's internal deployment boundary. REJECT remote Docker
installation and daemon mutation. ADAPT the command/file/Docker capabilities behind a
Harpoon-local transport.

### 3. Build and runtime strategies

Coolify supports Dockerfile, Compose, existing images, Static, Nixpacks, and Railpack.
Dockerfile and Compose are the best first Harpoon strategies because they preserve
Docker's existing authority and avoid introducing a language/framework detector.

Source: [Coolify build overview](https://coolify.io/docs/applications/builds/overview).

Decision: ADOPT Dockerfile, Compose, and image deployment first. DEFER Nixpacks and
Railpack.

### 4. Environment variables

Coolify separates team, project, environment, and server scopes. Variables can be
build-time or runtime, and references are resolved when resource configuration is
prepared.

Source: [Coolify shared variables](https://coolify.io/docs/core/team/shared-variables),
[application environment variables](https://coolify.io/docs/applications/configuration/environment-variables).

Decision: ADOPT global/project/environment/resource scopes. REPLACE server scope with
Harpoon runtime scope. Keep build-time/runtime separation.

### 5. GitHub integration

Coolify supports public HTTPS repositories, repository deploy keys, GitHub Apps, and
GitHub Actions-triggered deployment. GitHub Apps provide repository selection,
integrated webhooks, and pull-request events; deploy keys are narrower and read-only.

Source: [GitHub source overview](https://coolify.io/docs/applications/sources/github/overview),
[GitHub App setup](https://coolify.io/docs/applications/sources/github/app).

Decision: MVP uses read-only deploy keys plus verified webhooks. Add a GitHub App after
the deployment lifecycle is proven. Keep repository access and container-registry
access as separate credentials.

### 6. GitLab integration

Coolify supports public HTTPS repositories, deploy keys, and GitLab Apps. Its GitLab
App flow uses OAuth scopes for API/user/repository access and supports push and
merge-request events. Manual webhooks use a secret token.

Source: [GitLab integration](https://coolify.io/docs/applications/sources/gitlab).

Decision: Mirror the GitHub model: deploy key + webhook first, provider App second.
The internal deployment event should normalize GitHub push, GitLab push, and future
provider events into one Harpoon event shape.

### 7. Webhook and CI behavior

Coolify distinguishes repository webhooks from deploy webhooks. Repository webhooks
notify the application of source changes; deploy webhooks are authenticated API calls
from CI or another automation system. Its documentation recommends deploy-only
tokens for CI and explicitly says an accepted request does not prove that deployment
became healthy.

Sources: [automatic deployments](https://coolify.io/docs/applications/deployments/automatic-deployments),
[manual Git webhooks](https://coolify.io/docs/applications/deployments/manual-webhooks),
[deploy webhooks](https://coolify.io/docs/core/automation/deploy-webhooks).

Decision: ADOPT separate repository-event and deployment-command concepts. Require
idempotency, queue status, and final health status instead of treating HTTP 2xx as
deployment success.

## Harpoon-specific architecture consequence

> **Superseded 2026-10-30.** This section originally proposed running the control plane
> in the guest or as a host client, with a new Harpoon transport for bounded guest-local
> command and file operations. The follow-up review decided otherwise (see
> [ADR 0004](../decisions/0004-deployment-control-plane-boundary.md)): the control plane
> runs **host-side** and uses the Docker API/CLI through the existing private socket
> (the CLI uploads build contexts, so no separate file-transfer path is needed). It does
> not use the guest management channel, which buffers output and lacks cancellation,
> timeouts and file transfer. The Coolify findings above remain valid as a behavior
> reference. For the developer-experience reference see
> [SPIKE_VERCEL_UX](SPIKE_VERCEL_UX.md).

Relevant current Harpoon evidence:

- [`UnixBridge.swift`](../../harpoon/Sources/UnixBridge.swift)
  already forwards Docker API traffic and translates supported bind paths.
- [`VMManager.swift`](../../harpoon/Sources/VMManager.swift)
  owns the VM/vsock boundary.
- [`init`](../../tools/guest-builder/src/init)
  starts Docker and exposes the current Docker and management vsock services.
- [`runtime-anatomy.md`](../architecture/runtime-anatomy.md)
  records that Harpoon must not become a second authority for Docker state.

## Follow-up prototype

> **Reordered 2026-10-30.** The list below was the original sketch. The implementation
> order is now the five milestones in the
> [requirements](../deployment-control-plane-requirements.md): the first slice is a
> local-directory Dockerfile deployment, and Git checkout and signed webhook fixtures
> (steps 2, 3 and 7) arrive in Milestone D. Polling, not inbound webhooks, is the
> primary trigger on a laptop.

1. Implement no provider Apps yet.
2. Define one normalized `repository push` event shape.
3. Add deploy-key-backed Git checkout.
4. Add a deployment record and serialized queue for one application.
5. Build a Dockerfile image through Harpoon's Docker API.
6. Start the new container, run one health check, and retain the previous container
   on failure.
7. Trigger the same path through a signed GitHub webhook and a signed GitLab webhook
   fixture.
8. Measure control-plane idle memory, build overhead, deployment latency, and failure
   cleanup before adding broader orchestration.

## Constraints discovered

- Coolify's 2 CPU / 2 GB self-hosting guidance is a useful lower-bound reference for a
  full control-plane stack, not a Harpoon requirement.
- The first Harpoon control plane should avoid mandatory Postgres/Redis until the
  product requirements prove that a full multi-user control plane is needed. A local
  single-user prototype can start with the smallest durable store that supports the
  required deployment history and secrets safely.
- Do not expose Harpoon's Docker API on wildcard TCP merely to make an existing SSH-
  oriented product fit.
