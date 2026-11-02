// The real Deployments view in a DOM: navigation, held vs selected events, the review dialog, and dialogs under polling.
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { click, find, fakeEngine, mountView, shutdown, texts, type, win } from "./viewHarness.mjs";

after(() => shutdown());
import { toDeployState } from "./deployEngine.ts";

const NOW = 20_000_000;
const SHA = "3da0fcb5b2c8a5992c2ed0c5522c89b5836497af";
const GATE = { liveDeploy: false, liveReplacement: false, reason: "Live deployment is closed in this build." };
const ev = (status, delivery = "poll:shop:1", revision = SHA) => ({ delivery, source: "poll-late", revision, receivedMs: NOW - 120000, status, decidedMs: null, related: null });
const snapshot = (events, extra = {}) => ({
  schema: 1, generatedMs: NOW, runtimeDir: "/x", store: "ok", storeError: null, writerHeld: false, gate: GATE, definitions: [],
  repos: [{ name: "shop", url: "https://example.com/shop.git", app: "app_1", project: "demo", environment: null, branch: "main", filters: [], credential: "none", webhook: null, containerPort: 3000, dockerfile: "Dockerfile", context: null, target: null, healthPath: "/", seen: [] }],
  apps: [{ id: "app_1", repo: "shop", deployments: [], serving: null, pauses: [], events }], ...extra,
});
const stateOf = (events, extra) => toDeployState(snapshot(events, extra), { observed: {}, keychain: "unlocked" });
const engine = (state, calls) => fakeEngine(state, calls);

async function openApp(m) {
  await click(find(m.host, "button.dp-card", "shop"));
}

test("clicking Deployments in the sidebar, or '← Projects', returns from an application to the project list", async () => {
  const m = await mountView(engine);
  await m.render(stateOf([]), { homeSignal: 0 });
  assert.ok(find(m.host, "h1", "Projects"), "starts on the project list");
  await openApp(m);
  assert.ok(!find(m.host, "h1", "Projects") && find(m.host, "button", "← Projects"), "an application's screen, with a way back");
  // the sidebar item clicked again: App bumps homeSignal
  await m.render(stateOf([]), { homeSignal: 1 });
  assert.ok(find(m.host, "h1", "Projects") && !find(m.host, "button", "← Projects"), "back on the project list");
  // and the in-view button does the same
  await openApp(m);
  await click(find(m.host, "button", "← Projects"));
  assert.ok(find(m.host, "h1", "Projects"));
  // an unchanged signal on a later re-render (polling) does not throw the person back
  await openApp(m);
  await m.render(stateOf([]), { homeSignal: 1 });
  assert.ok(find(m.host, "button", "← Projects"), "still on the application after an ordinary re-render");
  await m.unmount();
});

test("the sidebar signal also closes an open dialog", async () => {
  const m = await mountView(engine);
  await m.render(stateOf([ev("held")]), { homeSignal: 0 });
  await openApp(m);
  await click(find(m.host, "button", /Review held events/));
  assert.ok(m.host.querySelector('[role="dialog"]'));
  await m.render(stateOf([ev("held")]), { homeSignal: 1 });
  assert.equal(m.host.querySelector('[role="dialog"]'), null);
  await m.unmount();
});

test("a held commit says 'held' and offers a review; once selected it is no longer counted as held", async () => {
  const held = await mountView(engine);
  await held.render(stateOf([ev("held")]));
  await openApp(held);
  assert.ok(texts(held.host, ".dp-banner h3").includes("1 held Git event"), texts(held.host, ".dp-banner h3").join("|"));
  assert.ok(find(held.host, "button", "Review held events (1)"));
  await held.unmount();

  const sel = await mountView(engine);
  await sel.render(stateOf([ev("selected")]));
  await openApp(sel);
  const titles = texts(sel.host, ".dp-banner h3");
  assert.ok(!titles.some((t) => /held Git event/.test(t)), `no held banner: ${titles.join("|")}`);
  assert.ok(titles.includes("Commit 3da0fcb is selected, not yet run"), titles.join("|"));
  assert.ok(!find(sel.host, "button", /Review held events/), "nothing is held, so no 'held' review button");
  assert.ok(find(sel.host, "button", "Review selected commit"), "but the selection can still be reviewed and dismissed");
  assert.match(sel.host.textContent, /nothing has run/i);
  await sel.unmount();
});

