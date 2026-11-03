// The first-run Deployments walkthrough: persistence and versioning, the step list against what is on screen, the
// controller's lifecycle, and the real view with the tour switched on. Driver.js is faked except in the smoke tests.
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { readFileSync } from "node:fs";
import { click, find, fakeEngine, mountView, shutdown, win } from "./viewHarness.mjs";
import { toDeployState } from "./deployEngine.ts";
import { browserStore, DEPLOYMENTS_TOUR_KEY, DEPLOYMENTS_TOUR_VERSION, memoryStore, readTourStatus, writeTourStatus } from "./tour/onboardingStore.ts";
import { resolveSteps, STEPS } from "./tour/deploymentsTour.ts";
import { startDeploymentsTour } from "./tour/tourController.ts";

after(() => shutdown());

const NOW = 20_000_000;
const GATE = { liveDeploy: false, liveReplacement: false, reason: "Live deployment is closed in this build." };
const snapshot = (withApp) => ({
  schema: 1, generatedMs: NOW, runtimeDir: "/x", source: { kind: "own", path: "/x", journalFormat: "sqlite", readOnly: false }, store: "ok", storeError: null, writerHeld: false, gate: GATE, definitions: [],
  repos: withApp ? [{ name: "shop", url: "https://example.com/shop.git", app: "app_1", project: "demo", environment: null, branch: "main", filters: [], credential: "none", webhook: null, containerPort: 3000, dockerfile: "Dockerfile", context: null, target: null, healthPath: "/", seen: [] }] : [],
  apps: withApp ? [{ id: "app_1", repo: "shop", deployments: [], serving: null, pauses: [], events: [] }] : [],
});
const stateOf = (withApp) => toDeployState(snapshot(withApp), { observed: {}, keychain: "unlocked" });
const engine = (state, calls) => fakeEngine(state, calls);

/** A stand-in for Driver.js that records how it was configured and used. */
function fakeDriver() {
  const rec = { configs: [], drives: 0, destroys: 0, active: false };
  const load = async () => (config) => {
    rec.configs.push(config);
    return {
      drive() { rec.drives++; rec.active = true; },
      moveNext() { rec.moved = (rec.moved ?? 0) + 1; },
      hasNextStep: () => (rec.moved ?? 0) < config.steps.length - 1,
      destroy() { rec.destroys++; rec.active = false; },
      isActive: () => rec.active,
    };
  };
  return { rec, load };
}
const tick = (ms = 30) => new Promise((r) => setTimeout(r, ms));
const done = (rec) => { while (rec.active) rec.configs.at(-1).onNextClick(); };
const stored = (store) => JSON.parse(store.getItem(DEPLOYMENTS_TOUR_KEY) ?? "null");

// ---------------------------------------------------------------- persistence and versioning

test("the remembered outcome is versioned: an older or damaged record is never read", () => {
  const s = memoryStore();
  assert.equal(readTourStatus(s), null, "nothing remembered yet");
  writeTourStatus(s, "dismissed", DEPLOYMENTS_TOUR_KEY, 5);
  assert.equal(readTourStatus(s), "dismissed");
  assert.match(DEPLOYMENTS_TOUR_KEY, new RegExp(`\\.v${DEPLOYMENTS_TOUR_VERSION}$`));
  // a revised tour reads a different key, so the old record does not apply
  assert.equal(readTourStatus(s, `harpoon.onboarding.deployments.v${DEPLOYMENTS_TOUR_VERSION + 1}`), null);
  // a record from a different version number under this key, junk, or an unknown status is ignored
  for (const raw of [JSON.stringify({ version: DEPLOYMENTS_TOUR_VERSION + 1, status: "completed" }), "{", "null", JSON.stringify({ version: DEPLOYMENTS_TOUR_VERSION, status: "maybe" }), ""]) {
    assert.equal(readTourStatus(memoryStore({ [DEPLOYMENTS_TOUR_KEY]: raw })), null, raw);
  }
  // storage that throws neither reads nor writes loudly
  const broken = { getItem() { throw new Error("blocked"); }, setItem() { throw new Error("blocked"); } };
  assert.equal(readTourStatus(broken), null);
  writeTourStatus(broken, "completed");
});

test("the browser store falls back to memory when local storage is unavailable", () => {
  const s = browserStore(); // no localStorage in this environment
  assert.equal(s.getItem("k"), null);
  s.setItem("k", "v");
  assert.equal(s.getItem("k"), "v", "remembered for the session");
});

// ---------------------------------------------------------------- the step list

