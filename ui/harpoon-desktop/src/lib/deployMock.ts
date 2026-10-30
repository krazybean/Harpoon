// Seed data for the SIMULATED Deployments experience. Every name, revision, image and number
// below is invented for demonstration. Nothing here is read from Docker, Git, or Harpoon.
import type { AppModel, Deployment, DeployState, LogLine, Release, Revision, StepRecord, StepName, Variable } from "./deployTypes.ts";

export const REV = {
  next: { sha: "b7d20e1", message: "feat: tax rounding per region", branch: "main", author: "juan" },
  d42: { sha: "a41c9e2", message: "fix: cart rounding", branch: "main", author: "juan" },
  d41: { sha: "9ce0f33", message: "chore: bump deps", branch: "main", author: "juan" },
  d40: { sha: "5d1a8b0", message: "feat: product search", branch: "main", author: "juan" },
  d39: { sha: "11fe902", message: "refactor: pricing module", branch: "main", author: "juan" },
  d38: { sha: "c0bb77d", message: "fix: session cookie", branch: "main", author: "juan" },
  held1: { sha: "c13f5aa", message: "feat: add VAT lines", branch: "main", author: "juan" },
  held2: { sha: "e90a117", message: "wip: invoice totals", branch: "main", author: "juan" },
} satisfies Record<string, Revision>;

export const STEP_ORDER: StepName[] = ["source", "build", "preliminary_check", "cutover", "final_check", "recovery"];

export function pendingSteps(): StepRecord[] {
  return STEP_ORDER.map((name) => ({ name, status: "pending", durationLabel: "—", note: "" }));
}

export const LOG_OK: LogLine[] = [
  { kind: "step", text: "[1/6] FROM alpine:3.22" },
  { kind: "", text: "[2/6] COPY package*.json ./" },
  { kind: "step", text: "[3/6] RUN npm ci --omit=dev" },
  { kind: "", text: "added 214 packages in 9s" },
  { kind: "step", text: "[4/6] RUN npm run build" },
  { kind: "", text: "> api@1.4.0 build" },
  { kind: "", text: "> tsc -p ." },
  { kind: "", text: "using STRIPE_KEY from the Keychain: [REDACTED:STRIPE_KEY]" },
  { kind: "ok", text: "[5/6] exporting layers" },
  { kind: "ok", text: "[6/6] naming to harpoon/storefront-api:{id}" },
];

export const LOG_FAIL_TAIL: LogLine[] = [
  { kind: "err", text: "src/tax.ts(41,7): error TS2322: Type 'string' is not assignable to type 'number'." },
  { kind: "err", text: "src/tax.ts(58,15): error TS2345: Argument of type 'undefined' is not assignable to 'Region'." },
  { kind: "err", text: 'ERROR: process "/bin/sh -c npm run build" did not complete successfully: exit code: 2' },
];

export const RUNTIME_LOG: LogLine[] = [
  { kind: "", text: "listening on :3000" },
  { kind: "", text: "GET /healthz 200 2ms" },
  { kind: "", text: "GET /api/cart 200 14ms" },
  { kind: "warn", text: "slow query: orders.byRegion 412ms" },
  { kind: "", text: "GET /api/products 200 9ms" },
  { kind: "", text: "connected to postgres at db:5432 · password [REDACTED:DATABASE_URL]" },
  { kind: "", text: "GET /healthz 200 1ms" },
];

function okSteps(note: string): StepRecord[] {
  return [
    { name: "source", status: "succeeded", durationLabel: "3s", note: "revision resolved" },
    { name: "build", status: "succeeded", durationLabel: "31s", note: "image sha256:7be1…c09 · 214 MiB" },
    { name: "preliminary_check", status: "succeeded", durationLabel: "6s", note: "HTTP /healthz 200 on a temporary port; the running release was untouched" },
    { name: "cutover", status: "succeeded", durationLabel: "2s", note },
    { name: "final_check", status: "succeeded", durationLabel: "3s", note: "HTTP /healthz 200" },
    { name: "recovery", status: "skipped", durationLabel: "—", note: "not needed" },
  ];
}

function okDeployment(n: number, rev: Revision, ageMin: number, trigger: Deployment["trigger"], first = false): Deployment {
  return {
    id: `d-${n}`, env: "production", status: "succeeded", trigger, revision: rev, releaseId: `r-${n}`, ageMin, durationLabel: "1m 04s",
    interruption: first ? undefined : { label: "4 s", ongoing: false },
    steps: okSteps(first ? "started the first release · nothing to stop" : `stopped r-${n - 1}, started r-${n}`),
    buildLog: LOG_OK.map((l) => ({ ...l, text: l.text.replace("{id}", `d-${n}`) })),
  };
}

function release(n: number, rev: Revision, ageMin: number, secrets: Release["secrets"]): Release {
  return { id: `r-${n}`, deploymentId: `d-${n}`, image: `harpoon/storefront-api:d-${n}`, revision: rev, ageMin, secrets };
}

