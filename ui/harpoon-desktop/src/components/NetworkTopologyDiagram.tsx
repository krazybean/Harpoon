import { useMemo } from "react";
import { Background, Controls, Handle, Position, ReactFlow, type Edge, type Node, type NodeProps } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { ShareNetwork } from "@phosphor-icons/react";
import { ContainerTechnologyIcon } from "./ContainerTechnologyIcon";
import { inferContainerTechnology } from "../lib/containerTechnology.js";
import { mapNetworkTopology } from "../lib/networkTopology.js";

type Topology = ReturnType<typeof mapNetworkTopology>;
type NodeData = { kind: "container" | "network"; label: string; fullName: string; status?: string; image?: string | null; missing?: boolean };
type TopologyNode = Node<NodeData, "network-topology">;

const shortName = (value: string, limit = 28) => value.length > limit ? `${value.slice(0, limit - 1)}…` : value;

function GraphNode({ data }: NodeProps<TopologyNode>) {
  const isNetwork = data.kind === "network";
  const technology = !isNetwork ? inferContainerTechnology({ Image: data.image, Config: { Image: data.image } }) : null;
  return <div tabIndex={0} className={`network-topology__node network-topology__node--${data.kind}`} title={data.fullName} aria-label={`${data.label}. Full identifier: ${data.fullName}. ${data.status ?? "Docker network"}. Network attachment metadata only.`}>
    <Handle type="target" position={Position.Left} />
    {isNetwork ? <ShareNetwork size={18} aria-hidden="true" /> : technology && <ContainerTechnologyIcon technology={technology} size={18} />}
    <div><strong>{data.label}</strong><span>{isNetwork ? data.missing ? "Not in current network inventory" : "Docker network" : data.status}</span></div>
    <Handle type="source" position={Position.Right} />
  </div>;
}

const nodeTypes = { "network-topology": GraphNode };

function graphFor(topology: Topology) {
  const nodes: TopologyNode[] = [
    ...topology.containers.map((container) => {
      const point = topology.layout.node(container.id);
      return { id: container.id, type: "network-topology" as const, position: { x: point.x - 240 / 2, y: point.y - 78 / 2 }, data: { kind: "container" as const, label: shortName(container.name), fullName: container.fullName, status: container.status, image: container.image } };
    }),
    ...topology.networks.map((network) => {
      const point = topology.layout.node(network.id);
      return { id: network.id, type: "network-topology" as const, position: { x: point.x - 220 / 2, y: point.y - 70 / 2 }, data: { kind: "network" as const, label: shortName(network.name), fullName: network.name, missing: network.missing } };
    }),
  ];
  const edges: Edge[] = topology.containers.flatMap((container) => container.networks.map((network) => {
    const networkNode = topology.networks.find((candidate) => candidate.name === network);
    const running = container.status === "running" && !networkNode?.missing;
    return { id: `${container.id}-${network}`, source: container.id, target: networkNode?.id ?? `network-${network}`, label: "attachment", animated: running, className: networkNode?.missing ? "is-unresolved" : undefined, style: { stroke: running ? "var(--accent-yellow)" : "var(--text-tertiary)" } };
  }));
  return { nodes, edges };
}

export function NetworkTopologyDiagram({ networks, containers }: { networks: any[]; containers: any[] }) {
  const topology = useMemo(() => mapNetworkTopology(networks, containers), [networks, containers]);
  const graph = useMemo(() => graphFor(topology), [topology]);
  const hasAnimatedEdges = graph.edges.some((edge) => edge.animated);
  return <div className="network-topology__diagram-wrap">
    <div className="network-topology__flow" role="img" aria-label="Network topology. Docker-reported network membership only; edges do not represent measured traffic or communication.">
      <ReactFlow nodes={graph.nodes} edges={graph.edges} nodeTypes={nodeTypes} fitView fitViewOptions={{ padding: 0.2, minZoom: 0.45, maxZoom: 1.35 }} nodesConnectable={false} nodesDraggable={false} elementsSelectable={false} proOptions={{ hideAttribution: true }}>
        <Background color="var(--border-strong)" gap={22} />
        <Controls showInteractive={false} aria-label="Network topology zoom controls" />
      </ReactFlow>
    </div>
    <div className="network-topology__note"><span className={`network-topology__legend-line${hasAnimatedEdges ? " is-animated" : ""}`} /> {hasAnimatedEdges ? "Animated = running container attached to network · Not measured traffic" : "Verified Docker membership · Not measured traffic"}</div>
    {!graph.edges.length && <p className="text-meta network-topology__empty">Docker reported no attached containers for the current network inventory.</p>}
  </div>;
}

export function NetworkTopologyList({ networks, containers }: { networks: any[]; containers: any[] }) {
  const topology = useMemo(() => mapNetworkTopology(networks, containers), [networks, containers]);
  return <ul className="network-topology__list" aria-label="Docker network membership details">
    {topology.networks.map((network) => {
      const members = topology.containers.filter((container) => container.networks.includes(network.name));
      return <li key={network.id}><ShareNetwork size={17} aria-hidden="true" /><div><strong>{network.name}</strong><span className="text-meta">{network.missing ? "Not in current network inventory · " : "Docker network · "}{members.length ? `${members.length} attached container${members.length === 1 ? "" : "s"}: ${members.map((member) => member.name).join(", ")}` : "No attached containers reported"}</span></div></li>;
    })}
  </ul>;
}
