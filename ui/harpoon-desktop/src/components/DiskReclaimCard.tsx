import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirmAction } from "./ConfirmDialog";
import { formatSize, parseDiskCapacity, parseReclaimResult, reclaimBlockedReason, reclaimConfirmationMessage } from "../lib/diskCapacity";
import type { DiskCapacity, ExpansionState, ReclaimResult } from "../lib/diskCapacity";

/**
 * "Reclaim unused space": shows the virtual capacity and the APFS allocation, and on explicit confirmation asks the runtime to run
 * `fstrim /` in the guest (through the management channel; no restart, no Docker pruning). The result is the measured change in
 * allocation, including zero. Nothing runs without the confirmation, and no amount is estimated beforehand.
 */
export function DiskReclaimCard({ enabled, expansion }: { enabled: boolean; expansion: ExpansionState | null | undefined }) {
  const [cap, setCap] = useState<DiskCapacity | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ReclaimResult | null>(null);
  const busyRef = useRef(false);
  busyRef.current = busy;

  const load = useCallback(async () => {
    if (busyRef.current) return;
    try { setCap(parseDiskCapacity(await invoke<string>("get_disk_capacity"))); } catch { setCap(null); }
  }, []);
  useEffect(() => { if (enabled) void load(); }, [enabled, load, expansion?.phase]);

  const blocked = reclaimBlockedReason(enabled ? cap : null, expansion);

  const reclaim = async () => {
    if (!cap || blocked || busy) return;
    if (!(await confirmAction(reclaimConfirmationMessage(cap), "Reclaim unused space"))) return;
    setBusy(true); setResult(null);
    try { setResult(parseReclaimResult(await invoke<string>("reclaim_disk_confirmed"))); }
    catch (e) { setResult(parseReclaimResult(String(e))); }
    finally { setBusy(false); void load(); }
  };

  const ok = result && (result.status === "reclaimed" || result.status === "nothing-reclaimed");
  return (
    <div className="panel" style={{ padding: 14 }}>
      <div className="text-panel-title">Docker disk space</div>
      <div className="text-body" style={{ lineHeight: 1.7, marginTop: 6 }}>
        {cap && cap.provisioned
          ? <>Capacity {formatSize(cap.logicalBytes)} • {formatSize(cap.physicalBytes)} currently allocated on this Mac</>
          : <>Capacity and allocation appear once the disk exists.</>}
      </div>
      <div className="text-meta" style={{ margin: "6px 0" }}>
        Reclaiming asks the guest to hand blocks it no longer uses back to macOS. It does not delete Docker images, containers or volumes, and the amount returned is only known afterwards.
      </div>
      <div className="dialog-actions">
        <button className="btn btn--secondary" disabled={Boolean(blocked) || busy} title={blocked ?? undefined} onClick={reclaim}>
          {busy ? "Reclaiming…" : "Reclaim unused space…"}
        </button>
      </div>
      {blocked && !busy && <div className="text-meta">{blocked}</div>}
      {busy && <div role="status" className="text-meta">Trimming the guest disk. Harpoon keeps running.</div>}
      {result && !busy && (
        <div role={ok ? "status" : "alert"} style={{ marginTop: 8, color: ok ? "var(--text-primary)" : "var(--error-fg)", whiteSpace: "pre-wrap" }}>
          {result.message}
          {ok && result.physicalBeforeBytes !== undefined && result.physicalAfterBytes !== undefined && (
            <div className="text-meta">Allocated before {formatSize(result.physicalBeforeBytes)} • after {formatSize(result.physicalAfterBytes)} • capacity {formatSize(result.logicalBytes ?? cap?.logicalBytes ?? 0)} unchanged</div>
          )}
        </div>
      )}
    </div>
  );
}