test("six steps, each honest about whether its target is on screen", () => {
  assert.equal(STEPS.length, 6);
  const none = resolveSteps(() => false);
  assert.ok(none.every((s) => s.anchor === null && s.text.length > 20), "with nothing on screen every step is a plain explanation");
  const all = resolveSteps(() => true);
  assert.deepEqual(all.map((s) => s.anchor), ["create-project", "app-status", "tab-deployments", "tab-variables", "recovery-actions", "replay-tour"]);
  const home = resolveSteps((a) => a === "create-project" || a === "replay-tour" || a === "app-list");
  assert.deepEqual(home.map((s) => s.anchor), ["create-project", "app-list", null, null, null, "replay-tour"]);
  assert.notEqual(home[2].text, all[2].text, "the wording changes with whether the target is shown");
  for (const s of STEPS) assert.ok(s.title.length < 40 && s.here.length < 220 && s.elsewhere.length < 260, `${s.id} stays short`);
});

// ---------------------------------------------------------------- the controller with a fake Driver.js

test("the controller: safe configuration, skip, close and finish are remembered, an interruption is not", async () => {
  for (const [how, want] of [["skip", "dismissed"], ["close", "dismissed"], ["finish", "completed"], ["escape-path", "dismissed"]]) {
    const { rec, load } = fakeDriver();
    const store = memoryStore();
    let ended;
    const h = await startDeploymentsTour({ doc: document, store, load, reducedMotion: false, onEnd: (st) => (ended = st) });
    assert.ok(h.active && rec.drives === 1);
    const c = rec.configs[0];
    assert.equal(c.steps.length, 6);
    assert.equal(c.disableActiveInteraction, true, "the highlighted control can be looked at, never pressed");
    assert.equal(c.overlayClickBehavior, "none");
    assert.equal(c.allowClose, true);
    assert.equal(c.allowKeyboardControl, true);
    if (how === "skip") { const footer = document.createElement("div"); c.onPopoverRender({ footer, closeButton: document.createElement("button") }); footer.querySelector(".harpoon-tour-skip").click(); }
    if (how === "close") c.onCloseClick();
    if (how === "finish") done(rec);
    if (how === "escape-path") c.onDestroyStarted();
    assert.equal(ended, want, how);
    assert.equal(readTourStatus(store), want);
    assert.equal(rec.active, false);
    assert.equal(h.active, false);
  }
  // interrupted by the app (unmount, another screen): torn down, nothing remembered
  const { rec, load } = fakeDriver();
  const store = memoryStore();
  const h = await startDeploymentsTour({ doc: document, store, load });
  h.destroy();
  assert.equal(rec.active, false);
  assert.equal(readTourStatus(store), null);
  h.destroy(); // idempotent
  assert.equal(rec.destroys, 1);
});

test("a Driver.js that fails to load or to start leaves nothing behind and remembers nothing", async () => {
  const store = memoryStore();
  assert.equal(await startDeploymentsTour({ doc: document, store, load: async () => { throw new Error("no chunk"); } }), null);
  const bad = async () => () => ({ drive() { throw new Error("boom"); }, destroy() {}, isActive: () => false, moveNext() {}, hasNextStep: () => false });
  assert.equal(await startDeploymentsTour({ doc: document, store, load: bad }), null);
  assert.equal(readTourStatus(store), null);
});

test("reduced motion turns the animation and smooth scrolling off", async () => {
  for (const [reduced, want] of [[true, false], [false, true]]) {
    const { rec, load } = fakeDriver();
    const h = await startDeploymentsTour({ doc: document, store: memoryStore(), load, reducedMotion: reduced });
    assert.equal(rec.configs[0].animate, want);
    assert.equal(rec.configs[0].smoothScroll, want);
    h.destroy();
  }
  // and it follows the operating-system setting when not told
  const old = win.matchMedia;
  win.matchMedia = (q) => ({ matches: /reduce/.test(q) });
  const { rec, load } = fakeDriver();
  (await startDeploymentsTour({ doc: document, store: memoryStore(), load })).destroy();
  win.matchMedia = old;
  assert.equal(rec.configs[0].animate, false);
});

test("focus returns to where the person was when the tour ends", async () => {
  const b = document.createElement("button");
  document.body.appendChild(b);
  b.focus();
  const { rec, load } = fakeDriver();
  const h = await startDeploymentsTour({ doc: document, store: memoryStore(), load });
  const other = document.createElement("button");
  document.body.appendChild(other);
  other.focus();
  done(rec);
  assert.equal(document.activeElement, b);
  assert.ok(!h.active);
  b.remove(); other.remove();
});

// ---------------------------------------------------------------- the real view

