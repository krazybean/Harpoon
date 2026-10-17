import { useMemo, useState } from "react";
import { Background, Controls, Handle, Position, ReactFlow, type Edge, type Node, type NodeProps } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { ContainerTechnologyIcon } from "./ContainerTechnologyIcon";
import { inferContainerTechnology } from "../lib/containerTechnology.js";
import { layoutTopology } from "../lib/topologyPrototypeLayout";
import type { mapContainerTopology } from "../lib/containerTopology.js";

type Topology = ReturnType<typeof mapContainerTopology>;
type TopologyNodeData = { kind: "container" | "network"; label: string; fullName: string; status?: string; image?: string | null };
type TopologyNode = Node<TopologyNodeData, "container-topology">;

const NODE_WIDTH = 240;
const NODE_HEIGHT = 78;
const NETWORK_WIDTH = 220;
const NETWORK_HEIGHT = 70;

function shortName(value: string, limit = 28) {
  return value.length > limit ? `${value.slice(0, limit - 1)}…` : value;
}

function GraphNode({ data }: NodeProps<TopologyNode>) {
  const isNetwork = data.kind === "network";
  const technology = !isNetwork ? inferContainerTechnology({ Image: data.image, Config: { Image: data.image } }) : null;
  return <div className={`container-topology__flow-node container-topology__flow-node--${data.kind}`} title={data.fullName} aria-label={`${data.label}. Full identifier: ${data.fullName}. ${data.status ?? "Docker network"}. Network attachment metadata only.`}>
    <Handle type="target" position={Position.Left} />
    {!isNetwork && technology && <ContainerTechnologyIcon technology={technology} size={18} />}
    <div><strong>{data.label}</strong><span>{isNetwork ? "Docker network" : data.status}</span></div>
    <Handle type="source" position={Position.Right} />
  </div>;
}

const nodeTypes = { "container-topology": GraphNode };

function graphFor(topology: Topology, dimmed: string | null) {
  const selected = topology.selected;
  const selectedId = "selected-container";
  const containers = selected ? [{ id: selectedId, service: topology.stack?.service || selected.name, fullName: selected.name, status: selected.status as "running" | "stopped" | "unknown", image: selected.image, technology: "unknown", networks: topology.networks.map((network) => network.name), links: [] }] : [];
  const networks = topology.networks.map((network) => ({ id: `network-${network.name}`, name: network.name }));
  const layout = layoutTopology({ containers, networks });
  const nodes: TopologyNode[] = [
    ...containers.map((container) => { const point = layout.node(container.id); return { id: container.id, type: "container-topology" as const, position: { x: point.x - NODE_WIDTH / 2, y: point.y - NODE_HEIGHT / 2 }, data: { kind: "container" as const, label: shortName(container.service), fullName: container.fullName, status: container.status, image: container.image } }; }),
    ...networks.map((network) => { const point = layout.node(network.id); return { id: network.id, type: "container-topology" as const, position: { x: point.x - NETWORK_WIDTH / 2, y: point.y - NETWORK_HEIGHT / 2 }, data: { kind: "network" as const, label: shortName(network.name), fullName: network.name } }; }),
  ];
  const status = selected?.status.toLowerCase();
  const edges: Edge[] = topology.networks.map((network) => ({ id: `${selectedId}-${network.name}`, source: selectedId, target: `network-${network.name}`, animated: status === "running", className: dimmed && dimmed !== network.name ? "is-dimmed" : undefined, data: { network: network.name }, style: { stroke: status === "running" ? "var(--accent-yellow)" : status === "stopped" ? "var(--text-tertiary)" : "var(--border-strong)" } }));
  return { nodes, edges };
}

export function ContainerTopologyDiagram({ topology, name }: { topology: Topology; name: string }) {
  const [hoveredNetwork, setHoveredNetwork] = useState<string | null>(null);
  const graph = useMemo(() => graphFor(topology, hoveredNetwork), [hoveredNetwork, topology]);
  const hasAnimatedEdges = topology.selected?.status.toLowerCase() === "running" && topology.networks.length > 0;
  return <div className="container-topology__diagram-wrap">
    <div className="container-topology__flow" role="img" aria-label={`Topology for ${name}. Docker-reported network attachments only; edges do not represent measured traffic.`}>
      <ReactFlow nodes={graph.nodes} edges={graph.edges} nodeTypes={nodeTypes} fitView fitViewOptions={{ padding: 0.24, minZoom: 0.55, maxZoom: 1.35 }} nodesConnectable={false} nodesDraggable={false} elementsSelectable={false} onEdgeMouseEnter={(_, edge) => setHoveredNetwork(String(edge.data?.network ?? null))} onEdgeMouseLeave={() => setHoveredNetwork(null)} proOptions={{ hideAttribution: true }}>
        <Background color="var(--border-strong)" gap={22} />
        <Controls showInteractive={false} aria-label="Topology zoom controls" />
      </ReactFlow>
    </div>
    <div className="container-topology__diagram-note"><span className={`container-topology__legend-line${hasAnimatedEdges ? " is-animated" : ""}`} /> {hasAnimatedEdges ? "Animated = running container attached to network · Not measured traffic" : "Verified attachment metadata · Not measured traffic"}</div>
    {!topology.networks.length && <p className="text-meta container-topology__diagram-empty">Docker did not report a network attachment for this container.</p>}
    {topology.networks.length > 0 && <p className="text-meta container-topology__diagram-empty">Other connected containers are not resolved in this view. Explicit Docker links remain separate metadata.</p>}
  </div>;
}
