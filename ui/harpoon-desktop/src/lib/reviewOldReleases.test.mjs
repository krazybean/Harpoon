// The real DockerArtifacts component (bundled with esbuild, rendered by React in jsdom); only Tauri's invoke is replaced.
import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const dom = new JSDOM('<!doctype html><html><body></body></html>', { pretendToBeVisual: true });
for (const k of ["window", "document", "navigator", "HTMLElement", "KeyboardEvent", "MouseEvent", "Event", "Node"]) {
  Object.defineProperty(globalThis, k, { value: dom.window[k], configurable: true, writable: true });
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

// inside node_modules so the bundle resolves the same react as this test
const out = new URL("../../node_modules/.cache/review-old/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const mock = join(out, "tauri-core.js");
writeFileSync(mock, "export const invoke = (...a) => globalThis.__invoke(...a);");
const outfile = join(out, "card.mjs");
await build({
  stdin: { resolveDir: new URL("../components/", import.meta.url).pathname, loader: "ts", contents: 'export * from "./DiskReclaimCard.tsx"; export { ConfirmDialogHost } from "./ConfirmDialog.tsx";' }, outfile, bundle: true, format: "esm", platform: "node", logLevel: "silent",
  external: ["react", "react-dom"], alias: { "@tauri-apps/api/core": mock }, jsx: "automatic",
});
const React = await import("react");
const { createRoot } = await import("react-dom/client");
const { DockerArtifacts, ConfirmDialogHost } = await import(pathToFileURL(outfile).href);
const { act } = React;

const item = (o) => "item: " + JSON.stringify({ app: "app_1", appName: "collectiv", environment: "production", deployment: "dep_01ABCDEF", createdMs: 1, status: "Succeeded", commit: "abc1234", kind: "image", name: "harpoon/x:1", verdict: "ready", reason: "superseded", detail: "", physical: true, ...o });
const ok = (lines) => ({ ok: true, summary: "", hint: "", detail: lines.join("\n") });

async function mount(handler) {
  const calls = [];
  globalThis.__invoke = (cmd, args) => { if (cmd !== "deploy_op") return Promise.reject(new Error(`no ${cmd} in this test`)); calls.push(args); return handler(args); }; // only the engine operations are recorded
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(React.createElement(React.Fragment, null, React.createElement(DockerArtifacts, { enabled: true, onDone: () => {} }), React.createElement(ConfirmDialogHost))));
  const button = () => [...host.querySelectorAll("button")].find((b) => b.textContent.startsWith("Review old") || b.textContent === "Looking…");
  const click = (b) => act(async () => { b.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  return { host, calls, button, click, dialog: () => document.querySelector('[role="dialog"]'), unmount: async () => {
    // a confirmation left open holds the page's one-cleanup-at-a-time lock until it is answered, so answer it before leaving
    const cancel = [...document.querySelectorAll('[role="dialog"] button')].find((b) => b.textContent === "Cancel");
    if (cancel) await act(async () => { cancel.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
    await act(async () => root.unmount());
  } };
}

test("an empty inventory opens an informative dialog with no removal action", async () => {
  const m = await mount(async () => ok([]));
  await m.click(m.button());
  const d = m.dialog();
  assert.ok(d, "a dialog opens");
  assert.match(d.textContent, /No obsolete deployment artifacts found/);
  assert.match(d.textContent, /No obsolete Harpoon deployment artifacts were found/);
  assert.doesNotMatch(d.textContent, /Everything is clean/);
  assert.match(d.textContent, /Other categories, such as build cache|other categories, such as build cache/i);
  assert.match(d.textContent, /Running releases and persistent volumes remain protected/);
  assert.deepEqual([...d.querySelectorAll("button")].map((b) => b.textContent), ["Close"]);
  assert.equal(document.activeElement, d.querySelector("button"));
  assert.ok(![...m.host.querySelectorAll("button")].some((b) => /clean up/i.test(b.textContent)), "no removal button on the page");
  assert.ok(m.calls.every((a) => a.op.apply === false), "only the read-only listing ran");
  await m.unmount();
});

test("the empty-state dialog closes with Close, Escape and the backdrop", async () => {
  const m = await mount(async () => ok([]));
  for (const how of ["close", "escape", "scrim"]) {
    await m.click(m.button());
    assert.ok(m.dialog(), how);
    if (how === "close") await m.click(m.dialog().querySelector("button"));
    if (how === "escape") await act(async () => { m.dialog().dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true })); });
    if (how === "scrim") await m.click(document.querySelector(".dialog-scrim"));
    assert.equal(m.dialog(), null, `${how} closes it`);
  }
  await m.unmount();
});

test("a nonempty inventory shows the grouped review, not the empty dialog", async () => {
  const m = await mount(async () => ok([item({}), "reclaimed-containers: 0", "reclaimed-images: 0"]));
  await m.click(m.button());
  assert.equal(m.dialog(), null);
  assert.match(m.host.textContent, /collectiv/);
  assert.match(m.host.textContent, /1 Docker object to remove/);
  assert.ok([...m.host.querySelectorAll("button")].some((b) => b.textContent === "Remove 1 Docker artifact"));
  await m.unmount();
});

test("items that only need review are listed, not called clean", async () => {
  const m = await mount(async () => ok([item({ verdict: "review", reason: "needs-review" })]));
  await m.click(m.button());
  assert.equal(m.dialog(), null);
  assert.match(m.host.textContent, /1 need review/);
  await m.unmount();
});

test("loading is visible while the engine answers", async () => {
  let release;
  const m = await mount(() => new Promise((r) => { release = r; }));
  await m.click(m.button());
  assert.match(m.host.textContent, /Looking for old releases/);
  assert.equal(m.button().disabled, true);
  await act(async () => release(ok([])));
  assert.doesNotMatch(m.host.textContent, /Looking for old releases/);
  assert.ok(m.dialog());
  await m.unmount();
});

test("a failed request shows an error, never an empty inventory", async () => {
  for (const handler of [async () => { throw new Error("engine unreachable"); }, async () => ({ ok: false, summary: "", hint: "Start the engine, then try again.", detail: "" })]) {
    const m = await mount(handler);
    await m.click(m.button());
    assert.equal(m.dialog(), null);
    assert.match(m.host.querySelector('[role="alert"]').textContent, /engine unreachable|Start the engine/);
    assert.doesNotMatch(m.host.textContent, /No obsolete/);
    assert.equal(m.button().disabled, false, "can retry");
    await m.unmount();
  }
});

test("repeated clicks send one request and open one dialog", async () => {
  let release;
  const m = await mount(() => new Promise((r) => { release = r; }));
  const b = m.button();
  await act(async () => { for (let i = 0; i < 3; i++) b.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  assert.equal(m.calls.length, 1);
  await act(async () => release(ok([])));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  await m.unmount();
});

// ---- summary, collapsible groups, action labels ----
const items = (spec) => spec.flatMap(([app, name, verdict, kind, n], g) => Array.from({ length: n }, (_, k) =>
  item({ app, appName: name, verdict, kind, name: `${kind}-${g}-${k}`, deployment: `dep_${g}${k}`, reason: verdict === "reconcile" ? "already-absent" : verdict === "protected" ? "serving-release" : "superseded", physical: verdict === "ready" })));
const lines = (spec) => ok(items(spec));
const group = (m, name) => [...m.host.querySelectorAll("section")].find((s) => s.getAttribute("aria-label") === name);
const toggle = (m, name) => group(m, name).querySelector("button[aria-expanded]");
const COLLECTIV = [["a1", "Collectiv", "reconcile", "container", 16], ["a1", "Collectiv", "protected", "image", 2], ["a2", "juancastro.me", "protected", "image", 2]];

test("the summary counts every item once as eligible, protected or review", async () => {
  const m = await mount(async () => lines([...COLLECTIV, ["a3", "Other", "review", "image", 1]]));
  await m.click(m.button());
  const t = m.host.textContent;
  assert.match(t, /Total artifacts21/);
  assert.match(t, /Eligible for cleanup16/);
  assert.match(t, /Protected4/);
  assert.match(t, /0 Docker objects to remove · 16 journal records to reconcile · 1 need review/);
  assert.match(t, /frees no Docker disk space/);
  await m.unmount();
});

test("groups start collapsed, show counts only, and expand and collapse by click", async () => {
  const m = await mount(async () => lines(COLLECTIV));
  await m.click(m.button());
  assert.equal(toggle(m, "Collectiv").getAttribute("aria-expanded"), "false");
  assert.match(group(m, "Collectiv").textContent, /16 eligible for reconciliation · 2 protected/);
  assert.match(group(m, "juancastro.me").textContent, /0 eligible · 2 protected/);
  assert.equal(m.host.querySelectorAll("li").length, 0, "no artifact rows while collapsed");
  assert.doesNotMatch(m.host.textContent, /dep…|sha256|dep_/);
  await m.click(toggle(m, "juancastro.me"));
  assert.equal(toggle(m, "juancastro.me").getAttribute("aria-expanded"), "true");
  assert.equal(group(m, "juancastro.me").querySelectorAll("li").length, 2);
  assert.ok(group(m, "juancastro.me").querySelector("details summary"), "technical details are kept per row");
  assert.equal(document.getElementById(toggle(m, "juancastro.me").getAttribute("aria-controls")) !== null, true);
  await m.click(toggle(m, "juancastro.me"));
  assert.equal(group(m, "juancastro.me").querySelectorAll("li").length, 0);
  await m.unmount();
});

test("Expand all and Collapse all", async () => {
  const m = await mount(async () => lines(COLLECTIV));
  await m.click(m.button());
  const btn = (t) => [...m.host.querySelectorAll("button")].find((b) => b.textContent === t);
  await m.click(btn("Expand all"));
  assert.equal(m.host.querySelectorAll("li").length, 20);
  await m.click(btn("Collapse all"));
  assert.equal(m.host.querySelectorAll("li").length, 0);
  await m.unmount();
});

test("a group header is a real button, so keyboard activation and focus come from the browser", async () => {
  const m = await mount(async () => lines(COLLECTIV));
  await m.click(m.button());
  const b = toggle(m, "Collectiv");
  assert.equal(b.tagName, "BUTTON");
  assert.equal(b.type, "button");
  assert.equal(b.tabIndex, 0);
  await m.unmount();
});

test("expansion survives a refresh and a refresh claims no cleanup happened", async () => {
  let n = 0;
  const m = await mount(async () => lines(n++ === 0 ? COLLECTIV : [...COLLECTIV, ["a1", "Collectiv", "ready", "image", 1]]));
  await m.click(m.button());
  await m.click(toggle(m, "juancastro.me"));
  await m.click(m.button());
  assert.equal(toggle(m, "juancastro.me").getAttribute("aria-expanded"), "true");
  assert.equal(toggle(m, "Collectiv").getAttribute("aria-expanded"), "false");
  assert.doesNotMatch(m.host.textContent, /Removed \d+ container/);
  await m.unmount();
});

test("an artifact with no known owner is listed under Unassociated artifacts", async () => {
  const m = await mount(async () => lines([["a1", "Collectiv", "protected", "image", 1], ["zz", null, "reconcile", "port-lease", 2]]));
  await m.click(m.button());
  assert.match(group(m, "Unassociated artifacts").textContent, /2 eligible for reconciliation/);
  await m.click(toggle(m, "Unassociated artifacts"));
  assert.match(group(m, "Unassociated artifacts").textContent, /none is guessed/);
  await m.unmount();
});

test("the primary action says what it will do", async () => {
  const label = async (spec) => {
    const m = await mount(async () => lines(spec));
    await m.click(m.button());
    const b = [...m.host.querySelectorAll(".dialog-actions button")].map((x) => x.textContent).filter((t) => !/Review old|Looking|Expand|Collapse|build cache/i.test(t));
    await m.unmount();
    return b;
  };
  assert.deepEqual(await label([["a", "A", "reconcile", "container", 16]]), ["Reconcile 16 records"]);
  assert.deepEqual(await label([["a", "A", "reconcile", "container", 1]]), ["Reconcile 1 record"]);
  assert.deepEqual(await label([["a", "A", "ready", "image", 3]]), ["Remove 3 Docker artifacts"]);
  assert.deepEqual(await label([["a", "A", "ready", "image", 3], ["a", "A", "reconcile", "container", 16]]), ["Review cleanup (19 items)"]);
  assert.deepEqual(await label([["a", "A", "protected", "image", 2], ["a", "A", "review", "image", 1]]), [], "no enabled destructive action");
});

test("viewing, expanding and toggling details never invokes a cleanup", async () => {
  const m = await mount(async () => lines([...COLLECTIV, ["a1", "Collectiv", "ready", "image", 1]]));
  await m.click(m.button());
  await m.click(toggle(m, "Collectiv"));
  await m.click(m.host.querySelector("details summary"));
  await m.click([...m.host.querySelectorAll("button")].find((b) => b.textContent === "Expand all") ?? toggle(m, "Collectiv"));
  assert.ok(m.calls.every((a) => a.op.apply === false));
  assert.equal(m.calls.length, 1);
  await m.unmount();
});

// ---- the confirmation dialog ----
const ready = (app, name, kind, n) => [app, name, "ready", kind, n];
const confirmOf = async (spec, answer) => {
  const m = await mount(async (a) => (a.op.apply ? { ok: true, summary: "", hint: "", detail: "reclaimed-containers: 0\nreclaimed-images: 0" } : lines(spec)));
  await m.click(m.button());
  const primary = [...m.host.querySelectorAll(".dialog-actions button")].find((b) => /^(Reconcile|Remove|Review cleanup)/.test(b.textContent));
  await m.click(primary);
  return m;
};
const dlg = () => document.querySelector('[role="dialog"]');
const tile = (label) => [...dlg().querySelectorAll(".artifact-metric")].find((t) => t.textContent.includes(label));

test("journal-only confirmation: 16 records, no Docker deletion, no rollback warning, not red", async () => {
  const m = await confirmOf([["a1", "Collectiv", "reconcile", "container", 12], ["a1", "Collectiv", "reconcile", "port-lease", 3], ["a1", "Collectiv", "reconcile", "image", 1], ["a1", "Collectiv", "protected", "image", 2], ["a2", "juancastro.me", "protected", "image", 2]]);
  const d = dlg();
  assert.match(d.querySelector("#confirm-action-title").textContent, /Reconcile 16 records/);
  assert.match(tile("Journal records to reconcile").textContent, /^16/);
  assert.match(tile("Docker containers to remove").textContent, /^0/);
  assert.match(tile("Docker images to remove").textContent, /^0/);
  assert.match(tile("Protected resources").textContent, /^4/);
  assert.match(d.textContent, /No Docker objects will be deleted/);
  assert.match(d.textContent, /frees no Docker disk space/);
  assert.doesNotMatch(d.textContent, /rolled back|Image sizes are shared/);
  assert.ok(!d.querySelector(".artifact-metric--remove"), "no red tile");
  assert.ok(!d.querySelector(".btn--destructive"), "confirm button is not red");
  assert.deepEqual([...d.querySelectorAll(".cleanup-impact__apps li")].map((l) => l.textContent), ["Collectiv · Production"], "protected-only application is not affected");
  assert.match(d.textContent, /Stale port reservations3/);
  assert.match(d.textContent, /References to already-removed containers12/);
  assert.match(d.textContent, /References to already-removed images1/);
  assert.match(d.textContent, /Persistent volumes are never touched/);
  assert.match(d.textContent, /Deployment history stays/);
  await m.unmount();
});

test("Docker removal confirmations name the objects, the rollback impact and a red action", async () => {
  for (const [spec, label, containers, images] of [[[ready("a", "A", "container", 2)], "Remove 2 Docker artifacts", 2, 0], [[ready("a", "A", "image", 1)], "Remove 1 Docker artifact", 0, 1]]) {
    const m = await confirmOf(spec);
    const d = dlg();
    assert.match(d.querySelector("#confirm-action-title").textContent, new RegExp(label));
    assert.match(tile("Docker containers to remove").textContent, new RegExp(`^${containers}`));
    assert.match(tile("Docker images to remove").textContent, new RegExp(`^${images}`));
    assert.match(d.textContent, /will be deleted/);
    assert.match(d.textContent, /cannot be predicted/);
    assert.equal(/rolled back/.test(d.textContent), images > 0, "rollback note only when an image is deleted");
    assert.ok(d.querySelector(".btn--destructive"));
    assert.ok(d.querySelector(".artifact-metric--remove"));
    await m.unmount();
  }
});

test("a mixed operation shows both counts separately and an explicit label", async () => {
  const m = await confirmOf([ready("a", "A", "image", 3), ["a", "A", "reconcile", "container", 5], ["b", "B", "reconcile", "port-lease", 1], ["c", "C", "protected", "image", 1]]);
  const d = dlg();
  assert.match(d.querySelector("#confirm-action-title").textContent, /Remove 3 Docker objects and reconcile 6 records/);
  assert.match(tile("Journal records to reconcile").textContent, /^6/);
  assert.match(tile("Docker images to remove").textContent, /^3/);
  assert.deepEqual([...d.querySelectorAll(".cleanup-impact__apps li")].map((l) => l.textContent.split(" ·")[0]), ["A", "B"], "multiple affected applications, no protected-only one");
  await m.unmount();
});

test("affected records are behind a disclosure; opening it, Cancel, Escape and the backdrop run nothing", async () => {
  for (const how of ["cancel", "escape", "scrim"]) {
    const m = await confirmOf([["a", "A", "reconcile", "container", 2]]);
    const d = dlg();
    const details = d.querySelector("details");
    assert.equal(details.open, false);
    details.open = true;
    assert.match(details.textContent, /dep_/);
    assert.equal(document.activeElement.textContent, "Cancel", "focus starts on the safe button");
    if (how === "cancel") await m.click([...d.querySelectorAll("button")].find((b) => b.textContent === "Cancel"));
    if (how === "escape") await act(async () => { d.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true })); });
    if (how === "scrim") await m.click(document.querySelector(".dialog-scrim"));
    assert.equal(dlg(), null, how);
    assert.ok(m.calls.every((a) => a.op.apply === false), `${how}: no cleanup ran`);
    await m.unmount();
  }
});

test("only the confirm button runs the cleanup, once, with the live-changes confirmation", async () => {
  const m = await confirmOf([["a", "A", "reconcile", "container", 2]]);
  assert.equal(m.calls.length, 1, "opening the dialog ran nothing");
  await m.click([...dlg().querySelectorAll("button")].find((b) => b.textContent === "Reconcile 2 records"));
  assert.equal(m.calls.length, 2);
  assert.deepEqual(m.calls[1], { op: { op: "cleanup-releases", apply: true }, confirmation: { liveChanges: true, cutover: false } });
  await m.unmount();
});
