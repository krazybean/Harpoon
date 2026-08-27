import { memo, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { MagnifyingGlass, Trash } from "@phosphor-icons/react";
import { fmtTs } from "../components/ShelfPrimitives";

export const ImagesView = memo(function ImagesView({
  images, refreshing, error, updatedAt, hasCache, actionInProgress, doAction, refresh
}: {
  images: any[];
  refreshing: boolean;
  error: string | null;
  updatedAt: number | null;
  hasCache: boolean;
  actionInProgress: string | null;
  doAction: (name: string, fn: () => Promise<any>) => Promise<void>;
  refresh: () => Promise<any>;
}) {
  const [detail,setDetail]=useState<string>("");
  const [confirmImage,setConfirmImage]=useState<any>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(()=>{ if(confirmImage) cancelRef.current?.focus(); }, [confirmImage]);

  if (error && !hasCache) return <div className="panel" style={{ padding: 12, background: "var(--error-bg)", borderColor: "#7F1D1D", color: "var(--error-fg)" }}>Error: {error} <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>;
  if (!hasCache && refreshing) {
    return (
      <div className="table-wrap">
        <div className="panel-header" style={{ padding: "0 12px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span className="text-panel-title">Images</span>
          <span className="text-meta" style={{ color: "var(--accent-cyan)" }}>• refreshing…</span>
        </div>
        <div style={{ padding: 16 }}><div aria-busy="true" style={{ display: "flex", flexDirection: "column", gap: 8 }}>{Array.from({length: 3}).map((_,i)=><div key={i} style={{ height: 12, background: "var(--panel-header)", borderRadius: 6, opacity: 0.6+ i*0.1, animation: "pulse 1.5s ease-in-out infinite" }} />)}<style>{`@keyframes pulse{0%,100%{opacity:0.6}50%{opacity:1}}`}</style></div></div>
      </div>
    );
  }
  if (!hasCache && !refreshing) return <div className="panel" style={{ padding: 16 }}>No images. <span className="text-meta" style={{ color: "var(--text-secondary)" }}>Pull with <code className="text-code">docker --context harpoon pull &lt;image&gt;</code> or build.</span> <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Refresh</button></div>;

  return (
    <div className="table-wrap" aria-busy={refreshing ? "true" : undefined}>
      <div className="panel-header" style={{ padding: "0 12px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <span className="text-panel-title">Images ({images.length}) <span style={{ fontWeight: 400, fontSize: 10, color: "var(--text-secondary)", textTransform: "none", letterSpacing: 0 }}>updated {fmtTs(updatedAt)}</span> {refreshing && <span className="text-meta" style={{ marginLeft: 6, color: "var(--accent-cyan)" }}>• refreshing…</span>}{error && hasCache && <span className="text-meta" style={{ marginLeft: 6, color: "var(--error-fg)" }}>• error</span>}</span>
        <button onClick={refresh} disabled={refreshing} className="btn btn--ghost btn--sm">Refresh</button>
      </div>
      {error && hasCache && <div style={{ padding: "8px 12px", background: "var(--error-bg)", color: "var(--error-fg)", fontSize: 12, borderBottom: "1px solid #7F1D1D" }}>Error: {error} <button onClick={refresh} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>}
      <div style={{ overflowX: "auto" }}>
        <table>
          <thead className="table-head"><tr><th scope="col" className="col-repo">Repository:Tag</th><th scope="col" className="col-img-id">ID</th><th scope="col" className="col-size">Size</th><th scope="col" className="col-created">Created</th><th scope="col" className="col-img-actions" style={{ textAlign: "right" }}>Actions</th></tr></thead>
          <tbody>{images.map((im:any)=>(
            <tr key={im.ID} className="table-row">
              <td className="col-repo" title={`${im.Repository}:${im.Tag}`} aria-label={`${im.Repository}:${im.Tag}`}><span className="cell-inner">{im.Repository}:{im.Tag}</span></td>
              <td className="mono col-img-id" title={im.ID||""} aria-label={im.ID||""}><span className="cell-inner">{(im.ID||"").slice(0,12)}</span></td>
              <td className="col-size" title={im.Size||""}><span className="cell-inner">{im.Size||"—"}</span></td>
              <td className="col-created" title={im.CreatedAt||im.CreatedSince||""}><span className="cell-inner">{im.CreatedAt||im.CreatedSince||"—"}</span></td>
              <td className="col-img-actions"><div className="table-actions">
                <button onClick={async ()=>{ const j=await invoke<any>("inspect_image", { id: im.ID }); setDetail(JSON.stringify(j,null,2)); }} className="btn btn--icon btn--secondary" aria-label={`Inspect image ${im.Repository}:${im.Tag}`} data-tooltip="Inspect" title={`Inspect ${im.Repository}:${im.Tag}`}><MagnifyingGlass size={16} weight="regular" /></button>
                <button disabled={actionInProgress===`rmi-${im.ID}`} onClick={()=>setConfirmImage(im)} className="btn btn--icon btn--destructive" aria-label={`Remove image ${im.Repository}:${im.Tag}`} data-tooltip="Remove" title={`Remove ${im.Repository}:${im.Tag}`}><Trash size={16} weight="regular" /></button>
              </div></td>
            </tr>
          ))}</tbody>
        </table>
      </div>
      {confirmImage && (
        <div className="dialog-scrim" role="presentation" onClick={()=>setConfirmImage(null)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-image-title" onClick={e=>e.stopPropagation()} onKeyDown={e=>{ if(e.key==="Escape") setConfirmImage(null); }}>
            <div id="confirm-image-title" className="dialog-title">Remove image?</div>
            <div className="dialog-body">This will permanently remove <strong>{confirmImage.Repository}:{confirmImage.Tag}</strong>. This action cannot be undone.</div>
            <div className="dialog-actions">
              <button ref={cancelRef} onClick={()=>setConfirmImage(null)} className="btn btn--secondary">Cancel</button>
              <button onClick={()=>{ const im=confirmImage; setConfirmImage(null); doAction(`rmi-${im.ID}`, ()=>invoke("remove_image", { id: im.ID })); }} className="btn btn--destructive">Remove image</button>
            </div>
          </div>
        </div>
      )}
      {detail && <pre className="text-code" style={{ margin: 12, background: "var(--panel-header)", color: "var(--text-primary)", padding: 10, borderRadius: 8, maxHeight: 300, overflow: "auto", border: "1px solid var(--border)" }}>{detail}<button onClick={()=>setDetail("")} className="btn btn--ghost btn--sm" style={{ float: "right" }}>Close</button></pre>}
    </div>
  );
});
