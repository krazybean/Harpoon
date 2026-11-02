import { Component, lazy, Suspense, useEffect, useState, useCallback, useRef, memo } from "react";
import type { ErrorInfo, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./theme.css";
import "./styles/primitives.css";
import { House, Stack, Images, Database, ShareNetwork, Sliders, Heartbeat, Gear, CaretDown, Play, Stop, ArrowClockwise, RocketLaunch } from "@phosphor-icons/react";
import { useShelf } from "./hooks/useShelf";
import { OverviewView } from "./views/OverviewView";
import { ContainersView } from "./views/ContainersView";
import { ImagesView } from "./views/ImagesView";
import { VolumesView } from "./views/VolumesView";
import { NetworksView } from "./views/NetworksView";
import { ResourcesView } from "./views/ResourcesView";
import { DiagnosticsView } from "./views/DiagnosticsView";
import { ContainerDetailView } from "./views/ContainerDetailView";
import { ContextsView } from "./views/ContextsView";
import { ResourceMeters } from "./components/ResourceMeters";
import { TechnologyIconShowcase } from "./components/TechnologyIconShowcase";
import { QuakeShellPrototype } from "./components/QuakeShellPrototype";
import { configureResourceSampler } from "./lib/resourceSamplerStore";
import type { Destination, Nav } from "./lib/navigation";
import { containerDestination } from "./lib/navigation";
import { ConfirmDialogHost, confirmAction } from "./components/ConfirmDialog";
import { useDeploySimulation } from "./hooks/useDeploySimulation";
import { useDeployEngine } from "./hooks/useDeployEngine";
import { navBadge } from "./lib/deployStore";
import { parseDeployHash } from "./lib/deployPreview";
import type { RuntimeState } from "./lib/deployTypes";

type HarpoonStatus = {
  state: string;
  pid?: number;
  cpus?: number;
  memoryMiB?: number;
  diskPath?: string;
  diskLogicalBytes?: number;
  socketPath?: string;
  sockExists?: boolean;
  lockHeld?: boolean;
  lockPath?: string;
  logPath?: string;
  dockerReady?: boolean;
  failure?: RuntimeFailure;
};
type RuntimeFailure = { category: string; message: string; source: string; occurredAt: string; underlyingError?: string | null };
type DoctorResult = { raw: string; passed: number; warnings: number; failures: number };
type ConfigResult = { cpus: number; memory: number; raw: string; path: string };
type Counts = { containers: number; running: number; images: number; volumes: number; networks: number };
type DiscoveredContext = { id: string; name: string; engine: string; endpoint?: string | null; source: string; state: "detected" | "connected" | "unavailable" | "authentication_needed"; current: boolean; harpoon: boolean; detail: string; capabilities: Record<string, "available" | "unsupported" | "unknown"> };
type BootstrapPhase = "launching" | "discovering" | "stopped" | "starting" | "vm_booting" | "docker_starting" | "ready" | "failed";
type RuntimeRefreshEvent = { key: string; ok: boolean; generation?: number; value?: unknown; error?: string; values?: RuntimeRefreshEvent[] };
type BootstrapEvent = { status?: HarpoonStatus; observedAtMs?: number; ok?: boolean; error?: string | null };
// Deployments shows the real deployment engine (a simulation only in an explicit development preview); it is code-split so the
// existing screens do not pay for it.
const DeployView = lazy(() => import("./views/deploy/DeployView").then((module) => ({ default: module.DeployView })));
const DevTopologyPrototype = import.meta.env.DEV ? lazy(() => import("./components/TopologyPrototype").then((module) => ({ default: module.TopologyPrototype }))) : null;

function stateBadge(state: string) {
  const s = state.toLowerCase();
  if (s === "running") return { bg: "var(--success-bg)", fg: "var(--success-fg)", label: "RUNNING", cls: "badge--running" };
  if (s === "starting" || s === "booting") return { bg: "var(--warning-bg)", fg: "var(--warning-fg)", label: state.toUpperCase(), cls: "badge--warning" };
  if (s === "stopped") return { bg: "var(--panel-header)", fg: "var(--text-secondary)", label: "STOPPED", cls: "badge--stopped" };
  if (s === "failed") return { bg: "var(--error-bg)", fg: "var(--error-fg)", label: "FAILED", cls: "badge--error" };
  if (s === "stale") return { bg: "var(--warning-bg)", fg: "var(--warning-fg)", label: "STALE", cls: "badge--warning" };
  return { bg: "var(--info-bg)", fg: "var(--info-fg)", label: state.toUpperCase(), cls: "badge--info" };
}
function BootstrapStatus({ message, trace }: { message: string; trace: boolean }) {
  const [current, setCurrent] = useState(message);
  const [leaving, setLeaving] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    if (message === current) return;
    if (trace) console.debug("[bootstrap-trace] render", { message, atMs: Date.now() });
    if (timer.current !== null) window.clearTimeout(timer.current);
    setLeaving(current);
    setCurrent(message);
    timer.current = window.setTimeout(() => {
      setLeaving(null);
      timer.current = null;
    }, 360);
  }, [message]);

  useEffect(() => () => {
    if (timer.current !== null) window.clearTimeout(timer.current);
  }, []);

  return <div className="bootstrap-status" role="status" aria-live="polite" aria-atomic="true">
    {leaving && <span key={`leaving-${leaving}`} onAnimationStart={trace ? () => console.debug("[bootstrap-trace] animation-start", { message: leaving, atMs: Date.now() }) : undefined} className="bootstrap-status__message bootstrap-status__message--leaving">{leaving}</span>}
    <span key={`current-${current}`} onAnimationStart={trace ? () => console.debug("[bootstrap-trace] animation-start", { message: current, atMs: Date.now() }) : undefined} className="bootstrap-status__message bootstrap-status__message--current">{current}</span>
  </div>;
}
function fmtTs(ts: number | null) {
  if (!ts) return "—";
  return new Date(ts).toLocaleTimeString();
}

