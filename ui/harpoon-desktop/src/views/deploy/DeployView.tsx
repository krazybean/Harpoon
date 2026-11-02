import { useCallback, useEffect, useMemo, useState } from "react";
import "./deploy.css";
import type { DeploySimulation } from "../../hooks/useDeploySimulation.ts";
import type { DeployEngine } from "../../hooks/useDeployEngine.ts";
import type { KeychainState, NextOutcome, RuntimeState, ScenarioId } from "../../lib/deployTypes.ts";
import { SIMULATION_NOTE } from "../../lib/deployPresentation.ts";
import { SCENARIOS, type DeployPreview } from "../../lib/deployPreview.ts";
import { Banner } from "./parts.tsx";
import { dialogAllowed, type Ctx, type DialogState, type Route } from "./context.ts";
import { AppScreen, DeploymentScreen, HomeScreen } from "./Screens.tsx";
import { DeployDialogs } from "./Dialogs.tsx";
import { Play } from "@phosphor-icons/react";

const SCENARIO_LABELS: Record<ScenarioId, string> = {
  healthy: "Healthy", building: "Building (live)", "build-failed": "Build failed", "cutover-restored": "Cutover failed · restored · paused",
  "restore-failed": "Cutover failed · restore failed · down", recovering: "Recovering now", "rolled-back-paused": "Rolled back · paused", empty: "Empty", loading: "Loading",
};
const OUTCOME_LABELS: Record<NextOutcome, string> = { success: "Succeeds", "build-failure": "Build fails", "cutover-restored": "Cutover fails · restored", "restore-failed": "Cutover fails · restore fails" };

// Deep links are applied once per page load, not on every remount of this view.
let previewApplied = false;

const NOOP = () => {};

type Props = {
  /** Explicit simulation/preview mode (development builds only). Mutually exclusive with `engine`. */
  sim?: DeploySimulation;
  /** The real engine. Production always uses this. */
  engine?: DeployEngine;
  /** What Harpoon really reports. The views believe this unless a simulation override is set. */
  realRuntime: RuntimeState;
  onStartHarpoon: () => void;
  onOpenContainers?: () => void;
  preview?: DeployPreview;
};

