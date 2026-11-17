// What a cleanup tells the person afterwards. Three measurements are kept apart because they are different things and must never be
// read as one another:
//   Docker   what Docker (or the engine) says it removed: an accounting figure, not free disk space;
//   Guest    free bytes on the guest filesystem, measured before and after through the management channel;
//   Mac      the disk image's physical allocation on macOS, which only shrinks when "Reclaim unused space" (fstrim) runs.
// A figure that was not measured is `null` and is shown as "not measured": it is never zero, and nothing is inferred from a request
// that merely returned or never answered.

import type { OpResult } from "./deployEngine.ts";
import type { CacheReport, CleanupReport } from "./artifactCleanup.ts";
import { operationSummary, reconcilable } from "./artifactCleanup.ts";
import type { ReclaimResult } from "./diskCapacity.ts";

export type Measured = number | null;
export type Pair = { before: Measured; after: Measured } | "not-applicable";
export type ResultKind = "old-releases" | "build-cache" | "disk-blocks";
/** success; nothing (there was nothing to do); partial (some of it was done); refused (the operation declined, nothing changed);
 *  failed (the engine answered that it did not complete); unknown (no answer: it may have run fully, partly or not at all). */
export type ResultStatus = "success" | "nothing" | "partial" | "refused" | "failed" | "unknown";

export interface Figures {
  /** Docker's or the engine's own accounting. `bytes` is `null` when no figure was given; `note` says what was counted. */
  docker: { applicable: boolean; bytes: Measured; note: string };
  guest: Pair;
  mac: Pair;
}

export interface CleanupOutcome { kind: ResultKind; status: ResultStatus; headline: string; detail: string | null; figures: Figures }

/** The host allocation taken around an operation. */
export interface HostPair { before: Measured; after: Measured }
const NONE: HostPair = { before: null, after: null };
const EXIT_REFUSED = 6; // deploy-cli's exit code for "declined, nothing was changed"

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The outcome when the engine answered "not ok" or the request never produced an answer. Neither is ever shown as success. */
export function failureOutcome(kind: ResultKind, answer: OpResult | null, thrown: unknown, host: HostPair = NONE): CleanupOutcome {
  const figures: Figures = { docker: { applicable: kind !== "disk-blocks", bytes: null, note: "No figure was reported." }, guest: kind === "disk-blocks" ? "not-applicable" : { before: null, after: null }, mac: { before: host.before, after: host.after } };
  if (answer) {
    const refused = answer.exitCode === EXIT_REFUSED;
    const why = answer.hint || answer.summary || "The engine gave no reason.";
    return refused
      ? { kind, status: "refused", headline: "Not carried out. Nothing was changed.", detail: why, figures }
      : { kind, status: "failed", headline: "The operation did not complete.", detail: `${why} Anything already removed is not restored; review again to see what remains.`, figures };
  }
  return { kind, status: "unknown", headline: "No answer was received, so the result is not known.", detail: `${String(thrown instanceof Error ? thrown.message : thrown ?? "The request ended.").replace(/^Error:\s*/, "")} The operation may have completed fully, partly or not at all. Review again to see the current state before doing anything else.`, figures };
}

/** `cleanup-releases --apply`: what was planned (from the reviewed dry run) against what the engine reports it did. The engine re-checks
 *  every object when it applies, so fewer removals than planned means it skipped some after looking again. */
