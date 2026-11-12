// Low-disk notification policy for the desktop: when to show the notice, how a dismissal is remembered, which sizes to offer, and
// what the confirmation says. Pure functions over the numbers `harpoon disk status --json` reports; the runtime owns the thresholds
// (pressure) and every refusal, this only decides what to show and never stops, starts or resizes anything.

export type DiskPressure = "unknown" | "ok" | "warning" | "critical";

export interface DiskCapacity {
  provisioned: boolean;
  state: string;
  logicalBytes: number;
  physicalBytes: number;
  hostAvailableBytes?: number;
  pressure: DiskPressure;
  suggestedSizes: number[];
  suggestions?: { bytes: number; oversubscribed: boolean }[];
  maxBytes: number;
  hostReserveBytes: number;
  guest?: { capacityBytes: number; usedBytes: number; availableBytes: number };
  pendingExpansion?: { phase: string; fromBytes: number; toBytes: number; active?: boolean; recovery: string };
}

export interface Dismissal {
  level: "warning" | "critical";
  logicalBytes: number;
  usedBucket: number;
  at: number;
}

export const GIB = 1024 * 1024 * 1024;
export const MIB = 1024 * 1024;
const BUCKET_PERCENT = 5;
const CRITICAL_REMIND_MS = 60 * 60 * 1000;

export function parseDiskCapacity(text: string): DiskCapacity | null {
  try {
    const o = JSON.parse(text);
    if (!o || typeof o !== "object" || typeof o.logicalBytes !== "number" || typeof o.pressure !== "string") return null;
    return o as DiskCapacity;
  } catch {
    return null;
  }
}

export function usedPercent(c: DiskCapacity): number | null {
  if (!c.guest) return null;
  const total = c.guest.usedBytes + c.guest.availableBytes;
  return total > 0 ? (c.guest.usedBytes / total) * 100 : null;
}

const bucket = (c: DiskCapacity) => Math.floor((usedPercent(c) ?? 0) / BUCKET_PERCENT);

export function formatSize(bytes: number): string {
  if (bytes >= GIB) { const g = bytes / GIB; return `${Number.isInteger(g) ? g : g.toFixed(1)} GiB`; }
  return `${Math.round(bytes / MIB)} MiB`;
}

/** The size argument the CLI takes, e.g. 8589934592 -> "8G". */
export function sizeArg(bytes: number): string {
  return bytes % GIB === 0 ? `${bytes / GIB}G` : `${Math.floor(bytes / MIB)}M`;
}

/**
 * What to show. Nothing unless Harpoon is running and the runtime reports warning or critical. A dismissed warning stays quiet while
 * the disk is the same size, still only at "warning", and usage has not climbed a further 5 points; critical always re-appears when
 * the level escalates, and a dismissed critical comes back after an hour while it persists. Growing the disk resets everything.
 */
export function noticeLevel(c: DiskCapacity | null, dismissed: Dismissal | null, now: number): "none" | "warning" | "critical" {
  if (!c || c.state !== "running" || !c.provisioned) return "none";
  if (c.pressure !== "warning" && c.pressure !== "critical") return "none";
  if (!dismissed || dismissed.logicalBytes !== c.logicalBytes) return c.pressure;
  if (c.pressure === "critical" && dismissed.level === "warning") return "critical";
  if (c.pressure === "critical") return now - dismissed.at >= CRITICAL_REMIND_MS ? "critical" : "none";
  return bucket(c) > dismissed.usedBucket ? "warning" : "none";
}

export function dismissalFor(c: DiskCapacity, now: number): Dismissal {
  return { level: c.pressure === "critical" ? "critical" : "warning", logicalBytes: c.logicalBytes, usedBucket: bucket(c), at: now };
}

export interface SizeAssessment { error: string | null; oversubscribed: boolean; worstCaseBytes: number }

/**
 * The runtime's host-space policy, mirrored so the dialog can explain a size before asking. Sparse growth allocates almost nothing
 * now (ext4 metadata, about 2% of the added capacity, must fit with the reserve); filling the whole disk is only a worst case, which
 * may be at most 4x the headroom (free minus reserve) and is flagged when it exceeds it. The runtime re-checks everything.
 */
export function assessSize(requested: number, c: DiskCapacity): SizeAssessment {
  const worst = Math.max(0, requested - c.physicalBytes);
  const no = (error: string): SizeAssessment => ({ error, oversubscribed: false, worstCaseBytes: worst });
  if (requested <= c.logicalBytes) return no(`Choose more than the current ${formatSize(c.logicalBytes)}; disks only grow.`);
  if (requested > c.maxBytes) return no(`The growth cap is ${formatSize(c.maxBytes)}.`);
  const host = c.hostAvailableBytes;
  if (host === undefined) return no("Host free space is unknown.");
  const immediate = Math.ceil((requested - c.logicalBytes) * 0.02);
  if (immediate > host || host - immediate < c.hostReserveBytes) {
    return no(`The host has ${formatSize(host)} free; the grow needs about ${formatSize(immediate)} for filesystem metadata and ${formatSize(c.hostReserveBytes)} must stay free.`);
  }
  const headroom = host - c.hostReserveBytes;
  if (worst > headroom * 4) {
    return no(`Filling a ${formatSize(requested)} disk could need ${formatSize(worst)}, more than 4 times the ${formatSize(headroom)} this Mac can spare. Choose a smaller size or free space.`);
  }
  return { error: null, oversubscribed: worst > headroom, worstCaseBytes: worst };
}

