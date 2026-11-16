// One shared reading of `harpoon disk status --json` for every place the desktop shows disk usage (the Resources meter, the header
// chip, the low-disk notice, the reclaim card). A single poll feeds them all. A failed refresh never erases the last good reading:
// it is kept with its time and shown as stale. Warning/critical is the runtime's own `pressure`; nothing here defines a threshold.

import { parseDiskCapacity, usedPercent } from "./diskCapacity.ts";
import type { DiskCapacity, DiskPressure } from "./diskCapacity.ts";

export const POLL_MS = 60_000;
/** A reading older than this (three missed polls) is no longer "live", even if no refresh has reported a failure. */
export const STALE_AFTER_MS = 3 * POLL_MS;

export interface Reading { cap: DiskCapacity; at: number }
export interface Failure { kind: "failed" | "unsupported"; message: string; at: number }
export interface Telemetry { enabled: boolean; last: Reading | null; failure: Failure | null; tick: number }

/**
 * live: a fresh reading with guest figures.
 * stale: the last good reading, older than expected or the latest refresh failed.
 * engine-down: Harpoon is not running; the figures, if any, are from before.
 * disconnected: the VM runs but the guest did not answer, so there are no guest figures (the host allocation still is real).
 * unsupported: the runtime answered in a shape this desktop does not understand.
 * unavailable: no reading yet, and the refresh failed or has not completed.
 */
export type DiskStatus = "live" | "stale" | "engine-down" | "disconnected" | "unsupported" | "unavailable";

export interface DiskView {
  status: DiskStatus;
  cap: DiskCapacity | null;
  at: number | null;
  /** Guest filesystem utilization, only when the last reading has guest figures. */
  percent: number | null;
  /** The runtime's classification of the last reading; "unknown" when there is none. */
  pressure: DiskPressure;
  note: string;
}

export const initialTelemetry = (): Telemetry => ({ enabled: false, last: null, failure: null, tick: 0 });

export function diskView(t: Telemetry, now: number): DiskView {
  const cap = t.last?.cap ?? null;
  const at = t.last?.at ?? null;
  const base = { cap, at, percent: cap ? usedPercent(cap) : null, pressure: (cap?.pressure ?? "unknown") as DiskPressure };
  if (!t.enabled) return { ...base, status: "engine-down", note: "Harpoon is not running." };
  if (t.failure?.kind === "unsupported") return { ...base, cap: null, at: null, percent: null, pressure: "unknown", status: "unsupported", note: "This runtime reports disk usage in a form this app does not understand." };
  if (t.failure) return t.last
    ? { ...base, status: "stale", note: `The last refresh failed: ${t.failure.message || "no reason given"}.` }
    : { ...base, status: "unavailable", note: `Disk usage could not be read: ${t.failure.message || "no reason given"}.` };
  if (!t.last) return { ...base, status: "unavailable", note: "Reading disk usage…" };
  if (t.last.cap.state !== "running") return { ...base, status: "engine-down", note: "Harpoon is not running." };
  if (!t.last.cap.guest) return { ...base, status: "disconnected", note: "The guest is not answering on its management channel, so its filesystem figures are unavailable." };
  if (now - t.last.at > STALE_AFTER_MS) return { ...base, status: "stale", note: "No recent refresh has completed." };
  return { ...base, status: "live", note: "" };
}

export interface DiskTelemetryStore {
  get(): Telemetry;
  subscribe(fn: () => void): () => void;
  refresh(): Promise<void>;
  setEnabled(enabled: boolean): void;
  /** Re-notify subscribers without a refresh, so time-based staleness is re-evaluated. */
  touch(): void;
}

export function createDiskTelemetry(fetchText: () => Promise<string>, now: () => number = Date.now): DiskTelemetryStore {
  let state = initialTelemetry();
  const subs = new Set<() => void>();
  let inflight: Promise<void> | null = null;
  const set = (next: Telemetry) => { state = next; subs.forEach((f) => f()); };
  return {
    get: () => state,
    subscribe(fn) { subs.add(fn); return () => { subs.delete(fn); }; },
    setEnabled(enabled) { if (enabled !== state.enabled) set({ ...state, enabled }); },
    touch() { set({ ...state, tick: state.tick + 1 }); },
    refresh() {
      if (inflight) return inflight; // one request at a time, whoever asks
      inflight = (async () => {
        try {
          const cap = parseDiskCapacity(await fetchText());
          set(cap
            ? { ...state, last: { cap, at: now() }, failure: null }
            : { ...state, failure: { kind: "unsupported", message: "unrecognized disk status", at: now() } });
        } catch (e) {
          set({ ...state, failure: { kind: "failed", message: String(e).trim(), at: now() } });
        } finally { inflight = null; }
      })();
      return inflight;
    },
  };
}

/** Meter text, kept apart so the guest filesystem and the Mac's sparse image are never mixed in one sentence. */
export function guestLine(c: DiskCapacity, fmt: (b: number) => string): string | null {
  const g = c.guest;
  if (!g) return null;
  return `${fmt(g.usedBytes)} used of ${fmt(g.capacityBytes)} · ${fmt(g.availableBytes)} free`;
}

export function hostLine(c: DiskCapacity, fmt: (b: number) => string): string | null {
  if (!c.provisioned) return null;
  return `${fmt(c.physicalBytes)} allocated on this Mac of the ${fmt(c.logicalBytes)} disk image`;
}

export function ageLabel(at: number | null, now: number): string {
  if (at === null) return "never";
  const s = Math.max(0, Math.round((now - at) / 1000));
  if (s < 5) return "just now";
  if (s < 90) return `${s} s ago`;
  const m = Math.round(s / 60);
  return m < 90 ? `${m} min ago` : `${Math.round(m / 60)} h ago`;
}
