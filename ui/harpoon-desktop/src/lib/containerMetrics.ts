// Per-container telemetry for the Metrics tab. The backend returns one measurement of the serving container (or the
// reason there is none); everything here only turns a sequence of those into what a person may be told. Nothing is
// invented: a figure the backend did not provide stays null, a rate needs two valid samples of the same run, and a
// value that has not been refreshed recently is called stale, never shown as current.

export type MetricsState = "ok" | "stopped" | "absent" | "not-owned" | "unavailable";

export type MetricsDto = {
  state: MetricsState; detail: string | null; sampledMs: number;
  containerId: string | null; startedAt: string | null; restartCount: number | null;
  /** Percent of one core, as `docker stats` shows it: 100 is one fully used core, so it can exceed 100. */
  cpuPercent: number | null; cpus: number | null;
  memRaw: number | null; memCache: number | null; memWorkingSet: number | null; memLimit: number | null;
  netRx: number | null; netTx: number | null; pids: number | null;
};

/** What the backend verifies before it measures anything: the serving release's container, by name, labels and recorded id. */
export type Target = { app: string; deployment: string; container: string; containerId: string | null };
export const targetKey = (t: Target): string => `${t.app}/${t.deployment}/${t.container}/${t.containerId ?? ""}`;

export const POLL_MS = 3000;
export const HISTORY_MS = 10 * 60 * 1000;
/** A reading older than this (about three polls) is stale. */
export const STALE_MS = 10_000;
/** A gap longer than this between two points is drawn as a break, never joined. */
export const GAP_MS = POLL_MS * 2.5;

export type Point = { t: number; cpu: number | null; mem: number | null; rx: number | null; tx: number | null };

export type Track = {
  key: string | null;
  /** `containerId|startedAt` of the run the samples belong to. */
  run: string | null;
  /** The last valid reading of the current run: the baseline for rates. */
  base: MetricsDto | null;
  samples: Point[];
  latest: MetricsDto | null;
  lastOkMs: number | null;
  error: { message: string; atMs: number } | null;
  /** Times the same container was seen starting again during this session. */
  restartsSeen: number;
};

export const emptyTrack = (key: string | null): Track => ({ key, run: null, base: null, samples: [], latest: null, lastOkMs: null, error: null, restartsSeen: 0 });

const runOf = (m: MetricsDto): string => `${m.containerId ?? ""}|${m.startedAt ?? ""}`;
const rate = (cur: number | null, prev: number | null, dtMs: number): number | null =>
  cur === null || prev === null || dtMs <= 0 || cur < prev ? null : (cur - prev) / (dtMs / 1000); // a counter that went down was reset: no rate

export type Event = { reading: MetricsDto } | { error: string; atMs: number };

/** Fold one backend answer (or failure) into the track. A different target starts an empty track: samples of two releases are never joined. */
export function advance(track: Track, key: string, ev: Event, nowMs: number): Track {
  let t = track.key === key ? track : emptyTrack(key);
  if ("error" in ev) return { ...t, error: { message: ev.error, atMs: ev.atMs } };
  const m = ev.reading;
  t = { ...t, error: null, latest: m };
  if (m.state !== "ok") return { ...t, base: null, run: m.state === "stopped" ? t.run : null }; // the next reading starts a fresh baseline
  const run = runOf(m);
  const sameRun = t.run === run && t.base !== null;
  const restartsSeen = t.run !== null && t.run !== run && t.run.split("|")[0] === (m.containerId ?? "") ? t.restartsSeen + 1 : t.restartsSeen;
  const prev = sameRun ? t.base : null;
  const dt = prev ? m.sampledMs - prev.sampledMs : 0;
  const point: Point = {
    t: m.sampledMs, cpu: m.cpuPercent, mem: m.memWorkingSet ?? m.memRaw,
    rx: prev ? rate(m.netRx, prev.netRx, dt) : null, tx: prev ? rate(m.netTx, prev.netTx, dt) : null,
  };
  const kept = (sameRun ? t.samples : []).filter((p) => nowMs - p.t <= HISTORY_MS);
  return { ...t, run, base: m, samples: [...kept, point], lastOkMs: m.sampledMs, restartsSeen };
}

export type Status = "no-sample" | "ok" | "stale" | "stopped" | "absent" | "not-owned" | "unavailable";

