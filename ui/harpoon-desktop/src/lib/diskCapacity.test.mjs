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

import { EXPANSION_STAGES, expansionPanel, expansionStages, interruptedText, parseReclaimResult, reclaimBlockedReason, reclaimConfirmationMessage } from "./diskCapacity.ts";

test("expansion stages follow the recorded phase, in order, with no invented progress", () => {
  assert.deepEqual(EXPANSION_STAGES.map((s) => s.label), ["Stopping Harpoon", "Growing sparse disk", "Starting Harpoon", "Verifying filesystem and Docker readiness"]);
  const states = (phase) => expansionStages({ active: true, phase }).map((s) => s.state);
  assert.deepEqual(states("stopping"), ["active", "pending", "pending", "pending"]);
  assert.deepEqual(states("growing"), ["done", "active", "pending", "pending"]);
  assert.deepEqual(states("starting"), ["done", "done", "active", "pending"]);
  assert.deepEqual(states("verifying"), ["done", "done", "done", "active"]);
  assert.deepEqual(states("from-the-future"), ["pending", "pending", "pending", "pending"], "an unknown phase marks nothing active");
  assert.deepEqual(expansionStages(undefined).map((s) => s.state), ["pending", "pending", "pending", "pending"]);
});

test("the expansion panel exists only while a record does: running, interrupted, then gone after success", () => {
  assert.equal(expansionPanel(undefined), "none", "no record after a verified success: the panel clears");
  assert.equal(expansionPanel(null), "none");
  assert.equal(expansionPanel({ active: true, phase: "growing" }), "running");
  assert.equal(expansionPanel({ active: false, phase: "growing" }), "interrupted");
  assert.equal(expansionPanel({ phase: "growing" }), "interrupted", "a record without `active` is not treated as running");
  assert.ok(expansionActive({ diskExpansion: { active: true } }) && !expansionActive({ diskExpansion: { active: false } }) && !expansionActive({}), "lifecycle controls are blocked only while running");
});

test("an interrupted expansion keeps its recovery text, and older runtimes still get something actionable", () => {
  assert.equal(interruptedText({ active: false, phase: "growing", recovery: "Run `harpoon start`." }), "Run `harpoon start`.");
  assert.match(interruptedText({ active: false, phase: "starting" }), /stopped at "starting".*harpoon disk status/s);
});

test("reclaim results parse from the runtime's JSON, and anything else becomes an error message", () => {
  const ok = parseReclaimResult('{"status":"reclaimed","message":"Reclaimed 2 GiB","reclaimedBytes":2147483648,"logicalBytes":4294967296}');
  assert.equal(ok.status, "reclaimed"); assert.equal(ok.reclaimedBytes, 2 * GIB);
  assert.equal(parseReclaimResult('{"status":"nothing-reclaimed","message":"Nothing was reclaimed","reclaimedBytes":0}').reclaimedBytes, 0, "zero is a result, not an error");
  assert.deepEqual(parseReclaimResult("HARPOON_CONTROL_BUSY: another lifecycle operation is in progress"), { status: "error", message: "HARPOON_CONTROL_BUSY: another lifecycle operation is in progress" });
  assert.equal(parseReclaimResult("").status, "error");
  assert.equal(parseReclaimResult('{"nope":1}').status, "error");
});

test("reclaim is offered only for a running provisioned disk with no expansion running", () => {
  assert.equal(reclaimBlockedReason(cap(), undefined), null);
  assert.match(reclaimBlockedReason(cap({ state: "stopped" }), undefined), /must be running/);
  assert.match(reclaimBlockedReason(cap({ provisioned: false }), undefined), /No Docker disk/);
  assert.match(reclaimBlockedReason(null, undefined), /No Docker disk/);
  assert.match(reclaimBlockedReason(cap(), { active: true, phase: "growing" }), /expansion is in progress/);
  assert.equal(reclaimBlockedReason(cap(), { active: false, phase: "growing" }), null, "an interrupted record does not block (the runtime still refuses what is unsafe)");
});

test("the reclaim confirmation states what it does not do and promises no amount", () => {
  const m = reclaimConfirmationMessage(cap({ physicalBytes: 3 * GIB }));
  assert.match(m, /4 GiB capacity stays the same/); assert.match(m, /3 GiB is currently allocated/);
  assert.match(m, /does not delete Docker images, containers or volumes/); assert.match(m, /does not restart Harpoon/);
  assert.match(m, /may be small or zero/);
  assert.doesNotMatch(m, /will (free|reclaim|return) \d/i, "no promised amount");
});
