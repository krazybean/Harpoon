// The real StorageBreakdown (and the Resources page around it) rendered by React in jsdom, with Tauri's invoke replaced by a recorder.
// The inventory text is the committed fixture the Rust module's own test keeps in step with what the runtime writes.
import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const dom = new JSDOM('<!doctype html><html><body></body></html>', { pretendToBeVisual: true });
for (const k of ["window", "document", "navigator", "HTMLElement", "KeyboardEvent", "MouseEvent", "Event", "Node"]) {
  Object.defineProperty(globalThis, k, { value: dom.window[k], configurable: true, writable: true });
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const out = new URL("../../node_modules/.cache/storage-breakdown/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const mock = join(out, "tauri-core.js");
writeFileSync(mock, "export const invoke = (...a) => globalThis.__invoke(...a);");
const outfile = join(out, "breakdown.mjs");
await build({
  stdin: { resolveDir: new URL("../components/", import.meta.url).pathname, loader: "ts",
    contents: 'export * from "./StorageBreakdown.tsx"; export { ResourcesView } from "../views/ResourcesView.tsx"; export { storageInventory } from "../hooks/useStorageInventory.ts"; export { diskTelemetry } from "../hooks/useDiskTelemetry.ts";' },
  outfile, bundle: true, format: "esm", platform: "node", logLevel: "silent", external: ["react", "react-dom"], alias: { "@tauri-apps/api/core": mock }, jsx: "automatic", loader: { ".css": "empty" },
});
const React = await import("react");
const { createRoot } = await import("react-dom/client");
const { StorageBreakdown, ResourcesView, storageInventory, diskTelemetry } = await import(pathToFileURL(outfile).href);
const { inventoryView, createInventoryStore, initialInventory } = await import("./storageInventory.ts");
const { diskView } = await import("./diskTelemetry.ts");
const { act } = React;

const FIXTURE = readFileSync(new URL("./fixtures/storage-inventory.json", import.meta.url), "utf8");
const MIB = 1024 ** 2, GIB = 1024 ** 3;
const view = (over = {}) => ({ status: "current", inv: JSON.parse(FIXTURE), at: 1000, loading: false, note: "", now: 1000, ...over });
const flush = () => act(async () => { await new Promise((r) => setImmediate(r)); });

async function render(el) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(el));
  return { host, unmount: () => act(async () => { root.unmount(); host.remove(); }) };
}
const click = (el) => act(async () => { el.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true, cancelable: true })); });
const rowBtn = (host, title) => [...host.querySelectorAll(".storage-row__head")].find((b) => b.textContent.includes(title));
const text = (host) => host.textContent.replace(/\s+/g, " ");

test("rows appear in the product order, each with its own sizing statement and no shared total", async () => {
  const r = await render(React.createElement(StorageBreakdown, { view: view(), onRefresh() {} }));
  const titles = [...r.host.querySelectorAll(".storage-row__title")].map((e) => e.textContent);
  assert.deepEqual(titles, ["Deployment images", "Other images", "Build cache", "Persistent volumes", "Container writable layers", "Obsolete deployment artifacts", "Other / unattributed usage"]);
  const fig = (t) => rowBtn(r.host, t).querySelector(".storage-row__figure").textContent;
  assert.equal(fig("Deployment images"), "Sizes overlap, not totalled");
  assert.equal(fig("Other images"), "Sizes overlap, not totalled");
  assert.equal(fig("Obsolete deployment"), "Sizes overlap, not totalled");
  assert.equal(fig("Persistent volumes"), "at least 1000 MiB (1 not measured)");
  assert.equal(fig("Other / unattributed"), "Not reliably quantifiable");
  assert.doesNotMatch(r.host.textContent, /%/, "no percentage is derived from overlapping measurements");
  assert.doesNotMatch(text(r.host), /\btotal(?!led)\b.*\d+ ?GiB/i);
  await r.unmount();
});

