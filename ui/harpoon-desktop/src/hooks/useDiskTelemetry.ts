import { useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { createDiskTelemetry, diskView, POLL_MS } from "../lib/diskTelemetry";
import type { DiskView } from "../lib/diskTelemetry";

/** The one disk reading every view shares; `useDiskTelemetryPolling` (mounted once, in App) is the only thing that polls it. */
export const diskTelemetry = createDiskTelemetry(() => invoke<string>("get_disk_capacity"));

/** One `df` over the management channel per minute while Harpoon is ready; a hidden window skips the refresh, never the staleness check. */
export function useDiskTelemetryPolling(enabled: boolean) {
  useEffect(() => {
    diskTelemetry.setEnabled(enabled);
    if (!enabled) return;
    void diskTelemetry.refresh();
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") void diskTelemetry.refresh(); else diskTelemetry.touch();
    }, POLL_MS);
    return () => window.clearInterval(id);
  }, [enabled]);
}

export function useDiskView(): DiskView {
  const t = useSyncExternalStore(diskTelemetry.subscribe, diskTelemetry.get);
  return diskView(t, Date.now());
}
