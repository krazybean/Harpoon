import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirmAction } from "./ConfirmDialog";
import { formatSize, parseDiskCapacity, parseReclaimResult, reclaimBlockedReason, reclaimConfirmationMessage } from "../lib/diskCapacity";
import type { DiskCapacity, ExpansionState, ReclaimResult } from "../lib/diskCapacity";
import { freedSummary, nothingToClean, parseCache, parseCleanup } from "../lib/artifactCleanup";
import type { CacheReport, CleanupReport } from "../lib/artifactCleanup";
import type { OpResult } from "../lib/deployEngine";

/**
 * "Reclaim unused space": shows the virtual capacity and the APFS allocation, and on explicit confirmation asks the runtime to run
 * `fstrim /` in the guest (through the management channel; no restart, no Docker pruning). The result is the measured change in
 * allocation, including zero. Nothing runs without the confirmation, and no amount is estimated beforehand.
 */
/**
 * A. Docker artifacts: superseded and abandoned deployment containers and images that Harpoon itself created, found
 * through its journal. It always lists first; nothing is removed until the list is confirmed. The serving release,
 * volumes, networks and other applications' images are never in it, and Docker's own cache is not touched here.
 */
function DockerArtifacts({ enabled, onDone }: { enabled: boolean; onDone: () => void }) {
  const [busy, setBusy] = useState<"review" | "apply" | null>(null);
  const [plan, setPlan] = useState<CleanupReport | null>(null);
  const [done, setDone] = useState<{ report: CleanupReport; ok: boolean; message: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const call = (apply: boolean) => invoke<OpResult>("deploy_op", apply ? { op: { op: "cleanup-releases", apply: true }, confirmation: { liveChanges: true, cutover: false } } : { op: { op: "cleanup-releases", apply: false } });
  const review = async () => {
    setBusy("review"); setError(null); setDone(null); setPlan(null);
    try {
      const r = await call(false);
      if (!r.ok) setError(r.hint || r.summary || "The engine could not list what can be cleaned up.");
      else setPlan(parseCleanup(r.detail));
    } catch (e) { setError(String(e)); } finally { setBusy(null); }
  };
  const apply = async () => {
    if (!plan || nothingToClean(plan)) return;
    const lines = plan.actions.slice(0, 12).join("\n");
    if (!(await confirmAction(`Harpoon will remove these artifacts it created earlier:\n\n${lines}${plan.actions.length > 12 ? `\n… and ${plan.actions.length - 12} more` : ""}\n\nThe release serving now, your volumes, networks and Docker's build cache are not touched. A release whose image is removed can no longer be rolled back to instantly; it can be rebuilt from its recorded commit.`, "Clean up old releases"))) return;
    setBusy("apply"); setError(null);
    try {
      const r = await call(true);
      if (!r.ok) setError(r.hint || r.summary || "The cleanup did not complete. Nothing already removed is restored; the journal records what was.");
      else { const report = parseCleanup(r.detail); setDone({ report, ok: true, message: `Removed ${report.containers} container(s) and ${report.images} image(s).` }); setPlan(null); }
    } catch (e) { setError(String(e)); } finally { setBusy(null); onDone(); }
  };
  return (
    <div style={{ marginBottom: 12 }}>
      <div className="text-panel-title">Docker artifacts</div>
      <div className="text-meta" style={{ margin: "6px 0" }}>
        Old deployment containers and images that Harpoon created and that no longer serve anything, and the leftovers of failed deployments. Deployment history is kept.
      </div>
      <div className="dialog-actions">
        <button className="btn btn--secondary" disabled={!enabled || busy !== null} onClick={review}>{busy === "review" ? "Looking…" : "Review old releases…"}</button>
        {plan && !nothingToClean(plan) && <button className="btn btn--secondary" disabled={busy !== null} onClick={apply}>{busy === "apply" ? "Removing…" : `Remove ${plan.actions.length} item(s)…`}</button>}
      </div>
      {plan && nothingToClean(plan) && <div role="status" className="text-meta">Nothing to clean up: only what is serving now is left.</div>}
      {plan && !nothingToClean(plan) && (
        <ul className="text-meta" style={{ margin: "6px 0", paddingLeft: 18 }}>
          {plan.actions.map((a) => <li key={a} className="mono">{a}</li>)}
          {plan.kept.map((k) => <li key={k}>Kept: {k}</li>)}
        </ul>
      )}
      {done && <div role="status">{done.message}<div className="text-meta">{freedSummary(done.report, formatSize)}</div></div>}
      {error && <div role="alert" style={{ color: "var(--error-fg)", whiteSpace: "pre-wrap" }}>{error}</div>}
      <BuildCache enabled={enabled} onDone={onDone} />
    </div>
  );
}

/** BuildKit's layer cache: separate from images, never pruned automatically, and only after its own confirmation. */
function BuildCache({ enabled, onDone }: { enabled: boolean; onDone: () => void }) {
  const [busy, setBusy] = useState<"check" | "prune" | null>(null);
  const [size, setSize] = useState<number | null>(null);
  const [done, setDone] = useState<CacheReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const call = (apply: boolean) => invoke<OpResult>("deploy_op", apply ? { op: { op: "prune-build-cache", apply: true }, confirmation: { liveChanges: true, cutover: false } } : { op: { op: "prune-build-cache", apply: false } });
  const check = async () => {
    setBusy("check"); setError(null); setDone(null);
    try { const r = await call(false); if (!r.ok) setError(r.hint || r.summary); else setSize(parseCache(r.detail).reclaimable); }
    catch (e) { setError(String(e)); } finally { setBusy(null); }
  };
  const prune = async () => {
    if (!(await confirmAction(`Harpoon will ask Docker to delete ${size !== null ? `about ${formatSize(size)} of ` : ""}BuildKit cache that nothing is using.\n\nImages, containers and volumes are not touched. The next builds will be slower while the cache fills again.`, "Prune build cache"))) return;
    setBusy("prune"); setError(null);
    try { const r = await call(true); if (!r.ok) setError(r.hint || r.summary); else { setDone(parseCache(r.detail)); setSize(null); } }
    catch (e) { setError(String(e)); } finally { setBusy(null); onDone(); }
  };
  return (
    <div style={{ marginTop: 10 }}>
      <div className="text-meta">Build cache speeds up rebuilds. Deleting it frees space but makes the next builds slower.</div>
      <div className="dialog-actions">
        <button className="btn btn--secondary" disabled={!enabled || busy !== null} onClick={check}>{busy === "check" ? "Checking…" : "Check build cache"}</button>
        {size !== null && size > 0 && <button className="btn btn--secondary" disabled={busy !== null} onClick={prune}>{busy === "prune" ? "Pruning…" : `Prune ${formatSize(size)}…`}</button>}
      </div>
      {size === 0 && <div role="status" className="text-meta">No unused build cache.</div>}
      {done && <div role="status">Docker reports {formatSize(done.reclaimed ?? 0)} of build cache removed.<div className="text-meta">{freedSummary({ actions: [], protectedCount: 0, kept: [], containers: 0, images: 0, guestFreeBefore: done.guestFreeBefore, guestFreeAfter: done.guestFreeAfter }, formatSize)}</div></div>}
      {error && <div role="alert" style={{ color: "var(--error-fg)", whiteSpace: "pre-wrap" }}>{error}</div>}
    </div>
  );
}

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
      <DockerArtifacts enabled={enabled} onDone={() => void load()} />
      <div className="text-panel-title">Guest filesystem</div>
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
