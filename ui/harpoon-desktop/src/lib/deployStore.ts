// Pure reducer + selectors for the SIMULATED Deployments experience.
//
// Design rules (all covered by deployStore.test.mjs):
//  * One source of truth. An application's status is *derived* from its current release, its
//    deployments and the runtime — never stored — so scenarios and actions cannot disagree.
//  * Every scenario is reached by replaying real actions on the healthy seed, so each is a
//    state the simulator can legitimately produce.
//  * A secret's value is never stored, logged or returned. A secret is a name, a type and a version.
//  * A failed build or preliminary check never pauses automation; only a failed cutover or a
//    rollback does, and only an explicit `resume` lifts it. Held events are kept, never dropped.
import type {
  AppModel, AppStatus, Deployment, DeployAction, DeployState, HeldEvent, LogLine, NextOutcome, RecoveryOutcome, Release,
  Revision, RuntimeState, ScenarioId, StepName, StepRecord,
} from "./deployTypes.ts";
import { LOG_FAIL_TAIL, LOG_OK, REV, pendingSteps, seedState } from "./deployMock.ts";

const clone = <T>(v: T): T => structuredClone(v);
const setStep = (d: Deployment, name: StepName, patch: Partial<StepRecord>) => {
  const s = d.steps.find((x) => x.name === name);
  if (s) Object.assign(s, patch);
};

export function initialState(scenario: ScenarioId = "healthy"): DeployState {
  return applyScenario(seedState(), scenario);
}

/** Reach a scenario by replaying real actions on the healthy seed. */
export function applyScenario(base: DeployState, scenario: ScenarioId): DeployState {
  let s: DeployState = { ...clone(seedState()), keychain: base.keychain, scenario };
  const run = (outcome: NextOutcome, advances: number | "all", keychain?: "locked" | "unlocked") => {
    if (keychain) s.keychain = "unlocked";
    s = reduce(s, { type: "set-next-outcome", outcome });
    s = reduce(s, { type: "start-deploy", appId: "api", trigger: "poll", revision: REV.next });
    for (let i = 0; (advances === "all" ? s.job : i < advances && s.job); i++) s = reduce(s, { type: "advance" });
  };
  switch (scenario) {
    case "building": run("success", 2); break;
    case "build-failed": run("build-failure", "all"); break;
    case "cutover-restored": run("cutover-restored", "all"); break;
    case "restore-failed": run("restore-failed", "all"); break;
    case "recovering": run("restore-failed", 6); break; // stop just before the restoration finishes
    case "rolled-back-paused": s = reduce(s, { type: "rollback", appId: "api", releaseId: "r-41" }); break;
    case "empty": s = { ...s, projects: [], apps: {} }; break;
    default: break;
  }
  s.nextOutcome = "success";
  s.scenario = scenario;
  return s;
}

// ------------------------------------------------------------------ simulated phases

function newDeployment(s: DeployState, trigger: Deployment["trigger"], revision: Revision): Deployment {
  const n = s.nextDeploymentNumber++;
  const d: Deployment = {
    id: `d-${n}`, env: "production", status: "running", trigger, revision, releaseId: null, ageMin: 0, durationLabel: "0:00", steps: pendingSteps(), buildLog: [],
  };
  setStep(d, "source", { status: "running" });
  return d;
}

function jobDeployment(app: AppModel, id: string): Deployment {
  const d = app.deployments.find((x) => x.id === id);
  if (!d) throw new Error(`simulation lost deployment ${id}`);
  return d;
}

