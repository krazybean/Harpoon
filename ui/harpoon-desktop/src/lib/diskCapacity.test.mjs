import assert from "node:assert/strict";
import test from "node:test";
import { diskCapacityProblem, assessSize, confirmationMessage, customSizeError, expansionActive, dismissalFor, formatSize, noticeLevel, parseDiskCapacity, sizeArg, summary, usedPercent, GIB, MIB } from "./diskCapacity.ts";

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

// --- the runtime contract (`harpoon disk status --json`, HarpoonDiskCLI.swift `diskStatusJSON`) --------------------------------------
// Keys and shapes exactly as the Swift code writes them (sorted keys, UInt64 sizes, optional guest/hostAvailableBytes/configured/pendingExpansion).
const thresholds = { warnUsedPercent: 80, criticalUsedPercent: 92, warnAvailableBytes: GIB, criticalAvailableBytes: 384 * MIB };
const contract = {
  running: { backingPath: "/x/disk.img", configured: "12G", defaultBytes: 4 * GIB, expansionRequiresRestart: true, hostAvailableBytes: 90 * GIB, hostReserveBytes: 5 * GIB,
    logicalBytes: 12 * GIB, maxBytes: 512 * GIB, physicalBytes: 7 * GIB, pressure: "warning", provisioned: true, state: "running", thresholds,
    suggestedSizes: [16 * GIB, 32 * GIB], suggestions: [{ bytes: 16 * GIB, oversubscribed: false }, { bytes: 32 * GIB, oversubscribed: true }],
    guest: { availableBytes: 1.7 * GIB | 0, capacityBytes: 11 * GIB, usedBytes: 9 * GIB } },
  unprovisioned: { backingPath: "/x/disk.img", defaultBytes: 4 * GIB, expansionRequiresRestart: true, hostReserveBytes: 5 * GIB, logicalBytes: 0, maxBytes: 512 * GIB,
    physicalBytes: 0, pressure: "unknown", provisioned: false, state: "stopped", suggestedSizes: [], suggestions: [], thresholds },
  pending: { backingPath: "/x/disk.img", defaultBytes: 4 * GIB, expansionRequiresRestart: true, hostAvailableBytes: 50 * GIB, hostReserveBytes: 5 * GIB, logicalBytes: 8 * GIB,
    maxBytes: 512 * GIB, physicalBytes: 3 * GIB, pressure: "unknown", provisioned: true, state: "stopped", suggestedSizes: [16 * GIB], suggestions: [{ bytes: 16 * GIB, oversubscribed: false }],
    thresholds, pendingExpansion: { active: false, fromBytes: 4 * GIB, phase: "growing", recovery: "Run harpoon disk expand again.", toBytes: 8 * GIB } },
};

test("every shape the runtime actually writes is accepted, unchanged", () => {
  for (const [name, o] of Object.entries(contract)) {
    const parsed = parseDiskCapacity(JSON.stringify(o));
    assert.deepEqual(parsed, o, name);
  }
  const guestless = { ...contract.running }; delete guestless.guest;
  assert.notEqual(parseDiskCapacity(JSON.stringify({ ...guestless, pressure: "unknown" })), null, "a running VM whose guest did not answer is still a valid reply");
  for (const pressure of ["unknown", "ok", "warning", "critical"]) assert.notEqual(parseDiskCapacity(JSON.stringify({ ...contract.running, pressure })), null, pressure);
});

