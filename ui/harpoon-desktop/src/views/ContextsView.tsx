import { useEffect } from "react";
import { ArrowClockwise, CircleNotch, Eye, ShieldCheck, Warning, XCircle } from "@phosphor-icons/react";

type Context = {
  id: string; name: string; engine: string; endpoint?: string | null; source: string;
  state: "detected" | "connected" | "unavailable" | "authentication_needed";
  current: boolean; harpoon: boolean; detail: string;
  capabilities: Record<string, "available" | "unsupported" | "unknown">;
};

function stateMeta(state: Context["state"]) {
  if (state === "connected") return { label: "Connected", className: "badge--running", Icon: ShieldCheck };
  if (state === "authentication_needed") return { label: "Authentication needed", className: "badge--warning", Icon: Warning };
  if (state === "unavailable") return { label: "Unavailable", className: "badge--error", Icon: XCircle };
  return { label: "Detected", className: "badge--info", Icon: Eye };
}

export function ContextsView({ contexts, refreshing, error, updatedAt, hasCache, stale, refresh }: { contexts: Context[]; refreshing: boolean; error: string | null; updatedAt: number | null; hasCache: boolean; stale: boolean; refresh: () => Promise<unknown> }) {
  useEffect(() => {
    if (hasCache && !stale) return;
    const frame = requestAnimationFrame(() => { void refresh(); });
    return () => cancelAnimationFrame(frame);
  }, [refresh]);
  return <section className="contexts-view" aria-labelledby="contexts-title">
    <div className="contexts-intro">
      <div><div className="overview-eyebrow">Settings / Contexts</div><h1 id="contexts-title">Container engines</h1><p>Read-only discovery of Docker and Podman endpoints available to this Mac.</p>{updatedAt && <div className="text-meta">Last checked {new Date(updatedAt).toLocaleTimeString()}</div>}</div>
      <button className="btn btn--secondary" onClick={() => void refresh()} disabled={refreshing}><ArrowClockwise size={14} aria-hidden="true" /> {refreshing ? "Discovering…" : "Refresh"}</button>
    </div>
    <div className="panel contexts-scope"><Eye size={16} aria-hidden="true" /><span><strong>Discovery only.</strong> Harpoon does not switch contexts, start runtimes, change sockets, or request credentials here.</span></div>
    {error && <div className="panel contexts-error" role="alert">Discovery could not complete: {error}</div>}
    {refreshing && !contexts.length && <div className="panel contexts-empty"><CircleNotch className="contexts-spin" size={18} aria-hidden="true" /> Checking local CLI and socket state…</div>}
    {!refreshing && !contexts.length && !error && <div className="panel contexts-empty">No Docker or Podman contexts were discovered.</div>}
    <div className="contexts-grid">{contexts.map((context) => { const meta = stateMeta(context.state); const Icon = meta.Icon; return <article className="panel context-card" key={context.id}>
      <div className="context-card__header"><div className="context-card__identity"><div className="context-card__name"><span className="context-card__name-text" title={context.name} aria-label={`Context name: ${context.name}`} tabIndex={0}>{context.name}</span>{context.harpoon && <span className="badge badge--sm badge--warning">Harpoon</span>}</div><div className="text-meta">{context.engine} · {context.source}{context.current ? " · current" : ""}</div></div><span className={`badge ${meta.className}`}><Icon size={12} aria-hidden="true" /> {meta.label}</span></div>
      <code className="context-card__endpoint" title={context.endpoint || "Endpoint not exposed by CLI"} aria-label={`Endpoint: ${context.endpoint || "Endpoint not exposed by CLI"}`} tabIndex={0}>{context.endpoint || "Endpoint not exposed by CLI"}</code>
      <p className="context-card__detail">{context.detail || "No additional details reported."}</p>
      <div className="context-card__capabilities" aria-label="Read-only capabilities">{Object.entries(context.capabilities).filter(([key]) => key !== "mutations").map(([key, value]) => <span key={key} className={value === "available" ? "capability capability--available" : "capability"}>{key}: {value}</span>)}</div>
      <div className="context-card__footer"><span className="text-meta">Mutating operations</span><span className="text-meta">unsupported in discovery</span></div>
    </article>; })}</div>
  </section>;
}
