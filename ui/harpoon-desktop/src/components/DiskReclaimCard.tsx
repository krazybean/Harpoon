import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirmAction } from "./ConfirmDialog";
import { formatSize, parseDiskCapacity, parseReclaimResult, reclaimBlockedReason, reclaimConfirmationMessage } from "../lib/diskCapacity";
import type { DiskCapacity, ExpansionState, ReclaimResult } from "../lib/diskCapacity";
import { freedSummary, groupByApplication, nothingToClean, operationSummary, parseCache, parseCleanup, reasonLabel, reconcilable, shortDeployment, STATUS_LABEL } from "../lib/artifactCleanup";
import type { CacheReport, CleanupGroup, CleanupReport, Verdict } from "../lib/artifactCleanup";
import type { OpResult } from "../lib/deployEngine";

/**
 * "Reclaim unused space": shows the virtual capacity and the APFS allocation, and on explicit confirmation asks the runtime to run
 * `fstrim /` in the guest (through the management channel; no restart, no Docker pruning). The result is the measured change in
 * allocation, including zero. Nothing runs without the confirmation, and no amount is estimated beforehand.
 */
/**
 * A. Docker artifacts: superseded and abandoned deployment containers and images that Harpoon itself created. The
 * engine names the application, deployment and release each belongs to and decides every verdict; this groups them by
 * application and says in plain words what each is and what the operation will change. It always lists first; nothing
 * is removed until the summary is confirmed. Running releases, volumes, networks and other applications' images are
 * never in the operation, and Docker's own cache is handled separately below.
 */
function DockerArtifacts({ enabled, onDone }: { enabled: boolean; onDone: () => void }) {
  const [busy, setBusy] = useState<"review" | "apply" | null>(null);
  const [plan, setPlan] = useState<CleanupReport | null>(null);
  const [done, setDone] = useState<{ report: CleanupReport; message: string } | null>(null);
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
    const sum = operationSummary(plan);
    const parts = [
      `Applications affected: ${sum.apps.join(", ")}.`,
      `Docker objects Harpoon will remove: ${sum.containers} container${sum.containers === 1 ? "" : "s"} and ${sum.images} image${sum.images === 1 ? "" : "s"}.`,
      `Journal records that only need reconciling: ${sum.journalOnly}. Their objects are already gone, so this frees no disk space.`,
      `Protected and left alone: ${sum.protectedCount} (running releases${sum.review ? `; ${sum.review} more need review` : ""}). Persistent volumes are never touched.`,
      "Harpoon cannot tell beforehand how much disk space this frees, because images share layers. It measures the guest's free space afterwards.",
      "A release whose image is removed can no longer be rolled back to instantly; it can be rebuilt from its recorded commit.",
    ];
    if (!(await confirmAction(parts.join("\n\n"), "Clean up old releases"))) return;
    setBusy("apply"); setError(null);
    try {
      const r = await call(true);
      if (!r.ok) setError(r.hint || r.summary || "The cleanup did not complete. Nothing already removed is restored; the journal records what was.");
      else { const report = parseCleanup(r.detail); setDone({ report, message: `Removed ${report.containers} container(s) and ${report.images} image(s); reconciled ${reconcilable(report).length} journal record(s).` }); setPlan(null); }
    } catch (e) { setError(String(e)); } finally { setBusy(null); onDone(); }
  };
  const sum = plan ? operationSummary(plan) : null;
  return (
    <div style={{ marginBottom: 12 }}>
      <div className="text-panel-title">Docker artifacts</div>
      <div className="text-meta" style={{ margin: "6px 0" }}>
        Old deployment containers and images that Harpoon created and that no longer serve anything, and the leftovers of failed deployments. Deployment history is kept. Running releases and persistent volumes are never removed.
      </div>
      <div className="dialog-actions">
        <button className="btn btn--secondary" disabled={!enabled || busy !== null} onClick={review}>{busy === "review" ? "Looking…" : "Review old releases…"}</button>
        {plan && sum && !nothingToClean(plan) && <button className="btn btn--secondary" disabled={busy !== null} onClick={apply}>{busy === "apply" ? "Cleaning up…" : "Review and clean up…"}</button>}
      </div>
      {plan && nothingToClean(plan) && <div role="status" className="text-meta">Nothing to clean up: only what is serving now is left{sum && sum.review ? `, and ${sum.review} item${sum.review === 1 ? "" : "s"} need${sum.review === 1 ? "s" : ""} review below` : ""}.</div>}
      {plan && sum && (
        <div role="region" aria-label="Cleanup candidates by application" style={{ display: "grid", gap: 8, margin: "8px 0" }}>
          <div className="text-meta">{sum.containers + sum.images} Docker object{sum.containers + sum.images === 1 ? "" : "s"} to remove ({sum.containers} container{sum.containers === 1 ? "" : "s"}, {sum.images} image{sum.images === 1 ? "" : "s"}) · {sum.journalOnly} journal record{sum.journalOnly === 1 ? "" : "s"} to reconcile (no disk space) · {sum.protectedCount} protected · {sum.review} need review. Disk savings cannot be estimated before removal; the measured change is shown afterwards.</div>
          {groupByApplication(plan).map((g) => <ArtifactGroup key={g.key} g={g} />)}
        </div>
      )}
      {done && <div role="status">{done.message}<div className="text-meta">{freedSummary(done.report, formatSize)}</div></div>}
      {error && <div role="alert" style={{ color: "var(--error-fg)", whiteSpace: "pre-wrap" }}>{error}</div>}
      <BuildCache enabled={enabled} onDone={onDone} />
    </div>
  );
}

