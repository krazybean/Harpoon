// Read-only storage inventory: what Docker reports it stores (sizes with their meaning) and what Harpoon can prove about who owns it.
// The runtime command returns JSON text; `parseStorageInventory` validates it strictly (a wrong shape is rejected, never repaired),
// and the store keeps the last good inventory when a refresh fails. It is fetched on demand only: no polling, one request at a time.

export type Bytes = number | null; // null: Docker did not report it. Never zero.
export interface Ref { name: string; running: boolean }
export interface Ownership { property: string; evidence: string }
export interface InvItem {
  id: string; name: string; kind: string; bytes: Bytes; bytesNote: string; state: string;
  references: Ref[] | null; ownership: Ownership[]; protection: "active" | "in-use" | "persistent" | "none"; protectionNote: string;
  details: { label: string; value: string }[];
}
export interface InvCategory {
  id: string; title: string; sizeBasis: string; status: "ok" | "partial" | "unavailable"; note: string | null;
  count: number; bytes: Bytes; bytesComplete: boolean; skipped: number; omitted: number;
  figures: { label: string; bytes: Bytes; meaning: string }[]; counts: { label: string; count: number }[]; items: InvItem[];
}
export interface InvSource { endpoint: string; status: "ok" | "failed" | "oversized" | "invalid"; detail: string }
export interface Inventory {
  generatedMs: number; sources: InvSource[]; journalAvailable: boolean; imageLayersBytes: Bytes; imageLayersMeaning: string;
  unknownOwnershipItems: number; categories: InvCategory[];
}

export const CATEGORY_ORDER = ["deployment-images", "other-images", "build-cache", "volumes", "writable-layers", "obsolete-artifacts"] as const;

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const count = (v: unknown): number | null => (typeof v === "number" && Number.isInteger(v) && v >= 0 ? v : null);
/** `undefined` = malformed; `null` = Docker did not report it. A negative or non-finite number is malformed, not zero. */
const bytes = (v: unknown): Bytes | undefined => (v === null ? null : typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : undefined);
const oneOf = <T extends string>(v: unknown, set: readonly T[]): T | null => (typeof v === "string" && (set as readonly string[]).includes(v) ? (v as T) : null);

function item(o: unknown): InvItem | null {
  if (!isObj(o)) return null;
  const b = bytes(o.bytes), prot = oneOf(o.protection, ["active", "in-use", "persistent", "none"] as const);
  if (b === undefined || !prot || str(o.id) === null || str(o.name) === null || str(o.state) === null || !Array.isArray(o.ownership) || !Array.isArray(o.details)) return null;
  const refs = o.references === null ? null : Array.isArray(o.references) ? o.references.map((r) => (isObj(r) && str(r.name) !== null && typeof r.running === "boolean" ? { name: r.name as string, running: r.running } : null)) : undefined;
  if (refs === undefined || (refs && refs.includes(null))) return null;
  const own = o.ownership.map((x) => (isObj(x) && str(x.property) && str(x.evidence) !== null ? { property: x.property as string, evidence: x.evidence as string } : null));
  const det = o.details.map((x) => (isObj(x) && str(x.label) !== null && str(x.value) !== null ? { label: x.label as string, value: x.value as string } : null));
  if (own.includes(null) || det.includes(null)) return null;
  return { id: o.id as string, name: o.name as string, kind: str(o.kind) ?? "", bytes: b, bytesNote: str(o.bytesNote) ?? "", state: o.state as string, references: refs as Ref[] | null, ownership: own as Ownership[], protection: prot, protectionNote: str(o.protectionNote) ?? "", details: det as InvItem["details"] };
}

function category(o: unknown): InvCategory | null {
  if (!isObj(o)) return null;
  const status = oneOf(o.status, ["ok", "partial", "unavailable"] as const), b = bytes(o.bytes);
  if (!status || b === undefined || str(o.id) === null || str(o.title) === null || count(o.count) === null || count(o.skipped) === null || count(o.omitted) === null || typeof o.bytesComplete !== "boolean") return null;
  if (!Array.isArray(o.items) || !Array.isArray(o.figures) || !Array.isArray(o.counts)) return null;
  const items = o.items.map(item);
  const figures = o.figures.map((f) => (isObj(f) && str(f.label) && bytes(f.bytes) !== undefined ? { label: f.label as string, bytes: f.bytes as Bytes, meaning: str(f.meaning) ?? "" } : null));
  const counts = o.counts.map((c) => (isObj(c) && str(c.label) && count(c.count) !== null ? { label: c.label as string, count: c.count as number } : null));
  if (items.includes(null) || figures.includes(null) || counts.includes(null)) return null;
  return { id: o.id as string, title: o.title as string, sizeBasis: str(o.sizeBasis) ?? "", status, note: str(o.note), count: o.count as number, bytes: b, bytesComplete: o.bytesComplete, skipped: o.skipped as number, omitted: o.omitted as number, figures: figures as InvCategory["figures"], counts: counts as InvCategory["counts"], items: items as InvItem[] };
}

