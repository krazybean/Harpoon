import assert from "node:assert/strict";
import test from "node:test";
import { reconcilePhase, startAvailable, stoppedPanelVisible } from "./lifecycle.ts";
import { expansionActive, expansionPanel } from "./diskCapacity.ts";

const ctx = { binaryPath: true, statusError: false };
const running = { state: "running", dockerReady: true };
const apply = (phase, status, c = ctx) => reconcilePhase(phase, status, c) ?? phase;
// feeds a series of polled statuses through the reconciler the way the App's effect does
const poll = (phase, statuses) => statuses.reduce((p, s) => apply(p, s), phase);

test("an external stop followed by an external start ends in ready, not stuck on stopped (the reported bug)", () => {
  let phase = "ready";
  phase = apply(phase, { state: "stopped" });
  assert.equal(phase, "stopped");
  assert.equal(stoppedPanelVisible(phase, { state: "stopped" }), true, "stopped really is stopped here");
  phase = poll(phase, [{ state: "starting" }, { state: "booting" }, { state: "running", dockerReady: false }]);
  assert.equal(phase, "docker_starting", "the start is followed while it happens");
  phase = apply(phase, running);
  assert.equal(phase, "ready");
});

test("a runtime that is confirmed running with Docker ready overrides a stale stopped phase at once", () => {
  assert.equal(apply("stopped", running), "ready");
  assert.equal(stoppedPanelVisible("stopped", running), false, "the Start panel is never shown against a running VM, even before the phase catches up");
  assert.equal(startAvailable(running), false);
  for (const s of ["starting", "booting", "stopping"]) assert.equal(startAvailable({ state: s }), false, s);
  assert.equal(startAvailable({ state: "stopped" }), true);
  assert.equal(startAvailable(null), true);
});

test("polling is idempotent and a Refresh with the same status changes nothing", () => {
  assert.equal(reconcilePhase("ready", running, ctx), null);
  assert.equal(poll("stopped", [running, running, running]), "ready");
  assert.equal(poll("ready", [running, running]), "ready");
});

test("explicit failures are kept unless the runtime proves otherwise", () => {
  assert.equal(apply("failed", { state: "stopped" }), "failed", "a failed start stays visible while the runtime is stopped");
  assert.equal(apply("failed", { state: "failed" }), "failed");
  assert.equal(apply("failed", { ...running, failure: { message: "x" } }), "failed", "a reported failure is not hidden");
  assert.equal(reconcilePhase("failed", running, { binaryPath: true, statusError: true }), null, "an unreadable status proves nothing");
  assert.equal(apply("failed", running), "ready", "a slow start that did succeed clears the stale failure");
  assert.equal(apply("ready", { state: "failed" }), "failed");
});

test("in-progress phases are followed to stopped and ready, and no status leaves them untouched", () => {
  assert.equal(apply("starting", { state: "stopped" }), "stopped");
  assert.equal(apply("vm_booting", running), "ready");
  assert.equal(reconcilePhase("starting", null, ctx), null);
  assert.equal(reconcilePhase("stopped", running, { binaryPath: false, statusError: false }), null, "nothing is concluded before the binary is known");
});

test("an expansion done from outside: stages while the record is active, then everything clears and normal controls return", () => {
  let phase = "ready";
  const exp = (p) => ({ state: "stopped", diskExpansion: { active: true, phase: p } });
  phase = apply(phase, exp("stopping")); // stopped with an active record: the panel is the progress list, not the Start panel
  assert.equal(phase, "stopped");
  assert.equal(stoppedPanelVisible(phase, exp("growing")), false);
  assert.equal(startAvailable(exp("growing")), false);
  assert.equal(expansionActive(exp("growing")), true);
  phase = poll(phase, [{ state: "starting", diskExpansion: { active: true, phase: "starting" } }, { state: "running", dockerReady: false, diskExpansion: { active: true, phase: "verifying" } }]);
  assert.equal(phase, "docker_starting");
  const done = { state: "running", dockerReady: true, diskLogicalBytes: 8589934592 }; // record cleared after verified success
  phase = apply(phase, done);
  assert.equal(phase, "ready");
  assert.equal(expansionPanel(done.diskExpansion), "none", "no stale expansion panel");
  assert.equal(expansionActive(done), false);
  assert.equal(startAvailable(done), false, "Start is not offered for a running VM");
  assert.equal(stoppedPanelVisible(phase, done), false);
});

test("a failed expansion leaves Start available with the recovery panel, once nothing is active", () => {
  const interrupted = { state: "stopped", diskExpansion: { active: false, phase: "growing" } };
  assert.equal(startAvailable(interrupted), true);
  assert.equal(stoppedPanelVisible("stopped", interrupted), true);
  assert.equal(expansionPanel(interrupted.diskExpansion), "interrupted");
});

import { MGMT_WARNING_AFTER_MS, degradedManagement, overlayShouldYield, withTimeout } from "./lifecycle.ts";

test("withTimeout gives up on a step that never settles and never rejects", async () => {
  assert.equal(await withTimeout(new Promise(() => {}), 20), undefined, "a hung step resolves empty after the timeout");
  assert.equal(await withTimeout(Promise.resolve(7), 1000), 7);
  assert.equal(await withTimeout(Promise.reject(new Error("x")), 1000), undefined, "a failing step does not throw into the bootstrap");
});

test("the startup overlay yields once the runtime says running with Docker ready, whatever the startup steps are doing", () => {
  for (const phase of ["launching", "discovering", "starting", "vm_booting", "docker_starting"]) {
    assert.equal(overlayShouldYield(phase, running, false), true, phase);
  }
  assert.equal(overlayShouldYield("discovering", { state: "running", dockerReady: true, mgmtReady: false }, false), true, "management being down does not hold the overlay");
  assert.equal(overlayShouldYield("discovering", { state: "running", dockerReady: false }, false), false, "Docker not ready yet: keep waiting");
  assert.equal(overlayShouldYield("discovering", { state: "stopped" }, false), false);
  assert.equal(overlayShouldYield("ready", running, false), false, "nothing to yield from");
  assert.equal(overlayShouldYield("failed", running, false), false, "a failure is not hidden by the overlay rule");
  assert.equal(overlayShouldYield("starting", running, true), false, "a restart this window is performing is not cut short");
  assert.equal(overlayShouldYield("discovering", null, false), false);
});

test("a degraded-management warning appears only for a sustained outage with Docker ready", () => {
  const down = { state: "running", dockerReady: true, mgmtReady: false };
  assert.equal(degradedManagement(down, 1000, 1000 + MGMT_WARNING_AFTER_MS - 1), false, "a booting guest is not an outage");
  assert.equal(degradedManagement(down, 1000, 1000 + MGMT_WARNING_AFTER_MS), true);
  assert.equal(degradedManagement({ ...down, mgmtReady: true }, 1000, 1e9), false);
  assert.equal(degradedManagement({ state: "running", dockerReady: true }, 1000, 1e9), false, "an older runtime that does not report it says nothing");
  assert.equal(degradedManagement({ ...down, dockerReady: false }, 1000, 1e9), false, "Docker not ready is a different problem");
  assert.equal(degradedManagement({ ...down, state: "stopped" }, 1000, 1e9), false);
  assert.equal(degradedManagement(down, null, 1e9), false, "no outage start recorded");
});
