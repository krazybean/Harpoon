import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { appStatus } from "./deployStore.ts";
import { deploymentOf, liveBlock, logLines, observedFrom, toDeployState } from "./deployEngine.ts";

const NOW = 10_000_000;
const GATE = { liveDeploy: false, liveReplacement: false, reason: "closed in this build" };
const step = (name, status, extra = {}) => ({ name, status, startedMs: NOW - 60000, finishedMs: NOW - 50000, note: "", log: false, ...extra });
const dep = (id, extra = {}) => ({
  id, status: "succeeded", trigger: "cli", createdMs: NOW - 120000, startedMs: NOW - 119000, finishedMs: NOW - 100000, steps: [step("source", "succeeded"), step("build", "succeeded")],
  cutover: "confirmed", cutoverAtMs: NOW - 101000, failure: null, recovery: null, interruption: null, release: `rel_${id}`, replaces: null, rollbackTo: null, mayBeDown: false, unfinished: false,
  source: { repo: "shop", url: "https://example.com/shop.git", selector: "branch", requested: "main", commit: "0123456789abcdef", trigger: "manual", delivery: null },
  pin: { imageTag: `harpoon/${id}:1`, imageId: "sha256:x", hostPort: 41000, containerPort: 3000, healthPath: "/healthz", envNames: ["MODE"], secrets: [{ name: "DB", id: "app:a/DB", version: 2 }], snapshot: id, build: null },
  config: null, configDiff: [], ...extra,
});
const app = (deployments, extra = {}) => ({ id: "app_1", repo: "shop", deployments, serving: null, pauses: [], events: [], ...extra });
const snap = (apps, extra = {}) => ({
  schema: 1, generatedMs: NOW, runtimeDir: "/x", store: "ok", storeError: null, writerHeld: false, gate: GATE, apps, definitions: [],
  repos: [{ name: "shop", url: "https://example.com/shop.git", app: "app_1", project: null, environment: null, branch: "main", filters: [], credential: "none", webhook: null, containerPort: 3000, dockerfile: "Dockerfile", context: null, target: null, healthPath: "/healthz", seen: [] }], ...extra,
});
const st = (s, o = {}) => toDeployState(s, { observed: {}, keychain: "unlocked", ...o });

test("nothing loaded is 'loading', and a failed first load is an error with no invented apps", () => {
  assert.equal(st(null).scenario, "loading");
  const failed = st(null, { loadError: "backend unavailable" });
  assert.deepEqual([Object.keys(failed.apps).length, failed.projects.length, failed.engine.loadError], [0, 0, "backend unavailable"]);
});

test("a missing store is empty, a damaged store shows nothing and carries the reason", () => {
  assert.equal(st(snap([], { store: "missing" })).scenario, "empty");
  const bad = st(snap([{ ...app([dep("dep_1")]) }], { store: "error", storeError: "database disk image is malformed" }));
  assert.deepEqual([Object.keys(bad.apps).length, bad.engine.storeError], [0, "database disk image is malformed"]);
});

test("no seeded or fabricated content: every app, id and release comes from the snapshot", () => {
  const s = st(snap([app([dep("dep_1")])]));
  assert.deepEqual(Object.keys(s.apps), ["app_1"]);
  assert.deepEqual(s.apps.app_1.deployments.map((d) => d.id), ["dep_1"]);
  assert.deepEqual(s.apps.app_1.previews, []);
  assert.equal(s.apps.app_1.resources, null);
  assert.equal(s.job, null);
});

test("recorded serving release is only 'running' when Docker was observed running it", () => {
  const serving = { release: "rel_dep_1", deployment: "dep_1", container: "hp-dep_1", via: "original", sinceMs: NOW - 90000, hostPort: 41000, healthPath: "/healthz" };
  const a = app([dep("dep_1")], { serving });
  const up = st(snap([a]), { observed: { "hp-dep_1": "running" } }).apps.app_1;
  assert.deepEqual([up.currentReleaseId, up.url], ["rel_dep_1", "http://127.0.0.1:41000"]);
  assert.equal(appStatus(up, "connected").kind, "running");
  const gone = st(snap([a]), { observed: {} }).apps.app_1;
  assert.deepEqual([gone.currentReleaseId, gone.lastKnownReleaseId, gone.url], [null, "rel_dep_1", ""]);
  assert.equal(appStatus(gone, "connected").kind, "down", "recorded but not running is down, not 'running'");
  assert.equal(appStatus(gone, "stopped").kind, "unknown", "a stopped VM makes the state unknown");
  const unknown = st(snap([a]), { observed: null }).apps.app_1;
  assert.equal(unknown.currentReleaseId, null, "an unanswered Docker is never read as running");
});

test("an unfinished deployment with no writer is interrupted; with a writer it is running", () => {
  const d = dep("dep_2", { status: "running", unfinished: true, finishedMs: null, release: null, steps: [step("source", "succeeded"), step("build", "running", { finishedMs: null })] });
  const gone = deploymentOf(d, NOW, false);
  assert.equal(gone.status, "interrupted");
  assert.match(gone.failure, /stopped before this deployment finished/);
  assert.equal(gone.steps[1].status, "warning");
  assert.equal(deploymentOf(d, NOW, true).status, "running");
});

test("an interrupted cutover says the application may be down", () => {
  const d = dep("dep_3", { status: "running", unfinished: true, cutover: "begun", mayBeDown: true, finishedMs: null });
  assert.match(deploymentOf(d, NOW, false).failure, /may be down/);
});

