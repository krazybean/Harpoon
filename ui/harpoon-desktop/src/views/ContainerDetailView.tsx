import { lazy, memo, Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, ArrowClockwise, Clipboard, Cube, MagnifyingGlass, Pulse, ShareNetwork, TerminalWindow, Database } from "@phosphor-icons/react";
import { useShelf } from "../hooks/useShelf";
import type { ContainerEngine, ResourceContext } from "../lib/navigation";
const ShellTab = lazy(() => import("./ShellTab").then((module) => ({ default: module.ShellTab })));
import { ContainerTechnologyIcon } from "../components/ContainerTechnologyIcon";
import { inferContainerTechnology } from "../lib/containerTechnology.js";
import { mapContainerTopology } from "../lib/containerTopology.js";
import { ContainerTopologyDiagram as TopologyDiagram } from "../components/ContainerTopologyDiagram";

type DetailTab = "overview" | "stats" | "shell" | "logs" | "inspect" | "networks" | "topology" | "storage";
type ContainerStats = { source: string; containerId: string; cpuPercent?: string | null; memoryUsage?: string | null; memoryPercent?: string | null; networkIo?: string | null; blockIo?: string | null; pids?: string | null };
const text = (value: unknown, fallback = "—") => typeof value === "string" && value.trim() ? value : fallback;
const firstInspect = (value: any) => Array.isArray(value) ? value[0] ?? null : value;
const displayName = (value: unknown) => text(value, "Container").replace(/^\//, "");

function statusClass(status: string) {
  const value = status.toLowerCase();
  if (value.includes("running") || value === "healthy") return "badge badge--running";
  if (value.includes("restart") || value.includes("starting") || value.includes("unhealthy")) return "badge badge--warning";
  if (value.includes("dead") || value.includes("error")) return "badge badge--error";
  return "badge badge--stopped";
}

function portRows(ports: any) {
  if (!ports || typeof ports !== "object") return [];
  return Object.entries(ports).flatMap(([containerPort, bindings]) => {
    if (!Array.isArray(bindings) || bindings.length === 0) return [{ containerPort, published: "not published" }];
    return bindings.map((binding: any) => ({ containerPort, published: `${binding.HostIp || "0.0.0.0"}:${binding.HostPort || "—"}` }));
  });
}

function networkRows(networks: any) {
  if (!networks || typeof networks !== "object") return [];
  return Object.entries(networks).map(([name, value]) => ({ name, ...(value as object) }));
}

function mountRows(mounts: any) {
  return Array.isArray(mounts) ? mounts.filter((mount) => mount && typeof mount === "object") : [];
}

function redactInspect(value: any): any {
  if (Array.isArray(value)) return value.map(redactInspect);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.entries(value).map(([key, entry]) => {
    if (key === "Env" && Array.isArray(entry)) return [key, entry.map((item) => `${String(item).split("=", 1)[0]}=[redacted]`)];
    return [key, redactInspect(entry)];
  }));
}