const VARIABLES: Variable[] = [
  { name: "NODE_ENV", type: "config", used: "runtime", scope: "project", value: "production" },
  { name: "PORT", type: "config", used: "runtime", scope: "application", value: "3000" },
  { name: "REGION_DEFAULT", type: "config", used: "build+runtime", scope: "environment", value: "us-east" },
  { name: "DATABASE_URL", type: "secret", used: "runtime", scope: "environment", version: 2, changedAgoMin: 4320 },
  { name: "STRIPE_KEY", type: "secret", used: "runtime", scope: "environment", version: 3, changedAgoMin: 17280 },
  { name: "NPM_TOKEN", type: "secret", used: "build secret", scope: "global", version: 1, changedAgoMin: 57600 },
];

export function seedApi(): AppModel {
  const secrets = (db: number, stripe: number) => [{ name: "STRIPE_KEY", version: stripe }, { name: "DATABASE_URL", version: db }, { name: "NPM_TOKEN", version: 1 }];
  const failed39: Deployment = {
    id: "d-39", env: "production", status: "failed", trigger: "poll", revision: REV.d39, releaseId: null, ageMin: 2880, durationLabel: "22s",
    failedAt: "build", failure: "Build failed at step 4/6 (npm run build). Nothing was stopped.", nothingStopped: true,
    steps: [
      { name: "source", status: "succeeded", durationLabel: "3s", note: "revision resolved" },
      { name: "build", status: "failed", durationLabel: "18s", note: "npm run build exited with code 2" },
      ...(["preliminary_check", "cutover", "final_check", "recovery"] as StepName[]).map((name): StepRecord => ({ name, status: "skipped", durationLabel: "—", note: "not reached" })),
    ],
    buildLog: [...LOG_OK.slice(0, 7), ...LOG_FAIL_TAIL],
  };
  return {
    id: "api", projectId: "storefront", name: "api",
    source: { kind: "git", label: "acme/storefront", detail: "api/Dockerfile · main" },
    url: "http://localhost:38101", containerName: "storefront-api-r42",
    currentReleaseId: "r-42", lastKnownReleaseId: "r-42",
    deployments: [okDeployment(42, REV.d42, 12, "poll"), okDeployment(41, REV.d41, 1440, "manual"), okDeployment(40, REV.d40, 2700, "poll"), failed39, okDeployment(38, REV.d38, 5760, "manual")],
    releases: [release(42, REV.d42, 12, secrets(2, 3)), release(41, REV.d41, 1440, secrets(2, 2)), release(40, REV.d40, 2700, secrets(1, 2)), release(38, REV.d38, 5760, secrets(1, 1))],
    automation: { state: "active", held: [] },
    variables: VARIABLES.map((v) => ({ ...v })),
    settings: { dockerfile: "api/Dockerfile", context: "api/", buildArgs: "REGION_DEFAULT", port: 3000, healthCheck: "HTTP GET /healthz · 3 tries over 20 s", keepReleases: 2, productionBranch: "main", pollSeconds: 60 },
    previews: [
      { branch: "feat/vat-lines", status: "running", sha: "c13f5aa", url: "http://localhost:38112", expiresInDays: 6 },
      { branch: "wip/invoice", status: "building", sha: "e90a117", url: null, expiresInDays: 7 },
    ],
    resources: { cpuPct: 7.4, memMiB: 142, memLimitMiB: 512, pids: 21 },
  };
}

function seedSimple(id: string, projectId: string, name: string, source: AppModel["source"], image: string, port: number, rev: Revision): AppModel {
  const dep: Deployment = { ...okDeployment(17, rev, 4300, "manual", true), id: "d-17", releaseId: "r-17", steps: okSteps("started the first release · nothing to stop") };
  return {
    id, projectId, name, source, url: `http://localhost:${port}`, containerName: `${projectId}-${name}-r17`,
    currentReleaseId: "r-17", lastKnownReleaseId: "r-17", deployments: [dep],
    releases: [{ id: "r-17", deploymentId: "d-17", image, revision: rev, ageMin: 4300, secrets: [] }],
    automation: { state: "active", held: [] }, variables: [{ name: "NODE_ENV", type: "config", used: "runtime", scope: "project", value: "production" }],
    settings: { dockerfile: source.kind === "image" ? "(none — existing image)" : `${name}/Dockerfile`, context: `${name}/`, buildArgs: "", port: 80, healthCheck: "HTTP GET /", keepReleases: 2, productionBranch: "main", pollSeconds: 60 },
    previews: [], resources: { cpuPct: 1.2, memMiB: 38, memLimitMiB: 256, pids: 6 },
  };
}

export function seedState(): DeployState {
  const web = seedSimple("web", "storefront", "web", { kind: "git", label: "acme/storefront", detail: "web/Dockerfile · main" }, "harpoon/storefront-web:d-17", 38102, { sha: "7a2c1d9", message: "feat: hero banner", branch: "main", author: "juan" });
  const site = seedSimple("site", "docs-site", "site", { kind: "image", label: "nginx:1.27-alpine", detail: "existing image" }, "nginx:1.27-alpine", 38103, { sha: "—", message: "image pinned", branch: "—", author: "juan" });
  return {
    scenario: "healthy",
    projects: [
      { id: "storefront", name: "storefront", sourceLabel: "git deploy key", appIds: ["api", "web"] },
      { id: "docs-site", name: "docs-site", sourceLabel: "image", appIds: ["site"] },
    ],
    apps: { api: seedApi(), web, site },
    keychain: "unlocked", nextOutcome: "success", job: null, nextDeploymentNumber: 43,
  };
}
