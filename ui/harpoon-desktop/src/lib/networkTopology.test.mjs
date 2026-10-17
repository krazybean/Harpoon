import test from "node:test";
import assert from "node:assert/strict";
import { mapNetworkTopology } from "./networkTopology.js";

test("maps multi-homed Docker memberships without inventing peers", () => {
  const topology = mapNetworkTopology(
    [{ ID: "net-1", Name: "frontend" }, { ID: "net-2", Name: "backend" }],
    [{ ID: "abc", Names: "/api", State: "running", Image: "node:22", NetworkNames: ["frontend", "backend"] }, { ID: "def", Names: "/worker", State: "exited", NetworkNames: ["backend"] }],
  );
  assert.equal(topology.containers.length, 2);
  assert.deepEqual(topology.containers[0].networks, ["frontend", "backend"]);
  assert.equal(topology.networks.every((network) => !network.missing), true);
  assert.equal(topology.layout.edges().length, 3);
});

test("keeps empty and unknown membership explicit", () => {
  assert.equal(mapNetworkTopology([], []).containers.length, 0);
  const topology = mapNetworkTopology([], [{ ID: "abc", Names: "/unknown", NetworkNames: ["missing-net"] }]);
  assert.equal(topology.networks[0].missing, true);
  assert.equal(topology.containers[0].status, "unknown");
});