function DataRow({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return <div className="container-detail__data-row"><span>{label}</span><strong className={mono ? "mono" : undefined} title={value}>{value}</strong></div>;
}

export const ContainerDetailView = memo(function ContainerDetailView({ context, engine, containerId, onBack, onNavigate, actionInProgress, doAction }: {
  context: ResourceContext;
  engine: ContainerEngine;
  containerId: string;
  onBack: () => void;
  onNavigate: (nav: "volumes" | "networks") => void;
  actionInProgress?: string | null;
  doAction?: (name: string, fn: () => Promise<any>) => Promise<void>;
}) {
  const [tab, setTab] = useState<DetailTab>("overview");
  const [topologyView, setTopologyView] = useState<"diagram" | "list">("diagram");
  const [logQuery, setLogQuery] = useState("");
  const [copied, setCopied] = useState(false);
  const detailShelf = useShelf<any>(useCallback(() => invoke<any>("inspect_container", { id: containerId }), [containerId]), { ttlMs: 30000 });
  const logsShelf = useShelf<string>(useCallback(() => invoke<string>("logs_container", { id: containerId, tail: 500 }), [containerId]), { ttlMs: 15000 });
  const statsShelf = useShelf<ContainerStats>(useCallback(() => invoke<ContainerStats>("stats_container", { id: containerId }), [containerId]), { ttlMs: 2000 });

  useEffect(() => { detailShelf.refresh(); }, [detailShelf.refresh]);
  useEffect(() => { if (tab === "logs" && !logsShelf.hasCache) logsShelf.refresh(); }, [tab, logsShelf.hasCache, logsShelf.refresh]);

  const detail = firstInspect(detailShelf.data);
  const name = displayName(detail?.Name);
  const image = text(detail?.Config?.Image);
  const technology = inferContainerTechnology(detail ?? { Image: image });
  const lifecycle = text(detail?.State?.Status, detail?.State?.Running ? "running" : "unknown");
  const lifecycleState = lifecycle.toLowerCase();
  useEffect(() => { if (tab === "stats" && lifecycleState === "running" && !statsShelf.hasCache) statsShelf.refresh(); }, [tab, lifecycleState, statsShelf.hasCache, statsShelf.refresh]);
  const actionKey = (verb: string) => `${verb}-${containerId}`;
  const busy = (verb: string) => actionInProgress === actionKey(verb);
  const runLifecycle = (verb: "start" | "stop" | "restart") => {
    if (verb !== "start" && !window.confirm(`${verb === "stop" ? "Stop" : "Restart"} container “${name}”? This affects only this container in Docker context ${context}.`)) return;
    doAction?.(actionKey(verb), async () => {
      await invoke(`${verb}_container`, { id: containerId });
      await detailShelf.refresh();
    });
  };
  const health = detail?.State?.Health?.Status;
  const ports = portRows(detail?.NetworkSettings?.Ports);
  const networkAttachments = networkRows(detail?.NetworkSettings?.Networks);
  const mounts = mountRows(detail?.Mounts);
  const topology = mapContainerTopology(detail, containerId);
  const inspectJson = useMemo(() => JSON.stringify(redactInspect({ engine, context, ...detail }), null, 2), [context, detail, engine]);
  const filteredLogs = useMemo(() => {
    if (!logsShelf.data) return "";
    if (!logQuery.trim()) return logsShelf.data;
    const query = logQuery.toLowerCase();
    return logsShelf.data.split("\n").filter((line) => line.toLowerCase().includes(query)).join("\n");
  }, [logQuery, logsShelf.data]);

  const copy = async (value: string) => {
    try { await navigator.clipboard.writeText(value); setCopied(true); window.setTimeout(() => setCopied(false), 1500); } catch {}
  };
  const onTabKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
    event.preventDefault();
    const tabs: DetailTab[] = ["overview", "stats", "shell", "logs", "inspect", "networks", "topology", "storage"];
    const next = tabs[(tabs.indexOf(tab) + (event.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
    setTab(next);
    requestAnimationFrame(() => document.querySelector<HTMLButtonElement>(`[data-container-tab="${next}"]`)?.focus());
  };

  if (detailShelf.error && !detailShelf.hasCache) {
    return <section className="panel content-error" aria-labelledby="container-detail-title"><button onClick={onBack} className="btn btn--ghost btn--sm"><ArrowLeft size={16} aria-hidden="true" /> Back</button><div><h1 id="container-detail-title" className="text-panel-title">Container unavailable</h1><p className="text-body">{detailShelf.error}</p><button onClick={() => detailShelf.refresh()} className="btn btn--secondary btn--sm">Retry</button></div></section>;
  }

  return <section className="container-detail panel" aria-labelledby="container-detail-title">
    <header className="container-detail__header panel-header">
      <button onClick={onBack} className="btn btn--ghost btn--sm" aria-label="Back to containers"><ArrowLeft size={16} aria-hidden="true" /> Back</button>
      <div className="container-detail__identity"><span className="container-detail__icon" role="img" aria-label={`${technology.label} image`} title={`${technology.label} image`}><ContainerTechnologyIcon technology={technology} size={20} /></span><div><div className="overview-eyebrow">Container workspace · {context} / {engine}</div><h1 id="container-detail-title">{detail ? name : "Loading container…"}</h1><p className="text-meta mono" title={containerId}>{containerId.slice(0, 12)}</p></div></div>
      <div className="container-detail__header-meta"><span className={statusClass(lifecycle)}>{lifecycle}</span><span className="text-meta" title={image}>{image}</span></div>
    </header>
    <div className="container-detail__scopebar" aria-label="Container lifecycle controls">
      <div><strong>Container: {name}</strong><span className="text-meta">Docker · context {context} · ID {containerId.slice(0, 12)}</span></div>
      <div className="container-detail__scope-actions">
        {actionInProgress && actionInProgress.endsWith(containerId) && <span className="text-meta" role="status">{actionInProgress.split("-")[0]} in progress…</span>}
        <button className="btn btn--secondary btn--sm" disabled={!doAction || busy("start") || lifecycleState === "running"} onClick={() => runLifecycle("start")}>{busy("start") ? "Starting…" : "Start container"}</button>
        <button className="btn btn--secondary btn--sm" disabled={!doAction || busy("stop") || lifecycleState !== "running"} onClick={() => runLifecycle("stop")}>{busy("stop") ? "Stopping…" : "Stop container"}</button>
        <button className="btn btn--secondary btn--sm" disabled={!doAction || busy("restart") || lifecycleState !== "running"} onClick={() => runLifecycle("restart")}>{busy("restart") ? "Restarting…" : "Restart container"}</button>
      </div>
    </div>
    {detailShelf.error && detailShelf.hasCache && <div className="container-detail__stale" role="status">Refresh failed — showing cached container metadata. <button onClick={() => detailShelf.refresh()} className="btn btn--ghost btn--sm">Retry</button></div>}
    <div className="container-detail__tabs" role="tablist" aria-label="Container workspace tabs">
      {([["overview", "Overview", Cube], ["stats", "Stats", Pulse], ["shell", "Shell", TerminalWindow], ["logs", "Logs", TerminalWindow], ["inspect", "Inspect", MagnifyingGlass], ["networks", "Networks", ShareNetwork], ["topology", "Topology", ShareNetwork], ["storage", "Storage", Database]] as const).map(([value, label, Icon]) => <button key={value} data-container-tab={value} role="tab" aria-selected={tab === value} aria-controls={`container-panel-${value}`} className={`container-detail__tab${tab === value ? " is-active" : ""}`} onClick={() => setTab(value)} onKeyDown={onTabKeyDown}><Icon size={15} aria-hidden="true" />{label}{value === "logs" && logsShelf.stale && logsShelf.hasCache && <span className="container-detail__tab-dot" aria-label="cached logs" />}</button>)}
    </div>
    {!detail && detailShelf.refreshing && <div className="container-detail__loading" aria-busy="true"><span /><span /><span /></div>}
    {detail && tab === "overview" && <div id="container-panel-overview" role="tabpanel" className="container-detail__body">
      <div className="container-detail__overview-grid"><div className="container-detail__section"><div className="overview-section__kicker">Lifecycle</div><DataRow label="Status" value={lifecycle} /><DataRow label="Started" value={text(detail.State?.StartedAt, "Not started")} mono /><DataRow label="Finished" value={text(detail.State?.FinishedAt, "Not finished")} mono /><DataRow label="Restart policy" value={text(detail.HostConfig?.RestartPolicy?.Name, "none")} /></div><div className="container-detail__section"><div className="overview-section__kicker"><Pulse size={13} /> Health</div><DataRow label="Health" value={health ? String(health) : "No healthcheck reported"} /><DataRow label="Exit code" value={String(detail.State?.ExitCode ?? "—")} /><DataRow label="Restarts" value={String(detail.RestartCount ?? "0")} /></div></div>
      <div className="container-detail__section"><div className="container-detail__section-heading"><div className="overview-section__kicker">Published ports</div><span className="text-meta">Docker engine metadata</span></div>{ports.length ? <div className="container-detail__list">{ports.map((port, index) => <div className="container-detail__list-row" key={`${port.containerPort}-${index}`}><span className="mono">{port.published}</span><span className="text-meta">→ {port.containerPort}</span></div>)}</div> : <p className="text-meta">No published ports reported.</p>}</div>
      <div className="container-detail__section"><div className="container-detail__section-heading"><div className="overview-section__kicker">Networks</div>{networkAttachments.length > 0 && <button className="btn btn--ghost btn--sm" onClick={() => setTab("networks")}>View details</button>}</div>{networkAttachments.length ? <div className="container-detail__chips">{networkAttachments.map((network: any) => <span className="resource-chip" key={network.name}><ShareNetwork size={13} aria-hidden="true" />{network.name}</span>)}</div> : <p className="text-meta">No network attachments reported.</p>}</div>
    </div>}
    {tab === "stats" && <div id="container-panel-stats" role="tabpanel" className="container-detail__body"><div className="container-detail__toolbar"><span className="text-meta">One Docker-authoritative snapshot · no automatic polling</span><button onClick={() => statsShelf.refresh()} disabled={statsShelf.refreshing || lifecycleState !== "running"} className="btn btn--secondary btn--sm"><ArrowClockwise size={14} /> {statsShelf.refreshing ? "Sampling…" : "Refresh"}</button></div>{lifecycleState !== "running" ? <div className="container-detail__section"><strong>Stats unavailable while stopped</strong><p className="text-meta">Docker reports live CPU, memory, network, and block I/O only for a running container.</p></div> : statsShelf.error && !statsShelf.hasCache ? <div className="container-detail__error" role="alert">{statsShelf.error} <button onClick={() => statsShelf.refresh()} className="btn btn--secondary btn--sm">Retry</button></div> : !statsShelf.hasCache && statsShelf.refreshing ? <div className="container-detail__loading" aria-busy="true"><span /><span /><span /></div> : statsShelf.hasCache && statsShelf.data ? <><div className="container-detail__resource-grid"><div className="container-detail__section"><div className="overview-section__kicker">CPU</div><DataRow label="Usage" value={text(statsShelf.data.cpuPercent)} /></div><div className="container-detail__section"><div className="overview-section__kicker">Memory</div><DataRow label="Usage" value={text(statsShelf.data.memoryUsage)} /><DataRow label="Percent" value={text(statsShelf.data.memoryPercent)} /></div><div className="container-detail__section"><div className="overview-section__kicker">Network I/O</div><DataRow label="Reported" value={text(statsShelf.data.networkIo)} /></div><div className="container-detail__section"><div className="overview-section__kicker">Block I/O</div><DataRow label="Reported" value={text(statsShelf.data.blockIo)} /></div><div className="container-detail__section"><div className="overview-section__kicker">Processes</div><DataRow label="PIDs" value={text(statsShelf.data.pids)} /></div></div><p className="text-meta">Source: {statsShelf.data.source}. Values and units are preserved from Docker's response; they are not host or VM measurements.</p></> : <div className="container-detail__section"><p className="text-meta">Waiting for the first Docker stats sample.</p></div>}{statsShelf.error && statsShelf.hasCache && <div className="container-detail__stale" role="status">Refresh failed — showing the last Docker stats sample.</div>}</div>}
    {tab === "shell" && <div id="container-panel-shell" role="tabpanel" className="container-detail__body container-detail__body--shell"><Suspense fallback={<div className="panel" role="status">Loading terminal…</div>}><ShellTab context={context} containerId={containerId} running={lifecycleState === "running"} /></Suspense></div>}
    {tab === "logs" && <div id="container-panel-logs" role="tabpanel" className="container-detail__body"><div className="container-detail__toolbar"><label className="resource-search"><MagnifyingGlass size={14} aria-hidden="true" /><input value={logQuery} onChange={(event) => setLogQuery(event.target.value)} placeholder="Search logs" aria-label="Search container logs" /></label><button onClick={() => logsShelf.refresh()} disabled={logsShelf.refreshing} className="btn btn--secondary btn--sm"><ArrowClockwise size={14} /> {logsShelf.refreshing ? "Refreshing…" : "Refresh"}</button><button onClick={() => copy(logsShelf.data || "")} disabled={!logsShelf.data} className="btn btn--ghost btn--sm"><Clipboard size={14} /> {copied ? "Copied" : "Copy"}</button></div>{logsShelf.error && !logsShelf.hasCache && <div className="container-detail__error">{logsShelf.error} <button onClick={() => logsShelf.refresh()} className="btn btn--secondary btn--sm">Retry</button></div>}{logsShelf.error && logsShelf.hasCache && <div className="container-detail__stale">Refresh failed — showing cached logs.</div>}{!logsShelf.hasCache && logsShelf.refreshing && <div className="container-detail__loading" aria-busy="true"><span /><span /><span /></div>}{logsShelf.hasCache && <pre className="container-detail__code text-code">{filteredLogs || (logQuery ? "No matching log lines." : "(no logs reported)")}</pre>}<p className="text-meta">Docker log tail is bounded to the latest 500 lines / 64 KiB.</p></div>}
    {tab === "inspect" && <div id="container-panel-inspect" role="tabpanel" className="container-detail__body"><div className="container-detail__toolbar"><span className="text-meta">Environment values are redacted in this view.</span><button onClick={() => copy(inspectJson)} className="btn btn--ghost btn--sm"><Clipboard size={14} /> {copied ? "Copied" : "Copy JSON"}</button></div><pre className="container-detail__code text-code">{inspectJson}</pre></div>}
    {tab === "networks" && <div id="container-panel-networks" role="tabpanel" className="container-detail__body"><div className="container-detail__toolbar"><span className="text-meta">Docker network attachments from container inspect.</span><button onClick={() => onNavigate("networks")} className="btn btn--secondary btn--sm">Open Networks</button></div>{networkAttachments.length ? <div className="container-detail__resource-grid">{networkAttachments.map((network: any) => <div className="container-detail__section" key={network.name}><div className="container-detail__section-heading"><div className="overview-section__kicker"><ShareNetwork size={13} /> {network.name}</div><span className="text-meta">attachment</span></div><DataRow label="Endpoint ID" value={text(network.EndpointID)} mono /><DataRow label="IP address" value={text(network.IPAddress)} mono /><DataRow label="Gateway" value={text(network.Gateway)} mono /><DataRow label="MAC address" value={text(network.MacAddress)} mono /><DataRow label="Aliases" value={Array.isArray(network.Aliases) && network.Aliases.length ? network.Aliases.join(", ") : "—"} /><DataRow label="Endpoint metadata" value={network.DriverOpts && typeof network.DriverOpts === "object" ? Object.entries(network.DriverOpts).map(([key, value]) => key + "=" + String(value)).join(", ") : "—"} mono /></div>)}</div> : <div className="container-detail__section"><p className="text-meta">Docker reported no network attachments for this container.</p></div>}</div>}
    {tab === "topology" && <div id="container-panel-topology" role="tabpanel" className="container-detail__body"><div className="container-detail__toolbar"><div><div className="overview-section__kicker">Container topology · Docker-authoritative</div><span className="text-meta">Passive metadata only · shared network membership does not prove communication.</span></div><div className="container-topology__view-toggle" role="group" aria-label="Topology presentation"><button className={`btn btn--sm ${topologyView === "diagram" ? "btn--secondary" : "btn--ghost"}`} aria-pressed={topologyView === "diagram"} onClick={() => setTopologyView("diagram")}>Diagram</button><button className={`btn btn--sm ${topologyView === "list" ? "btn--secondary" : "btn--ghost"}`} aria-pressed={topologyView === "list"} onClick={() => setTopologyView("list")}>Details</button></div></div>{topology.status === "stale" ? <div className="container-detail__section" role="alert"><strong>Container metadata is stale</strong><p className="text-meta">The inspect response does not match the selected container identity. Refresh before reading relationships.</p><button onClick={() => detailShelf.refresh()} className="btn btn--secondary btn--sm">Refresh metadata</button></div> : topologyView === "diagram" ? <TopologyDiagram topology={topology} name={name} /> : <ul className="container-topology__list" aria-label={`Relationships for ${name}`}><li className="container-topology__item"><span className="container-topology__icon"><ContainerTechnologyIcon technology={technology} size={17} /></span><div><strong>{name}</strong><span className="text-meta">Selected container · Docker inspect · status {topology.selected?.status ?? "unknown"}</span></div></li><li className="container-topology__item"><ShareNetwork size={17} aria-hidden="true" /><div><strong>Stack membership</strong><span className="text-meta">{topology.stack ? `${topology.stack.project}${topology.stack.service ? ` · service ${topology.stack.service}` : ""}` : "No stack membership reported"}</span></div></li>{topology.networks.length ? topology.networks.map((network: { name: string; ipAddress: string | null; gateway: string | null }) => <li className="container-topology__item" key={network.name}><ShareNetwork size={17} aria-hidden="true" /><div><strong>Network · {network.name}</strong><span className="text-meta">{network.ipAddress ? `IP ${network.ipAddress}` : "IP address not reported"}{network.gateway ? ` · gateway ${network.gateway}` : " · gateway not reported"}</span></div></li>) : <li className="container-topology__item container-topology__item--empty"><ShareNetwork size={17} aria-hidden="true" /><div><strong>No network attachments reported</strong><span className="text-meta">Docker inspect returned no network membership for this container.</span></div></li>}{topology.links.map((link: { source: string; target: string | null }) => <li className="container-topology__item" key={`${link.source}-${link.target}`}><ShareNetwork size={17} aria-hidden="true" /><div><strong>Explicit Docker link · {link.source}</strong><span className="text-meta">HostConfig.Links metadata only · target {link.target || "not reported"}</span></div></li>)}{!topology.links.length && <li className="container-topology__item container-topology__item--empty"><ShareNetwork size={17} aria-hidden="true" /><div><strong>No explicit Docker links reported</strong><span className="text-meta">Other connected containers are not resolved from this selected-container inspect response.</span></div></li>}</ul>}</div>}
    {tab === "storage" && <div id="container-panel-storage" role="tabpanel" className="container-detail__body"><div className="container-detail__toolbar"><span className="text-meta">Mounts reported by Docker inspect. Bind paths are not named volumes.</span><button onClick={() => onNavigate("volumes")} className="btn btn--secondary btn--sm">Open Volumes</button></div>{mounts.length ? <div className="container-detail__resource-grid">{mounts.map((mount: any, index: number) => <div className="container-detail__section" key={mount.Destination || index}><div className="container-detail__section-heading"><div className="overview-section__kicker"><Database size={13} /> {text(mount.Type, "mount")}</div><span className="text-meta">{mount.Name || "un-named"}</span></div><DataRow label="Source" value={text(mount.Source)} mono /><DataRow label="Destination" value={text(mount.Destination)} mono /><DataRow label="Access mode" value={text(mount.Mode)} mono /><DataRow label="Read-only" value={typeof mount.RW === "boolean" ? (mount.RW ? "no" : "yes") : "—"} /><DataRow label="Propagation" value={text(mount.Propagation)} mono /></div>)}</div> : <div className="container-detail__section"><p className="text-meta">Docker reported no mounts for this container.</p></div>}</div>}
  </section>;
});
