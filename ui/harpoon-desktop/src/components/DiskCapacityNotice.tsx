import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirmAction } from "./ConfirmDialog";
import {
  assessSize, confirmationMessage, customSizeError, dismissalFor, formatSize, noticeLevel, parseDiskCapacity, sizeArg, summary, GIB,
} from "../lib/diskCapacity";
import type { DiskCapacity, Dismissal } from "../lib/diskCapacity";

const POLL_MS = 60_000; // one `df` over the management channel per minute, only while Harpoon is running and the window is visible
const KEY = "harpoon.diskNotice.dismissal";

function loadDismissal(): Dismissal | null {
  try { const raw = window.localStorage.getItem(KEY); return raw ? (JSON.parse(raw) as Dismissal) : null; } catch { return null; }
}
function saveDismissal(d: Dismissal | null) {
  try { if (d) window.localStorage.setItem(KEY, JSON.stringify(d)); else window.localStorage.removeItem(KEY); } catch { /* per-viewer convenience only */ }
}

/**
 * Low-disk notice. It never resizes or restarts anything by itself: "Not now" only remembers the dismissal, "Review expansion" opens the
 * choices, and the VM is touched only after the confirmation dialog is accepted. When the desktop is not connected to the runtime
 * (`enabled` false) it renders nothing and polls nothing.
 */
export function DiskCapacityNotice({ enabled, onExpanded }: { enabled: boolean; onExpanded: () => void }) {
  const [cap, setCap] = useState<DiskCapacity | null>(null);
  const [dismissal, setDismissal] = useState<Dismissal | null>(() => loadDismissal());
  const [reviewing, setReviewing] = useState(false);
  const [choice, setChoice] = useState<number | "custom" | null>(null);
  const [custom, setCustom] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const busyRef = useRef(false);
  busyRef.current = busy;

  const poll = useCallback(async () => {
    if (busyRef.current) return;
    try { setCap(parseDiskCapacity(await invoke<string>("get_disk_capacity"))); } catch { /* status is shown elsewhere; stay quiet */ }
  }, []);

  useEffect(() => {
    if (!enabled) { if (!busyRef.current) setCap(null); return; }
    void poll();
    const id = window.setInterval(() => { if (document.visibilityState === "visible") void poll(); }, POLL_MS);
    return () => window.clearInterval(id);
  }, [enabled, poll]);

  const level = busy ? "none" : noticeLevel(cap, dismissal, Date.now());
  // While an expansion runs Harpoon is stopped for a while, so `enabled` goes false; keep the progress and the outcome on screen.
  if (!busy && !result && (!enabled || level === "none")) return null;

  const requested: number | null = choice === "custom" ? (custom.trim() && /^\d+$/.test(custom.trim()) ? Number(custom.trim()) * GIB : null) : choice;
  const customError = choice === "custom" && cap ? customSizeError(custom, cap) : null;
  const canConfirm = Boolean(cap && requested && !busy && !customError);

  const dismiss = () => {
    if (!cap) return;
    const d = dismissalFor(cap, Date.now());
    saveDismissal(d); setDismissal(d); setReviewing(false); setChoice(null);
  };

  const confirm = async () => {
    if (!cap || !requested) return;
    // The only path to a lifecycle action: an explicit confirmation that says the containers are interrupted.
    const ok = await confirmAction(confirmationMessage(cap, requested), "Restart and expand");
    if (!ok) return;
    setBusy(true); setResult(null);
    try {
      await invoke("expand_disk_confirmed", { size: sizeArg(requested) });
      saveDismissal(null); setDismissal(null);
      setResult({ ok: true, text: `Docker disk expanded to ${formatSize(requested)} and Docker is ready.` });
      setReviewing(false); setChoice(null);
    } catch (e) {
      setResult({ ok: false, text: String(e) });
    } finally {
      setBusy(false);
      onExpanded();
      void poll();
    }
  };

  return (
    <div role={level === "critical" ? "alert" : "status"} className={level === "critical" ? "disk-notice disk-notice--critical" : "disk-notice"}>
      {busy && <div>Expanding the Docker disk: stopping Harpoon, growing the disk, starting it again. This can take a few minutes; running containers are interrupted.</div>}
      {!busy && result && (
        <div>
          <div style={{ color: result.ok ? "var(--text-primary)" : "var(--error-fg)", whiteSpace: "pre-wrap" }}>{result.text}</div>
          <div className="dialog-actions"><button className="btn btn--secondary" onClick={() => setResult(null)}>Dismiss</button></div>
        </div>
      )}
      {!busy && !result && enabled && cap && (
        <>
          <div>
            <strong>{level === "critical" ? "Docker disk is almost full." : "Docker disk is running low."}</strong>{" "}
            {summary(cap)}{cap.hostAvailableBytes !== undefined && <> Host has {formatSize(cap.hostAvailableBytes)} free.</>}
          </div>
          {!reviewing && (
            <div className="dialog-actions">
              <button className="btn btn--secondary" onClick={dismiss}>Not now</button>
              <button className="btn btn--primary" onClick={() => { setReviewing(true); setChoice(cap.suggestedSizes[0] ?? "custom"); }}>Review expansion</button>
            </div>
          )}
          {reviewing && (
            <div>
              <div className="text-meta" style={{ margin: "8px 0" }}>
                Growing the disk restarts Harpoon and interrupts running containers. Nothing happens until you confirm.
                Current capacity {formatSize(cap.logicalBytes)}.
              </div>
              <fieldset style={{ border: 0, padding: 0, margin: 0 }} disabled={busy}>
                <legend className="text-meta">New capacity</legend>
                {cap.suggestedSizes.map((s) => (
                  <label key={s} style={{ display: "block", margin: "4px 0" }}>
                    <input type="radio" name="disk-size" checked={choice === s} onChange={() => setChoice(s)} /> {formatSize(s)}
                    {assessSize(s, cap).oversubscribed && <span className="text-meta"> — more than this Mac can spare if it ever fills</span>}
                  </label>
                ))}
                <label style={{ display: "block", margin: "4px 0" }}>
                  <input type="radio" name="disk-size" checked={choice === "custom"} onChange={() => setChoice("custom")} /> Custom{" "}
                  <input aria-label="Custom size in GiB" inputMode="numeric" style={{ width: 64 }} value={custom} onFocus={() => setChoice("custom")} onChange={(e) => setCustom(e.target.value)} /> GiB
                </label>
                {cap.suggestedSizes.length === 0 && <div className="text-meta">No larger size fits the host free space and the growth cap; free space on this Mac first.</div>}
                {customError && <div role="alert" style={{ color: "var(--error-fg)" }}>{customError}</div>}
              </fieldset>
              <div className="dialog-actions">
                <button className="btn btn--secondary" onClick={dismiss}>Not now</button>
                <button className="btn btn--primary" disabled={!canConfirm} onClick={confirm}>Confirm expansion…</button>
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}
