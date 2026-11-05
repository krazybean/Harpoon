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
import { exposureOf, parseDockerPorts, type HostListeners, type PortMapping } from "./portExposure.ts";
import type {
  AppAddress, AppModel, Automation, ConfigFact, ConfigFacts, Deployment, DeployState, Observation, ObservationCheck, EngineMeta, HeldEvent, KeychainState, LogLine, PauseReason, Project, RecoveryOutcome,
  EngineRepo, Release, Revision, RestoreReason, StepName, StepRecord, StepStatus, Trigger, Variable, VariableScope, VariableUse,
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
  id: string; repo: string | null;
  /** Present while the application has no registered repository because its registration was removed. */
  disconnected?: { name: string; url: string; registeredMs: number; removedMs: number } | null;
  /** The stable `<label>.harpoon.localhost` name of the application, and whether it routes (serving / idle / conflict / external). */
  hostname?: string; route?: "serving" | "idle" | "conflict" | "external"; deployments: SnapDeployment[];
  serving: { release: string; deployment: string; container: string; via: string; sinceMs: number; hostPort: number | null; healthPath: string | null } | null;
  pauses: { origin: string; kind: string; sinceMs: number; context: string; recorded: boolean }[];
  events: { delivery: string; source: string; revision: string; receivedMs: number; status: string; decidedMs: number | null; related: string | null }[];
};
export type SnapRepo = {
  name: string; url: string; app: string; project: string | null; environment: string | null; branch: string; filters: string[]; credential: string; webhook: string | null;
  containerPort: number; dockerfile: string | null; context: string | null; target: string | null; healthPath: string | null; registeredMs: number; seen: { refName: string; commit: string; atMs: number }[];
};
export type SnapDefinition = { scope: string; name: string; kind: string; build: boolean; runtime: boolean; value: string | null; secretId: string | null; version: number | null; atMs: number };
export type SnapSource = { kind: "own" | "external"; path: string; journalFormat: "sqlite" | "legacy" | "none"; readOnly: boolean };
export type SnapRouter = { domain: string; port: number | null; error: string | null; resolves?: boolean | null; resolveError?: string | null };
export type EngineSnapshot = {
  router?: SnapRouter;
  schema: number; generatedMs: number; runtimeDir: string; source: SnapSource; store: "missing" | "ok" | "error"; storeError: string | null; writerHeld: boolean;
  gate: { liveDeploy: boolean; liveReplacement: boolean; /** Executing queued Git events (Gate U): closed in every shipping build. */ liveTriggered?: boolean; /** Removing a registration: closed in every shipping build; open only for an isolated debug runtime. */ gitRemoval?: boolean; reason: string };
  repos: SnapRepo[]; apps: SnapApp[]; definitions: SnapDefinition[];
};
export type OpResult = { ok: boolean; exitCode: number; summary: string; detail: string; refusedByGate: boolean; /** What the person can do about a failure, in plain words; absent on success. */ hint?: string | null };
/** What the views show after an operation: the engine's answer, tagged with the operation that produced it. */
export type LastResult = OpResult & { op: string };
export type StepLog = { step: string; text: string; withheld: boolean; truncated: boolean; atMs: number };

/** What Docker shows for one container right now. `labels` is `null` when Docker's answer carried none to check. */
export type ObservedContainer = { id: string; state: string; labels: Record<string, string> | null; ports: number[]; mappings: PortMapping[] };
/** Container name -> what Docker shows, as observed now. `null` = Docker could not be asked. */
export type Observed = Record<string, ObservedContainer> | null;

/** A runtime attached read-only by path, as the hook holds it. */
export type ExternalInput = { path: string; snapshot: EngineSnapshot | null; error: string | null };

/** What the views may do with the real engine. They never see `invoke`. */
export type EngineApi = {
  /** The isolated test namespace this desktop drives (debug builds only), or null: the usual, production case. */
  testNamespace?: string | null;
  /** `confirmation` is what the person confirmed in a dialog for THIS operation; it is sent only with it and never remembered. */
  run: (op: Record<string, unknown>, confirmation?: LiveConfirmation) => Promise<OpResult>;
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
  /** Forgets only the remembered reference; the folder and its files are never touched. */
  detachRuntime: (path: string) => void | Promise<void>;
  /** Set when the remembered list itself could not be read; it is left as it is. */
  externalsError?: string | null;
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
    repos: (snap?.repos ?? []).map((r) => ({ name: r.name, url: r.url, app: r.app, project: r.project, branch: r.branch, containerPort: r.containerPort, dockerfile: r.dockerfile, healthPath: r.healthPath, credential: r.credential, registeredMs: r.registeredMs, seen: r.seen })),
    loadError, stale,
    externals: externals.map((x, i) => ({ id: `ext${i}`, path: x.path, format: x.snapshot?.source.journalFormat ?? "none", store: x.snapshot?.store ?? "missing", error: x.error ?? x.snapshot?.storeError ?? null })),
  };
}