test("failed build: nothing was stopped; failed cutover with a restored release keeps both facts", () => {
  const b = deploymentOf(dep("dep_4", { status: "failed", release: null, failure: { step: "build", kind: "build", message: "RUN failed", timedOut: false } }), NOW, false);
  assert.deepEqual([b.status, b.failedAt, b.nothingStopped], ["failed", "build", true]);
  const c = deploymentOf(dep("dep_5", {
    status: "failed", release: null, cutover: "failed", failure: { step: "cutover", kind: "cutover", message: "new release unhealthy", timedOut: false },
    recovery: { outcome: "restored", reason: null, reasonText: null, startedMs: NOW - 5000, finishedMs: NOW - 2000, budgetMs: 60000 }, interruption: { startedMs: NOW - 9000, endedMs: NOW - 2000 },
  }), NOW, false);
  assert.deepEqual([c.nothingStopped, c.recovery.outcome, c.interruption.label, c.recovery.durationLabel], [false, "restored", "7s", "3s"]);
});

test("a recovery recorded as in progress with no writer is never shown as restoring", () => {
  const d = dep("dep_6", { status: "failed", release: null, recovery: { outcome: "in_progress", reason: null, reasonText: null, startedMs: NOW - 1000, finishedMs: null, budgetMs: 60000 }, interruption: { startedMs: NOW - 2000, endedMs: null } });
  const gone = deploymentOf(d, NOW, false);
  assert.equal(gone.recovery.outcome, "restore_failed");
  assert.match(gone.recovery.detail, /result is unknown/);
  assert.match(gone.interruption.label, /unknown end/);
  assert.equal(deploymentOf(d, NOW, true).recovery.outcome, "in_progress");
});

test("pauses, held events and selected revisions are reported; resume needs the full revision", () => {
  const pausedApp = app([dep("dep_7", { status: "failed", release: null, cutover: "failed" })], {
    pauses: [{ origin: "dep_7", kind: "failed_cutover", sinceMs: NOW - 60000, context: "", recorded: true }],
    events: [{ delivery: "delivery-aaaaaaaaaaaa", source: "github", revision: "feedfacefeedface", receivedMs: NOW - 30000, status: "held", decidedMs: null, related: null },
      { delivery: "delivery-bbbbbbbbbbbb", source: "github", revision: "abcdabcdabcd", receivedMs: NOW - 20000, status: "selected", decidedMs: NOW - 10000, related: null },
      { delivery: "delivery-cccccccccccc", source: "github", revision: "0000000", receivedMs: NOW - 10000, status: "dismissed", decidedMs: NOW, related: null }],
  });
  const a = st(snap([pausedApp])).apps.app_1.automation;
  assert.equal(a.state, "paused");
  assert.deepEqual(a.held.map((h) => [h.status, h.revisionFull]), [["held", "feedfacefeedface"], ["selected", "abcdabcdabcd"]]);
  assert.equal(a.reason.kind, "failed_cutover");
});

test("secrets in variables and pins are names/ids/versions only", () => {
  const defs = [
    { scope: "app:app_1", name: "DB", kind: "secret", build: false, runtime: true, value: null, secretId: "app:app_1/DB", version: 3, atMs: NOW - 60000 },
    { scope: "app:app_1@production", name: "MODE", kind: "config", build: true, runtime: true, value: "prod", secretId: null, version: null, atMs: NOW - 60000 },
    { scope: "app:other", name: "NOPE", kind: "config", build: false, runtime: true, value: "x", secretId: null, version: null, atMs: NOW },
  ];
  const s = st(snap([app([dep("dep_1")])], { definitions: defs })).apps.app_1;
  assert.deepEqual(s.variables.map((v) => [v.name, v.type, v.scope, v.used, v.value, v.version]), [["DB", "secret", "application", "runtime", undefined, 3], ["MODE", "config", "environment", "build+runtime", "prod", undefined]]);
  assert.deepEqual(s.releases[0].secrets, [{ name: "DB", version: 2 }]);
  assert.doesNotMatch(JSON.stringify(s), /s3cr3t/);
});

test("the shipping gate is surfaced with the engine's own reason", () => {
  assert.equal(liveBlock(st(snap([]))), "closed in this build");
  assert.equal(liveBlock(st(snap([], { gate: { liveDeploy: true, liveReplacement: true, reason: "" } }))), null);
});

test("recorded step logs: withheld output is stated, truncation is stated, text is kept verbatim", () => {
  const lines = logLines([{ step: "build", text: "ok\nerror: boom\n", withheld: false, truncated: true, atMs: 1 }, { step: "source", text: "", withheld: true, truncated: false, atMs: 2 }]);
  assert.match(lines[0].text, /earlier output was cut/);
  assert.deepEqual(lines.slice(1, 3).map((l) => [l.kind, l.text]), [["", "ok"], ["err", "error: boom"]]);
  assert.match(lines.at(-1).text, /withheld/);
});

test("observed Docker state parses names and treats a non-answer as unknown", () => {
  assert.deepEqual(observedFrom([{ Names: "/a,b", State: "Running" }]), { a: "running", b: "running" });
  assert.equal(observedFrom(null), null);
});

test("the engine hook is the only place that calls Tauri; views never do", () => {
  const hook = readFileSync(new URL("../hooks/useDeployEngine.ts", import.meta.url), "utf8");
  assert.match(hook, /deploy_snapshot/);
  assert.match(hook, /deploy_op/);
  assert.doesNotMatch(hook, /\bdeploy_op\b[^\n]*(live|bypass|acceptance)/i);
});

test("production shows the engine; the simulation is only reachable through the development preview", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(app, /deployPreviewActive \? \{ sim: deploySim, preview: deployPreview \} : \{ engine: deployEngine \}/);
  assert.match(app, /const deployPreview = import\.meta\.env\.DEV/);
  const view = readFileSync(new URL("../views/deploy/DeployView.tsx", import.meta.url), "utf8");
  assert.match(view, /\{sim \? <>/, "simulation controls render only in simulation mode");
});
