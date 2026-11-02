// The Deployments view with a runtime attached read-only: the "Open runtime…" workflow, the badges, disabled actions,
// and the recorded-vs-observed panel, rendered by the real components.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test, { after } from "node:test";
import { click, fakeEngine, find, mountView, shutdown, texts } from "./viewHarness.mjs";
import { toDeployState } from "./deployEngine.ts";

after(() => shutdown());

const B7 = JSON.parse(readFileSync(new URL("../../src-tauri/tests/fixtures/b7-legacy-snapshot.json", import.meta.url), "utf8"));
const APP = "app_01M6DPQ543ASWBHW7PW17ZAJ3B", DEP = "dep_01M6DPQ543ASWBHW7PW17ZAJ3C", CTR = `hp-${DEP}`;
const CID = "2f70855fd334312f11ae3b2c5cb2ef42af49203820869637c71c02ead50ed936";
const PATH = "/Volumes/evidence/runtime";
const OWN = { schema: 1, generatedMs: B7.generatedMs, runtimeDir: "/own", source: { kind: "own", path: "/own", journalFormat: "none", readOnly: false }, store: "missing", storeError: null, writerHeld: false, gate: B7.gate, repos: [], apps: [], definitions: [] };
const LABELS = { "io.harpoon.managed": "true", "io.harpoon.app": APP, "io.harpoon.deployment": DEP };
const matching = { [CTR]: { id: CID, state: "running", labels: LABELS, ports: [38000] } };
const stateOf = (observed, externals = [{ path: PATH, snapshot: B7, error: null }]) => toDeployState(OWN, { observed, keychain: "unlocked", externals });
const engine = (state, calls, over) => fakeEngine(state, calls, over);

async function openExternalApp(m) {
  await click(find(m.host, "button.dp-card", "External"));
}

test("'Open runtime…' picks a folder, attaches it, and says why when it cannot", async () => {
  const picked = [];
  const m = await mountView((s, calls) => engine(s, calls, {
    pickFolder: async () => (picked.push(1), "/Volumes/evidence/runtime"),
    attachRuntime: async (p) => { calls.push({ attach: p }); return p.includes("evidence") ? { ok: true } : { ok: false, error: "nope" }; },
  }));
  await m.render(stateOf({}, []));
  const button = find(m.host, "button", "Open runtime…");
  assert.ok(button, "offered on the project list, also when Harpoon has no projects of its own");
  await click(button);
  assert.equal(picked.length, 1);
  assert.deepEqual(m.calls.at(-1), { attach: "/Volumes/evidence/runtime" });
  assert.equal(find(m.host, ".dp-banner", "cannot be opened"), undefined);
  await m.unmount();

  const bad = await mountView((s, calls) => engine(s, calls, { pickFolder: async () => "/somewhere/else", attachRuntime: async () => ({ ok: false, error: "No deploy-journal folder in /somewhere/else." }) }));
  await bad.render(stateOf({}, []));
  await click(find(bad.host, "button", "Open runtime…"));
  assert.match(find(bad.host, ".dp-banner", "cannot be opened").textContent, /No deploy-journal folder in \/somewhere\/else\./);
  await bad.unmount();

  const cancelled = await mountView((s, calls) => engine(s, calls, { pickFolder: async () => null }));
  await cancelled.render(stateOf({}, []));
  await click(find(cancelled.host, "button", "Open runtime…"));
  assert.equal(cancelled.calls.length, 0, "cancelling the picker attaches nothing");
  await cancelled.unmount();
});

test("an attached runtime is labelled 'External · read-only' and can be closed", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  assert.match(find(m.host, ".dp-h2", "External").textContent, /External · runtime/);
  assert.ok(find(m.host, ".dp-pill", "External · read-only"));
  assert.ok(find(m.host, "button.dp-card", "External · read-only"), "the application card carries the badge too");
  assert.match(m.host.textContent, /legacy journal\.wal, not migrated/);
  await click(find(m.host, "button", "Close"));
  assert.deepEqual(m.calls.at(-1), { detach: PATH });
  await m.unmount();
});

test("an attached runtime that cannot be read is reported with a way to close it", async () => {
  const m = await mountView(engine);
  await m.render(stateOf({}, [{ path: "/gone", snapshot: null, error: "Cannot read /gone: No such file or directory (os error 2)" }]));
  assert.match(find(m.host, ".dp-banner", "cannot be read").textContent, /Cannot read \/gone/);
  await click(find(m.host, ".dp-banner button", "Close"));
  assert.deepEqual(m.calls.at(-1), { detach: "/gone" });
  await m.unmount();
});

