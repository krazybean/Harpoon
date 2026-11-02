// Harpoon Deployments — the REAL data contract (Milestone E).
//
// `EngineSnapshot` is what the Tauri side (`deploy_api.rs`) reads from the deployment engine's own journal:
// recorded facts only (millisecond timestamps, statuses, ids). `toDeployState` turns it into the shape the
// existing Deployments views render. Rules, all covered by deployEngine.test.mjs:
//  * Nothing is invented. No snapshot means no apps; an unreadable store is an error, not an empty product.
//  * Recorded evidence and current Docker state are separate: an application is "running" only when its
//    recorded serving container was OBSERVED running; otherwise the release is only "last known".
//  * A deployment that never finished and has no live writer is "interrupted", not "running".
//  * Secrets are names, ids and versions. Values are never in the snapshot.
// Strip-types friendly: no enums, no parameter properties.
import type {
  AppModel, Automation, Deployment, DeployState, EngineMeta, HeldEvent, KeychainState, LogLine, PauseReason, Project, RecoveryOutcome,
  Release, Revision, RestoreReason, StepName, StepRecord, StepStatus, Trigger, Variable, VariableScope, VariableUse,
} from "./deployTypes.ts";

export type SnapStep = { name: string; status: string; startedMs: number | null; finishedMs: number | null; note: string; log: boolean };
export type SnapEntry = { name: string; kind: "config" | "secret"; build: boolean; runtime: boolean; scope: string; value: string | null; secretId: string | null; version: number | null };
export type SnapDeployment = {
  id: string; status: string; trigger: string; createdMs: number; startedMs: number | null; finishedMs: number | null; steps: SnapStep[];
  cutover: string; cutoverAtMs: number | null;
  failure: { step: string; kind: string; message: string; timedOut: boolean } | null;
  recovery: { outcome: string; reason: string | null; reasonText: string | null; startedMs: number; finishedMs: number | null; budgetMs: number } | null;
  interruption: { startedMs: number; endedMs: number | null } | null;
  release: string | null; replaces: string | null; rollbackTo: string | null; mayBeDown: boolean; unfinished: boolean;
  source: { repo: string; url: string; selector: string; requested: string; commit: string; trigger: string; delivery: string | null } | null;
  pin: { imageTag: string; imageId: string; hostPort: number; containerPort: number; healthPath: string; envNames: string[]; secrets: { name: string; id: string; version: number }[]; snapshot: string | null; build: string | null } | null;
  config: SnapEntry[] | null; configDiff: string[];
};
export type SnapApp = {
  id: string; repo: string | null; deployments: SnapDeployment[];
  serving: { release: string; deployment: string; container: string; via: string; sinceMs: number; hostPort: number | null; healthPath: string | null } | null;
  pauses: { origin: string; kind: string; sinceMs: number; context: string; recorded: boolean }[];
  events: { delivery: string; source: string; revision: string; receivedMs: number; status: string; decidedMs: number | null; related: string | null }[];
};
export type SnapRepo = {
  name: string; url: string; app: string; project: string | null; environment: string | null; branch: string; filters: string[]; credential: string; webhook: string | null;
  containerPort: number; dockerfile: string | null; context: string | null; target: string | null; healthPath: string | null; seen: { refName: string; commit: string; atMs: number }[];
};
export type SnapDefinition = { scope: string; name: string; kind: string; build: boolean; runtime: boolean; value: string | null; secretId: string | null; version: number | null; atMs: number };
export type EngineSnapshot = {
  schema: number; generatedMs: number; runtimeDir: string; store: "missing" | "ok" | "error"; storeError: string | null; writerHeld: boolean;
  gate: { liveDeploy: boolean; liveReplacement: boolean; reason: string };
  repos: SnapRepo[]; apps: SnapApp[]; definitions: SnapDefinition[];
};
export type OpResult = { ok: boolean; exitCode: number; summary: string; detail: string; refusedByGate: boolean };
export type StepLog = { step: string; text: string; withheld: boolean; truncated: boolean; atMs: number };

/** Container name -> Docker state, as observed now. `null` = Docker could not be asked. */
export type Observed = Record<string, string> | null;

