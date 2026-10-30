import { useEffect, useRef } from "react";
import type { ReactNode } from "react";
import { ArrowCounterClockwise, Check, Info, Pause, Warning, X, CaretRight } from "@phosphor-icons/react";
import type { LogLine, StepRecord } from "../../lib/deployTypes.ts";
import type { Tone } from "../../lib/deployPresentation.ts";
import { EVIDENCE_NOTE, stepLabel } from "../../lib/deployPresentation.ts";

const PILL: Record<Tone, string> = { ok: "dp-pill--ok", err: "dp-pill--err", warn: "dp-pill--warn", info: "dp-pill--info", neutral: "" };

export function Pill({ tone = "neutral", live, icon, children }: { tone?: Tone; live?: boolean; icon?: ReactNode; children: ReactNode }) {
  return <span className={`dp-pill ${PILL[tone]} ${live ? "dp-pill--live" : ""}`}>{icon}{children}</span>;
}

export function Tag({ children }: { children: ReactNode }) {
  return <span className="dp-tag">{children}</span>;
}

export function Dot({ tone }: { tone: Tone }) {
  return <span className={`dp-statusdot ${tone === "neutral" ? "" : `dp-statusdot--${tone}`}`} />;
}

export function Banner({ tone, title, children, actions, icon }: { tone: "info" | "warn" | "err" | "ok"; title: ReactNode; children: ReactNode; actions?: ReactNode; icon?: ReactNode }) {
  const defaultIcon = tone === "info" || tone === "ok" ? <Info size={18} /> : <Warning size={18} />;
  return (
    <div className={`dp-banner dp-banner--${tone}`} role={tone === "err" ? "alert" : "status"}>
      {icon ?? defaultIcon}
      <div><h3>{title}</h3><p>{children}</p></div>
      {actions ? <div className="dp-banner__actions">{actions}</div> : null}
    </div>
  );
}

export function Evidence({ children }: { children?: ReactNode }) {
  return <span className="dp-evidence"><Info size={12} />{children ?? EVIDENCE_NOTE}</span>;
}

function stepIcon(status: StepRecord["status"]) {
  switch (status) {
    case "succeeded": return <Check size={12} weight="bold" />;
    case "failed": return <X size={12} weight="bold" />;
    case "warning": return <Warning size={12} weight="bold" />;
    case "skipped": return <CaretRight size={12} />;
    case "running": return <ArrowCounterClockwise size={12} />;
    default: return null;
  }
}
const STEP_CLASS: Record<StepRecord["status"], string> = { succeeded: "done", failed: "fail", running: "run", warning: "warn", skipped: "skip", pending: "" };

export function Steps({ steps }: { steps: StepRecord[] }) {
  return (
    <div className="dp-steps" role="list">
      {steps.map((s) => (
        <div className={`dp-step dp-step--${STEP_CLASS[s.status]}`} key={s.name} role="listitem" aria-label={`${stepLabel(s.name)}: ${s.status}`}>
          <div className="dp-step__ico">{stepIcon(s.status)}</div>
          <div><b>{stepLabel(s.name)}</b><small>{s.note || (s.status === "pending" ? "waiting" : "")}</small></div>
          <time>{s.durationLabel}</time>
        </div>
      ))}
    </div>
  );
}

const REDACT = /\[REDACTED:[A-Z0-9_]+\]/g;
function LogText({ text }: { text: string }) {
  const parts = text.split(REDACT);
  const marks = text.match(REDACT) ?? [];
  return <span>{parts.map((p, i) => <span key={i}>{p}{marks[i] ? <span className="dp-redact">{marks[i]}</span> : null}</span>)}</span>;
}

export function LogPanel({ lines, bar, filter = "", follow = false, label }: { lines: LogLine[]; bar?: ReactNode; filter?: string; follow?: boolean; label: string }) {
  const body = useRef<HTMLDivElement>(null);
  useEffect(() => { if (follow && body.current) body.current.scrollTop = body.current.scrollHeight; }, [lines.length, follow]);
  const shown = filter ? lines.filter((l) => l.text.toLowerCase().includes(filter.toLowerCase())) : lines;
  return (
    <div className="dp-log" role="log" aria-label={label}>
      <div className="dp-log__bar">{bar}</div>
      <div className="dp-log__body" ref={body}>
        {shown.length === 0 ? <div className="dp-log__line"><time /><span className="dp-faint">No log lines{filter ? " match the filter" : " yet"}.</span></div> : shown.map((l, i) => (
          <div className={`dp-log__line ${l.kind}`} key={i}><time>{String(i + 1).padStart(3, "0")}</time><LogText text={l.text} /></div>
        ))}
      </div>
    </div>
  );
}

export function sparkPath(seed: number, hi = 1): { line: string; area: string } {
  let v = 0.5; let s = seed; const pts: number[] = [];
  for (let i = 0; i < 36; i++) { s = (s * 9301 + 49297) % 233280; v = Math.max(0.08, Math.min(0.95, v + (s / 233280 - 0.5) * 0.28)); pts.push(v * hi); }
  const w = 200, h = 56;
  const d = pts.map((p, i) => `${i ? "L" : "M"}${((i / 35) * w).toFixed(1)},${(h - p * (h - 6) - 3).toFixed(1)}`).join("");
  return { line: d, area: `${d}L${w},${h}L0,${h}Z` };
}

export function Spark({ seed, hi = 1 }: { seed: number; hi?: number }) {
  const { line, area } = sparkPath(seed, hi);
  return <svg className="dp-spark" viewBox="0 0 200 56" preserveAspectRatio="none" role="img" aria-label="illustrative trend (simulated)"><path className="area" d={area} /><path className="line" d={line} /></svg>;
}

export function PausedIcon() {
  return <Pause size={18} />;
}
