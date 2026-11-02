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
  AppModel, Automation, Deployment, DeployState, Observation, EngineMeta, HeldEvent, KeychainState, LogLine, PauseReason, Project, RecoveryOutcome,
  Release, Revision, RestoreReason, StepName, StepRecord, StepStatus, Trigger, Variable, VariableScope, VariableUse,
} from "./deployTypes.ts";

export type SnapStep = { name: string; status: string; startedMs: number | null; finishedMs: number | null; note: string; log: boolean };
export type SnapEntry = { name: string; kind: "config" | "secret"; build: boolean; runtime: boolean; scope: string; value: string | null; secretId: string | null; version: number | null };
export type SnapResource = { kind: string; name: string; id: string | null; state: string; keep: string };
export type SnapDeployment = {
  id: string; resources: SnapResource[]; status: string; trigger: string; createdMs: number; startedMs: number | null; finishedMs: number | null; steps: SnapStep[];
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
export type SnapSource = { kind: "own" | "external"; path: string; journalFormat: "sqlite" | "legacy" | "none"; readOnly: boolean };
export type EngineSnapshot = {
  schema: number; generatedMs: number; runtimeDir: string; source: SnapSource; store: "missing" | "ok" | "error"; storeError: string | null; writerHeld: boolean;
  gate: { liveDeploy: boolean; liveReplacement: boolean; reason: string };
  repos: SnapRepo[]; apps: SnapApp[]; definitions: SnapDefinition[];
};
export type OpResult = { ok: boolean; exitCode: number; summary: string; detail: string; refusedByGate: boolean };
/** What the views show after an operation: the engine's answer, tagged with the operation that produced it. */
export type LastResult = OpResult & { op: string };
export type StepLog = { step: string; text: string; withheld: boolean; truncated: boolean; atMs: number };

/** What Docker shows for one container right now. `labels` is `null` when Docker's answer carried none to check. */
export type ObservedContainer = { id: string; state: string; labels: Record<string, string> | null; ports: number[] };
/** Container name -> what Docker shows, as observed now. `null` = Docker could not be asked. */
export type Observed = Record<string, ObservedContainer> | null;

/** A runtime attached read-only by path, as the hook holds it. */
export type ExternalInput = { path: string; snapshot: EngineSnapshot | null; error: string | null };

/** What the views may do with the real engine. They never see `invoke`. */
export type EngineApi = {
  run: (op: Record<string, unknown>) => Promise<OpResult>;
  refresh: () => void;
  busy: boolean;
  /** The engine's own answer to the last operation, shown as-is (refusal reasons included). */
  last: LastResult | null;
  /** Held events that appeared since the person last looked (announced once). */
  notice: NewHeld[];
  clearNotice: () => void;
  /** The macOS folder picker; null when cancelled. */
  pickFolder: () => Promise<string | null>;
  /** Attach a runtime read-only by path (validated by the backend). Never changes it. */
  attachRuntime: (path: string) => Promise<{ ok: true } | { ok: false; error: string }>;
  detachRuntime: (path: string) => void;
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
  const toHeld = (e: SnapApp["events"][number]): HeldEvent => ({
    id: e.delivery, status: e.status, source: e.source, receivedMs: e.receivedMs, revisionFull: e.revision, ageMin: min(e.receivedMs, now),
    revision: { sha: e.revision.slice(0, 7), message: `${e.source} delivery ${e.delivery.slice(0, 12)}`, branch: "", author: "" },
  });
  const held = app.events.filter((e) => e.status === "held").map(toHeld);
  const selected = app.events.filter((e) => e.status === "selected").map(toHeld);
  const p = app.pauses[0];
  if (!p) return { state: "active", held, selected };
  let reason: PauseReason;
  if (p.kind === "rollback") reason = { kind: "rollback", releaseId: dep(p.origin)?.release ?? p.context };
  else if (p.kind === "interrupted_cutover") reason = { kind: "interrupted_cutover", deploymentId: p.origin };
  else reason = { kind: "failed_cutover", deploymentId: p.origin, outcome: (dep(p.origin)?.recovery?.outcome as RecoveryOutcome | undefined) ?? "not_attempted" };
  return { state: "paused", reason, sinceMin: min(p.sinceMs, now), held, selected };
}

const baseName = (p: string) => p.replace(/\/+$/, "").split("/").pop() || p;

export function engineMeta(snap: EngineSnapshot | null, loadError: string | null, stale: boolean, externals: ExternalInput[] = []): EngineMeta {
  return {
    generatedMs: snap?.generatedMs ?? 0, store: snap?.store ?? "missing", storeError: snap?.storeError ?? null, writerHeld: snap?.writerHeld ?? false,
    gate: snap?.gate ?? { liveDeploy: false, liveReplacement: false, reason: "The deployment engine has not answered yet." },
    repos: (snap?.repos ?? []).map((r) => ({ name: r.name, url: r.url, app: r.app, project: r.project, branch: r.branch, containerPort: r.containerPort, dockerfile: r.dockerfile, healthPath: r.healthPath, seen: r.seen })),
    loadError, stale,
    externals: externals.map((x, i) => ({ id: `ext${i}`, path: x.path, format: x.snapshot?.source.journalFormat ?? "none", store: x.snapshot?.store ?? "missing", error: x.error ?? x.snapshot?.storeError ?? null })),
  };
}

/** The applications of one snapshot. `ext` marks a runtime attached read-only: ids are prefixed so they can never collide with, or be mistaken for, Harpoon's own. */
function appsFrom(snap: EngineSnapshot, observed: Observed, ext?: { id: string; path: string }): { apps: Record<string, AppModel>; projects: Project[] } {
  const now = snap.generatedMs;
  const apps: Record<string, AppModel> = {};
  const projects: Record<string, Project> = {};
  const idOf = (id: string) => (ext ? `${ext.id}/${id}` : id);
  for (const a of snap.apps) {
    const repo = snap.repos.find((r) => r.app === a.id) ?? null;
    const byId = new Map(a.deployments.map((d) => [d.id, d] as const));
    const deployments = a.deployments.map((d) => ({ ...deploymentOf(d, now, snap.writerHeld), recorded: d.resources.map((r) => ({ kind: r.kind, name: r.name, id: r.id, state: r.state })) })).sort((x, y) => x.ageMin - y.ageMin);
    const releases: Release[] = a.deployments.filter((d) => d.release).map((d) => ({
      id: d.release!, deploymentId: d.id, image: d.pin?.imageTag ?? d.resources.find((r) => r.kind === "image")?.name ?? "", revision: revisionOf(d), ageMin: min(d.finishedMs ?? d.createdMs, now),
      secrets: (d.pin?.secrets ?? []).map((s) => ({ name: s.name, version: s.version })),
    })).sort((x, y) => x.ageMin - y.ageMin);
    const sv = a.serving;
    const observation = correlate(a, observed) ?? undefined;
    // current = recorded as serving AND observed running as that deployment's container (labels checked when Docker gave them)
    const running = !!sv && (observation?.kind === "match" || observation?.kind === "unverified");
    const servDep = sv ? a.deployments.find((d) => d.id === sv.deployment) : undefined;
    const lease = servDep?.resources.find((r) => r.kind === "port" && r.state === "created");
    const leasePort = lease ? Number(lease.name) : null;
    const port = sv?.hostPort ?? leasePort;
    const healthPath = sv?.healthPath ?? repo?.healthPath ?? "/";
    const pin = [...a.deployments].reverse().find((d) => d.pin)?.pin ?? null;
    const projectId = ext ? ext.id : repo?.project ?? repo?.name ?? "ungrouped";
    apps[idOf(a.id)] = {
      id: idOf(a.id), projectId, name: repo?.name ?? a.id,
      ...(ext ? { external: { path: ext.path, format: snap.source.journalFormat } } : {}),
      observation, urlSource: sv?.hostPort != null ? "pin" : leasePort ? "lease" : undefined,
      source: repo ? { kind: "git", label: displayUrl(repo.url), detail: repo.branch } : { kind: "folder", label: a.id, detail: ext ? "recorded in an external runtime" : "no repository registered" },
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
    const pid = projectId;
    (projects[pid] ??= ext
      ? { id: pid, name: `External · ${baseName(ext.path)}`, sourceLabel: `read-only · ${snap.source.journalFormat === "legacy" ? "legacy journal.wal, not migrated" : "SQLite journal"}`, appIds: [] }
      : { id: pid, name: pid, sourceLabel: repo ? "Git repository" : "recorded deployments", appIds: [] }).appIds.push(idOf(a.id));
  }
  return { apps, projects: Object.values(projects) };
}

/**
 * `observed` is what Docker shows now. An application is current only when the container its record names was
 * seen running and carries its ownership labels; otherwise that release is only "last known", and the mismatch is
 * reported on the application (`observation`). `externals` are runtimes attached read-only.
 */
export function toDeployState(snap: EngineSnapshot | null, o: { observed: Observed; keychain: KeychainState; loadError?: string | null; stale?: boolean; externals?: ExternalInput[] }): DeployState {
  const externals = o.externals ?? [];
  const engine = engineMeta(snap, o.loadError ?? null, o.stale ?? false, externals);
  const base = { keychain: o.keychain, nextOutcome: "success" as const, job: null, nextDeploymentNumber: 1, engine };
  if (!snap) return { ...base, scenario: o.loadError ? "empty" : "loading", projects: [], apps: {} };
  const own = snap.store === "error" ? { apps: {}, projects: [] as Project[] } : appsFrom(snap, o.observed);
  const apps: Record<string, AppModel> = { ...own.apps };
  const projects: Project[] = [...own.projects];
  externals.forEach((x, i) => {
    if (!x.snapshot || x.snapshot.store === "error") return;
    const e = appsFrom(x.snapshot, o.observed, { id: `ext${i}`, path: x.path });
    Object.assign(apps, e.apps);
    projects.push(...e.projects);
  });
  if (snap.store === "error") return { ...base, scenario: "empty", projects, apps };
  return { ...base, scenario: Object.keys(apps).length ? "healthy" : "empty", projects, apps };
}

/** Is this application one attached read-only? (Its ids are prefixed `ext<n>/`; Harpoon's own never are.) */
export const isExternalAppId = (id: unknown): boolean => typeof id === "string" && /^ext\d+\//.test(id);

/** Operations act on Harpoon's own store only. The backend refuses anything else; this keeps the request from being sent. */
export function externalOpProblem(op: Record<string, unknown>): string | null {
  return isExternalAppId(op.app) ? "That application belongs to a runtime attached read-only; Harpoon does not change it." : null;
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

/** The Docker label keys the engine stamps on every object it creates (`deploy/src/ids.rs`). */
export const LABEL_MANAGED = "io.harpoon.managed";
export const LABEL_APP = "io.harpoon.app";
export const LABEL_DEPLOYMENT = "io.harpoon.deployment";

/** What Docker shows, by container name, from `docker ps` rows (names, id, state, labels, published ports). */
export function observedFrom(rows: unknown): Observed {
  if (!Array.isArray(rows)) return null;
  const out: Record<string, ObservedContainer> = {};
  for (const r of rows as Record<string, unknown>[]) {
    const labelText = typeof r.Labels === "string" ? r.Labels : null;
    const labels = labelText === null ? null : Object.fromEntries(labelText.split(",").filter(Boolean).map((kv) => { const i = kv.indexOf("="); return i < 0 ? [kv, ""] : [kv.slice(0, i), kv.slice(i + 1)]; }));
    const ports = [...String(r.Ports ?? "").matchAll(/:(\d+)->/g)].map((m) => Number(m[1]));
    const row: ObservedContainer = { id: String(r.ID ?? r.Id ?? ""), state: String(r.State ?? "").toLowerCase(), labels, ports };
    for (const n of String(r.Names ?? r.Name ?? "").split(",")) if (n) out[n.replace(/^\//, "")] = row;
  }
  return out;
}

const shortId = (id: string) => id.replace(/^sha256:/, "").slice(0, 12);

/**
 * Recorded vs observed for an application's serving container. The journal says what Harpoon (or the tool that
 * wrote it) did; Docker says what exists now. They are compared, never merged: a record is "current" only when the
 * container it names is there, running, and carries this application's and this deployment's ownership labels.
 */
export function correlate(app: SnapApp, observed: Observed): Observation | null {
  const sv = app.serving;
  if (!sv) return null;
  const dep = app.deployments.find((d) => d.id === sv.deployment);
  const rec = dep?.resources.find((r) => r.kind === "container" && r.name === sv.container);
  const lease = dep?.resources.find((r) => r.kind === "port" && r.state === "created");
  const leasePort = lease ? Number(lease.name) : null;
  const recorded = `release ${sv.release} serving in container ${sv.container}${rec?.id ? ` (${shortId(rec.id)})` : ""}${leasePort ? `, port lease ${leasePort}` : ""}`;
  if (observed === null) return { kind: "unknown", recorded, observed: "Docker could not be asked", notes: ["The record cannot be compared with Docker right now."] };
  const row = observed[sv.container];
  if (!row) return { kind: "absent", recorded, observed: "no container with that name", notes: ["Recorded as serving, but Docker has no such container."] };
  const notes: string[] = [];
  if (rec?.id && row.id && !(row.id === rec.id || row.id.startsWith(rec.id) || rec.id.startsWith(row.id))) notes.push(`The container id differs: recorded ${shortId(rec.id)}, observed ${shortId(row.id)}.`);
  if (row.labels) {
    if (row.labels[LABEL_MANAGED] !== "true") notes.push(`The container is not labelled ${LABEL_MANAGED}=true.`);
    if (row.labels[LABEL_APP] !== app.id) notes.push(`Its ${LABEL_APP} label is ${JSON.stringify(row.labels[LABEL_APP] ?? "")}, not ${app.id}.`);
    if (row.labels[LABEL_DEPLOYMENT] !== sv.deployment) notes.push(`Its ${LABEL_DEPLOYMENT} label is ${JSON.stringify(row.labels[LABEL_DEPLOYMENT] ?? "")}, not ${sv.deployment}.`);
  }
  if (leasePort && row.ports.length && !row.ports.includes(leasePort)) notes.push(`The recorded port lease ${leasePort} is not among the published ports (${row.ports.join(", ")}).`);
  const observedText = `container ${shortId(row.id)} ${row.state}${row.labels ? ", ownership labels checked" : ", no labels to check"}${row.ports.length ? `, published ${row.ports.join(", ")}` : ""}`;
  if (notes.length) return { kind: "mismatch", recorded, observed: observedText, notes };
  if (row.state !== "running") return { kind: "stopped", recorded, observed: observedText, notes: [`Recorded as serving, but the container is ${row.state || "not running"}.`] };
  if (!row.labels) return { kind: "unverified", recorded, observed: observedText, notes: ["Docker's answer carried no labels, so ownership could not be checked."] };
  return { kind: "match", recorded, observed: observedText, notes: [] };
}

/**
 * The `file://` URL for an absolute local folder: every UTF-8 byte outside `A-Za-z0-9/._-~` is percent-encoded
 * (so spaces, `%`, `#`, `?` and non-ASCII are safe), and the engine decodes it exactly once. `null` when the path
 * is not an absolute path the engine would accept (relative, a `.`/`..` component, a control character).
 */
export function fileUrlFor(path: string): string | null {
  const p = path.startsWith("file://") ? path.slice(7) : path;
  // eslint-disable-next-line no-control-regex
  if (!p.startsWith("/") || /[\u0000-\u001f\u007f]/.test(p) || p.split("/").some((c) => c === ".." || c === ".")) return null;
  let out = "file://";
  for (const b of new TextEncoder().encode(p)) {
    const c = String.fromCharCode(b);
    out += /[A-Za-z0-9/._~-]/.test(c) ? c : `%${b.toString(16).toUpperCase().padStart(2, "0")}`;
  }
  return out;
}

/** A recorded repository URL as a person reads it: a file URL is shown as its folder path. Anything that does not decode cleanly is shown as recorded. */
export function displayUrl(url: string): string {
  if (!url.startsWith("file://")) return url;
  try {
    const d = decodeURIComponent(url.slice(7));
    // eslint-disable-next-line no-control-regex
    return d.startsWith("/") && !/[\u0000-\u001f\u007f]/.test(d) ? d : url;
  } catch { return url; }
}

/** A repository name from a folder's last component: the characters the engine accepts, nothing else. */
export function suggestRepoName(path: string): string {
  const last = path.replace(/\/+$/, "").split("/").pop() ?? "";
  return last.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^[^A-Za-z0-9]+/, "").slice(0, 64);
}

/** Reserved by Harpoon: the engine refuses these names (`deploy/src/envvars.rs`). */
export const RESERVED_ENV_PREFIX = "HARPOON_";
const MAX_NAME_BYTES = 128;
const MAX_VALUE_BYTES = 4096;

/** The same rules the engine applies to a variable name, so the form can say what is wrong before anything is sent. `null` = acceptable. */
export function variableNameProblem(name: string): string | null {
  if (name === "") return null; // empty is "not filled in yet", not an error to shout about
  if (new TextEncoder().encode(name).length > MAX_NAME_BYTES) return `Names can be at most ${MAX_NAME_BYTES} bytes.`;
  if (!/^[A-Za-z_]/.test(name)) return "A name must start with a letter or an underscore.";
  if (!/^[A-Za-z0-9_]+$/.test(name)) return "Use only letters, digits and underscores.";
  if (name.startsWith(RESERVED_ENV_PREFIX)) return `Names starting with ${RESERVED_ENV_PREFIX} are reserved for Harpoon (for example the per-container response marker) and cannot be set.`;
  return null;
}

/** The engine's limits on a Config value (a Secret has its own rules and is never inspected here). */
export function configValueProblem(value: string): string | null {
  if (new TextEncoder().encode(value).length > MAX_VALUE_BYTES) return `A value can be at most ${MAX_VALUE_BYTES} bytes.`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(value)) return "A value cannot contain control characters (newlines included).";
  return null;
}

export type NewHeld = { appId: string; app: string; eventId: string; sha: string };

/** Held events present in `next` that were not in `seen`. The caller owns `seen` (and seeds it from the first snapshot, so launching the app does not announce old events). */
export function newHeldEvents(next: DeployState, seen: Set<string>): NewHeld[] {
  const out: NewHeld[] = [];
  for (const a of Object.values(next.apps)) {
    for (const h of a.automation.held) {
      const key = `${a.id}/${h.id}`;
      if (h.status === "held" && !seen.has(key)) out.push({ appId: a.id, app: a.name, eventId: h.id, sha: h.revision.sha });
    }
  }
  return out;
}

/** Every event of the snapshot as `seen` keys, whatever its status: a decided event is never "new" again. */
export function allEventKeys(snap: EngineSnapshot): string[] {
  return snap.apps.flatMap((a) => a.events.map((e) => `${a.id}/${e.delivery}`));
}

/**
 * One poll's worth of announcement logic. `seen` is `null` until the first snapshot, which only seeds it: opening
 * the app does not announce events that were already there. Afterwards a held event is announced exactly once.
 * Returns the new `seen` and the events to announce.
 */
export function announceHeld(snap: EngineSnapshot, seen: Set<string> | null): { seen: Set<string>; fresh: NewHeld[] } {
  if (seen === null) return { seen: new Set(snap.store === "ok" ? allEventKeys(snap) : []), fresh: [] };
  if (snap.store !== "ok") return { seen, fresh: [] };
  const fresh = newHeldEvents(toDeployState(snap, { observed: null, keychain: "unlocked" }), seen);
  return { seen: new Set([...seen, ...allEventKeys(snap)]), fresh };
}

/** What a "Check for new commits" pass found, in a sentence, from the engine's own output lines. */
export function summarizePoll(ok: boolean, detail: string): string {
  const lines = detail.split("\n");
  const queued = lines.filter((l) => /queued as /.test(l)).length;
  const baseline = lines.filter((l) => /\(baseline;/.test(l)).length;
  if (!ok) return "Checking for new commits did not complete.";
  if (queued > 0) return `${queued} new commit${queued === 1 ? "" : "s"} recorded as held event${queued === 1 ? "" : "s"}. Nothing was deployed.`;
  if (baseline > 0) return `Starting point recorded for ${baseline} repositor${baseline === 1 ? "y" : "ies"}; existing history is not deployed.`;
  return "No new commits since the last check.";
}

/** Why a mutation is disabled in this build, or null when the engine would accept it. */
export function liveBlock(s: DeployState): string | null {
  const g = s.engine?.gate;
  return g && (!g.liveDeploy || !g.liveReplacement) ? g.reason : null;
}