const NOT: ConfigFact = { value: null, from: "none" };

/**
 * The friendly address of an application, or nothing when there is nothing true to show. The name belongs to the
 * application, so it is the same for every release; whether it answers depends on the recorded serving release
 * being observed running. An attached external runtime is never routed, so it never gets one.
 */
export function addressOf(a: SnapApp, router: SnapRouter | undefined, running: boolean, external: boolean): AppAddress | undefined {
  if (external || !a.hostname || !router) return undefined;
  if (a.route === "conflict") return { url: "", status: "conflict", note: "Another application was assigned the same address, so Harpoon routes neither of them. Nothing was changed." };
  if (a.route !== "serving") return undefined;
  if (router.port == null) return { url: "", status: "unavailable", note: router.error ?? "The application router is not running." };
  // Checked on this Mac at startup. Unknown (null/absent) is not a claim either way; a "no" means the URL would not open.
  if (router.resolves === false) return { url: "", status: "unresolvable", note: `${router.resolveError ?? "This Mac does not resolve *.harpoon.localhost"}, so the friendly address would not open. The direct address under "Recorded vs observed" still works.` };
  const url = `http://${a.hostname}:${router.port}`;
  return running
    ? { url, status: "running", note: "Answers while Harpoon is open and the release recorded as serving is running." }
    : { url, status: "stopped", note: "The release recorded as serving is not observed running, so this address will not respond until it is (it also needs Harpoon open)." };
}

/**
 * Configuration as RECORDED, field by field, each with where it came from: the repository registration, the
 * journal (a pinned release configuration, the ledger, the deployment record) or Docker (observed, and not in the
 * journal). A value nothing recorded stays `null` and is shown as "Not recorded": no default, placeholder or guess
 * is ever presented as history.
 */
function factsOf(a: SnapApp, repo: SnapRepo | null, servDep: SnapDeployment | undefined, row: ObservedContainer | undefined, leasePort: number | null): ConfigFacts {
  const latest = a.deployments[a.deployments.length - 1];
  const pin = servDep?.pin ?? [...a.deployments].reverse().find((d) => d.pin)?.pin ?? null;
  const src = [...a.deployments].reverse().find((d) => d.source)?.source ?? null;
  const reg = (v: string | null | undefined): ConfigFact => (v ? { value: v, from: "registration" } : NOT);
  const dep = servDep ?? latest;
  const healthPath = a.serving?.healthPath ?? pin?.healthPath ?? null;
  let containerPort: ConfigFact = NOT;
  if (pin) containerPort = { value: String(pin.containerPort), from: "journal", detail: "pinned in the release configuration" };
  else if (repo) containerPort = { value: String(repo.containerPort), from: "registration" };
  else if (row?.mappings.length) {
    const m = row.mappings.find((x) => x.hostPort === leasePort) ?? row.mappings[0];
    containerPort = { value: String(m.containerPort), from: "docker", detail: `Docker maps ${m.hostPort}→${m.containerPort}; the journal does not record it` };
  }
  let replacement: ConfigFact = NOT;
  if (dep?.replaces) replacement = { value: `Replaced release ${dep.replaces}${dep.interruption ? `, interruption ${dep.interruption.endedMs == null ? "not ended" : durationLabel(dep.interruption.endedMs - dep.interruption.startedMs)} (measured)` : ""}`, from: "journal" };
  else if (dep && dep.cutover === "not_required") replacement = { value: "Nothing was replaced (first deployment)", from: "journal", detail: "cutover not required" };
  const image = dep?.resources.find((r) => r.kind === "image");
  return {
    source: repo ? { value: `${displayUrl(repo.url)}`, from: "registration" } : a.disconnected ? { value: `Disconnected (was ${displayUrl(a.disconnected.url)})`, from: "journal", detail: `repository ${a.disconnected.name} was removed` } : src ? { value: `${displayUrl(src.url)} @ ${src.commit.slice(0, 7)}`, from: "journal", detail: "recorded with the deployment" } : NOT,
    branch: reg(repo?.branch), dockerfile: reg(repo?.dockerfile), context: reg(repo?.context), target: reg(repo?.target),
    containerPort,
    hostPort: a.serving?.hostPort != null ? { value: String(a.serving.hostPort), from: "journal", detail: "pinned in the release configuration" } : leasePort ? { value: String(leasePort), from: "journal", detail: "recorded port lease" } : NOT,
    healthCheck: healthPath ? { value: `HTTP GET ${healthPath}`, from: "journal", detail: "pinned in the release configuration" } : repo?.healthPath ? { value: `HTTP GET ${repo.healthPath}`, from: "registration" } : NOT,
    replacement,
    image: image ? { value: `${image.name}${image.id ? ` (${shortId(image.id)})` : ""}`, from: "journal", detail: "recorded in the ledger" } : pin?.imageTag ? { value: pin.imageTag, from: "journal" } : NOT,
  };
}

