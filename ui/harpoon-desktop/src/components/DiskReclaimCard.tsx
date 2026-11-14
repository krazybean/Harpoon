import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirmAction } from "./ConfirmDialog";
import { formatSize, parseDiskCapacity, parseReclaimResult, reclaimBlockedReason, reclaimConfirmationMessage } from "../lib/diskCapacity";
import type { DiskCapacity, ExpansionState, ReclaimResult } from "../lib/diskCapacity";
import { cleanupActionLabel, confirmLabel, impactBreakdown, freedSummary, groupByApplication, groupCounts, inventorySummary, nothingToClean, operationSummary, parseCache, parseCleanup, reasonLabel, reconcilable, shortDeployment, STATUS_LABEL } from "../lib/artifactCleanup";
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
export function DockerArtifacts({ enabled, onDone }: { enabled: boolean; onDone: () => void }) {
  const [busy, setBusy] = useState<"review" | "apply" | null>(null);
  const [plan, setPlan] = useState<CleanupReport | null>(null);
  const [done, setDone] = useState<{ report: CleanupReport; message: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [clean, setClean] = useState(false);
  // Which application groups are open. Kept across refreshes: a new inventory only changes the rows inside.
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const inFlight = useRef(false);
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { if (clean) closeRef.current?.focus(); }, [clean]);
  const call = (apply: boolean) => invoke<OpResult>("deploy_op", apply ? { op: { op: "cleanup-releases", apply: true }, confirmation: { liveChanges: true, cutover: false } } : { op: { op: "cleanup-releases", apply: false } });
  const review = async () => {
    if (inFlight.current) return; // a second click before the first answer must not start a second request
    inFlight.current = true;
    setBusy("review"); setError(null); setDone(null); setPlan(null); setClean(false);
    try {
      const r = await call(false);
      if (!r.ok) setError(r.hint || r.summary || "The engine could not list what can be cleaned up.");
      else {
        const p = parseCleanup(r.detail);
        setPlan(p);
        // An empty inventory is an answer, so it gets a dialog; items needing review are listed below instead.
        if (nothingToClean(p) && operationSummary(p).review === 0) setClean(true);
      }
    } catch (e) { setError(`Could not list old releases: ${String(e)}`); } finally { inFlight.current = false; setBusy(null); }
  };
  const apply = async () => {
    if (!plan || nothingToClean(plan)) return;
    const sum = operationSummary(plan);
    if (!(await confirmAction(<CleanupImpact report={plan} />, confirmLabel(plan), sum.containers + sum.images > 0))) return;
    setBusy("apply"); setError(null);
    try {
      const r = await call(true);
      if (!r.ok) setError(r.hint || r.summary || "The cleanup did not complete. Nothing already removed is restored; the journal records what was.");
      else { const report = parseCleanup(r.detail); setDone({ report, message: `Removed ${report.containers} container(s) and ${report.images} image(s); reconciled ${reconcilable(report).length} journal record(s).` }); setPlan(null); }
    } catch (e) { setError(String(e)); } finally { setBusy(null); onDone(); }
  };
  const sum = plan ? operationSummary(plan) : null;
  const inv = plan ? inventorySummary(plan) : null;
  const groups = plan ? groupByApplication(plan) : [];
  const actionLabel = plan ? cleanupActionLabel(plan) : null;
  const toggle = (key: string) => setOpen((o) => { const n = new Set(o); if (!n.delete(key)) n.add(key); return n; });
  return (
    <div style={{ marginBottom: 12 }}>
      <div className="text-panel-title">Docker artifacts</div>
      <div className="text-meta" style={{ margin: "6px 0" }}>
        Old deployment containers and images that Harpoon created and that no longer serve anything, and the leftovers of failed deployments. Deployment history is kept. Running releases and persistent volumes are never removed.
      </div>
      <div className="dialog-actions">
        <button className="btn btn--secondary" disabled={!enabled || busy !== null} onClick={review}>{busy === "review" ? "Looking…" : "Review old releases…"}</button>
        {plan && actionLabel && <button className="btn btn--primary" disabled={busy !== null} onClick={apply}>{busy === "apply" ? "Cleaning up…" : actionLabel}</button>}
      </div>
      {busy === "review" && <div role="status" className="text-meta">Looking for old releases…</div>}
      {clean && (
        <div className="dialog-scrim" role="presentation" onClick={() => setClean(false)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="clean-title" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => { if (e.key === "Escape") setClean(false); }}>
            <div id="clean-title" className="dialog-title">Everything is clean</div>
            <div className="dialog-body">No obsolete deployment artifacts were found. Running releases and persistent volumes remain protected, and nothing was changed.</div>
            <div className="dialog-actions"><button ref={closeRef} className="btn btn--secondary" onClick={() => setClean(false)}>Close</button></div>
          </div>
        </div>
      )}
      {plan && nothingToClean(plan) && <div role="status" className="text-meta">Nothing to clean up: only what is serving now is left{sum && sum.review ? `, and ${sum.review} item${sum.review === 1 ? "" : "s"} need${sum.review === 1 ? "s" : ""} review below` : ""}.</div>}
      {plan && inv && (
        <div role="region" aria-label="Cleanup candidates by application" style={{ display: "grid", gap: 8, margin: "8px 0" }}>
          <div className="artifact-metrics">
            {([["Total artifacts", inv.total, "all records", ""], ["Eligible for cleanup", inv.eligible, "can be cleaned", " artifact-metric--eligible"], ["Protected", inv.protectedCount, "left alone", " artifact-metric--protected"]] as const).map(([label, n, note, tone]) => (
              <div key={label} className={`artifact-metric${tone}`}><div className="artifact-metric__label">{label}</div><b className="artifact-metric__n">{n}</b><div className="artifact-metric__note">{note}</div></div>
            ))}
          </div>
          <div className="text-meta">{inv.dockerObjects} Docker object{inv.dockerObjects === 1 ? "" : "s"} to remove · {inv.journalOnly} journal record{inv.journalOnly === 1 ? "" : "s"} to reconcile · {inv.review} need review. Reconciling journal records frees no Docker disk space; disk savings cannot be estimated before removal, and the measured change is shown afterwards.</div>
          {groups.length > 1 && (
            <div style={{ display: "flex", gap: 12, justifyContent: "flex-end" }}>
              <button type="button" className="artifact-link" onClick={() => setOpen(new Set(groups.map((g) => g.key)))}>Expand all</button>
              <button type="button" className="artifact-link" onClick={() => setOpen(new Set())}>Collapse all</button>
            </div>
          )}
          {groups.map((g) => <ArtifactGroup key={g.key} g={g} expanded={open.has(g.key)} onToggle={() => toggle(g.key)} />)}
        </div>
      )}
      {done && <div role="status">{done.message}<div className="text-meta">{freedSummary(done.report, formatSize)}</div></div>}
      {error && <div role="alert" style={{ color: "var(--error-fg)", whiteSpace: "pre-wrap" }}>{error}</div>}
      <BuildCache enabled={enabled} onDone={onDone} />
    </div>
  );
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The confirmation's body: who is affected, exact counts per category, what stays protected, and whether Docker objects are deleted. */
export function CleanupImpact({ report }: { report: CleanupReport }) {
  const sum = operationSummary(report);
  const b = impactBreakdown(report);
  const docker = sum.containers + sum.images > 0;
  const rows: [string, number][] = [
    ["Stale port reservations", b.stalePortLeases], ["References to already-removed containers", b.goneContainers], ["References to already-removed images", b.goneImages],
    ["Docker containers to delete", b.removeContainers], ["Docker images to delete", b.removeImages],
  ];
  return (
    <div className="cleanup-impact">
      <div>
        <div className="cleanup-impact__head">Affected applications</div>
        <ul className="cleanup-impact__apps">{b.applications.map((a) => <li key={a.title}><b>{a.title}</b>{a.environment ? ` · ${a.environment === "production" ? "Production" : a.environment}` : ""}</li>)}</ul>
      </div>
      <div className="cleanup-impact__tiles">
        <div className="artifact-metric artifact-metric--eligible"><b className="artifact-metric__n">{sum.journalOnly}</b><div className="artifact-metric__label">Journal records to reconcile</div></div>
        <div className={`artifact-metric${sum.containers ? " artifact-metric--remove" : ""}`}><b className="artifact-metric__n">{sum.containers}</b><div className="artifact-metric__label">Docker containers to remove</div></div>
        <div className={`artifact-metric${sum.images ? " artifact-metric--remove" : ""}`}><b className="artifact-metric__n">{sum.images}</b><div className="artifact-metric__label">Docker images to remove</div></div>
        <div className="artifact-metric artifact-metric--protected"><b className="artifact-metric__n">{sum.protectedCount}</b><div className="artifact-metric__label">Protected resources</div></div>
      </div>
      <div>
        <div className="cleanup-impact__head">Changes</div>
        <ul className="cleanup-impact__list">{rows.filter(([, n]) => n > 0).map(([label, n]) => <li key={label}><span>{label}</span><b>{n}</b></li>)}</ul>
        <div className="text-meta">{docker ? `${plural(sum.containers + sum.images, "Docker object")} will be deleted. Image sizes are shared between images, so the disk space freed cannot be predicted; it is measured afterwards.` : "No Docker objects will be deleted. Reconciling journal records frees no Docker disk space."}</div>
        {docker && sum.images > 0 && <div className="text-meta">A release whose image is deleted can no longer be rolled back to instantly; it can be rebuilt from its recorded commit.</div>}
        <details className="cleanup-impact__records">
          <summary className="text-meta">Show affected records ({b.records.length})</summary>
          <ul>{b.records.map((i) => <li key={`${i.deployment}/${i.kind}/${i.name}`} className="text-meta mono">{reasonLabel(i)}<br />{i.kind} {i.name} · {i.deployment}</li>)}</ul>
        </details>
      </div>
      <div>
        <div className="cleanup-impact__head cleanup-impact__head--protected">Protected</div>
        <div className="text-meta">The running release and the image it is built from stay in place{sum.protectedCount ? ` (${plural(sum.protectedCount, "protected resource")})` : ""}. Persistent volumes are never touched. Deployment history stays in the journal.{sum.review ? ` ${plural(sum.review, "item")} needing review ${sum.review === 1 ? "is" : "are"} left alone.` : ""}</div>
      </div>
    </div>
  );
}

const VERDICT_PILL: Record<Verdict, string> = { ready: "dp-pill--ok", reconcile: "dp-pill--info", protected: "", review: "dp-pill--warn" };

/** One application: a collapsed row with its counts; expanding lists one plain row per artifact, the engine's raw detail behind a disclosure. */
function ArtifactGroup({ g, expanded, onToggle }: { g: CleanupGroup; expanded: boolean; onToggle: () => void }) {
  const id = `artifact-group-${g.key}`;
  return (
    <section className="artifact-group" data-expanded={expanded} data-tone={g.ready.containers + g.ready.images + g.reconcile > 0 ? "eligible" : "protected"} aria-label={g.title}>
      <button type="button" aria-expanded={expanded} aria-controls={id} onClick={onToggle} className="artifact-toggle">
        <span className="artifact-chevron" aria-hidden="true" />
        <span style={{ minWidth: 0, flex: 1 }}>
          <span style={{ display: "flex", gap: 8, alignItems: "baseline", flexWrap: "wrap" }}>
            <b style={{ overflowWrap: "anywhere" }}>{g.title}</b>
            {g.environment && <span className="text-meta">· {g.environment === "production" ? "Production" : g.environment}</span>}
          </span>
          <span className="text-meta" style={{ display: "block" }}>{groupCounts(g)}</span>
        </span>
      </button>
      {expanded && <div id={id} className="artifact-group__body">
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
      </div>}
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
