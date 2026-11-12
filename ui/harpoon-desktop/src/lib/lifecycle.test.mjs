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
