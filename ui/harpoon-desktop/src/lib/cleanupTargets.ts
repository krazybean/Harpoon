// Where each existing cleanup control lives on the Resources page. The Storage Breakdown only moves focus here: it starts nothing,
// and every action still goes through the existing control and its own confirmation.
export type CleanupTarget = "old-releases" | "build-cache" | "disk-blocks";

export const CLEANUP_TARGETS: Record<CleanupTarget, { section: string; button: string }> = {
  "old-releases": { section: "cleanup-old-releases", button: "cleanup-review-releases" },
  "build-cache": { section: "cleanup-build-cache", button: "cleanup-check-cache" },
  "disk-blocks": { section: "cleanup-disk-blocks", button: "cleanup-reclaim" },
};

/** Focus the control, or its section when the control is unavailable right now (the section then shows why). Returns what took focus. */
export function focusCleanup(target: CleanupTarget, doc: Document = document): "button" | "section" | null {
  const ids = CLEANUP_TARGETS[target];
  const button = doc.getElementById(ids.button) as HTMLButtonElement | null;
  const section = doc.getElementById(ids.section);
  const el = button && !button.disabled ? button : section;
  if (!el) return null;
  el.scrollIntoView?.({ block: "center" });
  el.focus({ preventScroll: true });
  return el === button ? "button" : "section";
}
