import { memo, useEffect, useRef, useState, Fragment } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Play, Stop, ArrowClockwise, TerminalWindow, MagnifyingGlass, Trash, CaretDown, CaretRight, StackSimple } from "@phosphor-icons/react";
import { fmtTs } from "../components/ShelfPrimitives";

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
}) {
  const [detail, setDetail] = useState<string>("");
  const [confirmContainer, setConfirmContainer] = useState<any>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(()=> new Set<string>());
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(()=>{ if(confirmContainer) cancelRef.current?.focus(); }, [confirmContainer]);
  const isRunning = status?.state==="running";
  const hasRows = containers.length>0;

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

  // Compose grouping derived every render (presentation-only, no persisted state)
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

  const renderRow = (c:any) => (
    <tr key={c.ID||c.Id} className="table-row">
      <td className="col-name" title={c.Names||c.Name} aria-label={c.Names||c.Name}><span className="cell-inner">{c.Names||c.Name}</span></td>
      <td className="mono col-id" title={c.ID||c.Id} aria-label={c.ID||c.Id}><span className="cell-inner">{(c.ID||c.Id||"").slice(0,12)}</span></td>
      <td className="col-image" title={c.Image} aria-label={c.Image}><span className="cell-inner">{c.Image}</span></td>
      <td className="col-state"><span className={stateCls(c.State||c.Status)}>{c.State||c.Status}</span></td>
      <td className="col-ports text-meta" title={c.Ports||""} aria-label={c.Ports||""}><span className="cell-inner">{c.Ports||"—"}</span></td>
      <td className="col-actions"><div className="table-actions">
        <button disabled={actionInProgress===`start-${c.ID||c.Id}`} onClick={()=>doAction(`start-${c.ID||c.Id}`, ()=>invoke("start_container", { id: c.ID||c.Id }))} className="btn btn--icon btn--secondary" aria-label={`Start container ${c.Names||c.Name}`} data-tooltip="Start" title={`Start ${c.Names||c.Name}`}><Play size={16} weight="regular" /></button>
        <button disabled={actionInProgress===`stop-${c.ID||c.Id}`} onClick={()=>doAction(`stop-${c.ID||c.Id}`, ()=>invoke("stop_container", { id: c.ID||c.Id }))} className="btn btn--icon btn--secondary" aria-label={`Stop container ${c.Names||c.Name}`} data-tooltip="Stop" title={`Stop ${c.Names||c.Name}`}><Stop size={16} weight="regular" /></button>
        <button disabled={actionInProgress===`restart-${c.ID||c.Id}`} onClick={()=>doAction(`restart-${c.ID||c.Id}`, ()=>invoke("restart_container", { id: c.ID||c.Id }))} className="btn btn--icon btn--secondary" aria-label={`Restart container ${c.Names||c.Name}`} data-tooltip="Restart" title={`Restart ${c.Names||c.Name}`}><ArrowClockwise size={16} weight="regular" /></button>
        <button onClick={async ()=>{ const l=await invoke<string>("logs_container", { id: c.ID||c.Id, tail:100 }); setDetail(`logs for ${c.Names||c.Name}:\n`+l); }} className="btn btn--icon btn--secondary" aria-label={`View logs for ${c.Names||c.Name}`} data-tooltip="Logs" title={`Logs ${c.Names||c.Name}`}><TerminalWindow size={16} weight="regular" /></button>
        <button onClick={async ()=>{ const j=await invoke<any>("inspect_container", { id: c.ID||c.Id }); setDetail(JSON.stringify(j,null,2)); }} className="btn btn--icon btn--secondary" aria-label={`Inspect container ${c.Names||c.Name}`} data-tooltip="Inspect" title={`Inspect ${c.Names||c.Name}`}><MagnifyingGlass size={16} weight="regular" /></button>
        <button disabled={actionInProgress===`rm-${c.ID||c.Id}`} onClick={()=>setConfirmContainer(c)} className="btn btn--icon btn--destructive" aria-label={`Remove container ${c.Names||c.Name}`} data-tooltip="Remove" title={`Remove ${c.Names||c.Name}`}><Trash size={16} weight="regular" /></button>
      </div></td>
    </tr>
  );

  return (
    <div className="table-wrap" aria-busy={refreshing ? "true" : undefined}>
      <div className="panel-header" style={{ padding: "0 12px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <span className="text-panel-title">Containers ({containers.length}) <span style={{ fontWeight: 400, fontSize: 10, color: "var(--text-secondary)", textTransform: "none", letterSpacing: 0 }}>updated {fmtTs(updatedAt)}</span> {refreshing && <span className="text-meta" style={{ marginLeft: 6, color: "var(--accent-cyan)" }}>• refreshing…</span>}{error && hasCache && <span className="text-meta" style={{ marginLeft: 6, color: "var(--error-fg)" }}>• error</span>}</span>
        <button onClick={refresh} disabled={refreshing} className="btn btn--ghost btn--sm">Refresh</button>
      </div>
      {error && hasCache && <div style={{ padding: "8px 12px", background: "var(--error-bg)", color: "var(--error-fg)", fontSize: 12, borderBottom: "1px solid #7F1D1D" }}>Error: {error} <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>}
      <div style={{ overflowX: "auto" }}>
        <table>
          <thead className="table-head"><tr><th scope="col" className="col-name">Name</th><th scope="col" className="col-id">ID</th><th scope="col" className="col-image">Image</th><th scope="col" className="col-state">State</th><th scope="col" className="col-ports">Ports</th><th scope="col" className="col-actions" style={{ textAlign: "right" }}>Actions</th></tr></thead>
          <tbody>
            {groups.map(g => (
              <Fragment key={g.project}>
                <tr className="group-header">
                  <td colSpan={6} style={{ background: "var(--panel-header)", padding: "6px 8px", borderBottom: "1px solid var(--border)" }}>
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
      {detail && <pre className="text-code" style={{ margin: 12, background: "var(--panel-header)", color: "var(--text-primary)", padding: 10, borderRadius: 8, maxHeight: 300, overflow: "auto", border: "1px solid var(--border)" }}>{detail}<button onClick={()=>setDetail("")} className="btn btn--ghost btn--sm" style={{ float: "right" }}>Close</button></pre>}
    </div>
  );
});
