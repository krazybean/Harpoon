import type { LogLine } from "./deployTypes.ts";

export function visibleLogLines(lines: LogLine[], filter: string): LogLine[] {
  const query = filter.trim().toLowerCase();
  return query ? lines.filter((line) => line.text.toLowerCase().includes(query)) : lines;
}

/**
 * Put text on the clipboard. The async clipboard API is tried first; WKWebView refuses it outside a user gesture or
 * a secure origin, so a refusal falls back to the selection-based copy, which works inside a click. Rejects only when both fail.
 */
export async function copyText(text: string): Promise<void> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch { /* fall through to the legacy copy */ }
  const area = typeof document === "undefined" ? null : document.createElement("textarea");
  if (!area) throw new Error("The clipboard is unavailable in this window.");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  try {
    if (!document.execCommand("copy")) throw new Error("The clipboard is unavailable in this window.");
  } finally {
    area.remove();
  }
}
