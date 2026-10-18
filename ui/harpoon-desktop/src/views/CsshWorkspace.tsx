import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Command, LockKey, TerminalWindow, Warning, PlugsConnected } from "@phosphor-icons/react";
import { containerLifecycleState } from "../lib/controlScope";
const ShellTab = lazy(() => import("./ShellTab").then((module) => ({ default: module.ShellTab })));

type CsshContainer = { ID?: string; Id?: string; Names?: string; Name?: string; Image?: string; State?: string; Status?: string };
type CommandResult = { containerId: string; status: "success" | "failed" | "skipped" | "timeout"; output: string; exitCode?: number; error?: string };
type CommandRecord = CommandResult & { command: string; timestamp: number; order: number };
const MAX_TRANSCRIPT_COMMANDS = 20;
const MAX_TRANSCRIPT_OUTPUT_BYTES = 256 * 1024;
const transcriptBytes = (value: string) => new TextEncoder().encode(value).byteLength;
const containerId = (container: CsshContainer) => container.ID || container.Id || container.Names || container.Name || "unknown";
const containerName = (container: CsshContainer) => container.Names || container.Name || containerId(container);
const hasFullContainerId = (id: string) => id.length === 64 && /^[a-f\d]+$/i.test(id);

export function CsshWorkspace({ project, containers }: { project: string; containers: CsshContainer[] }) {
  const eligible = useMemo(() => containers.filter((container) => containerLifecycleState(container) === "running" && hasFullContainerId(containerId(container))), [containers]);
  const eligibleIds = useMemo(() => eligible.map(containerId), [eligible]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set(eligibleIds));
  const [command, setCommand] = useState("");
  const [commandMessage, setCommandMessage] = useState<string | null>(null);
  const [commandPending, setCommandPending] = useState(false);
  const [transcripts, setTranscripts] = useState<Record<string, CommandRecord[]>>({});
  const [tileModes, setTileModes] = useState<Record<string, "shell" | "result">>({});
  const [connectAllRequest, setConnectAllRequest] = useState(0);
  const [shellStates, setShellStates] = useState<Record<string, string>>({});
  const previousEligibleIds = useRef(eligibleIds);
  const commandInputRef = useRef<HTMLInputElement>(null);
  const commandPendingRef = useRef(false);
  const transcriptOrderRef = useRef(0);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  useEffect(() => {
    const previous = new Set(previousEligibleIds.current);
    setSelected((current) => {
      const wasAllSelected = previous.size > 0 && [...previous].every((id) => current.has(id));
      return new Set(eligibleIds.filter((id) => current.has(id) || (wasAllSelected && !previous.has(id))));
    });
    previousEligibleIds.current = eligibleIds;
  }, [eligibleIds.join("|")]);

  const selectedContainers = eligible.filter((container) => selected.has(containerId(container)));
  const selectedNames = selectedContainers.map(containerName);
  const toggle = (id: string) => {
    setCommandMessage(null);
    setSelected((current) => { const next = new Set(current); if (next.has(id)) next.delete(id); else next.add(id); return next; });
  };
  const selectAll = () => { setCommandMessage(null); setSelected(new Set(eligibleIds)); };
  const connectableCount = eligible.filter(({ ID, Id, Names, Name }) => {
    const id = ID || Id || Names || Name || "unknown";
    return shellStates[id] !== "connected" && shellStates[id] !== "connecting";
  }).length;
  const connectAll = () => { setCommandMessage(null); setConnectAllRequest((value) => value + 1); };
  const preview = selectedNames.length > 3 ? `${selectedNames.slice(0, 3).join(", ")} +${selectedNames.length - 3}` : selectedNames.join(", ");
  const runCommand = async () => {
    if (commandPendingRef.current) return;
    const dispatchCommand = command.trim();
    const dispatchTargets = selectedContainers.map((container) => ({ containerId: containerId(container), context: "harpoon" }));
    if (!dispatchCommand) { setCommandMessage("Enter a command to run."); return; }
    if (!dispatchTargets.length) { setCommandMessage("Select at least one eligible running container."); return; }
    commandPendingRef.current = true;
    setCommandPending(true); setCommandMessage(`Running on ${dispatchTargets.length} selected container${dispatchTargets.length === 1 ? "" : "s"}…`); setTileModes({}); setCommand("");
    try {
      const results = await invoke<CommandResult[]>("run_universal_command", { command: dispatchCommand, targets: dispatchTargets });
      if (!mountedRef.current) return;
      const timestamp = Date.now();
      const records = results.map((result) => ({ ...result, command: dispatchCommand, timestamp, order: transcriptOrderRef.current++ }));
      setTranscripts((current) => {
        const next = { ...current };
        for (const record of records) {
          const history = [...(next[record.containerId] ?? []), record];
          let outputBytes = history.reduce((total, item) => total + transcriptBytes(item.output) + transcriptBytes(item.error ?? ""), 0);
          while (history.length > MAX_TRANSCRIPT_COMMANDS || outputBytes > MAX_TRANSCRIPT_OUTPUT_BYTES) {
            const removed = history.shift();
            outputBytes -= removed ? transcriptBytes(removed.output) + transcriptBytes(removed.error ?? "") : 0;
          }
          next[record.containerId] = history;
        }
        return next;
      });
      setTileModes(Object.fromEntries(results.map((result) => [result.containerId, "result" as const])));
      const failures = results.filter((result) => result.status !== "success").length;
      setCommandMessage(failures ? `Finished with ${failures} of ${results.length} target${results.length === 1 ? "" : "s"} needing attention.` : `Finished successfully on ${results.length} container${results.length === 1 ? "" : "s"}.`);
    } catch (error) { if (mountedRef.current) { setCommand(dispatchCommand); setCommandMessage(`Command could not run: ${error instanceof Error ? error.message : String(error)}`); } }
    finally { commandPendingRef.current = false; if (mountedRef.current) { setCommandPending(false); commandInputRef.current?.focus(); } }
  };

  return <section className="cssh-workspace" aria-labelledby="cssh-title">
    <div className="cssh-workspace__header"><div><div className="overview-eyebrow"><Command size={13} aria-hidden="true" /> Stack-level workspace</div><h3 id="cssh-title">CSSH <span>· cluster shell</span></h3><p>Select shell recipients from the tile headers. Terminal surfaces stay observational in this prototype.</p></div><div className="cssh-workspace__controls"><span className="cssh-count" role="status" aria-live="polite"><strong>{selectedContainers.length}</strong> / {eligible.length} selected</span><button className="btn btn--secondary btn--sm" type="button" onClick={selectAll} disabled={!eligible.length}><Check size={13} aria-hidden="true" /> All</button><button className="btn btn--primary btn--sm" type="button" onClick={connectAll} disabled={!connectableCount}><PlugsConnected size={13} aria-hidden="true" /> {connectableCount ? "Connect all" : eligible.length ? "All connected" : "Connect all"}</button></div></div>
    <div className="cssh-boundary-note"><LockKey size={14} aria-hidden="true" /><span><strong>Separate execution.</strong> Commands run through Docker per selected container and never write into interactive PTY sessions.</span></div>
    <div className="cssh-grid" aria-label={`${project} CSSH shell tiles`}>
      {containers.map((container) => { const id = containerId(container); const name = containerName(container); const state = containerLifecycleState(container); const isRunning = state === "running"; const isEligible = isRunning && hasFullContainerId(id); const isSelected = selected.has(id); const history = transcripts[id] ?? []; const latest = history[history.length - 1]; const mode = tileModes[id] ?? "shell"; return <article className={`cssh-tile${isSelected ? " is-selected" : ""}${!isEligible ? " is-ineligible" : ""}`} key={id}>
        <header className="cssh-tile__header"><label className="cssh-tile__selector"><input type="checkbox" checked={isSelected} disabled={!isEligible} onChange={() => toggle(id)} aria-label={`${isSelected ? "Remove" : "Add"} ${name} ${isSelected ? "from" : "to"} CSSH recipients`} /><span className="cssh-tile__check" aria-hidden="true"><Check size={11} weight="bold" /></span><span className="cssh-tile__identity"><strong title={name}>{name}</strong><span title={container.Image}>{container.Image || "image unavailable"}</span><span title={id}>{id.slice(0, 12)} · Docker / harpoon</span></span></label><div className="cssh-tile__header-actions"><span className={`cssh-tile__status cssh-tile__status--${isEligible ? "running" : !isRunning ? state : "unsupported"}`}><i aria-hidden="true" />{isEligible ? "ready" : !isRunning ? state : "unsupported"}</span>{isEligible && <div className="cssh-tile__mode" role="group" aria-label={`${name} tile view`}><button type="button" className={mode === "shell" ? "is-active" : ""} aria-pressed={mode === "shell"} onClick={() => setTileModes((current) => ({ ...current, [id]: "shell" }))}>Shell</button><button type="button" className={mode === "result" ? "is-active" : ""} aria-pressed={mode === "result"} onClick={() => latest && setTileModes((current) => ({ ...current, [id]: "result" }))} disabled={!latest}>Command result</button></div>}</div></header>
        <div className={`cssh-tile__view${mode === "result" ? " cssh-tile__view--hidden" : ""}`} aria-hidden={mode === "result"}>
          {isEligible ? <Suspense fallback={<div className="cssh-terminal" role="status">Loading terminal…</div>}><ShellTab context="harpoon" containerId={id} running connectRequest={connectAllRequest} onStateChange={(nextState) => setShellStates((current) => current[id] === nextState ? current : { ...current, [id]: nextState })} /></Suspense> : <div className="cssh-terminal" aria-label={`${name} shell unavailable`}><div className="cssh-terminal__toolbar"><span><TerminalWindow size={13} aria-hidden="true" /> shell</span><span className="mono">unavailable</span></div><pre><span className="cssh-terminal__prompt">#</span> {!isRunning ? "Container is not running." : "Full container ID unavailable."}</pre><span className="cssh-terminal__hint">{!isRunning ? "Start this container to make its shell eligible." : "Refresh Docker metadata before connecting."}</span></div>}
        </div>
        {latest && <div className={`cssh-tile__view cssh-tile__result-view${mode !== "result" ? " cssh-tile__view--hidden" : ""}`} aria-hidden={mode !== "result"}><div className="cssh-result__toolbar"><strong>Command transcript</strong><span>{history.length} of {MAX_TRANSCRIPT_COMMANDS} · independent runs</span></div><div className="cssh-result" role="log" aria-label={`Command transcript for ${name}`}>{[...history].reverse().map((result) => <article className={`cssh-result__entry cssh-result--${result.status}`} key={result.order}><div className="cssh-result__entry-header"><code>$ {result.command}</code><span>{new Date(result.timestamp).toLocaleTimeString([], { hour: "numeric", minute: "2-digit", second: "2-digit" })} · {result.status}{result.exitCode !== undefined ? ` · exit ${result.exitCode}` : ""}</span></div>{result.error && <p>{result.error}</p>}{result.output ? <pre>{result.output}</pre> : !result.error && <p>No output.</p>}</article>)}</div></div>}
      </article>; })}
    </div>
    <form className="cssh-composer" onSubmit={(event) => { event.preventDefault(); void runCommand(); }}><div className="cssh-composer__label"><span className="overview-eyebrow">Universal command</span><span>Run on {selectedContainers.length} selected · {preview || "no recipients"}</span></div><div className="cssh-composer__row"><span className="cssh-composer__prompt" aria-hidden="true">$</span><input ref={commandInputRef} value={command} onChange={(event) => { setCommand(event.target.value); setCommandMessage(null); }} aria-label="Universal command" placeholder="e.g. uptime" spellCheck={false} disabled={commandPending} /><button className="btn btn--primary" type="submit" disabled={commandPending || !selectedContainers.length}>{commandPending ? "Running…" : `Run on ${selectedContainers.length}`}</button></div>{commandMessage && <div className="cssh-composer__message" role="status" aria-live="polite" aria-atomic="true"><Warning size={14} aria-hidden="true" />{commandMessage}</div>}</form>
  </section>;
}
