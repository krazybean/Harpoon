import { Fragment } from "react";
import type { ReactNode } from "react";
import { Cpu, HardDrives, ArrowsDownUp, Cube } from "@phosphor-icons/react";
import { useContainerMetrics } from "../../hooks/useContainerMetrics.ts";
import { formatBytes, formatPercent, formatRate, formatUptime, HISTORY_MS, latestRate, memoryFigures, POLL_MS, segments } from "../../lib/containerMetrics.ts";
import type { MetricsDto, Point, Target } from "../../lib/containerMetrics.ts";
import { Banner, Disclosure, Pill } from "./parts.tsx";

/** A line chart of real samples only: segments are broken where a sample is missing, and nothing is interpolated. */
function Trend({ samples, pick, label, format }: { samples: Point[]; pick: (p: Point) => number | null; label: string; format: (v: number) => string }) {
  const segs = segments(samples, pick);
  const vals = segs.flat().map((p) => p.v);
  if (vals.length < 2) return <div className="dp-faint dp-trend__none">{label}: not enough valid samples yet</div>;
  const t0 = samples[0].t, t1 = samples[samples.length - 1].t, span = Math.max(1, t1 - t0);
  const hi = Math.max(...vals, 0), top = hi > 0 ? hi : 1;
  const x = (t: number) => ((t - t0) / span) * 200, y = (v: number) => 46 - (v / top) * 42;
  const d = segs.map((s) => s.map((p, i) => `${i ? "L" : "M"}${x(p.t).toFixed(1)} ${y(p.v).toFixed(1)}`).join(" ")).join(" ");
  const mins = Math.max(1, Math.round(span / 60000));
  return (
    <figure className="dp-trend">
      <figcaption><b>{label}</b> <span className="dp-faint">peak {format(hi)} · last {mins} min</span></figcaption>
      <svg viewBox="0 0 200 48" preserveAspectRatio="none" role="img" aria-label={`${label} over the last ${mins} minutes, peak ${format(hi)}`}><path d={d} /></svg>
    </figure>
  );
}

const card = (icon: ReactNode, title: string, big: string, rows: [string, string][], dim: boolean, note?: string) => (
  <section className={`dp-panel dp-mcard${dim ? " dp-mcard--stale" : ""}`}>
    <div className="dp-panel__head"><h2 className="dp-h2 dp-sec"><span aria-hidden="true" style={{ display: "inline-flex" }}>{icon}</span>{title}</h2></div>
    <div className="dp-panel__body"><strong className="dp-mcard__big">{big}</strong>
      <dl className="dp-kv">{rows.map(([k, v]) => <Fragment key={k}><dt>{k}</dt><dd>{v}</dd></Fragment>)}</dl>
      {note ? <p className="dp-faint" style={{ fontSize: 11, margin: "8px 0 0" }}>{note}</p> : null}</div>
  </section>
);

