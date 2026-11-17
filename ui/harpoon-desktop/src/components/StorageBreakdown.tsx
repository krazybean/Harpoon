import { useId, useState } from "react";
import { ageLabel } from "../lib/diskTelemetry";
import { CATEGORY_ORDER } from "../lib/storageInventory";
import type { Bytes, InvCategory, InvItem, InventoryView } from "../lib/storageInventory";

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

function figureText(c: InvCategory): string {
  if (c.status === "unavailable") return "Not available";
  if (c.bytes === null) return c.id === "obsolete-artifacts" || c.id.endsWith("images") ? "Sizes overlap, not totalled" : "No size reported";
  const unmeasured = c.items.filter((i) => i.bytes === null).length;
  return c.bytesComplete ? fmtBytes(c.bytes) : `at least ${fmtBytes(c.bytes)} (${unmeasured || "some"} not measured)`;
}

function Row({ c }: { c: InvCategory }) {
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

export function StorageBreakdown({ view, onRefresh }: { view: InventoryView; onRefresh: () => void }) {
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
          {!inv.journalAvailable && <div className="text-body" role="status">The deployment journal could not be read, so no ownership is confirmed.</div>}
          <ul className="storage-rows">
            {cats.map((c) => <Row key={c.id} c={c} />)}
            <Unattributed />
          </ul>
          <p className="text-meta" style={{ marginTop: 8 }}>
            {inv.imageLayersBytes !== null ? `All images: ${fmtBytes(inv.imageLayersBytes)} of layers, shared layers counted once. ` : ""}{inv.imageLayersMeaning}
            {inv.unknownOwnershipItems > 0 && ` ${inv.unknownOwnershipItems} listed objects have unknown ownership. Unknown does not mean unused or safe to remove.`}
          </p>
        </>
      )}
    </div>
  );
}