test("category sizes that add up to more than the guest's used space are shown as they are, never normalised", async () => {
  const guestUsed = 1 * GIB;
  const cap = { provisioned: true, state: "running", logicalBytes: 12 * GIB, physicalBytes: 3 * GIB, pressure: "ok", suggestedSizes: [], maxBytes: 512 * GIB, hostReserveBytes: 5 * GIB,
    guest: { capacityBytes: 4 * GIB, usedBytes: guestUsed, availableBytes: 3 * GIB } };
  const inv = JSON.parse(FIXTURE);
  const reported = inv.categories.filter((c) => c.bytes !== null).reduce((a, c) => a + c.bytes, 0);
  assert.ok(reported > guestUsed, `fixture precondition: reported ${reported} > guest used ${guestUsed}`);
  diskTelemetry.setEnabled(true);
  globalThis.__invoke = async (cmd) => (cmd === "get_disk_capacity" ? JSON.stringify(cap) : cmd === "storage_inventory" ? FIXTURE : Promise.reject(new Error(`unexpected ${cmd}`)));
  await diskTelemetry.refresh();
  const r = await render(React.createElement(ResourcesView, { running: true, config: null, configAt: null, status: null, binaryPath: "", actionInProgress: null, onSetCpus() {}, onSetMemory() {} }));
  await flush();
  const meter = r.host.querySelector('[role="meter"]');
  assert.equal(meter.getAttribute("aria-valuenow"), "25", "the guest meter is 1 of 4 GiB, whatever Docker's categories say");
  const bd = r.host.querySelector("[data-storage-status]");
  assert.doesNotMatch(bd.textContent, /\d+%/);
  assert.equal(bd.querySelectorAll('[role="meter"]').length, 0, "the breakdown has no meter that could read as a share of the disk");
  assert.match(text(bd), /not shares of the disk/);
  await r.unmount();
  diskTelemetry.setEnabled(false);
});

test("a row is a real button: toggles aria-expanded, names its region, and the region holds the accounting and the items", async () => {
  const r = await render(React.createElement(StorageBreakdown, { view: view(), onRefresh() {} }));
  const b = rowBtn(r.host, "Persistent volumes");
  assert.equal(b.tagName, "BUTTON"); assert.equal(b.getAttribute("type"), "button"); assert.equal(b.getAttribute("aria-expanded"), "false");
  assert.equal(r.host.querySelector(`[id="${b.getAttribute("aria-controls")}"]`), null, "collapsed: nothing rendered");
  await click(b);
  assert.equal(b.getAttribute("aria-expanded"), "true");
  const region = r.host.querySelector(`[id="${b.getAttribute("aria-controls")}"]`);
  assert.ok(region); assert.equal(region.getAttribute("role"), "group");
  assert.match(region.textContent, /Docker measured for each volume/);
  assert.match(region.textContent, /pgdata/);
  await click(b);
  assert.equal(b.getAttribute("aria-expanded"), "false");
  await r.unmount();
});

test("the PostgreSQL volume is shown as persistent data with its running reference and no ownership taken from its name", async () => {
  const r = await render(React.createElement(StorageBreakdown, { view: view(), onRefresh() {} }));
  await click(rowBtn(r.host, "Persistent volumes"));
  const head = [...r.host.querySelectorAll(".storage-item__head")].find((b) => b.textContent.includes("pgdata"));
  assert.match(head.textContent, /Protected: persistent data/);
  assert.match(head.textContent, /Unknown ownership/);
  assert.equal(head.getAttribute("aria-expanded"), "false");
  await click(head);
  const body = r.host.querySelector(`[id="${head.getAttribute("aria-controls")}"]`);
  assert.match(body.textContent, /db \(running\)/);
  assert.match(body.textContent, /Persistent data\. A Docker volume holds data that outlives containers/);
  assert.match(body.textContent, /Ownership is not inferred from the name/);
  await r.unmount();
});

test("deployment images show serving protection and journal evidence; unknown images are inspect-only and never called removable", async () => {
  const r = await render(React.createElement(StorageBreakdown, { view: view(), onRefresh() {} }));
  await click(rowBtn(r.host, "Deployment images"));
  const web2 = [...r.host.querySelectorAll(".storage-item__head")].find((b) => b.textContent.includes("web:2"));
  assert.match(web2.textContent, /Protected: serving now/);
  await click(web2);
  assert.match(r.host.textContent, /the one an application serves now/);
  await click(rowBtn(r.host, "Other images"));
  const scratch = [...r.host.querySelectorAll(".storage-item__head")].find((b) => b.textContent.includes("scratch:1"));
  assert.match(scratch.textContent, /Inspect only/); assert.match(scratch.textContent, /Unknown ownership/);
  await click(scratch);
  assert.match(r.host.textContent, /Untracked does not mean orphaned/);
  assert.doesNotMatch(r.host.textContent, /\b(is|are) (safe|safely)|safely removable|orphaned image/i);
  await r.unmount();
});

