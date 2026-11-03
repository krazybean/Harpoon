// Where "has this person seen the walkthrough" is remembered. The key carries the tour's version: a revised tour gets a
// new version, so it is offered once more deliberately, and an older record is simply never read.
export const DEPLOYMENTS_TOUR_VERSION = 1;
export const DEPLOYMENTS_TOUR_KEY = `harpoon.onboarding.deployments.v${DEPLOYMENTS_TOUR_VERSION}`;

export type TourStatus = "completed" | "dismissed";

export interface KeyValueStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export function memoryStore(initial: Record<string, string> = {}): KeyValueStore {
  const m = new Map(Object.entries(initial));
  return { getItem: (k) => m.get(k) ?? null, setItem: (k, v) => void m.set(k, v) };
}

/**
 * The webview's own local storage (kept in the app's data directory by the platform), never throwing: where it is
 * unavailable the walkthrough is remembered for this session only.
 */
export function browserStore(): KeyValueStore {
  const fallback = memoryStore();
  return {
    getItem(k) { try { return globalThis.localStorage.getItem(k); } catch { return fallback.getItem(k); } },
    setItem(k, v) { try { globalThis.localStorage.setItem(k, v); } catch { fallback.setItem(k, v); } },
  };
}

/** The stored outcome for this tour version, or null when the person has not finished or closed it (or the record is unreadable). */
export function readTourStatus(store: KeyValueStore, key = DEPLOYMENTS_TOUR_KEY): TourStatus | null {
  try {
    const raw = store.getItem(key);
    if (!raw) return null;
    const v = JSON.parse(raw) as { version?: unknown; status?: unknown };
    if (v.version !== DEPLOYMENTS_TOUR_VERSION) return null;
    return v.status === "completed" || v.status === "dismissed" ? v.status : null;
  } catch { return null; }
}

export function writeTourStatus(store: KeyValueStore, status: TourStatus, key = DEPLOYMENTS_TOUR_KEY, now = Date.now()): void {
  try { store.setItem(key, JSON.stringify({ version: DEPLOYMENTS_TOUR_VERSION, status, at: now })); } catch { /* the walkthrough is optional; failing to remember it must never surface */ }
}
