import { ageLabel, guestLine, hostLine } from "../lib/diskTelemetry";
import type { DiskStatus, DiskView } from "../lib/diskTelemetry";
import { useDiskView } from "../hooks/useDiskTelemetry";
import { formatSize } from "../lib/diskCapacity";
import type { DiskPressure } from "../lib/diskCapacity";

const STATUS_LABEL: Record<DiskStatus, string> = {
  live: "Live", stale: "Stale", "engine-down": "Harpoon stopped", disconnected: "Guest not answering", unsupported: "Unsupported", unavailable: "Unavailable",
};
const PRESSURE_LABEL: Record<DiskPressure, string> = { ok: "Healthy", warning: "Warning: running low", critical: "Critical: almost full", unknown: "" };
const PRESSURE_BADGE: Record<DiskPressure, string> = { ok: "badge--running", warning: "badge--warning", critical: "badge--error", unknown: "badge--stopped" };

/**
 * Always-visible disk usage. The bar is the guest filesystem's real utilization; its colour is the runtime's own pressure class. The Mac's
 * sparse disk image is a separate line: it only grows, and freed guest space is not returned to macOS until it is reclaimed.
 */
export function DiskMeter({ view, now }: { view: DiskView; now: number }) {
  const { cap, status, percent, pressure } = view;
  const g = cap && cap.guest ? guestLine(cap, formatSize) : null;
  const host = cap ? hostLine(cap, formatSize) : null;
  const pct = percent === null ? null : Math.min(100, Math.max(0, percent));
  const shown = status !== "unsupported" && pct !== null;
  return (
    <div className="panel" style={{ padding: 14 }} data-disk-status={status} data-disk-pressure={pressure}>
      <div style={{ display: "flex", justifyContent: "space-between", gap: 8, flexWrap: "wrap", alignItems: "baseline" }}>
        <span className="text-panel-title">Docker disk</span>
        <span className="text-meta" style={{ textTransform: "none", letterSpacing: 0 }}>
          <span className={`badge badge--sm ${status === "live" ? PRESSURE_BADGE[pressure] : "badge--stopped"}`}>{STATUS_LABEL[status]}</span>{" "}
          {view.at !== null && <>Last successful reading {ageLabel(view.at, now)}</>}
        </span>
      </div>
      {shown ? (
        <>
          <div className="text-body" style={{ margin: "8px 0 6px", display: "flex", gap: 8, alignItems: "baseline", flexWrap: "wrap" }}>
            <b style={{ fontSize: 20 }}>{Math.round(pct!)}%</b> <span>of the guest filesystem is used</span>
            {PRESSURE_LABEL[pressure] && <span className={`badge badge--sm ${PRESSURE_BADGE[pressure]}`}>{PRESSURE_LABEL[pressure]}</span>}
          </div>
          <div className="disk-meter" data-pressure={pressure} data-stale={status === "live" ? undefined : "true"}
            role="meter" aria-label="Guest filesystem used" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct!)} aria-valuetext={`${Math.round(pct!)}% used, ${g ?? ""}`}>
            <span className="disk-meter__fill" style={{ width: `${pct}%` }} />
          </div>
          <div className="text-body" style={{ marginTop: 6 }}>{g}</div>
          <div className="text-meta" style={{ marginTop: 2 }}>Guest filesystem: the space Docker's images, containers, volumes and build cache actually use inside Harpoon.</div>
        </>
      ) : (
        <div className="text-body" style={{ margin: "8px 0" }} role="status">{view.note}</div>
      )}
      {shown && status !== "live" && <div className="text-meta" role="status" style={{ marginTop: 6 }}>{view.note}{status === "stale" ? " These figures may be out of date." : " These are the last figures read."}</div>}
      {host && (
        <div style={{ marginTop: 10 }}>
          <div className="text-body">{host}</div>
          <div className="text-meta">Disk image on macOS (a sparse file). This is Mac storage, not guest usage: it only grows until unused space is reclaimed below.</div>
        </div>
      )}
    </div>
  );
}

/** Header indicator: the same reading, small. Only for a real reading, live or stale; the Resources page explains every other state. */
export function DiskChip({ view, onOpen }: { view: DiskView; onOpen: () => void }) {
  if ((view.status !== "live" && view.status !== "stale") || view.percent === null) return null;
  const pct = Math.round(view.percent);
  const stale = view.status === "stale";
  const label = `Docker disk ${pct}% used${PRESSURE_LABEL[view.pressure] ? `, ${PRESSURE_LABEL[view.pressure].toLowerCase()}` : ""}${stale ? ", reading is stale" : ""}. Open Resources.`;
  return (
    <button type="button" className={`badge ${stale ? "badge--stopped" : PRESSURE_BADGE[view.pressure]} disk-chip`} aria-label={label} title={label} onClick={onOpen}>
      Disk {pct}%{stale ? " · stale" : ""}
    </button>
  );
}

export function ConnectedDiskChip({ onOpen }: { onOpen: () => void }) { return <DiskChip view={useDiskView()} onOpen={onOpen} />; }
