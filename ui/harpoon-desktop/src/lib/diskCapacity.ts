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
  maxBytes: number;
  hostReserveBytes: number;
  guest?: { capacityBytes: number; usedBytes: number; availableBytes: number };
  pendingExpansion?: { phase: string; fromBytes: number; toBytes: number; recovery: string };
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

/** Why a custom size (in whole GiB, as typed) cannot be offered for confirmation, or null. The runtime re-checks everything. */
export function customSizeError(input: string, c: DiskCapacity): string | null {
  if (!/^\d+$/.test(input.trim())) return "Enter a whole number of GiB.";
  const bytes = Number(input.trim()) * GIB;
  if (bytes <= c.logicalBytes) return `Choose more than the current ${formatSize(c.logicalBytes)}; disks only grow.`;
  if (bytes > c.maxBytes) return `The growth cap is ${formatSize(c.maxBytes)}.`;
  const host = c.hostAvailableBytes;
  if (host === undefined) return "Host free space is unknown.";
  const added = bytes - c.logicalBytes;
  if (added > host || host - added < c.hostReserveBytes) {
    return `The host has ${formatSize(host)} free; growing by ${formatSize(added)} must leave ${formatSize(c.hostReserveBytes)} free.`;
  }
  return null;
}

export function summary(c: DiskCapacity): string {
  const g = c.guest;
  if (!g) return `Docker disk ${formatSize(c.logicalBytes)}.`;
  const pct = Math.round(usedPercent(c) ?? 0);
  return `Docker disk is ${pct}% full: ${formatSize(g.availableBytes)} free of ${formatSize(g.capacityBytes)}.`;
}

/** The text of the confirmation: what happens, to what, and that containers are interrupted. Shown before any lifecycle action. */
export function confirmationMessage(c: DiskCapacity, requestedBytes: number): string {
  return `Harpoon will stop its VM, grow the Docker disk from ${formatSize(c.logicalBytes)} to ${formatSize(requestedBytes)}, then start it again. `
    + `Running containers are interrupted and restart according to their restart policy. Your images, volumes and containers are kept. `
    + `This cannot be undone: disks only grow.`;
}
