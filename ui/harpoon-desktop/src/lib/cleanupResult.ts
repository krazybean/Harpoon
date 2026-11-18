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

export type Measured = number | null;
export type Pair = { before: Measured; after: Measured } | "not-applicable";
export type ResultKind = "old-releases" | "build-cache" | "disk-blocks";
/** success; nothing (there was nothing to do); partial (some of it was done); refused (the operation declined, nothing changed);
 *  failed (the engine answered that it did not complete); unknown (no answer: it may have run fully, partly or not at all). */
export type ResultStatus = "success" | "nothing" | "partial" | "refused" | "failed" | "unknown";

export interface Figures {
  /** Docker's or the engine's own accounting. `bytes` is `null` when no figure was given; `note` says what was counted. */
  docker: { applicable: boolean; bytes: Measured; note: string; /** `bytes` is an estimate, not what Docker reported. */ estimated?: boolean };
  guest: Pair;
  mac: Pair;
}

export interface CleanupOutcome {
  kind: ResultKind; status: ResultStatus; headline: string; detail: string | null; figures: Figures;
  /** Objects the engine looked at and left in place, with its own reasons (a partial result must keep them). */
  kept?: string[];
}

/** The host allocation taken around an operation. */
export interface HostPair { before: Measured; after: Measured }
const NONE: HostPair = { before: null, after: null };
const EXIT_REFUSED = 6; // deploy-cli's exit code for "declined, nothing was changed"
const UNKNOWN_MARK = "result unknown:"; // what the engine writes when Docker gave no usable answer: the work may or may not have happened

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
const unknownFigures = (kind: ResultKind, host: HostPair): Figures => ({ docker: { applicable: kind !== "disk-blocks", bytes: null, note: "No figure was reported." }, guest: kind === "disk-blocks" ? "not-applicable" : { before: null, after: null }, mac: { before: host.before, after: host.after } });
const REVIEW_AGAIN = "Review again to see the current state before doing anything else.";

/** The outcome when the engine answered "not ok" or the request never produced an answer. Neither is ever shown as success. */
export function failureOutcome(kind: ResultKind, answer: OpResult | null, thrown: unknown, host: HostPair = NONE): CleanupOutcome {
  const figures = unknownFigures(kind, host);
  if (answer) {
    const why = answer.hint || answer.summary || "The engine gave no reason.";
    // The engine itself says Docker gave no usable answer: the operation may have run. That is unknown, not a failure.
    if (answer.summary.startsWith(UNKNOWN_MARK) || answer.detail.includes(`\n${UNKNOWN_MARK}`) || answer.detail.startsWith(UNKNOWN_MARK)) {
      return { kind, status: "unknown", headline: "The engine could not confirm the result.", detail: why.includes("Review again") ? why : `${why} ${REVIEW_AGAIN}`, figures };
    }
    return answer.exitCode === EXIT_REFUSED
      ? { kind, status: "refused", headline: "Not carried out. Nothing was changed.", detail: why, figures }
      : { kind, status: "failed", headline: "The operation did not complete.", detail: why, figures };
  }
  return { kind, status: "unknown", headline: "No answer was received, so the result is not known.", detail: `${String(thrown instanceof Error ? thrown.message : thrown ?? "The request ended.").replace(/^Error:\s*/, "")} The operation may have completed fully, partly or not at all. ${REVIEW_AGAIN}`, figures };
}

/** `cleanup-releases --apply`: what was planned (from the reviewed dry run) against what the engine reports it did. The engine re-checks
 *  every object when it applies, so fewer removals than planned means it skipped some after looking again. A report without the engine's
 *  own counts is not read as "nothing removed": it is unknown. */
export function releasesOutcome(plan: CleanupReport, report: CleanupReport, host: HostPair): CleanupOutcome {
  const planned = operationSummary(plan);
  const plannedDocker = planned.containers + planned.images;
  const doneDocker = report.containers + report.images;
  const reconciled = reconcilable(report).length;
  const figures: Figures = {
    // The count is references: an image's space is freed only when no image that stays keeps its layers. The estimate comes from
    // layer sharing and is said apart from the measured guest change below.
    docker: report.estimatedBytes === null || (report.estimatedBytes === 0 && report.estimateExact === false)
      ? { applicable: true, bytes: null, note: `${plural(report.containers, "container")} and ${plural(report.images, "image reference")} removed. No byte estimate was available: image sizes are shared between images.` }
      : { applicable: true, bytes: report.estimatedBytes, estimated: true, note: `${plural(report.containers, "container")} and ${plural(report.images, "image reference")} removed. This is an estimate from layer sharing${report.estimateExact === false ? " (a lower bound)" : ""}: space is freed only for layers no remaining image keeps.` },
    guest: { before: report.guestFreeBefore, after: report.guestFreeAfter },
    mac: { before: host.before, after: host.after },
  };
  const kept = report.kept;
  if (!report.reported) {
    return { kind: "old-releases", status: "unknown", headline: "The engine's report was not in a form this app understands, so the result is not known.", detail: `It did not state how many objects it removed. Nothing is assumed from the absence of an error. ${REVIEW_AGAIN}`, figures: { ...figures, docker: { applicable: true, bytes: null, note: "No count was reported." } }, kept };
  }
  const removed = `Removed ${report.containers} container(s) and ${report.images} image(s); reconciled ${reconciled} journal record(s).`;
  if (plannedDocker > 0 && doneDocker === 0 && reconciled === 0) return { kind: "old-releases", status: "nothing", headline: "Nothing was removed.", detail: "The engine re-checked every object when applying and removed none of them. It keeps anything it can no longer prove is safe to remove.", figures, kept };
  if (doneDocker < plannedDocker) return { kind: "old-releases", status: "partial", headline: `${removed}`, detail: `${doneDocker} of ${plannedDocker} Docker objects were removed. The engine re-checks each object when applying and left the rest in place. ${REVIEW_AGAIN}`, figures, kept };
  if (plannedDocker === 0 && doneDocker === 0 && reconciled === 0) return { kind: "old-releases", status: "nothing", headline: "Nothing was removed.", detail: "There was nothing eligible.", figures, kept };
  const why = [report.reclaimCheck, ...report.sharedLayers].filter(Boolean).join(" ");
  return { kind: "old-releases", status: "success", headline: removed, detail: why || null, figures, kept };
}

