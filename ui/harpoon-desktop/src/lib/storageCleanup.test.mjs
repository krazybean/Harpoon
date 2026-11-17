// Slice 3: the Storage Breakdown leads to the EXISTING cleanup controls, and every result keeps Docker's figure, the guest's free space
// and the Mac's disk-image allocation apart. The real Resources page runs in jsdom; only Tauri's invoke is replaced by a script.
import assert from "node:assert/strict";
import test, { afterEach } from "node:test";
import { build } from "esbuild";
import { JSDOM } from "jsdom";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { cacheOutcome, failureOutcome, figureLines, releasesOutcome, reclaimOutcome } from "./cleanupResult.ts";
import { parseCleanup, parseCache } from "./artifactCleanup.ts";
import { CLEANUP_TARGETS } from "./cleanupTargets.ts";

const dom = new JSDOM('<!doctype html><html><body></body></html>', { pretendToBeVisual: true });
for (const k of ["window", "document", "navigator", "HTMLElement", "KeyboardEvent", "MouseEvent", "Event", "Node"]) {
  Object.defineProperty(globalThis, k, { value: dom.window[k], configurable: true, writable: true });
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const out = new URL("../../node_modules/.cache/storage-cleanup/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const mock = join(out, "tauri-core.js");
writeFileSync(mock, "export const invoke = (...a) => globalThis.__invoke(...a);");
const outfile = join(out, "cleanup.mjs");
await build({
  stdin: { resolveDir: new URL("../components/", import.meta.url).pathname, loader: "ts",
    contents: 'export { ResourcesView } from "../views/ResourcesView.tsx"; export { ConfirmDialogHost } from "./ConfirmDialog.tsx"; export { storageInventory } from "../hooks/useStorageInventory.ts"; export { diskTelemetry } from "../hooks/useDiskTelemetry.ts";' },
  outfile, bundle: true, format: "esm", platform: "node", logLevel: "silent", external: ["react", "react-dom"], alias: { "@tauri-apps/api/core": mock }, jsx: "automatic", loader: { ".css": "empty" },
});
const React = await import("react");
const { createRoot } = await import("react-dom/client");
const { ResourcesView, ConfirmDialogHost, storageInventory, diskTelemetry } = await import(pathToFileURL(outfile).href);
const { act } = React;
const FIXTURE = readFileSync(new URL("./fixtures/storage-inventory.json", import.meta.url), "utf8");
const GIB = 1024 ** 3, MIB = 1024 ** 2;

// ---- the scripted engine
let opLog = [];              // every deploy_op request
let reclaimLog = [];         // every reclaim request
let physical = [3 * GIB];    // successive Mac allocations the disk status reports (the last one repeats)
let diskDown = false;
let diskHook = null;         // when set, answers get_disk_capacity itself (to script ordering)
const events = [];           // ordered log of disk reads and engine operations
let deployReply = () => { throw new Error("no reply scripted"); };
let reclaimReply = () => JSON.stringify({ status: "reclaimed", message: "Reclaimed.", logicalBytes: 12 * GIB, physicalBeforeBytes: 3 * GIB, physicalAfterBytes: 2 * GIB });
const capJson = (phys) => JSON.stringify({ provisioned: true, state: "running", logicalBytes: 12 * GIB, physicalBytes: phys, pressure: "ok", suggestedSizes: [], maxBytes: 512 * GIB, hostReserveBytes: 5 * GIB, guest: { capacityBytes: 10 * GIB, usedBytes: 4 * GIB, availableBytes: 6 * GIB } });
globalThis.__invoke = async (cmd, args) => {
  if (cmd === "get_disk_capacity") {
    events.push("disk:start");
    await new Promise((r) => setImmediate(r));
    const done = (v) => { events.push("disk:end"); return v; };
    if (diskHook) return done(await diskHook());
    if (diskDown) { events.push("disk:end"); throw new Error("management channel timed out"); }
    return done(capJson(physical.length > 1 ? physical.shift() : physical[0]));
  }
  if (cmd === "storage_inventory") return FIXTURE;
  if (cmd === "deploy_op") { opLog.push(args); const tag = args.op.apply ? "apply" : "preview"; events.push(`op:${tag}:start`); try { return await deployReply(args); } finally { events.push(`op:${tag}:end`); } }
  if (cmd === "reclaim_disk_confirmed") { reclaimLog.push(cmd); const r = reclaimReply(); if (r instanceof Error) throw r; return r; }
  throw new Error(`unexpected command ${cmd}`);
};
const ok = (detail) => ({ ok: true, exitCode: 0, summary: "ok", detail, refusedByGate: false });
const bad = (code, summary, hint = null) => ({ ok: false, exitCode: code, summary, detail: "", refusedByGate: false, hint });
const itemLine = (kind, verdict, name = `${kind}-1`) => `item: ${JSON.stringify({ app: "app_1", appName: "shop", environment: "production", deployment: "dep_01M6DPQ543ASWBHW7PW17ZAJ3C", createdMs: 1, status: "succeeded", commit: "abc1234", kind, name, verdict, reason: "superseded", detail: "d", physical: true })}`;
const PLAN = [itemLine("container", "ready"), itemLine("image", "ready")].join("\n");

async function mount(running = true) {
  opLog = []; reclaimLog = []; physical = [3 * GIB]; diskDown = false; diskHook = null; events.length = 0; reclaimReply = () => JSON.stringify({ status: "reclaimed", message: "Reclaimed.", logicalBytes: 12 * GIB, physicalBeforeBytes: 3 * GIB, physicalAfterBytes: 2 * GIB });
  diskTelemetry.setEnabled(running);
  const host = document.createElement("div"); document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(React.createElement(React.Fragment, null,
    React.createElement(ResourcesView, { running, config: null, configAt: null, status: null, binaryPath: "", actionInProgress: null, onSetCpus() {}, onSetMemory() {} }),
    React.createElement(ConfirmDialogHost))));
  await act(async () => { await new Promise((r) => setImmediate(r)); });
  const q = (sel) => host.querySelector(sel);
  const btn = (text) => [...host.querySelectorAll("button")].find((b) => b.textContent.startsWith(text));
  const click = (b) => act(async () => { b.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true, cancelable: true })); });
  const settle = () => act(async () => { await new Promise((r) => setImmediate(r)); await new Promise((r) => setImmediate(r)); });
  const dialog = () => document.querySelector('[role="dialog"]');
  const dialogBtn = (text) => [...document.querySelectorAll('[role="dialog"] button')].find((b) => b.textContent.includes(text));
  const page = { host, q, btn, click, settle, dialog, dialogBtn, unmount: async () => { if (!live.delete(page)) return; await act(async () => { root.unmount(); host.remove(); }); diskTelemetry.setEnabled(false); } };
  live.add(page);
  return page;
}
// a failed assertion must not leave a page mounted (its timers and dialog host would leak into the next test)
const live = new Set();
afterEach(async () => {
  // answer any confirmation left open: it holds the one-cleanup-at-a-time lock until it is
  for (const b of [...document.querySelectorAll('[role="dialog"] button')].filter((b) => b.textContent === "Cancel")) await act(async () => { b.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  for (const p of [...live]) await p.unmount();
});
const text = (el) => el.textContent.replace(/\s+/g, " ");
const apply = () => opLog.filter((a) => a.op.apply === true);

// ---------------------------------------------------------------- navigation: focus only, nothing started

test("each actionable row moves focus to the existing control and starts nothing", async () => {
  const m = await mount();
  for (const [row, label, id] of [["Deployment images", "Go to Review old releases", CLEANUP_TARGETS["old-releases"].button], ["Obsolete deployment artifacts", "Go to Review old releases", CLEANUP_TARGETS["old-releases"].button], ["Build cache", "Go to Check build cache", CLEANUP_TARGETS["build-cache"].button]]) {
    const head = [...m.host.querySelectorAll(".storage-row__head")].find((b) => b.textContent.includes(row));
    if (head.getAttribute("aria-expanded") === "false") await m.click(head);
    const go = [...m.host.querySelectorAll(".storage-action")].find((b) => b.textContent === label && b.closest(".storage-row").textContent.includes(row));
    assert.ok(go, `${row}: has its action`);
    await m.click(go);
    assert.equal(document.activeElement.id, id, `${row}: focus is on the existing control`);
    assert.equal(document.activeElement.disabled, false);
  }
  assert.deepEqual(opLog, [], "navigation asked the engine nothing");
  assert.equal(m.dialog(), null, "and opened no confirmation");
  await m.unmount();
});

test("the disk-image link focuses the existing reclaim control, or its section when it is unavailable", async () => {
  const m = await mount();
  await m.settle();
  const go = () => [...m.host.querySelectorAll(".storage-action")].find((b) => b.textContent === "Go to Reclaim unused space");
  await m.click(go());
  assert.equal(document.activeElement.id, CLEANUP_TARGETS["disk-blocks"].button, "live disk status: the control itself");
  diskTelemetry.setEnabled(false);
  await act(async () => { await new Promise((r) => setImmediate(r)); });
  await m.click(go());
  assert.equal(document.activeElement.id, CLEANUP_TARGETS["disk-blocks"].section, "no live reading: the section that explains why");
  assert.deepEqual(reclaimLog, []);
  await m.unmount();
});

test("Harpoon stopped: the review link lands on its section, never on a disabled control", async () => {
  const warm = await mount(); await warm.unmount();          // an inventory read while Harpoon ran is kept, and labelled, when it stops
  const m = await mount(false);
  assert.equal(m.q(`#${CLEANUP_TARGETS["old-releases"].button}`).disabled, true);
  const head = [...m.host.querySelectorAll(".storage-row__head")].find((b) => b.textContent.includes("Deployment images"));
  await m.click(head);
  const go = [...m.host.querySelectorAll(".storage-action")].find((b) => b.textContent === "Go to Review old releases");
  await m.click(go);
  assert.equal(document.activeElement.id, CLEANUP_TARGETS["old-releases"].section, "the section, because the button cannot take focus");
  assert.deepEqual(opLog, []);
  await m.unmount();
});

test("inspection-only rows say so and offer no action", async () => {
  const m = await mount();
  for (const row of ["Other images", "Persistent volumes", "Container writable layers"]) {
    const head = [...m.host.querySelectorAll(".storage-row__head")].find((b) => b.textContent.includes(row));
    await m.click(head);
    const body = head.closest(".storage-row");
    assert.match(text(body), /Inspection only/, row);
    assert.equal(body.querySelectorAll(".storage-action").length, 0, `${row}: no action button`);
  }
  const un = [...m.host.querySelectorAll(".storage-row__head")].find((b) => b.textContent.includes("unattributed"));
  await m.click(un);
  assert.equal(un.closest(".storage-row").querySelectorAll("button.storage-action").length, 0);
  assert.match(text(un.closest(".storage-row")), /would be a made-up number/);
  await m.unmount();
});

// ---------------------------------------------------------------- confirmation, double clicks, changing state

test("the existing confirmation stays required: nothing is applied before it, and cancelling applies nothing", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? "reclaimed-containers: 1\nreclaimed-images: 1" : PLAN);
  await m.click(m.btn("Review old releases"));
  await m.settle();
  assert.deepEqual(opLog.map((a) => a.op.apply), [false], "only the read-only listing ran");
  await m.click(m.btn("Review cleanup") ?? m.btn("Remove 2 Docker artifacts"));
  assert.ok(m.dialog(), "a confirmation opens");
  assert.equal(apply().length, 0, "nothing ran before confirming");
  await act(async () => { m.dialogBtn("Cancel").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  assert.equal(apply().length, 0, "cancelled");
  await m.unmount();
});

test("a double click and a repeated confirmation run the cleanup once", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? "reclaimed-containers: 1\nreclaimed-images: 1" : PLAN);
  await m.click(m.btn("Review old releases")); await m.settle();
  const go = m.btn("Remove 2 Docker artifacts");
  await m.click(go); await m.click(go);                     // two clicks before the dialog is answered
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1, "one confirmation, not two");
  const confirm = m.dialogBtn("Remove 2 Docker artifacts");
  await act(async () => { confirm.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); confirm.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  assert.equal(apply().length, 1, "exactly one apply: " + JSON.stringify(opLog));
  await m.unmount();
});