export function releasesOutcome(plan: CleanupReport, report: CleanupReport, host: HostPair): CleanupOutcome {
  const planned = operationSummary(plan);
  const plannedDocker = planned.containers + planned.images;
  const doneDocker = report.containers + report.images;
  const reconciled = reconcilable(report).length;
  const figures: Figures = {
    docker: { applicable: true, bytes: null, note: `${plural(report.containers, "container")} and ${plural(report.images, "image")} removed. No byte figure exists: image sizes are shared between images.` },
    guest: { before: report.guestFreeBefore, after: report.guestFreeAfter },
    mac: { before: host.before, after: host.after },
  };
  const removed = `Removed ${report.containers} container(s) and ${report.images} image(s); reconciled ${reconciled} journal record(s).`;
  if (plannedDocker > 0 && doneDocker === 0 && reconciled === 0) return { kind: "old-releases", status: "nothing", headline: "Nothing was removed.", detail: "The engine re-checked every object when applying and removed none of them. It keeps anything it can no longer prove is safe to remove.", figures };
  if (doneDocker < plannedDocker) return { kind: "old-releases", status: "partial", headline: `${removed}`, detail: `${doneDocker} of ${plannedDocker} Docker objects were removed. The engine re-checks each object when applying and left the rest in place. Review again to see why.`, figures };
  return { kind: "old-releases", status: "success", headline: removed, detail: null, figures };
}

/** `prune-build-cache`: Docker's own reclaimed figure is shown as Docker's; the guest and Mac measurements stand apart. */
export function cacheOutcome(report: CacheReport, host: HostPair, fmt: (b: number) => string): CleanupOutcome {
  const figures: Figures = {
    docker: { applicable: true, bytes: report.reclaimed, note: report.reclaimed === null ? "Docker gave no figure." : "What Docker says it removed. It is an accounting figure, not guaranteed free disk space." },
    guest: { before: report.guestFreeBefore, after: report.guestFreeAfter },
    mac: { before: host.before, after: host.after },
  };
  if (report.reclaimed === null) return { kind: "build-cache", status: "unknown", headline: "Docker finished the prune but reported no figure.", detail: "Nothing is assumed about how much was removed.", figures };
  return { kind: "build-cache", status: report.reclaimed === 0 ? "nothing" : "success", headline: `Docker reports ${fmt(report.reclaimed)} of build cache removed.`, detail: null, figures };
}

/** `harpoon disk reclaim`: the guest hands blocks back to macOS. It does not touch Docker, so there is no Docker figure at all. */
export function reclaimOutcome(r: ReclaimResult): CleanupOutcome {
  const ok = r.status === "reclaimed" || r.status === "nothing-reclaimed";
  const figures: Figures = { docker: { applicable: false, bytes: null, note: "" }, guest: "not-applicable", mac: { before: r.physicalBeforeBytes ?? null, after: r.physicalAfterBytes ?? null } };
  return { kind: "disk-blocks", status: ok ? (r.status === "reclaimed" ? "success" : "nothing") : r.status === "not-running" ? "refused" : "failed", headline: r.message, detail: null, figures };
}

const size = (fmt: (b: number) => string, b: Measured) => (b === null ? "not measured" : fmt(b));

/** The three measurement lines, always in the same order and wording. */
export function figureLines(o: CleanupOutcome, fmt: (b: number) => string): { key: "docker" | "guest" | "mac"; title: string; text: string }[] {
  const f = o.figures;
  const docker = !f.docker.applicable ? "Not applicable: this operation does not change Docker." : `${f.docker.bytes === null ? "No byte figure" : fmt(f.docker.bytes)}. ${f.docker.note}`;
  const pair = (p: Pair, none: string, change: (d: number) => string) => {
    if (p === "not-applicable") return none;
    if (p.before === null || p.after === null) return p.before === null && p.after === null ? "Not measured." : `Before ${size(fmt, p.before)}, after ${size(fmt, p.after)}. A change cannot be stated.`;
    const d = p.after - p.before;
    return `${fmt(p.before)} before, ${fmt(p.after)} after (${change(d)}).`;
  };
  const sign = (d: number) => (d === 0 ? "unchanged" : `${d > 0 ? "+" : "−"}${fmt(Math.abs(d))}`);
  return [
    { key: "docker", title: "Docker reported", text: docker },
    { key: "guest", title: "Guest free space", text: pair(f.guest, "Not changed by this operation.", sign) },
    { key: "mac", title: "Mac disk-image allocation", text: `${pair(f.mac, "Not measured.", sign)}${o.kind === "disk-blocks" || f.mac === "not-applicable" ? "" : " Space freed inside Harpoon is only returned to macOS by Reclaim unused space, so this often stays the same."}` },
  ];
}
