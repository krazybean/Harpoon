import assert from "node:assert/strict";
import test from "node:test";
import { mapContainerTopology } from "./containerTopology.js";

const populated = {
  Id: "abcdef1234567890",
  Config: { Labels: { "com.docker.compose.project": "harpoon", "com.docker.compose.service": "api" } },
  NetworkSettings: { Networks: { frontend: { EndpointID: "endpoint-1", IPAddress: "172.20.0.4", Gateway: "172.20.0.1", Aliases: ["api"] } } },
  HostConfig: { Links: ["/db:/api/db"] },
};

test("maps only inspect-backed stack, network, and link metadata", () => {
  assert.deepEqual(mapContainerTopology(populated, "abcdef"), {
    status: "ready",
    selected: { name: "Container", status: "unknown", image: null },
    stack: { project: "harpoon", service: "api" },
    networks: [{ name: "frontend", endpointId: "endpoint-1", ipAddress: "172.20.0.4", gateway: "172.20.0.1", aliases: ["api"] }],
    links: [{ source: "db", target: "/api/db" }],
    peerResolution: "unresolved",
  });
});

test("rejects stale or mismatched inspect identity", () => {
  assert.equal(mapContainerTopology({ ...populated, Id: "other" }, "abcdef").status, "stale");
  assert.equal(mapContainerTopology(null, "abcdef").status, "stale");
});

test("keeps absent metadata explicit and empty", () => {
  assert.deepEqual(mapContainerTopology({ Id: "abcdef" }, "abcdef"), {
    status: "ready",
    selected: { name: "Container", status: "unknown", image: null },
    stack: null,
    networks: [],
    links: [],
    peerResolution: "unresolved",
  });
});

test("keeps selected status and image inspect-backed", () => {
  const topology = mapContainerTopology({
    Id: "abcdef",
    Name: "/api",
    State: { Status: "running", Running: true },
    Config: { Image: "harpoon-api:dev" },
  }, "abcdef");
  assert.deepEqual(topology.selected, { name: "api", status: "running", image: "harpoon-api:dev" });
  assert.equal(topology.peerResolution, "unresolved");
});