/** What the views may do with the real engine. They never see `invoke`. */
export type EngineApi = {
  run: (op: Record<string, unknown>) => Promise<OpResult>;
  refresh: () => void;
  busy: boolean;
  /** The engine's own answer to the last operation, shown as-is (refusal reasons included). */
  last: OpResult | null;
  clearLast: () => void;
  stepLogs: (deploymentId: string) => Promise<LogLine[]>;
  runtimeLogs: (container: string) => Promise<LogLine[]>;
};

export const SCHEMA = 1;
const STEPS: StepName[] = ["source", "build", "preliminary_check", "cutover", "final_check", "recovery"];
const RESTORE_REASONS: RestoreReason[] = ["secret_unavailable", "image_missing", "timeout", "start_failed", "docker_unreachable"];
const min = (ms: number, now: number) => Math.max(0, (now - ms) / 60000);
export function durationLabel(ms: number | null): string {
  if (ms == null) return "—";
  const s = Math.round(ms / 1000);
  return s < 1 ? `${Math.max(0, Math.round(ms))} ms` : s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}
const TRIGGERS: Record<string, Trigger> = { manual: "manual", cli: "manual", poll: "poll", webhook: "poll", rollback: "rollback", promote: "promote" };

function revisionOf(d: SnapDeployment): Revision {
  if (d.source) return { sha: d.source.commit.slice(0, 7) || "—", message: `${d.source.selector} ${d.source.requested}`.trim(), branch: d.source.requested, author: d.source.url };
  return { sha: "—", message: "no recorded source", branch: "", author: "" };
}

function stepOf(s: SnapStep, now: number): StepRecord | null {
  if (!STEPS.includes(s.name as StepName)) return null;
  const status: StepStatus = s.status === "interrupted" ? "warning" : (["pending", "running", "succeeded", "failed", "skipped"].includes(s.status) ? (s.status as StepStatus) : "pending");
  const took = s.startedMs != null ? (s.finishedMs ?? now) - s.startedMs : null;
  return { name: s.name as StepName, status, durationLabel: took == null ? "" : durationLabel(took), note: s.status === "interrupted" ? `${s.note} (interrupted)`.trim() : s.note };
}

export function deploymentOf(d: SnapDeployment, now: number, writerHeld: boolean): Deployment {
  // Not finished and nothing holds the journal: the process that ran it is gone.
  const abandoned = d.unfinished && !writerHeld;
  const status: Deployment["status"] = abandoned ? "interrupted" : d.status === "requested" || d.status === "queued" || d.status === "running" ? "running" : (d.status as Deployment["status"]);
  const steps = d.steps.map((s) => stepOf(s, now)).filter((s): s is StepRecord => s !== null);
  if (abandoned) for (const s of steps) if (s.status === "running") { s.status = "warning"; s.note = `${s.note} (the process stopped here)`.trim(); }
  const out: Deployment = {
    id: d.id, env: "production", status, trigger: TRIGGERS[d.trigger] ?? "manual", revision: revisionOf(d), releaseId: d.release, ageMin: min(d.createdMs, now),
    durationLabel: d.startedMs != null && d.finishedMs != null ? durationLabel(d.finishedMs - d.startedMs) : "—", steps, buildLog: [],
  };
  if (d.failure) {
    out.failedAt = STEPS.includes(d.failure.step as StepName) ? (d.failure.step as StepName) : undefined;
    out.failure = d.failure.message;
    out.nothingStopped = d.failure.kind === "source" || d.failure.kind === "build" || d.failure.kind === "preliminary_check";
  } else if (abandoned) {
    out.failure = d.mayBeDown
      ? "Harpoon stopped while this deployment was switching releases. The application may be down — check the container state before relying on it."
      : "Harpoon stopped before this deployment finished. Nothing recorded says it changed a running release.";
    out.nothingStopped = !d.mayBeDown && d.cutover === "not_started";
  }
  if (d.recovery) {
    const lost = d.recovery.outcome === "in_progress" && !writerHeld;
    const outcome: RecoveryOutcome = lost ? "restore_failed" : (d.recovery.outcome as RecoveryOutcome);
    out.recovery = {
      outcome,
      detail: lost ? "Recovery was interrupted and its result is unknown. Check the container state; do not assume the previous release is running." : d.recovery.reasonText ?? "",
      durationLabel: d.recovery.finishedMs != null ? durationLabel(d.recovery.finishedMs - d.recovery.startedMs) : undefined,
      reason: RESTORE_REASONS.includes(d.recovery.reason as RestoreReason) ? (d.recovery.reason as RestoreReason) : undefined,
    };
  }
  if (d.interruption) {
    const ongoing = d.interruption.endedMs == null;
    out.interruption = { ongoing, label: ongoing ? (writerHeld ? "ongoing" : "unknown end (the process stopped)") : durationLabel(d.interruption.endedMs! - d.interruption.startedMs) };
  }
  if (d.configDiff.length) out.note = `Configuration changes since the previous snapshot: ${d.configDiff.join("; ")}`;
  return out;
}

