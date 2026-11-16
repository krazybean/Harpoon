import assert from "node:assert/strict";
import test from "node:test";
import { ageLabel, createDiskTelemetry, diskView, guestLine, hostLine, initialTelemetry, POLL_MS, STALE_AFTER_MS } from "./diskTelemetry.ts";
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
  assert.equal(v.pressure, "warning");
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
