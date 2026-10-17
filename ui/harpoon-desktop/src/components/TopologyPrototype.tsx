import { useMemo, useState } from "react";
import { Background, Controls, Handle, Position, ReactFlow, type Edge, type Node, type NodeProps } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { ContainerTechnologyIcon } from "./ContainerTechnologyIcon";
import { inferContainerTechnology } from "../lib/containerTechnology.js";
import { TECHNOLOGY_ICONS } from "../lib/technologyIcons.js";
import { TECHNOLOGY_DISPLAY_COLORS } from "../lib/technologyIconColors.js";
import { layoutTopology, NETWORK_HEIGHT, NETWORK_WIDTH, NODE_HEIGHT, NODE_WIDTH } from "../lib/topologyPrototypeLayout";
import { makeTopology, type PrototypeContainer, type PrototypeTopology } from "../lib/topologyPrototypeFixtures";

const sizes = [2, 5, 10, 25];
const truncate = (value: string, limit = 27) => value.length > limit ? `${value.slice(0, limit - 1)}…` : value;
const containerFixture = (container: PrototypeContainer) => ({ Image: container.image, Config: { Image: container.image } });
type TopologyNodeData = { kind: "container" | "network"; label: string; fullName: string; status?: string; icon?: PrototypeContainer };
type TopologyNode = Node<TopologyNodeData, "topology">;

function GraphNode({ data }: NodeProps<TopologyNode>) {
  const isNetwork = data.kind === "network";
  const technology = data.icon ? inferContainerTechnology(containerFixture(data.icon)) : null;
  return <div className={`topology-prototype__node topology-prototype__node--${data.kind}`} title={data.fullName} aria-label={`${data.label}. Full identifier: ${data.fullName}. ${data.status ?? "Docker network"}. Network attachment metadata only.`}>
    <Handle type="target" position={Position.Left} />
    {!isNetwork && technology && <ContainerTechnologyIcon technology={technology} size={18} />}
    <div><strong>{data.label}</strong><span>{isNetwork ? "Docker network" : data.status}</span></div>
    <Handle type="source" position={Position.Right} />
  </div>;
}
const nodeTypes = { topology: GraphNode };

function prototypeGraph(topology: PrototypeTopology) {
  const graph = layoutTopology(topology);
  const nodes: TopologyNode[] = [...topology.containers.map((container) => {
    const point = graph.node(container.id);
    return { id: container.id, type: "topology" as const, position: { x: point.x - NODE_WIDTH / 2, y: point.y - NODE_HEIGHT / 2 }, data: { kind: "container" as const, label: truncate(container.service || "Unknown service"), fullName: container.fullName, status: container.status, icon: container } };
  }), ...topology.networks.map((network) => {
    const point = graph.node(network.id);
    return { id: network.id, type: "topology" as const, position: { x: point.x - NETWORK_WIDTH / 2, y: point.y - NETWORK_HEIGHT / 2 }, data: { kind: "network" as const, label: network.name, fullName: network.name } };
  })];
  const edges: Edge[] = topology.containers.flatMap((container) => container.networks.map((network) => ({ id: `${container.id}-${network}`, source: container.id, target: `network-${network}`, label: "attachment", animated: false, style: { stroke: "var(--accent-yellow)", strokeDasharray: "5 5" }, labelStyle: { fill: "var(--text-tertiary)", fontSize: 9 } })));
  return { nodes, edges };
}

