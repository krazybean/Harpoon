import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type ResourceSample = {
  timestampMs: number;
  cpuPercent?: number | null;
  memoryUsedBytes?: number | null;
  memoryTotalBytes?: number | null;
  diskLogicalBytes?: number | null;
  diskAllocatedBytes?: number | null;
};
export type ResourceSnapshot = { samples?: ResourceSample[] };

let snapshot: ResourceSnapshot | null = null;
let enabled = false;
let diskPath: string | null = null;
let generation = 0;
let unlisten: (() => void) | undefined;
const listeners = new Set<() => void>();

function publish(next: ResourceSnapshot) {
  snapshot = next;
  listeners.forEach((listener) => listener());
}

async function start(currentGeneration: number) {
  const cleanup = await listen<ResourceSnapshot>("resource-sample", ({ payload }) => {
    if (enabled && currentGeneration === generation) publish(payload);
  });
  if (!enabled || currentGeneration !== generation) {
    cleanup();
    return;
  }
  unlisten = cleanup;
  void invoke("start_resource_sampler", { diskPath });
}

function stop() {
  generation += 1;
  unlisten?.();
  unlisten = undefined;
  void invoke("stop_resource_sampler");
}

export function configureResourceSampler(next: { enabled: boolean; diskPath?: string | null }) {
  const nextDiskPath = next.diskPath ?? null;
  if (enabled === next.enabled && diskPath === nextDiskPath) return;
  enabled = next.enabled;
  diskPath = nextDiskPath;
  if (!enabled) {
    stop();
    snapshot = null;
    publish({ samples: [] });
    return;
  }
  stop();
  const currentGeneration = generation;
  void start(currentGeneration);
}

export function subscribeResourceSnapshot(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getResourceSnapshot() {
  return snapshot;
}
