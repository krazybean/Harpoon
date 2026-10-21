import { useEffect, useRef, useSyncExternalStore } from "react";

// WKWebView (Tauri on macOS) implements no window.confirm panel: it returns false
// without showing anything, which silently cancelled every Stop and Restart.
// Lifecycle confirmations use this in-app dialog instead.
type Pending = { message: string; confirmLabel: string; resolve: (ok: boolean) => void };
let pending: Pending | null = null;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((listener) => listener());
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };

export function confirmAction(message: string, confirmLabel: string): Promise<boolean> {
  pending?.resolve(false);
  return new Promise((resolve) => { pending = { message, confirmLabel, resolve }; emit(); });
}

function settle(ok: boolean) {
  const current = pending;
  pending = null;
  emit();
  current?.resolve(ok);
}

export function ConfirmDialogHost() {
  const current = useSyncExternalStore(subscribe, () => pending);
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { if (current) cancelRef.current?.focus(); }, [current]);
  if (!current) return null;
  return (
    <div className="dialog-scrim" role="presentation" onClick={() => settle(false)}>
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-action-title" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => { if (e.key === "Escape") settle(false); }}>
        <div id="confirm-action-title" className="dialog-title">{current.confirmLabel}?</div>
        <div className="dialog-body">{current.message}</div>
        <div className="dialog-actions">
          <button ref={cancelRef} onClick={() => settle(false)} className="btn btn--secondary">Cancel</button>
          <button onClick={() => settle(true)} className="btn btn--destructive">{current.confirmLabel}</button>
        </div>
      </div>
    </div>
  );
}