function advanceJob(s: DeployState): DeployState {
  const job = s.job;
  if (!job) return s;
  const app = s.apps[job.appId];
  const d = jobDeployment(app, job.deploymentId);
  const prev = app.currentReleaseId ? app.releases.find((r) => r.id === app.currentReleaseId) ?? null : null;
  const lastKnown = app.lastKnownReleaseId;
  const out = job.outcome;
  switch (job.phase) {
    case 0:
      setStep(d, "source", { status: "succeeded", durationLabel: "3s", note: `revision ${d.revision.sha} on ${d.revision.branch}` });
      setStep(d, "build", { status: "running", note: "step 3 of 6" });
      d.buildLog.push(...LOG_OK.slice(0, 4).map((l): LogLine => ({ ...l })));
      break;
    case 1:
      d.buildLog.push(...LOG_OK.slice(4, 8).map((l): LogLine => ({ ...l })));
      break;
    case 2:
      if (out === "build-failure") {
        d.buildLog.push(...LOG_FAIL_TAIL.map((l): LogLine => ({ ...l })), { kind: "sys", text: `build failed after 18.2s · nothing was stopped${prev ? ` · ${prev.deploymentId} is still serving` : ""}` });
        setStep(d, "build", { status: "failed", durationLabel: "18s", note: "npm run build exited with code 2" });
        d.status = "failed"; d.failedAt = "build"; d.nothingStopped = true; d.durationLabel = "22s";
        d.failure = `Build failed at step 4/6 (npm run build). Nothing was stopped${prev ? `; ${prev.deploymentId} is still serving` : ""}.`;
        for (const n of ["preliminary_check", "cutover", "final_check", "recovery"] as StepName[]) setStep(d, n, { status: "skipped", note: "not reached" });
        s.job = null;
        return s;
      }
      d.buildLog.push(...LOG_OK.slice(8).map((l): LogLine => ({ ...l, text: l.text.replace("{id}", d.id) })), { kind: "sys", text: "build finished in 31.4s · image sha256:7be1…c09" });
      setStep(d, "build", { status: "succeeded", durationLabel: "31s", note: "image sha256:7be1…c09 · 214 MiB" });
      setStep(d, "preliminary_check", { status: "running", note: "probing a temporary port" });
      d.releaseId = `r-${d.id.slice(2)}`;
      break;
    case 3:
      setStep(d, "preliminary_check", { status: "succeeded", durationLabel: "6s", note: "HTTP /healthz 200 on a temporary port; the running release was untouched" });
      if (job.hadPrevious) {
        setStep(d, "cutover", { status: "running", note: `stopping ${prev?.id}, starting ${d.releaseId}` });
        d.interruption = { label: "in progress", ongoing: true };
      } else {
        setStep(d, "cutover", { status: "running", note: "starting the first release · nothing to stop" });
      }
      break;
    case 4:
      setStep(d, "cutover", { status: "succeeded", durationLabel: "2s", note: job.hadPrevious ? `stopped ${prev?.id}, started ${d.releaseId}` : "started the first release" });
      setStep(d, "final_check", { status: "running", note: "probing the real address" });
      break;
    case 5:
      if (out === "success") {
        setStep(d, "final_check", { status: "succeeded", durationLabel: "3s", note: "HTTP /healthz 200" });
        setStep(d, "recovery", { status: "skipped", note: "not needed" });
        d.status = "succeeded"; d.durationLabel = "1m 04s"; d.interruption = job.hadPrevious ? { label: "4 s", ongoing: false } : undefined;
        const rel: Release = {
          id: d.releaseId as string, deploymentId: d.id, image: `harpoon/storefront-${app.name}:${d.id}`, revision: d.revision, ageMin: 0,
          secrets: prev ? prev.secrets.map((x) => ({ ...x })) : [],
        };
        app.releases.unshift(rel);
        app.currentReleaseId = rel.id; app.lastKnownReleaseId = rel.id;
        app.containerName = `${app.projectId}-${app.name}-${rel.id}`;
        s.job = null;
        return s;
      }
      if (!job.hadPrevious) {
        // First deployment: nothing was stopped, so there is nothing to restore and no cutover to fail.
        setStep(d, "final_check", { status: "failed", durationLabel: "20s", note: "HTTP /healthz → 503 ×3" });
        setStep(d, "recovery", { status: "skipped", note: "no previous release to restore" });
        d.status = "failed"; d.failedAt = "final_check"; d.durationLabel = "1m 12s";
        d.failure = "The first release failed its final check: GET /healthz → 503 (3 of 3 attempts). There was no previous release, so nothing was stopped or restored.";
        s.job = null;
        return s;
      }
      setStep(d, "cutover", { status: "warning", note: `stopped ${prev?.id}, started ${d.releaseId}` });
      setStep(d, "final_check", { status: "failed", durationLabel: "20s", note: "HTTP /healthz → 503 ×3" });
      setStep(d, "recovery", { status: "running", note: `starting ${lastKnown} · budget 60 s` });
      d.failedAt = "final_check";
      d.failure = "After cutover the new release failed its final check: GET /healthz → 503 (3 of 3 attempts over 20 s).";
      d.recovery = { outcome: "in_progress", detail: `Restarting ${lastKnown} from its retained image and pinned configuration (one bounded attempt, 60 s budget).` };
      app.currentReleaseId = null; // the previous release is stopped and nothing is confirmed running
      break;
    case 6: {
      const restored = out === "cutover-restored";
      const outcome: RecoveryOutcome = restored ? "restored" : "restore_failed";
      d.status = "failed"; d.durationLabel = "1m 21s";
      if (restored) {
        d.recovery = { outcome, detail: `${lastKnown} was restarted from its retained image and pinned configuration, and passed its own health check after 5.1 s.`, durationLabel: "6.4 s" };
        d.interruption = { label: "38 s", ongoing: false };
        setStep(d, "recovery", { status: "succeeded", durationLabel: "6s", note: `restored ${lastKnown} · verified healthy` });
        app.currentReleaseId = lastKnown;
      } else {
        d.recovery = { outcome, reason: "secret_unavailable", detail: `${lastKnown} could not start: pinned secret STRIPE_KEY@v3 could not be read (Keychain access denied). The application is down.`, durationLabel: "1.8 s" };
        d.interruption = { label: "ongoing", ongoing: true };
        setStep(d, "recovery", { status: "failed", durationLabel: "2s", note: "restore_failed · secret unavailable" });
        app.currentReleaseId = null;
      }
      app.automation = {
        state: "paused", reason: { kind: "failed_cutover", deploymentId: d.id, outcome }, sinceMin: 0,
        held: [...app.automation.held, { id: "evt-9f1", revision: REV.held1, ageMin: 3 }, ...(restored ? [{ id: "evt-9f2", revision: REV.held2, ageMin: 1 }] : [])],
      };
      s.job = null;
      return s;
    }
  }
  job.phase += 1;
  return s;
}

