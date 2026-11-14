// The engine's `cleanup-releases` report, read for the Reclaim Unused Space card. The engine states every removal
// before and after it happens; this only turns that text into numbers and lines the card can show.

export type CleanupReport = {
  /** What would be (or was) removed, one engine line each. */
  actions: string[];
  /** Deployments the cleanup never touches (the release serving now and whatever owns its container). */
  protectedCount: number;
  /** Artifacts that were left alone, with the engine's reason. */
  kept: string[];
  containers: number;
  images: number;
  /** Guest filesystem free space around an applied cleanup, when the management channel could measure it. */
  guestFreeBefore: number | null;
  guestFreeAfter: number | null;
};

export function parseCleanup(detail: string): CleanupReport {
  const lines = detail.split("\n").map((l) => l.trim()).filter(Boolean);
  const num = (key: string) => { const l = lines.find((x) => x.startsWith(`${key}:`)); return l ? Number(l.slice(key.length + 1)) : 0; };
  const bytes = (key: string) => { const l = lines.find((x) => x.startsWith(`${key}:`)); const n = l ? Number(l.slice(key.length + 1)) : NaN; return Number.isFinite(n) ? n : null; };
  const isAction = (l: string) => /^(would: )?(remove (container|image) |release port lease |.* is not in the runtime)/.test(l);
  return {
    actions: lines.filter(isAction).map((l) => l.replace(/^would: /, "")),
    protectedCount: lines.filter((l) => l.startsWith("protected:")).length,
    kept: lines.filter((l) => l.startsWith("kept:")).map((l) => l.slice(5).trim()),
    containers: num("reclaimed-containers"),
    images: num("reclaimed-images"),
    guestFreeBefore: bytes("guest-free-before-bytes"),
    guestFreeAfter: bytes("guest-free-after-bytes"),
  };
}

/** Whether there is anything to remove (a dry run lists it; an applied run reports counts). */
export const nothingToClean = (r: CleanupReport): boolean => r.actions.length === 0 && r.containers === 0 && r.images === 0;

/** The change in guest free space, in words, or why none can be given. Docker's image sizes share layers, so they are never summed. */
export function freedSummary(r: CleanupReport, size: (bytes: number) => string): string {
  if (r.guestFreeBefore === null || r.guestFreeAfter === null) return "The guest's free space could not be measured, so no figure is given.";
  const d = r.guestFreeAfter - r.guestFreeBefore;
  return `Guest free space: ${size(r.guestFreeBefore)} before, ${size(r.guestFreeAfter)} after (${d >= 0 ? "+" : "−"}${size(Math.abs(d))}).`;
}

export type CacheReport = { reclaimable: number | null; reclaimed: number | null; guestFreeBefore: number | null; guestFreeAfter: number | null };

/** The engine's `prune-build-cache` report. Docker's reclaimed figure is shown as Docker's; the guest's own free space is the measured change. */
export function parseCache(detail: string): CacheReport {
  const n = (key: string) => { const l = detail.split("\n").map((x) => x.trim()).find((x) => x.startsWith(`${key}:`)); const v = l ? Number(l.slice(key.length + 1)) : NaN; return Number.isFinite(v) ? v : null; };
  return { reclaimable: n("build-cache-reclaimable-bytes"), reclaimed: n("build-cache-reclaimed-bytes"), guestFreeBefore: n("guest-free-before-bytes"), guestFreeAfter: n("guest-free-after-bytes") };
}
