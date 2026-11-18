import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirmAction } from "./ConfirmDialog";
import { useDiskView, diskTelemetry, hostAllocationNow } from "../hooks/useDiskTelemetry";
import { CleanupResult } from "./CleanupResult";
import { cacheOutcome, failureOutcome, releasesOutcome, reclaimOutcomeFromText } from "../lib/cleanupResult";
import type { CleanupOutcome } from "../lib/cleanupResult";
import { fmtBytes } from "./StorageBreakdown";
import { CLEANUP_TARGETS } from "../lib/cleanupTargets";
import { acquireCleanup, useCleanupActive } from "../lib/cleanupLock";
import { formatSize, reclaimBlockedReason, reclaimConfirmationMessage } from "../lib/diskCapacity";
import type { ExpansionState } from "../lib/diskCapacity";
import { cleanupActionLabel, confirmLabel, impactBreakdown, groupByApplication, groupCounts, cacheExplanation, inventorySummary, nothingToClean, operationSummary, parseCache, parseCleanup, reasonLabel, reconcilable, shortDeployment, STATUS_LABEL } from "../lib/artifactCleanup";
import type { CleanupGroup, CleanupReport, Verdict } from "../lib/artifactCleanup";
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
  const [outcome, setOutcome] = useState<CleanupOutcome | null>(null);
  const otherCleanup = useCleanupActive() !== null;
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
    setBusy("review"); setError(null); setOutcome(null); setPlan(null); setClean(false);
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
    // one apply at a time, from the click through the confirmation to the result: a second click or a repeated confirmation starts nothing
    if (!plan || nothingToClean(plan) || inFlight.current) return;
    const release = acquireCleanup("old-releases"); // another cleanup running would land inside this one's measurements
    if (!release) return;
    inFlight.current = true;
    try {
      const sum = operationSummary(plan);
      if (!(await confirmAction(<CleanupImpact report={plan} />, confirmLabel(plan), sum.containers + sum.images > 0))) return;
      setBusy("apply"); setError(null); setOutcome(null);
      // The reviewed list is only what the person saw: the engine re-checks every object when it applies. The list is dropped either way, so a
      // result (even a failed or unknown one) can never be applied again from a plan that may no longer be true.
      const before = await hostAllocationNow();
      let result: CleanupOutcome;
      try {
        const r = await call(true);
        const after = await hostAllocationNow();
        result = r.ok ? releasesOutcome(plan, parseCleanup(r.detail), { before, after }) : failureOutcome("old-releases", r, null, { before, after });
      } catch (e) { result = failureOutcome("old-releases", null, e, { before, after: await hostAllocationNow() }); }
      setOutcome(result); setPlan(null);
    } finally { inFlight.current = false; release(); setBusy(null); onDone(); }
  };
  const sum = plan ? operationSummary(plan) : null;
  const inv = plan ? inventorySummary(plan) : null;
  const groups = plan ? groupByApplication(plan) : [];
  const actionLabel = plan ? cleanupActionLabel(plan) : null;
  const toggle = (key: string) => setOpen((o) => { const n = new Set(o); if (!n.delete(key)) n.add(key); return n; });
  return (
    <div id={CLEANUP_TARGETS["old-releases"].section} tabIndex={-1} style={{ marginBottom: 12 }}>
      <div className="text-panel-title">Docker artifacts</div>
      <div className="text-meta" style={{ margin: "6px 0" }}>
        Old deployment containers and images that Harpoon created and that no longer serve anything, and the leftovers of failed deployments. Deployment history is kept. Running releases and persistent volumes are never removed.
      </div>
      <div className="dialog-actions">
        <button id={CLEANUP_TARGETS["old-releases"].button} className="btn btn--secondary" disabled={!enabled || busy !== null} onClick={review}>{busy === "review" ? "Looking…" : "Review old releases…"}</button>
        {plan && actionLabel && <button className="btn btn--primary" disabled={busy !== null || otherCleanup} onClick={apply}>{busy === "apply" ? "Cleaning up…" : actionLabel}</button>}
      </div>
      {busy === "review" && <div role="status" className="text-meta">Looking for old releases…</div>}
      {clean && (
        <div className="dialog-scrim" role="presentation" onClick={() => setClean(false)}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="clean-title" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => { if (e.key === "Escape") setClean(false); }}>
            <div id="clean-title" className="dialog-title">No obsolete deployment artifacts found</div>
            <div className="dialog-body">No obsolete Harpoon deployment artifacts were found. This does not mean Docker storage is clean: other categories, such as build cache and images Harpoon did not deploy, may still contain reclaimable space. Running releases and persistent volumes remain protected, and nothing was changed.</div>
            <div className="dialog-actions"><button ref={closeRef} className="btn btn--secondary" onClick={() => setClean(false)}>Close</button></div>
          </div>
        </div>
      )}
      {plan && nothingToClean(plan) && <div role="status" className="text-meta">No obsolete Harpoon deployment artifacts found: only what is serving now is left{sum && sum.review ? `, and ${sum.review} item${sum.review === 1 ? "" : "s"} need${sum.review === 1 ? "s" : ""} review below` : ""}. Other Docker storage may still contain reclaimable space.</div>}
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
      {outcome && <CleanupResult outcome={outcome} label="Old releases cleanup" />}
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
  const [blocked, setBlocked] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<CleanupOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [empty, setEmpty] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const otherCleanup = useCleanupActive() !== null;
  const inFlight = useRef(false);
  const call = (apply: boolean) => invoke<OpResult>("deploy_op", apply ? { op: { op: "prune-build-cache", apply: true }, confirmation: { liveChanges: true, cutover: false } } : { op: { op: "prune-build-cache", apply: false } });
  const check = async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy("check"); setError(null); setOutcome(null); setEmpty(false); setBlocked(null); setSize(null); setNote(null);
    try {
      const r = await call(false);
      if (!r.ok) setError(r.hint || r.summary);
      else {
        const c = parseCache(r.detail);
        // a missing figure is "unknown", never "nothing to prune"
        if (c.reclaimable === null) setError("The engine gave no build-cache figure, so nothing can be offered.");
        else { setSize(c.reclaimable); setBlocked(c.blocked); setEmpty(c.reclaimable === 0); setNote(cacheExplanation(c, fmtBytes)); }
      }
    } catch (e) { setError(String(e)); } finally { inFlight.current = false; setBusy(null); }
  };
  const prune = async () => {
    if (inFlight.current || size === null || size <= 0 || blocked) return;
    const release = acquireCleanup("build-cache");
    if (!release) return;
    inFlight.current = true;
    try {
      if (!(await confirmAction(`Harpoon will ask Docker to delete BuildKit cache that Docker reports as unused${size !== null ? ` (about ${fmtBytes(size)} by its last count)` : ""}.\n\nUnused is not the same as useless: this cache can be reusable, so removing it can make later builds slower while it fills again. Docker decides what is unused when it prunes, and that can differ from this figure. The amount is Docker's accounting, not a promise of free disk space, and the space stays allocated on your Mac until you reclaim unused space.\n\nImages, containers and volumes are not touched. Pruning does not protect against builds started by other Docker clients.`, "Prune build cache"))) return;
      setBusy("prune"); setError(null); setOutcome(null);
      const before = await hostAllocationNow();
      let result: CleanupOutcome;
      try {
        const r = await call(true);
        const after = await hostAllocationNow();
        result = r.ok ? cacheOutcome(parseCache(r.detail), { before, after }, fmtBytes) : failureOutcome("build-cache", r, null, { before, after });
      } catch (e) { result = failureOutcome("build-cache", null, e, { before, after: await hostAllocationNow() }); }
      setOutcome(result); setSize(null); setBlocked(null); setNote(null);
    } finally { inFlight.current = false; release(); setBusy(null); onDone(); }
  };
  return (
    <div id={CLEANUP_TARGETS["build-cache"].section} tabIndex={-1} style={{ marginTop: 10 }}>
      <div className="text-meta">Build cache speeds up rebuilds. Deleting it frees space but makes the next builds slower.</div>
      <div className="dialog-actions">
        <button id={CLEANUP_TARGETS["build-cache"].button} className="btn btn--secondary" disabled={!enabled || busy !== null} onClick={check}>{busy === "check" ? "Checking…" : "Check build cache"}</button>
        {size !== null && size > 0 && !blocked && <button className="btn btn--secondary" disabled={busy !== null || otherCleanup} onClick={prune}>{busy === "prune" ? "Pruning…" : `Prune ${fmtBytes(size)}…`}</button>}
      </div>
      {empty && <div role="status" className="text-meta">No unused build cache.</div>}
      {note && <div role="status" className="text-meta">{note}</div>}
      {blocked && <div role="status" className="text-body">Pruning is unavailable right now: {blocked}. Nothing was changed.</div>}
      {outcome && <CleanupResult outcome={outcome} label="Build cache prune" />}
      {error && <div role="alert" style={{ color: "var(--error-fg)", whiteSpace: "pre-wrap" }}>{error}</div>}
    </div>
  );
}