test("the external application says what it is, offers no action, and shows recorded vs observed", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  const banner = find(m.host, ".dp-banner", "External · read-only");
  assert.match(banner.textContent, /\/Volumes\/evidence\/runtime/);
  assert.match(banner.textContent, /read where it is and not migrated/);
  assert.match(banner.textContent, /cannot change it/);
  for (const label of ["Deploy", "Redeploy", "Roll back…", "Promote…"]) {
    const b = find(m.host, ".dp-actions button", label);
    assert.ok(b, label);
    assert.equal(b.disabled, true, `${label} is disabled`);
    assert.match(b.title, /attached read-only/, `${label} says why`);
  }
  for (const gone of [/Check for new commits/, /Review held events/, /Review selected commit/]) assert.equal(find(m.host, "button", gone), undefined, String(gone));
  const panel = find(m.host, "section.dp-panel", "Recorded vs observed");
  assert.match(panel.textContent, /Matches Docker/);
  assert.match(panel.textContent, /Recorded.*release rel_01M6DPQ543ASWBHW7PW17ZAJ3C serving in container hp-dep_01M6DPQ543ASWBHW7PW17ZAJ3C \(2f70855fd334\), port lease 38000/);
  assert.match(panel.textContent, /Observed.*container 2f70855fd334 running/);
  assert.match(panel.textContent, /http:\/\/127\.0\.0\.1:38000.*from the recorded port lease/);
  await m.unmount();
});

test("when Docker disagrees with the record the page says so instead of showing the release as running", async () => {
  for (const [name, observed, verdict, note] of [
    ["absent", {}, "Recorded, not in Docker", /no such container/],
    ["another deployment's container", { [CTR]: { id: CID, state: "running", labels: { ...LABELS, "io.harpoon.deployment": "dep_OTHER" }, ports: [38000] } }, "Does not match Docker", /dep_OTHER/],
    ["stopped", { [CTR]: { id: CID, state: "exited", labels: LABELS, ports: [] } }, "Recorded, not running", /exited/],
  ]) {
    const m = await mountView(engine);
    await m.render(stateOf(observed));
    await openExternalApp(m);
    const panel = find(m.host, "section.dp-panel", "Recorded vs observed");
    assert.match(panel.textContent, new RegExp(verdict), name);
    assert.match(panel.textContent, note, name);
    assert.doesNotMatch(panel.textContent, /Loopback URL/, `${name}: no URL for something not confirmed`);
    assert.ok(!find(m.host, ".dp-hero .dp-pill", "Running"), `${name}: not shown as running`);
    await m.unmount();
  }
});

test("variables of an attached runtime cannot be added or edited", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  await click(find(m.host, '[role="tab"]', "Variables"));
  const add = find(m.host, "button", "Add variable");
  assert.equal(add.disabled, true);
  assert.match(add.title, /attached read-only/);
  await click(add);
  assert.equal(m.host.querySelector('[role="dialog"]'), null, "no dialog opens");
  await m.unmount();
});

test("an external deployment shows what the ledger recorded, with ids, and asks the store for no logs", async () => {
  let asked = 0;
  const m = await mountView((s, calls) => engine(s, calls, { stepLogs: async () => (asked++, []) }));
  await m.render(stateOf(matching));
  await openExternalApp(m);
  await click(find(m.host, '[role="tab"]', /Deployments/));
  await click(find(m.host, "tr.is-click", DEP));
  const docker = find(m.host, "section.dp-panel", "Docker objects");
  assert.match(docker.textContent, /container.*hp-dep_01M6DPQ543ASWBHW7PW17ZAJ3C.*2f70855fd334.*created/);
  assert.match(docker.textContent, /port.*38000.*created/);
  assert.match(docker.textContent, /port.*39000.*released/);
  for (const label of [/Retry deployment/, /Roll back to/, /Redeploy this revision/]) {
    const b = find(m.host, ".dp-actions button", label);
    if (b) assert.equal(b.disabled, true, String(label));
  }
  assert.equal(asked, 0, "Harpoon's own step-log store is never asked about another runtime's deployment");
  await m.unmount();
});
