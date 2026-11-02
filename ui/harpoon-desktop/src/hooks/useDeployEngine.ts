import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { HostListeners } from "../lib/portExposure.ts";
import { announceHeld, externalOpProblem, logLines, observedFrom, toDeployState, type EngineApi, type ExternalInput, type EngineSnapshot, type LastResult, type NewHeld, type Observed, type OpResult, type StepLog } from "../lib/deployEngine.ts";
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
  const [last, setLast] = useState<LastResult | null>(null);
  const [notice, setNotice] = useState<NewHeld[]>([]);
  // delivery keys already known; null until the first snapshot, which seeds it so old events are not announced
  const seen = useRef<Set<string> | null>(null);
  // runtimes attached read-only; the backend remembers the folder references (not their content) across restarts and reads each again on every refresh, never writing it
  const [externals, setExternals] = useState<ExternalInput[]>([]);
  const [externalsError, setExternalsError] = useState<string | null>(null);
  const [keychain, setKeychain] = useState<KeychainState>("unlocked");
  const inflight = useRef(false);
  // what is actually listening on this Mac (lsof), kept apart from what Docker reports; refreshed only while Deployments is showing
  const [hostListeners, setHostListeners] = useState<HostListeners>(null);
  const activeRef = useRef(active);
  activeRef.current = active;

  const refresh = useCallback(async () => {
    if (inflight.current) return;
    inflight.current = true;
    try {
      const next = await invoke<EngineSnapshot>("deploy_snapshot");
      setSnap(next);
      setLoadError(null);
      const a = announceHeld(next, seen.current);
      seen.current = a.seen;
      if (a.fresh.length) setNotice((n) => [...n, ...a.fresh]);
    } catch (e) {
      setLoadError(String(e));
    }
    // attached runtimes: an unreadable one stays listed with the reason (its reference is kept); an unreadable list is reported, never replaced
    try {
      setExternals(await invoke<ExternalInput[]>("external_runtimes"));
      setExternalsError(null);
    } catch (e) { setExternalsError(String(e)); }
    if (activeRef.current) {
      try { setHostListeners(await invoke<NonNullable<HostListeners>>("host_tcp_listeners")); } catch (e) { setHostListeners({ verified: false, error: String(e), checkedMs: Date.now(), listeners: [] }); }
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
    const refused = externalOpProblem(op);
    if (refused) {
      const r: LastResult = { ok: false, exitCode: 6, summary: refused, detail: refused, refusedByGate: false, op: String(op.op ?? "") };
      setLast(r);
      return r;
    }
    setBusy(true);
    let r: OpResult;
    try { r = await invoke<OpResult>("deploy_op", { op }); }
    catch (e) { r = { ok: false, exitCode: -1, summary: String(e), detail: String(e), refusedByGate: false }; }
    setLast({ ...r, op: String(op.op ?? "") });
    // The Keychain is only known to be locked when the engine said so; any successful operation clears it.
    if (r.ok) setKeychain("unlocked"); else if (/keychain.*lock|lock.*keychain/i.test(r.detail)) setKeychain("locked");
    setBusy(false);
    void refresh();
    return r;
  }, [refresh]);

  const attachRuntime = useCallback(async (path: string) => {
    try {
      const entry = await invoke<ExternalInput>("external_runtime_add", { path });
      setExternals((prev) => [...prev.filter((p) => p.path !== entry.path), entry]);
      return { ok: true } as const;
    } catch (e) { return { ok: false, error: String(e) } as const; }
  }, []);
  // forgets the reference only; the folder and its files are never touched
  const detachRuntime = useCallback(async (path: string) => {
    try {
      await invoke<boolean>("external_runtime_remove", { path });
      setExternals((prev) => prev.filter((p) => p.path !== path));
    } catch (e) { setExternalsError(String(e)); }
  }, []);
  const pickFolder = useCallback(async () => invoke<string | null>("deploy_pick_folder"), []);
  const clearNotice = useCallback(() => setNotice([]), []);
  const stepLogs = useCallback(async (id: string) => logLines(await invoke<StepLog[]>("deploy_step_logs", { deployment: id })), []);
  const runtimeLogs = useCallback(async (container: string) => {
    const text = await invoke<string>("logs_container", { id: container, tail: 300 });
    return String(text).split("\n").filter((t) => t !== "").map((t): LogLine => ({ kind: "", text: t }));
  }, []);

  const stale = loadError != null && snap != null;
  const state: DeployState = useMemo(() => toDeployState(snap, { observed, keychain, loadError, stale, externals, hostListeners }), [snap, observed, keychain, loadError, stale, externals, hostListeners]);
  // Docker could not be asked although the VM says it is up: do not claim to know the current state.
  const runtime: RuntimeState = realRuntime === "connected" && observed === null && snap != null ? "docker-not-ready" : realRuntime;
  const api: EngineApi = useMemo(() => ({ run, refresh: () => void refresh(), busy, last, clearLast: () => setLast(null), notice, clearNotice, pickFolder, attachRuntime, detachRuntime, externalsError, stepLogs, runtimeLogs }), [run, refresh, busy, last, notice, clearNotice, pickFolder, attachRuntime, detachRuntime, externalsError, stepLogs, runtimeLogs]);
  return { state, runtime, api };
}

export type DeployEngine = ReturnType<typeof useDeployEngine>;
