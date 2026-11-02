// Harpoon Deployments — frontend contract (MOCK-BACKED).
//
// These types describe what the Deployments views render. They are the interface boundary
// for a future deployment engine: nothing in this file, or in the mock service behind it,
// talks to Docker, the Keychain, Git, or any engine. Every record here is SIMULATED.
// Secret values are never part of the contract: a secret is a name, a type and a version.
// Strip-types friendly: no enums, no parameter properties.

export type Env = "development" | "preview" | "production";
export type RuntimeState = "connected" | "stopped" | "docker-not-ready";
export type KeychainState = "unlocked" | "locked";

export type ScenarioId =
  | "healthy"
  | "building"
  | "build-failed"
  | "cutover-restored"
  | "restore-failed"
  | "recovering"
  | "rolled-back-paused"
  | "empty"
  | "loading";

/** What the *next* simulated deployment will do. Simulation control only. */
export type NextOutcome = "success" | "build-failure" | "cutover-restored" | "restore-failed";

export type DeploymentStatus = "running" | "succeeded" | "failed" | "cancelled" | "interrupted" | "superseded";
export type Trigger = "manual" | "poll" | "rollback" | "promote" | "retry" | "redeploy";
export type StepName = "source" | "build" | "preliminary_check" | "cutover" | "final_check" | "recovery";
export type StepStatus = "pending" | "running" | "succeeded" | "failed" | "warning" | "skipped";
export type RecoveryOutcome = "in_progress" | "restored" | "restored_unverified" | "restore_failed" | "not_attempted";
export type RestoreReason = "secret_unavailable" | "image_missing" | "timeout" | "start_failed" | "docker_unreachable";

export type LogKind = "" | "step" | "ok" | "err" | "warn" | "sys";
/** `text` may contain `[REDACTED:KEY]` tokens; the contract never carries secret values. */
export type LogLine = { kind: LogKind; text: string };

export type Revision = { sha: string; message: string; branch: string; author: string };
export type StepRecord = { name: StepName; status: StepStatus; durationLabel: string; note: string };

export type Deployment = {
  id: string;
  env: Env;
  status: DeploymentStatus;
  trigger: Trigger;
  revision: Revision;
  /** The release this deployment produced (set when the build succeeded). */
  releaseId: string | null;
  ageMin: number;
  durationLabel: string;
  failedAt?: StepName;
  failure?: string;
  /** True when the failure happened before anything was stopped. */
  nothingStopped?: boolean;
  interruption?: { label: string; ongoing: boolean };
  recovery?: { outcome: RecoveryOutcome; detail: string; durationLabel?: string; reason?: RestoreReason };
  steps: StepRecord[];
  buildLog: LogLine[];
  note?: string;
};

export type Release = {
  id: string;
  deploymentId: string;
  image: string;
  revision: Revision;
  ageMin: number;
  /** Pinned secret versions only — never values. */
  secrets: { name: string; version: number }[];
};

export type HeldEvent = { id: string; revision: Revision; ageMin: number; /** Engine mode: the full revision, which a resume decision needs. */ revisionFull?: string; /** Engine mode: `held`, or `selected` (chosen by a resume; nothing has run yet). */ status?: string };
export type PauseReason =
  | { kind: "failed_cutover"; deploymentId: string; outcome: RecoveryOutcome }
  | { kind: "interrupted_cutover"; deploymentId: string }
  | { kind: "rollback"; releaseId: string };
export type Automation = { state: "active" | "paused"; reason?: PauseReason; sinceMin?: number; held: HeldEvent[] };

export type VariableType = "config" | "secret";
export type VariableUse = "runtime" | "build" | "build+runtime" | "build secret";
export type VariableScope = "global" | "project" | "environment" | "application";
export type Variable = {
  name: string;
  type: VariableType;
  used: VariableUse;
  scope: VariableScope;
  /** Config values only. Secrets never carry a value. */
  value?: string;
  /** Secrets only. */
  version?: number;
  changedAgoMin?: number;
};

export type PreviewInstance = { branch: string; status: "running" | "building"; sha: string; url: string | null; expiresInDays: number };
export type AppSource = { kind: "git" | "image" | "folder"; label: string; detail: string };
export type AppSettings = {
  dockerfile: string;
  context: string;
  buildArgs: string;
  port: number | null;
  healthCheck: string;
  keepReleases: number;
  productionBranch: string;
  pollSeconds: number;
};

export type AppModel = {
  id: string;
  projectId: string;
  name: string;
  source: AppSource;
  url: string;
  containerName: string;
  /** The release serving right now according to the (simulated) runtime, or null if none. */
  currentReleaseId: string | null;
  /** The release that was current most recently; used for "last known" while disconnected. */
  lastKnownReleaseId: string | null;
  deployments: Deployment[];
  releases: Release[];
  automation: Automation;
  variables: Variable[];
  settings: AppSettings;
  previews: PreviewInstance[];
  resources: { cpuPct: number; memMiB: number; memLimitMiB: number; pids: number } | null;
};

export type Project = { id: string; name: string; sourceLabel: string; appIds: string[] };

/** A simulated deployment in flight; advanced one phase at a time. */
export type SimJob = { appId: string; deploymentId: string; outcome: NextOutcome; hadPrevious: boolean; phase: number };

/** Present only when the data comes from the real engine. Absent in the explicit simulation/preview mode. */
export type EngineMeta = {
  generatedMs: number;
  store: "missing" | "ok" | "error";
  storeError: string | null;
  writerHeld: boolean;
  gate: { liveDeploy: boolean; liveReplacement: boolean; reason: string };
  repos: EngineRepo[];
  /** The last refresh failed; what is shown is the last good snapshot (or nothing). */
  loadError: string | null;
  stale: boolean;
};
export type EngineRepo = { name: string; url: string; app: string; project: string | null; branch: string; containerPort: number; dockerfile: string | null; healthPath: string | null; seen: { refName: string; commit: string; atMs: number }[] };

export type DeployState = {
  /** Engine mode only ever uses `loading` (nothing loaded yet) and `empty`/`healthy` (a loaded snapshot with/without apps). */
  scenario: ScenarioId;
  engine?: EngineMeta;
  projects: Project[];
  apps: Record<string, AppModel>;
  keychain: KeychainState;
  nextOutcome: NextOutcome;
  job: SimJob | null;
  nextDeploymentNumber: number;
};

/** Derived, never stored: one source of truth for what an application "is" right now. */
export type AppStatusKind = "running" | "building" | "recovering" | "down" | "unknown" | "idle";
export type AppStatus = {
  kind: AppStatusKind;
  label: string;
  tone: "ok" | "info" | "warn" | "err" | "neutral";
  /** Release shown as running, or the last known one when the runtime is not connected. */
  releaseId: string | null;
  /** True only when the figures come from a connected runtime. */
  live: boolean;
};

/** Everything a view may ask the data layer to do. The mock implements it; an engine will. */
export type DeployAction =
  | { type: "set-scenario"; scenario: ScenarioId }
  | { type: "set-keychain"; keychain: KeychainState }
  | { type: "set-next-outcome"; outcome: NextOutcome }
  | { type: "start-deploy"; appId: string; trigger: Trigger; revision?: Revision; env?: Env }
  | { type: "advance" }
  | { type: "rollback"; appId: string; releaseId: string }
  | { type: "recover"; appId: string }
  | { type: "resume"; appId: string; mode: "deploy" | "skip"; heldEventId?: string }
  | { type: "create-project"; name: string; source: AppSource["kind"] }
  | { type: "save-variable"; appId: string; name: string; varType: VariableType; used: VariableUse; scope: VariableScope; value?: string };
