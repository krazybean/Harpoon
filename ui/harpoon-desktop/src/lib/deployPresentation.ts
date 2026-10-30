// Presentation vocabulary for the Deployments views. Pure; unit-tested.
// The wording rules here encode the product philosophy: a recovery is only "restored" when it
// was, a failed deployment is not a failed recovery, and a green local check is evidence about
// the local container — never a promise about any other environment.
import type { DeploymentStatus, RecoveryOutcome, RestoreReason, StepStatus } from "./deployTypes.ts";

export type Tone = "ok" | "info" | "warn" | "err" | "neutral";

export const EVIDENCE_NOTE = "Evidence about the local container only — not a guarantee of behavior on Vercel or any other environment.";
export const SIMULATION_NOTE = "Simulated data. No deployment engine is connected: nothing here is a real deployment, container, secret, log or metric.";
export const ROLLBACK_WARNING = "Application rollback only. Your database, volumes and schema are not rolled back and may be incompatible with the earlier release.";

export function formatAge(min: number): string {
  if (min < 1) return "just now";
  if (min < 60) return `${Math.round(min)} min ago`;
  if (min < 60 * 24) return `${Math.round(min / 60)} h ago`;
  const d = Math.round(min / (60 * 24));
  return d === 1 ? "yesterday" : `${d} days ago`;
}

export function statusTone(s: DeploymentStatus): Tone {
  return ({ succeeded: "ok", failed: "err", running: "info", cancelled: "neutral", interrupted: "warn", superseded: "neutral" } as const)[s];
}
export function statusLabel(s: DeploymentStatus, recovering: boolean): string {
  if (s === "running" && recovering) return "Recovering";
  return ({ succeeded: "Succeeded", failed: "Failed", running: "Running", cancelled: "Cancelled", interrupted: "Interrupted", superseded: "Superseded" } as const)[s];
}

export function recoveryTone(o: RecoveryOutcome): Tone {
  return ({ restored: "ok", restored_unverified: "warn", restore_failed: "err", not_attempted: "neutral", in_progress: "warn" } as const)[o];
}
/** Never calls anything "recovered" except a verified restoration. */
export function recoveryLabel(o: RecoveryOutcome): string {
  return ({ restored: "Restored", restored_unverified: "Restored · unverified", restore_failed: "Restore failed", not_attempted: "Restore not attempted", in_progress: "Restoring" } as const)[o];
}
export function recoveryHeadline(o: RecoveryOutcome): string {
  return ({
    restored: "The new release failed — the previous release is running again",
    restored_unverified: "The new release failed — the previous release started but is unverified",
    restore_failed: "The new release failed and the previous release could not be restored",
    not_attempted: "The new release failed and no restoration was attempted",
    in_progress: "The new release failed — restoring the previous release",
  } as const)[o];
}
export function reasonLabel(r: RestoreReason): string {
  return ({ secret_unavailable: "a pinned secret is unavailable", image_missing: "the retained image is missing", timeout: "the 60 s budget ran out", start_failed: "the release did not start", docker_unreachable: "Docker was unreachable" } as const)[r];
}
export function stepTone(s: StepStatus): Tone {
  return ({ pending: "neutral", running: "info", succeeded: "ok", failed: "err", warning: "warn", skipped: "neutral" } as const)[s];
}
export function stepLabel(name: string): string {
  return ({ source: "Source", build: "Build", preliminary_check: "Preliminary check", cutover: "Cutover", final_check: "Final check", recovery: "Recovery" } as Record<string, string>)[name] ?? name;
}

/** Best-effort exact-value redaction (no minimum length), used on any text that could echo a secret. */
export function redact(text: string, secrets: { name: string; value: string }[]): string {
  let out = text;
  for (const s of [...secrets].filter((x) => x.value.length > 0).sort((a, b) => b.value.length - a.value.length)) {
    out = out.split(s.value).join(`[REDACTED:${s.name}]`);
  }
  return out;
}