/** `null` when the text is not an inventory this app understands. */
export function parseStorageInventory(text: string): Inventory | null {
  let j: unknown;
  try { j = JSON.parse(text); } catch { return null; }
  if (!isObj(j) || j.schema !== 1 || !Array.isArray(j.sources) || !Array.isArray(j.categories) || typeof j.journalAvailable !== "boolean") return null;
  const gen = j.generatedMs, layers = bytes(j.imageLayersBytes), unknown = count(j.unknownOwnershipItems);
  if (typeof gen !== "number" || layers === undefined || unknown === null) return null;
  const sources = j.sources.map((s) => (isObj(s) && str(s.endpoint) && oneOf(s.status, ["ok", "failed", "oversized", "invalid"] as const) ? { endpoint: s.endpoint as string, status: s.status as InvSource["status"], detail: str(s.detail) ?? "" } : null));
  const cats = j.categories.map(category);
  if (sources.includes(null) || cats.includes(null)) return null;
  const all = cats as InvCategory[];
  if (!CATEGORY_ORDER.every((id) => all.some((c) => c.id === id))) return null;
  return { generatedMs: gen, sources: sources as InvSource[], journalAvailable: j.journalAvailable, imageLayersBytes: layers, imageLayersMeaning: str(j.imageLayersMeaning) ?? "", unknownOwnershipItems: unknown, categories: all };
}

export const FETCH_TIMEOUT_MS = 20_000;

export interface InventoryState {
  enabled: boolean; loading: boolean;
  last: { inv: Inventory; at: number } | null;
  failure: { kind: "failed" | "unsupported"; message: string; at: number } | null;
}
export type InventoryStatus = "engine-down" | "loading" | "current" | "stale" | "unsupported" | "unavailable";
export interface InventoryView { status: InventoryStatus; inv: Inventory | null; at: number | null; loading: boolean; note: string; now: number }

export const initialInventory = (): InventoryState => ({ enabled: false, loading: false, last: null, failure: null });

/** What the page shows. The last good inventory is kept (and labelled stale) when a refresh fails; "unsupported" never shows the old one. */
export function inventoryView(t: InventoryState, now: number): InventoryView {
  const base = { inv: t.last?.inv ?? null, at: t.last?.at ?? null, loading: t.loading, now };
  if (!t.enabled) return { ...base, status: "engine-down", note: "Harpoon is not running, so Docker storage cannot be read." };
  if (t.failure?.kind === "unsupported") return { ...base, inv: null, at: null, status: "unsupported", note: "This runtime reports storage in a form this app does not understand." };
  if (t.failure) return t.last
    ? { ...base, status: "stale", note: `The last refresh failed: ${t.failure.message || "no reason given"}. These figures are from before.` }
    : { ...base, status: "unavailable", note: `Storage could not be read: ${t.failure.message || "no reason given"}.` };
  if (!t.last) return { ...base, status: "loading", note: "Reading Docker storage…" };
  return { ...base, status: "current", note: "" };
}

export interface InventoryStore { get(): InventoryState; subscribe(fn: () => void): () => void; refresh(): Promise<void>; setEnabled(e: boolean): void }

/**
 * One request at a time, whoever asks. A refresh that outlives FETCH_TIMEOUT_MS is abandoned as failed and its late answer is ignored
 * (only the race winner is applied), so it can never overwrite a newer inventory. The runtime itself allows one inventory at once and
 * refuses a second while the first still runs, so repeated timeouts cannot pile up native work.
 */
export function createInventoryStore(fetchText: () => Promise<string>, now: () => number = () => Date.now()): InventoryStore {
  let state = initialInventory();
  const subs = new Set<() => void>();
  let inflight: Promise<void> | null = null;
  // The runtime call cannot be cancelled from here: after a timeout it may still run, and no second one starts until it ends.
  let native: Promise<unknown> | null = null;
  const set = (next: InventoryState) => { state = next; subs.forEach((f) => f()); };
  return {
    get: () => state,
    subscribe(fn) { subs.add(fn); return () => { subs.delete(fn); }; },
    setEnabled(enabled) { if (enabled !== state.enabled) set({ ...state, enabled }); },
    refresh() {
      if (inflight) return inflight;
      if (!state.enabled) return Promise.resolve();
      set({ ...state, loading: true });
      inflight = (async () => {
        let timer: ReturnType<typeof setTimeout> | undefined;
        try {
          if (native) throw new Error("the previous storage inventory request is still running");
          const call = Promise.resolve(fetchText());
          native = call;
          const release = () => { if (native === call) native = null; };
          call.then(release, release);
          const timeout = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("storage inventory timed out")), FETCH_TIMEOUT_MS); });
          const inv = parseStorageInventory(await Promise.race([call, timeout]));
          set(inv
            ? { ...state, loading: false, last: { inv, at: now() }, failure: null }
            : { ...state, loading: false, failure: { kind: "unsupported", message: "unrecognized storage inventory", at: now() } });
        } catch (e) {
          set({ ...state, loading: false, failure: { kind: "failed", message: String(e).replace(/^Error:\s*/, "").trim(), at: now() } });
        } finally { clearTimeout(timer); }
      })().finally(() => { inflight = null; });
      return inflight;
    },
  };
}
