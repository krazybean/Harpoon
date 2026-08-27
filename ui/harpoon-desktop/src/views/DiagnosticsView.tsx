import { memo } from "react";
import { fmtTs } from "../components/ShelfPrimitives";

type DoctorResult = { raw: string; passed: number; warnings: number; failures: number };

export const DiagnosticsView = memo(function DiagnosticsView({
  binaryPath,
  logs,
  logsAt,
  logsRefreshing,
  logsError,
  logPath,
  doctor,
  doctorAt,
  doctorRefreshing,
  doctorError,
  status,
  configRaw,
  bootstrapPhase,
  onCopy,
  copied,
  onRefreshDoctor,
  onRefreshLogs,
}: {
  binaryPath: string;
  logs: string | null;
  logsAt: number | null;
  logsRefreshing: boolean;
  logsError: string | null;
  logPath: string;
  doctor: DoctorResult | null;
  doctorAt: number | null;
  doctorRefreshing: boolean;
  doctorError: string | null;
  status: any;
  configRaw: string | null;
  bootstrapPhase: string;
  onCopy: () => void;
  copied: boolean;
  onRefreshDoctor: () => void;
  onRefreshLogs: () => void;
}) {
  const hasDoctorCache = !!(doctor && doctor.raw);
  const hasLogsCache = !!(logs && logs.length > 0);

  return (
    <div style={{ display: "grid", gap: 12 }}>
      <div className="panel" style={{ overflow: "hidden" }}>
        <div className="panel-header" style={{ padding: "10px 14px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span className="text-panel-title">Diagnostics <span className="text-meta" style={{ textTransform: "none", letterSpacing: 0, marginLeft: 8 }}>logs {fmtTs(logsAt)} • doctor {fmtTs(doctorAt)} • bootstrap {bootstrapPhase}</span></span>
          <button onClick={onCopy} className={copied ? "btn btn--primary btn--sm" : "btn btn--secondary btn--sm"}>{copied ? "Copied!" : "Copy diagnostics"}</button>
        </div>
        <div style={{ padding: "14px" }}>
          <div className="text-body" style={{ marginBottom: 6 }}><span className="text-meta">Binary</span> <code className="text-code" style={{ marginLeft: 6, wordBreak: "break-all" }} title={binaryPath}>{binaryPath || "—"}</code></div>

          {/* Doctor region - independent shelf */}
          <div style={{ border: "1px solid var(--border)", borderRadius: 8, padding: 10, marginBottom: 12 }} aria-busy={doctorRefreshing ? "true" : undefined}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
              <span className="text-panel-title" style={{ fontSize: 11 }}>Doctor <span className="text-meta" style={{ textTransform: "none", letterSpacing: 0, marginLeft: 6 }}>{fmtTs(doctorAt)}</span> {doctorRefreshing && <span className="text-meta" style={{ marginLeft: 6, color: "var(--accent-cyan)" }}>• refreshing…</span>}</span>
              <button onClick={onRefreshDoctor} disabled={doctorRefreshing} className="btn btn--ghost btn--sm">Refresh</button>
            </div>
            {doctorError && !hasDoctorCache && <div style={{ padding: 8, background: "var(--error-bg)", color: "var(--error-fg)", fontSize: 12, borderRadius: 6, marginBottom: 6 }}>Error: {doctorError} <button onClick={onRefreshDoctor} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>}
            {doctorError && hasDoctorCache && <div style={{ padding: "6px 8px", background: "var(--error-bg)", color: "var(--error-fg)", fontSize: 11, borderRadius: 6, marginBottom: 6 }}>Error refreshing doctor: {doctorError} — showing cached</div>}
            {!hasDoctorCache && doctorRefreshing && <div aria-busy="true" style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 6 }}>{Array.from({length: 3}).map((_,i)=><div key={i} style={{ height: 10, background: "var(--panel-header)", borderRadius: 6, opacity: 0.6+ i*0.1 }} />)}</div>}
            <div className="text-body"><span className="text-meta">Doctor</span> <span style={{ marginLeft: 6 }}>{doctor ? `${doctor.passed} passed, ${doctor.warnings} warnings, ${doctor.failures} failures` : "—"}</span> {doctor?.failures ? <span className="badge badge--error badge--sm" style={{ marginLeft: 6 }}>{doctor.failures} failures</span> : doctor?.warnings ? <span className="badge badge--warning badge--sm" style={{ marginLeft: 6 }}>{doctor.warnings} warnings</span> : doctor?.passed ? <span className="badge badge--running badge--sm" style={{ marginLeft: 6 }}>healthy</span> : null}</div>
            <pre className="text-code" style={{ background: "var(--panel-header)", border: "1px solid var(--border)", borderRadius: 8, padding: 10, maxHeight: 200, overflow: "auto", whiteSpace: "pre-wrap", wordBreak: "break-all", marginTop: 8 }}>{doctor?.raw ?? "—"}</pre>
          </div>

          {/* Logs region - independent shelf */}
          <div style={{ border: "1px solid var(--border)", borderRadius: 8, padding: 10 }} aria-busy={logsRefreshing ? "true" : undefined}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
              <span className="text-panel-title" style={{ fontSize: 11 }}>Recent logs <span className="text-meta" style={{ textTransform: "none", letterSpacing: 0, marginLeft: 6 }}>{fmtTs(logsAt)}</span> {logsRefreshing && <span className="text-meta" style={{ marginLeft: 6, color: "var(--accent-cyan)" }}>• refreshing…</span>}</span>
              <button onClick={onRefreshLogs} disabled={logsRefreshing} className="btn btn--ghost btn--sm">Refresh</button>
            </div>
            <div className="text-code" style={{ color: "var(--text-secondary)", wordBreak: "break-all" }} title={logPath || status?.logPath}>{logPath || status?.logPath || "—"}</div>
            {logsError && !hasLogsCache && <div style={{ padding: 8, background: "var(--error-bg)", color: "var(--error-fg)", fontSize: 12, borderRadius: 6, marginTop: 6, marginBottom: 6 }}>Error: {logsError} <button onClick={onRefreshLogs} className="btn btn--secondary btn--sm" style={{ marginLeft: 8 }}>Retry</button></div>}
            {logsError && hasLogsCache && <div style={{ padding: "6px 8px", background: "var(--error-bg)", color: "var(--error-fg)", fontSize: 11, borderRadius: 6, marginTop: 6, marginBottom: 6 }}>Error refreshing logs: {logsError} — showing cached</div>}
            {!hasLogsCache && logsRefreshing && <div aria-busy="true" style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 6 }}>{Array.from({length: 3}).map((_,i)=><div key={i} style={{ height: 10, background: "var(--panel-header)", borderRadius: 6, opacity: 0.6+ i*0.1 }} />)}</div>}
            {(hasLogsCache || !logsRefreshing) && <pre className="text-code" style={{ background: "var(--app-bg)", color: "#E5E7EB", border: "1px solid var(--border)", borderRadius: 8, padding: 10, maxHeight: 260, overflow: "auto", whiteSpace: "pre-wrap", wordBreak: "break-all", marginTop: 6 }}>{logs || "(no logs)"}</pre>}
            <details style={{ marginTop: 10 }}>
              <summary className="text-body" style={{ cursor: "pointer", fontSize: 12 }}>Raw status JSON</summary>
              <pre className="text-code" style={{ background: "var(--panel-header)", border: "1px solid var(--border)", borderRadius: 8, padding: 10, overflow: "auto", marginTop: 6 }}>{JSON.stringify(status,null,2)}</pre>
              <pre className="text-code" style={{ background: "var(--panel-header)", border: "1px solid var(--border)", borderRadius: 8, padding: 10, overflow: "auto", marginTop: 6 }}>{configRaw || "—"}</pre>
            </details>
          </div>
        </div>
      </div>
    </div>
  );
});
