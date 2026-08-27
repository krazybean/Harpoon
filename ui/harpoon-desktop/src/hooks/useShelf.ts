import { useState, useCallback, useRef, useMemo } from "react";

export type Shelf<T> = {
  data: T | null;
  updatedAt: number | null;
  refreshing: boolean;
  error: string | null;
  hasCache: boolean;
  stale: boolean;
};

export function isStale(at: number | null, ttlMs = 30000): boolean {
  if (!at) return true;
  return Date.now() - at > ttlMs;
}

function computeHasCache<T>(data: T | null): boolean {
  if (data === null || data === undefined) return false;
  // authoritative empty array/string IS cached data — distinguish "never fetched" (null) from "fetched empty"
  return true;
}

export function useShelf<T>(
  fetcher: () => Promise<T>,
  opts?: { ttlMs?: number }
): Shelf<T> & { refresh: () => Promise<T | null>; setData: (v: T | null) => void; setError: (e: string | null) => void } {
  const ttlMs = opts?.ttlMs ?? 30000;
  const [data, setData] = useState<T | null>(null);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inflight = useRef(false);

  const refresh = useCallback(async (): Promise<T | null> => {
    if (inflight.current) return null;
    inflight.current = true;
    setRefreshing(true);
    // do not clear error until fetch succeeds; keep data visible
    try {
      const v = await fetcher();
      setData(v);
      setUpdatedAt(Date.now());
      setError(null);
      return v;
    } catch (e: unknown) {
      setError(String(e));
      return null;
    } finally {
      setRefreshing(false);
      inflight.current = false;
    }
  }, [fetcher]);

  const hasCache = computeHasCache(data);
  const stale = isStale(updatedAt, ttlMs);

  return useMemo(()=>({ data, updatedAt, refreshing, error, hasCache, stale, refresh, setData, setError }), [data, updatedAt, refreshing, error, hasCache, stale, refresh]);
}
