import { useState, useCallback, useRef, useMemo, useEffect } from "react";

/** What one read produced: a value, or the reason it failed. Unlike a bare `null`, it cannot be mistaken for "nothing happened". */
export type Fetched<T> = { ok: true; value: T } | { ok: false; error: string };

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
): Shelf<T> & { refresh: () => Promise<T | null>; refreshResult: () => Promise<Fetched<T>>; setData: (v: T | null) => void; setError: (e: string | null) => void; publish: (v: T, error?: string | null) => void } {
  const ttlMs = opts?.ttlMs ?? 30000;
  const skipUnchangedUpdates = opts?.skipUnchangedUpdates ?? false;
  const [data, setDataState] = useState<T | null>(null);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const skippedUpdates = useRef(0);
  const mounted = useRef(true);
  const dataRef = useRef<T | null>(data);
  const fetcherRef = useRef(fetcher);
  const skipUnchangedUpdatesRef = useRef(skipUnchangedUpdates);
  dataRef.current = data;
  fetcherRef.current = fetcher;
  skipUnchangedUpdatesRef.current = skipUnchangedUpdates;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  // One read at a time: callers that arrive while it runs share its promise (and its result) instead of starting another
  // command or being told "nothing". `version` moves whenever a newer value is pushed in (publish/setData), so a read that
  // began before it cannot overwrite it when it finally lands.
  const flight = useRef<Promise<Fetched<T>> | null>(null);
  const version = useRef(0);

  const refreshResult = useCallback((): Promise<Fetched<T>> => {
    if (flight.current) return flight.current;
    const startedAt = version.current;
    setRefreshing(true);
    // do not clear error until fetch succeeds; keep data visible
    const p = (async (): Promise<Fetched<T>> => {
      let r: Fetched<T>;
      try { r = { ok: true, value: await fetcherRef.current() }; } catch (e: unknown) { r = { ok: false, error: String(e) }; }
      try {
        if (version.current !== startedAt) {
          // a newer value arrived while this read was running: it wins, and so does what it says
          return dataRef.current !== null ? { ok: true, value: dataRef.current } : r;
        }
        if (!mounted.current) return r;
        if (r.ok) {
          if (skipUnchangedUpdatesRef.current && deepEqual(dataRef.current, r.value)) skippedUpdates.current += 1;
          else { dataRef.current = r.value; setDataState(r.value); }
          setUpdatedAt(Date.now());
          setError(null);
        } else setError(r.error);
        return r;
      } finally {
        if (mounted.current) setRefreshing(false);
      }
    })();
    flight.current = p;
    // registered before any caller's continuation, so the next refresh after an await starts a new read; a rejection is never kept
    void p.then(() => { if (flight.current === p) flight.current = null; });
    return p;
  }, []);

  /** Value of the shared read, or null when it genuinely failed (the reason is `error`). */
  const refresh = useCallback(async (): Promise<T | null> => {
    const r = await refreshResult();
    return r.ok ? r.value : null;
  }, [refreshResult]);

  const setData = useCallback((v: T | null) => { version.current += 1; dataRef.current = v; setDataState(v); }, []);

  const publish = useCallback((v: T, nextError: string | null = null) => {
    if (!mounted.current) return;
    version.current += 1;
    if (nextError) { setError(nextError); return; }
    if (skipUnchangedUpdatesRef.current && deepEqual(dataRef.current, v)) skippedUpdates.current += 1;
    else { dataRef.current = v; setDataState(v); }
    setUpdatedAt(Date.now());
    setError(null);
  }, []);

  const hasCache = computeHasCache(data);
  const stale = isStale(updatedAt, ttlMs);

  return useMemo(()=>({ data, updatedAt, refreshing, error, hasCache, stale, skippedUpdates: skippedUpdates.current, refresh, refreshResult, setData, setError, publish }), [data, updatedAt, refreshing, error, hasCache, stale, refresh, refreshResult, publish]);
}
