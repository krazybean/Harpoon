import { memo, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { MagnifyingGlass, Trash } from "@phosphor-icons/react";
import { fmtTs } from "../components/ShelfPrimitives";
import { NetworkTopologyDiagram, NetworkTopologyList } from "../components/NetworkTopologyDiagram";

export const NetworksView = memo(function NetworksView({
  networks, containers, refreshing, error, updatedAt, hasCache, containersError, containersUpdatedAt, containersHasCache, actionInProgress, doAction, refresh, refreshContainers
}: {
  networks: any[];
  containers: any[];
  refreshing: boolean;
  error: string | null;
  updatedAt: number | null;
  hasCache: boolean;
  containersError: string | null;
  containersUpdatedAt: number | null;
  containersHasCache: boolean;
  actionInProgress: string | null;
  doAction: (name: string, fn: () => Promise<any>) => Promise<void>;
  refresh: () => Promise<any>;
  refreshContainers: () => Promise<any>;
}) {
  const [detail,setDetail]=useState<string>("");
  const [confirmNetwork,setConfirmNetwork]=useState<any>(null);
  const [alertMsg,setAlertMsg]=useState<string>("");
  const [view,setView]=useState<"inventory"|"topology">("inventory");
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(()=>{ if(confirmNetwork) cancelRef.current?.focus(); }, [confirmNetwork]);
  const isDefault = (n:string)=> n==="bridge"||n==="host"||n==="none";

  if (error && !hasCache) return <div className="panel" style={{ padding: 12, background: "var(--error-bg)", borderColor: "#7F1D1D", color: "var(--error-fg)" }}>Error: {error} <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>;
  if (!hasCache && refreshing) {
    return (
      <div className="table-wrap">
        <div className="panel-header" style={{ padding: "0 12px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span className="text-panel-title">Networks</span>
          <span className="text-meta" style={{ color: "var(--accent-cyan)" }}>• refreshing…</span>
        </div>
        <div style={{ padding: 16 }}><div aria-busy="true" style={{ display: "flex", flexDirection: "column", gap: 8 }}>{Array.from({length: 3}).map((_,i)=><div key={i} style={{ height: 12, background: "var(--panel-header)", borderRadius: 6, opacity: 0.6+ i*0.1, animation: "pulse 1.5s ease-in-out infinite" }} />)}<style>{`@keyframes pulse{0%,100%{opacity:0.6}50%{opacity:1}}`}</style></div></div>
      </div>
    );
  }
  if (!hasCache && !refreshing) return <div className="panel" style={{ padding: 16 }}>No networks. <span className="text-meta" style={{ color: "var(--text-secondary)" }}>Create with <code className="text-code">docker --context harpoon network create &lt;name&gt;</code>.</span> <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Refresh</button></div>;

  if (view === "topology") return <div className="table-wrap" aria-busy={refreshing ? "true" : undefined}>
    <div className="panel-header networks-view__header"><span className="text-panel-title">Network Topology <span className="text-meta">Docker-authoritative · updated {fmtTs(updatedAt)}</span></span><div className="networks-view__actions"><button className="btn btn--ghost btn--sm" aria-pressed="false" onClick={() => setView("inventory")}>Inventory</button><button className="btn btn--secondary btn--sm" aria-pressed="true">Topology</button><button onClick={() => { refresh(); refreshContainers(); }} disabled={refreshing} className="btn btn--ghost btn--sm">Refresh</button></div></div>
    {(error || containersError) && <div className="networks-view__warning" role="status">{error || containersError} · showing last available data</div>}
    <div className="networks-view__timestamp" role="status">Last successful Docker reads: networks {fmtTs(updatedAt)} · containers {fmtTs(containersUpdatedAt)}. Age alone does not indicate changed membership.</div>
    {!containersHasCache && <div className="panel" style={{ margin: 12, padding: 16 }} role="status"><strong>Container membership is unavailable</strong><p className="text-meta">Docker network inventory is available, but attached-container data has not loaded.</p><button onClick={refreshContainers} className="btn btn--secondary btn--sm">Retry container data</button></div>}
    {containersHasCache && <><NetworkTopologyDiagram networks={networks} containers={containers} /><details className="network-topology__details"><summary>Keyboard-accessible details list</summary><NetworkTopologyList networks={networks} containers={containers} /></details></>}
  </div>;

  return (
    <div className="table-wrap" aria-busy={refreshing ? "true" : undefined}>
      <div className="panel-header networks-view__header">
        <span className="text-panel-title">Networks ({networks.length}) <span style={{ fontWeight: 400, fontSize: 10, color: "var(--text-secondary)", textTransform: "none", letterSpacing: 0 }}>updated {fmtTs(updatedAt)}</span> {refreshing && <span className="text-meta" style={{ marginLeft: 6, color: "var(--accent-cyan)" }}>• refreshing…</span>}{error && hasCache && <span className="text-meta" style={{ marginLeft: 6, color: "var(--error-fg)" }}>• error</span>}</span>
        <div className="networks-view__actions"><button onClick={() => setView("inventory")} className="btn btn--secondary btn--sm" aria-pressed="true">Inventory</button><button onClick={() => setView("topology")} className="btn btn--ghost btn--sm" aria-pressed="false">Topology</button><button onClick={refresh} disabled={refreshing} className="btn btn--ghost btn--sm">Refresh</button></div>
      </div>
      {error && hasCache && <div style={{ padding: "8px 12px", background: "var(--error-bg)", color: "var(--error-fg)", fontSize: 12, borderBottom: "1px solid #7F1D1D" }}>Error: {error} <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>}
      <div style={{ overflowX: "auto" }}>
        <table>
          <thead className="table-head"><tr><th scope="col" className="col-net-name">Name</th><th scope="col" className="col-net-id">ID</th><th scope="col" className="col-net-driver">Driver</th><th scope="col" className="col-net-scope">Scope</th><th scope="col" className="col-net-actions" style={{ textAlign: "right" }}>Actions</th></tr></thead>
          <tbody>{networks.map((n:any)=>(
            <tr key={n.ID} className="table-row">
              <td className="col-net-name" title={n.Name} aria-label={n.Name}><span className="cell-inner">{n.Name} {isDefault(n.Name) ? <span className="badge badge--info badge--sm" style={{ marginLeft: 6 }}>default</span> : null}</span></td>
              <td className="mono col-net-id" title={n.ID||""} aria-label={n.ID||""}><span className="cell-inner">{(n.ID||"").slice(0,12)}</span></td>
              <td className="col-net-driver" title={n.Driver} aria-label={n.Driver}><span className="cell-inner">{n.Driver}</span></td>
              <td className="col-net-scope"><span className="badge badge--sm" style={{ background: "var(--panel-header)", color: "var(--text-secondary)", borderColor: "var(--border)" }}>{n.Scope||"—"}</span></td>
              <td className="col-net-actions"><div className="table-actions">
                <button onClick={async ()=>{ const j=await invoke<any>("inspect_network", { name: n.Name }); setDetail(JSON.stringify(j,null,2)); }} className="btn btn--icon btn--secondary" aria-label={`Inspect network ${n.Name}`} data-tooltip="Inspect" title={`Inspect ${n.Name}`}><MagnifyingGlass size={16} weight="regular" /></button>
                <button disabled={actionInProgress===`rmnet-${n.Name}` || isDefault(n.Name)} onClick={()=>{
                  if(isDefault(n.Name)){ setAlertMsg(`Cannot remove default network "${n.Name}"`); return; }
                  setConfirmNetwork(n);
                }} className="btn btn--icon btn--destructive" aria-label={`Remove network ${n.Name}`} data-tooltip={isDefault(n.Name) ? "Cannot remove default network" : "Remove"} title={isDefault(n.Name) ? `Cannot remove default network "${n.Name}"` : `Remove ${n.Name}`}><Trash size={16} weight="regular" /></button>
              </div></td>
            </tr>
          ))}</tbody>
        </table>
      </div>
      {confirmNetwork && (
        <div className="dialog-scrim" role="presentation" onClick={()=>setConfirmNetwork(null)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-net-title" onClick={e=>e.stopPropagation()} onKeyDown={e=>{ if(e.key==="Escape") setConfirmNetwork(null); }}>
            <div id="confirm-net-title" className="dialog-title">Remove network?</div>
            <div className="dialog-body">This will permanently remove network <strong>{confirmNetwork.Name}</strong>. This action cannot be undone.</div>
            <div className="dialog-actions">
              <button ref={cancelRef} onClick={()=>setConfirmNetwork(null)} className="btn btn--secondary">Cancel</button>
              <button onClick={()=>{ const n=confirmNetwork; setConfirmNetwork(null); doAction(`rmnet-${n.Name}`, ()=>invoke("remove_network", { name: n.Name })); }} className="btn btn--destructive">Remove network</button>
            </div>
          </div>
        </div>
      )}
      {alertMsg && (
        <div className="dialog-scrim" role="presentation" onClick={()=>setAlertMsg("")}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="alert-net-title" onClick={e=>e.stopPropagation()} onKeyDown={e=>{ if(e.key==="Escape") setAlertMsg(""); }}>
            <div id="alert-net-title" className="dialog-title">Cannot remove network</div>
            <div className="dialog-body">{alertMsg}</div>
            <div className="dialog-actions"><button onClick={()=>setAlertMsg("")} className="btn btn--secondary">OK</button></div>
          </div>
        </div>
      )}
      {detail && <pre className="text-code" style={{ margin: 12, background: "var(--panel-header)", color: "var(--text-primary)", padding: 10, borderRadius: 8, maxHeight: 300, overflow: "auto", border: "1px solid var(--border)" }}>{detail}<button onClick={()=>setDetail("")} className="btn btn--ghost btn--sm" style={{ float: "right" }}>Close</button></pre>}
    </div>
  );
});
