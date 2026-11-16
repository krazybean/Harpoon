// One shared reading of `harpoon disk status --json` for every place the desktop shows disk usage (the Resources meter, the header
// chip, the low-disk notice, the reclaim card). A single poll feeds them all. A failed refresh never erases the last good reading:
// it is kept with its time and shown as stale. Warning/critical is the runtime's own `pressure`; nothing here defines a threshold.

import { parseDiskCapacity, usedPercent } from "./diskCapacity.ts";
import type { DiskCapacity, DiskPressure } from "./diskCapacity.ts";

export const POLL_MS = 60_000;
/** A reading older than this (three missed polls) is no longer "live", even if no refresh has reported a failure. */
export const STALE_AFTER_MS = 3 * POLL_MS;
/** A refresh that has not answered by now is abandoned as failed, so one hung request can never block every later refresh. */
export const FETCH_TIMEOUT_MS = 20_000;

export interface Reading { cap: DiskCapacity; at: number }
export interface Failure { kind: "failed" | "unsupported"; message: string; at: number }
export interface Telemetry { enabled: boolean; last: Reading | null; failure: Failure | null }

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
  /** The runtime's classification of the CURRENT reading. "unknown" unless the status is live: a remembered figure must not look like current health. */
  pressure: DiskPressure;
  note: string;
  /** The clock this view was computed against, so "N ago" labels agree with the staleness decision. */
  now: number;
}

export const initialTelemetry = (): Telemetry => ({ enabled: false, last: null, failure: null });

export function diskView(t: Telemetry, now: number): DiskView {
  const cap = t.last?.cap ?? null;
  const at = t.last?.at ?? null;
  // Historical usage (percent) stays available; the pressure class is only ever the live one.
  const base = { cap, at, percent: cap ? usedPercent(cap) : null, pressure: "unknown" as DiskPressure, now };
  if (!t.enabled) return { ...base, status: "engine-down", note: "Harpoon is not running." };
  if (t.failure?.kind === "unsupported") return { ...base, cap: null, at: null, percent: null, status: "unsupported", note: "This runtime reports disk usage in a form this app does not understand." };
  if (t.failure) return t.last
    ? { ...base, status: "stale", note: `The last refresh failed: ${t.failure.message || "no reason given"}.` }
    : { ...base, status: "unavailable", note: `Disk usage could not be read: ${t.failure.message || "no reason given"}.` };
  if (!t.last) return { ...base, status: "unavailable", note: "Reading disk usage…" };
  if (t.last.cap.state !== "running") return { ...base, status: "engine-down", note: "Harpoon is not running." };
  if (!t.last.cap.guest) return { ...base, status: "disconnected", note: "The guest is not answering on its management channel, so its filesystem figures are unavailable." };
  if (now - t.last.at > STALE_AFTER_MS) return { ...base, status: "stale", note: "No recent refresh has completed." };
  return { ...base, status: "live", pressure: t.last.cap.pressure, note: "" };
}

export interface DiskTelemetryStore {
  get(): Telemetry;
  subscribe(fn: () => void): () => void;
  refresh(): Promise<void>;
  setEnabled(enabled: boolean): void;
}

export function createDiskTelemetry(fetchText: () => Promise<string>, now: () => number = () => Date.now()): DiskTelemetryStore {
  let state = initialTelemetry();
  const subs = new Set<() => void>();
  let inflight: Promise<void> | null = null;
  // The runtime call itself. It cannot be cancelled from here, so after a timeout it may still be running: no second one starts until it ends.
  let native: Promise<unknown> | null = null;
  const set = (next: Telemetry) => { state = next; subs.forEach((f) => f()); };
  return {
    get: () => state,
    subscribe(fn) { subs.add(fn); return () => { subs.delete(fn); }; },
    setEnabled(enabled) { if (enabled !== state.enabled) set({ ...state, enabled }); },
    refresh() {
      if (inflight) return inflight; // one request at a time, whoever asks
      inflight = (async () => {
        try {
          if (native) throw new Error("the previous disk status request is still running");
          const call = Promise.resolve(fetchText());
          native = call;
          const release = () => { if (native === call) native = null; };
          call.then(release, release);
          let timer: ReturnType<typeof setTimeout> | undefined;
          const timeout = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("disk status timed out")), FETCH_TIMEOUT_MS); });
          // Only this race's winner is applied; a reply that arrives after the timeout is ignored.
          const cap = parseDiskCapacity(await Promise.race([call, timeout]).finally(() => clearTimeout(timer)));
          set(cap
            ? { ...state, last: { cap, at: now() }, failure: null }
            : { ...state, failure: { kind: "unsupported", message: "unrecognized disk status", at: now() } });
        } catch (e) {
          set({ ...state, failure: { kind: "failed", message: String(e).replace(/^Error:\s*/, "").trim(), at: now() } });
        }
      })().finally(() => { inflight = null; }); // after the assignment: a body that finishes without awaiting must not leave a settled promise stuck here
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
