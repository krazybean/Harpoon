import { useEffect, useState, useCallback, useRef, memo } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./theme.css";
import "./styles/primitives.css";
import { House, Stack, Images, Database, ShareNetwork, Sliders, Heartbeat } from "@phosphor-icons/react";
import { useShelf } from "./hooks/useShelf";
import { OverviewView } from "./views/OverviewView";
import { ContainersView } from "./views/ContainersView";
import { ImagesView } from "./views/ImagesView";
import { VolumesView } from "./views/VolumesView";
import { NetworksView } from "./views/NetworksView";
import { ResourcesView } from "./views/ResourcesView";
import { DiagnosticsView } from "./views/DiagnosticsView";

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
};
type DoctorResult = { raw: string; passed: number; warnings: number; failures: number };
type ConfigResult = { cpus: number; memory: number; raw: string; path: string };
type Counts = { containers: number; running: number; images: number; volumes: number; networks: number };
type Nav = "overview" | "containers" | "images" | "volumes" | "networks" | "resources" | "diagnostics";
type BootstrapPhase = "launching" | "discovering" | "stopped" | "starting" | "vm_booting" | "docker_starting" | "ready" | "failed";

function stateBadge(state: string) {
  const s = state.toLowerCase();
  if (s === "running") return { bg: "var(--success-bg)", fg: "var(--success-fg)", label: "RUNNING", cls: "badge--running" };
  if (s === "starting" || s === "booting") return { bg: "var(--warning-bg)", fg: "var(--warning-fg)", label: state.toUpperCase(), cls: "badge--warning" };
  if (s === "stopped") return { bg: "var(--panel-header)", fg: "var(--text-secondary)", label: "STOPPED", cls: "badge--stopped" };
  if (s === "failed") return { bg: "var(--error-bg)", fg: "var(--error-fg)", label: "FAILED", cls: "badge--error" };
  if (s === "stale") return { bg: "var(--warning-bg)", fg: "var(--warning-fg)", label: "STALE", cls: "badge--warning" };
  return { bg: "var(--info-bg)", fg: "var(--info-fg)", label: state.toUpperCase(), cls: "badge--info" };
}
function phaseLabel(p: BootstrapPhase): string {
  switch(p) {
    case "launching": return "Preparing Harpoon…";
    case "discovering": return "Checking runtime…";
    case "stopped": return "Harpoon stopped";
    case "starting": return "Starting Harpoon…";
    case "vm_booting": return "Starting Linux VM…";
    case "docker_starting": return "Waiting for Docker Engine…";
    case "ready": return "Ready";
    case "failed": return "Harpoon could not start";
  }
}
function phaseDetail(p: BootstrapPhase, status: HarpoonStatus | null): string {
  if (p==="launching") return "Resolving Harpoon binary…";
  if (p==="discovering") return "Checking runtime…";
  if (p==="starting") return status ? `Harpoon state: ${status.state}` : "Starting runtime…";
  if (p==="vm_booting") return status ? `VM ${status.state} • PID ${status.pid ?? "—"}` : "Booting Linux VM…";
  if (p==="docker_starting") return "Docker socket exists, waiting for Engine…";
  if (p==="ready") return "Harpoon ready";
  return "";
}
function fmtTs(ts: number | null) {
  if (!ts) return "—";
  return new Date(ts).toLocaleTimeString();
}