function EngineManageMenu({ state, disabled, onAction }: { state: string; disabled: boolean; onAction: (action: "start" | "stop" | "restart") => void }) {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => { if (!menuRef.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { setOpen(false); (menuRef.current?.querySelector("button") as HTMLButtonElement | null)?.focus(); } };
    document.addEventListener("mousedown", close); document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("mousedown", close); document.removeEventListener("keydown", escape); };
  }, [open]);
  const running = state === "running" || state === "starting" || state === "booting";
  const choose = async (action: "start" | "stop" | "restart") => {
    setOpen(false);
    if (action !== "start" && !(await confirmAction(`${action === "stop" ? "Stop" : "Restart"} Harpoon Engine? This affects the VM, Docker Engine, and workloads running in the Harpoon context.`, action === "stop" ? "Stop engine" : "Restart engine"))) return;
    onAction(action);
  };
  return <div className="engine-manage" ref={menuRef}>
    <button className="btn btn--secondary engine-manage__trigger" aria-haspopup="menu" aria-expanded={open} disabled={disabled} onClick={() => setOpen(!open)}><span className="engine-manage__label">Harpoon Engine · {state || "unknown"}</span><span className="engine-manage__manage">Manage</span><CaretDown size={14} aria-hidden="true" /></button>
    {open && <div className="engine-manage__menu" role="menu" aria-label="Harpoon Engine actions">
      <div className="engine-manage__scope">Scope: VM + Docker Engine + Harpoon workloads</div>
      <button role="menuitem" disabled={running} onClick={() => choose("start")}><Play size={14} aria-hidden="true" /> Start Engine</button>
      <button role="menuitem" disabled={!running} onClick={() => choose("stop")}><Stop size={14} aria-hidden="true" /> Stop Engine</button>
      <button role="menuitem" disabled={!running} onClick={() => choose("restart")}><ArrowClockwise size={14} aria-hidden="true" /> Restart Engine</button>
    </div>}
  </div>;
}

const NavItems = memo(function NavItems({ active, onNav, deployBadge }: { active: Nav; onNav: (n: Nav)=>void; deployBadge: "none" | "attention" | "error" }) {
  return (
    <>
      {([
        ["overview", House, "Overview"],
        ["containers", Stack, "Containers"],
        ["deployments", RocketLaunch, "Deployments"],
        ["images", Images, "Images"],
        ["volumes", Database, "Volumes"],
        ["networks", ShareNetwork, "Networks"],
        ["resources", Sliders, "Resources"],
        ["diagnostics", Heartbeat, "Diagnostics"],
        ["contexts", Gear, "Contexts"],
      ] as const).map(([n, Icon, label]) => (
        <button key={n} aria-current={active===n ? "page" : undefined} aria-label={label} title={label} onClick={() => onNav(n as Nav)} className={active===n ? "shell-nav__item shell-nav__item--active" : "shell-nav__item"}>
          <Icon size={20} weight={active===n ? "fill" : "regular"} className="shell-nav__icon" aria-hidden="true" />
          <span className="shell-nav__label">{label}</span>
          {n === "deployments" && deployBadge !== "none" ? <span role="img" aria-label={deployBadge === "error" ? "a deployment needs attention (error)" : "a deployment needs attention"} title="A deployment needs attention" style={{ marginLeft: "auto", flexShrink: 0, width: 8, height: 8, borderRadius: "50%", background: active === "deployments" ? "var(--nav-selected-text)" : deployBadge === "error" ? "var(--error)" : "var(--warning)" }} /> : null}
        </button>
      ))}
    </>
  );
});
const NavFooter = memo(function NavFooter({ binaryPath, statusAt }: { binaryPath: string; statusAt: number | null }) {
  return (
    <div style={{ marginTop: "auto", fontSize: 9, color: "var(--text-tertiary)", padding: "8px 4px", textAlign: "center", lineHeight: 1.3 }}>
      <div style={{ fontWeight: 600, color: "var(--text-secondary)", fontSize: 9 }}>{binaryPath ? binaryPath.split("/").pop() : "resolving…"}</div>
      <span style={{ fontSize: 8, color: "var(--text-tertiary)" }}>{fmtTs(statusAt)}</span>
    </div>
  );
});
const ShellNav = memo(function ShellNav({ active, onNav, binaryPath, statusAt, deployBadge }: { active: Nav; onNav: (n: Nav)=>void; binaryPath: string; statusAt: number | null; deployBadge: "none" | "attention" | "error" }) {
  return (
    <nav className="shell-nav" aria-label="Primary">
      <div className="text-title" style={{ padding: "4px 0 8px 0", textAlign: "center", fontSize: 12, letterSpacing: "-0.02em" }}>Harpoon</div>
      <NavItems active={active} onNav={onNav} deployBadge={deployBadge} />
      <NavFooter binaryPath={binaryPath} statusAt={statusAt} />
    </nav>
  );
});

class ContentErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Harpoon content render failed", error, info.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="panel content-error" role="alert">
        <strong>Harpoon could not render this view.</strong>
        <span>{this.state.error.message}</span>
        <button className="btn btn--secondary btn--sm" onClick={() => this.setState({ error: null })}>Try again</button>
      </div>
    );
  }
}

