import { useId, useState } from "react";
import { ageLabel } from "../lib/diskTelemetry";
import { CATEGORY_ORDER } from "../lib/storageInventory";
import type { Bytes, InvCategory, InvItem, InventoryView } from "../lib/storageInventory";
import type { CleanupTarget } from "../lib/cleanupTargets";

// Read-only. Every row is an independent Docker-reported measurement: nothing here is a share of the disk, nothing is summed across
// rows, and no row offers to remove anything. Meaning is written in words next to each figure, not carried by colour.

export function fmtBytes(b: number): string {
  const u = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = b, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${i === 0 || v >= 100 ? Math.round(v) : v.toFixed(1)} ${u[i]}`;
}
const size = (b: Bytes) => (b === null ? "size not reported" : fmtBytes(b));

const PROPERTY: Record<string, string> = {
  "active-deployment": "Confirmed by the journal: active deployment", "historical-deployment": "Confirmed by the journal: historical deployment",
  "referenced-by-running-container": "Reference: used by a running container", "referenced-by-stopped-container": "Reference: used by a stopped container",
  "persistent-data": "Persistent data", "unknown-ownership": "Unknown ownership", "harpoon-labelled-unconfirmed": "Label claim only, not confirmed",
  "label-journal-mismatch": "Labels disagree with the journal", "journal-released": "Journal: released",
};
const STATE: Record<string, string> = {
  "referenced-by-running": "In use by a running container", "referenced-by-stopped": "Only stopped containers use it", dangling: "Dangling (no tag, no container)",
  "tagged-unreferenced": "Tagged, no container uses it", "references-unknown": "Container use unknown", running: "Running", exited: "Stopped", "in-use": "In use",
  "not-in-use": "Docker says not in use", "build-in-use": "Docker says a build uses it", "use-unknown": "Use not reported", "not-running": "No running container",
};
const PROTECTION: Record<InvItem["protection"], { label: string; cls: string }> = {
  active: { label: "Protected: serving now", cls: "badge--running" }, "in-use": { label: "In use", cls: "badge--info" },
  persistent: { label: "Protected: persistent data", cls: "badge--running" }, none: { label: "Inspect only", cls: "badge--stopped" },
};

function ItemRow({ item }: { item: InvItem }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const prot = PROTECTION[item.protection];
  return (
    <li className="storage-item">
      <button type="button" className="storage-item__head" aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}>
        <span className="storage-item__name" title={item.name}>{item.name}</span>
        <span className="storage-item__size">{size(item.bytes)}</span>
        <span className="storage-chips">
          <span className={`badge badge--sm ${prot.cls}`}>{prot.label}</span>
          <span className="badge badge--sm badge--stopped">{STATE[item.state] ?? item.state}</span>
          {item.ownership.some((o) => o.property === "unknown-ownership") && <span className="badge badge--sm badge--warning">Unknown ownership</span>}
        </span>
      </button>
      {open && (
        <div id={id} className="storage-item__body" role="group" aria-label={`Details for ${item.name}`}>
          <div className="text-meta">{item.protectionNote} {item.bytesNote}</div>
          <div className="text-meta" style={{ marginTop: 6 }}>Ownership evidence</div>
          <ul className="storage-evidence text-body">
            {item.ownership.map((o) => <li key={o.property}><b>{PROPERTY[o.property] ?? o.property}.</b> {o.evidence}</li>)}
          </ul>
          {item.references !== null && (
            <div className="text-body" style={{ marginTop: 6 }}>
              <span className="text-meta">Containers</span>{" "}
              {item.references.length === 0 ? "none" : item.references.map((r) => `${r.name} (${r.running ? "running" : "stopped"})`).join(", ")}
            </div>
          )}
          <dl className="storage-details">
            {item.details.map((d) => <div key={d.label}><dt className="text-meta">{d.label}</dt><dd className="text-code">{d.value}</dd></div>)}
          </dl>
        </div>
      )}
    </li>
  );
}

/** What each row may lead to. A target only moves focus to the existing control; the rest are inspection only, and say so. This list is
 *  information, not permission: the engine re-checks everything when the person reviews, and again when they confirm. */
const REVIEW_NOTE = "This list is information, not permission. Reviewing runs the engine's own checks now, and nothing is removed until you confirm there.";
const ACTIONS: Record<string, { target?: CleanupTarget; label?: string; note: string }> = {
  "deployment-images": { target: "old-releases", label: "Go to Review old releases", note: `Only releases the journal records as no longer serving can be reviewed. The serving release and the image it pins are always kept. ${REVIEW_NOTE}` },
  "obsolete-artifacts": { target: "old-releases", label: "Go to Review old releases", note: `${REVIEW_NOTE} Objects listed here that the engine cannot prove are safe to remove are left alone.` },
  "build-cache": { target: "build-cache", label: "Go to Check build cache", note: "Checking asks Docker how much cache nothing is using and removes nothing. Pruning is a separate step with its own confirmation. Docker's figure is not a promise of free disk space." },
  "other-images": { note: "Inspection only. Images Harpoon did not deploy, and images of unknown ownership, are not removed from here." },
  volumes: { note: "Inspection only. Harpoon never removes persistent volumes." },
  "writable-layers": { note: "Inspection only. Containers are not removed from here, and running or stopped containers keep their images protected." },
};

function figureText(c: InvCategory): string {
  if (c.status === "unavailable") return "Not available";
  if (c.bytes === null) return c.id === "obsolete-artifacts" || c.id.endsWith("images") ? "Sizes overlap, not totalled" : "No size reported";
  const unmeasured = c.items.filter((i) => i.bytes === null).length;
  return c.bytesComplete ? fmtBytes(c.bytes) : `at least ${fmtBytes(c.bytes)} (${unmeasured || "some"} not measured)`;
}

function Row({ c, onAction }: { c: InvCategory; onAction?: (t: CleanupTarget) => void }) {
  const action = ACTIONS[c.id];
  const [open, setOpen] = useState(false);
  const id = useId();
  return (
    <li className="storage-row">
      <button type="button" className="storage-row__head" aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}>
        <span className="storage-row__title">{c.title}</span>
        <span className="storage-row__figure">{figureText(c)}</span>
        <span className="text-meta storage-row__count">{c.status === "unavailable" ? "" : `${c.count} ${c.count === 1 ? "item" : "items"}`}</span>
        {c.status !== "ok" && <span className={`badge badge--sm ${c.status === "partial" ? "badge--warning" : "badge--stopped"}`}>{c.status === "partial" ? "Partial" : "Unavailable"}</span>}
      </button>
      {open && (
        <div id={id} className="storage-row__body" role="group" aria-label={`${c.title} details`}>
          <p className="text-meta">{c.sizeBasis}</p>
          {action && (
            <div className="storage-action-box">
              <p className="text-meta" style={{ margin: 0 }}>{action.note}</p>
              {action.target && onAction && <button type="button" className="btn btn--secondary storage-action" onClick={() => onAction(action.target!)}>{action.label}</button>}
            </div>
          )}
          {c.note && <p className="text-body" role="status">{c.note}</p>}
          {c.figures.map((f) => (
            <p key={f.label} className="text-body"><b>{f.label}:</b> {f.bytes === null ? "unknown (a record has no size)" : fmtBytes(f.bytes)} <span className="text-meta">{f.meaning}</span></p>
          ))}
          {c.counts.length > 0 && <p className="text-meta">{c.counts.map((x) => `${x.count} ${x.label}`).join(" · ")}</p>}
          {c.items.length === 0 && c.status !== "unavailable" && <p className="text-body">Nothing here.</p>}
          {c.items.length > 0 && <ul className="storage-items">{c.items.map((i) => <ItemRow key={`${i.kind}:${i.id}`} item={i} />)}</ul>}
          {c.omitted > 0 && <p className="text-meta">{c.omitted} more not listed (the largest {c.items.length} are shown).</p>}
        </div>
      )}
    </li>
  );
}

function Unattributed() {
  const [open, setOpen] = useState(false);
  const id = useId();
  return (
    <li className="storage-row">
      <button type="button" className="storage-row__head" aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}>
        <span className="storage-row__title">Other / unattributed usage</span>
        <span className="storage-row__figure">Not reliably quantifiable</span>
      </button>
      {open && (
        <div id={id} className="storage-row__body" role="group" aria-label="Other and unattributed usage details">
          <p className="text-body">Harpoon does not subtract the rows above from the guest's used space. Docker's image, cache and layer figures overlap, and they measure different things from the filesystem, so the difference would be a made-up number.</p>
          <p className="text-meta">The Docker disk meter above is the authoritative measure of how full the guest is.</p>
        </div>
      )}
    </li>
  );
}

export function StorageBreakdown({ view, onRefresh, onAction }: { view: InventoryView; onRefresh: () => void; onAction?: (t: CleanupTarget) => void }) {
  const { inv, status, loading, now } = view;
  const cats = inv ? CATEGORY_ORDER.map((id) => inv.categories.find((c) => c.id === id)!) : [];
  const failedSources = inv ? inv.sources.filter((s) => s.status !== "ok") : [];
  return (
    <div className="panel" style={{ padding: 14 }} data-storage-status={status}>
      <div style={{ display: "flex", justifyContent: "space-between", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
        <span className="text-panel-title">Storage breakdown</span>
        <span style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          {view.at !== null && <span className="text-meta" style={{ textTransform: "none", letterSpacing: 0 }}>Read {ageLabel(view.at, now)}</span>}
          {status === "stale" && <span className="badge badge--sm badge--warning">Stale</span>}
          <button type="button" className="btn btn--secondary storage-refresh" onClick={onRefresh} disabled={loading || status === "engine-down"} aria-busy={loading}>{loading ? "Reading…" : "Refresh"}</button>
        </span>
      </div>
      <p className="text-meta" style={{ margin: "6px 0 10px" }}>
        What Docker reports it stores. Rows are separate measurements, not shares of the disk: they overlap and do not add up to the usage above. Inspection only.
      </p>
      {status !== "current" && status !== "stale" && <div className="text-body" role="status">{view.note}</div>}
      {status === "stale" && <div className="text-body" role="status" style={{ marginBottom: 8 }}>{view.note}</div>}
      {inv && (
        <>
          {failedSources.map((s) => <div key={s.endpoint} className="text-body" role="status">{s.endpoint}: {s.status === "failed" ? `failed (${s.detail})` : s.detail}</div>)}
          {!inv.journalAvailable && <div className="text-body" role="status">The deployment journal could not be used{inv.journalNote ? ` (${inv.journalNote})` : ""}, so no ownership is confirmed. Harpoon labels are claims only.</div>}
          <ul className="storage-rows">
            {cats.map((c) => <Row key={c.id} c={c} onAction={onAction} />)}
            <Unattributed />
          </ul>
          {onAction && (
            <div className="storage-action-box" style={{ marginTop: 8 }}>
              <p className="text-meta" style={{ margin: 0 }}>Space freed inside Harpoon stays allocated on your Mac until unused disk blocks are returned. That is a separate step and does not delete Docker objects.</p>
              <button type="button" className="btn btn--secondary storage-action" onClick={() => onAction("disk-blocks")}>Go to Reclaim unused space</button>
            </div>
          )}
          <p className="text-meta" style={{ marginTop: 8 }}>
            {inv.imageLayersBytes !== null ? `All images: ${fmtBytes(inv.imageLayersBytes)} of layers, shared layers counted once. ` : ""}{inv.imageLayersMeaning}
            {inv.unknownOwnershipItems > 0 && ` ${inv.unknownOwnershipItems} listed objects have unknown ownership. Unknown does not mean unused or safe to remove.`}
          </p>
        </>
      )}
    </div>
  );
}
