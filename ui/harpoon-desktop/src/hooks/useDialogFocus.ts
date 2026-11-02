import { useEffect, useRef } from "react";
import type { RefObject } from "react";

/**
 * Focus handling for a modal dialog: put focus on its first field ONCE, when it opens, and close on Escape.
 *
 * The dialog re-renders whenever the view does (the deployment snapshot is refreshed every few seconds), and
 * `onClose` is a new function on every render. Neither may touch focus: pulling focus back to the first field
 * would end whatever the person is doing in another field and dismiss an open native dropdown. `onClose` is read
 * through a ref, so Escape always calls the latest one without re-running the effect.
 */
export function useDialogFocus(ref: RefObject<HTMLElement>, onClose: () => void) {
  const latest = useRef(onClose);
  latest.current = onClose;
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>("input, select, button.btn--primary, button")?.focus();
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") latest.current(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [ref]);
}