test("the inventory changing between the review and the confirmation is a partial result, and the reviewed list is dropped", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? "reclaimed-containers: 1\nreclaimed-images: 0\nguest-free-before-bytes: 1000\nguest-free-after-bytes: 2000" : PLAN);
  await m.click(m.btn("Review old releases")); await m.settle();
  await m.click(m.btn("Remove 2 Docker artifacts"));
  await act(async () => { m.dialogBtn("Remove 2 Docker artifacts").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  const r = m.q('[data-result-kind="old-releases"]');
  assert.equal(r.dataset.resultStatus, "partial");
  assert.match(text(r), /1 of 2 Docker objects were removed/);
  assert.match(text(r), /re-checks each object when applying/);
  assert.equal(m.btn("Remove 2 Docker artifacts"), undefined, "the old list cannot be applied again");
  await m.unmount();
});

test("the engine removing none of what was reviewed is 'nothing was removed', not success", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? "reclaimed-containers: 0\nreclaimed-images: 0" : PLAN);
  await m.click(m.btn("Review old releases")); await m.settle();
  await m.click(m.btn("Remove 2 Docker artifacts"));
  await act(async () => { m.dialogBtn("Remove 2 Docker artifacts").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  assert.equal(m.q('[data-result-kind="old-releases"]').dataset.resultStatus, "nothing");
  await m.unmount();
});

