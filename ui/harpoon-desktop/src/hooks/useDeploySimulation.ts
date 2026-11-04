import { useCallback, useEffect, useReducer, useState } from "react";
import { initialState, reduce } from "../lib/deployStore.ts";
import type { DeployAction, DeployState, RuntimeState, ScenarioId } from "../lib/deployTypes.ts";

const TICK_MS = 900;

/** Owns the SIMULATED deployment state. Nothing here touches Docker, Harpoon, or the Keychain. */
export function useDeploySimulation(initial?: { scenario?: ScenarioId }) {
  const [state, rawDispatch] = useReducer(reduce, initial?.scenario ?? "healthy", (s: ScenarioId) => initialState(s));
  const [autoRun, setAutoRun] = useState(false);
  const [runtimeOverride, setRuntimeOverride] = useState<RuntimeState | null>(null);

  // User-started simulated deployments advance on their own; seeded scenarios hold still until stepped.
  useEffect(() => {
    if (!autoRun) return;
    if (!state.job) { setAutoRun(false); return; }
    const t = setTimeout(() => rawDispatch({ type: "advance" }), TICK_MS);
    return () => clearTimeout(t);
  }, [autoRun, state]);

  const dispatch = useCallback((a: DeployAction, run = false) => {
    if (a.type === "set-scenario") setAutoRun(false);
    rawDispatch(a);
    if (run) setAutoRun(true);
  }, []);

  return { state: state as DeployState, dispatch, autoRun, setAutoRun, runtimeOverride, setRuntimeOverride };
}

export type DeploySimulation = ReturnType<typeof useDeploySimulation>;
