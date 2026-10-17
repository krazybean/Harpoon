import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

type ShellEvent = { sessionId: string; kind: "output" | "exit" | "error"; data?: string; code?: number; error?: string };
type ShellState = "disconnected" | "connecting" | "connected" | "exited" | "error";
const MAX_PENDING_OUTPUT = 512 * 1024;

export function shellDimensions(cols: number, rows: number) {
  return { cols, rows };
}

export function shellEventForSession(event: ShellEvent, sessionId: string | null) {
  return Boolean(sessionId && event.sessionId === sessionId);
}

export function ShellTab({ context, containerId, running, connectRequest = 0, onStateChange }: { context: string; containerId: string; running: boolean; connectRequest?: number; onStateChange?: (state: ShellState) => void }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const sessionRef = useRef<string | null>(null);
  const closingRef = useRef(false);
  const [state, setState] = useState<ShellState>("disconnected");
  const [message, setMessage] = useState("");
  const [connectNonce, setConnectNonce] = useState(0);

  useEffect(() => { onStateChange?.(state); }, [onStateChange, state]);

  const closeSession = useCallback(async () => {
    const sessionId = sessionRef.current;
    if (!sessionId || closingRef.current) return;
    closingRef.current = true;
    sessionRef.current = null;
    try { await invoke("close_shell_session", { sessionId }); } catch {}
  }, []);

  useEffect(() => {
    if (!hostRef.current) return;
    const terminal = new Terminal({
      convertEol: true, cursorBlink: true, cursorStyle: "bar", fontFamily: "var(--font-mono)", fontSize: 13, lineHeight: 1.15, scrollback: 2000,
      theme: { background: "#0b0d0f", foreground: "#f4f1e9", cursor: "#f2c94c", cursorAccent: "#0b0d0f", selectionBackground: "rgba(242,201,76,.3)", black: "#0b0d0f", brightBlack: "#7f8a94", yellow: "#f2c94c", brightYellow: "#ffe08a" },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(hostRef.current);
    terminalRef.current = terminal;
    fitRef.current = fit;
    return () => { terminal.dispose(); terminalRef.current = null; fitRef.current = null; };
  }, []);

  useEffect(() => {
    const terminal = terminalRef.current;
    const host = hostRef.current;
    if (!terminal || !host || !running || connectNonce === 0) return;
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    let pending: string[] = [];
    let pendingBytes = 0;
    let frame: number | null = null;
    const earlyEvents = new Map<string, ShellEvent[]>();
    const flush = () => { frame = null; if (!disposed) { terminal.write(pending.join("")); pending = []; pendingBytes = 0; } };
    const queueOutput = (data: string) => {
      pending.push(data); pendingBytes += data.length;
      while (pendingBytes > MAX_PENDING_OUTPUT && pending.length > 1) pendingBytes -= pending.shift()?.length ?? 0;
      if (frame === null) frame = requestAnimationFrame(flush);
    };
    const start = async () => {
      setState("connecting"); setMessage(""); terminal.clear();
      try {
        const handleEvent = (payload: ShellEvent) => {
          if (!shellEventForSession(payload, sessionRef.current)) return;
          if (payload.kind === "output") queueOutput(payload.data ?? "");
          if (payload.kind === "error") { setState("error"); setMessage(payload.error ?? "The shell failed to start."); }
          if (payload.kind === "exit") { sessionRef.current = null; setState("exited"); setMessage(`Shell exited${typeof payload.code === "number" ? ` with code ${payload.code}` : ""}.`); }
        };
        unlisten = await listen<ShellEvent>("shell-event", ({ payload }) => {
          if (!sessionRef.current) {
            const events = earlyEvents.get(payload.sessionId) ?? [];
            if (events.length < 32) events.push(payload);
            earlyEvents.set(payload.sessionId, events);
            return;
          }
          handleEvent(payload);
        });
        if (host.clientWidth > 0 && host.clientHeight > 0) fitRef.current?.fit();
        const session = await invoke<{ sessionId: string }>("open_shell_session", { context, containerId, ...shellDimensions(terminal.cols, terminal.rows) });
        if (disposed) { await invoke("close_shell_session", { sessionId: session.sessionId }).catch(() => undefined); return; }
        sessionRef.current = session.sessionId; closingRef.current = false; setState("connected"); terminal.focus();
        for (const payload of earlyEvents.get(session.sessionId) ?? []) handleEvent(payload);
        earlyEvents.clear();
      } catch (error) { setState("error"); setMessage(error instanceof Error ? error.message : String(error)); }
    };
    void start();
    const dataDisposable = terminal.onData((input) => { const sessionId = sessionRef.current; if (sessionId && !closingRef.current) void invoke("write_shell_input", { sessionId, input }).catch((error) => setMessage(String(error))); });
    const resizeDisposable = terminal.onResize(({ cols, rows }) => { const sessionId = sessionRef.current; if (sessionId && !closingRef.current) void invoke("resize_shell_session", { sessionId, ...shellDimensions(cols, rows) }).catch(() => undefined); });
    let resizeFrame: number | null = null;
    const resizeObserver = new ResizeObserver(() => {
      if (resizeFrame !== null) cancelAnimationFrame(resizeFrame);
      resizeFrame = requestAnimationFrame(() => { resizeFrame = null; if (!disposed && host.clientWidth > 0 && host.clientHeight > 0) fitRef.current?.fit(); });
    });
    resizeObserver.observe(host);
    return () => { disposed = true; dataDisposable.dispose(); resizeDisposable.dispose(); resizeObserver.disconnect(); if (frame !== null) cancelAnimationFrame(frame); if (resizeFrame !== null) cancelAnimationFrame(resizeFrame); if (unlisten) void unlisten(); void closeSession(); };
  }, [closeSession, connectNonce, containerId, context, running]);

  const copySelection = () => { const selection = terminalRef.current?.getSelection() ?? ""; if (selection) void navigator.clipboard.writeText(selection); };
  const pasteClipboard = async () => { const input = await navigator.clipboard.readText().catch(() => ""); const sessionId = sessionRef.current; if (input && sessionId) void invoke("write_shell_input", { sessionId, input }); };
  const connect = () => { if (state === "connected" || state === "connecting") return; setConnectNonce((value) => value + 1); };
  useEffect(() => { if (connectRequest > 0) connect(); }, [connectRequest]);

  const stateLabel = state === "disconnected" ? "Ready to connect" : state[0].toUpperCase() + state.slice(1);
  return <div className="shell-tab" aria-label="Container shell">
    <div className="shell-tab__toolbar"><div className="shell-tab__identity"><span className={`shell-tab__dot shell-tab__dot--${state}`} aria-hidden="true" /><strong>{stateLabel}</strong><span className="text-meta">Docker · {context} · {containerId.slice(0, 12)}</span></div><div className="shell-tab__actions"><button className="btn btn--ghost btn--sm" onClick={copySelection} disabled={state !== "connected"}>Copy selection</button><button className="btn btn--ghost btn--sm" onClick={pasteClipboard} disabled={state !== "connected"}>Paste</button>{state !== "connected" && state !== "connecting" && <button className="btn btn--secondary btn--sm" onClick={connect} disabled={!running}>{state === "exited" || state === "error" ? "Reconnect" : "Connect"}</button>}</div></div>
    {message && <div className="shell-tab__message" role="alert">{message}</div>}
    {!running && <div className="shell-tab__empty"><strong>Container is not running</strong><span>Start the container before opening an interactive shell.</span></div>}
    {running && <div ref={hostRef} className="shell-tab__terminal" role="application" aria-label={`Shell for ${containerId.slice(0, 12)}`} />}
    {running && state === "connecting" && <div className="shell-tab__overlay" role="status">Opening interactive shell…</div>}
    {running && state === "disconnected" && <div className="shell-tab__overlay"><strong>Interactive shell</strong><span>Connect to open /bin/sh in this running container.</span></div>}
    {state === "connected" && <p className="text-meta shell-tab__hint">Select text to copy. Scrollback keeps the latest 2,000 terminal lines.</p>}
  </div>;
}