// ---------------------------------------------------------------- build cache: guard, confirmation, measurements

const CACHE_PREVIEW = (guard) => `build cache: 5 bytes\nbuild-cache-reclaimable-bytes: ${1250 * MIB}\n${guard}`;
const CACHE_DONE = `build-cache-reclaimed-bytes: ${1250 * MIB}\nguest-free-before-bytes: ${5 * GIB}\nguest-free-after-bytes: ${6 * GIB}`;

test("an unfinished deployment is shown up front and offers no prune; a refused apply is 'not carried out'", async () => {
  const m = await mount();
  deployReply = () => ok(CACHE_PREVIEW("prune-guard: blocked: deployment dep_x is running (run `recover` to mark it interrupted)"));
  await m.click(m.btn("Check build cache")); await m.settle();
  assert.match(text(m.host), /Pruning is unavailable right now: deployment dep_x is running/);
  assert.equal(m.btn("Prune"), undefined, "no prune button while the guard is blocked");
  // and if it is applied anyway (the guard clears in the preview, then a deployment starts), the engine refuses
  deployReply = (a) => (a.op.apply ? bad(6, "the build cache was not pruned: deployment dep_y is running.", "Prune only when no deployment is running or unfinished.") : ok(CACHE_PREVIEW("prune-guard: clear")));
  await m.click(m.btn("Check build cache")); await m.settle();
  await m.click(m.btn("Prune"));
  await act(async () => { m.dialogBtn("Prune build cache").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  const r = m.q('[data-result-kind="build-cache"]');
  assert.equal(r.dataset.resultStatus, "refused");
  assert.match(text(r), /Not carried out\. Nothing was changed\./);
  assert.doesNotMatch(text(r), /Docker reports/);
  assert.equal(r.getAttribute("role"), "alert");
  await m.unmount();
});

test("the prune needs its confirmation, which says it does not protect against other Docker clients", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? CACHE_DONE : CACHE_PREVIEW("prune-guard: clear"));
  await m.click(m.btn("Check build cache")); await m.settle();
  await m.click(m.btn("Prune"));
  assert.ok(m.dialog()); assert.equal(apply().length, 0);
  assert.match(text(m.dialog()), /does not protect against builds started by other Docker clients/);
  await act(async () => { m.dialogBtn("Cancel").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  assert.equal(apply().length, 0);
  await m.unmount();
});

test("a build-cache result lists Docker's figure, the guest's free space and the Mac allocation apart", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? CACHE_DONE : CACHE_PREVIEW("prune-guard: clear"));
  await m.click(m.btn("Check build cache")); await m.settle();
  physical = [3 * GIB, 3 * GIB];                          // the Mac allocation does not move
  await m.click(m.btn("Prune"));
  await act(async () => { m.dialogBtn("Prune build cache").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  const r = m.q('[data-result-kind="build-cache"]');
  assert.equal(r.dataset.resultStatus, "success");
  assert.match(text(r.querySelector('[data-figure="docker"]')), /1\.2 GiB.*accounting figure, not guaranteed free disk space/);
  assert.match(text(r.querySelector('[data-figure="guest"]')), /5\.0 GiB before, 6\.0 GiB after \(\+1\.0 GiB\)/);
  assert.match(text(r.querySelector('[data-figure="mac"]')), /3\.0 GiB before, 3\.0 GiB after \(unchanged\).*only returned to macOS by Reclaim unused space/);
  assert.equal(apply().length, 1);
  await m.unmount();
});

test("measurements that were not taken stay unknown: no zero is invented", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? `build-cache-reclaimed-bytes: ${100 * MIB}\nguest-free: not measurable (the management channel did not answer)` : CACHE_PREVIEW("prune-guard: clear"));
  await m.click(m.btn("Check build cache")); await m.settle();
  diskDown = true;                                        // the disk status stops answering: the Mac allocation cannot be read either
  await m.click(m.btn("Prune"));
  await act(async () => { m.dialogBtn("Prune build cache").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  const r = m.q('[data-result-kind="build-cache"]');
  assert.match(text(r.querySelector('[data-figure="guest"]')), /^Guest free spaceNot measured\./);
  assert.match(text(r.querySelector('[data-figure="mac"]')), /^Mac disk-image allocationNot measured\./);
  assert.doesNotMatch(text(r), /\b0 B\b|\b0 MiB\b/);
  assert.match(text(r.querySelector('[data-figure="docker"]')), /100\.0? MiB|100 MiB/);
  await m.unmount();
});

test("a missing cache figure is unknown, not 'nothing to prune'", async () => {
  const m = await mount();
  deployReply = () => ok("build cache: unknown\n");
  await m.click(m.btn("Check build cache")); await m.settle();
  assert.match(text(m.host), /gave no build-cache figure, so nothing can be offered/);
  assert.doesNotMatch(text(m.host), /No unused build cache/);
  assert.equal(m.btn("Prune"), undefined);
  await m.unmount();
});

// ---------------------------------------------------------------- timeout, interruption, reclaim after cleanup

test("a request that never answers is an unknown result, never success", async () => {
  const m = await mount();
  deployReply = (a) => { if (a.op.apply) throw new Error("the request timed out"); return ok(PLAN); };
  await m.click(m.btn("Review old releases")); await m.settle();
  await m.click(m.btn("Remove 2 Docker artifacts"));
  await act(async () => { m.dialogBtn("Remove 2 Docker artifacts").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  const r = m.q('[data-result-kind="old-releases"]');
  assert.equal(r.dataset.resultStatus, "unknown");
  assert.equal(r.getAttribute("role"), "alert");
  assert.match(text(r), /No answer was received, so the result is not known/);
  assert.match(text(r), /may have completed fully, partly or not at all/);
  assert.doesNotMatch(text(r), /Removed \d+ container/);
  assert.equal(m.btn("Remove 2 Docker artifacts"), undefined, "the reviewed list is gone: review again before anything else");
  await m.unmount();
});

test("an engine failure is its own state and keeps what was already removed honest", async () => {
  const m = await mount();
  deployReply = (a) => (a.op.apply ? bad(5, "Docker did not remove the image", "Check Docker and review again.") : ok(PLAN));
  await m.click(m.btn("Review old releases")); await m.settle();
  await m.click(m.btn("Remove 2 Docker artifacts"));
  await act(async () => { m.dialogBtn("Remove 2 Docker artifacts").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  const r = m.q('[data-result-kind="old-releases"]');
  assert.equal(r.dataset.resultStatus, "failed");
  assert.match(text(r), /Check Docker and review again/); assert.match(text(r), /Anything already removed is not restored/);
  await m.unmount();
});

test("a reclaim that fails after a successful Docker cleanup does not hide or change the cleanup's result", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? "reclaimed-containers: 1\nreclaimed-images: 1\nguest-free-before-bytes: 1000\nguest-free-after-bytes: 5000" : PLAN);
  await m.click(m.btn("Review old releases")); await m.settle();
  await m.click(m.btn("Remove 2 Docker artifacts"));
  await act(async () => { m.dialogBtn("Remove 2 Docker artifacts").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  assert.equal(m.q('[data-result-kind="old-releases"]').dataset.resultStatus, "success");
  reclaimReply = () => JSON.stringify({ status: "unsupported", message: "The guest does not support trimming this disk. Nothing was reclaimed.", logicalBytes: 12 * GIB, physicalBeforeBytes: 3 * GIB, physicalAfterBytes: 3 * GIB });
  await m.click(m.btn("Reclaim unused space"));
  await act(async () => { m.dialogBtn("Reclaim unused space").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  const cleanup = m.q('[data-result-kind="old-releases"]'), reclaim = m.q('[data-result-kind="disk-blocks"]');
  assert.equal(cleanup.dataset.resultStatus, "success", "the Docker cleanup result is unchanged");
  assert.equal(reclaim.dataset.resultStatus, "failed");
  assert.equal(reclaim.getAttribute("role"), "alert");
  assert.match(text(reclaim.querySelector('[data-figure="docker"]')), /Not applicable: this operation does not change Docker/);
  assert.match(text(cleanup.querySelector('[data-figure="docker"]')), /1 container and 1 image removed/);
  await m.unmount();
});

test("a repeated reclaim click and confirmation run it once", async () => {
  const m = await mount();
  await m.settle();
  const go = m.btn("Reclaim unused space");
  await m.click(go); await m.click(go);
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  const c = m.dialogBtn("Reclaim unused space");
  await act(async () => { c.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); c.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  assert.equal(reclaimLog.length, 1);
  await m.unmount();
});

// ---------------------------------------------------------------- pure outcome model

test("the outcome model keeps unknown, measured zero and not-applicable distinct", () => {
  const plan = parseCleanup(PLAN);
  const done = parseCleanup("reclaimed-containers: 1\nreclaimed-images: 1\nguest-free-before-bytes: 10\nguest-free-after-bytes: 10");
  const o = releasesOutcome(plan, done, { before: 7, after: 7 });
  const lines = Object.fromEntries(figureLines(o, (b) => `${b}B`).map((l) => [l.key, l.text]));
  assert.match(lines.guest, /10B before, 10B after \(unchanged\)/, "a measured no-change is stated as a measurement");
  assert.match(lines.mac, /7B before, 7B after \(unchanged\)/);
  const unknown = figureLines(releasesOutcome(plan, parseCleanup("reclaimed-containers: 1\nreclaimed-images: 1"), { before: null, after: 7 }), (b) => `${b}B`);
  assert.match(unknown.find((l) => l.key === "guest").text, /^Not measured\./);
  assert.match(unknown.find((l) => l.key === "mac").text, /Before not measured, after 7B\. A change cannot be stated\./);
  const zero = cacheOutcome(parseCache("build-cache-reclaimed-bytes: 0"), { before: null, after: null }, (b) => `${b}B`);
  assert.equal(zero.status, "nothing"); assert.equal(zero.figures.docker.bytes, 0);
  assert.equal(cacheOutcome(parseCache("x"), { before: null, after: null }, (b) => `${b}B`).status, "unknown", "no figure is unknown, not zero");
  const rc = reclaimOutcome({ status: "reclaimed", message: "m", physicalBeforeBytes: 9, physicalAfterBytes: 4 });
  assert.equal(rc.figures.docker.applicable, false); assert.equal(rc.figures.guest, "not-applicable");
  assert.equal(failureOutcome("old-releases", { ok: false, exitCode: 6, summary: "s", detail: "", refusedByGate: false }, null).status, "refused");
  assert.equal(failureOutcome("old-releases", { ok: false, exitCode: 5, summary: "s", detail: "", refusedByGate: false }, null).status, "failed");
  assert.equal(failureOutcome("old-releases", null, new Error("timed out")).status, "unknown");
});

// ---------------------------------------------------------------- no new removal paths

test("the breakdown and its helpers cannot invoke anything, and no image, container or volume removal was added", () => {
  const read = (p) => readFileSync(new URL(p, import.meta.url), "utf8");
  for (const f of ["../components/StorageBreakdown.tsx", "./cleanupTargets.ts", "./cleanupResult.ts", "../components/CleanupResult.tsx", "./storageInventory.ts"]) {
    assert.doesNotMatch(read(f), /\binvoke\b|@tauri-apps/, `${f} cannot call the runtime`);
  }
  const card = read("../components/DiskReclaimCard.tsx");
  const cmds = [...card.matchAll(/invoke<[^>]*>\("([a-z_]+)"/g)].map((m) => m[1]).sort();
  assert.deepEqual([...new Set(cmds)], ["deploy_op", "reclaim_disk_confirmed"], "the cleanup card still reaches only the existing operations");
  assert.deepEqual([...new Set([...card.matchAll(/op: "([a-z-]+)"/g)].map((m) => m[1]))].sort(), ["cleanup-releases", "prune-build-cache"]);
  assert.doesNotMatch(card, /remove_image|remove_volume|remove_container|rmi|--force|force=true|all=true/);
  const api = read("../../src-tauri/src/deploy_api.rs");
  assert.doesNotMatch(api, /RemoveImage|remove-image|RemoveVolume|RemoveContainer/, "no new deployment operation removes an image, volume or container");
});

// ---------------------------------------------------------------- measurement ordering

const confirmCachePrune = async (m) => { await m.click(m.btn("Prune")); await act(async () => { m.dialogBtn("Prune build cache").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); }); };
const applyWindow = () => events.slice(events.indexOf("op:apply:start") - 4);

test("the Mac allocation is read to completion before the operation starts and only after it has ended", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? CACHE_DONE : CACHE_PREVIEW("prune-guard: clear"));
  await m.click(m.btn("Check build cache")); await m.settle();
  events.length = 0;
  physical = [3 * GIB, 2 * GIB];                                  // before, after
  await confirmCachePrune(m); await m.settle();
  const seq = events.filter((e) => e !== "op:preview:start" && e !== "op:preview:end");
  const start = seq.indexOf("op:apply:start"), end = seq.indexOf("op:apply:end");
  assert.ok(start > 0 && end > start, JSON.stringify(seq));
  const before = seq.slice(0, start), after = seq.slice(end + 1);
  assert.deepEqual(before.slice(-2), ["disk:start", "disk:end"], "the before reading finished before the operation began");
  assert.ok(after.length >= 2 && after[0] === "disk:start", "the after reading began only after the operation ended: " + JSON.stringify(seq));
  assert.ok(after.includes("disk:end"));
  const mac = text(m.q('[data-result-kind="build-cache"] [data-figure="mac"]'));
  assert.match(mac, /3\.0 GiB before, 2\.0 GiB after \(−1\.0 GiB\)/, "before and after come from the two bracketing reads, in that order");
});

test("a disk read already running when the operation ends is not used as the 'after': a new one is taken", async () => {
  const m = await mount();
  let release; const midOp = new Promise((r) => { release = r; });
  deployReply = async (a) => {
    if (!a.op.apply) return ok(CACHE_PREVIEW("prune-guard: clear"));
    // the 60-second poll fires mid-operation: its read started before the operation ended and has not finished
    diskHook = () => midOp;
    void diskTelemetry.refresh();
    await new Promise((r) => setImmediate(r));
    diskHook = null; physical = [7 * GIB];
    return ok(CACHE_DONE);
  };
  await m.click(m.btn("Check build cache")); await m.settle();
  physical = [3 * GIB];
  const done = confirmCachePrune(m);
  await new Promise((r) => setTimeout(r, 30));
  release(capJson(5 * GIB));                                      // the mid-operation read finally answers (5 GiB)
  await done; await m.settle();
  const mac = text(m.q('[data-result-kind="build-cache"] [data-figure="mac"]'));
  assert.match(mac, /3\.0 GiB before, 7\.0 GiB after/, "the after reading is the fresh one (7 GiB), not the mid-operation one (5 GiB): " + mac);
});

test("a failed or missing bracketing reading makes only that side unavailable, and no change is claimed", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? CACHE_DONE : CACHE_PREVIEW("prune-guard: clear"));
  await m.click(m.btn("Check build cache")); await m.settle();
  let n = 0;
  diskHook = async () => { if (++n === 2) throw new Error("channel down"); return capJson(3 * GIB); }; // the read after the operation fails
  await confirmCachePrune(m); await m.settle();
  const mac = text(m.q('[data-result-kind="build-cache"] [data-figure="mac"]'));
  assert.match(mac, /Before 3\.0 GiB, after not measured\. A change cannot be stated\./);
  assert.doesNotMatch(mac, /unchanged|\+|−/);
});

test("an earlier cleanup result is never updated by a later reclaim", async () => {
  const m = await mount();
  deployReply = (a) => ok(a.op.apply ? CACHE_DONE : CACHE_PREVIEW("prune-guard: clear"));
  await m.click(m.btn("Check build cache")); await m.settle();
  physical = [3 * GIB, 3 * GIB];
  await confirmCachePrune(m); await m.settle();
  const mine = () => text(m.q('[data-result-kind="build-cache"] [data-figure="mac"]'));
  const first = mine();
  physical = [2 * GIB];                                           // the guest then hands blocks back
  await m.click(m.btn("Reclaim unused space"));
  await act(async () => { m.dialogBtn("Reclaim unused space").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await m.settle();
  assert.equal(mine(), first, "the cache result still shows only what happened around the prune");
  const reclaim = text(m.q('[data-result-kind="disk-blocks"] [data-figure="mac"]'));
  assert.match(reclaim, /3\.0 GiB before, 2\.0 GiB after/, "the trim's own measurement is on its own result");
});

test("only one apply-type cleanup runs at a time, so none can land inside another's measurements", async () => {
  const m = await mount();
  let finish; const gate = new Promise((r) => { finish = r; });
  deployReply = async (a) => { if (a.op.apply) { await gate; return ok("reclaimed-containers: 1\nreclaimed-images: 1"); } return ok(PLAN); };
  await m.click(m.btn("Review old releases")); await m.settle();
  await m.click(m.btn("Remove 2 Docker artifacts"));
  await act(async () => { m.dialogBtn("Remove 2 Docker artifacts").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
  await new Promise((r) => setTimeout(r, 20));                    // the cleanup is executing
  assert.equal(m.btn("Reclaim unused space").disabled, true, "reclaim is unavailable while a cleanup runs");
  await m.click(m.btn("Reclaim unused space"));
  assert.equal(m.dialog(), null); assert.equal(reclaimLog.length, 0);
  finish(); await m.settle();
  assert.equal(m.btn("Reclaim unused space").disabled, false, "available again once the result is in");
});

// ---------------------------------------------------------------- build-cache confirmation wording

test("the prune confirmation says all=false can remove reusable cache and that the figure is not recovered disk space", async () => {
  const m = await mount();
  deployReply = () => ok(CACHE_PREVIEW("prune-guard: clear"));
  await m.click(m.btn("Check build cache")); await m.settle();
  await m.click(m.btn("Prune"));
  const body = text(m.dialog());
  assert.match(body, /Unused is not the same as useless: this cache can be reusable, so removing it can make later builds slower/);
  assert.match(body, /Docker decides what is unused when it prunes, and that can differ from this figure/);
  assert.match(body, /Docker's accounting, not a promise of free disk space/);
  assert.match(body, /space stays allocated on your Mac until you reclaim unused space/);
  assert.match(body, /does not protect against builds started by other Docker clients/);
  assert.doesNotMatch(body, /guarantee|will free|frees up|recover(s|ed)? (the )?(entire|full|all)/i);
  const src = readFileSync(new URL("../components/DiskReclaimCard.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(src, /all=true|all: true|--all\b/, "the prune was not widened");
  await act(async () => { m.dialogBtn("Cancel").dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); });
});
