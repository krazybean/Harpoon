# Spike — Vercel Developer Experience (UX Discovery)

Status: COMPLETE — documentation investigation; no authenticated UI access
Date: 2026-10-30
Scope: Vercel's current developer workflows as a *developer-experience* reference
for Harpoon's local deployment control plane. This spike makes no commitment to
deploy to Vercel and proposes no provider abstraction. It complements
[the Coolify spike](SPIKE_COOLIFY_DEPLOYMENT.md), which studied orchestration
internals; this one studies what the developer sees and does.

## Method and evidence classes

All findings come from Vercel's public documentation pages fetched on 2026-10-30
(each page's own `last_updated` date is given in the reference list). No Vercel
account, dashboard, or CLI session was used.

- **Documented** — stated in a cited Vercel page. Screenshot captions on those pages
  count as documented, not as observed behavior.
- **Inferred** — a reasonable consequence of documented behavior, marked as such.
- **Not verified** — would need an authenticated dashboard or CLI session, or a page
  this spike did not read. Examples: exact status labels in the deployments list,
  dashboard layout, GitLab-specific behavior, rate and size limits beyond those quoted.

The goal is **developer-experience parity where practical, not infrastructure or
runtime parity.** Vercel's serverless/Edge functions, CDN, managed domains and TLS,
and global routing are not reproduced and are not implied by any Harpoon feature.

## Findings by workflow

Each row: what the developer sees, what they do, what the platform does (where
documented), what Harpoon can reproduce locally, what needs infrastructure Harpoon
lacks, and the recommended milestone (A–E are defined in
[the requirements](../deployment-control-plane-requirements.md)).

### 1. Connecting a repository and importing a project

- **Sees/does (documented):** *New Project*, a list of repositories from connected
  Git providers, then a configuration page: project name, Framework Preset, root
  directory, build output settings, environment variables, and a **Deploy** button.
  GitHub, GitLab, Bitbucket and Azure DevOps are supported; the GitHub integration
  requires Owner or an org member with repository access. [git, vercel-for-github]
- **Behind the scenes (documented):** a Git-provider app/webhook delivers push and PR
  events; builds use a shallow clone (`--depth=10`). The **first deployment of a new
  project is always a production deployment**, even from a non-production branch
  or after failure. [git, configure-a-build, environments]
- **Harpoon can:** import by pasting a repo URL or choosing a local directory;
  record the revision; detect a Dockerfile/Compose file. It cannot list a user's
  repositories without a provider app.
- **Needs infrastructure Harpoon lacks:** provider app installation and inbound webhooks
  (a laptop is not publicly reachable).
- **Milestone:** B (local directory/image), D (Git URL, deploy key). The "first
  deployment is production" rule is **not** adopted; see Recommended flows.

### 2. Framework and build configuration discovery

- **Sees/does (documented):** Vercel auto-detects a framework and fills Build,
  Output, Install and Development commands; the developer may override each, per
  project or per deployment (`vercel.json`). "Other" is chosen when nothing is
  detected, and a build can be skipped. A **Container** preset deploys a root
  Dockerfile. [configure-a-build]
- **Behind the scenes (documented):** the preset decides how output is served
  (static output, functions). Container behavior (stateless, scale-to-zero,
  Compose not a deployment target) is from a changelog/guide summary, not the primary
  Container Images page, and is **not verified**.
- **Harpoon can:** detect Dockerfile / Compose / image; show what it detected and
  let the developer override. Framework-aware buildpacks are a separate, deferred
  decision (requirements: deferred).
- **Needs infrastructure Harpoon lacks:** none for Dockerfile; framework presets that
  emit Vercel's Build Output format do not map to containers and are explicitly
  not a goal.
- **Milestone:** C (Dockerfile + overrides), later (framework detection).

### 3. Development, Preview and Production environments

- **Documented:** three defaults — Local development, Preview (any non-production
  branch, any PR, or CLI deploy without `--prod`) and Production (the production
  branch, default `main`, then `master`, then the repo default). Custom
  environments (e.g. staging) exist on paid plans with branch tracking, their own
  variables and domains. Production can be set to *not* auto-assign domains, so a
  build is **staged** and later **promoted** without a rebuild. [environments, git,
  promoting-a-deployment]
- **Note:** "Development" on Vercel is a *variable scope for local work*
  (`vercel env pull`, `vercel dev`), not a hosted deployment target.
- **Harpoon can:** all three as configuration scopes on the one local runtime.
  Every Harpoon environment is a local container; "Production" is a label, not
  production-grade infrastructure. Staged/promote semantics are reproducible
  because a release is a retained image plus a config snapshot.
- **Needs infrastructure Harpoon lacks:** custom domains, TLS, per-environment
  public URLs.
- **Milestone:** B (Production/Development), E (Preview instances).

### 4. Environment variables and secrets

- **Documented:** variables are scoped to team or project and to targets
  (Production, Preview, Development, custom); values are encrypted at rest.
  A variable is **Config** (readable after save) or **Secret** (write-only after
  save). Preview variables can be overridden per Git branch. A change applies
  **only to new deployments**; old deployments keep the old values. Rotation is:
  create the new credential, update the variable, redeploy, then revoke the old one.
  Build logs redact Secret values only when they are 32 characters or longer.
  A total size limit of 64 KB per deployment applies. An optional team policy can
  require Production secrets to differ from other environments.
  [environment-variables, sensitive-environment-variables, rotating-secrets]
- **Inferred:** because values are bound at deploy time, Vercel's rollback and
  redeploy semantics depend on per-deployment value snapshots.
- **Harpoon can:** scopes, Config/Secret types, deploy-time binding, rotation by
  redeploy. Harpoon applies best-effort redaction to captured output with no minimum
  secret length (a different trade-off from the 32-character threshold Vercel
  documents) and promises neither perfect redaction nor protection after injection
  (see requirements, D6).
- **Needs infrastructure Harpoon lacks:** none; the OS keychain replaces a hosted
  secret store, with different guarantees (see requirements, D6).
- **Milestone:** C.

### 5. Git push events and automatic deployment

- **Documented:** every push deploys by default. If a build is already running
  on a branch, the running build finishes and the newest queued commit is built
  next; intermediate queued commits are cancelled (disable with
  `github.autoJobCancelation`). Per-branch opt-out uses `git.deploymentEnabled`
  with minimatch patterns. Private-repo commits deploy only if the author has
  project access; PRs from forks need explicit authorization to protect environment
  variables. Deploy Hooks are secret URLs (GET/POST, no auth header); repeat requests
  for the same version cancel earlier deployments, and there is a per-project
  rate limit. [git, vercel-for-github, git-configuration, deploy-hooks]
- **Harpoon can:** per-application serialized queue with newest-wins supersede of
  queued (not running) requests; branch filters; a deploy-hook-style local trigger
  and a CLI trigger. Fork-PR authorization is a non-issue for single-user local use
  but must be remembered if sharing is ever added.
- **Needs infrastructure Harpoon lacks:** inbound delivery from GitHub/GitLab. The
  primary local trigger is **outbound polling** of the remote (`git ls-remote`),
  described as automatic only *while Harpoon is running* until the unattended
  feasibility decision (Gate U in the requirements) passes; webhook handling is
  verified against signed fixtures and works for real deliveries only through a
  user-provided tunnel or relay.
- **Milestone:** D.

### 6. Branch and pull-request preview deployments

- **Documented:** every non-production branch/PR gets a deployment. Two URLs: a
  commit-specific one (`<project>-<hash>-<scope>.vercel.app`) and a branch URL
  (`<project>-git-<branch>-<scope>.vercel.app`) that always points at the branch's
  newest deployment. The PR receives a bot comment with the URL and a commit status.
  Preview deployments are publicly reachable by default unless Deployment Protection
  is enabled. [environments, generated-urls, vercel-for-github, managing-deployments]
- **Harpoon can:** branch previews discovered by polling; one local URL per
  preview, plus a "latest on this branch" pointer. PR-aware previews require PR
  events (provider API or webhook) and are later work.
- **Needs infrastructure Harpoon lacks:** public URLs, PR comments (requires a
  provider token with write scope — a larger credential than a read-only deploy key),
  per-preview DNS/TLS.
- **Milestone:** E (branch previews by polling); PR comments deferred.

### 7. Deployment timelines, states and history

- **Documented:** the Deployments page lists all deployments and filters by branch,
  date range, environment and status. A deployment can be inspected, redeployed
  (with or without the build cache), promoted, assigned a domain, or deleted.
  Production deployments have states **Staged**, **Promoted**, **Current**.
  The event vocabulary seen in provider dispatches is: pending, ready, success,
  error, failed, canceled, ignored, skipped, promoted. A configurable retention
  policy deletes old deployments. [deployments, managing-deployments,
  promoting-a-deployment, vercel-for-github]
- **Not verified:** the exact status labels in the dashboard list and the timeline
  layout.
- **Harpoon can:** a SQLite-backed record per deployment with state, timestamps,
  revision, trigger, steps, and failure reason; filters on the same dimensions;
  retention.
- **Needs infrastructure Harpoon lacks:** none.
- **Milestone:** B (records, CLI), E (UI timeline).

### 8. Build logs, runtime logs and failed-deployment inspection

- **Documented:** build logs exist for every deployment, are retained indefinitely,
  truncated beyond 4 MB, and individual lines are linkable. Warnings and errors are
  coloured. Runtime logs are searchable at the project level with plan-dependent
  retention; drains export them. The CLI offers `vercel logs --deployment <id>
  --level error`. [deployments/logs, logs, deployments]
- **Harpoon can:** per-deployment build log (stored with the record, size-capped,
  redacted), live container logs via Docker, step-level failure reason. Runtime log
  *history* is whatever Docker retains; Harpoon should not promise more.
- **Needs infrastructure Harpoon lacks:** none, but long-term retention competes
  with the guest disk budget.
- **Milestone:** B (CLI), E (UI).

### 9. Rollback, redeploy and promotion

- **Documented:** three distinct operations. **Instant Rollback** re-points
  production domains at an earlier deployment that previously served production, with
  no rebuild; environment variables are *not* changed, and the dialog reminds the
  developer that external APIs and databases may behave differently. After a
  rollback, auto-assignment of production domains is **turned off** until the
  developer explicitly promotes a deployment ("undo rollback"), so a later push does
  not silently replace the rolled-back version. Only one rollback at a time per
  project; Hobby is limited to the previous deployment. **Promote** of a *preview*
  rebuilds with production variables; promote of a *staged* production build does
  not rebuild. **Redeploy** re-runs the build and may bypass the cache.
  [instant-rollback, promoting-a-deployment, managing-deployments]
- **Harpoon can:** all three, with different costs. Rollback restarts a retained
  image with its pinned config snapshot (a short interruption, since the old
  container is a container, not a pointer swap). The "rollback pauses automatic
  promotion" idea is a worthwhile safety behavior to copy.
- **Needs infrastructure Harpoon lacks:** instant, zero-interruption traffic switch.
- **Milestone:** B.

### 10. Local development versus Vercel-hosted execution

- **Documented:** `vercel dev` replicates the deployment environment locally for
  Vercel Functions and Middleware, and Vercel recommends the framework's own dev
  command when it already provides what is needed. `vercel pull`/`vercel env pull`
  copy Development variables to a local file. `vercel build` followed by
  `vercel deploy --prebuilt` builds elsewhere and uploads the output. [cli/dev,
  environments, vercel-for-github]
- **Harpoon's position:** Harpoon runs the *same container image* the developer
  builds, locally. It does not replicate Vercel's functions, edge, routing,
  caching, image optimisation, or regional behavior. A healthy local deployment is
  evidence about the container, not about Vercel.
- **Milestone:** parity notes ship with B; no emulation is planned.

## Workflow comparison matrix

| # | Workflow | Vercel (documented) | Harpoon local equivalent | Milestone |
|---|---|---|---|---|
| 1 | Import repo | Provider app, repo picker, configure, deploy | Repo URL / local dir; deploy key; no repo picker | B, D |
| 2 | Build config | Auto-detect preset; overrides | Detect Dockerfile/Compose/image; overrides | C |
| 3 | Environments | Local / Preview / Production (+ custom) | Three config scopes on one runtime | B, E |
| 4 | Variables and secrets | Scopes, Config/Secret, deploy-bound | Same model; Keychain; pinned versions | C |
| 5 | Push → deploy | Webhook, newest-wins queue, branch filters | Poll + manual + fixture-tested webhooks | D |
| 6 | Previews | Per branch/PR, URLs, PR comment | Branch previews by polling; local URL | E |
| 7 | History | List, filters, states, retention | SQLite records, CLI/UI filters | B, E |
| 8 | Logs | Build (kept) + runtime (searchable) | Stored build log; Docker logs | B, E |
| 9 | Rollback/promote | Instant (domain swap), staged promote | Restart retained image; short interruption | B |
| 10 | Local vs hosted | `vercel dev`, `env pull` | Same image locally; parity notes | B |

## Recommended Harpoon interaction flows

These are UX contracts to validate in milestones B–E, not commitments to specific
commands or screens.

1. **First deploy.** `deploy` shows detected source and strategy, the
   environment it will run in, and the *interruption notice* if a release is
   already running. Unlike Vercel, a first deployment is **not** forced to
   Production: the developer chooses the environment.
2. **Deploy.** Build runs without touching the running release. A preliminary
   health check on the new release is run before replacement. If replacement
   requires stopping the old release, the plan says so *before* confirmation
   and the record stores the measured interruption afterwards.
3. **Failure.** The failed release is retained for inspection (logs, exit reason,
   health evidence). The previous release is not removed. If the failure happens after
   Harpoon stopped the running release, Harpoon makes one bounded attempt to restore
   it and reports the outcome (restored, restored but unverified, restore failed, or
   not attempted, with the reason); it does not promise zero downtime or recovery.
   The output lists the explicit next actions: *retry*, *rollback to <release>*,
   *view logs*. Nothing else runs without the developer. After a failed cutover (not an
   ordinary build failure) automatic triggers for the application stay paused, with
   the reason shown and pending events held, until the developer resumes. (This is
   Harpoon's design, not a
   Vercel behavior: Vercel switches domains rather than stopping a container.)
4. **Rollback.** Names the release and its config snapshot, states that
   **application rollback does not roll back data or schema**, states the expected
   interruption, then waits for confirmation. After a rollback, automatic
   promotion of new pushes for that application is paused until the developer
   resumes it (adapted from Vercel's behavior).
5. **Promote.** "Promote preview to production" is a new deployment of the same
   revision under production configuration; the UI states whether the image is reused
   or rebuilt and why.
6. **Environment diff.** A first-class view of differences between environments
   and between the last two releases: variable names and scopes (never secret
   values), build vs runtime flags, secret *version* changes.
7. **Parity notes.** A static, versioned list in the UI/CLI of known differences
   between Harpoon execution and Vercel-hosted execution (functions, edge, routing,
   TLS, regions, caching, filesystem and process lifetime). Health results are
   labelled as evidence about the local container only.

## Local versus Vercel-hosted: differences Harpoon must not hide

- Execution unit: Harpoon runs one long-lived container per application; Vercel
  serves functions and static output (documented).
- Network: Harpoon exposes loopback ports through the Harpoon port forwarder;
  Vercel provides managed domains, TLS and a CDN.
- Architecture: the Harpoon guest is ARM64; an image built there is not proof that
  an x86_64 target behaves the same.
- Statefulness and process lifetime: Harpoon containers keep local state between
  requests and run until stopped. Vercel's container runtime is described as
  stateless and scale-to-zero only in secondary sources (not verified), and the
  function runtime pages were not read in this spike.
- Environment variables: Harpoon binds them at deploy time like Vercel, but Docker
  stores injected values in the container configuration (see requirements, D6).

## Items this spike could not establish

- Dashboard layout, exact status labels, and timeline presentation.
- GitLab behavior (merge-request previews, commit statuses); the provider is listed
  as supported but its page was not read.
- Container Images behavior from Vercel's primary page (secondary sources only).
- Plan-dependent limits other than those quoted above.
- Whether and how deployment to Vercel from inside Harpoon should work. This is
  intentionally **not** decided; see the requirements' deferred decisions.

## References (fetched 2026-10-30; `last_updated` as stated on each page)

- Deploying to Vercel — https://vercel.com/docs/deployments (2026-09-03)
- Environments — https://vercel.com/docs/deployments/environments (2026-09-17)
- Environment variables — https://vercel.com/docs/environment-variables (2026-09-17)
- Config and Secret environment variables —
  https://vercel.com/docs/environment-variables/sensitive-environment-variables (2026-08-28)
- Rotating environment variables —
  https://vercel.com/docs/environment-variables/rotating-secrets (2026-07-15)
- Git — https://vercel.com/docs/git (2026-09-18)
- Vercel for GitHub — https://vercel.com/docs/git/vercel-for-github (2026-09-17)
- Git configuration — https://vercel.com/docs/project-configuration/git-configuration (2026-08-25)
- Deploy hooks — https://vercel.com/docs/deploy-hooks (2026-09-16)
- Configuring a build — https://vercel.com/docs/builds/configure-a-build (2026-08-28)
- Managing deployments — https://vercel.com/docs/deployments/managing-deployments (2026-09-15)
- Promoting deployments — https://vercel.com/docs/deployments/promoting-a-deployment (2026-06-26)
- Instant Rollback — https://vercel.com/docs/instant-rollback (2026-07-07)
- Generated URLs — https://vercel.com/docs/deployments/generated-urls (2026-09-08)
- Build logs — https://vercel.com/docs/deployments/logs (2026-08-28)
- Logs — https://vercel.com/docs/logs (2026-06-16)
- `vercel dev` — https://vercel.com/docs/cli/dev (2026-08-11)
- Container deployments (secondary sources only, not verified): Vercel changelog
  "Deploy a container from a plain Dockerfile" and the knowledge-base guide "Does Vercel
  support Docker deployments?" (found via search, not read in full).
