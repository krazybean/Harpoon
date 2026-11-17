import { useEffect, useState, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { createInventoryStore, inventoryView } from "../lib/storageInventory";
import type { InventoryView } from "../lib/storageInventory";
import { TICK_MS } from "./useDiskTelemetry";

/** The one inventory the Resources page shows. Nothing polls it: it is read when the page opens and when the person asks. */
export const storageInventory = createInventoryStore(() => invoke<string>("storage_inventory"));

/** Read once when the page mounts with Harpoon running (a re-run of the effect shares the request in flight). The age label ticks; no request does. */
export function useStorageInventory(running: boolean): { view: InventoryView; refresh: () => void } {
  useEffect(() => {
    storageInventory.setEnabled(running);
    if (running) void storageInventory.refresh();
    return () => storageInventory.setEnabled(false);
  }, [running]);
  const t = useSyncExternalStore(storageInventory.subscribe, storageInventory.get);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), TICK_MS);
    return () => window.clearInterval(id);
  }, []);
  return { view: inventoryView(t, now), refresh: () => { void storageInventory.refresh(); } };
}
