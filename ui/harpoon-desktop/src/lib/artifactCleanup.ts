// The engine's `cleanup-releases` report, read for the Reclaim Unused Space card. The engine states every removal
// before and after it happens; this only turns that text into numbers and lines the card can show.

// What the engine decided for each artifact, as `item: {json}` lines. The engine resolves ownership from the journal
// (registered repository, else the removal record that disconnected it, else nothing), so nothing here guesses it.

export type Verdict = "ready" | "reconcile" | "protected" | "review";
export type Kind = "container" | "image" | "port-lease";

export type CleanupItem = {
  app: string; appName: string | null; environment: string | null;
  deployment: string; createdMs: number | null; status: string; commit: string | null;
  kind: Kind; name: string; verdict: Verdict; reason: string; detail: string;
  /** Whether Docker holds the object now. `null` for a port reservation, which is only a journal entry. */
  physical: boolean | null;
};

export type CleanupReport = {
  items: CleanupItem[];
  containers: number;
  images: number;
  /** Guest filesystem free space around an applied cleanup, when the management channel could measure it. */
  guestFreeBefore: number | null;
  guestFreeAfter: number | null;
  /** The engine's "reclaimed-containers" and "reclaimed-images" lines were both present and whole numbers. Without them nothing is known about what was removed: a missing count is not zero. */
  reported: boolean;
  /** The engine's "kept:" lines: objects it looked at and left in place, with its reason. */
  kept: string[];
};

const VERDICTS: Verdict[] = ["ready", "reconcile", "protected", "review"];
const KINDS: Kind[] = ["container", "image", "port-lease"];

function itemOf(raw: unknown): CleanupItem | null {
  if (typeof raw !== "object" || raw === null) return null;
  const o = raw as Record<string, unknown>;
  const str = (k: string) => (typeof o[k] === "string" ? (o[k] as string) : null);
  const verdict = str("verdict") as Verdict | null;
  const kind = str("kind") as Kind | null;
  if (!verdict || !kind || !VERDICTS.includes(verdict) || !KINDS.includes(kind) || !str("deployment") || !str("app")) return null;
  return {
    app: str("app")!, appName: str("appName"), environment: str("environment"),
    deployment: str("deployment")!, createdMs: typeof o.createdMs === "number" ? o.createdMs : null, status: str("status") ?? "unknown", commit: str("commit"),
    kind, name: str("name") ?? "", verdict, reason: str("reason") ?? "", detail: str("detail") ?? "",
    physical: typeof o.physical === "boolean" ? o.physical : null,
  };
}

export function parseCleanup(detail: string): CleanupReport {
  const lines = detail.split("\n").map((l) => l.trim()).filter(Boolean);
  const whole = (key: string): number | null => { const l = lines.find((x) => x.startsWith(`${key}:`)); const n = l && /^\d+$/.test(l.slice(key.length + 1).trim()) ? Number(l.slice(key.length + 1)) : NaN; return Number.isSafeInteger(n) ? n : null; };
  const num = (key: string) => whole(key) ?? 0;
  const bytes = whole;
  const items: CleanupItem[] = [];
  for (const l of lines) {
    if (!l.startsWith("item: ")) continue;
    try { const it = itemOf(JSON.parse(l.slice(6))); if (it) items.push(it); } catch { /* a malformed line is skipped, never guessed at */ }
  }
  return { items, containers: num("reclaimed-containers"), images: num("reclaimed-images"), reported: whole("reclaimed-containers") !== null && whole("reclaimed-images") !== null, kept: lines.filter((l) => l.startsWith("kept: ")).map((l) => l.slice(6)), guestFreeBefore: bytes("guest-free-before-bytes"), guestFreeAfter: bytes("guest-free-after-bytes") };
}

/** What a person can act on: Docker objects to remove, and journal records that only need reconciling. */
export const removable = (r: CleanupReport): CleanupItem[] => r.items.filter((i) => i.verdict === "ready");
export const reconcilable = (r: CleanupReport): CleanupItem[] => r.items.filter((i) => i.verdict === "reconcile");
export const nothingToClean = (r: CleanupReport): boolean => removable(r).length === 0 && reconcilable(r).length === 0;

export const UNASSOCIATED = "Unassociated artifacts";
export const shortDeployment = (id: string): string => `dep…${id.slice(-6)}`;

export type CleanupGroup = {
  key: string;
  title: string;
  environment: string | null;
  unassociated: boolean;
  items: CleanupItem[];
  ready: { containers: number; images: number };
  reconcile: number;
  protectedCount: number;
  review: number;
};

