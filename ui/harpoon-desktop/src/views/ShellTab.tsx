import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { shellSessions, type ShellSessionStore, type SharedShellEvent } from "../lib/shellSessionStore";

type ShellEvent = SharedShellEvent;
type ShellState = "disconnected" | "connecting" | "connected" | "exited" | "error";
const MAX_PENDING_OUTPUT = 512 * 1024;

export function shellDimensions(cols: number, rows: number) {
  return { cols, rows };
}

export function shellEventForSession(event: ShellEvent, sessionId: string | null) {
  return Boolean(sessionId && event.sessionId === sessionId);
}

export function ShellTab({ context, containerId, running, executable = "/bin/sh", user = "default", connectRequest = 0, onStateChange }: { context: string; containerId: string; running: boolean; executable?: string; user?: string; connectRequest?: number; onStateChange?: (state: ShellState) => void }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const handleRef = useRef<Awaited<ReturnType<ShellSessionStore["attach"]>> | null>(null);
  const [state, setState] = useState<ShellState>("disconnected");
  const [message, setMessage] = useState("");
  const [connectNonce, setConnectNonce] = useState(0);

  useEffect(() => { onStateChange?.(state); }, [onStateChange, state]);

  const closeSession = useCallback(async () => { await handleRef.current?.close(); handleRef.current = null; setState("disconnected"); }, []);

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
    let pending: string[] = [];
    let pendingBytes = 0;
    let frame: number | null = null;
    const flush = () => { frame = null; if (!disposed) { terminal.write(pending.join("")); pending = []; pendingBytes = 0; } };
    const queueOutput = (data: string) => {
      pending.push(data); pendingBytes += data.length;
      while (pendingBytes > MAX_PENDING_OUTPUT && pending.length > 1) pendingBytes -= pending.shift()?.length ?? 0;
      if (frame === null) frame = requestAnimationFrame(flush);
    };
    const start = async () => {
      setState("connecting"); setMessage(""); terminal.reset();
      try {
        const handleEvent = (payload: ShellEvent) => {
          if (payload.kind === "output") queueOutput(payload.data ?? "");
          if (payload.kind === "error") { setState("error"); setMessage(payload.error ?? "The shell failed to start."); }
          if (payload.kind === "exit") { handleRef.current = null; setState("exited"); setMessage(`Shell exited${typeof payload.code === "number" ? ` with code ${payload.code}` : ""}.`); }
        };
        if (host.clientWidth > 0 && host.clientHeight > 0) fitRef.current?.fit();
        const handle = await shellSessions.attach({ context, containerId, executable, user }, terminal.cols, terminal.rows, handleEvent);
        if (disposed) { await handle.close(); return; }
        handleRef.current = handle; setState("connected"); terminal.focus();
      } catch (error) { setState("error"); setMessage(error instanceof Error ? error.message : String(error)); }
    };
    void start();
    const dataDisposable = terminal.onData((input) => { if (handleRef.current) void handleRef.current.write(input).catch((error) => setMessage(String(error))); });
    const resizeDisposable = terminal.onResize(({ cols, rows }) => { if (handleRef.current) void handleRef.current.resize(cols, rows).catch(() => undefined); });
    let resizeFrame: number | null = null;
    const resizeObserver = new ResizeObserver(() => {
      if (resizeFrame !== null) cancelAnimationFrame(resizeFrame);
      resizeFrame = requestAnimationFrame(() => { resizeFrame = null; if (!disposed && host.clientWidth > 0 && host.clientHeight > 0) fitRef.current?.fit(); });
    });
    resizeObserver.observe(host);
    return () => { disposed = true; dataDisposable.dispose(); resizeDisposable.dispose(); resizeObserver.disconnect(); if (frame !== null) cancelAnimationFrame(frame); if (resizeFrame !== null) cancelAnimationFrame(resizeFrame); handleRef.current?.detach(); handleRef.current = null; };
  }, [connectNonce, containerId, context, executable, running, user]);

  const copySelection = () => { const selection = terminalRef.current?.getSelection() ?? ""; if (selection) void navigator.clipboard.writeText(selection); };
  const pasteClipboard = async () => { const input = await navigator.clipboard.readText().catch(() => ""); if (input && handleRef.current) void handleRef.current.write(input); };
  const connect = () => { if (state === "connected" || state === "connecting") return; setConnectNonce((value) => value + 1); };
  useEffect(() => { if (connectRequest > 0) connect(); }, [connectRequest]);

  const stateLabel = state === "disconnected" ? "Ready to connect" : state[0].toUpperCase() + state.slice(1);
  return <div className="shell-tab" aria-label="Container shell">
    <div className="shell-tab__toolbar"><div className="shell-tab__identity"><span className={`shell-tab__dot shell-tab__dot--${state}`} aria-hidden="true" /><strong>{stateLabel}</strong><span className="text-meta">Docker · {context} · {containerId.slice(0, 12)}</span></div><div className="shell-tab__actions"><button className="btn btn--ghost btn--sm" onClick={copySelection} disabled={state !== "connected"}>Copy selection</button><button className="btn btn--ghost btn--sm" onClick={pasteClipboard} disabled={state !== "connected"}>Paste</button>{state === "connected" && <button className="btn btn--ghost btn--sm" onClick={() => void closeSession()}>Close session</button>}{state !== "connected" && state !== "connecting" && <button className="btn btn--secondary btn--sm" onClick={connect} disabled={!running}>{state === "exited" || state === "error" ? "Reconnect" : "Connect"}</button>}</div></div>
    {message && <div className="shell-tab__message" role="alert">{message}</div>}
    {!running && <div className="shell-tab__empty"><strong>Container is not running</strong><span>Start the container before opening an interactive shell.</span></div>}
    {running && <div ref={hostRef} className="shell-tab__terminal" role="application" aria-label={`Shell for ${containerId.slice(0, 12)}`} />}
    {running && state === "connecting" && <div className="shell-tab__overlay" role="status">Opening interactive shell…</div>}
    {running && state === "disconnected" && <button className="shell-tab__overlay shell-tab__connect-target" type="button" onClick={connect} aria-label={`Connect to interactive shell for ${containerId.slice(0, 12)}`}><strong>Interactive shell</strong><span>Connect to open /bin/sh in this running container.</span><span className="shell-tab__connect-target-action">Connect</span></button>}
    {state === "connected" && <p className="text-meta shell-tab__hint">Select text to copy. Scrollback keeps the latest 2,000 terminal lines.</p>}
  </div>;
}
