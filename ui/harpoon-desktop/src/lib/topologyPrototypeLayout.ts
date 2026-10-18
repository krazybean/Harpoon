import dagre from "dagre";
import type { PrototypeTopology } from "./topologyPrototypeFixtures";

export const NODE_WIDTH = 220;
export const NODE_HEIGHT = 84;
export const NETWORK_WIDTH = 190;
export const NETWORK_HEIGHT = 64;

export function layoutTopology(topology: PrototypeTopology) {
  const graph = new dagre.graphlib.Graph().setDefaultEdgeLabel(() => ({}));
  graph.setGraph({ rankdir: "LR", ranksep: 110, nodesep: 34, marginx: 28, marginy: 28 });
  topology.containers.forEach((container) => graph.setNode(container.id, { width: NODE_WIDTH, height: NODE_HEIGHT }));
  topology.networks.forEach((network) => graph.setNode(network.id, { width: NETWORK_WIDTH, height: NETWORK_HEIGHT }));
  topology.containers.forEach((container) => container.networks.forEach((network) => graph.setEdge(container.id, `network-${network}`, { id: `${container.id}-${network}` })));
  dagre.layout(graph);
  return graph;
}