/** Groups by the application the journal says owns each artifact; an artifact with no known owner goes to one "Unassociated" group. */
export function groupByApplication(r: CleanupReport): CleanupGroup[] {
  const groups = new Map<string, CleanupGroup>();
  for (const it of r.items) {
    const unassociated = it.appName === null;
    const key = unassociated ? "unassociated" : it.app;
    let g = groups.get(key);
    if (!g) {
      g = { key, title: unassociated ? UNASSOCIATED : it.appName!, environment: unassociated ? null : it.environment, unassociated, items: [], ready: { containers: 0, images: 0 }, reconcile: 0, protectedCount: 0, review: 0 };
      groups.set(key, g);
    }
    g.items.push(it);
    if (it.verdict === "ready") { if (it.kind === "container") g.ready.containers++; else if (it.kind === "image") g.ready.images++; }
    else if (it.verdict === "reconcile") g.reconcile++;
    else if (it.verdict === "protected") g.protectedCount++;
    else g.review++;
  }
  // applications with something to act on first, the unassociated group last
  return [...groups.values()].sort((a, b) => Number(a.unassociated) - Number(b.unassociated) || (b.ready.containers + b.ready.images + b.reconcile) - (a.ready.containers + a.ready.images + a.reconcile) || a.title.localeCompare(b.title));
}

export const STATUS_LABEL: Record<Verdict, string> = { ready: "Ready to remove", reconcile: "Journal reconciliation", protected: "Protected", review: "Needs review" };

/** One plain sentence for why an artifact is (or is not) eligible, from the engine's own reason code. */
export function reasonLabel(i: CleanupItem): string {
  const what = i.kind === "container" ? "container" : i.kind === "image" ? "image" : "port reservation";
  switch (i.reason) {
    case "superseded": return i.kind === "container" ? "Stopped container from an older deployment" : "Unused image from a previous release";
    case "failed-attempt": return i.kind === "container" ? "Container left by a failed deployment" : "Image kept from a failed deployment";
    case "already-absent": return i.kind === "container" ? "Container already removed; journal needs reconciliation" : "Image already removed; journal needs reconciliation";
    case "stale-port-lease": return "Stale port reservation";
    case "serving-release": return `Serving now: this ${what} belongs to the running release`;
    case "serving-pins-image": return "Image the running release is built from";
    case "shared-with-protected": return "Image retained because a protected deployment still references it";
    case "rollback-target": return "Rollback target: the most recent previous release proven restorable, kept so the application can be rolled back";
    case "rollback-pins-image": return "Image a rollback of this application would start from";
    case "rollback-unverified": return "Kept: whether this release can be rolled back to could not be verified";
    case "nothing-proven-serving": return `No release of this application is recorded as serving, so its ${what} is left alone`;
    default: return i.detail ? `Needs review: ${i.detail}` : "Needs review";
  }
}

/** The "what will happen" counts for the confirmation. Docker's image sizes share layers, so no byte estimate is ever given. */
export function operationSummary(r: CleanupReport) {
  const ready = removable(r);
  const containers = ready.filter((i) => i.kind === "container").length;
  const images = ready.filter((i) => i.kind === "image").length;
  const journalOnly = reconcilable(r).length;
  const apps = [...new Set(r.items.filter((i) => i.verdict === "ready" || i.verdict === "reconcile").map((i) => i.appName ?? UNASSOCIATED))];
  return { containers, images, journalOnly, protectedCount: r.items.filter((i) => i.verdict === "protected").length, review: r.items.filter((i) => i.verdict === "review").length, apps };
}

/** The inventory headline: every item is counted once, as eligible (the engine would remove it or reconcile its journal record), protected, or needing review. */
export function inventorySummary(r: CleanupReport) {
  const o = operationSummary(r);
  const dockerObjects = o.containers + o.images;
  return { total: r.items.length, eligible: dockerObjects + o.journalOnly, protectedCount: o.protectedCount, review: o.review, dockerObjects, journalOnly: o.journalOnly };
}

/** The primary action says what it will do; null when nothing is eligible. */
export function cleanupActionLabel(r: CleanupReport): string | null {
  const { dockerObjects: d, journalOnly: j } = inventorySummary(r);
  if (d + j === 0) return null;
  if (d === 0) return `Reconcile ${j} record${j === 1 ? "" : "s"}`;
  if (j === 0) return `Remove ${d} Docker artifact${d === 1 ? "" : "s"}`;
  return `Review cleanup (${d + j} items)`;
}

/** The confirm button's label: it names what changes, and keeps Docker deletion and journal reconciliation apart. */
export function confirmLabel(r: CleanupReport): string {
  const { dockerObjects: d, journalOnly: j } = inventorySummary(r);
  const records = `${j} record${j === 1 ? "" : "s"}`;
  if (d === 0) return `Reconcile ${records}`;
  const objects = `${d} Docker ${j === 0 ? "artifact" : "object"}${d === 1 ? "" : "s"}`;
  return j === 0 ? `Remove ${objects}` : `Remove ${objects} and reconcile ${records}`;
}

