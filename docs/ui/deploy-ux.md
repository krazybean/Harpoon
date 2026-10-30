# Harpoon Deployments — UI/UX design (Milestone A/E input)

Status: **DESIGN PROPOSAL — mock data only; nothing here is implemented.**
Date: 2026-10-30
Inputs: [`SPIKE_VERCEL_UX.md`](../results/SPIKE_VERCEL_UX.md), the approved
[requirements](../deployment-control-plane-requirements.md) and
[ADR 0004](../decisions/0004-deployment-control-plane-boundary.md), and the shipping
desktop UI (`ui/harpoon-desktop`). Companion: the clickable prototype in
[`deploy-prototype/`](deploy-prototype/index.html) and the baseline measurements in
`docs/results/DEPLOY_BASELINE.md` (Track A branch).

This document keeps three things apart:

- **Grounded** — taken from the approved spec, the Vercel documentation read in the
  spike, the current Harpoon code, or a measurement.
- **Design hypothesis** — an interface choice that has *not* been tested with users. Each
  is listed in [§10](#10-hypotheses-to-validate) with how to check it.
- **Prototype scaffolding** — the scenario/runtime/window controls above the app frame.
  They are not product UI.

## 1. What was inspected first

Current Harpoon desktop UI (source and screenshots, not memory):

| Aspect | Observation (source) |
|---|---|
| Shell | Left rail with 8 items — Overview, Containers, Images, Volumes, Networks, Resources, Diagnostics, Contexts — icon + label, yellow fill on the selected item (`ui/harpoon-desktop/src/App.tsx`, `styles/primitives.css`) |
| Header | Sticky card: page title + `Harpoon / local runtime` crumb, status badges (`Docker: ready`), container/image counts, host CPU/RAM sparklines, `Harpoon Engine · running · Manage` menu, Refresh (`App.tsx`) |
| Container workspace | Full-workspace detail with tabs Overview · Stats · Shell · Logs · Inspect · Networks · Topology · Storage; stack cards with per-container status strips (`docs/ui/harpoon-2-container-workspace-spec.md`, `docs/ui/assets/harpoon-2/screenshots/`) |
| Tokens | Graphite surfaces, restrained yellow accent, semantic green/amber/red/blue, system fonts only (`src/theme.css`) |
| Honesty patterns | "Docker-authoritative", "Not measured traffic", explicit missing-data states, source/scope on every metric (`docs/ui/harpoon-2-container-workspace-spec.md`) |
| Data model | `useShelf` read caches, Tauri `invoke(...)` commands, a Rust-owned refresh worker (`src/hooks/useShelf.ts`, `src-tauri/src/runtime_refresh.rs`) |

The design reuses those tokens and primitives directly (the prototype links
`theme.css` and `primitives.css`), so it cannot drift from the shipped look.

## 2. Information architecture

```
Harpoon (existing left rail)
├─ Overview · Containers · Images · Volumes · Networks · Resources · Diagnostics · Contexts   ← unchanged
└─ Deployments  (new item, after Containers)
   ├─ Projects                         home: projects → applications, "needs attention"
   │   └─ Application                  environment switcher: Development · Preview · Production
   │       ├─ Overview                 current release, latest deployment timeline, automation, config summary
   │       ├─ Deployments              history table with filters
   │       │   └─ Deployment           timeline · recovery attempt · build log · snapshot · Docker objects
   │       ├─ Logs                     Build (stored) · Application (live from Docker)
   │       ├─ Metrics                  Docker-reported CPU/memory/net + health evidence + parity note
   │       ├─ Variables                scoped Config/Secret, diff, Keychain status
   │       └─ Settings                 build, deployment, automation, danger zone
   └─ dialogs: Create project · Deploy plan · Roll back · Promote · Resume automation · Retry recovery · Variable
```

Design decisions (grounded in the spec unless marked):

1. **One new rail item, not a new app.** Deployments sits beside Containers. Every
   existing view keeps its place and behavior. *(Boundary from the brief.)*
2. **Deployments and Containers cross-link, both ways.** A deployed application links to
   its container(s) ("container `storefront-api-r42`"), and Containers shows a small
   `Deployment` chip on containers carrying `io.harpoon.managed=true`, linking back.
   Docker stays authoritative for container state; the chip is metadata, not control.
   (Screenshot 20.)
3. **Environments are a switcher inside an application**, not three separate trees.
   Development / Preview / Production are *configuration scopes on the same local
   runtime* (requirements D1) and the UI says so in the sub-header.
4. **Attention is surfaced where you already look.** A dot on the rail item, a banner on
   the project home, and banners at the top of the application. Failure and recovery
   are never buried in a tab.
5. **The header stays the existing header.** Runtime badge, meters and Engine menu are
   unchanged. When Harpoon is stopped, deployment history stays readable and actions
   that need Docker are disabled with an explanation (§6).

## 3. Screen inventory

All screenshots are generated from the prototype by `deploy-prototype/shoot.sh`.

| # | Screen | State shown | File |
|---|---|---|---|
| 1 | Projects home with attention banner | cutover failed, restored, paused | `shots/01-projects-attention.png` |
| 2 | Application overview | healthy | `shots/02-app-healthy.png` |
| 3 | Application overview | failed cutover → restored → automation paused | `shots/03-app-cutover-restored.png` |
| 4 | Application overview | down (restore failed) | `shots/04-app-down.png` |
| 5 | Application overview | recovering (bounded attempt in progress) | `shots/05-app-recovering.png` |
| 6 | Deployment history | build failure in history | `shots/06-deployments-history.png` |
| 7 | Deployment detail | build failed, nothing stopped | `shots/07-deployment-build-failed.png` |
| 8 | Deployment detail | recovery failed | `shots/08-deployment-recovery-failed.png` |
| 9 | Logs | live build, streaming | `shots/09-logs-live-build.png` |
| 10 | Metrics | health evidence + parity note | `shots/10-metrics.png` |
| 11 | Variables | Keychain locked | `shots/11-variables-keychain-locked.png` |
| 12 | Settings | build / deployment / automation | `shots/12-settings.png` |
| 13 | Create project | step 1 (source) | `shots/13-create-source.png` |
| 14 | Resume automation | held events, explicit choice | `shots/14-resume-automation.png` |
| 15 | Roll back | confirmation with data/schema warning | `shots/15-rollback-confirm.png` |
| 16 | Deploy plan | blocked by locked Keychain | `shots/16-deploy-plan-keychain-locked.png` |
| 17–19 | Empty · Loading · Harpoon stopped | | `shots/17…19` |
| 20 | Containers integration chip | | `shots/20-containers-integration.png` |
| 21–23 | Compact 1100 · Narrow 860 · Minimum 700 | | `shots/21…23` |

## 4. Core flows

Each flow lists the developer's view, with the spec rule it implements.

**Create / import a project** (dialog, 3 steps — screenshot 13). Choose source
(Git repository / local folder / Docker image) → Harpoon detects Dockerfiles and
proposes applications and a health check → Review shows exactly what will be created
(project, applications, Keychain item for the deploy key, polling mode). *Nothing
deploys until the developer chooses Deploy.* Detection is a suggestion, never a
requirement. (D1, D2; Vercel's import flow, minus forced first-production.)

**Configure build and deployment settings** (Settings tab). Detected build shown with
overrides (Dockerfile, context, target stage, build args). Deployment: port, health
check, replacement mode, retention. Automation: source, production branch, poll
interval, and the line *"Automatic = while Harpoon is running. Unattended deployment is
not offered yet."* (D3, D5, D7, Gate U.)

**Deploy / redeploy / retry** (plan dialog — screenshot 16). Before anything changes the
dialog states, from the engine's *plan*: what will be built, that the running release is
untouched until the preliminary check passes, whether a **brief interruption** will
occur and how long it took last time, and that a failed cutover triggers **one bounded
restoration attempt (60 s) with no guarantee**. If a required secret is unreadable the
primary action is disabled and the reason is shown — the deployment would fail before
any change. (D5, D6.)

**Watch a build** (Logs tab, live). Progress bar, follow toggle, filter, stored with the
deployment. Known secret values appear as `[REDACTED:KEY]`; a footnote states what
redaction does **not** cover. (D6.)

**Inspect a failed deployment** (detail — screenshots 7, 8). The page leads with a plain
statement of what happened and what was and was not touched ("Nothing was stopped; d-42
is still serving"), then the timeline, the recovery attempt, the build log with the
error line highlighted, the configuration snapshot, and the Docker objects. Recovery
actions sit in the header next to the status. (D4, D5.)

**Recovery outcomes** are first-class, with distinct color and wording:
`restored` (green, "the previous release is running again"), `restored_unverified`
(amber), `restore_failed` (red, **application down**, with Retry recovery / Roll back),
`not_attempted` (neutral, with the reason), and an in-progress state with the time
budget. The UI never says "recovered" for anything but `restored`, and the panel always
says zero downtime and guaranteed recovery are **not** promised. (D5.)

**Paused automation** (banner, Automation panel, resume dialog — screenshot 14). Shown
only after an actual failed cutover or a rollback, never after an ordinary build
failure. It states the reason and time, how many pushes are **held** (not discarded),
and offers **Review & resume…**. The dialog lists held revisions and requires an
explicit choice: *resume and deploy the selected revision* or *resume without deploying*
(held events are kept in history as dismissed by you). (D5 "Paused triggers".)

**Roll back / promote.** Rollback (screenshot 15) names the release, states the
interruption, says the automation will pause, and shows an **"application rollback
only — data, volumes and schema are not rolled back"** warning. Promote is described as
a *new deployment of the same revision under Production configuration* and says
whether the image will be rebuilt and why. (D4, D5.)

**Environment variables and secrets** (screenshot 11). Table with Name · Type
(Config/Secret) · Value (secrets show `••••••••` and "write-only") · Used in
(build / runtime / BuildKit secret) · Scope · Version. A **Differences** panel compares
environments and the last release by name, scope and secret *version* — never values.
A locked Keychain is an error banner that explains the deployment would fail before any
change. A footer states what Keychain does and does not protect. (D6.)

## 5. Visual direction

Extend Harpoon; do not restyle it.

- **Palette and type:** the existing graphite surfaces, system fonts, yellow reserved
  for the brand/selection/primary action. Status uses the existing semantic tokens.
- **Status vocabulary** (color is never the only signal — every pill has text, most an
  icon): green *Succeeded/Running/Restored*, blue *Building* (pulsing dot), amber
  *Recovering/Paused/Restored·unverified*, red *Failed/Down/Restore failed*, neutral
  *Queued/Cancelled*.
- **Banners over modals** for state ("Needs attention", "Automatic deployments are
  paused"): they stay visible while the developer reads logs. Dialogs are reserved for
  decisions that change something.
- **Timeline** (vertical steps with durations and a one-line note) is the main way to
  read a deployment; it answers "how far did it get" at a glance.
- **Evidence chip** (dashed outline) accompanies health and metrics: *"Evidence about
  the local container only — not a guarantee of behavior on Vercel or any other
  environment."*
- **No Vercel branding or imagery.** Interaction patterns only (deployment list with
  filters, per-deployment inspection, promote/redeploy/rollback as distinct verbs).
- **Motion:** only status pulses, a progress shimmer and a skeleton; all disabled under
  `prefers-reduced-motion`.

## 6. States

| State | Treatment |
|---|---|
| Empty (no projects) | Explanation + Create / Import folder / Open Containers; reassurance that existing containers are untouched (screenshot 17) |
| Loading | Skeleton cards/rows, no spinners-with-no-context (screenshot 18) |
| Healthy | Green hero, current release, URL, container link, Docker-reported resources |
| Building (live) | Blue status, "running release untouched" banner, streaming log, progress |
| Build failed | Amber banner "d-42 still running — nothing was stopped"; Retry / Edit settings / Logs |
| Cutover failed, restored | Amber banner + paused-automation banner; app shown **running** the previous release |
| Cutover failed, restore failed | Red **Application down** banner with Retry recovery and Roll back; resource figures replaced by "not estimated" |
| Recovering | Warning hero, restoration progress against the 60 s budget; actions that would conflict are disabled |
| Rolled back, paused | Running an older release; Automation panel explains the pause |
| Harpoon stopped / Docker not ready (disconnected) | Banner with **Start Harpoon** (the existing Engine action); history, logs and configuration stay readable; deploy/rollback/recover/promote disabled (screenshot 19) |
| Keychain locked | Variables banner and a blocked Deploy button; read-only commands still work |

## 6a. Measured inputs and values that are still illustrative

Track A (`docs/results/DEPLOY_BASELINE.md`, separate branch) supplies a few numbers the
design depends on, and leaves others open. The mock data uses *illustrative* values for
anything not measured; nothing below should be read as a promise.

| Design element | Basis |
|---|---|
| A "publishing…" moment after a new release starts is brief (URL reachable ≈ 0.64 s median, max 0.69 s after `docker run` returns, n = 30) | **Measured** |
| A build can saturate both VM CPUs for its duration; a light co-resident app showed no request errors | **Measured** (light app only) |
| One build at a time, shown as a queue state | Spec decision, consistent with the measurement |
| "Interruption about 4 s", "restoration 6.4 s", build durations, resource figures, release and revision names | **Illustrative mock values — not measured.** The real cutover interruption (stop grace period, start, probe) is a pending Milestone B measurement |
| Resource figures in the UI come from Docker stats for the container, never the VM or host | Spec rule (D9) |

## 7. Responsive behavior

The prototype uses CSS container queries on the app frame so it responds to the
*window*, as a native window resize would (grounded: the shipping spec asks for compact
icon rail and overlays at narrow widths).

| Window width | Behavior |
|---|---|
| ≥ 1180 | Full rail with labels; two-column application overview |
| 940–1180 | Icon rail (labels become tooltips); overview collapses to one column; CPU meter hidden |
| 720–940 | Cards and grids stack; secondary table columns hidden; banner actions wrap below text; header meters hidden |
| < 720 | Header un-stickies; secondary header buttons hidden; titles shrink |

Screenshots 21–23 show 1100, 860 and 700 px. Nothing requires horizontal scrolling at
700 px except the tab strip, which scrolls.

## 8. Accessibility (design commitments)

Status always has text; banners use `role="alert"` for errors and `status` otherwise;
dialogs have `aria-modal` and close on Escape; tabs use `role="tab"`/`aria-selected`;
focus rings reuse `--focus-ring`; the live build log is a region that does not steal
focus; progress uses `role="progressbar"`. Not yet verified with a screen reader —
listed as a hypothesis.

## 9. UI ⇄ deployment-engine data contract (draft)

The UI is a **client**. It never composes Docker commands for deployments and never
decides safety wording from raw Docker output: the engine returns a *plan* and typed
outcomes, and the UI renders them. Types are TypeScript-style for readability; the
transport (Tauri command wrapping the engine library, or engine CLI with `--json`) is
an integration choice, not part of the contract.

```ts
type Env = "development" | "preview" | "production";
type DeploymentStatus = "queued" | "running" | "succeeded" | "failed" | "cancelled" | "interrupted";
type RecoveryOutcome = "restored" | "restored_unverified" | "restore_failed" | "not_attempted" | "in_progress";
type StepStatus = "pending" | "running" | "done" | "failed" | "warn" | "skipped";

interface Project { id: string; name: string; applications: ApplicationSummary[]; }
interface ApplicationSummary {
  id: string; projectId: string; name: string;
  source: { kind: "git" | "folder" | "image"; ref?: string; path?: string; image?: string };
  environments: Record<Env, EnvironmentSummary>;
  attention: null | { level: "info" | "warn" | "error"; code: string; message: string };
}
interface EnvironmentSummary {
  runtime: "running" | "building" | "recovering" | "down" | "idle";   // derived from Docker + records
  currentReleaseId: string | null;
  url: string | null;                                                  // loopback URL chosen by Harpoon
  containerIds: string[];                                              // Docker is authoritative
  automation: { state: "active" | "paused"; reason?: PauseReason; since?: string; heldEvents: HeldEvent[]; mode: "attended" };
  health: { docker: "healthy" | "unhealthy" | "starting" | "none"; probe?: { ok: boolean; status?: number; ms?: number; at: string } };
  resources: null | { cpuPct: number; memBytes: number; memLimitBytes: number; pids: number; source: "docker-stats" };
}
interface PauseReason { kind: "failed_cutover" | "rollback"; deploymentId: string; recovery?: RecoveryOutcome; message: string; }
interface HeldEvent { id: string; revision: string; message: string; branch: string; receivedAt: string; source: "poll" | "webhook" | "ci"; }

interface Deployment {
  id: string; applicationId: string; env: Env; status: DeploymentStatus;
  trigger: "manual" | "cli" | "poll" | "webhook" | "rollback" | "promote";
  revision: { sha: string; message: string; branch: string; author?: string };
  releaseId: string | null; startedAt: string; durationMs: number | null;
  steps: { name: "queued"|"source"|"build"|"preliminary_check"|"cutover"|"final_check"|"recovery"; status: StepStatus; durationMs?: number; note: string }[];
  failure?: { step: string; code: string; message: string; nothingStopped: boolean };
  interruption?: { ms: number | null; ongoing: boolean };
  recovery?: { outcome: RecoveryOutcome; detail: string; durationMs?: number; budgetMs: number };
}
interface Release { id: string; deploymentId: string; image: string; configSnapshot: { variables: VariableRef[]; port: number; healthCheck: unknown }; retained: boolean; }
interface VariableRef { name: string; type: "config" | "secret"; usedIn: ("build"|"runtime")[]; scope: "global"|"project"|"environment"|"application"; secretVersion?: number; value?: string /* config only */ }

/** What the dialog shows. The UI renders it; it does not recompute it. */
interface DeployPlan {
  steps: string[]; willInterrupt: boolean; lastInterruptionMs?: number; recoveryBudgetMs: number;
  blockers: { code: "KEYCHAIN_LOCKED" | "SECRET_MISSING" | "DOCKER_UNAVAILABLE" | "PORT_IN_USE"; message: string }[];
  warnings: string[]; requiresRebuild: boolean; rebuildReason?: string;
}
```

Commands (read, then write), all idempotent where possible and all returning typed
errors:

```
deploy.list_projects() · get_application(id) · list_deployments(appId, env, filter) · get_deployment(id)
deploy.stream_logs(deploymentId | containerId, kind: "build"|"runtime") -> events
deploy.plan(action, args) -> DeployPlan                         // action: deploy|redeploy|retry|rollback|promote|recover
deploy.start(planId) · cancel(id) · recover(deploymentId) · resume(appId, env, { mode: "deploy"|"skip", heldEventId? })
deploy.create_project(spec) · set_variable(scope, name, type, value, usedIn) · list_variables(scope) · diff(env, a, b)
Events: deploy://deployment-updated · deploy://log-line · deploy://automation-changed
Errors: KEYCHAIN_LOCKED · SECRET_MISSING · DOCKER_UNAVAILABLE · RUNTIME_STOPPED · PORT_IN_USE · CONFLICT(app busy) · INTERNAL
```

Rules the contract enforces for the UI: secrets are write-only (`value` is never present
for `type: "secret"`); resource figures are `null` rather than estimated when there is
no container; `recovery.outcome` is the only source for the words "restored" or "down";
`automation.mode` is always `"attended"` until the unattended-execution gate passes.

## 10. Hypotheses to validate

These are interface choices with no evidence yet. Cheap checks before building Milestone E.

| # | Hypothesis | How to check |
|---|---|---|
| H1 | A single **Deployments** rail item next to Containers is findable without renaming anything | 5-second test with 3–5 users on screenshot 1 |
| H2 | An environment *switcher* inside the application (not three trees) is understood as "same app, different config" | Task test: "find Preview's variables" |
| H3 | Developers read **"nothing was stopped"** as reassurance, and **"not rolled back: data"** as a real warning | Think-aloud on screenshots 7 and 15 |
| H4 | Banners (not modals) for failed/paused state are noticed on return to the app | Leave the prototype on a failed scenario; ask what changed |
| H5 | The **resume dialog** with explicit held-event choices is clearer than a single Resume button | A/B on screenshot 14 |
| H6 | The evidence chip is read as a limit, not boilerplate | Ask "what does a green check tell you?" |
| H7 | Icon-only rail at ≤ 1180 px keeps the Deployments attention dot discoverable | Compact screenshots with real users |
| H8 | Screen-reader and keyboard behavior is acceptable | Manual VoiceOver pass in the real implementation |

## 11. Recommended UI integration boundaries

1. **Code location:** `ui/harpoon-desktop/src/views/deploy/` (new folder) plus
   `lib/deploy*.ts` for pure presentation helpers (unit-testable like the existing
   `*Presentation.test.mjs` files). No edits to existing views except the items below.
2. **Existing files touched:** `lib/navigation.js` and the nav list in `App.tsx` (one new
   item and a lazy-loaded view); `ContainersView` row chip keyed on the
   `io.harpoon.managed` label. Nothing else.
3. **Data access:** a `deployShelf` built on the existing `useShelf` pattern, backed by a
   new Rust module `src-tauri/src/deploy.rs` that calls the engine library (separate
   crate). The Tauri layer is a thin adapter; the engine stays independently testable
   (approved decision).
4. **No second source of Docker truth:** container status and resource figures come from
   the existing Docker read paths and are joined by container ID/label. The deployment
   store contributes intent, history and configuration only.
5. **Runtime status** is read from the existing status shelf; the Deployments views
   degrade as in §6 when it reports stopped or not ready.
6. **Quake shell and Topology** stay reachable from the container link; Deployments does
   not embed them.
7. **Lifecycle:** deployments proceed without the UI (CLI-first engine); the UI observes
   events and reconnects.

## 12. Running the prototype

Open `docs/ui/deploy-prototype/index.html` in a browser. Use the control bar for
scenario, runtime, Keychain and window width. Deep links work through the hash, e.g.
`index.html#scenario=cutover-restored&screen=app&tab=overview`; add `&shot=1` to hide
the control bar. `sh docs/ui/deploy-prototype/shoot.sh` regenerates every screenshot with
headless Chrome. The prototype has no build step and no network access; all data is
mock.
