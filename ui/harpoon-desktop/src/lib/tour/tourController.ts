// A small controller around Driver.js. It owns the lifecycle (start, finish, interrupt, clean up) and the rules that
// make the walkthrough safe: the page under the spotlight is not interactive, closing is always possible, and only an
// explicit finish or dismissal is remembered. It knows nothing about React or about Deployments.
import { writeTourStatus, type KeyValueStore, type TourStatus } from "./onboardingStore.ts";
import { anchorSelector, resolveSteps, type Anchor } from "./deploymentsTour.ts";

/** The part of Driver.js this controller uses (so tests can supply a fake). */
export type DriverLike = {
  drive(index?: number): void;
  moveNext(): void;
  hasNextStep(): boolean;
  destroy(): void;
  isActive(): boolean;
};
export type DriverFactory = (config: Record<string, unknown>) => DriverLike;
export type LoadDriver = () => Promise<DriverFactory>;

export const loadRealDriver: LoadDriver = async () => (await import("driver.js")).driver as unknown as DriverFactory;

export type TourHandle = { destroy(): void; readonly active: boolean };

type StartOptions = {
  doc: Document;
  store: KeyValueStore;
  load?: LoadDriver;
  /** Defaults to the user's `prefers-reduced-motion` setting. */
  reducedMotion?: boolean;
  /** Called once when the person finishes or closes the tour (never for an interruption). */
  onEnd?: (status: TourStatus) => void;
  /** Called after any end, including an interruption: cleanup of the owner's reference. */
  onGone?: () => void;
};

const TOUR_CLASS = "harpoon-tour";

export function prefersReducedMotion(doc: Document): boolean {
  try { return !!doc.defaultView?.matchMedia?.("(prefers-reduced-motion: reduce)").matches; } catch { return false; }
}

/**
 * Start the walkthrough against what is on screen now. Resolves to a handle, or null if the tour could not start
 * (Driver.js failed to load, or the page went away) — in which case nothing was changed and nothing is remembered.
 */
export async function startDeploymentsTour(o: StartOptions): Promise<TourHandle | null> {
  const { doc } = o;
  const query = (a: Anchor) => doc.querySelector<HTMLElement>(anchorSelector(a));
  let factory: DriverFactory;
  try { factory = await (o.load ?? loadRealDriver)(); } catch { return null; }
  const steps = resolveSteps((a) => !!query(a));
  const reduced = o.reducedMotion ?? prefersReducedMotion(doc);
  const opener = doc.activeElement as HTMLElement | null;

  let ended = false;
  let drv: DriverLike | null = null;
  const end = (status: TourStatus | null) => {
    if (ended) return;
    ended = true;
    if (status) { writeTourStatus(o.store, status); o.onEnd?.(status); }
    try { if (drv?.isActive()) drv.destroy(); } catch { /* already torn down */ }
    // Give focus back to where the person was; if that is gone, to the replay action.
    const back = opener && opener.isConnected ? opener : query("replay-tour");
    try { back?.focus?.({ preventScroll: true }); } catch { /* not focusable */ }
    o.onGone?.();
  };

  drv = factory({
    steps: steps.map((s) => ({
      // Resolved when the step is shown, so a re-render that replaced the node is still found; a missing one
      // becomes Driver's centred, un-highlighted popover.
      element: s.anchor ? (() => query(s.anchor as Anchor)) : undefined,
      popover: { title: s.title, description: s.text },
    })),
    animate: !reduced,
    smoothScroll: !reduced,
    allowClose: true,
    allowKeyboardControl: true,
    overlayClickBehavior: "none", // a stray click must not throw the walkthrough away
    disableActiveInteraction: true, // the highlighted button can be looked at, never pressed
    stagePadding: 6,
    stageRadius: 10,
    overlayColor: "#05070a",
    overlayOpacity: 0.62,
    popoverClass: TOUR_CLASS,
    showProgress: true,
    progressText: "{{current}} of {{total}}",
    nextBtnText: "Next",
    prevBtnText: "Back",
    doneBtnText: "Done",
    onPopoverRender: (popover: { footer: HTMLElement; closeButton: HTMLElement }) => {
      popover.closeButton.setAttribute("aria-label", "Close walkthrough");
      if (popover.footer.querySelector(".harpoon-tour-skip")) return;
      const skip = doc.createElement("button");
      skip.type = "button";
      skip.className = "harpoon-tour-skip";
      skip.textContent = "Skip tour";
      skip.addEventListener("click", () => end("dismissed"));
      popover.footer.prepend(skip);
    },
    onNextClick: () => { if (drv && drv.hasNextStep()) drv.moveNext(); else end("completed"); },
    onCloseClick: () => end("dismissed"),
    // Reached for any other way out of Driver (its own Escape handling): that is the person closing it.
    onDestroyStarted: () => end("dismissed"),
  });
  try { drv.drive(); } catch { end(null); return null; }
  return {
    destroy: () => end(null),
    get active() { return !ended; },
  };
}