/** `prune-build-cache`: Docker's own reclaimed figure is shown as Docker's; the guest and Mac measurements stand apart. */
export function cacheOutcome(report: CacheReport, host: HostPair, fmt: (b: number) => string): CleanupOutcome {
  const figures: Figures = {
    docker: { applicable: true, bytes: report.reclaimed, note: report.reclaimed === null ? "Docker gave no figure." : "What Docker says it removed. It is an accounting figure, not guaranteed free disk space." },
    guest: { before: report.guestFreeBefore, after: report.guestFreeAfter },
    mac: { before: host.before, after: host.after },
  };
  if (report.reclaimed === null) return { kind: "build-cache", status: "unknown", headline: "The engine's report did not state what Docker removed, so the result is not known.", detail: `Nothing is assumed from the absence of an error. ${REVIEW_AGAIN}`, figures };
  return { kind: "build-cache", status: report.reclaimed === 0 ? "nothing" : "success", headline: report.reclaimed === 0 ? "Docker reports 0 B of build cache removed: there was nothing it could remove." : `Docker reports ${fmt(report.reclaimed)} of build cache removed.`, detail: null, figures };
}

const wholeBytes = (v: unknown): Measured => (typeof v === "number" && Number.isSafeInteger(v) && v >= 0 ? v : null);
export const RECLAIM_REFUSED = ["busy", "not-running", "unknown-disk", "engine-unreachable"]; // refused before anything ran: "nothing was changed"
export const RECLAIM_FAILED = ["unsupported", "failed", "error"];                              // the trim ran or was attempted and reports it did not work

/** `harpoon disk reclaim --yes --json`: one JSON object, on success and on failure. The guest hands blocks back to macOS; Docker is not touched,
 *  so there is no Docker figure. Anything that is not that object, or names a status this app does not know, is unknown, not a guess. */
export function reclaimOutcomeFromText(text: string): CleanupOutcome {
  const unknown = (why: string): CleanupOutcome => ({ kind: "disk-blocks", status: "unknown", headline: "The reclaim reported a result this app does not understand, so it is not known.", detail: `${why} ${REVIEW_AGAIN}`, figures: { docker: { applicable: false, bytes: null, note: "" }, guest: "not-applicable", mac: { before: null, after: null } } });
  let o: unknown;
  try { o = JSON.parse(text); } catch { return unknown(`The runtime said: ${text.trim().slice(0, 300) || "nothing"}.`); }
  if (typeof o !== "object" || o === null || typeof (o as Record<string, unknown>).status !== "string") return unknown("The runtime's answer was not a result object.");
  const r = o as Record<string, unknown>;
  const status = r.status as string;
  const figures: Figures = { docker: { applicable: false, bytes: null, note: "" }, guest: "not-applicable", mac: { before: wholeBytes(r.physicalBeforeBytes), after: wholeBytes(r.physicalAfterBytes) } };
  const message = typeof r.message === "string" && r.message ? r.message : status;
  const base = { kind: "disk-blocks" as const, headline: message, detail: null, figures };
  if (status === "reclaimed") return { ...base, status: "success" };
  if (status === "nothing-reclaimed") return { ...base, status: "nothing" };
  if (RECLAIM_REFUSED.includes(status)) return { ...base, status: "refused" };
  if (RECLAIM_FAILED.includes(status)) return { ...base, status: "failed" };
  return unknown(`Its status was "${status.slice(0, 60)}".`);
}

const size = (fmt: (b: number) => string, b: Measured) => (b === null ? "not measured" : fmt(b));

/** The three measurement lines, always in the same order and wording. */
export function figureLines(o: CleanupOutcome, fmt: (b: number) => string): { key: "docker" | "guest" | "mac"; title: string; text: string }[] {
  const f = o.figures;
  const docker = !f.docker.applicable ? "Not applicable: this operation does not change Docker." : `${f.docker.bytes === null ? "" : `${fmt(f.docker.bytes)}. `}${f.docker.note}`;
  const pair = (p: Pair, none: string, change: (d: number) => string) => {
    if (p === "not-applicable") return none;
    if (p.before === null || p.after === null) return p.before === null && p.after === null ? "Not measured." : `Before ${size(fmt, p.before)}, after ${size(fmt, p.after)}. A change cannot be stated.`;
    const d = p.after - p.before;
    return `${fmt(p.before)} before, ${fmt(p.after)} after (${change(d)}).`;
  };
  const sign = (d: number) => (d === 0 ? "unchanged" : `${d > 0 ? "+" : "−"}${fmt(Math.abs(d))}`);
  return [
    { key: "docker", title: f.docker.estimated ? "Estimated space freed" : "Docker reported", text: docker },
    { key: "guest", title: "Guest free space", text: pair(f.guest, "Not changed by this operation.", sign) },
    { key: "mac", title: "Mac disk-image allocation", text: `${pair(f.mac, "Not measured.", sign)}${o.kind === "disk-blocks" || f.mac === "not-applicable" ? "" : " Space freed inside Harpoon is only returned to macOS by Reclaim unused space, so this often stays the same."}` },
  ];
}
