// Test harness for the real <App/> and useShelf in jsdom. Only the two Tauri bridges are replaced: `invoke` goes to a test-owned
// table of controllable promises, `listen` records its handlers so a test can emit what the Rust refresh worker would.
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { buildSync } from "esbuild";
import { win, shutdown } from "./viewHarness.mjs";

export { win, shutdown };
const here = dirname(fileURLToPath(import.meta.url));

let seq = 0;
/** One bridge per mounted app, so a timer left behind by an earlier test can only talk to that test's own (dead) bridge. */
export function newBridge() {
  const id = `b${++seq}`;
  const bridge = { id, calls: [], handlers: {}, listeners: {} };
  (globalThis.__bridges ??= {})[id] = bridge;
  bridge.invoke = (cmd, args) => {
    bridge.calls.push(cmd);
    const fn = bridge.handlers[cmd];
    return fn ? Promise.resolve().then(() => fn(args)) : Promise.resolve(DEFAULTS[cmd] ?? null);
  };
  return bridge;
}
const DEFAULTS = {
  get_harpoon_binary_path: "/dev/harpoon", get_docker_info: "ok", get_counts: { running: 1, containers: 10, images: 3, volumes: 2, networks: 2 },
  get_config: { raw: "", cpus: 2, memoryMiB: 4096 }, get_log_path: "/tmp/harpoon.log", get_doctor: { checks: [] }, list_containers: [],
  start_runtime_refresh_worker: 7, ensure_harpoon_context: "ok",
  deploy_snapshot: { schema: 1, generatedMs: 0, runtimeDir: "/own", source: { kind: "own", path: "/own", journalFormat: "none", readOnly: false }, store: "missing", storeError: null, writerHeld: false, gate: { shipping: false, reason: "" }, repos: [], apps: [], definitions: [] },
  external_runtimes: [], host_tcp_listeners: { verified: false, error: null, checkedMs: 0, listeners: [] },
};

const build = (contents, bridge) => {
  const stubDir = mkdtempSync(join(tmpdir(), "tauri-stub-"));
  writeFileSync(join(stubDir, "core.js"), `export const invoke = (cmd, args) => globalThis.__bridges[${JSON.stringify(bridge.id)}].invoke(cmd, args);`);
  writeFileSync(join(stubDir, "event.js"), `const B = () => globalThis.__bridges[${JSON.stringify(bridge.id)}]; export const listen = async (name, fn) => { (B().listeners[name] ??= []).push(fn); return () => { const l = B().listeners[name]; l.splice(l.indexOf(fn) >>> 0, 1); }; };`);
  const out = buildSync({
    stdin: { contents, resolveDir: here, loader: "ts", sourcefile: "entry.ts" },
    bundle: true, write: false, format: "esm", platform: "node", jsx: "automatic", logLevel: "silent",
    loader: { ".css": "empty" }, alias: { "@tauri-apps/api/core": join(stubDir, "core.js"), "@tauri-apps/api/event": join(stubDir, "event.js") }, define: { "process.env.NODE_ENV": '"development"', "import.meta.env.DEV": "true" },
  });
  const file = join(mkdtempSync(join(tmpdir(), "status-")), "bundle.mjs");
  writeFileSync(file, out.outputFiles[0].text);
  return import(pathToFileURL(file).href);
};
export const loadShelf = () => build(`export { useShelf } from "../hooks/useShelf.ts"; export { createRoot } from "react-dom/client"; export { act, createElement } from "react";`, newBridge());
export const loadApp = (bridge) => build(`export { default as App } from "../App.tsx"; export { createRoot } from "react-dom/client"; export { act, createElement } from "react";`, bridge);

export const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
/** Let pending promise continuations and React renders run, until `cond` holds (a condition, not a delay). */
export async function until(act_, cond, what) {
  for (let i = 0; i < 600; i++) {
    if (cond()) return;
    await act_(async () => { await new Promise((r) => setTimeout(r, 10)); }); // the app's own short startup timers need real time to pass
  }
  throw new Error(`timed out waiting for: ${what}`);
}
