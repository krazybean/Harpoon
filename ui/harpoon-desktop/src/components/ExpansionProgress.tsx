import { expansionPanel, expansionStages, formatSize, interruptedText } from "../lib/diskCapacity";
import type { ExpansionState } from "../lib/diskCapacity";

/**
 * Where a disk expansion is, taken from the runtime's own record (`status.diskExpansion`) on every status poll, so a window that opens
 * or reconnects mid-expansion shows the same stage as the one that started it. Indeterminate on purpose: the runtime reports stages,
 * not a percentage. It disappears when the record is cleared after a verified success; an interrupted record stays, with what to do.
 */
export function ExpansionProgress({ expansion }: { expansion: (ExpansionState & { fromBytes?: number; toBytes?: number }) | null | undefined }) {
  const kind = expansionPanel(expansion);
  if (kind === "none" || !expansion) return null;
  if (kind === "interrupted") {
    return (
      <div role="alert" className="disk-notice disk-notice--critical">
        <strong>A disk expansion did not finish.</strong>
        <div style={{ whiteSpace: "pre-wrap", marginTop: 4 }}>{interruptedText(expansion)}</div>
      </div>
    );
  }
  const stages = expansionStages(expansion);
  const span = expansion.fromBytes && expansion.toBytes ? ` from ${formatSize(expansion.fromBytes)} to ${formatSize(expansion.toBytes)}` : "";
  return (
    <div role="status" aria-live="polite" className="disk-notice expansion-progress">
      <strong>Expanding the Docker disk{span}</strong>
      <progress aria-label="Disk expansion in progress" className="expansion-progress__bar" />
      <ol className="expansion-progress__stages">
        {stages.map((s) => (
          <li key={s.label} className={`expansion-progress__stage expansion-progress__stage--${s.state}`} aria-current={s.state === "active" ? "step" : undefined}>
            <span aria-hidden="true">{s.state === "done" ? "✓" : s.state === "active" ? "●" : "○"}</span> {s.label}
          </li>
        ))}
      </ol>
      <div className="text-meta">Running containers are interrupted. Start, Stop and Restart are unavailable until this finishes; Harpoon starts again by itself.</div>
    </div>
  );
}
