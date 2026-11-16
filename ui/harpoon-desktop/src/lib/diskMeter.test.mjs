// The real DiskMeter / DiskChip and the shared store hook, rendered by React in jsdom; only Tauri's invoke is replaced.
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

const out = new URL("../../node_modules/.cache/disk-meter/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const mock = join(out, "tauri-core.js");
writeFileSync(mock, "export const invoke = (...a) => globalThis.__invoke(...a);");
const outfile = join(out, "meter.mjs");
await build({
  stdin: { resolveDir: new URL("../components/", import.meta.url).pathname, loader: "ts",
    contents: 'export * from "./DiskMeter.tsx"; export { diskTelemetry, useDiskTelemetryPolling } from "../hooks/useDiskTelemetry.ts";' },
  outfile, bundle: true, format: "esm", platform: "node", logLevel: "silent", external: ["react", "react-dom"], alias: { "@tauri-apps/api/core": mock }, jsx: "automatic",
});
const React = await import("react");
const { createRoot } = await import("react-dom/client");
const { DiskMeter, DiskChip, ConnectedDiskChip, diskTelemetry, useDiskTelemetryPolling } = await import(pathToFileURL(outfile).href);
const { diskView, initialTelemetry } = await import("./diskTelemetry.ts");
const { act } = React;
const GIB = 1024 ** 3;

const cap = (usedPct, pressure, over = {}) => ({
  provisioned: true, state: "running", logicalBytes: 12 * GIB, physicalBytes: 3 * GIB, pressure, suggestedSizes: [], maxBytes: 512 * GIB, hostReserveBytes: 5 * GIB,
  guest: { capacityBytes: 100 * GIB, usedBytes: usedPct * GIB, availableBytes: (100 - usedPct) * GIB }, ...over,
});
const tele = (c, over = {}) => ({ ...initialTelemetry(), enabled: true, last: c ? { cap: c, at: 1000 } : null, ...over });

async function render(el) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(el));
  return { host, unmount: () => act(async () => { root.unmount(); host.remove(); }) };
}

test("the meter shows real utilization at 20, 50, 85 and 95 percent, with the runtime's own classification", async () => {
  for (const [pct, pressure, label] of [[20, "ok", /Healthy/], [50, "ok", /Healthy/], [85, "warning", /Warning/], [95, "critical", /Critical/]]) {
    const v = diskView(tele(cap(pct, pressure)), 1000);
    const r = await render(React.createElement(DiskMeter, { view: v }));
    const m = r.host.querySelector('[role="meter"]');
    assert.equal(m.getAttribute("aria-valuenow"), String(pct));
    assert.equal(m.dataset.pressure, pressure);
    assert.equal(r.host.querySelector(".disk-meter__fill").style.width, `${pct}%`);
    assert.match(r.host.textContent, label);
    assert.match(r.host.textContent, new RegExp(`${pct}%`));
    await r.unmount();
  }
});

test("guest usage and the Mac's sparse image are separate and explained", async () => {
  const r = await render(React.createElement(DiskMeter, { view: diskView(tele(cap(50, "ok")), 1000) }));
  const t = r.host.textContent;
  assert.match(t, /50 GiB used of 100 GiB · 50 GiB free/);
  assert.match(t, /Guest filesystem:/);
  assert.match(t, /3 GiB allocated on this Mac of the 12 GiB disk image/);
  assert.match(t, /sparse file/);
  await r.unmount();
});

test("every unavailable state is distinct and never draws a zero bar", async () => {
  const raw = cap(50, "ok"); delete raw.guest;
  const cases = [
    ["engine-down", tele(cap(50, "ok"), { enabled: false }), /Harpoon stopped/],
    ["disconnected", tele(raw), /Guest not answering/],
    ["unsupported", tele(null, { failure: { kind: "unsupported", message: "x", at: 1 } }), /Unsupported/],
    ["unavailable", tele(null, { failure: { kind: "failed", message: "boom", at: 1 } }), /Unavailable/],
    ["stale", tele(cap(95, "critical"), { failure: { kind: "failed", message: "boom", at: 2 } }), /Stale/],
  ];
  const seen = new Set();
  for (const [status, t, label] of cases) {
    const r = await render(React.createElement(DiskMeter, { view: diskView(t, 1000) }));
    assert.equal(r.host.firstChild.dataset.diskStatus, status);
    assert.match(r.host.textContent, label);
    seen.add(r.host.textContent.match(label)[0]);
    const m = r.host.querySelector('[role="meter"]');
    if (status === "engine-down" || status === "stale") { assert.equal(m.getAttribute("aria-valuenow"), status === "stale" ? "95" : "50"); assert.equal(m.dataset.stale, "true"); assert.match(r.host.textContent, /last figures|out of date/); }
    else assert.equal(m, null, `${status}: no bar`);
    assert.doesNotMatch(r.host.textContent, /Healthy|Warning: running low|Critical: almost full/, `${status}: no current-health label`);
    if (m) assert.equal(m.dataset.pressure, "unknown", `${status}: neutral bar`);
    await r.unmount();
  }
  assert.equal(seen.size, cases.length);
});

test("the chip is a real button, opens Resources, and says stale when it is", async () => {
  let opened = 0;
  const live = await render(React.createElement(DiskChip, { view: diskView(tele(cap(85, "warning")), 1000), onOpen: () => opened++ }));
  const b = live.host.querySelector("button");
  assert.equal(b.tagName, "BUTTON");
  assert.match(b.textContent, /Disk 85%/);
  assert.match(b.getAttribute("aria-label"), /85% used, warning/);
  await act(async () => { b.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  assert.equal(opened, 1);
  await live.unmount();
  const stale = await render(React.createElement(DiskChip, { view: diskView(tele(cap(85, "warning"), { failure: { kind: "failed", message: "x", at: 2 } }), 1000), onOpen() {} }));
  assert.match(stale.host.textContent, /Disk 85% · stale/);
  assert.doesNotMatch(stale.host.querySelector("button").getAttribute("aria-label"), /warning|critical|healthy/i, "a stale chip does not claim current health");
  assert.match(stale.host.querySelector("button").getAttribute("aria-label"), /stale/);
  await stale.unmount();
  for (const t of [tele(null), tele(cap(10, "ok"), { enabled: false })]) {
    const none = await render(React.createElement(DiskChip, { view: diskView(t, 1000), onOpen() {} }));
    assert.equal(none.host.querySelector("button"), null);
    await none.unmount();
  }
});

test("one shared poll feeds the meter and the chip; a failed refresh keeps the figures and marks them stale", async () => {
  let calls = 0; let fail = false;
  globalThis.__invoke = async (cmd) => { assert.equal(cmd, "get_disk_capacity"); calls++; if (fail) throw new Error("channel down"); return JSON.stringify(cap(85, "warning")); };
  function Page() { useDiskTelemetryPolling(true); return React.createElement(React.Fragment, null, React.createElement(ConnectedDiskChip, { onOpen() {} }), React.createElement(ConnectedDiskChip, { onOpen() {} })); }
  const r = await render(React.createElement(Page));
  try {
    assert.equal(calls, 1, "one request for two consumers: the mount refresh");
    assert.deepEqual([...r.host.querySelectorAll("button")].map((b) => b.textContent), ["Disk 85%", "Disk 85%"]);
    fail = true;
    await act(async () => { await diskTelemetry.refresh(); });
    assert.equal(calls, 2);
    assert.deepEqual([...r.host.querySelectorAll("button")].map((b) => b.textContent), ["Disk 85% · stale", "Disk 85% · stale"]);
  } finally { await r.unmount(); }
});