test("inspection offers no removal, pruning, reclaim or expansion, and never invokes one", async () => {
  const calls = [];
  globalThis.__invoke = async (cmd) => { calls.push(cmd); return FIXTURE; };
  const r = await render(React.createElement(StorageBreakdown, { view: view(), onRefresh() { calls.push("refresh"); } }));
  for (let pass = 0; pass < 3; pass++) {
    for (const b of [...r.host.querySelectorAll("button")].filter((b) => b.getAttribute("aria-expanded") === "false")) await click(b);
  }
  const labels = [...r.host.querySelectorAll("button")].map((b) => b.textContent).filter((t) => !/Refresh/.test(t));
  assert.ok(labels.length > 10, "everything expanded");
  assert.deepEqual([...r.host.querySelectorAll("button")].filter((b) => /remove|delete|prune|clean|reclaim|expand|free up/i.test(b.textContent + b.getAttribute("aria-label"))).map((b) => b.textContent), []);
  assert.deepEqual(calls, [], "expanding every row and item invoked nothing");
  assert.doesNotMatch(r.host.textContent, /recover(s|ed)? (the )?(entire|full|all)|will free/i);
  await r.unmount();
});

test("every non-current state says what is wrong and keeps the page usable", async () => {
  const cases = [
    [{ status: "loading", inv: null, at: null, loading: true, note: "Reading Docker storage…" }, /Reading Docker storage/],
    [{ status: "engine-down", inv: null, at: null, note: "Harpoon is not running, so Docker storage cannot be read." }, /not running/],
    [{ status: "unavailable", inv: null, at: null, note: "Storage could not be read: no docker socket." }, /no docker socket/],
    [{ status: "unsupported", inv: null, at: null, note: "This runtime reports storage in a form this app does not understand." }, /does not understand/],
    [{ status: "stale", note: "The last refresh failed: timed out. These figures are from before." }, /from before/],
  ];
  for (const [over, re] of cases) {
    const r = await render(React.createElement(StorageBreakdown, { view: view(over), onRefresh() {} }));
    assert.match(r.host.querySelector('[role="status"]').textContent, re);
    const refresh = [...r.host.querySelectorAll("button")].find((b) => /Refresh|Reading/.test(b.textContent));
    assert.equal(refresh.disabled, over.status === "engine-down" || over.status === "loading", over.status);
    if (over.status === "stale") { assert.match(r.host.textContent, /Stale/); assert.ok(rowBtn(r.host, "Persistent volumes"), "old figures are still inspectable"); }
    await r.unmount();
  }
});

test("empty, partial and unavailable categories are explicit, not zero", async () => {
  const inv = JSON.parse(FIXTURE);
  inv.categories.find((c) => c.id === "build-cache").status = "unavailable";
  inv.categories.find((c) => c.id === "build-cache").note = "This Docker does not report build cache in /system/df.";
  inv.categories.find((c) => c.id === "build-cache").bytes = null;
  inv.categories.find((c) => c.id === "volumes").items = []; inv.categories.find((c) => c.id === "volumes").count = 0; inv.categories.find((c) => c.id === "volumes").bytes = 0; inv.categories.find((c) => c.id === "volumes").bytesComplete = true;
  inv.sources[1] = { endpoint: "/containers/json?all=true", status: "oversized", detail: "The answer is larger than the 4194304-byte limit and was not read, so none of it is shown." };
  inv.journalAvailable = false;
  const r = await render(React.createElement(StorageBreakdown, { view: view({ inv }), onRefresh() {} }));
  assert.equal(rowBtn(r.host, "Build cache").querySelector(".storage-row__figure").textContent, "Not available");
  assert.match(rowBtn(r.host, "Build cache").textContent, /Unavailable/);
  assert.match(text(r.host), /larger than the 4194304-byte limit/);
  assert.match(text(r.host), /journal could not be read, so no ownership is confirmed/);
  await click(rowBtn(r.host, "Persistent volumes"));
  assert.match(r.host.textContent, /Nothing here/);
  await r.unmount();
});

