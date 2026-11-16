import assert from "node:assert/strict";
import test from "node:test";
import { ageLabel, createDiskTelemetry, FETCH_TIMEOUT_MS, diskView, guestLine, hostLine, initialTelemetry, POLL_MS, STALE_AFTER_MS } from "./diskTelemetry.ts";
import { formatSize, GIB } from "./diskCapacity.ts";

const capText = (over = {}, guest = { capacityBytes: 10 * GIB, usedBytes: 5 * GIB, availableBytes: 5 * GIB }) =>
  JSON.stringify({ provisioned: true, state: "running", logicalBytes: 12 * GIB, physicalBytes: 3 * GIB, hostAvailableBytes: 50 * GIB, pressure: "ok",
    suggestedSizes: [], maxBytes: 512 * GIB, hostReserveBytes: 5 * GIB, guest, ...over });

function store(replies) {
  let t = 1000;
  const calls = [];
  const s = createDiskTelemetry(async () => { calls.push(t); const r = replies.shift(); if (r instanceof Error) throw r; return r; }, () => t);
  return { s, calls, at: (v) => { t = v; } };
}

test("the runtime's pressure is shown as reported, whatever the utilization (no thresholds of our own)", async () => {
  for (const [used, pressure] of [[2, "ok"], [5, "ok"], [8.5, "warning"], [9.5, "critical"], [3, "critical"], [9.6, "ok"]]) {
    const { s } = store([capText({ pressure }, { capacityBytes: 10 * GIB, usedBytes: used * GIB, availableBytes: (10 - used) * GIB })]);
    s.setEnabled(true); await s.refresh();
    const v = diskView(s.get(), 1000);
    assert.equal(v.status, "live");
    assert.equal(v.pressure, pressure);
    assert.equal(Math.round(v.percent), Math.round(used * 10));
  }
});

test("a failed refresh keeps the last good reading and its time, and says it is stale", async () => {
  const { s, at } = store([capText({ pressure: "warning" }), new Error("management channel timed out")]);
  s.setEnabled(true); await s.refresh();
  at(5000); await s.refresh();
  const v = diskView(s.get(), 5000);
  assert.equal(v.status, "stale");
  assert.equal(v.at, 1000, "the time of the last SUCCESSFUL reading");
  assert.equal(v.percent, 50, "not reset to zero");
  assert.equal(v.pressure, "unknown", "a remembered warning is not current health");
  assert.match(v.note, /management channel timed out/);
});

test("the next success clears the failure", async () => {
  const { s, at } = store([capText(), new Error("x"), capText()]);
  s.setEnabled(true); await s.refresh(); await s.refresh(); at(9000); await s.refresh();
  const v = diskView(s.get(), 9000);
  assert.equal(v.status, "live"); assert.equal(v.at, 9000);
});

test("a reading older than three polls is stale even with no failure", async () => {
  const { s } = store([capText()]);
  s.setEnabled(true); await s.refresh();
  assert.equal(diskView(s.get(), 1000 + STALE_AFTER_MS).status, "live");
  assert.equal(diskView(s.get(), 1000 + STALE_AFTER_MS + 1).status, "stale");
  assert.equal(STALE_AFTER_MS, 3 * POLL_MS);
});

test("no reading and a failed refresh is unavailable, not zero", async () => {
  const { s } = store([new Error("no harpoon binary")]);
  s.setEnabled(true); await s.refresh();
  const v = diskView(s.get(), 1000);
  assert.equal(v.status, "unavailable"); assert.equal(v.cap, null); assert.equal(v.percent, null);
  assert.equal(diskView(initialTelemetry(), 0).status, "engine-down", "before Harpoon is ready nothing is read");
  const first = createDiskTelemetry(async () => "", () => 0); first.setEnabled(true);
  assert.equal(diskView(first.get(), 0).status, "unavailable");
});

test("an unrecognised reply is unsupported and shows no figures, even after a good reading", async () => {
  const { s } = store(["not json", capText(), '{"hello":1}']);
  s.setEnabled(true); await s.refresh();
  assert.equal(diskView(s.get(), 1000).status, "unsupported");
  await s.refresh();
  assert.equal(diskView(s.get(), 1000).status, "live");
  await s.refresh();
  const v = diskView(s.get(), 1000);
  assert.equal(v.status, "unsupported"); assert.equal(v.cap, null); assert.equal(v.percent, null);
});

test("engine-down: Harpoon not ready, or the runtime reports it stopped; the old figures are labelled, not zeroed", async () => {
  const { s } = store([capText()]);
  s.setEnabled(true); await s.refresh();
  s.setEnabled(false);
  const v = diskView(s.get(), 1000);
  assert.equal(v.status, "engine-down"); assert.equal(v.percent, 50); assert.equal(v.at, 1000);
  const stopped = store([capText({ state: "stopped" }, undefined)]);
  stopped.s.setEnabled(true); await stopped.s.refresh();
  assert.equal(diskView(stopped.s.get(), 1000).status, "engine-down");
});

