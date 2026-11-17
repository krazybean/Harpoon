import { useEffect, useState, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { createDiskTelemetry, diskView, POLL_MS } from "../lib/diskTelemetry";
import type { DiskView } from "../lib/diskTelemetry";

/** The one disk reading every view shares; `useDiskTelemetryPolling` (mounted once, in App) is the only thing that polls it. */
export const diskTelemetry = createDiskTelemetry(() => invoke<string>("get_disk_capacity"));

/** One `df` over the management channel per minute while Harpoon is ready. The effect owns the timer and the enabled flag, so a (Strict Mode) re-run or unmount cannot leave either behind; concurrent refreshes share one request. */
export function useDiskTelemetryPolling(enabled: boolean) {
  useEffect(() => {
    diskTelemetry.setEnabled(enabled);
    if (!enabled) return;
    void diskTelemetry.refresh();
    const id = window.setInterval(() => { if (document.visibilityState === "visible") void diskTelemetry.refresh(); }, POLL_MS);
    return () => { window.clearInterval(id); diskTelemetry.setEnabled(false); };
  }, [enabled]);
}

/** Re-evaluate "N ago" and staleness this often while mounted, independent of whether any refresh completes. */
export const TICK_MS = 15_000;

export function useDiskView(): DiskView {
  const t = useSyncExternalStore(diskTelemetry.subscribe, diskTelemetry.get);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), TICK_MS);
    return () => window.clearInterval(id);
  }, []);
  return diskView(t, now);
}

/**
 * The Mac-side allocation of the disk image right now, or `null`. It forces a fresh read and accepts only one that completed after this call
 * began, so a remembered or stale figure is never used as a before/after measurement.
 */
export async function hostAllocationNow(): Promise<number | null> {
  const started = Date.now();
  try { await diskTelemetry.refresh(); } catch { return null; }
  const t = diskTelemetry.get();
  if (!t.enabled || t.failure || !t.last || t.last.at < started || !t.last.cap.provisioned) return null;
  return t.last.cap.physicalBytes;
}
