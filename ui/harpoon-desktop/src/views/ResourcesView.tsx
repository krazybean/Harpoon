import { memo } from "react";
import { fmtTs } from "../components/ShelfPrimitives";

type ConfigResult = { cpus: number; memory: number; raw: string; path: string };
type HarpoonStatus = { cpus?: number; memoryMiB?: number; diskPath?: string; diskLogicalBytes?: number };

export const ResourcesView = memo(function ResourcesView({
  config, configAt, status, binaryPath, actionInProgress, onSetCpus, onSetMemory
}: {
  config: ConfigResult | null;
  configAt: number | null;
  status: HarpoonStatus | null;
  binaryPath: string;
  actionInProgress: string | null;
  onSetCpus: (v: number) => void;
  onSetMemory: (v: number) => void;
}) {
  return (
    <div style={{ display: "grid", gap: 12 }}>
      <div className="panel" style={{ overflow: "hidden" }}>
        <div className="panel-header" style={{ padding: "10px 14px", display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <span className="text-panel-title">Resources & Config <span className="text-meta" style={{ textTransform: "none", letterSpacing: 0, marginLeft: 8 }}>{fmtTs(configAt)}</span></span>
        </div>
        <div style={{ padding: "14px" }}>
          <div className="text-body" style={{ lineHeight: 1.7 }}>
            <div><span className="text-meta">CPUs (configured)</span> <span style={{ marginLeft: 6, fontWeight: 600 }}>{config?.cpus ?? "—"}</span> <span className="text-meta">• Status CPUs {status?.cpus ?? "—"}</span></div>
            <div><span className="text-meta">Memory (configured)</span> <span style={{ marginLeft: 6, fontWeight: 600 }}>{config?.memory ?? "—"} MiB</span> <span className="text-meta">• Status {status?.memoryMiB ?? "—"} MiB</span></div>
            <div><span className="text-meta">Disk</span> <code className="text-code" title={status?.diskPath ?? "—"} style={{ marginLeft: 6 }}>{status?.diskPath ?? "—"}</code> {status?.diskLogicalBytes ? <span className="text-meta">({status.diskLogicalBytes} bytes)</span> : null}</div>
            <div style={{ marginTop: 14, display: "flex", gap: 12, flexWrap: "wrap", alignItems: "end" }}>
              <label className="text-body" style={{ display: "flex", flexDirection: "column", gap: 4, fontSize: 12 }}>CPUs
                <select className="select-primitive" value={config?.cpus ?? 2} onChange={(e)=>onSetCpus(parseInt(e.target.value))} disabled={actionInProgress==="cpus"}>
                  <option value={1}>1</option><option value={2}>2</option><option value={4}>4</option>
                </select>
              </label>
              <label className="text-body" style={{ display: "flex", flexDirection: "column", gap: 4, fontSize: 12 }}>Memory
                <select className="select-primitive" value={config?.memory ?? 1024} onChange={(e)=>onSetMemory(parseInt(e.target.value))} disabled={actionInProgress==="memory"}>
                  <option value={512}>512 MiB</option><option value={768}>768 MiB</option><option value={1024}>1024 MiB</option><option value={1536}>1536 MiB</option><option value={2048}>2048 MiB</option>
                </select>
              </label>
            </div>
            <div className="text-meta" style={{ marginTop: 10 }}>Changes via <code className="text-code">harpoon config set</code>. Restart required.</div>
            <div className="text-code" style={{ marginTop: 4, wordBreak: "break-all" }} title={binaryPath}>Binary: {binaryPath || "—"}</div>
          </div>
        </div>
      </div>
    </div>
  );
});