const VERDICT_PILL: Record<Verdict, string> = { ready: "dp-pill--ok", reconcile: "dp-pill--info", protected: "", review: "dp-pill--warn" };

/** One application's candidates: the headline counts, one plain row per artifact, and the engine's raw detail behind a disclosure. */
function ArtifactGroup({ g }: { g: CleanupGroup }) {
  const bits = [
    g.ready.containers ? `${g.ready.containers} stopped container${g.ready.containers === 1 ? "" : "s"}` : "",
    g.ready.images ? `${g.ready.images} unused image${g.ready.images === 1 ? "" : "s"}` : "",
    g.reconcile ? `${g.reconcile} journal record${g.reconcile === 1 ? "" : "s"} to reconcile` : "",
    g.protectedCount ? `${g.protectedCount} protected` : "",
    g.review ? `${g.review} to review` : "",
  ].filter(Boolean);
  return (
    <section className="panel" style={{ padding: 10 }} aria-label={g.title}>
      <div style={{ display: "flex", gap: 8, alignItems: "baseline", flexWrap: "wrap" }}>
        <b>{g.title}</b>
        {g.environment && <span className="text-meta">{g.environment === "production" ? "Production" : g.environment}</span>}
        <span className="text-meta">{bits.join(" · ")}</span>
      </div>
      {g.unassociated && <div className="text-meta">Harpoon's journal does not say which application these belong to, so none is guessed.</div>}
      <ul style={{ listStyle: "none", margin: "6px 0 0", padding: 0, display: "grid", gap: 6 }}>
        {g.items.map((i) => (
          <li key={`${i.deployment}/${i.kind}/${i.name}`}>
            <div style={{ display: "flex", gap: 8, alignItems: "baseline", flexWrap: "wrap" }}>
              <span className={`dp-pill ${VERDICT_PILL[i.verdict]}`}>{STATUS_LABEL[i.verdict]}</span>
              <span>{reasonLabel(i)}</span>
            </div>
            <div className="text-meta">
              {i.commit ? <>revision <span className="mono">{i.commit}</span> · </> : null}{i.createdMs ? `${new Date(i.createdMs).toLocaleString()} · ` : ""}<span className="mono">{shortDeployment(i.deployment)}</span>
              {" · "}{i.physical === true ? "present in Docker" : i.physical === false ? "already gone from Docker (journal only)" : "journal entry only"}
            </div>
            <details>
              <summary className="text-meta">Technical details</summary>
              <div className="text-meta mono" style={{ overflowWrap: "anywhere" }}>
                {i.kind} {i.name}<br />deployment {i.deployment} ({i.status})<br />application {i.app}<br />engine: {i.reason || "—"}: {i.detail}
              </div>
            </details>
          </li>
        ))}
      </ul>
    </section>
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
      {done && <div role="status">Docker reports {formatSize(done.reclaimed ?? 0)} of build cache removed.<div className="text-meta">{freedSummary({ guestFreeBefore: done.guestFreeBefore, guestFreeAfter: done.guestFreeAfter }, formatSize)}</div></div>}
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
