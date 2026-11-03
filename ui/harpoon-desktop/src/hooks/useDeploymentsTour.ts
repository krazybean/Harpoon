import { useCallback, useEffect, useRef } from "react";
import { browserStore, readTourStatus, type KeyValueStore } from "../lib/tour/onboardingStore.ts";
import { startDeploymentsTour, type LoadDriver, type TourHandle } from "../lib/tour/tourController.ts";
import { anchorSelector } from "../lib/tour/deploymentsTour.ts";

export type Onboarding = { store?: KeyValueStore; load?: LoadDriver };

type Args = {
  /** Start by itself on the first meaningful visit. False in simulation/preview and while anything is loading, open, or running. */
  auto: boolean;
  /** Changes whenever the person moves to another screen: a running tour ends rather than point at something that left. */
  routeKey: string;
  onboarding?: Onboarding;
};

const FRAMES_TO_WAIT_FOR_TARGETS = 30;

/** Returns `replay`, which starts the walkthrough on demand whatever was remembered. */
export function useDeploymentsTour({ auto, routeKey, onboarding }: Args): () => void {
  const tour = useRef<TourHandle | null>(null);
  const epoch = useRef(0); // bumped by every stop, so a tour that was still loading when the screen changed is dropped
  const autoTried = useRef(false);
  const fallback = useRef<KeyValueStore | null>(null);
  const store = onboarding?.store ?? (fallback.current ??= browserStore());
  const load = onboarding?.load;

  const stop = useCallback(() => { epoch.current++; tour.current?.destroy(); tour.current = null; }, []);
  const begin = useCallback(async () => {
    if (tour.current?.active) return;
    const mine = epoch.current;
    const h = await startDeploymentsTour({ doc: document, store, load, onGone: () => { tour.current = null; } });
    if (!h) return;
    if (mine !== epoch.current) { h.destroy(); return; }
    tour.current = h;
  }, [store, load]);

  // A new screen, or leaving Deployments, ends a running tour. It is not remembered: it was interrupted, not finished.
  useEffect(() => stop, [routeKey, stop]);

  // First meaningful visit: wait (briefly) for the anchors to be on screen, then start once per visit.
  useEffect(() => {
    if (!auto || autoTried.current || readTourStatus(store) !== null) return;
    let frames = 0, raf = 0, cancelled = false;
    const tick = () => {
      if (cancelled) return;
      if (document.querySelector(anchorSelector("create-project")) || document.querySelector(anchorSelector("app-status"))) { autoTried.current = true; void begin(); return; }
      if (++frames < FRAMES_TO_WAIT_FOR_TARGETS) raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => { cancelled = true; cancelAnimationFrame(raf); };
  }, [auto, routeKey, store, begin]);

  return useCallback(() => { stop(); void begin(); }, [stop, begin]);
}
