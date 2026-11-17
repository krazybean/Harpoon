import assert from "node:assert/strict";
import test from "node:test";
import { createInventoryStore, FETCH_TIMEOUT_MS, NATIVE_BUDGET_MS, inventoryView, initialInventory, parseStorageInventory, CATEGORY_ORDER } from "./storageInventory.ts";

// A runtime-shaped inventory (the Rust module's output contract), built field by field so each test can break one thing.
const item = (o = {}) => ({ id: "i1", name: "web:2", kind: "image", bytes: 500, bytesNote: "", state: "tagged-unreferenced", references: [], ownership: [{ property: "unknown-ownership", evidence: "none" }], protection: "none", protectionNote: "", details: [{ label: "Image id", value: "aaaa" }], ...o });
const cat = (id, o = {}) => ({ id, title: id, sizeBasis: "basis", status: "ok", note: null, count: 0, bytes: null, bytesComplete: false, skipped: 0, omitted: 0, figures: [], counts: [], items: [], ...o });
export const inv = (over = {}) => ({ schema: 1, generatedMs: 1000, sources: [{ endpoint: "/system/df", status: "ok", detail: "" }], journalAvailable: true, journalNote: null, imageLayersBytes: 900, imageLayersMeaning: "m", unknownOwnershipItems: 0, categories: CATEGORY_ORDER.map((id) => cat(id)), ...over });
const text = (o) => JSON.stringify(inv(o));

test("a runtime-shaped inventory parses, including unknown (null) sizes", () => {
  const cats = CATEGORY_ORDER.map((id) => cat(id, { count: 1, bytes: id === "volumes" ? 1000 : null, items: [item({ bytes: null })] }));
  const p = parseStorageInventory(text({ categories: cats, imageLayersBytes: null }));
  assert.equal(p.categories.length, 6);
  assert.equal(p.categories[0].items[0].bytes, null, "null stays null, never 0");
  assert.equal(p.imageLayersBytes, null);
});

test("malformed inventories are rejected, never repaired into zeros", () => {
  const bad = [
    "", "not json", "[]", "null", '{"schema":2}',
    text({ schema: 2 }),
    text({ categories: CATEGORY_ORDER.slice(1).map((id) => cat(id)) }),                            // a known category is missing
    text({ categories: CATEGORY_ORDER.map((id) => cat(id, { bytes: -1 })) }),                      // Docker's sentinel must never reach the UI as a number
    text({ categories: CATEGORY_ORDER.map((id) => cat(id, { bytes: "12" })) }),
    text({ categories: CATEGORY_ORDER.map((id) => cat(id, { status: "weird" })) }),
    text({ categories: CATEGORY_ORDER.map((id) => cat(id, { count: 1, items: [item({ bytes: -1 })] })) }),
    text({ categories: CATEGORY_ORDER.map((id) => cat(id, { count: 1, items: [item({ protection: "removable" })] })) }),
    text({ categories: CATEGORY_ORDER.map((id) => cat(id, { count: 1, items: [item({ ownership: [{ property: "x" }] })] })) }),
    text({ imageLayersBytes: 7 }).replace('"imageLayersBytes":7', '"imageLayersBytes":1e999'), // JSON.stringify would turn Infinity into null
    text({ sources: [{ endpoint: "/system/df", status: "great" }] }),
  ];
  for (const b of bad) assert.equal(parseStorageInventory(b), null, String(b).slice(0, 60));
});

function store(replies) {
  let t = 1000;
  const calls = [];
  const s = createInventoryStore(async () => { calls.push(t); const r = replies.shift(); if (r instanceof Error) throw r; return r; }, () => t);
  return { s, calls, at: (v) => { t = v; } };
}

test("nothing is read until the engine is running, and a read completes into 'current'", async () => {
  const { s, calls } = store([text()]);
  await s.refresh();
  assert.equal(calls.length, 0, "disabled: no request");
  assert.equal(inventoryView(s.get(), 1000).status, "engine-down");
  s.setEnabled(true);
  const r = s.refresh();
  assert.equal(inventoryView(s.get(), 1000).status, "loading");
  assert.equal(inventoryView(s.get(), 1000).loading, true);
  await r;
  const v = inventoryView(s.get(), 1000);
  assert.equal(v.status, "current"); assert.equal(v.loading, false); assert.equal(v.at, 1000);
});

test("rapid refresh requests share one request", async () => {
  const { s, calls } = store([text()]);
  s.setEnabled(true);
  const a = s.refresh();
  assert.strictEqual(s.refresh(), a); assert.strictEqual(s.refresh(), a);
  await a;
  assert.equal(calls.length, 1);
});

test("a failed refresh keeps the last inventory, stale, with the reason; success clears it", async () => {
  const { s, at } = store([text(), new Error("Docker did not answer in time"), text()]);
  s.setEnabled(true); await s.refresh();
  at(5000); await s.refresh();
  const v = inventoryView(s.get(), 5000);
  assert.equal(v.status, "stale"); assert.equal(v.at, 1000); assert.ok(v.inv);
  assert.match(v.note, /did not answer in time/);
  at(9000); await s.refresh();
  assert.equal(inventoryView(s.get(), 9000).status, "current");
});

test("no inventory and a failed read is unavailable, not empty; an unrecognised answer is unsupported and hides the old one", async () => {
  const a = store([new Error("no docker socket")]); a.s.setEnabled(true); await a.s.refresh();
  assert.equal(inventoryView(a.s.get(), 1).status, "unavailable"); assert.equal(inventoryView(a.s.get(), 1).inv, null);
  const b = store([text(), '{"schema":9}']); b.s.setEnabled(true); await b.s.refresh(); await b.s.refresh();
  const v = inventoryView(b.s.get(), 1);
  assert.equal(v.status, "unsupported"); assert.equal(v.inv, null);
  assert.ok(b.s.get().last, "kept in the store so the next valid answer recovers");
});