// ------------------------------------------------------------------ reducer

export function reduce(state: DeployState, action: DeployAction): DeployState {
  if (action.type === "set-scenario") return applyScenario(state, action.scenario);
  const s = clone(state);
  switch (action.type) {
    case "set-keychain": s.keychain = action.keychain; return s;
    case "set-next-outcome": s.nextOutcome = action.outcome; return s;
    case "advance": return advanceJob(s);
    case "start-deploy": {
      const app = s.apps[action.appId];
      // Guards: one deployment at a time, and an unreadable secret fails *before any side effect*.
      if (!app || s.job || s.keychain === "locked") return state;
      const revision = action.revision ?? REV.next;
      const d = newDeployment(s, action.trigger, revision);
      app.deployments.unshift(d);
      s.job = { appId: app.id, deploymentId: d.id, outcome: s.nextOutcome, hadPrevious: app.currentReleaseId !== null, phase: 0 };
      return s;
    }
    case "rollback": {
      const app = s.apps[action.appId];
      const rel = app?.releases.find((r) => r.id === action.releaseId);
      if (!app || !rel || s.job || rel.id === app.currentReleaseId) return state;
      const n = s.nextDeploymentNumber++;
      const d: Deployment = {
        id: `d-${n}`, env: "production", status: "succeeded", trigger: "rollback", revision: rel.revision, releaseId: rel.id, ageMin: 0, durationLabel: "7s",
        note: `Rollback to ${rel.id} requested by you`,
        interruption: { label: "4 s", ongoing: false },
        steps: pendingSteps().map((st): StepRecord => st.name === "source" ? { ...st, status: "succeeded", durationLabel: "0s", note: `reusing the retained image of ${rel.deploymentId}` }
          : st.name === "cutover" ? { ...st, status: "succeeded", durationLabel: "4s", note: `stopped ${app.currentReleaseId ?? "nothing (application was down)"}, started ${rel.id}` }
          : st.name === "final_check" ? { ...st, status: "succeeded", durationLabel: "3s", note: "HTTP /healthz 200" }
          : { ...st, status: "skipped", note: "not needed" }),
        buildLog: [],
      };
      app.deployments.unshift(d);
      app.currentReleaseId = rel.id; app.lastKnownReleaseId = rel.id;
      app.containerName = `${app.projectId}-${app.name}-${rel.id}`;
      app.automation = { state: "paused", reason: { kind: "rollback", releaseId: rel.id }, sinceMin: 0, held: app.automation.held };
      return s;
    }
    case "recover": {
      const app = s.apps[action.appId];
      if (!app || s.job || app.currentReleaseId !== null || !app.lastKnownReleaseId) return state;
      const d = app.deployments.find((x) => x.recovery && (x.recovery.outcome === "restore_failed" || x.recovery.outcome === "not_attempted"));
      if (!d || !d.recovery) return state;
      if (s.keychain === "locked") {
        d.recovery = { ...d.recovery, outcome: "restore_failed", reason: "secret_unavailable", detail: `Retry failed: ${app.lastKnownReleaseId} still cannot read its pinned secrets (Keychain locked). The application is still down.` };
        return s;
      }
      d.recovery = { outcome: "restored", detail: `${app.lastKnownReleaseId} was restarted on your request and passed its own health check.`, durationLabel: "5.8 s" };
      d.interruption = { label: "7 m 40 s", ongoing: false };
      setStep(d, "recovery", { status: "succeeded", durationLabel: "6s", note: `restored ${app.lastKnownReleaseId} · verified healthy` });
      app.currentReleaseId = app.lastKnownReleaseId;
      return s; // automation stays paused until the developer resumes it
    }
    case "resume": {
      const app = s.apps[action.appId];
      if (!app || app.automation.state !== "paused") return state;
      const picked: HeldEvent | undefined = action.mode === "deploy" ? (app.automation.held.find((h) => h.id === action.heldEventId) ?? app.automation.held[app.automation.held.length - 1]) : undefined;
      app.automation = { state: "active", held: [] }; // held events are dismissed by the developer's explicit choice
      if (picked && !s.job && s.keychain !== "locked") {
        const d = newDeployment(s, "poll", picked.revision);
        app.deployments.unshift(d);
        s.job = { appId: app.id, deploymentId: d.id, outcome: s.nextOutcome, hadPrevious: app.currentReleaseId !== null, phase: 0 };
      }
      return s;
    }
    case "create-project": {
      const fresh = clone(seedState());
      const name = action.name.trim() || "storefront";
      if (name !== "storefront") fresh.projects[0].name = name;
      return { ...fresh, scenario: "healthy", keychain: s.keychain, nextOutcome: s.nextOutcome };
    }
    case "save-variable": {
      const app = s.apps[action.appId];
      if (!app) return state;
      const existing = app.variables.find((v) => v.name === action.name && v.scope === action.scope);
      if (action.varType === "secret") {
        // The value is deliberately discarded: the contract has no field for it.
        if (existing) { existing.version = (existing.version ?? 0) + 1; existing.changedAgoMin = 0; existing.used = action.used; existing.type = "secret"; delete existing.value; }
        else app.variables.push({ name: action.name, type: "secret", used: action.used, scope: action.scope, version: 1, changedAgoMin: 0 });
      } else if (existing) { existing.value = action.value ?? ""; existing.changedAgoMin = 0; existing.type = "config"; existing.used = action.used; delete existing.version; }
      else app.variables.push({ name: action.name, type: "config", used: action.used, scope: action.scope, value: action.value ?? "", changedAgoMin: 0 });
      return s;
    }
  }
  return state;
}