async function mountTour(withApp, o = {}) {
  const fd = fakeDriver();
  const store = o.store ?? memoryStore();
  const m = await mountView(engine, { onboarding: { store, load: fd.load } });
  await m.render(stateOf(withApp), o.props);
  return { m, fd, store };
}

test("first visit launches the tour once its targets are rendered; the six steps reach real, stable anchors", async () => {
  const { m, fd } = await mountTour(true);
  await tick();
  assert.equal(fd.rec.drives, 1, "started by itself");
  const el = fd.rec.configs[0].steps.map((s) => s.element?.());
  assert.ok(el[0]?.matches('[data-tour="create-project"]'), "registration step on the Create project button");
  assert.ok(el[1]?.matches('[data-tour="app-list"]'), "status step on the project list");
  assert.deepEqual(el.slice(2, 5), [undefined, undefined, undefined], "app-level steps are plain explanations on the project list");
  assert.ok(el[5]?.matches('[data-tour="replay-tour"]'), "last step points at Replay walkthrough");
  assert.ok(!find(m.host, "button", "Roll back"), "nothing app-level is on this screen");
  await m.unmount();
});

test("an empty first run still tours honestly: no invented project, no highlight where there is nothing", async () => {
  const { m, fd } = await mountTour(false);
  await tick();
  assert.equal(fd.rec.drives, 1);
  const steps = fd.rec.configs[0].steps;
  assert.ok(steps[0].element?.()?.matches('[data-tour="create-project"]'));
  assert.equal(steps[1].element, undefined, "no project list to highlight");
  assert.equal(steps[2].element, undefined);
  assert.match(steps[1].popover.description, /Once a project exists/);
  assert.equal(find(m.host, "button.dp-card", "demo"), undefined, "no sample application was added");
  await m.unmount();
});

test("an application page: its status, tabs and action row are the targets", async () => {
  const { m, fd } = await mountTour(true, { store: memoryStore({ [DEPLOYMENTS_TOUR_KEY]: JSON.stringify({ version: DEPLOYMENTS_TOUR_VERSION, status: "dismissed", at: 1 }) }) });
  await click(find(m.host, "button.dp-card", "shop"));
  await click(find(m.host, "button", "Replay walkthrough"));
  await tick();
  assert.equal(fd.rec.drives, 1, "replay works on any screen");
  const el = fd.rec.configs[0].steps.map((s) => s.element?.());
  assert.deepEqual(el.map((e) => e?.getAttribute("data-tour")), [undefined, "app-status", "tab-deployments", "tab-variables", "recovery-actions", "replay-tour"]);
  await m.unmount();
});

test("after completion or dismissal a later visit does not start it; Replay still does", async () => {
  for (const how of ["finish", "close"]) {
    const { m, fd, store } = await mountTour(true);
    await tick();
    how === "finish" ? done(fd.rec) : fd.rec.configs[0].onCloseClick();
    assert.equal(stored(store).status, how === "finish" ? "completed" : "dismissed");
    assert.equal(stored(store).version, DEPLOYMENTS_TOUR_VERSION);
    await m.unmount();

    const again = fakeDriver();
    const m2 = await mountView(engine, { onboarding: { store, load: again.load } });
    await m2.render(stateOf(true));
    await tick();
    assert.equal(again.rec.drives, 0, `not again after ${how}`);
    await click(find(m2.host, "button", "Replay walkthrough"));
    await tick();
    assert.equal(again.rec.drives, 1, "replay");
    await m2.unmount();
  }
});

test("a revised tour version is offered again", async () => {
  const old = memoryStore({ [`harpoon.onboarding.deployments.v${DEPLOYMENTS_TOUR_VERSION - 1}`]: JSON.stringify({ version: DEPLOYMENTS_TOUR_VERSION - 1, status: "completed" }) });
  const { m, fd } = await mountTour(true, { store: old });
  await tick();
  assert.equal(fd.rec.drives, 1);
  await m.unmount();
});

test("it does not start itself while loading or over a running job", async () => {
  const fd = fakeDriver();
  const store = memoryStore();
  // a loading state
  const loading = { ...stateOf(true), scenario: "loading" };
  const m = await mountView(engine, { onboarding: { store, load: fd.load } });
  await m.render(loading);
  await tick();
  assert.equal(fd.rec.drives, 0, "loading");
  // a running job
  await m.render({ ...stateOf(true), job: { id: "j" } });
  await tick();
  assert.equal(fd.rec.drives, 0, "a running job");
  await m.unmount();
  assert.equal(fd.rec.drives, 0);
});

