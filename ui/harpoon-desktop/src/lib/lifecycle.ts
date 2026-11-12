// How the desktop's bootstrap phase (footer, Overview "stopped" panel, overlay) follows the runtime's own status. The runtime is
// authoritative: a status that says Harpoon is running with Docker ready always wins over an earlier "stopped" or timed-out
// startup, so a stop/start (or an expansion) done from a terminal or another window reconciles by itself. Explicit in-progress phases
// started from this window and genuine failures are kept; only a confirmed status moves them.

export type BootstrapPhase = "launching" | "discovering" | "stopped" | "starting" | "vm_booting" | "docker_starting" | "ready" | "failed";

export interface LifecycleStatus {
  state: string;
  dockerReady?: boolean;
  mgmtReady?: boolean;
  failure?: unknown;
  diskExpansion?: { active?: boolean } | null;
}

export interface ReconcileContext { binaryPath: boolean; statusError: boolean }

const TRANSIENT: BootstrapPhase[] = ["discovering", "launching", "starting", "vm_booting", "docker_starting"];

/** The phase the UI should be in given a fresh status, or null to leave it alone. Pure; the caller applies it. */
export function reconcilePhase(phase: BootstrapPhase, status: LifecycleStatus | null | undefined, ctx: ReconcileContext): BootstrapPhase | null {
  if (!status) return null;
  const s = status.state;
  if (s === "running" && status.dockerReady) {
    if (!ctx.binaryPath) return null;
    // "stopped" and "failed" are not terminal: a start done outside this window, or a startup that was slow but succeeded, ends here.
    if (phase === "failed") return ctx.statusError || status.failure ? null : "ready";
    return phase === "ready" ? null : "ready";
  }
  // a start made elsewhere is followed through its stages; "ready" and "failed" are only changed by the cases above and below
  const following = phase === "stopped" || phase === "starting" || phase === "vm_booting" || phase === "docker_starting";
  if (s === "running") return following ? "docker_starting" : null;   // running, Docker still coming up
  if (s === "starting") return phase === "stopped" ? "starting" : null;
  if (s === "booting") return phase === "stopped" || phase === "starting" ? "vm_booting" : null;
  if (s === "stopped" || s === "stale") return phase === "ready" || TRANSIENT.includes(phase) ? "stopped" : null;
  if (s === "failed") return phase === "failed" ? null : "failed";
  return null;
}

/** Whether the "Harpoon is stopped / Start Harpoon" panel may be shown. Never against a runtime that reports itself running or starting. */
export function stoppedPanelVisible(phase: BootstrapPhase, status: LifecycleStatus | null | undefined): boolean {
  if (phase !== "stopped") return false;
  if (status && ["running", "starting", "booting", "stopping"].includes(status.state)) return false;
  return !status?.diskExpansion?.active;
}

/** Start is offered only when nothing is running, in transition, or being expanded. */
export function startAvailable(status: LifecycleStatus | null | undefined): boolean {
  if (!status) return true;
  return !["running", "starting", "booting", "stopping"].includes(status.state) && !status.diskExpansion?.active;
}

// --- startup that cannot hang, and degraded management ------------------------------------------------------------------------------

/** How long a running, Docker-ready runtime may sit behind the startup overlay before the overlay gives way. */
export const OVERLAY_GRACE_MS = 10_000;
/** Management channel down this long (while Docker is ready) before a warning is shown; a booting guest answers Docker first. */
export const MGMT_WARNING_AFTER_MS = 20_000;

/** Resolves with the promise's value, or `undefined` when it has not settled in `ms`. Never rejects. */
export function withTimeout<T>(p: Promise<T>, ms: number): Promise<T | undefined> {
  return new Promise((resolve) => {
    const t = setTimeout(() => resolve(undefined), ms);
    p.then((v) => { clearTimeout(t); resolve(v); }, () => { clearTimeout(t); resolve(undefined); });
  });
}

/** The startup overlay may be dropped: a step of the bootstrap sequence is stuck but the runtime itself says running with Docker ready. */
export function overlayShouldYield(phase: BootstrapPhase, status: LifecycleStatus | null | undefined, actionInProgress: boolean): boolean {
  return !actionInProgress && TRANSIENT.includes(phase) && status?.state === "running" && Boolean(status.dockerReady);
}

/** Docker works but the guest management channel (exec, shell, filesystem stats, reclaim) does not, for long enough to be worth saying. */
export function degradedManagement(status: LifecycleStatus | null | undefined, downSince: number | null, now: number): boolean {
  return status?.state === "running" && Boolean(status.dockerReady) && status.mgmtReady === false && downSince !== null && now - downSince >= MGMT_WARNING_AFTER_MS;
}
