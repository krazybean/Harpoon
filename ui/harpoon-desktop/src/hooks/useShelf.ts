import { useState, useCallback, useRef, useMemo, useEffect } from "react";

export type Shelf<T> = {
  data: T | null;
  updatedAt: number | null;
  refreshing: boolean;
  error: string | null;
  hasCache: boolean;
  stale: boolean;
  skippedUpdates: number;
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

export function deepEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) return a.length === (b as unknown[]).length && a.every((v, i) => deepEqual(v, (b as unknown[])[i]));
  const aKeys = Object.keys(a as object);
  const bKeys = Object.keys(b as object);
  if (aKeys.length !== bKeys.length) return false;
  return aKeys.every((key) => Object.prototype.hasOwnProperty.call(b, key) && deepEqual((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]));
}

export function useShelf<T>(
  fetcher: () => Promise<T>,
  opts?: { ttlMs?: number; skipUnchangedUpdates?: boolean }
): Shelf<T> & { refresh: () => Promise<T | null>; setData: (v: T | null) => void; setError: (e: string | null) => void; publish: (v: T, error?: string | null) => void } {
  const ttlMs = opts?.ttlMs ?? 30000;
  const skipUnchangedUpdates = opts?.skipUnchangedUpdates ?? false;
  const [data, setData] = useState<T | null>(null);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const skippedUpdates = useRef(0);
  const inflight = useRef(false);
  const mounted = useRef(true);
  const dataRef = useRef<T | null>(data);
  const fetcherRef = useRef(fetcher);
  const skipUnchangedUpdatesRef = useRef(skipUnchangedUpdates);
  dataRef.current = data;
  fetcherRef.current = fetcher;
  skipUnchangedUpdatesRef.current = skipUnchangedUpdates;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  const refresh = useCallback(async (): Promise<T | null> => {
    if (inflight.current) return null;
    inflight.current = true;
    setRefreshing(true);
    // do not clear error until fetch succeeds; keep data visible
    try {
      const v = await fetcherRef.current();
      if (!mounted.current) return v;
      if (skipUnchangedUpdatesRef.current && deepEqual(dataRef.current, v)) skippedUpdates.current += 1;
      else { dataRef.current = v; setData(v); }
      setUpdatedAt(Date.now());
      setError(null);
      return v;
    } catch (e: unknown) {
      if (mounted.current) setError(String(e));
      return null;
    } finally {
      if (mounted.current) setRefreshing(false);
      inflight.current = false;
    }
  }, []);

  const publish = useCallback((v: T, nextError: string | null = null) => {
    if (!mounted.current) return;
    if (nextError) { setError(nextError); return; }
    if (skipUnchangedUpdatesRef.current && deepEqual(dataRef.current, v)) skippedUpdates.current += 1;
    else { dataRef.current = v; setData(v); }
    setUpdatedAt(Date.now());
    setError(null);
  }, []);

  const hasCache = computeHasCache(data);
  const stale = isStale(updatedAt, ttlMs);

  return useMemo(()=>({ data, updatedAt, refreshing, error, hasCache, stale, skippedUpdates: skippedUpdates.current, refresh, setData, setError, publish }), [data, updatedAt, refreshing, error, hasCache, stale, refresh, publish]);
}
