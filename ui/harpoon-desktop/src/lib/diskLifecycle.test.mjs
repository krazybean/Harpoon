// Timer, subscription and Strict Mode behaviour of the shared disk telemetry hooks, and the elapsed-time label. Fake timers are
// the jsdom window's own setInterval, wrapped here so every interval is counted and can be fired by hand.
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

// the abandon-a-hung-request timer is captured too, and fired by hand
const timeouts = []; globalThis.setTimeout = (fn, ms) => { timeouts.push({ fn, ms }); return timeouts.length; }; globalThis.clearTimeout = () => {};
// every interval the code under test starts is recorded; none reaches a real timer
const live = new Map(); let nextId = 1;
dom.window.setInterval = (fn, ms) => { const id = nextId++; live.set(id, { fn, ms }); return id; };
dom.window.clearInterval = (id) => { live.delete(id); };
const fire = async (ms) => { const act = React.act; await act(async () => { for (const t of [...live.values()]) if (t.ms === ms) await t.fn(); }); };

const out = new URL("../../node_modules/.cache/disk-lifecycle/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const mock = join(out, "tauri-core.js");
writeFileSync(mock, "export const invoke = (...a) => globalThis.__invoke(...a);");
const outfile = join(out, "life.mjs");
await build({
  stdin: { resolveDir: new URL("../components/", import.meta.url).pathname, loader: "ts",
    contents: 'export * from "./DiskMeter.tsx"; export { diskTelemetry, useDiskTelemetryPolling, useDiskView, TICK_MS } from "../hooks/useDiskTelemetry.ts"; export { POLL_MS } from "../lib/diskTelemetry.ts";' },
  outfile, bundle: true, format: "esm", platform: "node", logLevel: "silent", external: ["react", "react-dom"], alias: { "@tauri-apps/api/core": mock }, jsx: "automatic",
});
const React = await import("react");
const { createRoot } = await import("react-dom/client");
const { DiskMeter, ConnectedDiskChip, diskTelemetry, useDiskTelemetryPolling, useDiskView, TICK_MS, POLL_MS } = await import(pathToFileURL(outfile).href);
const { act } = React;
const GIB = 1024 ** 3;
const reading = JSON.stringify({ provisioned: true, state: "running", logicalBytes: 12 * GIB, physicalBytes: 3 * GIB, pressure: "warning", suggestedSizes: [], maxBytes: 0, hostReserveBytes: 0,
  guest: { capacityBytes: 100 * GIB, usedBytes: 85 * GIB, availableBytes: 15 * GIB } });

let requests = 0, behaviour = "ok", endHung = () => {};
globalThis.__invoke = async () => { requests++; if (behaviour === "hang") return new Promise((res) => { endHung = () => res(reading); }); if (behaviour === "fail") throw new Error("down"); return reading; };

// count subscribe/unsubscribe through the store's own methods, before any component renders
let subscribed = 0;
const realSubscribe = diskTelemetry.subscribe;
diskTelemetry.subscribe = (fn) => { subscribed++; const off = realSubscribe(fn); return () => { subscribed--; off(); }; };

function Page({ polling = true }) {
  useDiskTelemetryPolling(polling);
  const view = useDiskView();
  return React.createElement(React.Fragment, null, React.createElement(DiskMeter, { view }), React.createElement(ConnectedDiskChip, { onOpen() {} }));
}
async function mountStrict(el) {
  const host = document.createElement("div"); document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(React.createElement(React.StrictMode, null, el)));
  return { host, unmount: () => act(async () => { root.unmount(); host.remove(); }) };
}

test("Strict Mode: one request, one poll timer per hook, and everything is released on unmount", async () => {
  requests = 0; behaviour = "ok";
  const r = await mountStrict(React.createElement(Page));
  try {
    assert.equal(requests, 1, "the double-run effect shares one in-flight request");
    const poll = [...live.values()].filter((t) => t.ms === POLL_MS);
    assert.equal(poll.length, 1, "exactly one poll timer survives Strict Mode's mount/unmount/mount");
    assert.equal(diskTelemetry.get().enabled, true);
    assert.ok(subscribed >= 2 && subscribed === [...live.values()].filter((t) => t.ms === TICK_MS).length, "each subscribed view has exactly one tick timer");
  } finally { await r.unmount(); }
  assert.equal(live.size, 0, "no timer outlives the page");
  assert.equal(subscribed, 0, "no subscription outlives the page");
  assert.equal(diskTelemetry.get().enabled, false, "nothing polls, so nothing claims to be live");
});

test("the poll fires a refresh each minute while mounted, and re-enabling does not stack timers", async () => {
  requests = 0; behaviour = "ok";
  function Toggle({ on }) { useDiskTelemetryPolling(on); return null; }
  const host = document.createElement("div"); document.body.appendChild(host);
  const root = createRoot(host);
  for (const on of [true, false, true, true]) await act(async () => root.render(React.createElement(Toggle, { on })));
  assert.equal([...live.values()].filter((t) => t.ms === POLL_MS).length, 1);
  const before = requests;
  await fire(POLL_MS);
  assert.equal(requests, before + 1);
  await act(async () => root.unmount());
  assert.equal(live.size, 0);
});

test("the elapsed-time label keeps advancing while mounted, after a failed refresh, and when a refresh never completes", async () => {
  const realNow = Date.now; let clock = 1_000_000; Date.now = () => clock;
  try {
    behaviour = "ok";
    const r = await mountStrict(React.createElement(Page));
    try {
      assert.match(r.host.textContent, /Last successful reading just now/);
      behaviour = "fail";
      await act(async () => { await diskTelemetry.refresh(); });
      clock += 2 * 60_000; await fire(TICK_MS);
      assert.match(r.host.textContent, /Last successful reading 2 min ago/, "advances after a failed refresh");
      assert.match(r.host.textContent, /Stale/);
      behaviour = "hang";
      const pending = diskTelemetry.refresh(); // never answers
      clock += 5 * 60_000; await fire(TICK_MS);
      assert.match(r.host.textContent, /Last successful reading 7 min ago/, "advances while a request is hanging");
      await act(async () => { timeouts.at(-1).fn(); await pending; }); // the abandon timer fires
      assert.match(diskTelemetry.get().failure.message, /timed out/);
      const before = requests;
      behaviour = "ok";
      await act(async () => { await diskTelemetry.refresh(); });
      assert.equal(requests, before, "while the abandoned call still runs, no second one starts");
      assert.match(r.host.textContent, /Last successful reading 7 min ago/);
      await act(async () => { endHung(); await new Promise((res) => setImmediate(res)); }); // the runtime call finally ends
      await act(async () => { await diskTelemetry.refresh(); });
      assert.equal(requests, before + 1);
      assert.match(r.host.textContent, /Last successful reading just now/, "refreshing resumes once the old call ended");
    } finally { await r.unmount(); }
  } finally { Date.now = realNow; }
});

test("a reading that simply stops being refreshed turns stale by age, with no failure reported", async () => {
  const realNow = Date.now; let clock = 2_000_000; Date.now = () => clock;
  try {
    // fresh store state: a successful reading, then silence
    behaviour = "ok";
    const r = await mountStrict(React.createElement(Page));
    try {
      await act(async () => { await diskTelemetry.refresh(); });
      assert.match(r.host.textContent, /Live/);
      clock += 4 * 60_000; await fire(TICK_MS);
      assert.match(r.host.textContent, /Stale/);
      assert.doesNotMatch(r.host.textContent, /Warning: running low/, "no current-health label on an aged reading");
    } finally { await r.unmount(); }
  } finally { Date.now = realNow; }
});