const USED = (build: boolean, runtime: boolean, secret: boolean): VariableUse => (build && runtime ? "build+runtime" : build ? (secret ? "build secret" : "build") : "runtime");
const scopeKind = (slug: string): VariableScope => (slug === "global" ? "global" : slug.startsWith("project:") ? "project" : slug.includes("@") ? "environment" : "application");

function variablesOf(snap: EngineSnapshot, app: SnapApp, now: number): Variable[] {
  const project = snap.repos.find((r) => r.app === app.id)?.project ?? null;
  const base = (slug: string) => slug.split("@")[0];
  const mine = (slug: string) => base(slug) === "global" || base(slug) === `app:${app.id}` || (project != null && base(slug) === `project:${project}`);
  return snap.definitions.filter((x) => x.kind !== "unset" && mine(x.scope)).map((x): Variable => ({
    name: x.name, type: x.kind === "secret" ? "secret" : "config", used: USED(x.build, x.runtime, x.kind === "secret"), scope: scopeKind(x.scope),
    value: x.kind === "config" ? x.value ?? "" : undefined, version: x.kind === "secret" ? x.version ?? undefined : undefined, changedAgoMin: min(x.atMs, now),
  }));
}

function automationOf(app: SnapApp, now: number, dep: (id: string) => SnapDeployment | undefined): Automation {
  const held: HeldEvent[] = app.events.filter((e) => e.status === "held" || e.status === "selected").map((e) => ({
    id: e.delivery, status: e.status, revisionFull: e.revision, ageMin: min(e.receivedMs, now),
    revision: { sha: e.revision.slice(0, 7), message: `${e.source} delivery ${e.delivery.slice(0, 12)}`, branch: "", author: "" },
  }));
  const p = app.pauses[0];
  if (!p) return { state: "active", held };
  let reason: PauseReason;
  if (p.kind === "rollback") reason = { kind: "rollback", releaseId: dep(p.origin)?.release ?? p.context };
  else if (p.kind === "interrupted_cutover") reason = { kind: "interrupted_cutover", deploymentId: p.origin };
  else reason = { kind: "failed_cutover", deploymentId: p.origin, outcome: (dep(p.origin)?.recovery?.outcome as RecoveryOutcome | undefined) ?? "not_attempted" };
  return { state: "paused", reason, sinceMin: min(p.sinceMs, now), held };
}

export function engineMeta(snap: EngineSnapshot | null, loadError: string | null, stale: boolean): EngineMeta {
  return {
    generatedMs: snap?.generatedMs ?? 0, store: snap?.store ?? "missing", storeError: snap?.storeError ?? null, writerHeld: snap?.writerHeld ?? false,
    gate: snap?.gate ?? { liveDeploy: false, liveReplacement: false, reason: "The deployment engine has not answered yet." },
    repos: (snap?.repos ?? []).map((r) => ({ name: r.name, url: r.url, app: r.app, project: r.project, branch: r.branch, containerPort: r.containerPort, dockerfile: r.dockerfile, healthPath: r.healthPath, seen: r.seen })),
    loadError, stale,
  };
}

/**
 * `observed` is the current Docker state of containers by name. An application is current only when the
 * container its recorded serving release lives in was seen running; otherwise that release is "last known".
 */
