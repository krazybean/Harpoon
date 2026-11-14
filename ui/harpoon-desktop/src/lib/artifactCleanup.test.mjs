import test from "node:test";
import assert from "node:assert/strict";
import { parseCleanup, nothingToClean, freedSummary, parseCache } from "./artifactCleanup.ts";

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

test("the build cache report carries Docker's figure and the guest's measured free space separately", () => {
  assert.deepEqual(parseCache("build-cache-reclaimable-bytes: 1250"), { reclaimable: 1250, reclaimed: null, guestFreeBefore: null, guestFreeAfter: null });
  const done = parseCache("build-cache-reclaimed-bytes: 1250\nguest-free-before-bytes: 10\nguest-free-after-bytes: 20");
  assert.equal(done.reclaimed, 1250);
  assert.equal(done.guestFreeAfter - done.guestFreeBefore, 10);
});
