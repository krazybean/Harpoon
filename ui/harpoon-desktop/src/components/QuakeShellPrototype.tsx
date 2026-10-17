import { lazy, Suspense, useEffect, useRef, useState, type CSSProperties } from "react";
import { CaretDown, CaretUp, Command, Warning, X } from "@phosphor-icons/react";
import { isQuakeTargetSafe, type QuakeTarget } from "../lib/quakeShellPrototype";
import { inferContainerTechnology } from "../lib/containerTechnology.js";
import { ContainerTechnologyIcon } from "./ContainerTechnologyIcon";
const ShellTab = lazy(() => import("../views/ShellTab").then((module) => ({ default: module.ShellTab })));

export function QuakeShellPrototype({ containers = [] }: { containers?: any[] }) {
  const [open, setOpen] = useState(false);
  const [targetIndex, setTargetIndex] = useState(0);
  const [panelHeight, setPanelHeight] = useState(420);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLElement>(null);
  const liveTargets: QuakeTarget[] = containers.map((container) => ({
    engine: "docker" as const, context: "harpoon", containerId: container.ID || container.Id || "", name: String(container.Names || container.Name || container.ID || "container").replace(/^\//, ""), executable: "/bin/sh", user: container.Config?.User || "default", state: (String(container.State || "").toLowerCase() === "running" ? "ready" : "stale") as QuakeTarget["state"],
    technology: inferContainerTechnology(container),
  })).filter((target) => target.containerId && target.state === "ready");
  const target: QuakeTarget | null = liveTargets[targetIndex] ?? null;

  useEffect(() => {
    if (targetIndex >= liveTargets.length && liveTargets.length > 0) setTargetIndex(0);
  }, [liveTargets.length, targetIndex]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.code === "Backquote" || event.key === "`") && event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey) {
        event.preventDefault();
        setOpen((value) => !value);
      }
      if (event.key === "Escape" && open) {
        event.preventDefault();
        setOpen(false);
        triggerRef.current?.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open]);

  useEffect(() => {
    if (open) panelRef.current?.querySelector<HTMLElement>("button, select")?.focus();
  }, [open]);

  const close = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };
  const toggle = () => open ? close() : setOpen(true);
  const safe = isQuakeTargetSafe(target);

  return <>
    <button ref={triggerRef} className="quake-prototype-trigger" type="button" aria-expanded={open} aria-controls="quake-shell-prototype" onClick={toggle}>
      <Command size={14} aria-hidden="true" /> Quake Shell <kbd>Ctrl+`</kbd> {open ? <CaretUp size={13} aria-hidden="true" /> : <CaretDown size={13} aria-hidden="true" />}
    </button>
    {open && <aside ref={panelRef} id="quake-shell-prototype" className="quake-prototype" aria-label="Quake Shell" style={{ "--quake-height": `${panelHeight}px` } as CSSProperties}>
      <header className="quake-prototype__header">
        <div className="quake-prototype__heading"><div className="overview-eyebrow"><Command size={13} aria-hidden="true" /> Quake Shell</div><h2>Quick console</h2><p>Shared interactive session for a running container.</p></div>
        <button className="btn btn--ghost btn--sm" type="button" aria-label="Close Quake Shell" onClick={close}><X size={16} aria-hidden="true" /> Close</button>
      </header>
      {!liveTargets.length ? <div className="quake-prototype__empty" role="status"><strong>No running containers available</strong><span>Start a container to open a shared interactive shell.</span></div> : <>
        <div className="quake-prototype__scope" aria-label="Shell target configuration">
          <div className="quake-prototype__target">
            <label htmlFor="quake-target">Target</label>
            <div className="quake-prototype__target-control">
              {target && <span className="quake-prototype__target-icon" role="img" aria-label={`${target.technology.label} technology`} title={`${target.technology.label} image`}><ContainerTechnologyIcon technology={target.technology} size={17} /></span>}
              <select id="quake-target" value={targetIndex} onChange={(event) => setTargetIndex(Number(event.target.value))} aria-label="Quake Shell target">
                {liveTargets.map((candidate, index) => <option key={candidate.containerId} value={index}>{candidate.name} · {candidate.technology.label} · {candidate.context}</option>)}
              </select>
            </div>
          </div>
          {target && <dl className="quake-prototype__metadata"><div><dt>Engine</dt><dd>{target.engine} / {target.context}</dd></div><div><dt>Container</dt><dd title={target.containerId}>{target.name} · {target.containerId.slice(0, 12)}</dd></div><div><dt>Exec</dt><dd>{target.executable}</dd></div><div><dt>User</dt><dd>{target.user}</dd></div></dl>}
        </div>
        <div className="quake-prototype__session-bar">
          <span className="quake-prototype__session-label" role={safe ? "status" : undefined}><span className="quake-prototype__session-dot" aria-hidden="true" /> {safe ? "Connected" : "Session"}</span>
          {!safe && <span className="quake-prototype__stale" role="alert"><Warning size={15} aria-hidden="true" /><span><strong>Target is stale.</strong> Re-select the current container before native input is enabled.</span></span>}
        </div>
        {safe && target && <div className="quake-prototype__terminal quake-prototype__terminal--live"><Suspense fallback={<div className="quake-prototype__terminal-loading" role="status">Opening terminal…</div>}><ShellTab context={target.context} containerId={target.containerId} executable={target.executable} user={target.user} running connectRequest={1} /></Suspense></div>}
      </>}
      <div className="quake-prototype__resize">
        <label htmlFor="quake-height">Panel height</label>
        <input id="quake-height" type="range" min="320" max="600" step="10" value={panelHeight} onChange={(event) => setPanelHeight(Number(event.target.value))} aria-valuetext={`${panelHeight} pixels`} />
        <span>{panelHeight}px</span>
      </div>
      <div className="quake-prototype__footer"><span>Hiding detaches the view without closing the session.</span><span>Close session is explicit.</span></div>
    </aside>}
  </>;
}