export function DiskReclaimCard({ enabled, expansion }: { enabled: boolean; expansion: ExpansionState | null | undefined }) {
  const view = useDiskView();
  const cap = view.status === "live" ? view.cap : null; // reclaiming needs the running guest, not a remembered reading
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<CleanupOutcome | null>(null);
  const inFlight = useRef(false);
  const otherCleanup = useCleanupActive() !== null;
  const load = useCallback(() => diskTelemetry.refresh(), []);
  useEffect(() => { if (enabled) void load(); }, [enabled, load, expansion?.phase]);

  const blocked = !cap && enabled && view.cap && expansion?.active !== true
    ? `Disk usage is not live (${view.note.replace(/\.$/, "")}), so reclaiming is unavailable until it refreshes.`
    : reclaimBlockedReason(enabled ? cap : null, expansion);

  const reclaim = async () => {
    if (!cap || blocked || busy || inFlight.current) return;
    const release = acquireCleanup("disk-blocks");
    if (!release) return;
    inFlight.current = true;
    try {
      if (!(await confirmAction(reclaimConfirmationMessage(cap), "Reclaim unused space"))) return;
      setBusy(true); setResult(null);
      try { setResult(reclaimOutcomeFromText(await invoke<string>("reclaim_disk_confirmed"))); }
      catch (e) { setResult(reclaimOutcomeFromText(String(e))); }
    } finally { inFlight.current = false; release(); setBusy(false); void load(); }
  };

  return (
    <div className="panel" style={{ padding: 14 }}>
      <DockerArtifacts enabled={enabled} onDone={() => void load()} />
      <div id={CLEANUP_TARGETS["disk-blocks"].section} tabIndex={-1} className="text-panel-title">Disk image on this Mac</div>
      <div className="text-body" style={{ lineHeight: 1.7, marginTop: 6 }}>
        {cap && cap.provisioned
          ? <>Capacity {formatSize(cap.logicalBytes)} • {formatSize(cap.physicalBytes)} currently allocated on this Mac</>
          : <>Current allocation appears once disk status is live.</>}
      </div>
      <div className="text-meta" style={{ margin: "6px 0" }}>
        Reclaiming asks the guest to hand blocks it no longer uses back to macOS. It does not delete Docker images, containers or volumes, and the amount returned is only known afterwards.
      </div>
      <div className="dialog-actions">
        <button id={CLEANUP_TARGETS["disk-blocks"].button} className="btn btn--secondary" disabled={Boolean(blocked) || busy || otherCleanup} title={blocked ?? (otherCleanup ? "Another cleanup is running." : undefined)} onClick={reclaim}>
          {busy ? "Reclaiming…" : "Reclaim unused space…"}
        </button>
      </div>
      {blocked && !busy && <div className="text-meta">{blocked}</div>}
      {busy && <div role="status" className="text-meta">Trimming the guest disk. Harpoon keeps running.</div>}
      {result && !busy && <CleanupResult outcome={result} label="Reclaim unused space" />}
    </div>
  );
}
