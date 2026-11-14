import assert from "node:assert/strict";
import test from "node:test";
import { advance, emptyTrack, formatBytes, formatPercent, formatRate, formatUptime, HISTORY_MS, latestRate, memoryFigures, POLL_MS, segments, STALE_MS, targetKey, view } from "./containerMetrics.ts";

const T0 = 1_700_000_000_000;
const KEY = "app_1/dep_1/hp-dep_1/abc";
const m = (over = {}) => ({ state: "ok", detail: null, sampledMs: T0, containerId: "abc", startedAt: "2026-11-14T00:00:00Z", restartCount: 0, cpuPercent: 1.5, cpus: 4, memRaw: 1000, memCache: 200, memWorkingSet: 800, memLimit: null, netRx: 1000, netTx: 500, pids: 3, ...over });
const feed = (events, key = KEY) => events.reduce((t, e) => advance(t, key, "error" in e ? e : { reading: e }, e.sampledMs ?? e.atMs ?? T0), emptyTrack(key));

test("a first reading has current figures but no rate; the second gives rates from the counter deltas", () => {
  let t = feed([m()]);
  assert.equal(view(t, T0).status, "ok");
  assert.equal(latestRate(t.samples, "rx"), null, "one sample is not a rate");
  t = feed([m(), m({ sampledMs: T0 + POLL_MS, netRx: 4000, netTx: 500 })]);
  assert.equal(latestRate(t.samples, "rx"), 1000, "3000 bytes in 3 s");
  assert.equal(latestRate(t.samples, "tx"), 0, "a measured zero rate is zero, not missing");
});

test("a counter that goes backwards (reset) gives no rate for that interval and resumes after", () => {
  const t = feed([m(), m({ sampledMs: T0 + 3000, netRx: 10 }), m({ sampledMs: T0 + 6000, netRx: 70 })]);
  assert.deepEqual(t.samples.map((p) => p.rx), [null, null, 20]);
});

test("a restarted container (new start time) starts a new run: history is cleared and no rate spans the restart", () => {
  const t = feed([m(), m({ sampledMs: T0 + 3000, netRx: 2000 }), m({ sampledMs: T0 + 6000, startedAt: "2026-11-14T00:00:05Z", netRx: 50 })]);
  assert.equal(t.samples.length, 1);
  assert.equal(t.samples[0].rx, null);
  assert.equal(t.restartsSeen, 1);
});

test("a different target (another release or container) never joins samples", () => {
  let t = feed([m(), m({ sampledMs: T0 + 3000 })]);
  assert.equal(t.samples.length, 2);
  t = advance(t, "app_1/dep_2/hp-dep_2/def", { reading: m({ sampledMs: T0 + 6000, containerId: "def" }) }, T0 + 6000);
  assert.equal(t.samples.length, 1);
  assert.equal(t.key, "app_1/dep_2/hp-dep_2/def");
  assert.equal(t.restartsSeen, 0, "a different container is a replacement, not a restart");
});

test("history is bounded to ten minutes and holds only real samples", () => {
  let t = emptyTrack(KEY);
  for (let i = 0; i < 400; i++) t = advance(t, KEY, { reading: m({ sampledMs: T0 + i * POLL_MS }) }, T0 + i * POLL_MS);
  assert.ok(t.samples.length <= HISTORY_MS / POLL_MS + 1, `${t.samples.length}`);
  assert.ok(t.samples.every((p) => T0 + 399 * POLL_MS - p.t <= HISTORY_MS));
});

test("missing figures stay null: nothing is filled in", () => {
  const t = feed([m({ cpuPercent: null, memRaw: null, memWorkingSet: null, netRx: null, netTx: null })]);
  assert.deepEqual([t.samples[0].cpu, t.samples[0].mem, t.samples[0].rx], [null, null, null]);
  assert.equal(memoryFigures(m({ memRaw: null, memWorkingSet: null })).used, null);
});

