import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { logLines, observedFrom, toDeployState, type EngineApi, type EngineSnapshot, type Observed, type OpResult, type StepLog } from "../lib/deployEngine.ts";
import type { DeployState, KeychainState, LogLine, RuntimeState } from "../lib/deployTypes.ts";

const FAST_MS = 3000;
const SLOW_MS = 20000;

/**
 * The REAL deployment state: the engine's journal (recorded evidence) joined with Docker's current state.
 * Failure to read either is surfaced, never replaced by invented data; the last good snapshot is kept and marked stale.
 */
export function useDeployEngine(realRuntime: RuntimeState, active: boolean) {
  const [snap, setSnap] = useState<EngineSnapshot | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [observed, setObserved] = useState<Observed>(null);
  const [busy, setBusy] = useState(false);
  const [last, setLast] = useState<OpResult | null>(null);
  const [keychain, setKeychain] = useState<KeychainState>("unlocked");
  const inflight = useRef(false);

  const refresh = useCallback(async () => {
    if (inflight.current) return;
    inflight.current = true;
    try {
      setSnap(await invoke<EngineSnapshot>("deploy_snapshot"));
      setLoadError(null);
    } catch (e) {
      setLoadError(String(e));
    }
    if (realRuntime === "connected") {
      try { setObserved(observedFrom(await invoke<unknown>("list_containers", { all: true }))); } catch { setObserved(null); }
    } else setObserved(null);
    inflight.current = false;
  }, [realRuntime]);

  useEffect(() => {
    void refresh();
    const t = setInterval(() => void refresh(), active ? FAST_MS : SLOW_MS);
    return () => clearInterval(t);
  }, [refresh, active]);

  const run = useCallback(async (op: Record<string, unknown>): Promise<OpResult> => {
    setBusy(true);
    let r: OpResult;
    try { r = await invoke<OpResult>("deploy_op", { op }); }
    catch (e) { r = { ok: false, exitCode: -1, summary: String(e), detail: String(e), refusedByGate: false }; }
    setLast(r);
    // The Keychain is only known to be locked when the engine said so; any successful operation clears it.
    if (r.ok) setKeychain("unlocked"); else if (/keychain.*lock|lock.*keychain/i.test(r.detail)) setKeychain("locked");
    setBusy(false);
    void refresh();
    return r;
  }, [refresh]);

  const stepLogs = useCallback(async (id: string) => logLines(await invoke<StepLog[]>("deploy_step_logs", { deployment: id })), []);
  const runtimeLogs = useCallback(async (container: string) => {
    const text = await invoke<string>("logs_container", { id: container, tail: 300 });
    return String(text).split("\n").filter((t) => t !== "").map((t): LogLine => ({ kind: "", text: t }));
  }, []);

  const stale = loadError != null && snap != null;
  const state: DeployState = useMemo(() => toDeployState(snap, { observed, keychain, loadError, stale }), [snap, observed, keychain, loadError, stale]);
  // Docker could not be asked although the VM says it is up: do not claim to know the current state.
  const runtime: RuntimeState = realRuntime === "connected" && observed === null && snap != null ? "docker-not-ready" : realRuntime;
  const api: EngineApi = useMemo(() => ({ run, refresh: () => void refresh(), busy, last, clearLast: () => setLast(null), stepLogs, runtimeLogs }), [run, refresh, busy, last, stepLogs, runtimeLogs]);
  return { state, runtime, api };
}

export type DeployEngine = ReturnType<typeof useDeployEngine>;