export default function App() {
  const tauriAvailable = typeof window !== "undefined" && Boolean((window as any).__TAURI_INTERNALS__);
  const technologyShowcase = import.meta.env.DEV && typeof window !== "undefined" && new URLSearchParams(window.location.search).has("technology-showcase");
  const topologyPrototype = import.meta.env.DEV && typeof window !== "undefined" && new URLSearchParams(window.location.search).has("topology-prototype");
  const bootstrapEventPush = import.meta.env.DEV && typeof window !== "undefined" && new URLSearchParams(window.location.search).has("bootstrap-events");
  const bootstrapTrace = import.meta.env.DEV && typeof window !== "undefined" && new URLSearchParams(window.location.search).has("bootstrap-trace");
  // Review-only deep link into simulated Deployments states (development builds only).
  const deployPreview = import.meta.env.DEV && typeof window !== "undefined" ? parseDeployHash(window.location.hash) : undefined;
  const deployPreviewActive = Boolean(deployPreview && Object.values(deployPreview).some((v) => v !== undefined));
  const deploySim = useDeploySimulation({ scenario: deployPreview?.scenario });
  const [destination, setDestination] = useState<Destination>({ kind: "nav", nav: deployPreviewActive ? "deployments" : "overview" });
  const active: Nav = destination.kind === "container" ? "containers" : destination.nav;
  const [actionInProgress, setActionInProgress] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [bootstrapPhase, setBootstrapPhase] = useState<BootstrapPhase>("launching");
  const [bootstrapStatus, setBootstrapStatus] = useState("Resolving Harpoon binary…");
  const [bootstrapError, setBootstrapError] = useState<string | null>(null);
  const [rustRefreshFallback, setRustRefreshFallback] = useState(false);
  const bootstrapAttempted = useRef(false);
  const bootstrapActiveRef = useRef(false);
  const bootstrapEventReadyRef = useRef<Promise<void> | null>(null);
  const bootstrapEventCleanupRef = useRef<(() => void)[]>([]);

  // Shelves — smallest shared in-memory read model, no Redux/Zustand
  const statusShelf = useShelf<HarpoonStatus>(useCallback(()=>invoke<HarpoonStatus>("get_status"), []), { ttlMs: 30000 });
  const dockerShelf = useShelf<string>(useCallback(()=>invoke<string>("get_docker_info"), []), { ttlMs: 30000 });
  const countsShelf = useShelf<Counts>(useCallback(()=>invoke<Counts>("get_counts"), []), { ttlMs: 30000 });
  const configShelf = useShelf<ConfigResult>(useCallback(()=>invoke<ConfigResult>("get_config"), []), { ttlMs: 30000 });
  const binaryShelf = useShelf<string>(useCallback(()=>invoke<string>("get_harpoon_binary_path"), []), { ttlMs: 60000 });
  const logPathShelf = useShelf<string>(useCallback(()=>invoke<string>("get_log_path"), []), { ttlMs: 60000 });
  const doctorShelf = useShelf<DoctorResult>(useCallback(()=>invoke<DoctorResult>("get_doctor"), []), { ttlMs: 60000 });
  const logsShelf = useShelf<string>(useCallback(()=>invoke<string>("get_recent_logs", { lines: 120 }), []), { ttlMs: 15000 });
  const containersShelf = useShelf<any[]>(useCallback(()=>invoke<any[]>("list_containers", { all: true }), []), { ttlMs: 30000 });
  const imagesShelf = useShelf<any[]>(useCallback(()=>invoke<any[]>("list_images"), []), { ttlMs: 30000 });
  const volumesShelf = useShelf<any[]>(useCallback(()=>invoke<any[]>("list_volumes"), []), { ttlMs: 30000 });
  const networksShelf = useShelf<any[]>(useCallback(()=>invoke<any[]>("list_networks"), []), { ttlMs: 30000 });
  const contextsShelf = useShelf<DiscoveredContext[]>(useCallback(()=>invoke<DiscoveredContext[]>("discover_contexts"), []), { ttlMs: 300000 });

  const status = statusShelf.data;
  // What Harpoon REALLY reports; the Deployments views believe this unless a simulation override is set.
  const realRuntime: RuntimeState = status?.state === "running" ? (status.dockerReady ? "connected" : "docker-not-ready") : status?.state === "starting" || status?.state === "booting" ? "docker-not-ready" : "stopped";
  // Production always shows the REAL engine's state; the seeded simulation is reachable only through an explicit development preview link.
  const deployEngine = useDeployEngine(realRuntime, active === "deployments");
  const deployBadge = navBadge(deployPreviewActive ? deploySim.state : deployEngine.state, deployPreviewActive ? deploySim.runtimeOverride ?? realRuntime : deployEngine.runtime);
  const statusAt = statusShelf.updatedAt;
  const statusError = statusShelf.error;
  const dockerVersion = dockerShelf.data;
  const dockerAt = dockerShelf.updatedAt;
  const counts = countsShelf.data;
  const countsAt = countsShelf.updatedAt;
  const config = configShelf.data;
  const configAt = configShelf.updatedAt;
  const binaryPath = binaryShelf.data ?? "";
  const logPath = logPathShelf.data ?? "";
  const doctor = doctorShelf.data;
  const doctorAt = doctorShelf.updatedAt;
  const logs = logsShelf.data ?? "";
  const logsAt = logsShelf.updatedAt;

  // Stable refresh refs for bootstrap — avoid shelf object identity churn
  const refreshStatus = statusShelf.refresh;
  const refreshDocker = dockerShelf.refresh;
  const refreshCounts = countsShelf.refresh;
  const refreshConfig = configShelf.refresh;
  const refreshBinary = binaryShelf.refresh;
  const refreshLogPath = logPathShelf.refresh;
  const refreshDoctor = doctorShelf.refresh;
  const refreshLogs = logsShelf.refresh;
  const refreshContainers = containersShelf.refresh;
  const refreshImages = imagesShelf.refresh;
  const refreshVolumes = volumesShelf.refresh;
  const refreshNetworks = networksShelf.refresh;
  const refreshContexts = contextsShelf.refresh;
  const publishStatus = statusShelf.publish;
  const publishDocker = dockerShelf.publish;
  const publishCounts = countsShelf.publish;
  const publishConfig = configShelf.publish;

  useEffect(() => {
    if (!tauriAvailable || !bootstrapEventPush) return;
    let disposed = false;
    bootstrapEventReadyRef.current = Promise.all([
      listen<BootstrapEvent>("bootstrap-status", ({ payload }) => {
        if (disposed || !payload.status) return;
        if (bootstrapTrace) console.debug("[bootstrap-trace] event", { observedAtMs: payload.observedAtMs, receivedAtMs: Date.now(), state: payload.status.state, dockerReady: payload.status.dockerReady });
        const status = payload.status;
        publishStatus(status);
        if (status.state === "starting") { setBootstrapStatus("Starting Harpoon…"); setBootstrapPhase("starting"); }
        else if (status.state === "booting") { setBootstrapStatus("Starting Linux VM…"); setBootstrapPhase("vm_booting"); }
        else if (status.state === "running" && !status.dockerReady) { setBootstrapStatus("Waiting for Docker Engine readiness…"); setBootstrapPhase("docker_starting"); }
        else if (status.state === "running" && status.dockerReady) setBootstrapStatus("Harpoon is ready.");
      }),
      listen<BootstrapEvent>("bootstrap-complete", ({ payload }) => {
        if (disposed) return;
        if (payload.ok) {
          setBootstrapStatus("Harpoon is ready.");
          setBootstrapPhase("ready");
          bootstrapActiveRef.current = false;
          refreshCounts(); refreshDocker();
          setTimeout(() => { refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); }, 300);
        } else {
          setBootstrapError(payload.error ?? "Harpoon could not start.");
          setBootstrapPhase("failed");
          bootstrapActiveRef.current = false;
        }
      }),
    ]).then(([unlistenStatus, unlistenComplete]) => {
      if (disposed) { unlistenStatus(); unlistenComplete(); return; }
      bootstrapEventCleanupRef.current = [unlistenStatus, unlistenComplete];
    }).then(() => undefined);
    return () => { disposed = true; bootstrapEventCleanupRef.current.splice(0).forEach((unlisten) => unlisten()); void invoke("stop_bootstrap_worker"); bootstrapEventReadyRef.current = null; };
  }, [tauriAvailable, bootstrapEventPush, bootstrapTrace, publishStatus, refreshCounts, refreshDocker, refreshContainers, refreshImages, refreshVolumes, refreshNetworks]);

  // Bootstrap state machine — must use authoritative shelf data, not stale closure
  const runBootstrap = useCallback(async (manual: boolean) => {
    if (bootstrapActiveRef.current) return;
    bootstrapActiveRef.current = true;
    setBootstrapError(null);
    setActionError(null);
    try {
      setBootstrapPhase("launching");
      setBootstrapStatus("Resolving Harpoon binary…");
      await refreshBinary();
      await new Promise(r => setTimeout(r, 120));
      setBootstrapPhase("discovering");
      setBootstrapStatus("Checking runtime status…");
      const s0 = await refreshStatus();
      setBootstrapStatus("Reading runtime configuration and Docker state…");
      await Promise.allSettled([refreshConfig(), refreshDocker(), refreshCounts(), refreshLogPath()]);
      // Stage 3B: ensure Harpoon docker context exists (idempotent, no default switch, Finder-safe resolver)
      // This fixes clean-install where context was manually required; safe to run repeatedly
      setBootstrapStatus("Ensuring Harpoon Docker context…");
      try { await invoke<string>("ensure_harpoon_context"); } catch (e) { console.warn("ensure_harpoon_context failed", e); }
      if (bootstrapEventPush) {
        await bootstrapEventReadyRef.current;
        try { await invoke("start_bootstrap_worker"); }
        catch (error) { setBootstrapError(String(error)); setBootstrapPhase("failed"); bootstrapActiveRef.current = false; }
        return;
      }
      // Doctor is independent; keep the verbose log tail out of clean performance runs.
      refreshDoctor();
      if (!s0) { setBootstrapError("Could not read Harpoon status."); setBootstrapPhase("failed"); return; }
      if (s0.state === "running" && s0.dockerReady) {
        setBootstrapStatus("Harpoon is ready.");
        setBootstrapPhase("ready");
        setTimeout(()=>{ refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); }, 300);
        return;
      }
      if (s0.state === "running" && !s0.dockerReady) {
        setBootstrapStatus("Waiting for Docker Engine readiness…");
        setBootstrapPhase("docker_starting");
        const deadline = Date.now() + 30000;
        while (Date.now() < deadline) {
          await new Promise(r => setTimeout(r, 750));
          const ns = await refreshStatus();
          await refreshDocker();
          if (ns && ns.state === "running" && ns.dockerReady) { setBootstrapStatus("Harpoon is ready."); setBootstrapPhase("ready"); refreshCounts(); refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); return; }
          if (ns && ns.state === "failed") break;
        }
        setBootstrapError("Docker Engine did not become ready within 30 seconds."); setBootstrapPhase("failed"); return;
      }
      if (s0.state === "starting" || s0.state === "booting") {
        const startPhase = s0.state === "booting" ? "vm_booting" : "starting";
        setBootstrapStatus(startPhase === "vm_booting" ? "Starting Linux VM…" : "Starting Harpoon…");
        setBootstrapPhase(startPhase as BootstrapPhase);
        const deadline = Date.now() + 60000;
        while (Date.now() < deadline) {
          await new Promise(r => setTimeout(r, 750));
          const ns = await refreshStatus();
          if (!ns) continue;
          if (ns.state === "starting") { setBootstrapStatus("Starting Harpoon…"); setBootstrapPhase("starting"); }
          else if (ns.state === "booting") { setBootstrapStatus("Starting Linux VM…"); setBootstrapPhase("vm_booting"); }
          else if (ns.state === "running" && !ns.dockerReady) { setBootstrapStatus("Waiting for Docker Engine readiness…"); setBootstrapPhase("docker_starting"); }
          else if (ns.state === "running" && ns.dockerReady) { setBootstrapStatus("Harpoon is ready."); setBootstrapPhase("ready"); refreshCounts(); refreshDocker(); setTimeout(()=>{ refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); }, 300); return; }
          else if (ns.state === "failed") break;
        }
        setBootstrapError("Harpoon startup timed out."); setBootstrapPhase("failed"); return;
      }
      if (s0.state === "failed") {
        setBootstrapError(s0.failure?.message ?? "Harpoon reported an authoritative runtime failure.");
        setBootstrapPhase("failed");
        return;
      }
      if (s0.state === "stopped" || s0.state === "stale") {
        if (!manual && bootstrapAttempted.current) { setBootstrapPhase("stopped"); return; }
        if (!manual) bootstrapAttempted.current = true;
        setBootstrapStatus("Starting Harpoon…");
        setBootstrapPhase("starting");
        try { await invoke<string>("start_harpoon"); } catch (e:any) {
          const msg = String(e);
          if (msg.includes("HOST_VZ_START_FAILURE") || msg.includes("VZErrorDomain")) { setBootstrapError("Virtualization.framework returned VZErrorDomain 1. The virtual machine failed to start."); setBootstrapPhase("failed"); return; }
          else if (!msg.toLowerCase().includes("already running")) { setBootstrapError(msg); setBootstrapPhase("failed"); return; }
        }
        const deadline = Date.now() + 60000;
        let dockerWaitStart: number | null = null;
        while (Date.now() < deadline) {
          await new Promise(r => setTimeout(r, 750));
          const ns = await refreshStatus();
          if (!ns) continue;
          if (ns.state === "starting") { setBootstrapStatus("Starting Harpoon…"); setBootstrapPhase("starting"); }
          else if (ns.state === "booting") { setBootstrapStatus("Starting Linux VM…"); setBootstrapPhase("vm_booting"); }
          else if (ns.state === "running" && !ns.dockerReady) {
            setBootstrapStatus("Waiting for Docker Engine readiness…");
            setBootstrapPhase("docker_starting");
            if (dockerWaitStart === null) dockerWaitStart = Date.now();
            if (Date.now() - dockerWaitStart > 30000) { setBootstrapError("Docker Engine did not become ready within 30 seconds."); setBootstrapPhase("failed"); return; }
          } else if (ns.state === "running" && ns.dockerReady) { setBootstrapStatus("Harpoon is ready."); setBootstrapPhase("ready"); await refreshCounts(); await refreshDocker(); await refreshConfig(); setTimeout(()=>{ refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); }, 300); return; }
          else if (ns.state === "failed" || ns.state === "stale") { setBootstrapError("Harpoon could not start. VM failed."); setBootstrapPhase("failed"); return; }
        }
        setBootstrapError("Harpoon startup timed out."); setBootstrapPhase("failed"); return;
      }
      setBootstrapStatus("Harpoon is ready.");
      setBootstrapPhase("ready");
    } finally { if (!bootstrapEventPush) bootstrapActiveRef.current = false; }
  }, [bootstrapEventPush, refreshBinary, refreshStatus, refreshConfig, refreshDocker, refreshCounts, refreshLogPath, refreshDoctor, refreshLogs, refreshContainers, refreshImages, refreshVolumes, refreshNetworks]);

  useEffect(()=>{
    if (!tauriAvailable) {
      setBootstrapPhase("stopped");
      return;
    }
    runBootstrap(false);
  }, [runBootstrap, tauriAvailable]);

  // Reconcile bootstrap with live authoritative shelf data — header and overlay must share truth
  useEffect(()=>{
    if (bootstrapActiveRef.current) return;
    if (status?.state==="running" && status?.dockerReady && binaryPath) {
      if (bootstrapPhase==="discovering" || bootstrapPhase==="launching" || bootstrapPhase==="starting" || bootstrapPhase==="vm_booting" || bootstrapPhase==="docker_starting") {
        setBootstrapPhase("ready");
      }
    } else if (status?.state==="stopped" || status?.state==="stale") {
      if (bootstrapPhase==="ready" || bootstrapPhase==="discovering" || bootstrapPhase==="launching" || bootstrapPhase==="docker_starting" || bootstrapPhase==="starting" || bootstrapPhase==="vm_booting") {
        setBootstrapPhase("stopped");
      }
    } else if (status?.state==="failed") {
      if (bootstrapPhase!=="failed") setBootstrapPhase("failed");
    }
  }, [status, binaryPath, bootstrapPhase]);

  // Rust owns normal refresh scheduling; the JS path is an emergency startup fallback only.
  useEffect(()=>{
    if (!tauriAvailable) return;
    const isBootstrapActive = bootstrapPhase==="starting"||bootstrapPhase==="vm_booting"||bootstrapPhase==="docker_starting"||bootstrapPhase==="launching"||bootstrapPhase==="discovering";
    const intervalMs = isBootstrapActive ? 800 : 3500;
    const rustMode = !isBootstrapActive && !rustRefreshFallback;
    if (rustMode) {
      let disposed = false;
      let workerGeneration: number | null = null;
      let unlisten: (() => void) | undefined;
      const apply = (item: RuntimeRefreshEvent) => {
        if (!item.key || item.key === "cycle" || item.generation !== workerGeneration) return;
        const publish = item.key === "get_status" ? publishStatus : item.key === "get_docker_info" ? publishDocker : item.key === "get_counts" ? publishCounts : item.key === "get_config" ? publishConfig : null;
        if (publish) publish(item.value as never, item.ok ? null : (item.error ?? "runtime refresh failed"));
      };
      void listen<RuntimeRefreshEvent>("runtime-refresh", ({ payload }) => {
        if (disposed) return;
        if (payload.key === "cycle") payload.values?.forEach(apply);
        else apply(payload);
      }).then((cleanup) => {
        if (disposed) cleanup(); else unlisten = cleanup;
      });
      const startWorker = () => {
        if (document.visibilityState !== "visible") return;
        void invoke("start_runtime_refresh_worker", {
          config: {
            status: true,
            dockerInfo: true,
            counts: true,
            config: true,
          },
        }).then((generation) => { if (!disposed) workerGeneration = generation as number; }).catch(() => {
          if (disposed) return;
          setRustRefreshFallback(true);
        });
      };
      startWorker();
      const onVis = () => {
        if (document.visibilityState === "visible") startWorker();
        else void invoke("stop_runtime_refresh_worker");
      };
      document.addEventListener("visibilitychange", onVis);
      return () => { disposed = true; unlisten?.(); document.removeEventListener("visibilitychange", onVis); void invoke("stop_runtime_refresh_worker"); };
    }
    const refreshFallback = () => { refreshStatus(); refreshDocker(); refreshCounts(); refreshConfig(); };
    const id = window.setInterval(() => { if (document.visibilityState === "visible") refreshFallback(); }, intervalMs);
    const onVis = () => { if (document.visibilityState === "visible") refreshFallback(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [bootstrapPhase, refreshStatus, refreshDocker, refreshCounts, refreshConfig, publishStatus, publishDocker, publishCounts, publishConfig, tauriAvailable, rustRefreshFallback]);

  // Native sampler owns collection/history; header and Overview subscribe to one shared store.
  useEffect(() => {
    configureResourceSampler({ enabled: tauriAvailable && bootstrapPhase === "ready" && document.visibilityState === "visible", diskPath: status?.diskPath });
    const onVisibility = () => configureResourceSampler({ enabled: tauriAvailable && bootstrapPhase === "ready" && document.visibilityState === "visible", diskPath: status?.diskPath });
    document.addEventListener("visibilitychange", onVisibility);
    return () => { document.removeEventListener("visibilitychange", onVisibility); configureResourceSampler({ enabled: false }); };
  }, [bootstrapPhase, status?.diskPath, tauriAvailable]);

  // Tab-aware polling — only when tab visible and ready
  // Defer initial refresh to next frame so nav paint commits first (rAF yield)
  useEffect(()=>{
    if (!tauriAvailable) return;
    if (active!=="containers" || bootstrapPhase!=="ready") return;
    const raf = requestAnimationFrame(()=> refreshContainers());
    const id = window.setInterval(()=>{
      if (document.visibilityState!=="visible" || active!=="containers") return;
      refreshContainers();
    }, 5000);
    const onVis = ()=>{ if (document.visibilityState==="visible" && active==="containers") refreshContainers(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ cancelAnimationFrame(raf); clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [active, bootstrapPhase, refreshContainers, tauriAvailable]);

  useEffect(()=>{
    if (!tauriAvailable) return;
    if (active!=="images" || bootstrapPhase!=="ready") return;
    const raf = requestAnimationFrame(()=> refreshImages());
    const id = window.setInterval(()=>{ if (document.visibilityState!=="visible"||active!=="images") return; refreshImages(); }, 20000);
    const onVis = ()=>{ if (document.visibilityState==="visible" && active==="images") refreshImages(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ cancelAnimationFrame(raf); clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [active, bootstrapPhase, refreshImages, tauriAvailable]);

  useEffect(()=>{
    if (!tauriAvailable) return;
    if (active!=="volumes" || bootstrapPhase!=="ready") return;
    const raf = requestAnimationFrame(()=> refreshVolumes());
    const id = window.setInterval(()=>{ if (document.visibilityState!=="visible"||active!=="volumes") return; refreshVolumes(); }, 20000);
    const onVis = ()=>{ if (document.visibilityState==="visible" && active==="volumes") refreshVolumes(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ cancelAnimationFrame(raf); clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [active, bootstrapPhase, refreshVolumes, tauriAvailable]);

  useEffect(()=>{
    if (!tauriAvailable) return;
    if (active!=="networks" || bootstrapPhase!=="ready") return;
    const raf = requestAnimationFrame(()=> refreshNetworks());
    const id = window.setInterval(()=>{ if (document.visibilityState!=="visible"||active!=="networks") return; refreshNetworks(); }, 20000);
    const onVis = ()=>{ if (document.visibilityState==="visible" && active==="networks") refreshNetworks(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ cancelAnimationFrame(raf); clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [active, bootstrapPhase, refreshNetworks, tauriAvailable]);

  // Diagnostics: doctor and logs are independent shelves, not coupled — defer to next frame
  useEffect(()=>{
    if (!tauriAvailable) return;
    if (active!=="diagnostics") return;
    const raf1 = requestAnimationFrame(()=> refreshDoctor());
    const raf2 = requestAnimationFrame(()=> refreshLogs());
    const id = window.setInterval(()=>{
      if (document.visibilityState!=="visible"||active!=="diagnostics") return;
      refreshLogs();
    }, 2500);
    const onVis = ()=>{ if (document.visibilityState==="visible" && active==="diagnostics") refreshLogs(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ cancelAnimationFrame(raf1); cancelAnimationFrame(raf2); clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [active, refreshDoctor, refreshLogs, tauriAvailable]);

  // Navigation stays synchronous; view refreshes are deferred to their effects.
  const handleNav = useCallback((n: Nav)=>{
    setDestination({ kind: "nav", nav: n });
  }, []);

  const openContainer = useCallback((containerId: string) => {
    if (!containerId) return;
    setDestination(containerDestination("harpoon", containerId));
  }, []);

  const doAction = async (name: string, fn: () => Promise<any>) => {
    if (actionInProgress) return;
    setActionInProgress(name);
    setActionError(null);
    setBootstrapError(null);
    if (name==="start" || name==="restart") { setBootstrapStatus("Starting Harpoon…"); setBootstrapPhase("starting"); }
    if (name==="stop") { setBootstrapStatus("Stopping Harpoon…"); setBootstrapPhase("starting"); }
    try {
      await fn();
      if (name==="start") { await runBootstrap(true); return; }
      if (name==="restart") {
        setBootstrapStatus("Starting Harpoon…");
        setBootstrapPhase("starting");
        const deadline = Date.now()+60000;
        let dockerWaitStart: number|null=null;
        while (Date.now()<deadline) {
          await new Promise(r=>setTimeout(r,750));
          const ns = await refreshStatus();
          if (!ns) continue;
          if (ns.state==="starting") { setBootstrapStatus("Starting Harpoon…"); setBootstrapPhase("starting"); }
          else if (ns.state==="booting") { setBootstrapStatus("Starting Linux VM…"); setBootstrapPhase("vm_booting"); }
          else if (ns.state==="running" && !ns.dockerReady) {
            setBootstrapStatus("Waiting for Docker Engine readiness…");
            setBootstrapPhase("docker_starting");
            if (dockerWaitStart===null) dockerWaitStart=Date.now();
            if (Date.now()-dockerWaitStart>30000){ setBootstrapError("Docker Engine did not become ready within 30 seconds."); setBootstrapPhase("failed"); break; }
          } else if (ns.state==="running" && ns.dockerReady){ setBootstrapStatus("Harpoon is ready."); setBootstrapPhase("ready"); await refreshCounts(); await refreshDocker(); setTimeout(()=>{ refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); },300); break; }
          else if (ns.state==="failed"){ setBootstrapError("Harpoon could not start."); setBootstrapPhase("failed"); break; }
          else if (ns.state==="stopped"||ns.state==="stale"){ setBootstrapPhase("stopped"); break; }
        }
        return;
      }
      if (name==="stop") {
        await new Promise(r=>setTimeout(r,400));
        const ns = await refreshStatus();
        if (ns && (ns.state==="stopped"||ns.state==="stale")) setBootstrapPhase("stopped");
        else if (ns && ns.state==="running" && ns.dockerReady) setBootstrapPhase("ready");
        await refreshCounts(); await refreshDocker();
        return;
      }
      if (name.startsWith("start-")||name.startsWith("stop-")||name.startsWith("restart-")||name.startsWith("rm-")){
        refreshContainers(); refreshCounts(); refreshStatus();
      } else if (name.startsWith("rmi-")){ refreshImages(); refreshCounts(); }
      else if (name.startsWith("rmvol-")){ refreshVolumes(); refreshCounts(); }
      else if (name.startsWith("rmnet-")){ refreshNetworks(); refreshCounts(); }
      else if (name==="cpus"||name==="memory"){ refreshConfig(); refreshStatus(); }
      else {
        refreshStatus(); refreshCounts();
        if (active==="containers") refreshContainers();
        if (active==="images") refreshImages();
        if (active==="volumes") refreshVolumes();
        if (active==="networks") refreshNetworks();
      }
    } catch (e:any) {
      const msg = String(e);
      if (msg.includes("HOST_VZ_START_FAILURE")||msg.includes("VZErrorDomain")){
        const vzMsg="Virtualization.framework returned VZErrorDomain 1. The virtual machine failed to start.";
        setActionError("HOST_VZ_START_FAILURE — "+vzMsg); setBootstrapError(vzMsg); setBootstrapPhase("failed");
      } else { setActionError(msg); setBootstrapError(msg); setBootstrapPhase("failed"); }
    } finally { setActionInProgress(null); }
  };

  const badge = status ? stateBadge(bootstrapPhase==="starting"||bootstrapPhase==="vm_booting"||bootstrapPhase==="docker_starting" ? bootstrapPhase : status.state) : null;
  const badgeLabel = (()=>{
    if (bootstrapPhase==="starting") return "STARTING";
    if (bootstrapPhase==="vm_booting") return "VM BOOTING";
    if (bootstrapPhase==="docker_starting") return "DOCKER STARTING";
    return badge?.label;
  })();
  const badgeStyle = (()=>{
    if (bootstrapPhase==="starting"||bootstrapPhase==="vm_booting") return { bg:"#fef9c3", fg:"#854d0e" };
    if (bootstrapPhase==="docker_starting") return { bg:"#dbeafe", fg:"#1e40af" };
    return badge ? { bg:badge.bg, fg:badge.fg } : null;
  })();

  const handleCopyDiagnostics = async () => {
    const bundle = `Harpoon Diagnostics\nbinary: ${binaryPath}\nstate: ${status?.state} pid:${status?.pid} cpus:${status?.cpus} mem:${status?.memoryMiB}\nsocket:${status?.socketPath} exists:${status?.sockExists} lock:${status?.lockPath} held:${status?.lockHeld}\nlog:${logPath}\nbootstrap:${bootstrapPhase}\n--- doctor ---\n${doctor?.raw}\n--- logs tail ---\n${logs}\n--- config ---\n${config?.raw}\n`;
    try { await navigator.clipboard.writeText(bundle); setCopied(true); setTimeout(()=>setCopied(false),1500); } catch {}
  };

  const isStopped = status?.state==="stopped" || status?.state==="stale" || bootstrapPhase==="stopped";
  const isRunning = status?.state==="running" || bootstrapPhase==="ready" || bootstrapPhase==="docker_starting";
  const showBootstrapOverlay = !technologyShowcase && !topologyPrototype && bootstrapPhase!=="ready" && bootstrapPhase!=="failed" && bootstrapPhase!=="stopped";
  const showFailed = !technologyShowcase && !topologyPrototype && bootstrapPhase==="failed";
  const showStoppedPanel = !technologyShowcase && !topologyPrototype && !deployPreviewActive && bootstrapPhase==="stopped" && !showBootstrapOverlay && !showFailed;

  return (
    <div className="shell">
      <ConfirmDialogHost />
      <ShellNav active={active} onNav={handleNav} binaryPath={binaryPath} statusAt={statusAt} deployBadge={deployBadge} />
      <div className={active === "containers" ? "shell-main shell-main--containers" : "shell-main"}>
        <header className="shell-header">
          <div className="shell-header__left">
            <div className="shell-header__title">
              <span className="text-title">{technologyShowcase ? "Technology marks" : destination.kind === "container" ? "Container workspace" : destination.nav[0].toUpperCase() + destination.nav.slice(1)}</span>
              <span className="text-meta">Harpoon / local runtime</span>
            </div>
            {badgeStyle && badgeLabel && <span className={"badge "+(badgeStyle as any).cls} style={{ background: badgeStyle.bg, color: badgeStyle.fg, borderColor: "transparent" }}>{badgeLabel}</span>}
            {status?.dockerReady!==undefined && <span className={status.dockerReady ? "badge badge--running" : "badge badge--error"}>Docker: {status.dockerReady ? "ready" : "not ready"}</span>}
            {counts && <span className="text-meta" style={{ color: "var(--text-secondary)", whiteSpace: "nowrap" }}>{counts.running}/{counts.containers} containers • {counts.images} imgs • {counts.volumes} vols • {counts.networks} nets</span>}
          </div>
          <div className="shell-header__right">
            <ResourceMeters />
            <EngineManageMenu state={status?.state ?? bootstrapPhase} disabled={!tauriAvailable || Boolean(actionInProgress)} onAction={(action) => doAction(action, () => invoke(`${action}_harpoon`))} />
            <button onClick={()=>{ refreshStatus(); refreshCounts(); refreshConfig(); refreshDocker(); if(active==="containers") refreshContainers(); if(active==="images") refreshImages(); if(active==="volumes") refreshVolumes(); if(active==="networks") refreshNetworks(); if(active==="diagnostics"){refreshDoctor(); refreshLogs();} }} disabled={!tauriAvailable} className="btn btn--secondary">Refresh</button>
          </div>
        </header>
        {actionError && !showFailed && <div style={{ background: "var(--error-bg)", border: "1px solid #7F1D1D", color: "var(--error-fg)", padding: 10, borderRadius: 8, marginBottom: 12, fontSize: 13 }}>{actionError}</div>}
        {statusError && !showFailed && <div style={{ background: "var(--error-bg)", border: "1px solid #7F1D1D", color: "var(--error-fg)", padding: 10, borderRadius: 8, marginBottom: 12, fontSize: 13 }}>status error: {statusError}</div>}
        {status?.failure && status.state !== "failed" && !showFailed && <div role="alert" style={{ background: "var(--error-bg)", border: "1px solid #7F1D1D", color: "var(--error-fg)", padding: 10, borderRadius: 8, marginBottom: 12, fontSize: 13 }}>{status.failure.message} <button className="btn btn--secondary btn--sm" onClick={() => handleNav("diagnostics")} style={{ marginLeft: 8 }}>View diagnostics</button></div>}
        {actionInProgress && <div style={{ fontSize: 12, color: "var(--text-secondary)", marginBottom: 8 }}>{actionInProgress} in progress…</div>}

        {showBootstrapOverlay && (
          <div className="panel bootstrap-panel" style={{ padding:"40px 24px", display:"flex", flexDirection:"column", alignItems:"center", justifyContent:"center", minHeight:320, textAlign:"center" }}>
            <div style={{ fontWeight:700, fontSize:20, marginBottom:12 }}>Harpoon</div>
            <div className="bootstrap-spinner" style={{ width:32, height:32, border:"3px solid var(--border-strong)", borderTopColor:"var(--accent-yellow)", borderRadius:"50%", animation:"spin 0.9s linear infinite", marginBottom:16 }}></div>
            <BootstrapStatus message={bootstrapStatus} trace={bootstrapTrace} />
            <style>{`@keyframes spin { to { transform: rotate(360deg); } }`}</style>
          </div>
        )}

        {showFailed && (
          <div className="panel" style={{ borderColor:"var(--error)", padding:24, minHeight:320 }}>
            <div style={{ fontWeight:700, fontSize:16, color:"#991b1b", marginBottom:8 }}>{status?.failure?.category === "vm_terminated" ? "Harpoon VM unexpectedly terminated" : status?.failure?.message || "Harpoon could not start"}</div>
            <div style={{ fontSize:13, color:"#374151", marginBottom:4 }}>Phase: <code style={{ background:"#f3f4f6", padding:"2px 6px", borderRadius:6 }}>{bootstrapPhase}</code> {status && <span style={{ fontSize:12, color:"#6b7280" }}>• state {status.state}</span>}</div>
            <div role="alert" aria-live="assertive" style={{ fontSize:13, color:"var(--error-fg)", background:"var(--error-bg)", border:"1px solid var(--error)", borderRadius:8, padding:10, marginBottom:12, whiteSpace:"pre-wrap", wordBreak:"break-word" }}>
              <div>{bootstrapError || actionError || "Unknown error"}</div>
              {status?.failure && <div style={{ marginTop:8, color:"var(--text-secondary)" }}>Source: {status.failure.source} · {new Date(status.failure.occurredAt).toLocaleString()}</div>}
              {status?.failure?.underlyingError && <details style={{ marginTop:8 }}><summary>View underlying diagnostics</summary><code>{status.failure.underlyingError}</code></details>}
            </div>
            <div style={{ display:"flex", gap:8 }}>
              <button onClick={()=>runBootstrap(true)} style={{ padding:"8px 14px", borderRadius:8, background:"#111827", color:"white", border:"1px solid #111827", fontSize:13 }}>Retry</button>
              <button onClick={()=>handleNav("diagnostics")} style={{ padding:"8px 14px", borderRadius:8, background:"white", border:"1px solid #d1d5db", fontSize:13 }}>Diagnostics</button>
              <button onClick={()=>{ refreshStatus(); setBootstrapPhase("discovering"); runBootstrap(true); }} style={{ padding:"8px 14px", borderRadius:8, background:"white", border:"1px solid #d1d5db", fontSize:13 }}>Check again</button>
            </div>
            <div style={{ fontSize:11, color:"#6b7280", marginTop:12 }}>The runtime is stopped; no automatic restart was attempted. Review Diagnostics before trying again.</div>
          </div>
        )}

        {showStoppedPanel && (
          <div className="panel" style={{ padding:24, marginBottom:12 }}>
            <div style={{ fontWeight:600, fontSize:14, marginBottom:6 }}>Harpoon is stopped</div>
            <div style={{ fontSize:13, color:"#6b7280", marginBottom:12 }}>Press Start to launch the Linux VM and Docker Engine.</div>
            <button onClick={()=>doAction("start", ()=>invoke("start_harpoon"))} disabled={!tauriAvailable} style={{ padding:"8px 14px", borderRadius:8, background:"#111827", color:"white", border:"none", fontSize:13 }}>Start Harpoon</button>
          </div>
        )}

        {!showBootstrapOverlay && !showFailed && (
          <ContentErrorBoundary>
          <>
            {technologyShowcase ? <TechnologyIconShowcase /> : topologyPrototype && DevTopologyPrototype ? <Suspense fallback={<div className="panel">Loading topology prototype…</div>}><DevTopologyPrototype /></Suspense> : <>
            {active==="overview" && <OverviewView status={status} statusAt={statusAt} dockerVersion={dockerVersion} dockerAt={dockerAt} counts={counts} countsAt={countsAt} config={config} configAt={configAt} doctor={doctor} doctorAt={doctorAt} binaryPath={binaryPath} logPath={logPath} bootstrapPhase={bootstrapPhase} runtimePolling={true} runtimePollingPaused={[]} onOpenDiagnostics={()=>handleNav("diagnostics")} onOpenContainers={()=>handleNav("containers")} onOpenResources={()=>handleNav("resources")} />}
            {active==="resources" && <ResourcesView config={config} configAt={configAt} status={status} binaryPath={binaryPath} actionInProgress={actionInProgress} onSetCpus={(v)=>doAction("cpus", ()=>invoke("set_cpus", { cpus: v }))} onSetMemory={(v)=>doAction("memory", ()=>invoke("set_memory", { memory: v }))} />}
            {active==="diagnostics" && <DiagnosticsView binaryPath={binaryPath} logs={logs} logsAt={logsAt} logsRefreshing={logsShelf.refreshing} logsError={logsShelf.error} logPath={logPath} doctor={doctor} doctorAt={doctorAt} doctorRefreshing={doctorShelf.refreshing} doctorError={doctorShelf.error} status={status} configRaw={config?.raw ?? null} bootstrapPhase={bootstrapPhase} onCopy={handleCopyDiagnostics} copied={copied} onRefreshDoctor={()=>doctorShelf.refresh()} onRefreshLogs={()=>logsShelf.refresh()} />}
            {active==="contexts" && <ContextsView contexts={contextsShelf.data ?? []} refreshing={contextsShelf.refreshing} error={contextsShelf.error} updatedAt={contextsShelf.updatedAt} hasCache={contextsShelf.hasCache} stale={contextsShelf.stale} refresh={refreshContexts} />}
            {active==="containers" && destination.kind==="container" && <ContainerDetailView engine={destination.engine} context={destination.context} containerId={destination.containerId} actionInProgress={actionInProgress} doAction={doAction} onBack={()=>handleNav("containers")} onNavigate={handleNav} />}
            {active==="containers" && destination.kind==="nav" && <ContainersView containers={containersShelf.data ?? []} refreshing={containersShelf.refreshing} error={containersShelf.error} status={status} updatedAt={containersShelf.updatedAt} hasCache={containersShelf.hasCache} actionInProgress={actionInProgress} doAction={doAction} refresh={()=>containersShelf.refresh()} onOpenContainer={openContainer} />}
            {active==="deployments" && <Suspense fallback={<div className="panel" style={{ padding: 16 }}>Loading Deployments…</div>}><DeployView {...(deployPreviewActive ? { sim: deploySim, preview: deployPreview } : { engine: deployEngine })} realRuntime={realRuntime} onStartHarpoon={()=>doAction("start", ()=>invoke("start_harpoon"))} onOpenContainers={()=>handleNav("containers")} /></Suspense>}
            {active==="images" && <ImagesView images={imagesShelf.data ?? []} refreshing={imagesShelf.refreshing} error={imagesShelf.error} updatedAt={imagesShelf.updatedAt} hasCache={imagesShelf.hasCache} actionInProgress={actionInProgress} doAction={doAction} refresh={()=>imagesShelf.refresh()} />}
            {active==="volumes" && <VolumesView volumes={volumesShelf.data ?? []} refreshing={volumesShelf.refreshing} error={volumesShelf.error} updatedAt={volumesShelf.updatedAt} hasCache={volumesShelf.hasCache} actionInProgress={actionInProgress} doAction={doAction} refresh={()=>volumesShelf.refresh()} />}
            {active==="networks" && <NetworksView networks={networksShelf.data ?? []} containers={containersShelf.data ?? []} refreshing={networksShelf.refreshing} error={networksShelf.error} updatedAt={networksShelf.updatedAt} hasCache={networksShelf.hasCache} containersError={containersShelf.error} containersUpdatedAt={containersShelf.updatedAt} containersHasCache={containersShelf.hasCache} actionInProgress={actionInProgress} doAction={doAction} refresh={()=>networksShelf.refresh()} refreshContainers={()=>containersShelf.refresh()} />}
            </>}
          </>
          </ContentErrorBoundary>
        )}
      </div>
      <QuakeShellPrototype containers={containersShelf.data ?? []} />
    </div>
  );
}