test("disconnected: the VM runs but the guest gave no filesystem figures; the Mac allocation is still real", async () => {
  const raw = JSON.parse(capText()); delete raw.guest;
  const { s } = store([JSON.stringify(raw)]);
  s.setEnabled(true); await s.refresh();
  const v = diskView(s.get(), 1000);
  assert.equal(v.status, "disconnected"); assert.equal(v.percent, null);
  assert.match(hostLine(v.cap, formatSize), /3 GiB allocated on this Mac of the 12 GiB disk image/);
  assert.equal(guestLine(v.cap, formatSize), null);
});

test("guest and host measurements are separate lines with their own numbers", async () => {
  const { s } = store([capText()]);
  s.setEnabled(true); await s.refresh();
  const c = diskView(s.get(), 1000).cap;
  assert.equal(guestLine(c, formatSize), "5 GiB used of 10 GiB · 5 GiB free");
  assert.equal(hostLine(c, formatSize), "3 GiB allocated on this Mac of the 12 GiB disk image");
});

test("concurrent refreshes make one request; every view shares it", async () => {
  const { s, calls } = store([capText()]);
  s.setEnabled(true);
  let n = 0; s.subscribe(() => n++);
  await Promise.all([s.refresh(), s.refresh(), s.refresh()]);
  assert.equal(calls.length, 1);
  assert.equal(n, 1, "subscribers hear one reading");
});

test("ageLabel", () => {
  assert.equal(ageLabel(null, 0), "never");
  assert.equal(ageLabel(1000, 1000), "just now");
  assert.equal(ageLabel(0, 30_000), "30 s ago");
  assert.equal(ageLabel(0, 5 * 60_000), "5 min ago");
});

test("only a live reading carries a pressure class; every other state reports unknown while keeping historical usage where it has any", async () => {
  const warn = capText({ pressure: "critical" });
  const noGuest = JSON.parse(warn); delete noGuest.guest;
  const mk = async (replies, enabled = true) => { const n = replies.length; const { s } = store(replies); s.setEnabled(true); for (let i = 0; i < n; i++) await s.refresh(); if (!enabled) s.setEnabled(false); return s; };
  const live = await mk([warn]);
  assert.equal(diskView(live.get(), 1000).pressure, "critical");
  const cases = {
    stale: await mk([warn, new Error("x")]),
    "stale by age": live,
    "engine-down": await mk([warn], false),
    stopped: await mk([capText({ state: "stopped", pressure: "critical" })]),
    disconnected: await mk([JSON.stringify({ ...noGuest, pressure: "critical" })]),
    unsupported: await mk([warn, "garbage"]),
    unavailable: await mk([new Error("x")]),
  };
  for (const [name, s] of Object.entries(cases)) {
    const v = diskView(s.get(), name === "stale by age" ? 1000 + STALE_AFTER_MS + 1 : 1000);
    assert.notEqual(v.status, "live", name);
    assert.equal(v.pressure, "unknown", name);
  }
  const hist = diskView(cases.stale.get(), 1000);
  assert.equal(hist.percent, 50, "historical usage remains for stale");
  assert.equal(diskView(cases.unsupported.get(), 1000).percent, null, "unsupported shows nothing");
});

test("the view carries the clock it was computed against", async () => {
  const { s } = store([capText()]);
  s.setEnabled(true); await s.refresh();
  assert.equal(diskView(s.get(), 4242).now, 4242);
});

test("a request that never answers is abandoned as a failure; refreshing resumes when the runtime call ends", async () => {
  const realSet = globalThis.setTimeout; const timers = [];
  globalThis.setTimeout = (fn, ms) => { timers.push({ fn, ms }); return timers.length; };
  const realClear = globalThis.clearTimeout; globalThis.clearTimeout = () => {};
  try {
    const calls = [];
    const s = createDiskTelemetry(() => new Promise((res) => calls.push(res)), () => 1000);
    s.setEnabled(true);
    const first = s.refresh();
    assert.strictEqual(s.refresh(), first, "a second caller joins the pending request");
    assert.equal(timers.at(-1).ms, FETCH_TIMEOUT_MS);
    timers.at(-1).fn();
    await first;
    assert.equal(diskView(s.get(), 1000).status, "unavailable");
    assert.match(s.get().failure.message, /timed out/);
    calls[0](capText());                                   // the old call finally ends
    await new Promise((r) => setImmediate(r));
    const next = s.refresh(); calls[1](capText()); await next;
    assert.equal(diskView(s.get(), 1000).status, "live", "the next refresh runs and recovers");
  } finally { globalThis.setTimeout = realSet; globalThis.clearTimeout = realClear; }
});

