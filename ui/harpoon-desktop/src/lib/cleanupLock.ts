import { useSyncExternalStore } from "react";

// Only one apply-type cleanup runs at a time on the Resources page: an old-releases cleanup, a build-cache prune or a disk-blocks reclaim.
// Each result brackets its own run with Mac allocation readings, so a second operation running meanwhile would put its effect inside the
// first one's "after" reading. The lock is held from the click, through the confirmation, to the result.
export type CleanupName = "old-releases" | "build-cache" | "disk-blocks" | "clean-up";
let active: CleanupName | null = null;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

/** The release function, or `null` when another cleanup holds the lock. */
export function acquireCleanup(name: CleanupName): (() => void) | null {
  if (active) return null;
  active = name; emit();
  return () => { if (active === name) { active = null; emit(); } };
}

/** Which cleanup is running now, if any. Components disable their apply buttons while one is. */
export function useCleanupActive(): CleanupName | null {
  return useSyncExternalStore((f) => { subs.add(f); return () => { subs.delete(f); }; }, () => active);
}
