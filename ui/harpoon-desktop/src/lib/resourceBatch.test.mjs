import assert from "node:assert/strict";
import test from "node:test";
import { clearSelection, deletePrompt, pruneSelection, runSequential, selectAll, toggleSelection } from "./resourceBatch.js";
import { containerDestination } from "./navigation.js";
import { actionSummary, containerLifecycleState, eligibleMembers } from "./controlScope.js";

const resources = [{ ID: "one" }, { ID: "two" }, { ID: "three" }];

test("selection supports individual, all, clear, isolated, and stale resources", () => {
  const one = toggleSelection(clearSelection(), "one");
  assert.deepEqual([...one], ["one"]);
  assert.deepEqual([...selectAll(resources)], ["one", "two", "three"]);
  assert.deepEqual([...clearSelection()], []);
  assert.deepEqual([...pruneSelection(new Set(["one", "gone"]), resources)], ["one"]);
  assert.deepEqual([...selectAll([{ ID: "image" }])], ["image"]);
});

test("batch dispatch is sequential, retains failures, and uses one delete prompt", async () => {
  const calls = [];
  const result = await runSequential(["one", "two", "three"], async (id) => {
    calls.push(id);
    if (id === "two") throw new Error("in use");
  });
  assert.deepEqual(calls, ["one", "two", "three"]);
  assert.deepEqual(result.succeeded, ["one", "three"]);
  assert.deepEqual(result.failed.map(({ id })=>id), ["two"]);
  assert.deepEqual([...pruneSelection(new Set(result.failed.map(({ id })=>id)), resources)], ["two"]);
  assert.deepEqual(deletePrompt("images", ["a", "b"]), { title: "Delete 2 images?", names: ["a", "b"] });
  assert.deepEqual(deletePrompt("containers", ["a", "b", "c", "d", "e", "f", "g"]), { title: "Delete 7 containers?", names: [] });
});

test("container navigation preserves engine context and identity", () => {
  assert.deepEqual(containerDestination("harpoon", "abc123"), {
    kind: "container",
    engine: "docker",
    context: "harpoon",
    containerId: "abc123",
  });
});

test("control scope only targets eligible containers", () => {
  const containers = [{ ID: "up", State: "running" }, { ID: "down", State: "exited" }, { ID: "restarting", State: "restarting" }];
  assert.equal(containerLifecycleState(containers[0]), "running");
  assert.deepEqual(eligibleMembers("start_container", containers).map(({ ID }) => ID), ["down", "restarting"]);
  assert.deepEqual(eligibleMembers("stop_container", containers).map(({ ID }) => ID), ["up"]);
  assert.deepEqual(eligibleMembers("restart_container", containers).map(({ ID }) => ID), ["up"]);
  assert.deepEqual(actionSummary("stop_container", containers), { eligible: [containers[0]], eligibleCount: 1, skippedCount: 2, total: 3 });
});