export function ContainerMetrics({ fetchOnce, target, pollMs = POLL_MS }: { fetchOnce: ((t: Target) => Promise<MetricsDto>) | undefined; target: Target; pollMs?: number }) {
  const m = useContainerMetrics(fetchOnce, target, pollMs);
  const { status, current, lastOk, message, ageMs, track, now } = m;
  const shown = current ?? lastOk; // only `current` is ever presented as now; a stale one is dimmed and labelled
  const dim = status !== "ok";
  const mem = shown ? memoryFigures(shown) : null;
  const rx = latestRate(track.samples, "rx"), tx = latestRate(track.samples, "tx");
  const head: Record<string, [string, "ok" | "warn" | "err" | "neutral" | "info"]> = {
    ok: ["Live", "ok"], "no-sample": ["Waiting for the first sample", "info"], stale: ["Stale", "warn"], stopped: ["Container stopped", "warn"],
    absent: ["Container not found", "err"], "not-owned": ["Container not verified", "err"], unavailable: ["Not available", "err"],
  };
  const age = ageMs === null ? null : Math.round(ageMs / 1000);
  return (
    <>
      <div className="dp-row dp-row--wrap" style={{ marginBottom: 10 }} role="status" aria-live="polite">
        <Pill tone={head[status][1]} live={status === "ok"}>{head[status][0]}</Pill>
        <span className="mono dp-muted">{target.container}</span>
        {age !== null ? <span className="dp-faint">· {status === "ok" ? `updated ${age} s ago` : `last measured ${age} s ago`}</span> : null}
        <span className="dp-spacer" /><span className="dp-faint hide-compact">This container only, not the VM · every {Math.round(pollMs / 1000)} s while this tab is open</span>
      </div>
      {status === "stale" && <Banner tone="warn" title="These figures are stale">Harpoon could not get a fresh measurement{message ? `: ${message}` : ""}. The values below are the last ones measured and are not current.</Banner>}
      {status === "stopped" && <Banner tone="warn" title="The container is stopped">Docker reports the serving release's container is not running, so there is nothing to measure.</Banner>}
      {(status === "absent" || status === "not-owned" || status === "unavailable") && <Banner tone="err" title={status === "absent" ? "Docker has no such container" : status === "not-owned" ? "Not measuring: this is not the serving release's container" : "Docker did not provide metrics"}>{message ?? "No further detail was reported."}</Banner>}
      {shown ? (
        <div className="dp-grid dp-grid--4">
          {card(<Cpu size={16} />, "CPU", formatPercent(shown.cpuPercent), [["Cores", shown.cpus === null ? "—" : String(shown.cpus)], ["Processes", shown.pids === null ? "—" : String(shown.pids)]], dim, shown.cpuPercent === null ? "Docker did not provide enough to compute this." : "Percent of one core; 100% is one fully used core, so it can exceed 100%.")}
          {card(<HardDrives size={16} />, "Memory", mem?.used === null || !mem ? "—" : formatBytes(mem.used), [[mem?.basis === "working set" ? "Excludes page cache" : "Basis", mem?.basis ?? "—"], ["Limit", mem?.limit ? formatBytes(mem.limit) : "none set"], ["Share of limit", mem?.percent === null || !mem ? "—" : formatPercent(mem.percent)]], dim, mem?.limit ? undefined : "This container has no memory limit, so no percentage is shown.")}
          {card(<ArrowsDownUp size={16} />, "Network", `${formatBytes(shown.netRx)} in`, [["Sent", formatBytes(shown.netTx)], ["Receive rate", formatRate(rx)], ["Send rate", formatRate(tx)]], dim, "Totals since the container started. Rates need two samples.")}
          {card(<Cube size={16} />, "Container", status === "ok" ? "Running" : status === "stopped" ? "Stopped" : "Last seen running", [["Up for", status === "ok" ? formatUptime(shown.startedAt, now) : "—"], ["Docker restarts", shown.restartCount === null ? "not reported" : String(shown.restartCount)], ["Restarted this session", String(track.restartsSeen)]], dim)}
        </div>
      ) : status === "no-sample" ? <div className="dp-panel dp-empty"><h2>Waiting for the first sample</h2><p>Docker takes about a second to produce one.</p></div> : null}
      {shown ? (
        <section className="dp-panel" style={{ marginTop: 14 }}>
          <div className="dp-panel__head"><h2 className="dp-h2">Short-term trend</h2><span className="dp-spacer" /><span className="dp-faint">Session only · up to {HISTORY_MS / 60000} min · kept in memory, cleared when you leave this tab or the container changes</span></div>
          <div className="dp-panel__body dp-grid dp-grid--3">
            <Trend samples={track.samples} pick={(p) => p.cpu} label="CPU" format={(v) => formatPercent(v)} />
            <Trend samples={track.samples} pick={(p) => p.mem} label="Memory" format={formatBytes} />
            <Trend samples={track.samples} pick={(p) => p.rx} label="Receive rate" format={formatRate} />
          </div>
        </section>
      ) : null}
      {shown ? (
        <Disclosure summary="Technical details">
          <dl className="dp-kv">
            <dt>Container</dt><dd className="mono">{target.container}</dd>
            <dt>Container id</dt><dd className="mono">{shown.containerId ?? "—"}</dd>
            <dt>Application</dt><dd className="mono">{target.app}</dd>
            <dt>Deployment</dt><dd className="mono">{target.deployment}</dd>
            <dt>Started at</dt><dd className="mono">{shown.startedAt ?? "—"}</dd>
            <dt>Raw memory</dt><dd>{formatBytes(shown.memRaw)} <span className="dp-faint">includes page cache{shown.memCache !== null ? ` (${formatBytes(shown.memCache)} reported)` : "; Docker did not report a cache figure"}</span></dd>
            <dt>Verified by</dt><dd>Harpoon labels and the recorded container id, checked before every measurement</dd>
          </dl>
        </Disclosure>
      ) : null}
    </>
  );
}