test("malformed or partial replies are rejected, never half-accepted", () => {
  const base = contract.running;
  const bad = (name, mutate) => { const o = JSON.parse(JSON.stringify(base)); mutate(o); assert.equal(parseDiskCapacity(JSON.stringify(o)), null, name); assert.ok(diskCapacityProblem(JSON.parse(JSON.stringify(o))), name); };
  for (const k of ["provisioned", "state", "logicalBytes", "physicalBytes", "pressure", "suggestedSizes", "maxBytes", "hostReserveBytes"]) bad(`missing ${k}`, (o) => { delete o[k]; });
  for (const p of ["OK", "severe", "", 1, null, true, ["ok"]]) bad(`pressure ${JSON.stringify(p)}`, (o) => { o.pressure = p; });
  for (const v of ["5", null, true, -1, [], {}]) {
    bad(`logicalBytes ${JSON.stringify(v)}`, (o) => { o.logicalBytes = v; });
    bad(`guest.usedBytes ${JSON.stringify(v)}`, (o) => { o.guest.usedBytes = v; });
    bad(`hostAvailableBytes ${JSON.stringify(v)}`, (o) => { o.hostAvailableBytes = v; });
  }
  bad("provisioned string", (o) => { o.provisioned = "true"; });
  bad("state number", (o) => { o.state = 3; });
  bad("negative physical", (o) => { o.physicalBytes = -5; });
  bad("suggested size string", (o) => { o.suggestedSizes = [16 * GIB, "32G"]; });
  bad("suggested sizes not a list", (o) => { o.suggestedSizes = 16 * GIB; });
  bad("negative suggestion", (o) => { o.suggestedSizes = [-1]; });
  bad("suggestion detail without oversubscribed", (o) => { o.suggestions = [{ bytes: 1 }]; });
  bad("guest not an object", (o) => { o.guest = 5; });
  bad("guest null", (o) => { o.guest = null; });
  bad("guest partial", (o) => { delete o.guest.availableBytes; });
  bad("guest used over capacity", (o) => { o.guest.usedBytes = o.guest.capacityBytes + 1; o.guest.availableBytes = 0; });
  bad("guest used plus available over capacity", (o) => { o.guest.usedBytes = 9 * GIB; o.guest.availableBytes = 3 * GIB; });
  bad("guest capacity zero", (o) => { o.guest = { capacityBytes: 0, usedBytes: 0, availableBytes: 0 }; });
  bad("guest with nothing used or free", (o) => { o.guest = { capacityBytes: GIB, usedBytes: 0, availableBytes: 0 }; });
  bad("pendingExpansion partial", (o) => { o.pendingExpansion = { phase: "growing" }; });
  bad("pendingExpansion active not boolean", (o) => { o.pendingExpansion = { phase: "growing", fromBytes: 1, toBytes: 2, recovery: "", active: "yes" }; });
});

test("NaN and Infinity cannot get through: not JSON, or an overflowing literal that parses to Infinity", () => {
  const text = JSON.stringify(contract.running);
  assert.equal(parseDiskCapacity(text.replace(`"usedBytes":${contract.running.guest.usedBytes}`, '"usedBytes":NaN')), null, "NaN is not JSON");
  assert.equal(parseDiskCapacity(text.replace(`"usedBytes":${contract.running.guest.usedBytes}`, '"usedBytes":Infinity')), null, "Infinity is not JSON");
  const overflow = text.replace(`"usedBytes":${contract.running.guest.usedBytes}`, '"usedBytes":1e999');
  assert.equal(JSON.parse(overflow).guest.usedBytes, Infinity, "precondition: the literal parses to Infinity");
  assert.equal(parseDiskCapacity(overflow), null);
  assert.equal(parseDiskCapacity(text.replace(`"logicalBytes":${contract.running.logicalBytes}`, '"logicalBytes":1e999')), null);
  assert.equal(parseDiskCapacity(text.replace(`"capacityBytes":${contract.running.guest.capacityBytes}`, '"capacityBytes":-1e999')), null);
});

test("anything accepted yields a utilization that is a real number from 0 to 100", () => {
  for (const [used, free, capacity] of [[1, 0, 1], [0, 1, 1], [5 * GIB, 5 * GIB, 10 * GIB], [9 * GIB, 0.5 * GIB, 10 * GIB], [1, 1, 2 ** 40]]) {
    const parsed = parseDiskCapacity(JSON.stringify({ ...contract.running, guest: { usedBytes: used, availableBytes: free, capacityBytes: capacity } }));
    const pct = usedPercent(parsed);
    assert.ok(Number.isFinite(pct) && pct >= 0 && pct <= 100, `${used}/${free}/${capacity} -> ${pct}`);
  }
});
