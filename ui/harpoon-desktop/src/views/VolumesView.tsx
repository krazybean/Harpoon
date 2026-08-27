import { memo, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { MagnifyingGlass, Trash } from "@phosphor-icons/react";
import { fmtTs } from "../components/ShelfPrimitives";

export const VolumesView = memo(function VolumesView({
  volumes, refreshing, error, updatedAt, hasCache, actionInProgress, doAction, refresh
}: {
  volumes: any[];
  refreshing: boolean;
  error: string | null;
  updatedAt: number | null;
  hasCache: boolean;
  actionInProgress: string | null;
  doAction: (name: string, fn: () => Promise<any>) => Promise<void>;
  refresh: () => Promise<any>;
}) {
  const [detail,setDetail]=useState<string>("");
  const [confirmVolume,setConfirmVolume]=useState<any>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(()=>{ if(confirmVolume) cancelRef.current?.focus(); }, [confirmVolume]);

  if (error && !hasCache) return <div className="panel" style={{ padding: 12, background: "var(--error-bg)", borderColor: "#7F1D1D", color: "var(--error-fg)" }}>Error: {error} <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>;
  if (!hasCache && refreshing) {
    return (
      <div className="table-wrap">
        <div className="panel-header" style={{ padding: "0 12px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span className="text-panel-title">Volumes</span>
          <span className="text-meta" style={{ color: "var(--accent-cyan)" }}>• refreshing…</span>
        </div>
        <div style={{ padding: 16 }}><div aria-busy="true" style={{ display: "flex", flexDirection: "column", gap: 8 }}>{Array.from({length: 3}).map((_,i)=><div key={i} style={{ height: 12, background: "var(--panel-header)", borderRadius: 6, opacity: 0.6+ i*0.1, animation: "pulse 1.5s ease-in-out infinite" }} />)}<style>{`@keyframes pulse{0%,100%{opacity:0.6}50%{opacity:1}}`}</style></div></div>
      </div>
    );
  }
  if (!hasCache && !refreshing) return <div className="panel" style={{ padding: 16 }}>No volumes. <span className="text-meta" style={{ color: "var(--text-secondary)" }}>Create with <code className="text-code">docker --context harpoon volume create &lt;name&gt;</code>.</span> <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Refresh</button></div>;

  return (
    <div className="table-wrap" aria-busy={refreshing ? "true" : undefined}>
      <div className="panel-header" style={{ padding: "0 12px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <span className="text-panel-title">Volumes ({volumes.length}) <span style={{ fontWeight: 400, fontSize: 10, color: "var(--text-secondary)", textTransform: "none", letterSpacing: 0 }}>updated {fmtTs(updatedAt)}</span> {refreshing && <span className="text-meta" style={{ marginLeft: 6, color: "var(--accent-cyan)" }}>• refreshing…</span>}{error && hasCache && <span className="text-meta" style={{ marginLeft: 6, color: "var(--error-fg)" }}>• error</span>}</span>
        <button onClick={refresh} disabled={refreshing} className="btn btn--ghost btn--sm">Refresh</button>
      </div>
      {error && hasCache && <div style={{ padding: "8px 12px", background: "var(--error-bg)", color: "var(--error-fg)", fontSize: 12, borderBottom: "1px solid #7F1D1D" }}>Error: {error} <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>}
      <div style={{ overflowX: "auto" }}>
        <table>
          <thead className="table-head"><tr><th scope="col" className="col-vol-name">Name</th><th scope="col" className="col-vol-driver">Driver</th><th scope="col" className="col-vol-actions" style={{ textAlign: "right" }}>Actions</th></tr></thead>
          <tbody>{volumes.map((v:any)=>(
            <tr key={v.Name} className="table-row">
              <td className="col-vol-name" title={v.Name} aria-label={v.Name}><span className="cell-inner">{v.Name}</span></td>
              <td className="col-vol-driver" title={v.Driver} aria-label={v.Driver}><span className="cell-inner">{v.Driver}</span></td>
              <td className="col-vol-actions"><div className="table-actions">
                <button onClick={async ()=>{ const j=await invoke<any>("inspect_volume", { name: v.Name }); setDetail(JSON.stringify(j,null,2)); }} className="btn btn--icon btn--secondary" aria-label={`Inspect volume ${v.Name}`} data-tooltip="Inspect" title={`Inspect ${v.Name}`}><MagnifyingGlass size={16} weight="regular" /></button>
                <button disabled={actionInProgress===`rmvol-${v.Name}`} onClick={()=>setConfirmVolume(v)} className="btn btn--icon btn--destructive" aria-label={`Remove volume ${v.Name}`} data-tooltip="Remove" title={`Remove ${v.Name}`}><Trash size={16} weight="regular" /></button>
              </div></td>
            </tr>
          ))}</tbody>
        </table>
      </div>
      {confirmVolume && (
        <div className="dialog-scrim" role="presentation" onClick={()=>setConfirmVolume(null)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-volume-title" onClick={e=>e.stopPropagation()} onKeyDown={e=>{ if(e.key==="Escape") setConfirmVolume(null); }}>
            <div id="confirm-volume-title" className="dialog-title">Remove volume?</div>
            <div className="dialog-body">This will permanently remove volume <strong>{confirmVolume.Name}</strong> and delete its data. This action cannot be undone.</div>
            <div className="dialog-actions">
              <button ref={cancelRef} onClick={()=>setConfirmVolume(null)} className="btn btn--secondary">Cancel</button>
              <button onClick={()=>{ const v=confirmVolume; setConfirmVolume(null); doAction(`rmvol-${v.Name}`, ()=>invoke("remove_volume", { name: v.Name })); }} className="btn btn--destructive">Remove volume</button>
            </div>
          </div>
        </div>
      )}
      {detail && <pre className="text-code" style={{ margin: 12, background: "var(--panel-header)", color: "var(--text-primary)", padding: 10, borderRadius: 8, maxHeight: 300, overflow: "auto", border: "1px solid var(--border)" }}>{detail}<button onClick={()=>setDetail("")} className="btn btn--ghost btn--sm" style={{ float: "right" }}>Close</button></pre>}
    </div>
  );
});
