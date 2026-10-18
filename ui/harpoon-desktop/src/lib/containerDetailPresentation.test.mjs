import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("../views/ContainerDetailView.tsx", import.meta.url), "utf8") + readFileSync(new URL("../components/ContainerTopologyDiagram.tsx", import.meta.url), "utf8");

test("container detail exposes inspect-backed network and storage workspaces", () => {
  assert.match(source, /type DetailTab = .*"networks".*"storage"/);
  assert.match(source, /NetworkSettings\?\.Networks/);
  assert.match(source, /detail\?\.Mounts/);
  assert.match(source, /Endpoint ID/);
  assert.match(source, /Propagation/);
  assert.match(source, /onNavigate\("networks"\)/);
  assert.match(source, /onNavigate\("volumes"\)/);
});

test("container detail exposes bounded Docker-authoritative stats states", () => {
  assert.match(source, /type DetailTab = .*"stats"/);
  assert.match(source, /stats_container/);
  assert.match(source, /no automatic polling/);
  assert.match(source, /Stats unavailable while stopped/);
  assert.match(source, /Values and units are preserved from Docker's response/);
});

test("container detail preserves the honest quiet-container logs state", () => {
  assert.match(source, /logs_container/);
  assert.match(source, /\(no logs reported\)/);
  assert.match(source, /Docker log tail is bounded to the latest 500 lines \/ 64 KiB/);
});

test("container topology is a bounded diagram with an accessible inspect-backed fallback", () => {
  const diagram = readFileSync(new URL("../components/ContainerTopologyDiagram.tsx", import.meta.url), "utf8");
  assert.match(source, /"topology"/);
  assert.match(source, /mapContainerTopology\(detail, containerId\)/);
  assert.match(source, /Container metadata is stale/);
  assert.match(source, /role="alert"/);
  assert.match(source, /TopologyDiagram/);
  assert.match(diagram, /container-topology__diagram-wrap/);
  assert.match(source, /Other connected containers are not resolved in this view/);
  assert.match(source, /container-topology__list/);
  assert.match(source, /aria-pressed={topologyView === "diagram"}/);
  assert.match(source, /aria-label={`Relationships for \$\{name\}`}/);
  assert.match(source, /shared network membership does not prove communication/);
  assert.match(source, /No stack membership reported/);
  assert.match(source, /No network attachments reported/);
  assert.match(source, /No explicit Docker links reported/);
  assert.match(source, /HostConfig\.Links metadata only/);
});

test("production topology uses React Flow semantics without claiming traffic", () => {
  const diagram = readFileSync(new URL("../components/ContainerTopologyDiagram.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../styles/primitives.css", import.meta.url), "utf8");
  assert.match(diagram, /ReactFlow/);
  assert.match(diagram, /animated: status === "running"/);
  assert.match(diagram, /Not measured traffic/);
  assert.match(diagram, /nodesConnectable={false}/);
  assert.match(styles, /prefers-reduced-motion/);
  assert.match(styles, /max-width: 760px/);
  assert.match(styles, /container-topology__flow/);
});
