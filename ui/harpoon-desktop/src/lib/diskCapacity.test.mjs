import assert from "node:assert/strict";
import test from "node:test";
import { assessSize, confirmationMessage, customSizeError, expansionActive, dismissalFor, formatSize, noticeLevel, parseDiskCapacity, sizeArg, summary, usedPercent, GIB, MIB } from "./diskCapacity.ts";

const cap = (over = {}) => ({
  provisioned: true, state: "running", logicalBytes: 4 * GIB, physicalBytes: 2 * GIB, hostAvailableBytes: 100 * GIB,
  pressure: "warning", suggestedSizes: [8 * GIB, 16 * GIB, 32 * GIB], maxBytes: 512 * GIB, hostReserveBytes: 5 * GIB,
  guest: { capacityBytes: 4 * GIB - 100 * MIB, usedBytes: 3.2 * GIB, availableBytes: 0.7 * GIB }, ...over,
});
const HOUR = 60 * 60 * 1000;

test("nothing is shown unless Harpoon is running and the runtime reports a warning", () => {
  assert.equal(noticeLevel(null, null, 0), "none");
  assert.equal(noticeLevel(cap({ pressure: "ok" }), null, 0), "none");
  assert.equal(noticeLevel(cap({ pressure: "unknown" }), null, 0), "none");
  assert.equal(noticeLevel(cap({ state: "stopped" }), null, 0), "none");
  assert.equal(noticeLevel(cap({ provisioned: false }), null, 0), "none");
  assert.equal(noticeLevel(cap(), null, 0), "warning");
  assert.equal(noticeLevel(cap({ pressure: "critical" }), null, 0), "critical");
});

test("a dismissed warning stays quiet at the same level and size, and returns when usage climbs or escalates", () => {
  const w = cap();
  const d = dismissalFor(w, 1000);
  assert.equal(noticeLevel(w, d, 1000 + 99 * HOUR), "none", "no nagging at the same utilization, however long it takes");
  const more = cap({ guest: { capacityBytes: 4 * GIB, usedBytes: 3.45 * GIB, availableBytes: 0.55 * GIB } });
  assert.ok(Math.floor(usedPercent(more) / 5) > Math.floor(usedPercent(w) / 5));
  assert.equal(noticeLevel(more, d, 2000), "warning", "returns after usage climbs another 5 points");
  assert.equal(noticeLevel(cap({ pressure: "critical" }), d, 2000), "critical", "escalation to critical is not suppressed by a warning dismissal");
});

test("a dismissed critical notice comes back after an hour, not before, and not after the disk grows", () => {
  const c = cap({ pressure: "critical" });
  const d = dismissalFor(c, 5000);
  assert.equal(d.level, "critical");
  assert.equal(noticeLevel(c, d, 5000 + HOUR - 1), "none");
  assert.equal(noticeLevel(c, d, 5000 + HOUR), "critical");
  assert.equal(noticeLevel(cap({ pressure: "warning", logicalBytes: 8 * GIB }), d, 5001), "warning", "a grown disk is a new situation");
});

test("custom sizes are checked against growth-only, the cap and the host policy", () => {
  const c = cap();
  assert.equal(customSizeError("12", c), null);
  assert.match(customSizeError("abc", c), /whole number/);
  assert.match(customSizeError("1.5", c), /whole number/);
  assert.match(customSizeError("", c), /whole number/);
  assert.match(customSizeError("4", c), /disks only grow/);
  assert.match(customSizeError("2", c), /disks only grow/);
  assert.match(customSizeError("600", c), /growth cap/);
  assert.match(customSizeError("12", cap({ hostAvailableBytes: undefined })), /unknown/);
  assert.match(customSizeError("12", cap({ hostAvailableBytes: 5 * GIB })), /metadata/, "free space equal to the reserve leaves nothing for the grow");
});

test("a low-storage Mac keeps useful choices: sparse growth needs metadata now, the rest is a flagged worst case", () => {
  const low = cap({ hostAvailableBytes: 19 * GIB, physicalBytes: 2 * GIB });
  assert.equal(assessSize(8 * GIB, low).error, null);
  assert.equal(assessSize(8 * GIB, low).oversubscribed, false);
  const big = assessSize(32 * GIB, low);
  assert.equal(big.error, null, "32 GiB is allowed on a 19 GiB-free Mac");
  assert.equal(big.oversubscribed, true, "...and flagged: filling it needs 30 GiB, headroom is 14");
  assert.equal(big.worstCaseBytes, 30 * GIB);
  assert.match(assessSize(64 * GIB, low).error, /more than 4 times the 14 GiB/, "62 GiB worst case is beyond 4x the headroom");
  assert.equal(assessSize(16 * GIB, cap({ hostAvailableBytes: 100 * GIB })).oversubscribed, false);
  const m = confirmationMessage(low, 32 * GIB);
  assert.match(m, /would need up to 30 GiB/);
  assert.match(m, /nothing stops a guest that fills it/);
  assert.doesNotMatch(confirmationMessage(low, 8 * GIB), /would need up to/);
});

test("the controls know when an expansion is running", () => {
  assert.equal(expansionActive(null), false);
  assert.equal(expansionActive({}), false);
  assert.equal(expansionActive({ diskExpansion: { active: false, phase: "growing" } }), false, "an interrupted record is not an operation in progress");
  assert.equal(expansionActive({ diskExpansion: { active: true, phase: "starting" } }), true);
});

test("the notice and the confirmation tell the user what will happen", () => {
  const c = cap();
  assert.match(summary(c), /Docker disk is 82% full: 717 MiB free of 3\.9 GiB\./);
  assert.match(summary(cap({ guest: undefined })), /Docker disk 4 GiB/);
  const m = confirmationMessage(c, 8 * GIB);
  assert.match(m, /stop its VM/);
  assert.match(m, /from 4 GiB to 8 GiB/);
  assert.match(m, /Running containers are interrupted/);
  assert.match(m, /images, volumes and containers are kept/);
  assert.doesNotMatch(m, /roll/i, "no rollback is promised");
});

test("sizes are formatted for people and for the CLI", () => {
  assert.equal(formatSize(8 * GIB), "8 GiB");
  assert.equal(formatSize(512 * MIB), "512 MiB");
  assert.equal(formatSize(1.5 * GIB), "1.5 GiB");
  assert.equal(sizeArg(16 * GIB), "16G");
  assert.equal(sizeArg(1536 * MIB), "1536M");
});

test("the runtime's JSON is parsed defensively", () => {
  assert.equal(parseDiskCapacity("not json"), null);
  assert.equal(parseDiskCapacity("{}"), null);
  assert.equal(parseDiskCapacity("null"), null);
  const ok = parseDiskCapacity(JSON.stringify(cap()));
  assert.equal(ok.logicalBytes, 4 * GIB);
});