/** What the tab may claim right now. `current` is set only for `ok`: a stale reading is available as `lastOk`, never as current. */
export function view(track: Track, nowMs: number): { status: Status; current: MetricsDto | null; lastOk: MetricsDto | null; message: string | null; ageMs: number | null } {
  const latest = track.latest;
  const lastOk = track.base ?? (latest?.state === "ok" ? latest : null);
  const ageMs = track.lastOkMs === null ? null : Math.max(0, nowMs - track.lastOkMs);
  const failedSince = track.error && (!latest || track.error.atMs >= latest.sampledMs);
  if (failedSince) {
    if (ageMs !== null && ageMs > STALE_MS) return { status: "stale", current: null, lastOk, message: track.error!.message, ageMs };
    if (!latest) return { status: "unavailable", current: null, lastOk: null, message: track.error!.message, ageMs };
  }
  if (!latest) return { status: "no-sample", current: null, lastOk: null, message: null, ageMs: null };
  if (latest.state !== "ok") return { status: latest.state, current: null, lastOk, message: latest.detail, ageMs };
  if (nowMs - latest.sampledMs > STALE_MS) return { status: "stale", current: null, lastOk: latest, message: null, ageMs };
  return { status: "ok", current: latest, lastOk: latest, message: null, ageMs };
}

// ------------------------------------------------------------------ figures

const UNITS = ["B", "KiB", "MiB", "GiB", "TiB"];
export function formatBytes(n: number | null): string {
  if (n === null || !Number.isFinite(n)) return "—";
  let v = n, i = 0;
  while (v >= 1024 && i < UNITS.length - 1) { v /= 1024; i++; }
  return i === 0 ? `${Math.round(v)} B` : `${v.toFixed(v >= 100 ? 0 : 1)} ${UNITS[i]}`;
}
export const formatRate = (bps: number | null): string => (bps === null ? "—" : `${formatBytes(bps)}/s`);
export const formatPercent = (p: number | null): string => (p === null ? "—" : `${p.toFixed(1)}%`);

/** Docker writes nanoseconds and `0001-01-01…` for a container that never started; neither is passed on as a date. */
export function startedMs(iso: string | null): number | null {
  if (!iso || iso.startsWith("0001-")) return null;
  const ms = Date.parse(iso.replace(/(\.\d{3})\d*/, "$1"));
  return Number.isFinite(ms) ? ms : null;
}
export function formatUptime(startedAt: string | null, nowMs: number): string {
  const s = startedMs(startedAt);
  if (s === null) return "—";
  const secs = Math.max(0, Math.floor((nowMs - s) / 1000));
  const d = Math.floor(secs / 86400), h = Math.floor((secs % 86400) / 3600), m = Math.floor((secs % 3600) / 60);
  return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : m ? `${m} min ${secs % 60} s` : `${secs} s`;
}

/** Memory in use (page cache excluded when Docker reports it) and, only when the container has its own limit, the share of it. */
export function memoryFigures(m: MetricsDto): { used: number | null; basis: "working set" | "raw usage" | null; percent: number | null; limit: number | null } {
  const used = m.memWorkingSet ?? m.memRaw;
  const limit = m.memLimit !== null && m.memLimit > 0 ? m.memLimit : null;
  return { used, basis: m.memWorkingSet !== null ? "working set" : m.memRaw !== null ? "raw usage" : null, limit, percent: used !== null && limit !== null ? (used / limit) * 100 : null };
}

/** The most recent rate, or null: a figure needs two valid samples of the same run. */
export const latestRate = (samples: Point[], key: "rx" | "tx"): number | null => samples.length ? samples[samples.length - 1][key] : null;

/** Line segments for a chart: valid points only, split wherever a point is missing or the gap is longer than a few polls. Never interpolated. */
export function segments(samples: Point[], pick: (p: Point) => number | null): { t: number; v: number }[][] {
  const out: { t: number; v: number }[][] = [];
  let cur: { t: number; v: number }[] = [];
  let lastT: number | null = null;
  for (const p of samples) {
    const v = pick(p);
    if (v === null || (lastT !== null && p.t - lastT > GAP_MS)) { if (cur.length) out.push(cur); cur = []; }
    if (v !== null) cur.push({ t: p.t, v });
    lastT = p.t;
  }
  if (cur.length) out.push(cur);
  return out;
}