test("an inventory failure leaves the Docker disk meter exactly as it was", async () => {
  const cap = { provisioned: true, state: "running", logicalBytes: 12 * GIB, physicalBytes: 3 * GIB, pressure: "warning", suggestedSizes: [], maxBytes: 512 * GIB, hostReserveBytes: 5 * GIB,
    guest: { capacityBytes: 10 * GIB, usedBytes: 8.5 * GIB, availableBytes: 1.5 * GIB } };
  diskTelemetry.setEnabled(true);
  globalThis.__invoke = async (cmd) => { if (cmd === "get_disk_capacity") return JSON.stringify(cap); throw new Error("Docker did not answer in time"); };
  await diskTelemetry.refresh();
  const props = { running: true, config: null, configAt: null, status: null, binaryPath: "", actionInProgress: null, onSetCpus() {}, onSetMemory() {} };
  const r = await render(React.createElement(ResourcesView, props));
  await flush();
  // the store is shared by the file's tests, so a prior good inventory makes this "stale" rather than "unavailable": both say why
  assert.match(r.host.querySelector("[data-storage-status]").dataset.storageStatus, /^(stale|unavailable)$/);
  assert.match(r.host.querySelector("[data-storage-status]").textContent, /did not answer in time/);
  const m = r.host.querySelector('[role="meter"]');
  assert.equal(m.getAttribute("aria-valuenow"), "85"); assert.equal(m.dataset.pressure, "warning");
  assert.equal(diskView(diskTelemetry.get(), Date.now()).status, "live");
  await r.unmount();
  diskTelemetry.setEnabled(false);
});

test("opening Resources reads once, Refresh reads again, rapid clicks share one request, and nothing polls", async () => {
  let n = 0; const pend = [];
  globalThis.__invoke = (cmd) => { if (cmd === "get_disk_capacity") return Promise.resolve("{}"); n++; return new Promise((res) => pend.push(() => res(FIXTURE))); };
  const props = { running: true, config: null, configAt: null, status: null, binaryPath: "", actionInProgress: null, onSetCpus() {}, onSetMemory() {} };
  const r = await render(React.createElement(ResourcesView, props));
  assert.equal(n, 1, "one read when the page opens");
  const refresh = () => [...r.host.querySelectorAll(".storage-refresh")][0];
  assert.equal(refresh().disabled, true, "disabled while a read is running");
  await click(refresh()); await click(refresh());
  assert.equal(n, 1, "clicks during a read start nothing");
  await act(async () => { pend.shift()(); await new Promise((x) => setImmediate(x)); });
  assert.equal(refresh().disabled, false);
  assert.equal(r.host.querySelector("[data-storage-status]").dataset.storageStatus, "current");
  await click(refresh());
  assert.equal(n, 2, "a manual refresh reads again");
  await act(async () => { pend.shift()(); await new Promise((x) => setImmediate(x)); });
  await new Promise((x) => setTimeout(x, 50));
  assert.equal(n, 2, "no timer-driven reads");
  await r.unmount();
  assert.equal(storageInventory.get().enabled, false, "leaving the page disables reads");
  assert.equal(n, 2);
});

test("Harpoon not running: the page reads nothing", async () => {
  let n = 0; globalThis.__invoke = async () => { n++; return FIXTURE; };
  const r = await render(React.createElement(ResourcesView, { running: false, config: null, configAt: null, status: null, binaryPath: "", actionInProgress: null, onSetCpus() {}, onSetMemory() {} }));
  await flush();
  assert.equal(n, 0);
  assert.equal(r.host.querySelector("[data-storage-status]").dataset.storageStatus, "engine-down");
  await r.unmount();
});

test("long names wrap or truncate with the full name available, and every control is at least 44px tall in the stylesheet", async () => {
  const inv = JSON.parse(FIXTURE);
  const long = "registry.example.com/team/" + "a-very-long-image-name-".repeat(8) + ":latest";
  inv.categories.find((c) => c.id === "other-images").items[0].name = long;
  const r = await render(React.createElement(StorageBreakdown, { view: view({ inv }), onRefresh() {} }));
  await click(rowBtn(r.host, "Other images"));
  assert.equal([...r.host.querySelectorAll(".storage-item__name")].find((e) => e.textContent === long).getAttribute("title"), long);
  await r.unmount();
  const css = readFileSync(new URL("../styles/primitives.css", import.meta.url), "utf8");
  assert.match(css, /\.storage-row__head, \.storage-item__head \{[^}]*min-height: 44px/);
  assert.match(css, /\.storage-refresh \{ min-height: 44px; min-width: 44px/);
  assert.match(css, /\.storage-row__head:focus-visible/);
});