test("memory: working set vs raw usage, and a percentage only when the container has its own limit", () => {
  assert.deepEqual(memoryFigures(m()), { used: 800, basis: "working set", percent: null, limit: null });
  assert.deepEqual(memoryFigures(m({ memWorkingSet: null })), { used: 1000, basis: "raw usage", percent: null, limit: null });
  const lim = memoryFigures(m({ memLimit: 1600 }));
  assert.equal(lim.percent, 50);
  assert.equal(memoryFigures(m({ memLimit: 0 })).percent, null);
});

test("statuses: no sample, ok, stale, stopped, absent, not-owned, unavailable are distinct, and stale values are never current", () => {
  assert.equal(view(emptyTrack(KEY), T0).status, "no-sample");
  const ok = feed([m()]);
  assert.equal(view(ok, T0 + 1000).status, "ok");
  const stale = view(ok, T0 + STALE_MS + 1);
  assert.deepEqual([stale.status, stale.current, stale.lastOk?.cpuPercent], ["stale", null, 1.5]);
  const failed = advance(ok, KEY, { error: "timed out", atMs: T0 + 3000 }, T0 + 3000);
  assert.equal(view(failed, T0 + 4000).status, "ok", "one missed poll is not yet stale");
  const stale2 = view(failed, T0 + STALE_MS + 5000);
  assert.deepEqual([stale2.status, stale2.message, stale2.current], ["stale", "timed out", null]);
  assert.equal(view(advance(emptyTrack(KEY), KEY, { error: "no engine", atMs: T0 }, T0), T0).status, "unavailable");
  for (const state of ["stopped", "absent", "not-owned", "unavailable"]) {
    const v = view(feed([m(), m({ state, sampledMs: T0 + 3000, detail: "why", cpuPercent: null })]), T0 + 3500);
    assert.deepEqual([v.status, v.current, v.message], [state, null, "why"]);
  }
  const recovered = advance(failed, KEY, { reading: m({ sampledMs: T0 + 6000 }) }, T0 + 6000);
  assert.equal(view(recovered, T0 + 6100).status, "ok");
});

test("a stopped container clears the rate baseline so a restart never computes across the gap", () => {
  const t = feed([m(), m({ state: "stopped", sampledMs: T0 + 3000 }), m({ sampledMs: T0 + 6000, netRx: 5000 })]);
  assert.equal(t.samples[t.samples.length - 1].rx, null);
});

test("chart segments break at missing points and at long gaps, and are never interpolated", () => {
  const pts = [{ t: 0, cpu: 1 }, { t: 3000, cpu: 2 }, { t: 6000, cpu: null }, { t: 9000, cpu: 4 }, { t: 40000, cpu: 5 }];
  const s = segments(pts, (p) => p.cpu);
  assert.deepEqual(s.map((x) => x.map((p) => p.v)), [[1, 2], [4], [5]]);
  assert.deepEqual(segments([], (p) => p.cpu), []);
});

test("units and uptime read naturally and refuse bad input", () => {
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(1536), "1.5 KiB");
  assert.equal(formatBytes(5 * 1024 ** 3), "5.0 GiB");
  assert.equal(formatBytes(null), "—");
  assert.equal(formatRate(2048), "2.0 KiB/s");
  assert.equal(formatRate(null), "—");
  assert.equal(formatPercent(0), "0.0%");
  assert.equal(formatPercent(null), "—");
  const now = Date.parse("2026-11-14T02:03:04Z");
  assert.equal(formatUptime("2026-11-14T00:00:00.123456789Z", now), "2 h 3 min");
  assert.equal(formatUptime("0001-01-01T00:00:00Z", now), "—");
  assert.equal(formatUptime("garbage", now), "—");
  assert.equal(formatUptime(null, now), "—");
});

test("the target key distinguishes application, deployment, container and recorded id", () => {
  const a = { app: "a", deployment: "d", container: "c", containerId: "i" };
  assert.notEqual(targetKey(a), targetKey({ ...a, deployment: "e" }));
  assert.notEqual(targetKey(a), targetKey({ ...a, containerId: null }));
});
