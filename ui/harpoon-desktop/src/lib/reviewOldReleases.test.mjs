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
  assert.ok([...m.host.querySelectorAll("button")].some((b) => /clean up/i.test(b.textContent)));
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
