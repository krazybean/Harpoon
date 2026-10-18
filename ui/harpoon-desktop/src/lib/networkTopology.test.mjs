import test from "node:test";
import assert from "node:assert/strict";
import { mapNetworkTopology } from "./networkTopology.js";

test("maps multi-homed Docker memberships without inventing peers", () => {
  const topology = mapNetworkTopology(
    [{ ID: "net-1", Name: "frontend" }, { ID: "net-2", Name: "backend" }],
    [{ ID: "abc", Names: "/harpoon-api-1", ComposeProject: "harpoon", ComposeService: "api", ComposeNumber: "1", State: "running", Image: "node:22", NetworkNames: ["frontend", "backend"], NetworkAttachments: [{ name: "frontend", ipAddress: "172.20.0.4", gateway: "172.20.0.1" }] }, { ID: "def", Names: "/worker", State: "exited", Image: "alpine", NetworkNames: ["backend"] }],
  );
  assert.equal(topology.containers.length, 2);
  assert.deepEqual(topology.containers[0].networks, ["frontend", "backend"]);
  assert.equal(topology.containers[0].label, "api");
  assert.equal(topology.containers[0].fullName, "harpoon-api-1");
  assert.equal(topology.containers[0].attachments[0].ipAddress, "172.20.0.4");
  assert.equal(topology.networks.every((network) => !network.missing), true);
  assert.equal(topology.layout.edges().length, 3);
});

test("keeps empty and unknown membership explicit", () => {
  assert.equal(mapNetworkTopology([], []).containers.length, 0);
  const topology = mapNetworkTopology([], [{ ID: "abc", Names: "/unknown", NetworkNames: ["missing-net"] }]);
  assert.equal(topology.networks[0].missing, true);
  assert.equal(topology.containers[0].status, "unknown");
});

test("keeps Compose replicas distinguishable while preserving the canonical name", () => {
  const topology = mapNetworkTopology([], [{ ID: "abc", Names: "/proj-api-2", ComposeService: "api", ComposeNumber: "2", NetworkNames: ["default"] }]);
  assert.equal(topology.containers[0].label, "api #2");
  assert.equal(topology.containers[0].fullName, "proj-api-2");
});
