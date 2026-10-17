import { memo, useEffect, useRef, useState, Fragment } from "react";
import type { CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Play, Stop, ArrowClockwise, TerminalWindow, MagnifyingGlass, Trash, CaretDown, CaretRight, CaretLeft, StackSimple, SquaresFour, Table, DotsThree, Cube, ShareNetwork, Database, FolderSimple } from "@phosphor-icons/react";
import { fmtTs } from "../components/ShelfPrimitives";
import { deletePrompt, resourceId, runSequential } from "../lib/resourceBatch";
import { useResourceSelection } from "../hooks/useResourceSelection";
import { actionSummary, containerLifecycleState } from "../lib/controlScope";
import { CsshWorkspace } from "./CsshWorkspace";
import { ContainerTechnologyIcon } from "../components/ContainerTechnologyIcon";
import { inferContainerTechnology } from "../lib/containerTechnology.js";

export const ContainersView = memo(function ContainersView({
  containers,
  refreshing,
  error,
  status,
  updatedAt,
  hasCache,
  actionInProgress,
  doAction,
  refresh,
  onOpenContainer,
}: {
  containers: any[];
  refreshing: boolean;
  error: string | null;
  status: any;
  updatedAt: number | null;
  hasCache: boolean;
  actionInProgress: string | null;
  doAction: (name: string, fn: () => Promise<any>) => Promise<void>;
  refresh: () => Promise<any>;
  onOpenContainer: (containerId: string) => void;
}) {
  const [detail, setDetail] = useState<string>("");
  const [confirmContainer, setConfirmContainer] = useState<any>(null);
  const [confirmBulkDelete, setConfirmBulkDelete] = useState<{ ids: string[]; title: string; names: string[] } | null>(null);
  const [batchResult, setBatchResult] = useState<{ message: string; failed: number } | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(()=> new Set<string>());
  const [view, setView] = useState<"table" | "stacks">("stacks");
  const [selectedStack, setSelectedStack] = useState<string | null>(null);
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const galleryRef = useRef<HTMLDivElement>(null);
  const galleryScrollTop = useRef(0);
  const stackCardRefs = useRef<Record<string, HTMLButtonElement | null>>({});
  const firstTileRef = useRef<HTMLButtonElement>(null);
  const transitionTimer = useRef<number | null>(null);
  const [stackTransition, setStackTransition] = useState<"idle" | "opening" | "closing">("idle");
  const cancelRef = useRef<HTMLButtonElement>(null);
  const selection = useResourceSelection(containers);
  useEffect(()=>{ if(confirmContainer || confirmBulkDelete) cancelRef.current?.focus(); }, [confirmContainer, confirmBulkDelete]);
  const isRunning = status?.state==="running";
  const hasRows = containers.length>0;
  const selectedContainers = containers.filter((c)=>selection.selected.has(resourceId(c)));
  const confirmLifecycle = (verb: string, names: string[], scope: string) => verb === "start" || window.confirm(`${verb === "stop" ? "Stop" : "Restart"} ${scope}? This affects ${names.length} container${names.length === 1 ? "" : "s"}: ${names.slice(0, 3).join(", ")}${names.length > 3 ? "…" : ""}.`);
  const runContainerAction = (container: any, verb: "start" | "stop" | "restart") => {
    const id = resourceId(container);
    const name = container.Names || container.Name || id;
    if (!confirmLifecycle(verb, [name], `container ${name}`)) return;
    doAction(`${verb}-${id}`, () => invoke(`${verb}_container`, { id }));
  };
  const runBatch = (command: string, verb: string, ids = [...selection.selected]) => {
    if (verb !== "started" && !confirmLifecycle(verb === "stopped" ? "stop" : "restart", selectedContainers.map((c)=>c.Names || c.Name || resourceId(c)), `${ids.length} selected containers`)) return;
    doAction("batch-containers", async ()=>{
      const result = await runSequential(ids, (id)=>invoke(command, { id }));
      selection.remove(result.succeeded);
      setBatchResult({ message: result.failed.length ? `${result.succeeded.length} of ${ids.length} containers ${verb}. ${result.failed.length} failed.` : `${result.succeeded.length} containers ${verb}.`, failed: result.failed.length });
    });
  };

  if (!isRunning) {
    return (
      <div className="panel" style={{ padding: 16 }}>
        <div className="text-body">Harpoon not running — containers unavailable. State: {status?.state ?? "—"}</div>
        <div className="text-meta" style={{ color: "var(--text-secondary)", marginTop: 4 }}>Start Harpoon from Overview {hasCache ? `• cached ${containers.length} rows stale` : ""}</div>
        {hasCache && <div className="text-meta" style={{ marginTop: 8, opacity: 0.6 }}>Cached (stale) — will refresh when Harpoon running</div>}
        <button onClick={refresh} className="btn btn--secondary" style={{ marginTop: 8 }}>Refresh</button>
      </div>
    );
  }
  // Error with cache: keep data visible, inline error
  if (error && !hasCache) {
    return <div className="panel" style={{ padding: 12, background: "var(--error-bg)", borderColor: "#7F1D1D", color: "var(--error-fg)" }}>Error: {error} <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>;
  }
  if (!hasCache && refreshing) {
    return (
      <div className="table-wrap">
        <div className="panel-header" style={{ padding: "0 12px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span className="text-panel-title">Containers</span>
          <span className="text-meta" style={{ color: "var(--accent-cyan)" }}>• refreshing…</span>
        </div>
        <div style={{ padding: 16 }}>
          <div aria-busy="true" style={{ display: "flex", flexDirection: "column", gap: 8 }}>{Array.from({length: 3}).map((_,i)=><div key={i} style={{ height: 12, background: "var(--panel-header)", borderRadius: 6, opacity: 0.6+ i*0.1, animation: "pulse 1.5s ease-in-out infinite" }} />)}<style>{`@keyframes pulse{0%,100%{opacity:0.6}50%{opacity:1}}`}</style></div>
        </div>
      </div>
    );
  }
  if (!hasCache && !refreshing) return <div className="panel" style={{ padding: 16 }}>No containers. <span className="text-meta" style={{ color: "var(--text-secondary)" }}>Try: <code>docker --context harpoon run -d --name hello nginx:alpine</code></span> <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Refresh</button></div>;
  if (hasCache && !hasRows && !refreshing) return <div className="panel" style={{ padding: 16 }}>No containers. <span className="text-meta" style={{ color: "var(--text-secondary)" }}>Try: <code>docker --context harpoon run -d --name hello nginx:alpine</code></span> <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Refresh</button></div>;
  if (hasCache && !hasRows && refreshing) {
    // keep empty state visible with subtle refreshing indicator, not skeleton
    return (
      <div className="table-wrap" aria-busy="true">
        <div className="panel-header" style={{ padding: "0 12px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span className="text-panel-title">Containers (0) <span style={{ fontWeight: 400, fontSize: 10, color: "var(--text-secondary)", textTransform: "none", letterSpacing: 0 }}>updated {fmtTs(updatedAt)}</span> <span className="text-meta" style={{ marginLeft: 6, color: "var(--accent-cyan)" }}>• refreshing…</span></span>
          <button onClick={refresh} disabled={refreshing} className="btn btn--ghost btn--sm">Refresh</button>
        </div>
        <div style={{ padding: 16 }} className="text-meta">No containers. <span style={{ color: "var(--text-secondary)" }}>Try: <code>docker --context harpoon run -d --name hello nginx:alpine</code></span></div>
      </div>
    );
  }

  const stateCls = (s:string) => {
    const v=(s||"").toLowerCase();
    if(v.includes("running")||v.includes("up")) return "badge badge--running badge--sm";
    if(v.includes("exited")||v.includes("created")) return "badge badge--stopped badge--sm";
    if(v.includes("restart")||v.includes("remov")) return "badge badge--warning badge--sm";
    if(v.includes("dead")||v.includes("error")) return "badge badge--error badge--sm";
    return "badge badge--info badge--sm";
  };
  const stateKind = containerLifecycleState;

  // Compose grouping keeps stack metadata and summaries in one place.
  const getComposeProject = (c:any): string | null => {
    const v = c.ComposeProject;
    if (typeof v === "string" && v.trim() !== "") return v.trim();
    return null;
  };
  const groups: { project: string; containers: any[] }[] = [];
  const standalone: any[] = [];
  const seen = new Map<string, number>();
  for (const c of containers) {
    const p = getComposeProject(c);
    if (p) {
      if (!seen.has(p)) { seen.set(p, groups.length); groups.push({ project: p, containers: [] }); }
      groups[seen.get(p)!].containers.push(c);
    } else {
      standalone.push(c);
    }
  }
  const toggle = (project: string) => setCollapsed(prev => {
    const next = new Set(prev);
    if (next.has(project)) next.delete(project); else next.add(project);
    return next;
  });
  const stackAction = (project: string, command: string, verb: string, stack: any[]) => {
    setOpenMenu(null);
    const summary = actionSummary(command, stack);
    if (!summary.eligibleCount) {
      setBatchResult({ message: `No eligible containers for ${verb} in ${project} (0/${summary.total} targeted).`, failed: 0 });
      return;
    }
    if (verb !== "started" && !confirmLifecycle(verb === "stopped" ? "stop" : "restart", summary.eligible.map((c) => c.Names || c.Name || resourceId(c)), `${project} stack (${summary.eligibleCount} eligible containers)`)) return;
    doAction(`stack-${command}-${project}`, async () => {
      const result = await runSequential(summary.eligible.map(resourceId), (id) => invoke(command, { id }));
      const skipped = summary.skippedCount ? ` ${summary.skippedCount} skipped as ineligible.` : "";
      setBatchResult({ message: result.failed.length ? `${result.succeeded.length}/${summary.eligibleCount} ${project} containers ${verb}; ${result.failed.length} failed.${skipped}` : `${result.succeeded.length}/${summary.eligibleCount} ${project} containers ${verb}.${skipped}`, failed: result.failed.length });
    });
  };
  const reducedMotion = () => typeof window === "undefined" || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
  const focusWithoutScroll = (target: HTMLElement | null) => target?.focus({ preventScroll: true });
  const selectStack = (project: string) => {
    galleryScrollTop.current = galleryRef.current?.scrollTop ?? 0;
    setOpenMenu(null);
    if (transitionTimer.current) window.clearTimeout(transitionTimer.current);
    setStackTransition("opening");
    setSelectedStack(project);
    if (reducedMotion()) {
      requestAnimationFrame(() => focusWithoutScroll(firstTileRef.current));
      return;
    }
    transitionTimer.current = window.setTimeout(() => {
      focusWithoutScroll(firstTileRef.current);
      setStackTransition("idle");
    }, 440);
  };
  const returnToGallery = () => {
    if (!selectedStack) return;
    if (transitionTimer.current) window.clearTimeout(transitionTimer.current);
    const card = stackCardRefs.current[selectedStack];
    if (reducedMotion()) {
      setSelectedStack(null);
      requestAnimationFrame(() => focusWithoutScroll(card));
      return;
    }
    setStackTransition("closing");
    transitionTimer.current = window.setTimeout(() => {
      setSelectedStack(null);
      setStackTransition("idle");
      requestAnimationFrame(() => focusWithoutScroll(card));
    }, 440);
  };
  useEffect(() => () => { if (transitionTimer.current) window.clearTimeout(transitionTimer.current); }, []);
  useEffect(() => {
    if (!selectedStack) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.preventDefault(); returnToGallery(); }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [selectedStack]);
  useEffect(() => {
    if (!selectedStack) requestAnimationFrame(() => { if (galleryRef.current) galleryRef.current.scrollTop = galleryScrollTop.current; });
  }, [selectedStack]);

  const renderRow = (c:any) => (
    <tr key={c.ID||c.Id} className="table-row">
      <td className="col-select"><input type="checkbox" checked={selection.selected.has(resourceId(c))} onChange={()=>selection.toggle(resourceId(c))} aria-label={`Select container ${c.Names||c.Name}`} /></td>
      <td className="col-name" title={c.Names||c.Name} aria-label={c.Names||c.Name}><button onClick={()=>onOpenContainer(c.ID||c.Id)} className="btn btn--ghost btn--sm cell-inner" style={{ padding: 0, maxWidth: "100%", textAlign: "left" }}>{c.Names||c.Name}</button></td>
      <td className="mono col-id" title={c.ID||c.Id} aria-label={c.ID||c.Id}><span className="cell-inner">{(c.ID||c.Id||"").slice(0,12)}</span></td>
      <td className="col-image" title={c.Image} aria-label={c.Image}><span className="cell-inner">{c.Image}</span></td>
      <td className="col-state"><span className={stateCls(c.State||c.Status)}>{c.State||c.Status}</span></td>
      <td className="col-ports text-meta" title={c.Ports||""} aria-label={c.Ports||""}><span className="cell-inner">{c.Ports||"—"}</span></td>
      <td className="col-actions"><div className="table-actions">
        <button disabled={actionInProgress===`start-${c.ID||c.Id}`} onClick={()=>runContainerAction(c, "start")} className="btn btn--icon btn--secondary" aria-label={`Start container ${c.Names||c.Name}`} data-tooltip="Start" title={`Start ${c.Names||c.Name}`}><Play size={16} weight="regular" /></button>
        <button disabled={actionInProgress===`stop-${c.ID||c.Id}`} onClick={()=>runContainerAction(c, "stop")} className="btn btn--icon btn--secondary" aria-label={`Stop container ${c.Names||c.Name}`} data-tooltip="Stop" title={`Stop ${c.Names||c.Name}`}><Stop size={16} weight="regular" /></button>
        <button disabled={actionInProgress===`restart-${c.ID||c.Id}`} onClick={()=>runContainerAction(c, "restart")} className="btn btn--icon btn--secondary" aria-label={`Restart container ${c.Names||c.Name}`} data-tooltip="Restart" title={`Restart ${c.Names||c.Name}`}><ArrowClockwise size={16} weight="regular" /></button>
        <button onClick={async ()=>{ const l=await invoke<string>("logs_container", { id: c.ID||c.Id, tail:100 }); setDetail(`logs for ${c.Names||c.Name}:\n`+l); }} className="btn btn--icon btn--secondary" aria-label={`View logs for ${c.Names||c.Name}`} data-tooltip="Logs" title={`Logs ${c.Names||c.Name}`}><TerminalWindow size={16} weight="regular" /></button>
        <button onClick={()=>onOpenContainer(c.ID||c.Id)} className="btn btn--icon btn--secondary" aria-label={`Inspect container ${c.Names||c.Name}`} data-tooltip="Inspect" title={`Inspect ${c.Names||c.Name}`}><MagnifyingGlass size={16} weight="regular" /></button>
        <button disabled={actionInProgress===`rm-${c.ID||c.Id}`} onClick={()=>setConfirmContainer(c)} className="btn btn--icon btn--destructive" aria-label={`Remove container ${c.Names||c.Name}`} data-tooltip="Remove" title={`Remove ${c.Names||c.Name}`}><Trash size={16} weight="regular" /></button>
      </div></td>
    </tr>
  );

  const stackCards = groups.map((group) => ({
    ...group,
    running: group.containers.filter((c) => stateKind(c) === "running").length,
    warning: group.containers.filter((c) => stateKind(c) === "warning").length,
    networks: [...new Set(group.containers.flatMap((c) => Array.isArray(c.NetworkNames) ? c.NetworkNames : []))],
    volumes: [...new Set(group.containers.flatMap((c) => Array.isArray(c.NamedVolumes) ? c.NamedVolumes.map((v:any) => v.name).filter(Boolean) : []))],
  }));
  const activeStack = selectedStack ? stackCards.find((stack) => stack.project === selectedStack) : null;

  return (
    <div className="containers-view" aria-busy={refreshing ? "true" : undefined}>
      <div className="containers-toolbar">
        <div>
          <div className="overview-eyebrow">Containers</div>
          <h1 className="containers-title">{activeStack ? activeStack.project : view === "stacks" ? "Stacks" : "Table"}</h1>
          <div className="text-meta">{activeStack ? "Compose project · Docker metadata" : "Docker containers from the active Harpoon context."}</div>
        </div>
        <div className="containers-toolbar__actions">
          {activeStack && <button className="btn btn--secondary" onClick={returnToGallery}><CaretLeft size={15} aria-hidden="true" /> Back to stacks</button>}
          <button onClick={refresh} disabled={refreshing} className="btn btn--secondary"><ArrowClockwise size={15} aria-hidden="true" /> Refresh</button>
        </div>
      </div>
      <div className="containers-view__content">
        {!activeStack && <div className="containers-view-switch" role="tablist" aria-label="Container view">
          <button role="tab" aria-selected={view === "table"} className={view === "table" ? "containers-view-switch__item is-active" : "containers-view-switch__item"} onClick={() => setView("table")}><Table size={15} aria-hidden="true" /> Table</button>
          <button role="tab" aria-selected={view === "stacks"} className={view === "stacks" ? "containers-view-switch__item is-active" : "containers-view-switch__item"} onClick={() => setView("stacks")}><SquaresFour size={15} aria-hidden="true" /> Stacks</button>
          <span className="containers-view-switch__note">Compose groups use engine metadata · no inferred stacks</span>
        </div>}
        {view === "stacks" ? (
        <div ref={galleryRef} className={`stack-gallery stack-gallery--${stackTransition}`} tabIndex={-1} onScroll={() => { galleryScrollTop.current = galleryRef.current?.scrollTop ?? 0; }}>
          <div className="stack-gallery__grid">
            {stackCards.map((group) => { const isOpen = selectedStack === group.project; return <article key={group.project} className={`stack-card${isOpen ? " is-open" : ""}`}>
              <button className="stack-card__menu" aria-label={`Actions for ${group.project}`} aria-expanded={openMenu === group.project} onClick={(event) => { event.stopPropagation(); setOpenMenu(openMenu === group.project ? null : group.project); }}><DotsThree size={18} aria-hidden="true" /></button>
              {openMenu === group.project && <div className="stack-card__menu-popover" role="menu" aria-label={`${group.project} per-container actions`} onClick={(event) => event.stopPropagation()}>
                <span className="stack-card__menu-note">Docker context: harpoon · Compose lifecycle unavailable. Actions target individual containers only.</span>
                <button role="menuitem" disabled={actionSummary("start_container", group.containers).eligibleCount === 0} onClick={() => stackAction(group.project, "start_container", "started", group.containers)}><Play size={14} aria-hidden="true" /> Start {actionSummary("start_container", group.containers).eligibleCount} eligible</button>
                <button role="menuitem" disabled={actionSummary("stop_container", group.containers).eligibleCount === 0} onClick={() => stackAction(group.project, "stop_container", "stopped", group.containers)}><Stop size={14} aria-hidden="true" /> Stop {actionSummary("stop_container", group.containers).eligibleCount} running</button>
                <button role="menuitem" disabled={actionSummary("restart_container", group.containers).eligibleCount === 0} onClick={() => stackAction(group.project, "restart_container", "restarted", group.containers)}><ArrowClockwise size={14} aria-hidden="true" /> Restart {actionSummary("restart_container", group.containers).eligibleCount} running</button>
              </div>}
              <button ref={(node) => { stackCardRefs.current[group.project] = node; }} className="stack-card__body" aria-expanded={isOpen} aria-controls={`stack-workspace-${group.project}`} onClick={() => isOpen ? undefined : selectStack(group.project)}>
                <span className="stack-card__heading"><span className="stack-card__icon"><StackSimple size={18} aria-hidden="true" /></span><span><strong>{group.project}</strong><span className="text-meta">Compose project · {group.containers.length} containers</span></span></span>
                <span className="stack-card__summary"><span className="badge badge--running">{group.running} running</span>{group.warning > 0 && <span className="badge badge--warning">{group.warning} restarting</span>}</span>
                <span className="stack-status-strip" aria-label={`${group.project}: ${group.containers.map((c) => `${c.Names || c.Name || resourceId(c)} ${stateKind(c)}`).join(", ")}`}>{group.containers.map((c) => <span key={resourceId(c)} data-state={stateKind(c)} title={`${c.Names || c.Name || resourceId(c)} · ${c.State || c.Status || "unknown"}`} />)}</span>
                <span className="stack-services">{group.containers.map((c) => <span key={resourceId(c)}><i data-state={stateKind(c)} />{c.ComposeService || c.Names || c.Name || resourceId(c)}</span>)}</span>
                <span className="stack-resources">{group.networks.map((network) => <span key={`network-${network}`}><ShareNetwork size={12} aria-hidden="true" />{network}</span>)}{group.volumes.map((volume) => <span key={`volume-${volume}`}><Database size={12} aria-hidden="true" />{volume}</span>)}{group.containers.some((c) => (c.BindMounts || []).length > 0) && <span title="Bind mounts are attached paths, not named volumes"><FolderSimple size={12} aria-hidden="true" />bind mounts</span>}</span>
              </button>
              {isOpen && <div id={`stack-workspace-${group.project}`} className="stack-workspace" aria-label={`${group.project} containers`}>
                <div className="stack-workspace__header"><div><div className="overview-eyebrow">Compose stack · main workspace</div><h2>{group.project}</h2><p>{group.containers.length} containers · Docker engine metadata</p></div><button className="btn btn--secondary" onClick={returnToGallery}>Back to stacks <span aria-hidden="true">×</span></button></div>
                {(group.networks.length > 0 || group.volumes.length > 0) && <div className="stack-resource-row">{group.networks.map((network) => <span className="resource-chip" key={`network-${network}`}><ShareNetwork size={13} aria-hidden="true" />{network}</span>)}{group.volumes.map((volume) => <span className="resource-chip" key={`volume-${volume}`}><Database size={13} aria-hidden="true" />{volume}</span>)}</div>}
                <div className="container-tile-grid">{group.containers.map((c, index) => { const id = resourceId(c); const name = c.Names || c.Name || id; const kind = stateKind(c); const technology = inferContainerTechnology(c); return <button ref={index === 0 ? firstTileRef : undefined} key={id} className="container-tile" style={{ "--tile-index": index } as CSSProperties} onClick={() => onOpenContainer(c.ID || c.Id)}><span className="container-tile__top"><span className="container-tile__technology" aria-label={`${technology.label} image`} title={`${technology.label} image`}><ContainerTechnologyIcon technology={technology} size={18} /></span><span className="container-tile__state" data-state={kind}>{kind}</span></span><strong title={name}>{name}</strong><span className="text-meta" title={c.Image || "Image unavailable"}>{c.Image || "Image unavailable"}</span><span className="text-meta mono" title={c.Ports || "No published ports"}>{c.Ports || "No published ports"}</span><span className="container-tile__attachments">{(c.NetworkNames || []).map((network:string) => <span key={network}><ShareNetwork size={11} aria-hidden="true" />{network}</span>)}{(c.NamedVolumes || []).map((volume:any) => <span key={volume.name}><Database size={11} aria-hidden="true" />{volume.name}</span>)}{(c.BindMounts || []).length > 0 && <span title="Bind mounts are attached paths, not named volumes"><FolderSimple size={11} aria-hidden="true" />{c.BindMounts.length} bind mount{c.BindMounts.length === 1 ? "" : "s"}</span>}</span></button>; })}</div>
                <CsshWorkspace project={group.project} containers={group.containers} />
              </div>}
            </article>; })}
            {standalone.map((c) => { const id = resourceId(c); const name = c.Names || c.Name || id; return <article key={id} className="stack-card stack-card--standalone">
              <button className="stack-card__menu" aria-label={`Actions for ${name}`} aria-expanded={openMenu === id} onClick={(event) => { event.stopPropagation(); setOpenMenu(openMenu === id ? null : id); }}><DotsThree size={18} aria-hidden="true" /></button>
              {openMenu === id && <div className="stack-card__menu-popover" role="menu" aria-label={`${name} actions`} onClick={(event) => event.stopPropagation()}><span className="stack-card__menu-note">Docker context: harpoon · container scope</span><button role="menuitem" disabled={stateKind(c) === "running"} onClick={() => stackAction(name, "start_container", "started", [c])}><Play size={14} aria-hidden="true" /> Start</button><button role="menuitem" disabled={stateKind(c) !== "running"} onClick={() => stackAction(name, "stop_container", "stopped", [c])}><Stop size={14} aria-hidden="true" /> Stop</button><button role="menuitem" disabled={stateKind(c) !== "running"} onClick={() => stackAction(name, "restart_container", "restarted", [c])}><ArrowClockwise size={14} aria-hidden="true" /> Restart</button></div>}
              <button className="stack-card__body" onClick={() => onOpenContainer(c.ID || c.Id)}><span className="stack-card__heading"><span className="stack-card__icon"><span className="container-technology-icon" role="img" aria-label={`${inferContainerTechnology(c).label} image`} title={`${inferContainerTechnology(c).label} image`}><ContainerTechnologyIcon technology={inferContainerTechnology(c)} size={18} /></span></span><span><strong>{name}</strong><span className="text-meta">Standalone container · {c.Image || "Image unavailable"}</span></span></span><span className="stack-card__summary"><span className="badge badge--info">{stateKind(c)}</span></span><span className="stack-services"><span><i data-state={stateKind(c)} />{c.Ports || "No published ports"}</span></span></button>
            </article>; })}
          </div>
        </div>
        ) : (
      <div className="table-wrap" aria-busy={refreshing ? "true" : undefined}>
      <div className="panel-header" style={{ padding: "0 12px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <span className="text-panel-title">Containers ({containers.length}) <span style={{ fontWeight: 400, fontSize: 10, color: "var(--text-secondary)", textTransform: "none", letterSpacing: 0 }}>updated {fmtTs(updatedAt)}</span> {refreshing && <span className="text-meta" style={{ marginLeft: 6, color: "var(--accent-cyan)" }}>• refreshing…</span>}{error && hasCache && <span className="text-meta" style={{ marginLeft: 6, color: "var(--error-fg)" }}>• error</span>}</span>
        <button onClick={refresh} disabled={refreshing} className="btn btn--ghost btn--sm">Refresh</button>
      </div>
      <div className="selection-bar">
        <span className="text-meta">{selection.selected.size} selected</span>
        <button onClick={selection.selectAll} disabled={!containers.length || Boolean(actionInProgress)} className="btn btn--secondary">Select all</button>
        <button onClick={selection.clear} disabled={!selection.selected.size || Boolean(actionInProgress)} className="btn btn--ghost">Clear selection</button>
        <button onClick={()=>runBatch("start_container", "started")} disabled={!selection.selected.size || Boolean(actionInProgress)} className="btn btn--secondary"><Play size={14} />Start</button>
        <button onClick={()=>runBatch("stop_container", "stopped")} disabled={!selection.selected.size || Boolean(actionInProgress)} className="btn btn--secondary"><Stop size={14} />Stop</button>
        <button onClick={()=>runBatch("restart_container", "restarted")} disabled={!selection.selected.size || Boolean(actionInProgress)} className="btn btn--secondary"><ArrowClockwise size={14} />Restart</button>
        <button onClick={()=>{ const prompt=deletePrompt("containers", selectedContainers.map((c)=>c.Names||c.Name)); setConfirmBulkDelete({ ids: selectedContainers.map(resourceId), ...prompt }); }} disabled={!selection.selected.size || Boolean(actionInProgress)} className="btn btn--destructive"><Trash size={14} />Delete</button>
        {batchResult && <span className="text-meta" style={{ color: batchResult.failed ? "var(--error-fg)" : "var(--success-fg)" }}>{batchResult.message}</span>}
      </div>
      {error && hasCache && <div style={{ padding: "8px 12px", background: "var(--error-bg)", color: "var(--error-fg)", fontSize: 12, borderBottom: "1px solid #7F1D1D" }}>Error: {error} <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>}
      <div style={{ overflowX: "auto" }}>
        <table>
          <thead className="table-head"><tr><th scope="col" className="col-select"><input type="checkbox" checked={containers.length>0 && selection.selected.size===containers.length} onChange={(e)=>e.target.checked ? selection.selectAll() : selection.clear()} aria-label="Select all containers" /></th><th scope="col" className="col-name">Name</th><th scope="col" className="col-id">ID</th><th scope="col" className="col-image">Image</th><th scope="col" className="col-state">State</th><th scope="col" className="col-ports">Ports</th><th scope="col" className="col-actions" style={{ textAlign: "right" }}>Actions</th></tr></thead>
          <tbody>
            {groups.map(g => (
              <Fragment key={g.project}>
                <tr className="group-header">
                  <td colSpan={7} style={{ background: "var(--panel-header)", padding: "6px 8px", borderBottom: "1px solid var(--border)" }}>
                    <button onClick={()=>toggle(g.project)} aria-expanded={!collapsed.has(g.project)} aria-label={`${collapsed.has(g.project) ? "Expand" : "Collapse"} project ${g.project}`} className="btn btn--ghost btn--sm" style={{ display: "inline-flex", alignItems: "center", gap: 6, fontWeight: 600, width: "100%", justifyContent: "flex-start" }}>
                      {collapsed.has(g.project) ? <CaretRight size={14} weight="bold" /> : <CaretDown size={14} weight="bold" />}
                      <StackSimple size={14} weight="regular" aria-hidden="true" />
                      <span>{g.project}</span>
                      <span className="text-meta" style={{ fontWeight: 400 }}>({g.containers.length})</span>
                    </button>
                  </td>
                </tr>
                {!collapsed.has(g.project) && g.containers.map((c:any)=> renderRow(c))}
              </Fragment>
            ))}
            {standalone.map((c:any)=> renderRow(c))}
          </tbody>
        </table>
      </div>
      {confirmContainer && (
        <div className="dialog-scrim" role="presentation" onClick={()=>setConfirmContainer(null)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-title" onClick={e=>e.stopPropagation()} onKeyDown={e=>{ if(e.key==="Escape") setConfirmContainer(null); }}>
            <div id="confirm-title" className="dialog-title">Remove container?</div>
            <div className="dialog-body">This will permanently remove <strong>{confirmContainer.Names||confirmContainer.Name}</strong>. This action cannot be undone.</div>
            <div className="dialog-actions">
              <button ref={cancelRef} onClick={()=>{ setConfirmContainer(null); }} className="btn btn--secondary">Cancel</button>
              <button onClick={()=>{ const c=confirmContainer; setConfirmContainer(null); doAction(`rm-${c.ID||c.Id}`, ()=>invoke("remove_container", { id: c.ID||c.Id })); }} className="btn btn--destructive">Remove container</button>
            </div>
          </div>
        </div>
      )}
      {confirmBulkDelete && (
        <div className="dialog-scrim" role="presentation" onClick={()=>setConfirmBulkDelete(null)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-bulk-title" onClick={e=>e.stopPropagation()} onKeyDown={e=>{ if(e.key==="Escape") setConfirmBulkDelete(null); }}>
            <div id="confirm-bulk-title" className="dialog-title">{confirmBulkDelete.title}</div>
            <div className="dialog-body">This will permanently remove the selected containers. This action cannot be undone.{confirmBulkDelete.names.length>0 && <><br />{confirmBulkDelete.names.map((name)=><span key={name} style={{ display: "block" }}>{name}</span>)}</>}</div>
            <div className="dialog-actions">
              <button ref={cancelRef} onClick={()=>setConfirmBulkDelete(null)} className="btn btn--secondary">Cancel</button>
              <button onClick={()=>{ const batch=confirmBulkDelete; setConfirmBulkDelete(null); runBatch("remove_container", "deleted", batch.ids); }} className="btn btn--destructive">Delete containers</button>
            </div>
          </div>
        </div>
      )}
      {detail && <pre className="text-code" style={{ margin: 12, background: "var(--panel-header)", color: "var(--text-primary)", padding: 10, borderRadius: 8, maxHeight: 300, overflow: "auto", border: "1px solid var(--border)" }}>{detail}<button onClick={()=>setDetail("")} className="btn btn--ghost btn--sm" style={{ float: "right" }}>Close</button></pre>}
      </div>
        )}
        {batchResult && (view !== "table" || activeStack) && <div className="containers-action-status" role="status" aria-live="polite">{batchResult.message}</div>}
      </div>
    </div>
  );
});
