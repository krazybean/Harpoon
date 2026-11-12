// How the desktop's bootstrap phase (footer, Overview "stopped" panel, overlay) follows the runtime's own status. The runtime is
// authoritative: a status that says Harpoon is running with Docker ready always wins over an earlier "stopped" or timed-out
// startup, so a stop/start (or an expansion) done from a terminal or another window reconciles by itself. Explicit in-progress phases
// started from this window and genuine failures are kept; only a confirmed status moves them.

export type BootstrapPhase = "launching" | "discovering" | "stopped" | "starting" | "vm_booting" | "docker_starting" | "ready" | "failed";

export interface LifecycleStatus {
  state: string;
  dockerReady?: boolean;
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