export function toDeployState(snap: EngineSnapshot | null, o: { observed: Observed; keychain: KeychainState; loadError?: string | null; stale?: boolean }): DeployState {
  const engine = engineMeta(snap, o.loadError ?? null, o.stale ?? false);
  const base = { keychain: o.keychain, nextOutcome: "success" as const, job: null, nextDeploymentNumber: 1, engine };
  if (!snap) return { ...base, scenario: o.loadError ? "empty" : "loading", projects: [], apps: {} };
  if (snap.store === "error") return { ...base, scenario: "empty", projects: [], apps: {} };
  const now = snap.generatedMs;
  const apps: Record<string, AppModel> = {};
  const projects: Record<string, Project> = {};
  for (const a of snap.apps) {
    const repo = snap.repos.find((r) => r.app === a.id) ?? null;
    const byId = new Map(a.deployments.map((d) => [d.id, d] as const));
    const deployments = a.deployments.map((d) => deploymentOf(d, now, snap.writerHeld)).sort((x, y) => x.ageMin - y.ageMin);
    const releases: Release[] = a.deployments.filter((d) => d.release).map((d) => ({
      id: d.release!, deploymentId: d.id, image: d.pin?.imageTag ?? "", revision: revisionOf(d), ageMin: min(d.finishedMs ?? d.createdMs, now),
      secrets: (d.pin?.secrets ?? []).map((s) => ({ name: s.name, version: s.version })),
    })).sort((x, y) => x.ageMin - y.ageMin);
    const sv = a.serving;
    const seen = sv ? o.observed?.[sv.container] : undefined;
    const running = !!sv && seen === "running";
    const port = sv?.hostPort ?? null;
    const healthPath = sv?.healthPath ?? repo?.healthPath ?? "/";
    const pin = [...a.deployments].reverse().find((d) => d.pin)?.pin ?? null;
    const projectId = repo?.project ?? repo?.name ?? "ungrouped";
    apps[a.id] = {
      id: a.id, projectId, name: repo?.name ?? a.id,
      source: repo ? { kind: "git", label: repo.url, detail: repo.branch } : { kind: "folder", label: a.id, detail: "no repository registered" },
      url: running && port ? `http://127.0.0.1:${port}` : "", containerName: sv?.container ?? "",
      currentReleaseId: running ? sv!.release : null, lastKnownReleaseId: sv?.release ?? null,
      deployments, releases, automation: automationOf(a, now, (id) => byId.get(id)),
      variables: variablesOf(snap, a, now),
      settings: {
        dockerfile: repo?.dockerfile ?? "Dockerfile", context: repo?.context ?? ".", buildArgs: "", port: repo?.containerPort ?? pin?.containerPort ?? null,
        healthCheck: `HTTP GET ${healthPath}`, keepReleases: 0, productionBranch: repo?.branch ?? "", pollSeconds: 0,
      },
      previews: [], resources: null,
    };
    (projects[projectId] ??= { id: projectId, name: projectId, sourceLabel: repo ? "Git repository" : "recorded deployments", appIds: [] }).appIds.push(a.id);
  }
  return { ...base, scenario: snap.apps.length ? "healthy" : "empty", projects: Object.values(projects), apps };
}

/** A recorded step log as the views' log lines. The text was redacted by the engine before it was stored. */
export function logLines(logs: StepLog[]): LogLine[] {
  const out: LogLine[] = [];
  for (const l of logs) {
    out.push({ kind: "step", text: `── ${l.step}${l.truncated ? " (earlier output was cut; the tail is kept)" : ""}` });
    if (l.withheld) { out.push({ kind: "warn", text: "Output withheld: it could not be checked for secrets." }); continue; }
    for (const t of l.text.split("\n")) if (t !== "") out.push({ kind: /\berror\b|failed/i.test(t) ? "err" : "", text: t });
  }
  return out;
}

/** The observed Docker state by container name, from `docker ps` rows. */
export function observedFrom(rows: unknown): Observed {
  if (!Array.isArray(rows)) return null;
  const out: Record<string, string> = {};
  for (const r of rows as Record<string, unknown>[]) {
    const names = String(r.Names ?? r.Name ?? "");
    for (const n of names.split(",")) if (n) out[n.replace(/^\//, "")] = String(r.State ?? "").toLowerCase();
  }
  return out;
}

/** Why a mutation is disabled in this build, or null when the engine would accept it. */
export function liveBlock(s: DeployState): string | null {
  const g = s.engine?.gate;
  return g && (!g.liveDeploy || !g.liveReplacement) ? g.reason : null;
}
