import assert from "node:assert/strict";
import test from "node:test";
import { makeTopology } from "./topologyPrototypeFixtures.ts";
import { layoutTopology } from "./topologyPrototypeLayout.ts";

test("topology prototypes share fixture sizes, long-name containment inputs, and multi-network membership", () => {
  for (const count of [2, 5, 10, 25]) {
    const topology = makeTopology(count);
    assert.equal(topology.containers.length, count);
    assert.ok(topology.containers.some((container) => container.fullName.length > 60));
    assert.ok(topology.containers.some((container) => container.networks.length > 1));
    assert.ok(layoutTopology(topology).graph().width > 0);
  }
});

test("unknown and empty states remain explicit", () => {
  assert.equal(makeTopology(5, "empty").containers.length, 0);
  assert.equal(makeTopology(5, "unknown").containers[0].service, null);
  assert.equal(makeTopology(5, "unknown").containers[0].image, null);
});
