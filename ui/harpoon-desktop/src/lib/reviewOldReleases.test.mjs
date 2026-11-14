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
  entryPoints: [new URL("../components/DiskReclaimCard.tsx", import.meta.url).pathname], outfile, bundle: true, format: "esm", platform: "node", logLevel: "silent",
  external: ["react", "react-dom"], alias: { "@tauri-apps/api/core": mock }, jsx: "automatic",
});
const React = await import("react");
const { createRoot } = await import("react-dom/client");
const { DockerArtifacts } = await import(pathToFileURL(outfile).href);
const { act } = React;

const item = (o) => "item: " + JSON.stringify({ app: "app_1", appName: "collectiv", environment: "production", deployment: "dep_01ABCDEF", createdMs: 1, status: "Succeeded", commit: "abc1234", kind: "image", name: "harpoon/x:1", verdict: "ready", reason: "superseded", detail: "", physical: true, ...o });
const ok = (lines) => ({ ok: true, summary: "", hint: "", detail: lines.join("\n") });

async function mount(handler) {
  const calls = [];
  globalThis.__invoke = (cmd, args) => { calls.push(args); return handler(args); };
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(React.createElement(DockerArtifacts, { enabled: true, onDone: () => {} })));
  const button = () => [...host.querySelectorAll("button")].find((b) => b.textContent.startsWith("Review old") || b.textContent === "Looking…");
  const click = (b) => act(async () => { b.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  return { host, calls, button, click, dialog: () => document.querySelector('[role="dialog"]'), unmount: () => act(async () => root.unmount()) };
}

test("an empty inventory opens an informative dialog with no removal action", async () => {
  const m = await mount(async () => ok([]));
  await m.click(m.button());
  const d = m.dialog();
  assert.ok(d, "a dialog opens");
  assert.match(d.textContent, /Everything is clean/);
  assert.match(d.textContent, /No obsolete deployment artifacts were found/);
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
    assert.doesNotMatch(m.host.textContent, /Nothing to clean up/);
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