test("the review dialog records exactly the decision chosen, and a pending selection blocks choosing another", async () => {
  const m = await mountView(engine);
  await m.render(stateOf([ev("held", "poll:shop:1", SHA), ev("held", "poll:shop:2", "b".repeat(40))]));
  await openApp(m);
  await click(find(m.host, "button", /Review held events/));
  const dialog = m.host.querySelector('[role="dialog"]');
  assert.ok(dialog.textContent.includes(SHA) && dialog.textContent.includes("b".repeat(40)), "full commits are shown");
  assert.match(dialog.textContent, /Held/);
  await click(find(dialog, "button", "Select this revision"));
  assert.deepEqual(m.calls.at(-1), { op: "resume", app: "app_1", mode: "revision", revision: "b".repeat(40) }, "the newest held commit is preselected");
  await m.unmount();

  const d = await mountView(engine);
  await d.render(stateOf([ev("held")]));
  await openApp(d);
  await click(find(d.host, "button", /Review held events/));
  await click(find(d.host.querySelector('[role="dialog"]'), "button", "Dismiss all"));
  assert.deepEqual(d.calls.at(-1), { op: "resume", app: "app_1", mode: "dismiss" });
  await d.unmount();

  const p = await mountView(engine);
  await p.render(stateOf([ev("selected", "poll:shop:1"), ev("held", "poll:shop:2", "c".repeat(40))]));
  await openApp(p);
  await click(find(p.host, "button", /Review/));
  const pd = p.host.querySelector('[role="dialog"]');
  const choose = find(pd, "button", "Select this revision");
  assert.equal(choose.disabled, true, "the engine refuses a second selection while one is pending");
  assert.match(pd.textContent, /Selected — waiting for an attended deployment/);
  await p.unmount();
});

test("a dialog stays usable through repeated background polling: focus, caret, text, dropdown and DOM nodes", async () => {
  const m = await mountView(engine);
  await m.render(stateOf([]));
  await openApp(m);
  await click(find(m.host, '[role="tab"]', "Variables"));
  await click(find(m.host, "button", "Add variable"));
  const dialog = () => m.host.querySelector('[role="dialog"]');
  assert.ok(dialog());
  await click(find(dialog(), "button", "Config")); // a Config value is readable text; a Secret input is a different control
  const valueInput = dialog().querySelector("#vv");
  const scope = [...dialog().querySelectorAll("select")][1];
  const nodes = [dialog().querySelector("#vn"), valueInput, scope];
  valueInput.focus();
  await type(valueInput, "ui-test");
  valueInput.setSelectionRange(2, 2);
  let blurs = 0;
  scope.addEventListener("blur", () => blurs++);
  for (let i = 0; i < 12; i++) await m.render(stateOf([], { generatedMs: NOW + i * 3000 })); // a fresh state object, as every poll produces
  assert.equal(win.document.activeElement, valueInput);
  assert.equal(valueInput.value, "ui-test");
  assert.deepEqual([valueInput.selectionStart, valueInput.selectionEnd], [2, 2]);
  assert.deepEqual([dialog().querySelector("#vn"), dialog().querySelector("#vv"), [...dialog().querySelectorAll("select")][1]], nodes, "same DOM nodes: nothing was remounted");
  scope.focus();
  for (let i = 0; i < 12; i++) await m.render(stateOf([], { generatedMs: NOW + (20 + i) * 3000 }));
  assert.equal(win.document.activeElement, scope, "a focused dropdown keeps focus");
  assert.equal(blurs, 0, "and is never blurred, which is what closes it");
  await m.unmount();
});

test("a reserved variable name is explained and cannot be saved", async () => {
  const m = await mountView(engine);
  await m.render(stateOf([]));
  await openApp(m);
  await click(find(m.host, '[role="tab"]', "Variables"));
  await click(find(m.host, "button", "Add variable"));
  const dialog = m.host.querySelector('[role="dialog"]');
  assert.match(dialog.textContent, /Names starting with HARPOON_ are reserved/);
  await click(find(dialog, "button", "Config"));
  const name = dialog.querySelector("#vn");
  await type(name, "harpoon_x");
  assert.equal(find(dialog, "button", /^Save/).disabled, true);
  assert.match(dialog.textContent, /reserved for Harpoon \(for example the per-container response marker\)/);
  await type(name, "1bad");
  assert.equal(find(dialog, "button", /^Save/).disabled, true);
  assert.match(dialog.textContent, /start with a letter/);
  await type(name, "mode");
  assert.equal(find(dialog, "button", /^Save/).disabled, false, "a valid name can be saved");
  await type(dialog.querySelector("#vv"), "bad\tvalue");
  assert.equal(find(dialog, "button", /^Save/).disabled, true, "a value the engine would refuse is caught too");
  assert.match(dialog.textContent, /control characters/);
  await m.unmount();
});