test("the engine stopping shows engine-down even with an old inventory", async () => {
  const { s } = store([text()]);
  s.setEnabled(true); await s.refresh(); s.setEnabled(false);
  const v = inventoryView(s.get(), 1);
  assert.equal(v.status, "engine-down"); assert.ok(v.inv, "the figures are kept to be labelled, not erased");
});

// --- timeout and concurrency: the page can give up on a request, never pile native work or be overwritten by an old answer
function fakeTimers() {
  const real = { set: globalThis.setTimeout, clear: globalThis.clearTimeout };
  const pending = new Map(); let id = 0;
  globalThis.setTimeout = (fn, ms) => { pending.set(++id, { fn, ms }); return id; };
  globalThis.clearTimeout = (i) => { pending.delete(i); };
  return { pending, fire: () => { const [i, t] = [...pending].at(-1); pending.delete(i); t.fn(); }, restore: () => { globalThis.setTimeout = real.set; globalThis.clearTimeout = real.clear; } };
}
const deferred = () => { let resolve, reject; const p = new Promise((a, b) => { resolve = a; reject = b; }); return { p, resolve, reject }; };

test("a late answer from a timed-out request never overwrites a newer inventory", async () => {
  const f = fakeTimers();
  try {
    const calls = []; let n = 0;
    const s = createInventoryStore(() => { const d = deferred(); calls.push(d); return d.p; }, () => 1000 + 100 * n++);
    s.setEnabled(true);
    const a = s.refresh(); assert.equal([...f.pending.values()][0].ms, FETCH_TIMEOUT_MS); f.fire(); await a;
    assert.equal(inventoryView(s.get(), 1000).status, "unavailable");
    calls[0].resolve(text({ generatedMs: 1, unknownOwnershipItems: 7 }));          // A answers late
    await new Promise((r) => setImmediate(r));
    assert.equal(s.get().last, null, "late answer ignored");
    const b = s.refresh(); calls[1].resolve(text({ unknownOwnershipItems: 2 })); await b;
    const good = s.get().last;
    assert.equal(good.inv.unknownOwnershipItems, 2);
    calls[0].resolve(text({ unknownOwnershipItems: 9 })); calls[1].resolve(text({ unknownOwnershipItems: 9 }));
    await new Promise((r) => setImmediate(r));
    assert.strictEqual(s.get().last, good);
  } finally { f.restore(); }
});

test("repeated timeouts start no second native request while the abandoned one still runs, and no timer is left behind", async () => {
  const f = fakeTimers();
  try {
    const calls = [];
    const s = createInventoryStore(() => { const d = deferred(); calls.push(d); return d.p; }, () => 1000);
    s.setEnabled(true);
    for (let i = 0; i < 10; i++) { const r = s.refresh(); if (f.pending.size) f.fire(); await r; }
    assert.equal(calls.length, 1, "ten refreshes, one native request");
    assert.equal(f.pending.size, 0);
    calls[0].reject(new Error("ended"));
    await new Promise((r) => setImmediate(r));
    const r = s.refresh(); calls[1].resolve(text()); await r;
    assert.equal(calls.length, 2); assert.equal(inventoryView(s.get(), 1000).status, "current");
  } finally { f.restore(); }
});

test("the page waits longer than the runtime's whole budget, so it never abandons work that is still within it", async () => {
  assert.equal(NATIVE_BUDGET_MS, 18_000);
  assert.ok(FETCH_TIMEOUT_MS >= NATIVE_BUDGET_MS + 5_000, "margin for discovery, the journal read and the hand-off");
  const f = fakeTimers();
  try {
    const calls = [];
    const s = createInventoryStore(() => { const d = deferred(); calls.push(d); return d.p; }, () => 1000);
    s.setEnabled(true);
    const r = s.refresh();
    assert.equal([...f.pending.values()][0].ms, FETCH_TIMEOUT_MS);
    // a slow native run that ends at its own deadline (inside the page's wait) is still applied
    calls[0].resolve(text({ unknownOwnershipItems: 3 })); await r;
    assert.equal(inventoryView(s.get(), 1000).status, "current"); assert.equal(s.get().last.inv.unknownOwnershipItems, 3);
    assert.equal(f.pending.size, 0);
  } finally { f.restore(); }
});

test("a native run that reports its own deadline failure is a stale/unavailable state with the reason, and the next run is allowed", async () => {
  const { s, at } = store([text(), new Error("the inventory's time limit was reached before this request could start"), text({ unknownOwnershipItems: 4 })]);
  s.setEnabled(true); await s.refresh();
  at(5000); await s.refresh();
  assert.equal(inventoryView(s.get(), 5000).status, "stale"); assert.match(inventoryView(s.get(), 5000).note, /time limit/);
  at(6000); await s.refresh();
  assert.equal(inventoryView(s.get(), 6000).status, "current"); assert.equal(s.get().last.inv.unknownOwnershipItems, 4);
});

test("the journal note is read when present and tolerated when absent", () => {
  assert.equal(parseStorageInventory(text({ journalAvailable: false, journalNote: "no deployment journal was found" })).journalNote, "no deployment journal was found");
  assert.equal(parseStorageInventory(text({ journalNote: null })).journalNote, null);
});