/** The applications of one snapshot. `ext` marks a runtime attached read-only: ids are prefixed so they can never collide with, or be mistaken for, Harpoon's own. */
function appsFrom(snap: EngineSnapshot, observed: Observed, hostListeners: HostListeners, ext?: { id: string; path: string }): { apps: Record<string, AppModel>; projects: Project[] } {
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
    const observation = correlate(a, observed, hostListeners) ?? undefined;
    // current = recorded as serving AND observed running as that deployment's container (labels checked when Docker gave them)
    const running = !!sv && (observation?.kind === "match" || observation?.kind === "unverified");
    const servDep = sv ? a.deployments.find((d) => d.id === sv.deployment) : undefined;
    const lease = servDep?.resources.find((r) => r.kind === "port" && r.state === "created");
    const leasePort = lease ? Number(lease.name) : null;
    const port = sv?.hostPort ?? leasePort;
    const healthPath = sv?.healthPath ?? repo?.healthPath ?? null;
    const pin = [...a.deployments].reverse().find((d) => d.pin)?.pin ?? null;
    const row = sv && observed ? observed[sv.container] : undefined;
    const projectId = ext ? ext.id : repo?.project ?? repo?.name ?? "ungrouped";
    apps[idOf(a.id)] = {
      id: idOf(a.id), projectId, name: repo?.name ?? a.disconnected?.name ?? a.id,
      ...(a.disconnected && !repo ? { disconnected: { name: a.disconnected.name, url: a.disconnected.url, removedMs: a.disconnected.removedMs } } : {}),
      ...(ext ? { external: { path: ext.path, format: snap.source.journalFormat } } : {}),
      observation, facts: factsOf(a, repo, servDep, row, leasePort), urlSource: sv?.hostPort != null ? "pin" : leasePort ? "lease" : undefined,
      source: repo ? { kind: "git", label: displayUrl(repo.url), detail: repo.branch } : a.disconnected ? { kind: "folder", label: `${a.disconnected.name} · source disconnected`, detail: `last source ${displayUrl(a.disconnected.url)}` } : { kind: "folder", label: a.id, detail: ext ? "recorded in an external runtime" : "no repository registered" },
      address: addressOf(a, snap.router, running, !!ext),
      url: running && port ? `http://127.0.0.1:${port}` : "", containerName: sv?.container ?? "",
      currentReleaseId: running ? sv!.release : null, lastKnownReleaseId: sv?.release ?? null,
      deployments, releases, automation: automationOf(a, now, (id) => byId.get(id)),
      variables: variablesOf(snap, a, now),
      // Engine mode never invents a setting: what the views show comes from `facts`; these stay empty when nothing was recorded.
      settings: {
        dockerfile: repo?.dockerfile ?? "", context: repo?.context ?? "", buildArgs: "", port: pin?.containerPort ?? repo?.containerPort ?? null,
        healthCheck: healthPath ? `HTTP GET ${healthPath}` : "", keepReleases: 0, productionBranch: repo?.branch ?? "", pollSeconds: 0,
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
export function toDeployState(snap: EngineSnapshot | null, o: { observed: Observed; keychain: KeychainState; loadError?: string | null; stale?: boolean; externals?: ExternalInput[]; hostListeners?: HostListeners }): DeployState {
  const externals = o.externals ?? [];
  const engine = engineMeta(snap, o.loadError ?? null, o.stale ?? false, externals);
  const base = { keychain: o.keychain, nextOutcome: "success" as const, job: null, nextDeploymentNumber: 1, engine };
  if (!snap) return { ...base, scenario: o.loadError ? "empty" : "loading", projects: [], apps: {} };
  const own = snap.store === "error" ? { apps: {}, projects: [] as Project[] } : appsFrom(snap, o.observed, o.hostListeners ?? null);
  const apps: Record<string, AppModel> = { ...own.apps };
  const projects: Project[] = [...own.projects];
  externals.forEach((x, i) => {
    if (!x.snapshot || x.snapshot.store === "error") return;
    const e = appsFrom(x.snapshot, o.observed, o.hostListeners ?? null, { id: `ext${i}`, path: x.path });
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
    const mappings = parseDockerPorts(typeof r.Ports === "string" ? r.Ports : "");
    const row: ObservedContainer = { id: String(r.ID ?? r.Id ?? ""), state: String(r.State ?? "").toLowerCase(), labels, ports: [...new Set(mappings.map((m) => m.hostPort))], mappings };
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
export function correlate(app: SnapApp, observed: Observed, hostListeners: HostListeners = null): Observation | null {
  const sv = app.serving;
  if (!sv) return null;
  const dep = app.deployments.find((d) => d.id === sv.deployment);
  const rec = dep?.resources.find((r) => r.kind === "container" && r.name === sv.container);
  const lease = dep?.resources.find((r) => r.kind === "port" && r.state === "created");
  const leasePort = lease ? Number(lease.name) : null;
  const recorded = `release ${sv.release} serving in container ${sv.container}${rec?.id ? ` (${shortId(rec.id)})` : ""}${leasePort ? `, port lease ${leasePort}` : ""}`;
  if (observed === null) return { kind: "unknown", recorded, observed: "Docker could not be asked", notes: ["The record cannot be compared with Docker right now."], checks: [] };
  const row = observed[sv.container];
  if (!row) return { kind: "absent", recorded, observed: "no container with that name", notes: ["Recorded as serving, but Docker has no such container."], checks: [] };

  const checks: ObservationCheck[] = [];
  const add = (id: ObservationCheck["id"], label: string, result: ObservationCheck["result"], detail: string) => checks.push({ id, label, result, detail });
  add("state", "Running", row.state === "running" ? "ok" : "fail", `Docker reports the container as ${row.state || "not running"}.`);
  if (!rec?.id) add("id", "Container id", "unavailable", "The journal recorded no container id to compare.");
  else if (!row.id) add("id", "Container id", "unavailable", "Docker's answer carried no id.");
  else if (row.id === rec.id || row.id.startsWith(rec.id) || rec.id.startsWith(row.id)) add("id", "Container id", "ok", `Both are ${shortId(rec.id)}.`);
  else add("id", "Container id", "fail", `The container id differs: recorded ${shortId(rec.id)}, observed ${shortId(row.id)}.`);
  const label = (id: "managed" | "app" | "deployment", key: string, want: string, text: string) => {
    if (!row.labels) return add(id, text, "unavailable", "Docker's answer carried no labels, so this was not checked.");
    const got = row.labels[key];
    if (got === want) add(id, text, "ok", `${key} is ${JSON.stringify(got)}.`);
    else add(id, text, "fail", id === "managed" ? `The container is not labelled ${key}=true.` : `Its ${key} label is ${JSON.stringify(got ?? "")}, not ${want}.`);
  };
  label("managed", LABEL_MANAGED, "true", "Managed by Harpoon");
  label("app", LABEL_APP, app.id, "Belongs to this application");
  label("deployment", LABEL_DEPLOYMENT, sv.deployment, "Created by this deployment");
  if (leasePort && row.mappings.length) {
    if (row.ports.includes(leasePort)) add("port", "Port lease", "ok", `Docker publishes the leased port ${leasePort}.`);
    else add("port", "Port lease", "fail", `The recorded port lease ${leasePort} is not among the published ports (${row.ports.join(", ")}).`);
  } else add("port", "Port lease", "unavailable", leasePort ? "Docker reports no published ports to compare." : "The journal recorded no port lease.");

  const identityFailed = checks.some((c) => c.id !== "state" && c.result === "fail");
  const ownership = checks.filter((c) => c.id === "managed" || c.id === "app" || c.id === "deployment");
  const kind: Observation["kind"] = identityFailed ? "mismatch" : row.state !== "running" ? "stopped" : ownership.every((c) => c.result === "ok") ? "match" : "unverified";
  const notes = checks.filter((c) => c.result === "fail" && (c.id !== "state" || !identityFailed)).map((c) => c.detail);
  if (kind === "unverified") notes.push("Docker's answer carried no labels, so ownership could not be checked.");
  const observedText = `container ${shortId(row.id)} ${row.state}${row.mappings.length ? `, Docker publishes ${row.mappings.map((m) => `${m.hostIp}:${m.hostPort}→${m.containerPort}`).join(", ")}` : ", no published ports"}`;
  return { kind, recorded, observed: observedText, notes, checks, port: exposureOf(row.mappings, hostListeners) };
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

/**
 * What the person confirmed in a dialog for one operation that changes real Docker state. The desktop backend turns
 * `liveChanges` into the engine's explicit opt-in and `cutover` into its cutover switch; nothing else ever does, and
 * nothing is remembered between operations.
 */
export type LiveConfirmation = { liveChanges: boolean; cutover: boolean };

/** The acknowledgement a replacement needs before Harpoon may stop the running release to switch. */
export const CUTOVER_ACKNOWLEDGEMENT = "I understand that the running release is stopped to switch, so the application is unavailable until the new release passes its health check.";

/**
 * The confirmation a Deploy dialog may send, or `null` when it may not send anything yet.
 * A first deployment has nothing to switch, so confirming it confirms only the Docker changes. A replacement needs the
 * separate cutover acknowledgement; `prepareOnly` builds and checks the candidate and never switches, so it does not.
 */
export function deployConfirmation(o: { replacing: boolean; cutoverAcknowledged: boolean; prepareOnly?: boolean }): LiveConfirmation | null {
  if (!o.replacing) return { liveChanges: true, cutover: false };
  if (o.prepareOnly) return { liveChanges: true, cutover: false };
  return o.cutoverAcknowledged ? { liveChanges: true, cutover: true } : null;
}

/** The words shown for a failed operation: the plain-language hint when the backend gave one, never a bare exit code. */
export function failureText(r: OpResult): string {
  const h = (r.hint ?? "").trim();
  if (h) return h;
  const s = (r.summary ?? "").trim();
  return s || "The engine reported an error. Nothing was retried.";
}

/**
 * What the Add repository dialog tells the person after the engine answered. A refusal keeps the dialog open
 * with the engine's own reason; a success lists what the engine confirmed (registered, and whether the source
 * was verified or is still unverified), and says plainly when the access token could not be saved.
 */
export type RegistrationOutcome =
  | { ok: true; lines: string[]; verified: boolean; tokenProblem: string | null }
  | { ok: false; message: string };

export function registrationOutcome(r: OpResult, token?: OpResult | null): RegistrationOutcome {
  if (!r.ok) return { ok: false, message: failureText(r) };
  const lines = (r.detail ?? "").split("\n").map((l) => l.trim()).filter((l) => /^(registered|updated|removed|verified|not verified|first removal|backup:|sha256:|restoring it)/.test(l));
  return {
    ok: true,
    lines: lines.length ? lines : [r.summary || "The repository was registered."],
    verified: lines.some((l) => l.startsWith("verified")),
    tokenProblem: token && !token.ok ? failureText(token) : null,
  };
}

// ------------------------------------------------------------------ editing a registration

export type RegistrationForm = { kind: "git" | "folder"; location: string; branch: string; dockerfile: string; port: string; health: string };
export type RegistrationEdit = { op: Record<string, unknown> | null; changed: string[]; problem: string | null };

/** The form as the registration stands: a local folder is shown as its path, and an unset value stays empty (never a made-up default). */
export function registrationFormOf(r: EngineRepo): RegistrationForm {
  return { kind: r.url.startsWith("file://") ? "folder" : "git", location: displayUrl(r.url), branch: r.branch, dockerfile: r.dockerfile ?? "", port: String(r.containerPort), health: r.healthPath ?? "" };
}

/**
 * What an edit asks the engine for: only the fields that differ from the registration the person opened, plus the
 * version of that registration so the engine can refuse if it changed meanwhile. `confirm` is the person's
 * acknowledgement for an application that already has deployments. The name, application and credential are not
 * part of an edit. Nothing here deploys anything.
 */
export function registrationEdit(r: EngineRepo, f: RegistrationForm, confirm: boolean): RegistrationEdit {
  const changed: string[] = [];
  const op: Record<string, unknown> = { op: "update-repo", name: r.name, expectRegisteredMs: r.registeredMs, confirmChange: confirm };
  const fail = (problem: string): RegistrationEdit => ({ op: null, changed: [], problem });
  const url = f.kind === "folder" ? fileUrlFor(f.location.trim()) : f.location.trim();
  if (!url) return fail(f.kind === "folder" ? "Enter an absolute folder path (starting with /, without . or .. parts)." : "Enter the repository URL.");
  if (url !== r.url) { op.url = url; changed.push("source"); }
  const branch = f.branch.trim();
  if (!branch) return fail("A branch is required.");
  if (branch !== r.branch) { op.branch = branch; changed.push("branch"); }
  for (const [label, key, now, was] of [["Dockerfile path", "dockerfile", f.dockerfile.trim(), r.dockerfile ?? ""], ["Health check path", "healthPath", f.health.trim(), r.healthPath ?? ""]] as const) {
    if (!now && was) return fail(`${label} cannot be cleared; enter the path to use.`);
    if (now && now !== was) { op[key] = now; changed.push(label.toLowerCase()); }
  }
  const port = Number(f.port);
  if (!Number.isInteger(port) || port < 1 || port > 65535) return fail("The container port must be a number from 1 to 65535.");
  if (port !== r.containerPort) { op.containerPort = port; changed.push("container port"); }
  return changed.length ? { op, changed, problem: null } : { op: null, changed: [], problem: null };
}

// ------------------------------------------------------------------ an application without a registered repository

/** Why Deploy, Redeploy and Retry are unavailable for an application that has no registered repository. */
export const NO_SOURCE_REASON = "No Git repository is registered for this application, so Harpoon cannot build a new release from a source. Rolling back to a retained release still works. Add a repository with the same name to connect a source again.";

/**
 * Whether this application, one of Harpoon's own, has no registered repository: nothing to build a release from, so
 * the Git-based actions (Deploy, Redeploy, Retry, Check for new commits) would only be refused by the engine. Rollback
 * and everything that reads history do not need a source. An attached read-only runtime is handled elsewhere.
 */
export function hasNoSource(engine: EngineMeta | undefined, app: AppModel): boolean {
  return !!engine && !app.external && !engine.repos.some((r) => r.app === app.id);
}

// ------------------------------------------------------------------ removing a registration

export const REMOVAL_CLOSED_REASON = "Removing a registration is not enabled in this build. It permanently upgrades the deployment journal so that older Harpoon versions can no longer open it, so it is available only in an isolated test runtime that was authorized for it.";

export type RemovalPlan = {
  /** The request to send, once everything holds. */
  op: Record<string, unknown> | null;
  /** Why removal cannot go ahead at all right now (a held event, a closed gate); null when only the confirmation is missing. */
  blocked: string | null;
};

/** Attributes for an input whose text must reach the engine exactly as typed (typed-name confirmations): macOS would otherwise autocorrect or capitalise it, even on Return. */
export const EXACT_TEXT_INPUT = { autoComplete: "off", autoCorrect: "off", autoCapitalize: "off", spellCheck: false } as const;

/**
 * What a removal needs before it is sent: the build must allow it, no Git event of the application may be waiting (the
 * engine refuses too; this says so before the person types anything), and the person must type the repository's name.
 * The request carries the registration version the person was shown, so the engine refuses if it changed meanwhile.
 */
export function registrationRemoval(engine: EngineMeta | undefined, app: AppModel, repo: EngineRepo, typedName: string): RemovalPlan {
  if (!engine?.gate.gitRemoval) return { op: null, blocked: REMOVAL_CLOSED_REASON };
  const waiting = app.automation.held.length + (app.automation.selected?.length ?? 0);
  if (waiting > 0) {
    return { op: null, blocked: `${waiting} held or selected Git event${waiting === 1 ? "" : "s"} from this repository ${waiting === 1 ? "is" : "are"} still waiting. Review ${waiting === 1 ? "it" : "them"} first: removing the registration never dismisses or replays events.` };
  }
  if (typedName.trim() !== repo.name) return { op: null, blocked: null };
  return { op: { op: "remove-repo", name: repo.name, expectRegisteredMs: repo.registeredMs, confirmRemoval: true }, blocked: null };
}