/** What the operation changes, by category, straight from the engine's verdicts. Protected-only applications are not "affected". */
export function impactBreakdown(r: CleanupReport) {
  const n = (v: Verdict, k: Kind) => r.items.filter((i) => i.verdict === v && i.kind === k).length;
  return {
    stalePortLeases: n("reconcile", "port-lease"),
    goneContainers: n("reconcile", "container"),
    goneImages: n("reconcile", "image"),
    removeContainers: n("ready", "container"),
    removeImages: n("ready", "image"),
    records: r.items.filter((i) => i.verdict === "ready" || i.verdict === "reconcile"),
    applications: groupByApplication(r).filter((g) => g.ready.containers + g.ready.images + g.reconcile > 0).map((g) => ({ title: g.title, environment: g.environment })),
  };
}

/** The collapsed row's one-line counts for an application. */
export function groupCounts(g: CleanupGroup): string {
  const remove = g.ready.containers + g.ready.images;
  const eligible = remove + g.reconcile;
  const head = eligible === 0 ? "0 eligible"
    : remove && g.reconcile ? `${eligible} eligible (${remove} to remove, ${g.reconcile} to reconcile)`
    : remove ? `${remove} eligible to remove` : `${g.reconcile} eligible for reconciliation`;
  return [head, g.protectedCount ? `${g.protectedCount} protected` : "", g.review ? `${g.review} to review` : ""].filter(Boolean).join(" · ");
}

/** The change in guest free space, in words, or why none can be given. Docker's image sizes share layers, so they are never summed. */
export function freedSummary(r: Pick<CleanupReport, "guestFreeBefore" | "guestFreeAfter">, size: (bytes: number) => string): string {
  if (r.guestFreeBefore === null || r.guestFreeAfter === null) return "The guest's free space could not be measured, so no figure is given.";
  const d = r.guestFreeAfter - r.guestFreeBefore;
  return `Guest free space: ${size(r.guestFreeBefore)} before, ${size(r.guestFreeAfter)} after (${d >= 0 ? "+" : "−"}${size(Math.abs(d))}).`;
}

export type CacheReport = {
  /** What a prune can remove: records that are unused and not shared with an image. An estimate, not a promise. */
  reclaimable: number | null; reclaimed: number | null; guestFreeBefore: number | null; guestFreeAfter: number | null;
  /** Docker's records, kept apart from `reclaimable`: their own sizes overlap each other and the images they share layers with. */
  records: number | null; total: number | null; inUse: number | null; shared: number | null;
  /** Why an apply would be refused right now (an unfinished deployment, an unreadable journal), or `null` when the guard is clear or was not reported. */
  blocked: string | null;
};

/** The engine's `prune-build-cache` report. Docker's reclaimed figure is shown as Docker's; the guest's own free space is the measured change. */
export function parseCache(detail: string): CacheReport {
  // a size is a whole, non-negative number of bytes; anything else (missing, negative, fractional, text) is "not reported", never a number
  const n = (key: string) => { const l = detail.split("\n").map((x) => x.trim()).find((x) => x.startsWith(`${key}:`)); const s = l ? l.slice(key.length + 1).trim() : ""; return /^\d+$/.test(s) && Number.isSafeInteger(Number(s)) ? Number(s) : null; };
  return { reclaimable: n("build-cache-reclaimable-bytes"), records: n("build-cache-records"), total: n("build-cache-total-bytes"), inUse: n("build-cache-in-use-bytes"), shared: n("build-cache-shared-bytes"), reclaimed: n("build-cache-reclaimed-bytes"), guestFreeBefore: n("guest-free-before-bytes"), guestFreeAfter: n("guest-free-after-bytes"), blocked: (() => { const l = detail.split("\n").map((x) => x.trim()).find((x) => x.startsWith("prune-guard: blocked")); return l ? l.replace(/^prune-guard: blocked:?\s*/, "") || "the engine did not say why" : null; })() };
}

/** Why a prune frees what it frees, from the engine's own figures. Null when there is nothing to explain (no records, or none shared with an
 *  image). Record sizes overlap each other and the images they share layers with, so they are never offered as space. */
export function cacheExplanation(c: CacheReport, size: (bytes: number) => string): string | null {
  if (c.reclaimable === null || !c.total || !c.shared) return null;
  const inUse = c.inUse ? ` and ${size(c.inUse)} is in use` : "";
  if (c.reclaimable === 0) {
    return `Docker lists ${c.records ?? "some"} build-cache record${c.records === 1 ? "" : "s"} whose own sizes add up to ${size(c.total)}, but that is not free space: the records overlap, and ${size(c.shared)} is shared with images${inUse}. A prune would free about 0 B. The space is held by images, so removing old releases is what frees it.`;
  }
  return `A prune can remove up to ${size(c.reclaimable)}. The rest of Docker's ${size(c.total)} of record sizes is not free space: ${size(c.shared)} is shared with images${inUse}, and stays.`;
}