function SvgPrototype({ topology }: { topology: PrototypeTopology }) {
  const graph = layoutTopology(topology);
  const width = Math.max(760, graph.graph().width ?? 760);
  const height = Math.max(300, graph.graph().height ?? 300);
  return <div className="topology-prototype__canvas topology-prototype__canvas--svg"><svg viewBox={`0 0 ${width} ${height}`} role="img" aria-labelledby="svg-prototype-title svg-prototype-desc"><title id="svg-prototype-title">Native SVG topology prototype</title><desc id="svg-prototype-desc">Docker network attachment metadata. Edges do not indicate observed traffic or communication.</desc>{topology.containers.flatMap((container) => container.networks.map((network) => { const a = graph.node(container.id); const b = graph.node(`network-${network}`); return <line key={`${container.id}-${network}`} className="topology-prototype__edge" x1={a.x + NODE_WIDTH / 2} y1={a.y} x2={b.x - NETWORK_WIDTH / 2} y2={b.y} />; }))}{topology.containers.map((container) => { const p = graph.node(container.id); const technology = inferContainerTechnology(containerFixture(container)); const mark = TECHNOLOGY_ICONS[technology.key]; const color = TECHNOLOGY_DISPLAY_COLORS[technology.key as keyof typeof TECHNOLOGY_DISPLAY_COLORS] ?? mark?.color ?? "var(--accent-yellow)"; return <g key={container.id} className="topology-prototype__svg-node" tabIndex={0} aria-label={`${container.service || "Unknown service"}. Full identifier: ${container.fullName}. Status ${container.status}. Network attachment metadata only.`}><title>{container.fullName}</title><rect x={p.x - NODE_WIDTH / 2} y={p.y - NODE_HEIGHT / 2} width={NODE_WIDTH} height={NODE_HEIGHT} rx="9" /><g transform={`translate(${p.x - NODE_WIDTH / 2 + 14} ${p.y - 24})`} aria-hidden="true">{mark ? <path d={mark.path} fill={color} transform="scale(.72)" /> : <circle cx="8" cy="8" r="7" fill={color} />}</g><text className="topology-prototype__svg-service" x={p.x - NODE_WIDTH / 2 + 38} y={p.y - 19}>{truncate(container.service || "Unknown service")}</text><text className="topology-prototype__svg-name" x={p.x - NODE_WIDTH / 2 + 14} y={p.y + 3}>{truncate(container.fullName, 30)}</text><text className="topology-prototype__svg-status" x={p.x - NODE_WIDTH / 2 + 14} y={p.y + 21}>{container.status} · {container.networks.length} network{container.networks.length === 1 ? "" : "s"}</text></g>; })}{topology.networks.map((network) => { const p = graph.node(network.id); return <g key={network.id} className="topology-prototype__svg-network" tabIndex={0} aria-label={`${network.name}. Docker network. Network attachment metadata only.`}><title>{network.name}</title><rect x={p.x - NETWORK_WIDTH / 2} y={p.y - NETWORK_HEIGHT / 2} width={NETWORK_WIDTH} height={NETWORK_HEIGHT} rx="9" /><text x={p.x - NETWORK_WIDTH / 2 + 16} y={p.y - 4}>DOCKER NETWORK</text><text x={p.x - NETWORK_WIDTH / 2 + 16} y={p.y + 18}>{truncate(network.name, 22)}</text></g>; })}</svg></div>;
}

export function TopologyPrototype() {
  const [count, setCount] = useState(5);
  const [state, setState] = useState<"populated" | "empty" | "unknown">("populated");
  const [renderer, setRenderer] = useState<"svg" | "flow">("svg");
  const topology = useMemo(() => makeTopology(count, state), [count, state]);
  const flow = useMemo(() => prototypeGraph(topology), [topology]);
  const explicitLinks = topology.containers.flatMap((container) => container.links.map((link) => `${container.service || "unknown"}: ${link}`));
  return <main className="topology-prototype" aria-labelledby="topology-prototype-title"><header className="topology-prototype__header"><div><div className="overview-eyebrow">Development-only A/B harness</div><h1 id="topology-prototype-title">Harpoon topology renderer</h1><p>Identical normalized Docker-authoritative fixtures. Attachment edges only; they do not show traffic or communication.</p></div><div className="topology-prototype__controls"><label>Renderer <select value={renderer} onChange={(event) => setRenderer(event.target.value as "svg" | "flow")}><option value="svg">A · Native SVG + Dagre</option><option value="flow">B · React Flow + Dagre</option></select></label><label>Nodes <select value={count} onChange={(event) => setCount(Number(event.target.value))}>{sizes.map((size) => <option key={size} value={size}>{size}</option>)}</select></label><label>State <select value={state} onChange={(event) => setState(event.target.value as typeof state)}><option value="populated">Populated</option><option value="unknown">Unknown / empty fields</option><option value="empty">Empty topology</option></select></label></div></header><section className="topology-prototype__legend" aria-label="Topology legend"><span><i className="topology-prototype__legend-line" /> network attachment</span><span>Explicit Docker links are metadata, not graph edges</span><span>{topology.containers.length} containers · {topology.networks.length} networks</span></section>{state === "empty" ? <div className="topology-prototype__empty" role="status"><strong>No Docker topology reported</strong><span>Unknown or empty states stay explicit; the prototype does not invent nodes.</span></div> : renderer === "svg" ? <SvgPrototype topology={topology} /> : <div className="topology-prototype__canvas topology-prototype__canvas--flow"><ReactFlow nodes={flow.nodes} edges={flow.edges} nodeTypes={nodeTypes} fitView fitViewOptions={{ padding: 0.2 }} nodesConnectable={false} nodesDraggable={false} elementsSelectable={false} proOptions={{ hideAttribution: true }}><Background color="var(--border-strong)" gap={22} /><Controls showInteractive={false} /></ReactFlow></div>}<aside className="topology-prototype__notes"><div><strong>Explicit Docker links</strong><span>{explicitLinks.length ? explicitLinks.join(" · ") : "None reported"}</span></div><div><strong>Full identifiers</strong><span>Hover a node for the full name; keyboard focus exposes the same accessible label.</span></div><div><strong>Runtime measurement</strong><span>Offline/browser-host measurements only in this harness. Tauri WKWebView CPU and interaction latency are not measured here.</span></div></aside></main>;
}