// ------------------------------------------------------------------ selectors

export function latestDeployment(app: AppModel): Deployment | null {
  return app.deployments[0] ?? null;
}

export function recoveryInProgress(app: AppModel): boolean {
  return app.deployments.some((d) => d.status === "running" && d.recovery?.outcome === "in_progress");
}

/** What an application is *right now*. Derived from records and the runtime only. */
export function appStatus(app: AppModel, runtime: RuntimeState): AppStatus {
  if (runtime !== "connected") {
    return { kind: "unknown", label: "Unknown", tone: "neutral", releaseId: app.currentReleaseId ?? app.lastKnownReleaseId, live: false };
  }
  if (recoveryInProgress(app)) return { kind: "recovering", label: "Recovering", tone: "warn", releaseId: null, live: true };
  if (app.currentReleaseId) {
    const inFlight = app.deployments.some((d) => d.status === "running");
    return { kind: inFlight ? "building" : "running", label: "Running", tone: "ok", releaseId: app.currentReleaseId, live: true };
  }
  if (app.deployments.some((d) => d.status === "running")) return { kind: "building", label: "Deploying", tone: "info", releaseId: null, live: true };
  if (app.lastKnownReleaseId) return { kind: "down", label: "Down", tone: "err", releaseId: null, live: true };
  return { kind: "idle", label: "No release", tone: "neutral", releaseId: null, live: true };
}

