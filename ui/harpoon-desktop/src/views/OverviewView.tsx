import { memo } from "react";
import { ChartLineUp, CircleNotch, Cpu, Database, HardDrives, Image as ImageIcon, Pulse, Stack } from "@phosphor-icons/react";
import { ShelfBadge, fmtTs } from "../components/ShelfPrimitives";

type HarpoonStatus = { state: string; cpus?: number; memoryMiB?: number; diskLogicalBytes?: number; dockerReady?: boolean };
type DoctorResult = { raw: string; passed: number; warnings: number; failures: number };
type ConfigResult = { cpus: number; memory: number; raw: string; path: string };
type Counts = { containers: number; running: number; images: number; volumes: number; networks: number };

function formatBytes(bytes?: number) {
  if (!bytes) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

function diagnosticSummary(doctor: DoctorResult | null) {
  if (!doctor) return { label: "Not collected", tone: "neutral" };
  if (doctor.failures > 0) return { label: `${doctor.failures} failure${doctor.failures === 1 ? "" : "s"}`, tone: "error" };
  if (doctor.warnings > 0) return { label: `${doctor.warnings} warning${doctor.warnings === 1 ? "" : "s"}`, tone: "warning" };
  return { label: `${doctor.passed} checks passed`, tone: "success" };
}

const InventoryTile = memo(function InventoryTile({ label, value, detail, icon: Icon, onOpen }: { label: string; value: string | number; detail: string; icon: typeof Stack; onOpen?: () => void }) {
  const content = <><span className="overview-inventory__icon"><Icon size={16} aria-hidden="true" /></span><span className="overview-inventory__label">{label}</span><strong>{value}</strong><span className="overview-inventory__detail">{detail}</span></>;
  return onOpen ? <button className="overview-inventory" onClick={onOpen} aria-label={`Open ${label}`}>{content}</button> : <div className="overview-inventory">{content}</div>;
});

export const OverviewView = memo(function OverviewView({ status, statusAt, dockerVersion, dockerAt, counts, countsAt, config, configAt, doctor, doctorAt, bootstrapPhase, runtimePolling, runtimePollingPaused, onOpenDiagnostics, onOpenContainers, onOpenResources }: {
  status: HarpoonStatus | null; statusAt: number | null; dockerVersion: string | null; dockerAt: number | null; counts: Counts | null; countsAt: number | null; config: ConfigResult | null; configAt: number | null; doctor: DoctorResult | null; doctorAt: number | null; binaryPath: string; logPath: string; bootstrapPhase: string; runtimePolling: boolean; runtimePollingPaused: string[]; onOpenDiagnostics: () => void; onOpenContainers: () => void; onOpenResources: () => void;
}) {
  const diagnostics = diagnosticSummary(doctor);
  const runtimeState = status?.state ?? "Unavailable";
  const memory = status?.memoryMiB ?? config?.memory;
  const cpus = status?.cpus ?? config?.cpus;
  const engineLabel = status?.dockerReady ? "Connected" : status?.dockerReady === false ? "Unavailable" : "Waiting";

  return <div className="overview-page">
    <section className="overview-intro" aria-labelledby="overview-title">
      <div><div className="overview-eyebrow"><Pulse size={14} aria-hidden="true" /> Local runtime</div><h1 id="overview-title">Overview</h1><p>Runtime health, workload inventory, and the latest operational signals.</p></div>
      <div className="overview-intro__status"><span className={`overview-status-dot overview-status-dot--${status?.dockerReady ? "success" : status?.dockerReady === false ? "error" : "neutral"}`} aria-hidden="true" /><div><strong>{engineLabel}</strong><span>Docker Engine</span></div><ShelfBadge at={dockerAt} /></div>
    </section>

    <section className="overview-inventory-grid" aria-label="Docker inventory">
      <InventoryTile label="Containers" value={counts?.containers ?? "—"} detail={counts ? `${counts.running} running` : "Waiting for engine"} icon={Stack} onOpen={onOpenContainers} />
      <InventoryTile label="Images" value={counts?.images ?? "—"} detail="Engine inventory" icon={ImageIcon} />
      <InventoryTile label="Volumes" value={counts?.volumes ?? "—"} detail="Engine inventory" icon={Database} />
      <InventoryTile label="Networks" value={counts?.networks ?? "—"} detail="Engine inventory" icon={HardDrives} />
    </section>

    <div className="overview-content-grid">
      <section className="panel overview-section" aria-labelledby="runtime-summary-title">
        <div className="overview-section__header"><div><span className="overview-section__kicker">Harpoon runtime</span><h2 id="runtime-summary-title">Runtime envelope</h2></div><button className="btn btn--ghost btn--sm" onClick={onOpenResources}>Resources <ChartLineUp size={14} aria-hidden="true" /></button></div>
        <div className="overview-runtime-grid">
          <div className="overview-runtime-stat"><Cpu size={18} aria-hidden="true" /><div><span>vCPUs</span><strong>{cpus ?? "—"}</strong><small>{config ? `configured ${config.cpus}` : "Unavailable"}</small></div></div>
          <div className="overview-runtime-stat"><HardDrives size={18} aria-hidden="true" /><div><span>Memory</span><strong>{memory ? `${memory} MiB` : "—"}</strong><small>{config ? `configured ${config.memory} MiB` : "Unavailable"}</small></div></div>
          <div className="overview-runtime-stat"><Database size={18} aria-hidden="true" /><div><span>Disk image</span><strong>{formatBytes(status?.diskLogicalBytes)}</strong><small>{status?.diskLogicalBytes ? "logical capacity" : "Unavailable"}</small></div></div>
        </div>
        <div className="overview-source-row"><span>Source</span><code>Harpoon status + config</code><span>Updated {fmtTs(statusAt || configAt)}</span></div>
        <div className="overview-data-note"><CircleNotch size={14} aria-hidden="true" /> Time-series CPU, memory, and network usage are not collected by this client.</div>
      </section>

      <section className="panel overview-section" aria-labelledby="signals-title">
        <div className="overview-section__header"><div><span className="overview-section__kicker">Live read model</span><h2 id="signals-title">Recent signals</h2></div><span className="text-meta">No fabricated history</span></div>
        <div className="overview-signal-list">
          <div className="overview-signal"><span className="overview-signal__marker overview-signal__marker--runtime" /><div><strong>Runtime {runtimeState}</strong><span>Harpoon lifecycle state</span></div><time>{fmtTs(statusAt)}</time></div>
          <div className="overview-signal"><span className={`overview-signal__marker overview-signal__marker--${status?.dockerReady ? "success" : "neutral"}`} /><div><strong>Docker {engineLabel.toLowerCase()}</strong><span>{dockerVersion || "Engine version unavailable"}</span></div><time>{fmtTs(dockerAt)}</time></div>
          <div className="overview-signal"><span className="overview-signal__marker overview-signal__marker--inventory" /><div><strong>{counts ? `${counts.containers} containers indexed` : "Inventory unavailable"}</strong><span>{counts ? `${counts.images} images · ${counts.volumes} volumes · ${counts.networks} networks` : "Waiting for engine"}</span></div><time>{fmtTs(countsAt)}</time></div>
          <button className="overview-signal overview-signal--button" onClick={onOpenDiagnostics}><span className={`overview-signal__marker overview-signal__marker--${diagnostics.tone}`} /><div><strong>Diagnostics: {diagnostics.label}</strong><span>{doctor ? `${doctor.passed} passed · ${doctor.warnings} warnings · ${doctor.failures} failures` : "Run Doctor to collect checks"}</span></div><time>{fmtTs(doctorAt)}</time></button>
        </div>
      </section>
    </div>

    <div className="overview-footer"><span>Bootstrap <strong>{bootstrapPhase.split("_").join(" ")}</strong></span><span>Inventory source <code>docker --context harpoon</code></span>{runtimePollingPaused.length > 0 && <span className="text-meta">Live refresh paused · cached data: {runtimePollingPaused.join(", ")}</span>}</div>
  </div>;
});
