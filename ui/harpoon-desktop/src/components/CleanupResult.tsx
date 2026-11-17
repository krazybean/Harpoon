import { figureLines } from "../lib/cleanupResult";
import type { CleanupOutcome } from "../lib/cleanupResult";
import { fmtBytes } from "./StorageBreakdown";

/** One cleanup's result. The status is stated in words (never by colour alone), and the three measurements are always listed apart. */
const STATUS: Record<CleanupOutcome["status"], string> = {
  success: "Completed", nothing: "Nothing to do", partial: "Partly completed", refused: "Not carried out", failed: "Did not complete", unknown: "Result unknown",
};

export function CleanupResult({ outcome, label }: { outcome: CleanupOutcome; label: string }) {
  const bad = outcome.status === "refused" || outcome.status === "failed" || outcome.status === "unknown";
  return (
    <div className="cleanup-result" role={bad ? "alert" : "status"} data-result-kind={outcome.kind} data-result-status={outcome.status} aria-label={`${label} result`}>
      <div><span className={`badge badge--sm ${outcome.status === "success" || outcome.status === "nothing" ? "badge--running" : outcome.status === "partial" ? "badge--warning" : "badge--error"}`}>{STATUS[outcome.status]}</span> <b>{outcome.headline}</b></div>
      {outcome.detail && <div className="text-body" style={{ marginTop: 4, whiteSpace: "pre-wrap" }}>{outcome.detail}</div>}
      <dl className="cleanup-result__figures">
        {figureLines(outcome, fmtBytes).map((l) => <div key={l.key} data-figure={l.key}><dt className="text-meta">{l.title}</dt><dd className="text-body">{l.text}</dd></div>)}
      </dl>
    </div>
  );
}
