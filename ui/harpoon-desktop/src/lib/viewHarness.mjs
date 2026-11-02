// Test harness: the real Deployments view, rendered in jsdom. esbuild (already installed with Vite) bundles
// DeployView.tsx and React once per test run; styles are dropped. Nothing is mocked except the engine the view is given.
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { buildSync } from "esbuild";
import { JSDOM } from "jsdom";

const here = dirname(fileURLToPath(import.meta.url));

const dom = new JSDOM('<!doctype html><html><body></body></html>', { pretendToBeVisual: true, url: "http://localhost/" });
for (const k of ["window", "document", "navigator", "HTMLElement", "KeyboardEvent", "MouseEvent", "Event", "Node", "getComputedStyle"]) {
  Object.defineProperty(globalThis, k, { value: dom.window[k], configurable: true, writable: true });
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
// React's `act` falls back to a new MessageChannel for every call when it is not loaded as CommonJS, and Node
// keeps each port alive forever. A channel made of setImmediate delivers the same message and holds nothing.
globalThis.MessageChannel = class {
  constructor() {
    const self = this;
    this.port1 = { set onmessage(f) { self.f = f; } };
    this.port2 = { postMessage() { setImmediate(() => self.f && self.f()); } };
  }
};
if (!dom.window.navigator.clipboard) Object.defineProperty(dom.window.navigator, "clipboard", { value: { writeText: async () => {} }, configurable: true });

const out = buildSync({ // synchronous: no long-lived helper process to keep the test run alive
  stdin: {
    contents: `export { DeployView } from "../views/deploy/DeployView.tsx";
export { createRoot } from "react-dom/client";
export { act, createElement } from "react";`,
    resolveDir: here, loader: "ts", sourcefile: "view-entry.ts",
  },
  bundle: true, write: false, format: "esm", platform: "node", jsx: "automatic", logLevel: "silent",
  loader: { ".css": "empty" },
  define: { "process.env.NODE_ENV": '"development"' },
});
const file = join(mkdtempSync(join(tmpdir(), "hv-views-")), "views.mjs");
writeFileSync(file, out.outputFiles[0].text);
export const { DeployView, createRoot, act, createElement: h } = await import(pathToFileURL(file).href);
export const win = dom.window;
/** Call once all tests are done: the window's timers would otherwise keep the process running. */
export const shutdown = () => dom.window.close();

/** Type into a controlled input the way a person does: the native setter, then an input event. */
export async function type(input, value) {
  const set = Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set;
  await act(async () => { set.call(input, value); input.dispatchEvent(new dom.window.Event("input", { bubbles: true })); });
}

/** The text of every element matching `sel` (trimmed, whitespace collapsed). */
export const texts = (root, sel) => [...root.querySelectorAll(sel)].map((e) => e.textContent.replace(/\s+/g, " ").trim());
export const find = (root, sel, text) => [...root.querySelectorAll(sel)].find((e) => (typeof text === "string" ? e.textContent.includes(text) : text.test(e.textContent)));
export const click = (el) => act(async () => { el.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true, cancelable: true })); });

/** Mount the view with a fake engine. `props()` returns the engine pieces for a given state, so tests can re-render with fresh state objects the way polling does. */
export async function mountView(makeEngine, extra = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const calls = [];
  const render = (state, o = {}) => act(async () => root.render(h(DeployView, {
    engine: makeEngine(state, calls), realRuntime: "connected", onStartHarpoon() {}, onOpenContainers() {}, ...extra, ...o,
  })));
  return { host, root, calls, render, unmount: () => act(async () => { root.unmount(); host.remove(); }) };
}

export const fakeEngine = (state, calls, over = {}) => ({
  state, runtime: "connected",
  api: {
    run: async (op) => { calls.push(op); return { ok: true, exitCode: 0, summary: "", detail: "", refusedByGate: false, op: String(op.op) }; },
    refresh() {}, busy: false, last: null, clearLast() {}, notice: [], clearNotice() {}, pickFolder: async () => null,
    stepLogs: async () => [], runtimeLogs: async () => [], ...over,
  },
});
