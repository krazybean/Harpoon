import { isStale } from "../hooks/useShelf";

export function ShelfBadge({ at, refreshing, ttlMs = 30000 }: { at: number | null; refreshing?: boolean; ttlMs?: number }) {
  const stale = isStale(at, ttlMs);
  if (refreshing) return <span style={{ fontSize: 11, color: "var(--accent-cyan)", marginLeft: 6 }} aria-busy="true">• refreshing…</span>;
  if (stale && at) return <span style={{ fontSize: 11, color: "var(--warning-fg)", background: "var(--warning-bg)", padding: "2px 6px", borderRadius: 999, marginLeft: 6 }}>STALE</span>;
  return null;
}

export function Skeleton({ lines = 3 }: { lines?: number }) {
  return <div aria-busy="true" style={{ display: "flex", flexDirection: "column", gap: 8 }}>{Array.from({length: lines}).map((_,i)=><div key={i} style={{ height: 12, background: "var(--panel-header)", borderRadius: 6, opacity: 0.6+ i*0.1, animation: "pulse 1.5s ease-in-out infinite" }} />)}<style>{`@keyframes pulse{0%,100%{opacity:0.6}50%{opacity:1}}`}</style></div>;
}

export function fmtTs(ts: number | null) {
  if (!ts) return "—";
  return new Date(ts).toLocaleTimeString();
}