/** Why a custom size (in whole GiB, as typed) cannot be offered for confirmation, or null. */
export function customSizeError(input: string, c: DiskCapacity): string | null {
  if (!/^\d+$/.test(input.trim())) return "Enter a whole number of GiB.";
  return assessSize(Number(input.trim()) * GIB, c).error;
}

export function summary(c: DiskCapacity): string {
  const g = c.guest;
  if (!g) return `Docker disk ${formatSize(c.logicalBytes)}.`;
  const pct = Math.round(usedPercent(c) ?? 0);
  return `Docker disk is ${pct}% full: ${formatSize(g.availableBytes)} free of ${formatSize(g.capacityBytes)}.`;
}

/** The text of the confirmation: what happens, to what, and that containers are interrupted. Shown before any lifecycle action. */
export function confirmationMessage(c: DiskCapacity, requestedBytes: number): string {
  const a = assessSize(requestedBytes, c);
  return `Harpoon will stop its VM, grow the Docker disk from ${formatSize(c.logicalBytes)} to ${formatSize(requestedBytes)}, then start it again. `
    + `Running containers are interrupted and restart according to their restart policy. Your images, volumes and containers are kept. `
    + `This cannot be undone: disks only grow.`
    + (a.oversubscribed ? ` If this disk ever filled completely it would need up to ${formatSize(a.worstCaseBytes)} on this Mac, more than it can spare; nothing stops a guest that fills it, so keep an eye on free space.` : "");
}

/** A disk expansion is running (or was started by another window); lifecycle controls must not offer Start, Stop or Restart. */
export function expansionActive(status: { diskExpansion?: { active?: boolean } } | null | undefined): boolean {
  return Boolean(status?.diskExpansion?.active);
}

// --- expansion progress -----------------------------------------------------------------------------------------------------------
// The runtime writes its phase to the expansion record before each step (`stopping`, `growing`, `starting`, `verifying`) and
// `harpoon status --json` relays it as `diskExpansion`. The stages below are exactly those phases: no percentages, no timers.

export interface ExpansionState { active?: boolean; phase: string; fromBytes?: number; toBytes?: number; recovery?: string }

export const EXPANSION_STAGES = [
  { phase: "stopping", label: "Stopping Harpoon" },
  { phase: "growing", label: "Growing sparse disk" },
  { phase: "starting", label: "Starting Harpoon" },
  { phase: "verifying", label: "Verifying filesystem and Docker readiness" },
] as const;

export type StageState = "done" | "active" | "pending";

/** "running": show stages and block lifecycle actions; "interrupted": show what to do; "none": no record, show nothing (cleared after success). */
export function expansionPanel(e: ExpansionState | null | undefined): "none" | "running" | "interrupted" {
  if (!e) return "none";
  return e.active ? "running" : "interrupted";
}

/** The four stages with the one the record names marked active. An unknown phase (a newer runtime) marks none active rather than guessing. */
export function expansionStages(e: ExpansionState | null | undefined): { label: string; state: StageState }[] {
  const at = EXPANSION_STAGES.findIndex((s) => s.phase === e?.phase);
  return EXPANSION_STAGES.map((s, i) => ({ label: s.label, state: at < 0 ? "pending" : i < at ? "done" : i === at ? "active" : "pending" }));
}

/** Interrupted-expansion text: the runtime's own recovery instructions when it supplies them (older runtimes do not). */
export function interruptedText(e: ExpansionState): string {
  return e.recovery || `A disk expansion stopped at "${e.phase}". Nothing was lost. Run \`harpoon status\` and \`harpoon disk status\` to see where it stopped.`;
}

// --- space reclamation ------------------------------------------------------------------------------------------------------------

export interface ReclaimResult {
  status: string; message: string;
  logicalBytes?: number; physicalBeforeBytes?: number; physicalAfterBytes?: number; reclaimedBytes?: number; changeBytes?: number;
}

/** `harpoon disk reclaim --yes --json` prints one JSON object (also on failure). Anything else is shown as the message it is. */
export function parseReclaimResult(text: string): ReclaimResult {
  try {
    const o = JSON.parse(text);
    if (o && typeof o === "object" && typeof o.status === "string") return { ...o, message: typeof o.message === "string" ? o.message : o.status };
  } catch { /* fall through */ }
  return { status: "error", message: text.trim() || "Reclaiming unused space failed." };
}

/** Why the action is unavailable right now, or null. Reclamation needs the running guest and must not overlap an expansion. */
export function reclaimBlockedReason(c: DiskCapacity | null, expansion: ExpansionState | null | undefined): string | null {
  if (expansionPanel(expansion) === "running") return "A disk expansion is in progress.";
  if (!c || !c.provisioned) return "No Docker disk exists yet.";
  if (c.state !== "running") return "Harpoon must be running to reclaim unused space.";
  return null;
}

/** The text of the confirmation. It states what reclamation is and is not, and promises no amount. */
export function reclaimConfirmationMessage(c: DiskCapacity): string {
  return `Harpoon will ask the guest to trim unused blocks (fstrim /) so macOS can take back disk space. The disk's ${formatSize(c.logicalBytes)} capacity stays the same, `
    + `${formatSize(c.physicalBytes)} is currently allocated on this Mac, and the amount returned is measured afterwards: it may be small or zero. `
    + `This does not delete Docker images, containers or volumes, does not prune anything, and does not restart Harpoon.`;
}