test("moving to another screen, or unmounting, ends a running tour without remembering it", async () => {
  const { m, fd, store } = await mountTour(true);
  await tick();
  assert.equal(fd.rec.active, true);
  await m.render(stateOf(true), { homeSignal: 0 });
  assert.equal(fd.rec.active, true, "an ordinary re-render (polling) leaves it running");
  await click(find(m.host, "button.dp-card", "shop")); // the app navigates (the fake Driver.js does not block clicks)
  assert.equal(fd.rec.active, false, "a new screen ends it");
  assert.equal(fd.rec.destroys, 1);
  assert.equal(readTourStatus(store), null, "interrupted, not finished");
  await m.unmount();
  assert.equal(fd.rec.destroys, 1, "already gone: unmount destroys nothing twice");

  const again = await mountTour(true);
  await tick();
  assert.equal(again.fd.rec.active, true);
  await again.m.unmount();
  assert.equal(again.fd.rec.active, false, "unmount destroys the tour");
  assert.equal(readTourStatus(again.store), null);
});

test("nothing in the tour or the Replay button runs a deployment operation", async () => {
  const { m, fd } = await mountTour(true);
  await tick();
  const c = fd.rec.configs[0];
  for (let i = 0; i < 6; i++) { c.onNextClick(); }
  await click(find(m.host, "button", "Replay walkthrough"));
  await tick();
  fd.rec.configs.at(-1).onCloseClick();
  assert.deepEqual(m.calls, [], "the engine was never asked to do anything");
  await m.unmount();
  for (const f of ["tour/tourController.ts", "tour/deploymentsTour.ts", "tour/onboardingStore.ts", "../hooks/useDeploymentsTour.ts"]) {
    const src = readFileSync(new URL(f, import.meta.url), "utf8");
    assert.doesNotMatch(src, /invoke\s*\(|@tauri-apps|\.run\(|engine|fetch\s*\(|console\./, `${f} has no way to mutate anything`);
  }
});

// ---------------------------------------------------------------- the real Driver.js

test("with the real Driver.js: missing targets do not break it, and teardown leaves no overlay or listeners", async () => {
  const { driver } = await import("driver.js");
  const load = async () => driver;
  const before = document.body.className;
  const store = memoryStore();
  const h = await startDeploymentsTour({ doc: document, store, load, reducedMotion: true }); // an empty page: every anchor is missing
  assert.ok(h?.active);
  assert.ok(document.querySelector(".driver-popover.harpoon-tour"), "the popover renders, centred, for a step without a target");
  assert.equal(document.querySelector(".driver-popover").getAttribute("role"), "dialog");
  assert.ok(document.querySelector(".harpoon-tour-skip"), "skip is always offered");
  h.destroy();
  assert.equal(document.querySelector(".driver-popover"), null);
  assert.equal(document.querySelector(".driver-overlay"), null);
  assert.equal(document.body.className, before, "no leftover body classes");
  assert.equal(readTourStatus(store), null);
});

test("with the real Driver.js: Next walks all six steps and Done is remembered", async () => {
  const { driver } = await import("driver.js");
  const store = memoryStore();
  const host = document.createElement("div");
  host.innerHTML = '<button data-tour="create-project">Create</button><button data-tour="replay-tour">Replay</button>';
  document.body.appendChild(host);
  const h = await startDeploymentsTour({ doc: document, store, load: async () => driver, reducedMotion: true });
  const titles = [];
  for (let i = 0; i < 6; i++) {
    titles.push(document.querySelector(".driver-popover-title")?.textContent);
    document.querySelector(".driver-popover-next-btn").click();
  }
  assert.deepEqual(titles, ["Register an application", "See what is running", "Release history", "Configuration", "Recovery controls", "Come back any time"]);
  assert.equal(readTourStatus(store), "completed");
  assert.ok(!h.active);
  assert.equal(document.querySelector(".driver-popover"), null);
  host.remove();
});

// ---------------------------------------------------------------- appearance

test("the tour is styled with the app's own tokens, and honours reduced motion in CSS too", () => {
  const css = readFileSync(new URL("../styles/tour.css", import.meta.url), "utf8");
  const rules = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const token of ["--panel-elevated", "--border-strong", "--radius-card", "--accent-yellow", "--text-primary", "--text-secondary", "--focus-ring", "--font-sans"]) assert.ok(rules.includes(`var(${token})`), token);
  assert.doesNotMatch(rules.replace(/rgba\(0, 0, 0, 0\.45\)/, ""), /#[0-9a-fA-F]{3,8}\b/, "no hard-coded colours beyond the shadow");
  assert.match(rules, /@media \(prefers-reduced-motion: reduce\)/);
});
