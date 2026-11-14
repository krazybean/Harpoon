import { useEffect, useState } from "react";
import { advance, emptyTrack, POLL_MS, targetKey, view } from "../lib/containerMetrics.ts";
import type { MetricsDto, Target, Track } from "../lib/containerMetrics.ts";

const ANSWER_MS = 10_000;

/**
 * Samples the serving container while the component using it is mounted (the Metrics tab). One request at a time,
 * none while the window is hidden, and everything is dropped when the target changes or the tab is left: the history
 * lives only here, in memory. A request that does not answer in time is a failure, never a pending forever.
 */
export function useContainerMetrics(fetchOnce: ((t: Target) => Promise<MetricsDto>) | undefined, target: Target | null, pollMs = POLL_MS) {
  const key = target ? targetKey(target) : null;
  const [track, setTrack] = useState<Track>(() => emptyTrack(key));
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    setTrack(emptyTrack(key));
    if (!fetchOnce || !target || !key) return;
    let alive = true, busy = false, timer: ReturnType<typeof setTimeout> | undefined, limit: ReturnType<typeof setTimeout> | undefined;
    const schedule = () => { if (alive) timer = setTimeout(pull, pollMs); };
    const pull = async () => {
      if (!alive) return;
      if (busy || document.visibilityState === "hidden") { schedule(); return; }
      busy = true;
      try {
        const reading = await Promise.race([fetchOnce(target), new Promise<never>((_, no) => { limit = setTimeout(() => no(new Error("Docker did not answer in time.")), ANSWER_MS); })]);
        if (alive) setTrack((t) => advance(t, key, { reading }, Date.now()));
      } catch (e) {
        if (alive) setTrack((t) => advance(t, key, { error: e instanceof Error ? e.message : String(e), atMs: Date.now() }, Date.now()));
      } finally { clearTimeout(limit); busy = false; if (alive) setNow(Date.now()); schedule(); }
    };
    const onVisible = () => { if (document.visibilityState === "visible") { clearTimeout(timer); void pull(); } };
    document.addEventListener("visibilitychange", onVisible);
    void pull();
    return () => { alive = false; clearTimeout(timer); clearTimeout(limit); document.removeEventListener("visibilitychange", onVisible); };
    // the target's own fields are folded into `key`
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fetchOnce, key, pollMs]);
  // staleness is judged against the clock, so a silent backend ages the reading without another answer
  useEffect(() => { if (!key) return; const i = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(i); }, [key]);
  return { track: track.key === key ? track : emptyTrack(key), now, ...view(track.key === key ? track : emptyTrack(key), now) };
}