const NavItems = memo(function NavItems({ active, onNav }: { active: Nav; onNav: (n: Nav)=>void }) {
  return (
    <>
      {([
        ["overview", House, "Overview"],
        ["containers", Stack, "Containers"],
        ["images", Images, "Images"],
        ["volumes", Database, "Volumes"],
        ["networks", ShareNetwork, "Networks"],
        ["resources", Sliders, "Resources"],
        ["diagnostics", Heartbeat, "Diagnostics"],
      ] as const).map(([n, Icon, label]) => (
        <button key={n} aria-current={active===n ? "page" : undefined} aria-label={label} title={label} onClick={() => onNav(n as Nav)} className={active===n ? "shell-nav__item shell-nav__item--active" : "shell-nav__item"}>
          <Icon size={20} weight={active===n ? "fill" : "regular"} className="shell-nav__icon" aria-hidden="true" />
          <span className="shell-nav__label">{label}</span>
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
const ShellNav = memo(function ShellNav({ active, onNav, binaryPath, statusAt }: { active: Nav; onNav: (n: Nav)=>void; binaryPath: string; statusAt: number | null }) {
  return (
    <nav className="shell-nav" aria-label="Primary">
      <div className="text-title" style={{ padding: "4px 0 8px 0", textAlign: "center", fontSize: 12, letterSpacing: "-0.02em" }}>Harpoon</div>
      <NavItems active={active} onNav={onNav} />
      <NavFooter binaryPath={binaryPath} statusAt={statusAt} />
    </nav>
  );
});

export default function App() {
  const [active, setActive] = useState<Nav>("overview");
  const [actionInProgress, setActionInProgress] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [bootstrapPhase, setBootstrapPhase] = useState<BootstrapPhase>("launching");
  const [bootstrapError, setBootstrapError] = useState<string | null>(null);
  const bootstrapAttempted = useRef(false);
  const bootstrapActiveRef = useRef(false);

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

  const status = statusShelf.data;
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

  // Bootstrap state machine — must use authoritative shelf data, not stale closure
  const runBootstrap = useCallback(async (manual: boolean) => {
    if (bootstrapActiveRef.current) return;
    bootstrapActiveRef.current = true;
    setBootstrapError(null);
    setActionError(null);
    try {
      setBootstrapPhase("launching");
      await refreshBinary();
      await new Promise(r => setTimeout(r, 120));
      setBootstrapPhase("discovering");
      const s0 = await refreshStatus();
      await Promise.allSettled([refreshConfig(), refreshDocker(), refreshCounts(), refreshLogPath()]);
      // Stage 3B: ensure Harpoon docker context exists (idempotent, no default switch, Finder-safe resolver)
      // This fixes clean-install where context was manually required; safe to run repeatedly
      try { await invoke<string>("ensure_harpoon_context"); } catch (e) { console.warn("ensure_harpoon_context failed", e); }
      // doctor/logs are independent shelves; refresh without blocking chrome
      refreshDoctor();
      refreshLogs();
      if (!s0) { setBootstrapError("Could not read Harpoon status."); setBootstrapPhase("failed"); return; }
      if (s0.state === "running" && s0.dockerReady) {
        setBootstrapPhase("ready");
        setTimeout(()=>{ refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); }, 300);
        return;
      }
      if (s0.state === "running" && !s0.dockerReady) {
        setBootstrapPhase("docker_starting");
        const deadline = Date.now() + 30000;
        while (Date.now() < deadline) {
          await new Promise(r => setTimeout(r, 750));
          const ns = await refreshStatus();
          await refreshDocker();
          if (ns && ns.state === "running" && ns.dockerReady) { setBootstrapPhase("ready"); refreshCounts(); refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); return; }
          if (ns && ns.state === "failed") break;
        }
        setBootstrapError("Docker Engine did not become ready within 30 seconds."); setBootstrapPhase("failed"); return;
      }
      if (s0.state === "starting" || s0.state === "booting") {
        const startPhase = s0.state === "booting" ? "vm_booting" : "starting";
        setBootstrapPhase(startPhase as BootstrapPhase);
        const deadline = Date.now() + 60000;
        while (Date.now() < deadline) {
          await new Promise(r => setTimeout(r, 750));
          const ns = await refreshStatus();
          if (!ns) continue;
          if (ns.state === "starting") setBootstrapPhase("starting");
          else if (ns.state === "booting") setBootstrapPhase("vm_booting");
          else if (ns.state === "running" && !ns.dockerReady) setBootstrapPhase("docker_starting");
          else if (ns.state === "running" && ns.dockerReady) { setBootstrapPhase("ready"); refreshCounts(); refreshDocker(); setTimeout(()=>{ refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); }, 300); return; }
          else if (ns.state === "failed") break;
        }
        setBootstrapError("Harpoon startup timed out."); setBootstrapPhase("failed"); return;
      }
      if (s0.state === "stopped" || s0.state === "stale" || s0.state === "failed") {
        if (!manual && bootstrapAttempted.current) { setBootstrapPhase("stopped"); return; }
        if (!manual) bootstrapAttempted.current = true;
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
          if (ns.state === "starting") setBootstrapPhase("starting");
          else if (ns.state === "booting") setBootstrapPhase("vm_booting");
          else if (ns.state === "running" && !ns.dockerReady) {
            setBootstrapPhase("docker_starting");
            if (dockerWaitStart === null) dockerWaitStart = Date.now();
            if (Date.now() - dockerWaitStart > 30000) { setBootstrapError("Docker Engine did not become ready within 30 seconds."); setBootstrapPhase("failed"); return; }
          } else if (ns.state === "running" && ns.dockerReady) { setBootstrapPhase("ready"); await refreshCounts(); await refreshDocker(); await refreshConfig(); setTimeout(()=>{ refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); }, 300); return; }
          else if (ns.state === "failed" || ns.state === "stale") { setBootstrapError("Harpoon could not start. VM failed."); setBootstrapPhase("failed"); return; }
        }
        setBootstrapError("Harpoon startup timed out."); setBootstrapPhase("failed"); return;
      }
      setBootstrapPhase("ready");
    } finally { bootstrapActiveRef.current = false; }
  }, [refreshBinary, refreshStatus, refreshConfig, refreshDocker, refreshCounts, refreshLogPath, refreshDoctor, refreshLogs, refreshContainers, refreshImages, refreshVolumes, refreshNetworks]);

  useEffect(()=>{ runBootstrap(false); }, [runBootstrap]);

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

  // Global polling: visibility-aware, bootstrap fast vs normal 3.5s
  // Defer heavy counts refresh if nav just happened to avoid paint contention
  useEffect(()=>{
    const isBootstrapActive = bootstrapPhase==="starting"||bootstrapPhase==="vm_booting"||bootstrapPhase==="docker_starting"||bootstrapPhase==="launching"||bootstrapPhase==="discovering";
    const intervalMs = isBootstrapActive ? 800 : 3500;
    const tick = () => {
      if (document.visibilityState!=="visible" && !isBootstrapActive) return;
      // Skip tick if nav happened within last 100ms to protect paint
      if (navTimingRef.current !== null && performance.now() - navTimingRef.current < 120) return;
      refreshStatus();
      refreshDocker();
      // Defer heavy counts to next frame to avoid blocking nav paint
      requestAnimationFrame(()=> refreshCounts());
      refreshConfig();
    };
    const id = window.setInterval(tick, intervalMs);
    const onVis = () => { if (document.visibilityState==="visible") tick(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [bootstrapPhase, refreshStatus, refreshDocker, refreshCounts, refreshConfig]);

  // Tab-aware polling — only when tab visible and ready
  // Defer initial refresh to next frame so nav paint commits first (rAF yield)
  useEffect(()=>{
    if (active!=="containers" || bootstrapPhase!=="ready") return;
    const raf = requestAnimationFrame(()=> refreshContainers());
    const id = window.setInterval(()=>{
      if (document.visibilityState!=="visible" || active!=="containers") return;
      refreshContainers();
    }, 5000);
    const onVis = ()=>{ if (document.visibilityState==="visible" && active==="containers") refreshContainers(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ cancelAnimationFrame(raf); clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [active, bootstrapPhase, refreshContainers]);

  useEffect(()=>{
    if (active!=="images" || bootstrapPhase!=="ready") return;
    const raf = requestAnimationFrame(()=> refreshImages());
    const id = window.setInterval(()=>{ if (document.visibilityState!=="visible"||active!=="images") return; refreshImages(); }, 20000);
    const onVis = ()=>{ if (document.visibilityState==="visible" && active==="images") refreshImages(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ cancelAnimationFrame(raf); clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [active, bootstrapPhase, refreshImages]);

  useEffect(()=>{
    if (active!=="volumes" || bootstrapPhase!=="ready") return;
    const raf = requestAnimationFrame(()=> refreshVolumes());
    const id = window.setInterval(()=>{ if (document.visibilityState!=="visible"||active!=="volumes") return; refreshVolumes(); }, 20000);
    const onVis = ()=>{ if (document.visibilityState==="visible" && active==="volumes") refreshVolumes(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ cancelAnimationFrame(raf); clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [active, bootstrapPhase, refreshVolumes]);

  useEffect(()=>{
    if (active!=="networks" || bootstrapPhase!=="ready") return;
    const raf = requestAnimationFrame(()=> refreshNetworks());
    const id = window.setInterval(()=>{ if (document.visibilityState!=="visible"||active!=="networks") return; refreshNetworks(); }, 20000);
    const onVis = ()=>{ if (document.visibilityState==="visible" && active==="networks") refreshNetworks(); };
    document.addEventListener("visibilitychange", onVis);
    return ()=>{ cancelAnimationFrame(raf); clearInterval(id); document.removeEventListener("visibilitychange", onVis); };
  }, [active, bootstrapPhase, refreshNetworks]);

  // Diagnostics: doctor and logs are independent shelves, not coupled — defer to next frame
  useEffect(()=>{
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
  }, [active, refreshDoctor, refreshLogs]);

  // Navigation must stay synchronous — active updates immediately, refresh is async
  // Instrumentation for nav latency (dev only, removed if noisy)
  const navTimingRef = useRef<number | null>(null);
  const handleNav = useCallback((n: Nav)=>{
    // NAV_HANDLER_START — synchronous, must not wait for Docker
    if (typeof performance !== "undefined") {
      navTimingRef.current = performance.now();
      // optional dev mark — keep minimal, remove if noisy
      // performance.mark(`NAV_${n}_HANDLER`);
    }
    setActive(n);
    // Allow browser to paint active state before any refresh work
    // (refresh is deferred to effect + rAF, not here)
    if (typeof performance !== "undefined" && navTimingRef.current !== null) {
      const t0 = navTimingRef.current;
      const handlerMs = performance.now() - t0;
      if (handlerMs > 16) console.debug(`[nav] ${n} handler ${handlerMs.toFixed(1)}ms`);
      requestAnimationFrame(()=>{
        const raf1 = performance.now() - t0;
        // FIRST_RAF ~ next paint, should be <50ms
        if (raf1 > 50) console.debug(`[nav] ${n} first RAF ${raf1.toFixed(1)}ms`);
        requestAnimationFrame(()=>{
          const raf2 = performance.now() - t0;
          if (raf2 > 100) console.debug(`[nav] ${n} second RAF ${raf2.toFixed(1)}ms`);
        });
      });
    }
  }, []);

  const doAction = async (name: string, fn: () => Promise<any>) => {
    if (actionInProgress) return;
    setActionInProgress(name);
    setActionError(null);
    setBootstrapError(null);
    if (name==="start") setBootstrapPhase("starting");
    if (name==="restart") setBootstrapPhase("starting");
    if (name==="stop") setBootstrapPhase("starting");
    try {
      await fn();
      if (name==="start") { await runBootstrap(true); return; }
      if (name==="restart") {
        setBootstrapPhase("starting");
        const deadline = Date.now()+60000;
        let dockerWaitStart: number|null=null;
        while (Date.now()<deadline) {
          await new Promise(r=>setTimeout(r,750));
          const ns = await refreshStatus();
          if (!ns) continue;
          if (ns.state==="starting") setBootstrapPhase("starting");
          else if (ns.state==="booting") setBootstrapPhase("vm_booting");
          else if (ns.state==="running" && !ns.dockerReady) {
            setBootstrapPhase("docker_starting");
            if (dockerWaitStart===null) dockerWaitStart=Date.now();
            if (Date.now()-dockerWaitStart>30000){ setBootstrapError("Docker Engine did not become ready within 30 seconds."); setBootstrapPhase("failed"); break; }
          } else if (ns.state==="running" && ns.dockerReady){ setBootstrapPhase("ready"); await refreshCounts(); await refreshDocker(); setTimeout(()=>{ refreshContainers(); refreshImages(); refreshVolumes(); refreshNetworks(); },300); break; }
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
  const showBootstrapOverlay = bootstrapPhase!=="ready" && bootstrapPhase!=="failed" && bootstrapPhase!=="stopped";
  const showFailed = bootstrapPhase==="failed";
  const showStoppedPanel = bootstrapPhase==="stopped" && !showBootstrapOverlay && !showFailed;

  return (
    <div className="shell">
      <ShellNav active={active} onNav={handleNav} binaryPath={binaryPath} statusAt={statusAt} />
      <div className="shell-main">
        <header className="shell-header">
          <div className="shell-header__left">
            {badgeStyle && badgeLabel && <span className={"badge "+(badgeStyle as any).cls} style={{ background: badgeStyle.bg, color: badgeStyle.fg, borderColor: "transparent" }}>{badgeLabel}</span>}
            {status?.dockerReady!==undefined && <span className={status.dockerReady ? "badge badge--running" : "badge badge--error"}>Docker: {status.dockerReady ? "ready" : "not ready"}</span>}
            {counts && <span className="text-meta" style={{ color: "var(--text-secondary)", whiteSpace: "nowrap" }}>{counts.running}/{counts.containers} containers • {counts.images} imgs • {counts.volumes} vols • {counts.networks} nets</span>}
          </div>
          <div className="shell-header__right">
            <button onClick={()=>doAction("start", ()=>invoke("start_harpoon"))} disabled={actionInProgress==="start" || bootstrapPhase==="starting" || bootstrapPhase==="vm_booting" || bootstrapPhase==="docker_starting" || isRunning && bootstrapPhase==="ready"} className={isRunning && bootstrapPhase==="ready" ? "btn btn--secondary" : "btn btn--primary"}>{(actionInProgress==="start" || bootstrapPhase==="starting") && <span style={{ width:12, height:12, border:"2px solid currentColor", borderTopColor:"transparent", borderRadius:"50%", display:"inline-block", animation:"spin 0.8s linear infinite" }}></span>}{actionInProgress==="start" ? "Starting…" : "Start"}</button>
            <button onClick={()=>doAction("stop", ()=>invoke("stop_harpoon"))} disabled={actionInProgress==="stop" || isStopped as boolean} className="btn btn--secondary">{actionInProgress==="stop" && <span style={{ width:12, height:12, border:"2px solid currentColor", borderTopColor:"transparent", borderRadius:"50%", display:"inline-block", animation:"spin 0.8s linear infinite" }}></span>}{actionInProgress==="stop" ? "Stopping…" : "Stop"}</button>
            <button onClick={()=>doAction("restart", ()=>invoke("restart_harpoon"))} disabled={actionInProgress==="restart" || bootstrapPhase==="starting" || bootstrapPhase==="vm_booting"} className="btn btn--secondary">{actionInProgress==="restart" && <span style={{ width:12, height:12, border:"2px solid currentColor", borderTopColor:"transparent", borderRadius:"50%", display:"inline-block", animation:"spin 0.8s linear infinite" }}></span>}{actionInProgress==="restart" ? "Restarting…" : "Restart"}</button>
            <button onClick={()=>{ refreshStatus(); refreshCounts(); refreshConfig(); refreshDocker(); if(active==="containers") refreshContainers(); if(active==="images") refreshImages(); if(active==="volumes") refreshVolumes(); if(active==="networks") refreshNetworks(); if(active==="diagnostics"){refreshDoctor(); refreshLogs();} }} className="btn btn--secondary">Refresh</button>
          </div>
        </header>
        {actionError && !showFailed && <div style={{ background: "var(--error-bg)", border: "1px solid #7F1D1D", color: "var(--error-fg)", padding: 10, borderRadius: 8, marginBottom: 12, fontSize: 13 }}>{actionError}</div>}
        {statusError && !showFailed && <div style={{ background: "var(--error-bg)", border: "1px solid #7F1D1D", color: "var(--error-fg)", padding: 10, borderRadius: 8, marginBottom: 12, fontSize: 13 }}>status error: {statusError}</div>}
        {actionInProgress && <div style={{ fontSize: 12, color: "var(--text-secondary)", marginBottom: 8 }}>{actionInProgress} in progress…</div>}

        {showBootstrapOverlay && (
          <div style={{ background:"white", border:"1px solid #e5e7eb", borderRadius:12, padding:"40px 24px", display:"flex", flexDirection:"column", alignItems:"center", justifyContent:"center", minHeight:320, textAlign:"center" }}>
            <div style={{ fontWeight:700, fontSize:20, marginBottom:12 }}>Harpoon</div>
            <div style={{ width:32, height:32, border:"3px solid #e5e7eb", borderTopColor:"#111827", borderRadius:"50%", animation:"spin 0.9s linear infinite", marginBottom:16 }}></div>
            <div style={{ fontWeight:600, fontSize:14, marginBottom:6 }}>{phaseLabel(bootstrapPhase)}</div>
            <div style={{ fontSize:12, color:"#6b7280", marginBottom:4 }}>{phaseDetail(bootstrapPhase, status)}</div>
            <div style={{ fontSize:11, color:"#9ca3af" }}>{status ? `${status.state} • ${status.dockerReady? "Docker ready":"Docker starting"} • ${binaryPath ? "binary resolved":"resolving…"}` : "Checking runtime…"}</div>
            <style>{`@keyframes spin { to { transform: rotate(360deg); } }`}</style>
          </div>
        )}

        {showFailed && (
          <div style={{ background:"white", border:"1px solid #fecaca", borderRadius:12, padding:24, minHeight:320 }}>
            <div style={{ fontWeight:700, fontSize:16, color:"#991b1b", marginBottom:8 }}>Harpoon could not start</div>
            <div style={{ fontSize:13, color:"#374151", marginBottom:4 }}>Phase: <code style={{ background:"#f3f4f6", padding:"2px 6px", borderRadius:6 }}>{bootstrapPhase}</code> {status && <span style={{ fontSize:12, color:"#6b7280" }}>• state {status.state}</span>}</div>
            <div style={{ fontSize:13, color:"#991b1b", background:"#fef2f2", border:"1px solid #fecaca", borderRadius:8, padding:10, marginBottom:12, whiteSpace:"pre-wrap", wordBreak:"break-word" }}>{bootstrapError || actionError || "Unknown error"}</div>
            <div style={{ display:"flex", gap:8 }}>
              <button onClick={()=>runBootstrap(true)} style={{ padding:"8px 14px", borderRadius:8, background:"#111827", color:"white", border:"1px solid #111827", fontSize:13 }}>Retry</button>
              <button onClick={()=>handleNav("diagnostics")} style={{ padding:"8px 14px", borderRadius:8, background:"white", border:"1px solid #d1d5db", fontSize:13 }}>Diagnostics</button>
              <button onClick={()=>{ refreshStatus(); setBootstrapPhase("discovering"); runBootstrap(true); }} style={{ padding:"8px 14px", borderRadius:8, background:"white", border:"1px solid #d1d5db", fontSize:13 }}>Check again</button>
            </div>
            <div style={{ fontSize:11, color:"#6b7280", marginTop:12 }}>Binary: <code>{binaryPath || "—"}</code> • Log: <code>{logPath || status?.logPath || "—"}</code></div>
          </div>
        )}

        {showStoppedPanel && (
          <div style={{ background:"white", border:"1px solid #e5e7eb", borderRadius:12, padding:24, marginBottom:12 }}>
            <div style={{ fontWeight:600, fontSize:14, marginBottom:6 }}>Harpoon is stopped</div>
            <div style={{ fontSize:13, color:"#6b7280", marginBottom:12 }}>Press Start to launch the Linux VM and Docker Engine.</div>
            <button onClick={()=>doAction("start", ()=>invoke("start_harpoon"))} style={{ padding:"8px 14px", borderRadius:8, background:"#111827", color:"white", border:"none", fontSize:13 }}>Start Harpoon</button>
          </div>
        )}

        {!showBootstrapOverlay && !showFailed && (
          <>
            {active==="overview" && <OverviewView status={status} statusAt={statusAt} dockerVersion={dockerVersion} dockerAt={dockerAt} counts={counts} countsAt={countsAt} config={config} configAt={configAt} doctor={doctor} doctorAt={doctorAt} binaryPath={binaryPath} logPath={logPath} bootstrapPhase={bootstrapPhase} onOpenDiagnostics={()=>handleNav("diagnostics")} />}
            {active==="resources" && <ResourcesView config={config} configAt={configAt} status={status} binaryPath={binaryPath} actionInProgress={actionInProgress} onSetCpus={(v)=>doAction("cpus", ()=>invoke("set_cpus", { cpus: v }))} onSetMemory={(v)=>doAction("memory", ()=>invoke("set_memory", { memory: v }))} />}
            {active==="diagnostics" && <DiagnosticsView binaryPath={binaryPath} logs={logs} logsAt={logsAt} logsRefreshing={logsShelf.refreshing} logsError={logsShelf.error} logPath={logPath} doctor={doctor} doctorAt={doctorAt} doctorRefreshing={doctorShelf.refreshing} doctorError={doctorShelf.error} status={status} configRaw={config?.raw ?? null} bootstrapPhase={bootstrapPhase} onCopy={handleCopyDiagnostics} copied={copied} onRefreshDoctor={()=>doctorShelf.refresh()} onRefreshLogs={()=>logsShelf.refresh()} />}
            {active==="containers" && <ContainersView containers={containersShelf.data ?? []} refreshing={containersShelf.refreshing} error={containersShelf.error} status={status} updatedAt={containersShelf.updatedAt} hasCache={containersShelf.hasCache} actionInProgress={actionInProgress} doAction={doAction} refresh={()=>containersShelf.refresh()} />}
            {active==="images" && <ImagesView images={imagesShelf.data ?? []} refreshing={imagesShelf.refreshing} error={imagesShelf.error} updatedAt={imagesShelf.updatedAt} hasCache={imagesShelf.hasCache} actionInProgress={actionInProgress} doAction={doAction} refresh={()=>imagesShelf.refresh()} />}
            {active==="volumes" && <VolumesView volumes={volumesShelf.data ?? []} refreshing={volumesShelf.refreshing} error={volumesShelf.error} updatedAt={volumesShelf.updatedAt} hasCache={volumesShelf.hasCache} actionInProgress={actionInProgress} doAction={doAction} refresh={()=>volumesShelf.refresh()} />}
            {active==="networks" && <NetworksView networks={networksShelf.data ?? []} refreshing={networksShelf.refreshing} error={networksShelf.error} updatedAt={networksShelf.updatedAt} hasCache={networksShelf.hasCache} actionInProgress={actionInProgress} doAction={doAction} refresh={()=>networksShelf.refresh()} />}
          </>
        )}
      </div>
    </div>
  );
}