export type Attention = { tone: "info" | "warn" | "err"; title: string; text: string };

export function attention(app: AppModel, runtime: RuntimeState): Attention | null {
  if (runtime !== "connected") return null; // the runtime banner speaks for itself; no stale claims
  const st = appStatus(app, runtime);
  const d = latestDeployment(app);
  if (st.kind === "recovering") return { tone: "warn", title: "Needs attention", text: `Cutover failed. Harpoon is restoring ${app.lastKnownReleaseId} (one bounded attempt, 60 s budget).` };
  if (st.kind === "down") return { tone: "err", title: "Application down", text: "The new release failed and the previous release could not be restored. This application is down." };
  const running = app.deployments.find((x) => x.status === "running");
  if (running) return { tone: "info", title: "Deploying", text: `Deploying ${running.id}. The running release (${app.currentReleaseId}) is untouched until the new build passes its preliminary check.` };
  if (d && d.status === "failed" && d.recovery?.outcome === "restored") return { tone: "warn", title: "Needs attention", text: `Cutover to ${d.id} failed; ${app.currentReleaseId} was restored.` };
  if (d && d.status === "failed" && d.failedAt === "build") return { tone: "warn", title: "Needs attention", text: `Latest deployment ${d.id} failed at Build. ${app.currentReleaseId ? `${app.currentReleaseId} is still running` : "No release is running"} — nothing was stopped.` };
  if (d && d.status === "failed" && d.failedAt === "preliminary_check") return { tone: "warn", title: "Needs attention", text: `Latest deployment ${d.id} failed its preliminary check. Nothing was stopped.` };
  if (app.automation.state === "paused" && app.automation.reason?.kind === "rollback") return { tone: "warn", title: "Needs attention", text: `Running ${app.currentReleaseId} after a rollback you requested.` };
  return null;
}

export function appsNeedingAttention(s: DeployState, runtime: RuntimeState): { app: AppModel; attention: Attention }[] {
  if (runtime !== "connected") return [];
  const out: { app: AppModel; attention: Attention }[] = [];
  for (const app of Object.values(s.apps)) {
    const a = attention(app, runtime);
    if (a && a.tone !== "info") out.push({ app, attention: a });
  }
  return out;
}

export function releaseById(app: AppModel, id: string | null): Release | null {
  return id ? app.releases.find((r) => r.id === id) ?? null : null;
}

export type NavBadge = "none" | "attention" | "error";
export function navBadge(s: DeployState, runtime: RuntimeState): NavBadge {
  const items = appsNeedingAttention(s, runtime);
  if (items.some((i) => i.attention.tone === "err") || Object.values(s.apps).some((a) => recoveryInProgress(a))) return runtime === "connected" ? "error" : "none";
  return items.length ? "attention" : "none";
}
