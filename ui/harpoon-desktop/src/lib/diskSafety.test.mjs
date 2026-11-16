// Safety behaviour of the two disk controls that act on the guest, driven by the shared telemetry: the low-disk notice (VM-restarting
// expansion) and "Reclaim unused space" (fstrim). A stale, unavailable or otherwise non-live reading must never enable either, and a
// remembered warning must not disappear just because a refresh failed. The real components run in jsdom; only Tauri's invoke is replaced.
import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const dom = new JSDOM('<!doctype html><html><body></body></html>', { pretendToBeVisual: true, url: "http://localhost/" });
for (const k of ["window", "document", "navigator", "HTMLElement", "KeyboardEvent", "MouseEvent", "Event", "Node"]) {
  Object.defineProperty(globalThis, k, { value: dom.window[k], configurable: true, writable: true });
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const out = new URL("../../node_modules/.cache/disk-safety/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const mock = join(out, "tauri-core.js");
writeFileSync(mock, "export const invoke = (...a) => globalThis.__invoke(...a);");
const outfile = join(out, "safety.mjs");
await build({
  stdin: { resolveDir: new URL("../components/", import.meta.url).pathname, loader: "ts",
    contents: 'export { DiskCapacityNotice } from "./DiskCapacityNotice.tsx"; export { DiskReclaimCard } from "./DiskReclaimCard.tsx"; export { ConfirmDialogHost } from "./ConfirmDialog.tsx"; export { diskTelemetry } from "../hooks/useDiskTelemetry.ts";' },
  outfile, bundle: true, format: "esm", platform: "node", logLevel: "silent", external: ["react", "react-dom"], alias: { "@tauri-apps/api/core": mock }, jsx: "automatic",
});
const React = await import("react");
const { createRoot } = await import("react-dom/client");
const { DiskCapacityNotice, DiskReclaimCard, ConfirmDialogHost, diskTelemetry } = await import(pathToFileURL(outfile).href);
const { act } = React;
const GIB = 1024 ** 3;

const cap = (pressure, o = {}) => ({ provisioned: true, state: "running", logicalBytes: 12 * GIB, physicalBytes: 3 * GIB, hostAvailableBytes: 100 * GIB, pressure,
  suggestedSizes: [16 * GIB, 32 * GIB], maxBytes: 512 * GIB, hostReserveBytes: 5 * GIB, guest: { capacityBytes: 11 * GIB, usedBytes: 10 * GIB, availableBytes: 1 * GIB }, ...o });
let reply = null; const log = [];
globalThis.__invoke = async (cmd) => {
  log.push(cmd);
  if (cmd === "get_disk_capacity") { if (reply && reply.hang) return reply.hang; if (reply instanceof Error) throw reply; return typeof reply === "string" ? reply : JSON.stringify(reply); }
  if (cmd === "reclaim_disk_confirmed") return JSON.stringify({ status: "reclaimed", message: "ok", logicalBytes: 12 * GIB, physicalBeforeBytes: 3 * GIB, physicalAfterBytes: 2 * GIB });
  if (cmd === "expand_disk_confirmed") return "done";
  throw new Error(`unexpected ${cmd}`);
};
const count = (c) => log.filter((x) => x === c).length;

async function mount(enabled = true) {
  const host = document.createElement("div"); document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(React.createElement(React.Fragment, null,
    React.createElement(DiskCapacityNotice, { enabled, stagesShown: false, onExpanded() {} }),
    React.createElement(DiskReclaimCard, { enabled, expansion: null }), React.createElement(ConfirmDialogHost))));
  const btn = (text) => [...host.querySelectorAll("button")].find((b) => b.textContent.startsWith(text));
  const click = (b) => act(async () => { b.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  return { host, btn, click, notice: () => host.querySelector(".disk-notice"), unmount: () => act(async () => { root.unmount(); host.remove(); }) };
}
const refresh = () => act(async () => { await diskTelemetry.refresh(); });
const dialogButton = (text) => [...document.querySelectorAll('[role="dialog"] button')].find((b) => b.textContent.includes(text));

test("nothing read yet: no notice, and reclaim is unavailable", async () => {
  diskTelemetry.setEnabled(true); reply = new Error("no binary");
  const m = await mount(); await refresh();
  assert.equal(m.notice(), null);
  assert.equal(m.btn("Reclaim unused space").disabled, true);
  await m.unmount();
});

test("a live warning offers expansion and reclaim, each only through its confirmation", async () => {
  reply = cap("warning"); log.length = 0;
  const m = await mount(); await refresh();
  assert.match(m.notice().textContent, /Docker disk is running low\./);
  assert.equal(m.btn("Review expansion").disabled, false);
  const reclaim = m.btn("Reclaim unused space");
  assert.equal(reclaim.disabled, false);
  await m.click(reclaim);
  assert.ok(document.querySelector('[role="dialog"]'), "confirmation opens");
  assert.equal(count("reclaim_disk_confirmed"), 0, "nothing ran before confirming");
  await act(async () => { dialogButton("Cancel").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  assert.equal(count("reclaim_disk_confirmed"), 0, "cancelled");
  await m.click(reclaim);
  await act(async () => { dialogButton("Reclaim unused space").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  assert.equal(count("reclaim_disk_confirmed"), 1);
  await m.unmount();
});

test("a failed refresh does not hide the warning, but disables expansion and reclaim and says the figures are old", async () => {
  reply = cap("critical"); await refresh();
  log.length = 0;
  const m = await mount();
  assert.match(m.notice().textContent, /Docker disk is almost full\./);
  reply = new Error("channel down"); await refresh();
  assert.ok(m.notice(), "the warning is still shown");
  assert.match(m.notice().textContent, /was almost full at the last reading/);
  assert.match(m.notice().textContent, /out of date/);
  assert.equal(m.btn("Review expansion").disabled, true);
  const reclaim = m.btn("Reclaim unused space");
  assert.equal(reclaim.disabled, true);
  assert.match(m.host.textContent, /Disk usage is not live/);
  await m.click(reclaim);
  assert.equal(document.querySelector('[role="dialog"]'), null, "a disabled control opens nothing");
  assert.equal(count("reclaim_disk_confirmed") + count("expand_disk_confirmed"), 0);
  await m.unmount();
});

test("expansion cannot be confirmed from a reading that went stale while the choices were open", async () => {
  reply = cap("warning"); await refresh();
  const m = await mount();
  await m.click(m.btn("Review expansion"));
  assert.equal(m.btn("Confirm expansion").disabled, false);
  reply = new Error("channel down"); await refresh();
  assert.equal(m.btn("Confirm expansion").disabled, true);
  reply = cap("warning"); await refresh();
  assert.equal(m.btn("Confirm expansion").disabled, false, "live again");
  await m.unmount();
});

test("unsupported, disconnected and engine-down readings neither show a warning nor enable reclaim", async () => {
  const noGuest = cap("unknown"); delete noGuest.guest;
  for (const [name, r, enabled] of [["unsupported", "garbage", true], ["disconnected", noGuest, true], ["engine-down", cap("critical"), false]]) {
    reply = cap("critical"); diskTelemetry.setEnabled(true); await refresh();
    reply = r; if (enabled) await refresh();
    diskTelemetry.setEnabled(enabled);
    const m = await mount(enabled);
    assert.equal(m.notice(), null, `${name}: no warning from a reading that is not current`);
    assert.equal(m.btn("Reclaim unused space").disabled, true, `${name}: reclaim disabled`);
    await m.unmount();
  }
});

test("a timed-out refresh leaves both actions disabled, a late live reply cannot re-enable them, and the next real refresh does", async () => {
  const realSet = globalThis.setTimeout, realClear = globalThis.clearTimeout; const timers = [];
  reply = cap("critical"); diskTelemetry.setEnabled(true); await refresh();
  const m = await mount();
  assert.equal(m.btn("Reclaim unused space").disabled, false, "live to start with");
  globalThis.setTimeout = (fn, ms) => { timers.push({ fn, ms }); return timers.length; }; globalThis.clearTimeout = () => {};
  try {
    let late; reply = { hang: new Promise((res) => { late = res; }) };
    const pending = act(async () => { const p = diskTelemetry.refresh(); await Promise.resolve(); timers.at(-1).fn(); await p; });
    await pending;
    assert.equal(m.btn("Reclaim unused space").disabled, true, "disabled after the timeout");
    assert.equal(m.btn("Review expansion").disabled, true);
    assert.match(m.notice().textContent, /last reading/);
    log.length = 0;
    await act(async () => { late(JSON.stringify(cap("critical"))); await new Promise((r) => setImmediate(r)); });
    assert.equal(m.btn("Reclaim unused space").disabled, true, "the late reply is discarded: still disabled");
    assert.equal(m.btn("Review expansion").disabled, true);
    reply = cap("critical");
    await act(async () => { await diskTelemetry.refresh(); });
    assert.equal(m.btn("Reclaim unused space").disabled, false, "a refresh that really completes re-enables it");
    assert.equal(m.btn("Review expansion").disabled, false);
  } finally { globalThis.setTimeout = realSet; globalThis.clearTimeout = realClear; await m.unmount(); }
});