test("only a live reading carries a pressure class; every other state reports unknown while keeping historical usage where it has any", async () => {
  const warn = capText({ pressure: "critical" });
  const noGuest = JSON.parse(warn); delete noGuest.guest;
  const mk = async (replies, enabled = true) => { const n = replies.length; const { s } = store(replies); s.setEnabled(true); for (let i = 0; i < n; i++) await s.refresh(); if (!enabled) s.setEnabled(false); return s; };
  const live = await mk([warn]);
  assert.equal(diskView(live.get(), 1000).pressure, "critical");
  const cases = {
    stale: await mk([warn, new Error("x")]),
    "stale by age": live,
    "engine-down": await mk([warn], false),
    stopped: await mk([capText({ state: "stopped", pressure: "critical" })]),
    disconnected: await mk([JSON.stringify({ ...noGuest, pressure: "critical" })]),
    unsupported: await mk([warn, "garbage"]),
    unavailable: await mk([new Error("x")]),
  };
  for (const [name, s] of Object.entries(cases)) {
    const v = diskView(s.get(), name === "stale by age" ? 1000 + STALE_AFTER_MS + 1 : 1000);
    assert.notEqual(v.status, "live", name);
    assert.equal(v.pressure, "unknown", name);
  }
  const hist = diskView(cases.stale.get(), 1000);
  assert.equal(hist.percent, 50, "historical usage remains for stale");
  assert.equal(diskView(cases.unsupported.get(), 1000).percent, null, "unsupported shows nothing");
});

test("the view carries the clock it was computed against", async () => {
  const { s } = store([capText()]);
  s.setEnabled(true); await s.refresh();
  assert.equal(diskView(s.get(), 4242).now, 4242);
});

// --- timeout guarantees --------------------------------------------------------------------------------------------------------
// A fake setTimeout registry: pending timers are listed, and firing one removes it, like a real timer.
function fakeTimers() {
  const real = { set: globalThis.setTimeout, clear: globalThis.clearTimeout };
  const pending = new Map(); let id = 0;
  globalThis.setTimeout = (fn, ms) => { pending.set(++id, { fn, ms }); return id; };
  globalThis.clearTimeout = (i) => { pending.delete(i); };
  return { pending, fire: () => { const [i, t] = [...pending].at(-1); pending.delete(i); t.fn(); }, restore: () => { globalThis.setTimeout = real.set; globalThis.clearTimeout = real.clear; } };
}
const deferred = () => { let resolve, reject; const p = new Promise((a, b) => { resolve = a; reject = b; }); return { p, resolve, reject }; };

test("a late reply from a timed-out request is discarded and never overwrites newer state", async () => {
  const f = fakeTimers();
  try {
    const calls = []; let n = 0;
    const s = createDiskTelemetry(() => { const d = deferred(); calls.push(d); return d.p; }, () => 1000 + 100 * n++);
    s.setEnabled(true);
    const a = s.refresh(); f.fire(); await a;                          // request A is abandoned
    assert.equal(diskView(s.get(), 1000).status, "unavailable");
    calls[0].resolve(capText({ pressure: "critical" }));                // A answers late with a live-looking reading
    await new Promise((r) => setImmediate(r));
    assert.equal(s.get().last, null, "the late reply was not applied");
    const b = s.refresh(); calls[1].resolve(capText({ pressure: "ok" })); await b; // a newer request succeeds
    const good = s.get().last;
    assert.equal(good.cap.pressure, "ok");
    const c = s.refresh(); calls[2].reject(new Error("later failure")); await c;
    calls[0].resolve(capText({ pressure: "critical" }));                // A (long settled) cannot change anything now
    calls[1].resolve(capText({ pressure: "critical" }));
    await new Promise((r) => setImmediate(r));
    assert.strictEqual(s.get().last, good, "newer telemetry is untouched by old replies");
  } finally { f.restore(); }
});

test("repeated timeouts never run a second native request while the abandoned one is still running", async () => {
  const f = fakeTimers();
  try {
    const calls = [];
    const s = createDiskTelemetry(() => { const d = deferred(); calls.push(d); return d.p; }, () => 1000);
    s.setEnabled(true);
    for (let i = 0; i < 10; i++) { const r = s.refresh(); if (f.pending.size) f.fire(); await r; }
    assert.equal(calls.length, 1, "ten polls, one native request");
    assert.match(s.get().failure.message, /still running|timed out/);
    assert.equal(f.pending.size, 0, "no timer left behind");
    calls[0].reject(new Error("finally failed"));                         // the runtime call ends
    await new Promise((r) => setImmediate(r));
    const r = s.refresh(); calls[1].resolve(capText()); await r;
    assert.equal(calls.length, 2, "refreshing resumes once the old call has ended");
    assert.equal(diskView(s.get(), 1000).status, "live");
  } finally { f.restore(); }
});

test("the abandon timer never outlives a request: success, failure and timeout all leave none pending", async () => {
  const f = fakeTimers();
  try {
    const replies = [() => Promise.resolve(capText()), () => Promise.reject(new Error("x")), () => new Promise(() => {})];
    const s = createDiskTelemetry(() => replies.shift()(), () => 1000);
    s.setEnabled(true);
    await s.refresh(); assert.equal(f.pending.size, 0, "after success");
    await s.refresh(); assert.equal(f.pending.size, 0, "after failure");
    const r = s.refresh(); assert.equal(f.pending.size, 1, "one timer while waiting"); assert.equal([...f.pending.values()][0].ms, FETCH_TIMEOUT_MS);
    f.fire(); await r; assert.equal(f.pending.size, 0, "after timeout");
  } finally { f.restore(); }
});
