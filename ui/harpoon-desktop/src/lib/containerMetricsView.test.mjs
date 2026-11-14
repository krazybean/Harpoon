// The real polling hook and Metrics UI in a DOM. Only the one-shot backend call is replaced.
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { mkdirSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { click, find, fakeEngine, mountView, shutdown } from "./viewHarness.mjs";
import { toDeployState } from "./deployEngine.ts";

after(() => shutdown());
const out = new URL("../../node_modules/.cache/metrics-view/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const outfile = out + "cm.mjs";
await build({ stdin: { resolveDir: new URL("../views/deploy/", import.meta.url).pathname, loader: "ts", contents: 'export { ContainerMetrics } from "./ContainerMetrics.tsx";' }, outfile, bundle: true, format: "esm", platform: "node", logLevel: "silent", external: ["react", "react-dom"], jsx: "automatic", loader: { ".css": "empty" } });
const { ContainerMetrics } = await import(pathToFileURL(outfile).href);
const { createRoot } = await import("react-dom/client");
const React = await import("react");
const { act } = React;
const h = React.createElement;

const T0 = Date.now();
const reading = (over = {}) => ({ state: "ok", detail: null, sampledMs: Date.now(), containerId: "abc123", startedAt: "2026-11-14T00:00:00Z", restartCount: 0, cpuPercent: 2.5, cpus: 4, memRaw: 2 * 1024 ** 2, memCache: 1024 ** 2, memWorkingSet: 1024 ** 2, memLimit: null, netRx: 1000, netTx: 500, pids: 3, ...over });
const TARGET = { app: "app_1", deployment: "dep_1", container: "hp-dep_1", containerId: "abc123" };
const wait = (ms) => act(async () => { await new Promise((r) => setTimeout(r, ms)); });

async function mountMetrics(fetchOnce, target = TARGET, pollMs = 15) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const render = (t = target, f = fetchOnce) => act(async () => root.render(h(ContainerMetrics, { fetchOnce: f, target: t, pollMs })));
  await render();
  return { host, render, unmount: () => act(async () => { root.unmount(); host.remove(); }), text: () => host.textContent.replace(/\s+/g, " ") };
}

test("a running container shows CPU, memory (cache excluded), cumulative network and uptime, and says it is this container only", async () => {
  const m = await mountMetrics(async () => reading({ memLimit: 4 * 1024 ** 2 }));
  await wait(30);
  const t = m.text();
  assert.match(t, /Live/);
  assert.match(t, /2\.5%/);
  assert.match(t, /1\.0 MiB/, "working set, not raw");
  assert.match(t, /Excludes page cache/);
  assert.match(t, /Share of limit25\.0%/);
  assert.match(t, /1000 B in/);
  assert.match(t, /This container only, not the VM/);
  assert.match(t, /Session only/);
  await m.unmount();
});

test("without a container memory limit there is no percentage, and missing figures are dashes, not zeros", async () => {
  const m = await mountMetrics(async () => reading({ cpuPercent: null, netRx: null, netTx: null, memWorkingSet: null }));
  await wait(30);
  const t = m.text();
  assert.match(t, /Limitnone set/);
  assert.match(t, /Share of limit—/);
  assert.match(t, /CPU—/);
  assert.match(t, /Basisraw usage/);
  assert.match(t, /Docker did not provide enough to compute this/);
  await m.unmount();
});

test("a genuine measured zero is shown as zero", async () => {
  const m = await mountMetrics(async () => reading({ cpuPercent: 0, netRx: 0, netTx: 0 }));
  await wait(30);
  assert.match(m.text(), /CPU0\.0%/);
  assert.match(m.text(), /0 B in/);
  await m.unmount();
});

test("before the first answer: waiting; a failing backend: not available, with the reason", async () => {
  let release;
  const m = await mountMetrics(() => new Promise((r) => { release = r; }));
  assert.match(m.text(), /Waiting for the first sample/);
  await m.unmount();
  const bad = await mountMetrics(async () => { throw new Error("engine unreachable"); });
  await wait(30);
  assert.match(bad.text(), /Not available/);
  assert.match(bad.text(), /engine unreachable/);
  assert.doesNotMatch(bad.text(), /CPU/);
  await bad.unmount();
});

test("stopped, absent and not-owned containers are explained and show no live figures", async () => {
  for (const [state, shown] of [["stopped", /The container is stopped/], ["absent", /Docker has no such container/], ["not-owned", /not the serving release's container/]]) {
    const m = await mountMetrics(async () => reading({ state, detail: "why", cpuPercent: null }));
    await wait(30);
    assert.match(m.text(), shown, state);
    assert.doesNotMatch(m.text(), /Live/, state);
    await m.unmount();
  }
});

test("when the backend stops answering the figures go stale and are labelled, not shown as current", async () => {
  let calls = 0;
  const m = await mountMetrics(async () => { if (calls++ === 0) return reading({ sampledMs: Date.now() - 20_000 }); throw new Error("timed out"); });
  await wait(60);
  const t = m.text();
  assert.match(t, /Stale/);
  assert.match(t, /not current/);
  assert.doesNotMatch(t, /Live/);
  assert.ok(m.host.querySelector(".dp-mcard--stale"), "dimmed");
  await m.unmount();
});

test("sampling stops when the component unmounts", async () => {
  let calls = 0;
  const m = await mountMetrics(async () => { calls++; return reading(); });
  await wait(50);
  const before = calls;
  assert.ok(before >= 2, "it polled");
  await m.unmount();
  await wait(60);
  assert.equal(calls, before, "no request after unmount");
});

test("only one request is in flight at a time", async () => {
  let inflight = 0, max = 0;
  const m = await mountMetrics(async () => { max = Math.max(max, ++inflight); await new Promise((r) => setTimeout(r, 40)); inflight--; return reading(); });
  await wait(150);
  assert.equal(max, 1);
  await m.unmount();
});

test("a different serving container resets the history and queries the new target only", async () => {
  const seen = [];
  const f = async (t) => { seen.push(t.deployment); return reading({ containerId: t.deployment === "dep_1" ? "abc123" : "def456" }); };
  const m = await mountMetrics(f);
  await wait(50);
  await m.render({ ...TARGET, deployment: "dep_2", container: "hp-dep_2", containerId: "def456" });
  seen.length = 0;
  await wait(50);
  assert.ok(seen.length > 0 && seen.every((d) => d === "dep_2"), `only the new target is sampled: ${seen}`);
  assert.match(m.text(), /hp-dep_2/);
  await m.unmount();
});

test("a hidden window is not sampled", async () => {
  let calls = 0;
  Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
  const m = await mountMetrics(async () => { calls++; return reading(); });
  await wait(60);
  assert.equal(calls, 0);
  Object.defineProperty(document, "visibilityState", { value: "visible", configurable: true });
  await act(async () => { document.dispatchEvent(new window.Event("visibilitychange")); });
  await wait(30);
  assert.ok(calls > 0, "sampling resumes when visible");
  await m.unmount();
});

test("technical details hold the identifiers, closed by default", async () => {
  const m = await mountMetrics(async () => reading());
  await wait(30);
  const d = m.host.querySelector("details");
  assert.equal(d.open, false);
  assert.match(d.textContent, /abc123/);
  assert.match(d.textContent, /app_1/);
  assert.match(d.textContent, /includes page cache/);
  assert.doesNotMatch([...m.host.querySelectorAll("section")].map((s) => s.textContent).join(" "), /abc123/);
  await m.unmount();
});

// ---- the tab, in the real view: what it asks the backend to verify

const NOW = 20_000_000;
const GATE = { liveDeploy: false, liveReplacement: false, reason: "closed" };
const step = (name) => ({ name, status: "succeeded", startedMs: NOW - 60000, finishedMs: NOW - 50000, note: "", log: false });
const dep = (id) => ({ id, resources: [{ kind: "container", name: `hp-${id}`, id: "abc123def", state: "created" }], status: "succeeded", trigger: "cli", createdMs: NOW - 120000, startedMs: NOW - 119000, finishedMs: NOW - 100000, steps: [step("source"), step("build")], cutover: "confirmed", cutoverAtMs: NOW - 101000, failure: null, recovery: null, interruption: null, release: `rel_${id}`, replaces: null, rollbackTo: null, mayBeDown: false, unfinished: false, source: null, pin: { imageTag: "x:1", imageId: "sha256:x", hostPort: 41000, containerPort: 3000, healthPath: "/", envNames: [], secrets: [], snapshot: id, build: null }, config: null, configDiff: [] });
const snap = (id, serving) => ({ schema: 1, generatedMs: NOW, runtimeDir: "/x", source: { kind: "own", path: "/x", journalFormat: "sqlite", readOnly: false }, store: "ok", storeError: null, writerHeld: false, gate: GATE, definitions: [], repos: [{ name: "shop", url: "https://example.com/shop.git", app: "app_1", project: null, environment: null, branch: "main", filters: [], credential: "none", webhook: null, containerPort: 3000, dockerfile: "Dockerfile", context: null, target: null, healthPath: "/", seen: [] }], apps: [{ id: "app_1", repo: "shop", deployments: [dep(id)], serving, pauses: [], events: [] }] });
const sv = (id) => ({ release: `rel_${id}`, deployment: id, container: `hp-${id}`, via: "original", sinceMs: NOW - 90000, hostPort: 41000, healthPath: "/" });
const live = (id) => ({ [`hp-${id}`]: { id: "abc123def", state: "running", labels: { "io.harpoon.managed": "true", "io.harpoon.app": "app_1", "io.harpoon.deployment": id }, ports: [41000], mappings: [] } });

async function openMetrics(state, metrics) {
  const m = await mountView((s, calls) => fakeEngine(s, calls, { metrics }));
  await m.render(state);
  await click(find(m.host, "button.dp-card", "shop"));
  await click(find(m.host, "button[role=tab]", "Metrics"));
  return m;
}

test("the Metrics tab asks for exactly the serving release's container, with the recorded id", async () => {
  const targets = [];
  const m = await openMetrics(toDeployState(snap("dep_1", sv("dep_1")), { observed: live("dep_1"), keychain: "unlocked" }), async (t) => { targets.push(t); return reading(); });
  await wait(30);
  assert.deepEqual(targets[0], { app: "app_1", deployment: "dep_1", container: "hp-dep_1", containerId: "abc123def" });
  assert.match(m.host.textContent, /Live/);
  await m.unmount();
});

test("with no running serving container the tab says so and makes no request", async () => {
  let calls = 0;
  const m = await openMetrics(toDeployState(snap("dep_1", sv("dep_1")), { observed: {}, keychain: "unlocked" }), async () => { calls++; return reading(); });
  await wait(30);
  assert.equal(calls, 0);
  assert.match(m.host.textContent, /No serving container/);
  assert.doesNotMatch(m.host.textContent, /Resource charts are not part of this build/);
  await m.unmount();
});

test("leaving the Metrics tab stops sampling", async () => {
  let calls = 0;
  const m = await openMetrics(toDeployState(snap("dep_1", sv("dep_1")), { observed: live("dep_1"), keychain: "unlocked" }), async () => { calls++; return reading(); });
  await wait(30);
  await click(find(m.host, "button[role=tab]", "Overview"));
  const before = calls;
  await wait(3500);
  assert.equal(calls, before);
  await m.unmount();
});