export function DeployView({ sim, engine, realRuntime, onStartHarpoon, onOpenContainers, preview: previewProp }: Props) {
  const preview = previewApplied ? undefined : previewProp;
  const state = engine ? engine.state : sim!.state;
  const dispatch = sim ? sim.dispatch : NOOP;
  const runtimeOverride = sim ? sim.runtimeOverride : null;
  const setRuntimeOverride = sim ? sim.setRuntimeOverride : NOOP;
  const runtime = engine ? engine.runtime : runtimeOverride ?? realRuntime;
  const firstApp = Object.keys(state.apps)[0];
  const [route, setRouteState] = useState<Route>({ screen: preview?.screen ?? "home", appId: preview?.app ?? (engine ? firstApp ?? "" : "api"), env: preview?.env ?? "production", tab: preview?.tab ?? "overview", depId: preview?.dep ?? null });
  const [dialog, setDialog] = useState<DialogState | null>(null);
  const setRoute = useCallback((p: Partial<Route>) => setRouteState((r) => ({ ...r, ...p })), []);

  // A scenario change must never strand the user on a screen for something that no longer exists.
  useEffect(() => {
    setDialog(null);
    if (route.screen !== "home" && !state.apps[route.appId]) setRouteState((r) => ({ ...r, screen: "home" }));
    if (route.screen === "dep" && state.apps[route.appId] && !state.apps[route.appId].deployments.some((d) => d.id === route.depId)) setRouteState((r) => ({ ...r, depId: state.apps[route.appId].deployments[0]?.id ?? null }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.scenario, engine ? Object.keys(state.apps).join(",") : ""]);

  // Deep-link state for review builds only.
  useEffect(() => {
    if (!preview) return;
    previewApplied = true;
    if (preview.scenario) dispatch({ type: "set-scenario", scenario: preview.scenario });
    if (preview.keychain) dispatch({ type: "set-keychain", keychain: preview.keychain });
    if (preview.outcome) dispatch({ type: "set-next-outcome", outcome: preview.outcome });
    if (preview.runtime) setRuntimeOverride(preview.runtime);
    if (preview.dialog) {
      const d = preview.dialog;
      setDialog(d === "create" ? { kind: "create", step: 1, src: "git" } : d === "rollback" ? { kind: "rollback", releaseId: "r-41" } : d === "variable" ? { kind: "variable" } : { kind: d });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const startRuntime = useCallback(() => {
    if (runtimeOverride) setRuntimeOverride(null); // end the simulated outage; the real runtime is untouched
    else onStartHarpoon();
  }, [runtimeOverride, onStartHarpoon, setRuntimeOverride]);

  // Production-changing dialogs are refused outside the Production route, even if a button is missed.
  const guardedOpen = useCallback((d: DialogState | null) => { if (d && !dialogAllowed(d, route.env)) return; setDialog(d); }, [route.env]);
  const ctx: Ctx = useMemo(() => ({ state, runtime, runtimeSource: runtimeOverride ? "simulated" : "harpoon", engine: engine?.api, openContainers: onOpenContainers, route, setRoute, dispatch, open: guardedOpen, startRuntime }),
    [state, runtime, runtimeOverride, engine?.api, onOpenContainers, route, setRoute, dispatch, startRuntime, guardedOpen]);
  const meta = state.engine;
  const last = engine?.api.last ?? null;

  const showBanner = runtime !== "connected";
  return (
    <div className="dp-root" data-testid="deployments-view" style={preview?.width ? { width: preview.width } : undefined}>
      {sim ? <>
      <div className="dp-sim" role="group" aria-label="Simulation controls — not part of the product">
        <b>Simulation</b>
        <label>Scenario <select value={state.scenario} onChange={(e) => dispatch({ type: "set-scenario", scenario: e.target.value as ScenarioId })}>{SCENARIOS.map((s) => <option key={s} value={s}>{SCENARIO_LABELS[s]}</option>)}</select></label>
        <label>Next deployment <select value={state.nextOutcome} onChange={(e) => dispatch({ type: "set-next-outcome", outcome: e.target.value as NextOutcome })}>{(Object.keys(OUTCOME_LABELS) as NextOutcome[]).map((o) => <option key={o} value={o}>{OUTCOME_LABELS[o]}</option>)}</select></label>
        <label>Runtime <select value={runtimeOverride ?? "real"} onChange={(e) => setRuntimeOverride(e.target.value === "real" ? null : (e.target.value as RuntimeState))}>
          <option value="real">Follow Harpoon ({realRuntime === "connected" ? "connected" : realRuntime === "stopped" ? "stopped" : "not ready"})</option><option value="connected">Simulate connected</option><option value="stopped">Simulate stopped</option><option value="docker-not-ready">Simulate Docker not ready</option></select></label>
        <label>Keychain <select value={state.keychain} onChange={(e) => dispatch({ type: "set-keychain", keychain: e.target.value as KeychainState })}><option value="unlocked">Unlocked</option><option value="locked">Locked</option></select></label>
        {state.job && !sim.autoRun && <button className="btn btn--secondary btn--sm" onClick={() => dispatch({ type: "advance" })}>Step ▸</button>}
        {state.job && !sim.autoRun && <button className="btn btn--secondary btn--sm" onClick={() => sim.setAutoRun(true)}><Play size={12} /> Run</button>}
        <button className="btn btn--ghost btn--sm" onClick={() => { setRuntimeOverride(null); dispatch({ type: "set-scenario", scenario: "healthy" }); setRouteState({ screen: "home", appId: "api", env: "production", tab: "overview", depId: null }); }}>Reset</button>
      </div>
      <div className="dp-sim-note">{SIMULATION_NOTE}</div>
      </> : null}
      {meta?.store === "error" && <Banner tone="err" title="Harpoon cannot read its deployment records">{meta.storeError} Nothing is shown rather than guessing. The records were not modified.</Banner>}
      {meta?.loadError && <Banner tone="warn" title={meta.stale ? "Showing the last data Harpoon could read" : "Harpoon could not read deployment state"}
        actions={<button className="btn btn--secondary" onClick={() => engine?.api.refresh()}>Retry</button>}>{meta.loadError}{meta.stale ? ` Last read ${new Date(meta.generatedMs).toLocaleTimeString()}; what follows may be out of date.` : ""}</Banner>}
      {meta && meta.store !== "error" && (!meta.gate.liveDeploy || !meta.gate.liveReplacement) && <Banner tone="info" title="Live deployment is closed in this build">{meta.gate.reason} History, releases, configuration and logs are real and readable; configuration and repository changes are recorded.</Banner>}
      {meta?.writerHeld && <Banner tone="info" title="Harpoon is working on deployment records right now">A deployment or another command holds the records. Items shown as running may be genuinely in progress.</Banner>}
      {last && <Banner tone={last.ok ? "ok" : "err"} title={last.ok ? "Done" : last.refusedByGate ? "Refused by the shipping gate" : "That did not work"}
        actions={<button className="btn btn--ghost" onClick={() => engine?.api.clearLast()}>Dismiss</button>}>{last.summary || (last.ok ? "The engine finished." : "The engine reported an error.")}</Banner>}
      {showBanner && (
        <Banner tone="warn" title={runtime === "stopped" ? "Harpoon is stopped" : "Docker is not ready"}
          actions={<button className="btn btn--primary" onClick={startRuntime}><Play size={14} /> {runtimeOverride ? "End simulated outage" : "Start Harpoon"}</button>}>
          {runtime === "stopped"
            ? "Deployment history, logs and configuration are still readable. Building, deploying, rolling back and recovering need the runtime. Nothing is deployed or changed while it is stopped."
            : "The VM is up but Docker has not answered yet. Actions that touch Docker are disabled until it is ready."}{runtimeOverride ? " (Simulated outage — your real Harpoon is unaffected.)" : ""}
        </Banner>
      )}
      {route.screen === "home" ? <HomeScreen c={ctx} /> : route.screen === "dep" ? <DeploymentScreen c={ctx} /> : <AppScreen c={ctx} />}
      {dialog && dialogAllowed(dialog, route.env) ? <DeployDialogs c={ctx} dialog={dialog} /> : null}
    </div>
  );
}
