import assert from "node:assert/strict";
import test from "node:test";
import { clearSelection, deletePrompt, pruneSelection, runSequential, selectAll, toggleSelection } from "./resourceBatch.js";

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
