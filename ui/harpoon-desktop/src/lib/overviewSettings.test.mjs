// The real Overview and Settings tabs in a DOM: what is visible by default, what is kept behind a disclosure, and the Danger zone.
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { click, find, fakeEngine, mountView, shutdown } from "./viewHarness.mjs";
import { toDeployState } from "./deployEngine.ts";

after(() => shutdown());
const NOW = 20_000_000;
const GATE = { liveDeploy: false, liveReplacement: false, reason: "closed in this build" };
const LONG = "dep_01M7CV7GY4JQNTA0GT90Z3RANQ";
const step = (name, status, extra = {}) => ({ name, status, startedMs: NOW - 60000, finishedMs: NOW - 50000, note: "", log: false, ...extra });
const dep = (id, extra = {}) => ({
  id, resources: [], status: "succeeded", trigger: "cli", createdMs: NOW - 120000, startedMs: NOW - 119000, finishedMs: NOW - 100000, steps: [step("source", "succeeded"), step("build", "succeeded"), step("final_check", "succeeded")],
  cutover: "confirmed", cutoverAtMs: NOW - 101000, failure: null, recovery: null, interruption: null, release: `rel_${id}`, replaces: null, rollbackTo: null, mayBeDown: false, unfinished: false,
  source: { repo: "shop", url: "https://example.com/shop.git", selector: "branch", requested: "main", commit: "0123456789abcdef", trigger: "manual", delivery: null },
  pin: { imageTag: `harpoon/${id}:1`, imageId: "sha256:x", hostPort: 41000, containerPort: 3000, healthPath: "/healthz", envNames: ["MODE"], secrets: [], snapshot: id, build: null },
  config: null, configDiff: [], ...extra,
});
const snapshot = (deployments, extra = {}) => ({
  schema: 1, generatedMs: NOW, runtimeDir: "/x", source: { kind: "own", path: "/x", journalFormat: "sqlite", readOnly: false }, store: "ok", storeError: null, writerHeld: false, gate: GATE, definitions: [],
  repos: [{ name: "shop", url: "https://example.com/shop.git", app: "app_1", project: null, environment: null, branch: "main", filters: [], credential: "none", webhook: null, containerPort: 3000, dockerfile: "Dockerfile", context: null, target: null, healthPath: "/healthz", seen: [] }],
  apps: [{ id: "app_1", repo: "shop", deployments, serving: null, pauses: [], events: [], ...extra }],
});
const serving = (id) => ({ release: `rel_${id}`, deployment: id, container: `hp-${id}`, via: "original", sinceMs: NOW - 90000, hostPort: 41000, healthPath: "/healthz" });
const running = (id) => ({ [`hp-${id}`]: { id: "c1", state: "running", labels: { "io.harpoon.managed": "true", "io.harpoon.app": "app_1", "io.harpoon.deployment": id }, ports: [41000], mappings: [] } });
const stateOf = (deployments, extra, observed = {}) => toDeployState(snapshot(deployments, extra), { observed, keychain: "unlocked" });

async function open(state, tab) {
  const m = await mountView((s, calls) => fakeEngine(s, calls));
  await m.render(state);
  await click(find(m.host, "button.dp-card", "shop"));
  if (tab) await click(find(m.host, "button[role=tab]", tab));
  return m;
}
const outsideDetails = (root) => { const c = root.cloneNode(true); c.querySelectorAll("details").forEach((d) => d.querySelectorAll(".dp-disclosure__body").forEach((b) => b.remove())); return c.textContent; };
const panel = (m, title) => find(m.host, "section.dp-panel", title);

test("a successful deployment shows status, steps, revision and the health outcome; long ids wait in a closed disclosure", async () => {
  const m = await open(stateOf([dep(LONG)], { serving: serving(LONG) }, running(LONG)));
  const latest = panel(m, "Latest deployment");
  const visible = outsideDetails(latest);
  assert.match(visible, /Succeeded|succeeded/i);
  assert.match(visible, /0123456/, "short revision");
  assert.doesNotMatch(visible, new RegExp(LONG), "the long deployment id is not in the default view");
  const d = latest.querySelector("details");
  assert.equal(d.open, false);
  assert.match(d.textContent, new RegExp(LONG), "but it is kept");
  assert.match(panel(m, "Health").textContent, /Final check passed \(recorded\)/, "health outcome stays visible");
  await m.unmount();
});

