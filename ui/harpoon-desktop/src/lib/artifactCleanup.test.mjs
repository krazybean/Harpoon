import test from "node:test";
import assert from "node:assert/strict";
import { parseCleanup, nothingToClean, freedSummary } from "./artifactCleanup.ts";

const DRY = `cleanup of Harpoon-owned artifacts — dry run: nothing will be changed
protected: deployment dep_B (serves now)
would: remove container hp-dep_A (id abc) of dep_A
would: release port lease 38000 (ledger only)
would: remove image sha256:1 (id sha256:1) of dep_A
kept: image sha256:2 of dep_C: a protected deployment records the same image
cleanup: would remove 1 container(s)...
reclaimed-containers: 1
reclaimed-images: 1`;

test("a dry run lists what would go, what is protected and what is kept, without claiming anything was removed", () => {
  const r = parseCleanup(DRY);
  assert.equal(r.actions.length, 3);
  assert.ok(r.actions.every((a) => !a.startsWith("would")));
  assert.equal(r.protectedCount, 1);
  assert.deepEqual(r.kept, ["image sha256:2 of dep_C: a protected deployment records the same image"]);
  assert.equal(nothingToClean(parseCleanup("reclaimed-containers: 0\nreclaimed-images: 0")), true);
});

test("freed space is the guest filesystem's own before/after, never a sum of image sizes", () => {
  const applied = parseCleanup(`${DRY}\nguest-free-before-bytes: 1000\nguest-free-after-bytes: 4000`);
  const size = (b) => `${b}B`;
  assert.equal(freedSummary(applied, size), "Guest free space: 1000B before, 4000B after (+3000B).");
  assert.match(freedSummary(parseCleanup("guest-free: not measurable"), size), /could not be measured/);
});
