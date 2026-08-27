import { memo } from "react";
import { ShelfBadge, fmtTs } from "../components/ShelfPrimitives";

type HarpoonStatus = {
  state: string;
  pid?: number;
  cpus?: number;
  memoryMiB?: number;
  diskPath?: string;
  diskLogicalBytes?: number;
  socketPath?: string;
  sockExists?: boolean;
  lockHeld?: boolean;
  lockPath?: string;
  logPath?: string;
  dockerReady?: boolean;
};
type DoctorResult = { raw: string; passed: number; warnings: number; failures: number };
type ConfigResult = { cpus: number; memory: number; raw: string; path: string };
type Counts = { containers: number; running: number; images: number; volumes: number; networks: number };

export const OverviewView = memo(function OverviewView({
  status, statusAt,
  dockerVersion, dockerAt,
  counts, countsAt,
  config, configAt,
  doctor, doctorAt,
  binaryPath,
  logPath,
  bootstrapPhase,
  onOpenDiagnostics,
}: {
  status: HarpoonStatus | null;
  statusAt: number | null;
  dockerVersion: string | null;
  dockerAt: number | null;
  counts: Counts | null;
  countsAt: number | null;
  config: ConfigResult | null;
  configAt: number | null;
  doctor: DoctorResult | null;
  doctorAt: number | null;
  binaryPath: string;
  logPath: string;
  bootstrapPhase: string;
  onOpenDiagnostics: () => void;
}) {
  return (
    <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
      <div className="panel" aria-busy={!!status} style={{ overflow: "hidden" }}>
        <div className="panel-header" style={{ padding: "10px 14px", display: "flex", alignItems: "center", gap: 8 }}>
          <span className="text-panel-title">Runtime</span> <ShelfBadge at={statusAt} />
        </div>
        <div style={{ padding: "12px 14px" }}>
          <div className="text-body" style={{ lineHeight: 1.7 }}>
            <div><span className="text-meta" style={{ color: "var(--text-secondary)" }}>State</span> <span className="badge badge--sm" style={{ marginLeft: 6, background: status?.state==="running" ? "var(--success-bg)" : "var(--panel-header)", color: status?.state==="running" ? "var(--success-fg)" : "var(--text-secondary)", borderColor: "var(--border)" }}>{status?.state ?? "—"}</span> {status?.pid ? <span className="text-meta">PID {status.pid}</span> : null} <span className="text-meta" style={{ marginLeft: 6 }}>{fmtTs(statusAt)}</span> <span className="text-meta">• bootstrap {bootstrapPhase}</span></div>
            <div className="text-body"><span className="text-meta">Socket</span> <code className="text-code" title={status?.socketPath ?? "—"}>{status?.socketPath ?? "—"}</code> <span className={status?.sockExists ? "badge badge--running badge--sm" : "badge badge--stopped badge--sm"} style={{ marginLeft: 6 }}>{status?.sockExists ? "exists" : "missing"}</span></div>
            <div className="text-body"><span className="text-meta">Lock</span> <code className="text-code" title={status?.lockPath ?? "—"}>{status?.lockPath ?? "—"}</code> <span className="text-meta" style={{ marginLeft: 6 }}>{status?.lockHeld ? "held" : "not held"}</span></div>
            <div className="text-body"><span className="text-meta">Disk</span> <code className="text-code" title={status?.diskPath ?? "—"}>{status?.diskPath ?? "—"}</code> {status?.diskLogicalBytes ? <span className="text-meta">({status.diskLogicalBytes} bytes)</span> : null}</div>
            <div className="text-body"><span className="text-meta">Log</span> <code className="text-code" title={logPath || status?.logPath || "—"}>{logPath || status?.logPath || "—"}</code></div>
            <div className="text-body"><span className="text-meta">Binary</span> <code className="text-code" style={{ wordBreak: "break-all" }} title={binaryPath || "—"}>{binaryPath || "—"}</code></div>
          </div>
        </div>
      </div>
      <div className="panel" style={{ overflow: "hidden" }}>
        <div className="panel-header" style={{ padding: "10px 14px", display: "flex", alignItems: "center", gap: 8 }}>
          <span className="text-panel-title">Docker</span> <ShelfBadge at={dockerAt} />
        </div>
        <div style={{ padding: "12px 14px" }}>
          <div className="text-body" style={{ lineHeight: 1.7 }}>
            <div><span className="text-meta">Ready</span> <span className={status?.dockerReady ? "badge badge--running badge--sm" : "badge badge--error badge--sm"} style={{ marginLeft: 6 }}>{status?.dockerReady ? "ready" : "not ready"}</span> <span className="text-meta" style={{ marginLeft: 6 }}>{fmtTs(dockerAt)}</span></div>
            <div><span className="text-meta">Engine</span> <span className="text-body" style={{ marginLeft: 6 }}>{dockerVersion ?? "—"}</span></div>
            {counts && <div><span className="text-meta">Containers</span> <span className="text-body" style={{ marginLeft: 6 }}>{counts.running} running / {counts.containers} total</span> <span className="text-meta" style={{ marginLeft: 6 }}>{fmtTs(countsAt)}</span></div>}
            {counts && <div className="text-meta">Images {counts.images} • Volumes {counts.volumes} • Networks {counts.networks}</div>}
            <div className="text-meta">via <code className="text-code">docker --context harpoon</code></div>
          </div>
        </div>
      </div>
      <div className="panel" style={{ overflow: "hidden" }}>
        <div className="panel-header" style={{ padding: "10px 14px" }}><span className="text-panel-title">Resources</span></div>
        <div style={{ padding: "12px 14px" }}>
          <div className="text-body" style={{ lineHeight: 1.7 }}>
            <div><span className="text-meta">CPUs</span> <span style={{ marginLeft: 6 }}>{status?.cpus ?? config?.cpus ?? "—"}</span> {config && <span className="text-meta">({config.cpus} configured)</span>} <span className="text-meta" style={{ marginLeft: 6 }}>{fmtTs(configAt)}</span></div>
            <div><span className="text-meta">Memory</span> <span style={{ marginLeft: 6 }}>{status?.memoryMiB ?? config?.memory ?? "—"} MiB</span> {config && <span className="text-meta">({config.memory} MiB)</span>}</div>
            <div className="text-meta" style={{ marginTop: 6 }}><code className="text-code">harpoon config</code> — restart required for changes</div>
          </div>
        </div>
      </div>
      <div className="panel" style={{ overflow: "hidden" }}>
        <div className="panel-header" style={{ padding: "10px 14px", display: "flex", alignItems: "center", gap: 8 }}>
          <span className="text-panel-title">Diagnostics</span> <ShelfBadge at={doctorAt} />
        </div>
        <div style={{ padding: "12px 14px" }}>
          <div className="text-body"><span className="text-meta">Doctor</span> <span style={{ marginLeft: 6 }}>{doctor ? `${doctor.passed} passed, ${doctor.warnings} warnings, ${doctor.failures} failures` : "—"}</span> <span className="text-meta" style={{ marginLeft: 6 }}>{fmtTs(doctorAt)}</span></div>
          <div className="text-code" style={{ marginTop: 8, color: "var(--text-secondary)", wordBreak: "break-all" }} title={binaryPath}>{binaryPath || "—"}</div>
          <button onClick={onOpenDiagnostics} className="btn btn--ghost btn--sm" style={{ marginTop: 8 }}>Open diagnostics</button>
        </div>
      </div>
    </div>
  );
});