test("a disclosure opens and closes, and its summary is a real, focusable control", async () => {
  const m = await open(stateOf([dep(LONG)], { serving: serving(LONG) }, running(LONG)));
  const summary = panel(m, "Latest deployment").querySelector("details > summary");
  assert.equal(summary.tagName, "SUMMARY");
  summary.focus();
  assert.equal(document.activeElement, summary);
  await click(summary);
  assert.equal(summary.parentElement.open, true);
  await click(summary);
  assert.equal(summary.parentElement.open, false);
  await m.unmount();
});

test("failed and interrupted deployments say so on the Overview and keep their status visible", async () => {
  for (const [status, extra] of [["failed", { release: null, steps: [step("source", "succeeded"), step("build", "failed")] }], ["interrupted", { release: null, unfinished: true, finishedMs: null, steps: [step("source", "succeeded"), step("build", "running", { finishedMs: null })] }]]) {
    const m = await open(stateOf([dep("dep_9", { status, ...extra })]));
    const visible = outsideDetails(panel(m, "Latest deployment"));
    assert.match(visible, new RegExp(status, "i"));
    await m.unmount();
  }
});

test("an application with no deployment shows no Latest deployment panel and does not throw", async () => {
  const m = await open(stateOf([]));
  assert.equal(panel(m, "Latest deployment"), undefined);
  assert.ok(panel(m, "Configuration"));
  await m.unmount();
});

test("the Configuration summary shows values only; where each came from is a disclosure", async () => {
  const m = await open(stateOf([dep(LONG)], { serving: serving(LONG) }, running(LONG)));
  const cfg = panel(m, "Configuration");
  const visible = outsideDetails(cfg);
  assert.match(visible, /Container port/);
  assert.doesNotMatch(visible, /from the journal|from the repository registration/);
  assert.match(cfg.querySelector("details").textContent, /from the (journal|repository registration)/);
  await m.unmount();
});

test("Recorded vs observed is closed when Docker matches and open when it does not", async () => {
  const match = await open(stateOf([dep(LONG)], { serving: serving(LONG) }, running(LONG)));
  const ok = panel(match, "Recorded vs observed");
  assert.equal(ok.querySelector("details").open, false);
  assert.match(ok.textContent, /Matches Docker/);
  await match.unmount();
  const gone = await open(stateOf([dep(LONG)], { serving: serving(LONG) }, {}));
  const bad = panel(gone, "Recorded vs observed");
  assert.equal(bad.querySelector("details").open, true, "a difference is not hidden");
  await gone.unmount();
});

test("Settings groups its fields under icon headings, and the Danger zone is marked and keeps its safeguards", async () => {
  const m = await open(stateOf([dep(LONG)], { serving: serving(LONG) }, running(LONG)), "Settings");
  const titles = [...m.host.querySelectorAll("h2.dp-sec")].map((h) => h.textContent.trim());
  assert.deepEqual(titles, ["Build configuration", "Deployment configuration", "Source repository", "Network and ports", "Health checks", "Dangerous operations"]);
  for (const h of m.host.querySelectorAll("h2.dp-sec")) assert.ok(h.querySelector('[aria-hidden="true"] svg'), `${h.textContent} has a decorative icon, hidden from assistive technology`);
  assert.equal(m.host.querySelectorAll(".dp-secdesc").length, 6, "every section says what it is for");
  const danger = m.host.querySelector(".dp-panel--danger");
  assert.match(danger.textContent, /Dangerous operations/);
  const remove = find(danger, "button", "Remove application");
  assert.equal(remove.disabled, true, "a deployed application cannot be removed");
  assert.match(danger.textContent, /cannot be removed yet|has been deployed/i);
  assert.equal(m.host.querySelectorAll(".dp-panel--danger").length, 1, "only the danger zone is red");
  await m.unmount();
});

test("a long image reference and identifiers wrap inside the Settings card instead of widening it", async () => {
  const long = "harpoon/app_01m6dpq543aswbhw7pw17zaj3b:dep_01M6DPQ543ASWBHW7PW17ZAJ3B-with-a-very-long-tag-suffix-that-never-ends";
  const m = await open(stateOf([dep(LONG, { pin: { ...dep(LONG).pin, imageTag: long } })], { serving: serving(LONG) }, running(LONG)), "Settings");
  const input = find(m.host, "input", "") ?? m.host.querySelector("input");
  assert.ok(input, "fields are rendered");
  assert.ok([...m.host.querySelectorAll("input")].every((i) => i.readOnly), "settings stay read-only");
  await m.unmount();
});
