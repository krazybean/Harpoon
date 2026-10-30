import { useState } from "react";
import type { ReactNode } from "react";
import { ArrowCounterClockwise, ArrowsClockwise, ArrowUp, Copy, Cube, FolderSimple, GitBranch, Key, Lock, Pause, Play, Plus, Question, RocketLaunch, Trash, Warning, Check } from "@phosphor-icons/react";
import type { AppModel, Deployment, Env, LogLine } from "../../lib/deployTypes.ts";
import { appStatus, appsNeedingAttention, attention, latestDeployment, recoveryInProgress, releaseById } from "../../lib/deployStore.ts";
import {
  EVIDENCE_NOTE, formatAge, reasonLabel, recoveryHeadline, recoveryLabel, recoveryTone, statusLabel, statusTone,
} from "../../lib/deployPresentation.ts";
import { RUNTIME_LOG } from "../../lib/deployMock.ts";
import type { Ctx, Tab } from "./context.ts";
import { Banner, Dot, Evidence, LogPanel, PausedIcon, Pill, Spark, Steps, Tag } from "./parts.tsx";

const btn = (cls: string, onClick: () => void, label: ReactNode, o: { disabled?: boolean; sm?: boolean; title?: string } = {}) => (
  <button className={`btn ${cls} ${o.sm ? "btn--sm" : ""}`} onClick={onClick} disabled={o.disabled} title={o.title}>{label}</button>
);
const connected = (c: Ctx) => c.runtime === "connected";

// ------------------------------------------------------------------ home

export function HomeScreen({ c }: { c: Ctx }) {
  const { state } = c;
  if (state.scenario === "loading") {
    return (
      <div className="dp-col" aria-busy="true" aria-label="Loading deployments">
        <div className="dp-skel" style={{ height: 58 }} />
        <div className="dp-grid dp-grid--3">{[0, 1, 2].map((i) => <div key={i} className="dp-skel" style={{ height: 150 }} />)}</div>
      </div>
    );
  }
  if (state.projects.length === 0) {
    return (
      <div className="dp-panel dp-empty">
        <div className="art"><RocketLaunch size={34} /></div>
        <h2>Deploy something to your local Harpoon</h2>
        <p>Import a project from a Git repository, a local folder with a Dockerfile, or an existing image. Harpoon builds and runs it in your local Docker Engine — no cloud, no account. Your existing containers stay exactly where they are.</p>
        <div className="dp-row dp-row--wrap" style={{ justifyContent: "center" }}>
          <button className="btn btn--primary" onClick={() => c.open({ kind: "create", step: 1, src: "git" })}><Plus size={14} /> Create project</button>
          <button className="btn btn--secondary" onClick={() => c.open({ kind: "create", step: 1, src: "folder" })}><FolderSimple size={14} /> Import a local folder</button>
        </div>
        <Evidence>Everything runs on this Mac, under Harpoon. Local runs are evidence, not a production guarantee.</Evidence>
      </div>
    );
  }
  const needs = appsNeedingAttention(state, c.runtime);
  const card = (app: AppModel) => {
    const st = appStatus(app, c.runtime);
    const att = needs.find((n) => n.app.id === app.id);
    return (
      <button key={app.id} className={`dp-card ${att ? (att.attention.tone === "err" ? "dp-card--err" : "dp-card--warn") : ""}`} onClick={() => c.setRoute({ screen: "app", appId: app.id, tab: "overview", env: "production" })}>
        <div className="dp-row"><Dot tone={st.tone} /><b style={{ fontSize: 14 }}>{app.name}</b><span className="dp-spacer" /><Pill tone={st.tone} live={st.kind === "recovering"}>{st.label}</Pill></div>
        <div className="dp-muted" style={{ fontSize: 12 }}>
          {app.source.kind === "git" ? <GitBranch size={12} style={{ verticalAlign: -2 }} /> : <Cube size={12} style={{ verticalAlign: -2 }} />} {app.source.label} · {app.source.detail}{st.live && st.releaseId ? ` · release ${st.releaseId}` : ""}
        </div>
        <div className="dp-envstrip">
          <span className="dp-env"><Dot tone={st.tone} />production · {st.label.toLowerCase()}</span>
          <span className="dp-env"><Dot tone={st.live && app.previews.length ? "ok" : "neutral"} />preview · {app.previews.length ? `${app.previews.length} branches` : "none"}</span>
          <span className="dp-env"><Dot tone="neutral" />development · idle</span>
        </div>
      </button>
    );
  };
  return (
    <>
      <div className="dp-row dp-row--wrap" style={{ marginBottom: 14 }}>
        <div><div className="dp-kicker">Local-first deployments</div><h1 className="dp-h1">Projects</h1></div>
        <span className="dp-spacer" />
        <button className="btn btn--primary" onClick={() => c.open({ kind: "create", step: 1, src: "git" })}><Plus size={14} /> Create project</button>
      </div>
      {needs.map((n) => (
        <Banner key={n.app.id} tone={n.attention.tone === "err" ? "err" : "warn"} title={`${n.app.projectId} / ${n.app.name} needs attention`}
          actions={btn("btn--primary", () => c.setRoute({ screen: "app", appId: n.app.id, tab: "overview" }), `Open ${n.app.name}`)}>
          {n.attention.text}{n.app.automation.state === "paused" ? " Automatic deployments are paused." : ""}
        </Banner>
      ))}
      {state.projects.map((p) => (
        <section key={p.id}>
          <div className="dp-h2" style={{ margin: "14px 0 10px" }}>{p.name} <span className="dp-faint" style={{ textTransform: "none", letterSpacing: 0, fontWeight: 400 }}>· {p.appIds.length} application{p.appIds.length === 1 ? "" : "s"} · {p.sourceLabel}</span></div>
          <div className="dp-grid dp-grid--3">{p.appIds.map((id) => state.apps[id] && card(state.apps[id]))}</div>
        </section>
      ))}
      <p style={{ marginTop: 22 }}><Evidence /></p>
    </>
  );
}

// ------------------------------------------------------------------ application

const TABS: [Tab, string][] = [["overview", "Overview"], ["deployments", "Deployments"], ["logs", "Logs"], ["metrics", "Metrics"], ["variables", "Variables"], ["settings", "Settings"]];

export function AppScreen({ c }: { c: Ctx }) {
  const app = c.state.apps[c.route.appId];
  if (!app) return <Banner tone="warn" title="Application not found">This simulated application no longer exists in the current scenario.</Banner>;
  const st = appStatus(app, c.runtime);
  const att = attention(app, c.runtime);
  const latest = latestDeployment(app);
  const rel = releaseById(app, st.releaseId);
  const ok = connected(c);
  const env = c.route.env;
  const recovering = recoveryInProgress(app);
  const paused = app.automation.state === "paused";
  const down = st.kind === "down";

  const heroIcon = st.kind === "down" ? <Warning size={20} /> : st.kind === "recovering" ? <ArrowCounterClockwise size={20} /> : st.kind === "unknown" ? <Question size={20} /> : <Check size={20} />;
  const heroClass = st.kind === "down" ? "err" : st.kind === "recovering" ? "info" : st.kind === "unknown" ? "idle" : "";
  const lastKnown = app.lastKnownReleaseId;
  const subline = st.kind === "unknown"
    ? `Last known: ${lastKnown ? `running release ${lastKnown}` : "no release"}. Harpoon is ${c.runtime === "stopped" ? "stopped" : "not ready"}, so the current state is not known.`
    : st.kind === "down" ? "No running container for this application." : st.kind === "recovering" ? "Restoring the previous release…" : null;

  const rollbackTarget = app.releases.find((r) => r.id !== app.currentReleaseId);
  return (
    <>
      <div className="dp-row dp-row--wrap" style={{ marginBottom: 12 }}>
        <div className="dp-seg" role="group" aria-label="Environment">
          {(["development", "preview", "production"] as Env[]).map((e) => <button key={e} aria-pressed={env === e} onClick={() => c.setRoute({ env: e, tab: "overview" })}>{e[0].toUpperCase() + e.slice(1)}</button>)}
        </div>
        <span className="dp-faint hide-compact" style={{ fontSize: 11 }}>{env === "production" ? `tracks ${app.settings.productionBranch}` : env === "preview" ? "one instance per branch · TTL 7 d" : "manual · local folder"} · all environments run locally on this Mac</span>
      </div>
      {att && <Banner tone={att.tone} title={att.title}
        actions={<>
          {down && btn("btn--primary", () => c.open({ kind: "recover" }), <><ArrowCounterClockwise size={14} /> Retry recovery</>, { disabled: !ok })}
          {down && rollbackTarget && btn("btn--secondary", () => c.open({ kind: "rollback", releaseId: rollbackTarget.id }), `Roll back to ${rollbackTarget.deploymentId}`, { disabled: !ok })}
          {latest && btn("btn--secondary", () => c.setRoute({ screen: "dep", depId: latest.id }), `Inspect ${latest.id}`)}
        </>}>{att.text}</Banner>}
      {paused && ok && (
        <Banner tone="warn" icon={<PausedIcon />} title="Automatic deployments are paused"
          actions={btn("btn--primary", () => c.open({ kind: "resume" }), <><Play size={14} /> Review &amp; resume…</>)}>
          {pauseText(app)} <b>{app.automation.held.length} push{app.automation.held.length === 1 ? "" : "es"} held</b> — nothing was discarded. Manual deployments still work.
        </Banner>
      )}
      <div className="dp-panel"><div className="dp-panel__body dp-hero">
        <div className="dp-hero__state">
          <div className={`big ${heroClass}`} style={{ width: 38, height: 38, borderRadius: 10, display: "flex", alignItems: "center", justifyContent: "center" }}>{heroIcon}</div>
          <div>
            <div className="dp-row dp-row--wrap">
              <Pill tone={st.tone} live={st.kind === "recovering"}>{st.label}</Pill>
              {st.kind === "unknown" ? <Tag>not live</Tag> : null}
              {ok && (st.kind === "running" || st.kind === "building") && rel ? <><b>release {rel.id}</b><span className="mono dp-muted">{rel.revision.sha}</span></> : null}
              <Tag>simulated</Tag>
            </div>
            <div className="dp-muted" style={{ fontSize: 12, marginTop: 4 }}>
              {subline ?? <><span className="mono">{app.url}</span> · {env} · container <span className="mono" title="Simulated container name — it does not exist in your real Containers view">{app.containerName}</span></>}
            </div>
          </div>
        </div>
        <div className="dp-actions">
          {btn("btn--primary", () => c.open({ kind: "deploy" }), <><RocketLaunch size={14} /> Deploy</>, { disabled: !ok })}
          {btn("btn--secondary", () => c.open({ kind: "redeploy" }), <><ArrowsClockwise size={14} /> Redeploy</>, { disabled: !ok || !app.releases.length })}
          {btn("btn--secondary", () => rollbackTarget && c.open({ kind: "rollback", releaseId: rollbackTarget.id }), <><ArrowCounterClockwise size={14} /> Roll back…</>, { disabled: !ok || !rollbackTarget || recovering || !!c.state.job })}
          {btn("btn--secondary", () => c.open({ kind: "promote" }), <><ArrowUp size={14} /> Promote…</>, { disabled: !ok || !app.previews.length })}
        </div>
      </div></div>
      <div className="dp-tabs" role="tablist">
        {TABS.map(([k, l]) => <button key={k} role="tab" aria-selected={c.route.tab === k} onClick={() => c.setRoute({ tab: k })}>{l}{k === "deployments" ? <span className="dp-count">{app.deployments.length}</span> : null}</button>)}
      </div>
      {env === "preview" && c.route.tab === "overview" ? <PreviewTab app={app} /> : env === "development" && c.route.tab === "overview" ? <DevTab c={c} /> :
        c.route.tab === "overview" ? <OverviewTab c={c} app={app} /> : c.route.tab === "deployments" ? <DeploymentsTab c={c} app={app} /> :
        c.route.tab === "logs" ? <LogsTab c={c} app={app} /> : c.route.tab === "metrics" ? <MetricsTab c={c} app={app} /> :
        c.route.tab === "variables" ? <VariablesTab c={c} app={app} /> : <SettingsTab c={c} app={app} />}
    </>
  );
}

function pauseText(app: AppModel): string {
  const r = app.automation.reason;
  if (!r) return "Paused.";
  if (r.kind === "rollback") return `You rolled back to ${r.releaseId}. Automatic deployments stay paused until you resume them.`;
  return `A cutover to ${r.deploymentId} failed (restore outcome: ${r.outcome.replace("_", " ")}).`;
}

function ResourceBlock({ c, app }: { c: Ctx; app: AppModel }) {
  const st = appStatus(app, c.runtime);
  if (!st.live || !app.resources || !app.currentReleaseId) {
    return <div className="dp-muted">{st.live ? "No running container, so there are no resource figures. They are not estimated." : "Resource figures are unavailable while Harpoon is stopped. Nothing shown here is current."}</div>;
  }
  const r = app.resources;
  return (
    <>
      <div className="dp-grid dp-grid--3">
        <div className="dp-metric"><span>CPU</span><strong>{r.cpuPct}%</strong><Spark seed={11} /></div>
        <div className="dp-metric"><span>Memory</span><strong>{r.memMiB} MiB</strong><Spark seed={23} /></div>
        <div className="dp-metric"><span>Processes</span><strong>{r.pids}</strong><Spark seed={5} hi={0.6} /></div>
      </div>
      <div className="dp-faint" style={{ fontSize: 11, marginTop: 8 }}>Simulated figures for this mock container · limit {r.memLimitMiB} MiB. In the real feature these come from Docker stats for the container, not the VM or host.</div>
    </>
  );
}

function OverviewTab({ c, app }: { c: Ctx; app: AppModel }) {
  const L = latestDeployment(app);
  const st = appStatus(app, c.runtime);
  const ok = connected(c);
  const a = app.automation;
  return (
    <div className="dp-grid dp-grid--main">
      <div className="dp-col">
        {L && (
          <section className="dp-panel">
            <div className="dp-panel__head"><h2 className="dp-h2">Latest deployment</h2><span className="dp-spacer" />{btn("btn--secondary", () => c.setRoute({ screen: "dep", depId: L.id }), "Inspect", { sm: true })}</div>
            <div className="dp-panel__body">
              <div className="dp-row dp-row--wrap" style={{ marginBottom: 10 }}>
                <b>{L.id}</b><DeploymentPills d={L} /><span className="mono dp-muted">{L.revision.sha}</span><span className="dp-muted">{L.revision.message}</span>
                <span className="dp-spacer" /><span className="dp-faint">{formatAge(L.ageMin)} · {L.durationLabel} · {L.trigger}</span>
              </div>
              {L.failure && <p className="dp-muted" style={{ margin: "0 0 10px" }}>{L.failure}</p>}
              <Steps steps={L.steps} />
              {L.recovery?.outcome === "in_progress" && <div className="dp-bar dp-bar--live" style={{ marginTop: 6 }} role="progressbar" aria-valuenow={23} aria-valuemax={60}><i style={{ width: "38%" }} /></div>}
            </div>
          </section>
        )}
        <section className="dp-panel">
          <div className="dp-panel__head"><h2 className="dp-h2">Resources &amp; health</h2><span className="dp-spacer" /><Evidence>Evidence, not a guarantee</Evidence></div>
          <div className="dp-panel__body">
            <ResourceBlock c={c} app={app} />
            <div className="dp-row dp-row--wrap" style={{ marginTop: 12 }}>
              <Pill tone={st.kind === "running" || st.kind === "building" ? "ok" : st.kind === "down" ? "err" : "warn"}>Health · {st.kind === "running" || st.kind === "building" ? "healthy" : st.kind === "down" ? "down" : "unknown"}</Pill>
              <span className="dp-muted" style={{ fontSize: 12 }}>{st.live ? (st.kind === "running" || st.kind === "building" ? "Docker HEALTHCHECK healthy · HTTP /healthz 200 (2 ms) — simulated" : "No running container") : "Not known while Harpoon is not connected."}</span>
            </div>
          </div>
        </section>
      </div>
      <div className="dp-col">
        <section className="dp-panel">
          <div className="dp-panel__head"><h2 className="dp-h2">Automation</h2><span className="dp-spacer" />
            {!ok ? <Pill icon={<Pause size={11} />}>Not running</Pill> : <Pill tone={a.state === "paused" ? "warn" : "ok"} icon={a.state === "paused" ? <Pause size={11} /> : <Check size={11} />}>{a.state === "paused" ? "Paused" : "Active"}</Pill>}
          </div>
          <div className="dp-panel__body">
            <dl className="dp-kv">
              <dt>Trigger</dt><dd>Poll <code>origin/{app.settings.productionBranch}</code> every {app.settings.pollSeconds} s</dd>
              <dt>Mode</dt><dd>Attended — runs while Harpoon is open</dd>
              <dt>State</dt><dd>{!ok ? "Not watching — automatic deployment only runs while Harpoon is running" : a.state === "paused" ? <>{pauseText(app)}<div className="dp-faint">since {formatAge(a.sinceMin ?? 0)}</div></> : "Watching for pushes"}</dd>
              <dt>Held events</dt><dd>{a.held.length ? a.held.map((h) => <div key={h.id}><span className="mono">{h.revision.sha}</span> {h.revision.message} <span className="dp-faint">{formatAge(h.ageMin)}</span></div>) : "None"}</dd>
            </dl>
            {a.state === "paused" && ok && <div style={{ marginTop: 12 }}>{btn("btn--primary", () => c.open({ kind: "resume" }), <><Play size={14} /> Review &amp; resume…</>)}</div>}
            <p className="dp-faint" style={{ fontSize: 11, margin: "10px 0 0" }}>Pushes that arrive while no Harpoon process is running are found on the next run and shown as <i>late</i>, never dropped.</p>
          </div>
        </section>
        <section className="dp-panel">
          <div className="dp-panel__head"><h2 className="dp-h2">Configuration</h2><span className="dp-spacer" />{btn("btn--secondary", () => c.setRoute({ tab: "settings" }), "Edit", { sm: true })}</div>
          <div className="dp-panel__body"><dl className="dp-kv">
            <dt>Source</dt><dd>{app.source.kind} · {app.source.label}</dd><dt>Build</dt><dd>Dockerfile <code>{app.settings.dockerfile}</code></dd>
            <dt>Port</dt><dd>{app.settings.port ?? "—"} → {app.url.replace("http://", "")}</dd><dt>Health check</dt><dd>{app.settings.healthCheck}</dd>
            <dt>Replacement</dt><dd>stop old → start new (brief interruption, reported)</dd><dt>Retention</dt><dd>current + {app.settings.keepReleases} earlier releases</dd>
          </dl></div>
        </section>
      </div>
    </div>
  );
}

export function DeploymentPills({ d }: { d: Deployment }) {
  const rec = d.recovery;
  return (
    <>
      <Pill tone={statusTone(d.status)} live={d.status === "running"}>{statusLabel(d.status, rec?.outcome === "in_progress")}</Pill>
      {rec ? <Pill tone={recoveryTone(rec.outcome)} live={rec.outcome === "in_progress"}>{recoveryLabel(rec.outcome)}</Pill> : null}
    </>
  );
}

function PreviewTab({ app }: { app: AppModel }) {
  return (
    <>
      <div className="dp-tablewrap"><table className="dp-table"><thead><tr><th>Branch</th><th>Status</th><th className="hide-compact">Revision</th><th className="hide-compact">URL</th><th>Expires</th></tr></thead><tbody>
        {app.previews.map((p) => (
          <tr key={p.branch}><td><b>{p.branch}</b><div className="sub">preview instance (simulated)</div></td><td><Pill tone={p.status === "running" ? "ok" : "info"} live={p.status === "building"}>{p.status === "running" ? "Running" : "Building"}</Pill></td>
            <td className="hide-compact mono">{p.sha}</td><td className="hide-compact">{p.url ? <span className="mono">{p.url.replace("http://", "")}</span> : <span className="dp-faint">—</span>}</td><td>in {p.expiresInDays} d</td></tr>
        ))}
      </tbody></table></div>
      <p><Evidence>Preview instances are found by polling branches — local containers, not hosted preview URLs.</Evidence></p>
    </>
  );
}

function DevTab({ c }: { c: Ctx }) {
  return (
    <div className="dp-panel dp-empty"><div className="art"><FolderSimple size={34} /></div><h2>Development deploys from a local folder</h2>
      <p>Point Harpoon at a directory with a Dockerfile and deploy it to your local runtime. There are no automatic triggers in Development.</p>
      {btn("btn--primary", () => c.open({ kind: "deploy" }), <><FolderSimple size={14} /> Deploy from folder…</>, { disabled: !connected(c) })}</div>
  );
}

function DeploymentsTab({ c, app }: { c: Ctx; app: AppModel }) {
  return (
    <div className="dp-tablewrap">
      <div className="dp-filters"><input className="dp-input" placeholder="Search revision or message" aria-label="Search deployments" /><span className="dp-spacer" /><span className="dp-faint">{app.deployments.length} deployments · retention: 30 d</span></div>
      <table className="dp-table"><thead><tr><th>Deployment</th><th>Status</th><th>Revision</th><th className="hide-compact">Trigger</th><th className="hide-compact">Duration</th><th>When</th></tr></thead><tbody>
        {app.deployments.map((x) => (
          <tr key={x.id} className="is-click" onClick={() => c.setRoute({ screen: "dep", depId: x.id })} tabIndex={0} onKeyDown={(e) => e.key === "Enter" && c.setRoute({ screen: "dep", depId: x.id })}>
            <td><b>{x.id}</b><div className="sub">{x.releaseId ? `release ${x.releaseId}` : "no release"}</div></td>
            <td><div className="dp-row dp-row--wrap"><DeploymentPills d={x} /></div>{x.failedAt ? <div className="sub">failed at {x.failedAt.replace("_", " ")}</div> : null}</td>
            <td><span className="mono">{x.revision.sha}</span><div className="sub">{x.revision.message}</div></td>
            <td className="hide-compact">{x.trigger}</td><td className="hide-compact">{x.durationLabel}</td><td>{formatAge(x.ageMin)}</td>
          </tr>
        ))}
      </tbody></table>
    </div>
  );
}

function LogsTab({ c, app }: { c: Ctx; app: AppModel }) {
  const [which, setWhich] = useState<"build" | "runtime">("build");
  const [filter, setFilter] = useState("");
  const [follow, setFollow] = useState(true);
  const live = app.deployments.find((d) => d.status === "running");
  const target = live ?? app.deployments.find((d) => d.buildLog.length) ?? null;
  const runtimeOk = connected(c) && !!app.currentReleaseId;
  const lines: LogLine[] = which === "build" ? target?.buildLog ?? [] : runtimeOk ? RUNTIME_LOG : [];
  const bar = (
    <>
      <div className="dp-seg"><button aria-pressed={which === "build"} onClick={() => setWhich("build")}>Build{live ? <span className="dp-pill dp-pill--info dp-pill--live" style={{ height: 16 }}>live</span> : null}</button><button aria-pressed={which === "runtime"} onClick={() => setWhich("runtime")}>Application</button></div>
      <input className="dp-input" placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter log" />
      <label className="dp-muted"><input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> Follow</label>
      <span className="dp-spacer" />
      <span className="dp-faint">{which === "build" ? `${target?.id ?? "no deployment"} · stored with the deployment record (simulated)` : "live from Docker (simulated)"}</span>
      <button className="btn btn--secondary btn--sm" onClick={() => void navigator.clipboard?.writeText(lines.map((l) => l.text).join("\n"))}><Copy size={14} /> Copy</button>
    </>
  );
  return (
    <>
      {which === "runtime" && !runtimeOk && <Banner tone="info" title="Application logs need a running container">{connected(c) ? "No release is running, so there is nothing to stream." : "Harpoon is not connected, so live application logs are unavailable. Build logs stay readable."}</Banner>}
      {live && which === "build" && <div className="dp-bar dp-bar--live" style={{ marginBottom: 10 }} role="progressbar" aria-label="build in progress"><i style={{ width: `${Math.min(95, 15 + live.buildLog.length * 7)}%` }} /></div>}
      <LogPanel lines={lines} bar={bar} filter={filter} follow={follow} label={which === "build" ? "Build log" : "Application log"} />
      <p className="dp-faint" style={{ fontSize: 11 }}>Known secret values are masked in what Harpoon stores and shows (best effort, any length). Transformed values, application output in Docker's own log store, and <code>docker inspect</code> are not covered.</p>
    </>
  );
}

function MetricsTab({ c, app }: { c: Ctx; app: AppModel }) {
  const st = appStatus(app, c.runtime);
  if (!st.live || !app.resources || !app.currentReleaseId) {
    return <div className="dp-panel dp-empty"><h2>{st.live ? "No running container" : "Not connected"}</h2><p>{st.live ? "Resource charts need a running container. Harpoon does not estimate missing data." : "Harpoon is not connected, so resource charts and health checks are unavailable. Nothing here is shown as live."}</p></div>;
  }
  const r = app.resources;
  const tile = (t: string, v: string, seed: number) => <div className="dp-panel" key={t}><div className="dp-panel__body"><div className="dp-metric"><span>{t}</span><strong>{v}</strong><Spark seed={seed} /></div></div></div>;
  return (
    <>
      <div className="dp-grid dp-grid--3">{tile("CPU", `${r.cpuPct}%`, 11)}{tile("Memory", `${r.memMiB} / ${r.memLimitMiB} MiB`, 23)}{tile("Network I/O", "1.2 MB / 0.4 MB", 41)}</div>
      <section className="dp-panel" style={{ marginTop: 14 }}>
        <div className="dp-panel__head"><h2 className="dp-h2">Health evidence</h2><span className="dp-spacer" /><Evidence /></div>
        <div className="dp-panel__body"><table className="dp-table"><tbody>
          <tr><td>Docker HEALTHCHECK</td><td><Pill tone="ok">healthy</Pill></td><td className="dp-muted">reported by Docker (simulated)</td></tr>
          <tr><td>HTTP <code>/healthz</code></td><td><Pill tone="ok">200 · 2 ms</Pill></td><td className="dp-muted">probed by Harpoon every 30 s while this view is open</td></tr>
          <tr><td>Preliminary check (before cutover)</td><td><Pill tone="ok">passed</Pill></td><td className="dp-muted">6 s on a temporary port</td></tr>
        </tbody></table></div>
      </section>
      <Banner tone="info" title="How this differs from Vercel">One long-lived container, not serverless functions; loopback ports instead of managed domains and TLS; an ARM64 guest; local filesystem state. {EVIDENCE_NOTE}</Banner>
    </>
  );
}

function VariablesTab({ c, app }: { c: Ctx; app: AppModel }) {
  const locked = c.state.keychain === "locked";
  return (
    <>
      {locked && <Banner tone="err" icon={<Lock size={18} />} title="Keychain is locked" actions={btn("btn--primary", () => {}, <><Key size={14} /> How to unlock</>)}>Harpoon can't read secret values. Deployments that need them will fail <b>before any change</b> — nothing is built, stopped or replaced. History and logs still work.</Banner>}
      <div className="dp-row dp-row--wrap" style={{ marginBottom: 10 }}>
        <div className="dp-seg"><button aria-pressed="true">{c.route.env[0].toUpperCase() + c.route.env.slice(1)}</button><button aria-pressed="false" disabled title="Not part of this mock">Compare with Preview</button></div>
        <span className="dp-spacer" /><span className="dp-faint hide-compact">Values resolve at deploy time; releases keep a snapshot. Changing a value affects <b>new</b> deployments only.</span>
        <button className="btn btn--primary" onClick={() => c.open({ kind: "variable" })}><Plus size={14} /> Add variable</button>
      </div>
      <div className="dp-tablewrap"><table className="dp-table"><thead><tr><th>Name</th><th>Type</th><th>Value</th><th className="hide-compact">Used in</th><th className="hide-compact">Scope</th><th className="hide-compact">Version</th><th /></tr></thead><tbody>
        {app.variables.map((v) => (
          <tr key={`${v.scope}:${v.name}`}>
            <td className="mono">{v.name}</td>
            <td>{v.type === "secret" ? <Pill tone="warn" icon={<Key size={11} />}>Secret</Pill> : <Pill>Config</Pill>}</td>
            <td>{v.type === "secret" ? <><span className="dp-secret">{locked ? "locked" : "••••••••"}</span> <span className="dp-faint">write-only</span></> : <code>{v.value}</code>}</td>
            <td className="hide-compact"><span className="dp-flag">{v.used}</span></td><td className="hide-compact dp-muted">{v.scope}</td>
            <td className="hide-compact dp-faint">{v.type === "secret" ? `v${v.version}${v.changedAgoMin != null ? ` · ${formatAge(v.changedAgoMin)}` : ""}` : "—"}</td>
            <td style={{ textAlign: "right" }}>{btn("btn--secondary", () => c.open({ kind: "variable", name: v.name }), v.type === "secret" ? "Replace" : "Edit", { sm: true })}</td>
          </tr>
        ))}
      </tbody></table></div>
      <section className="dp-panel" style={{ marginTop: 14 }}><div className="dp-panel__head"><h2 className="dp-h2">Differences · Production vs. last release</h2></div><div className="dp-panel__body"><table className="dp-table"><tbody>
        <tr className="dp-diff--chg"><td className="mono">STRIPE_KEY</td><td colSpan={2} className="dp-muted">secret version v2 → v3 (value never shown)</td></tr>
        <tr className="dp-diff--add"><td className="mono">REGION_DEFAULT</td><td colSpan={2} className="dp-muted">added · build + runtime → new build required</td></tr>
      </tbody></table></div></section>
      <p><Evidence>The real feature will keep secret values in the macOS Keychain; this mock stores none. Once injected into a container, a runtime variable is visible to docker inspect and to the container itself.</Evidence></p>
    </>
  );
}

function SettingsTab({ app }: { c: Ctx; app: AppModel }) {
  const f = (label: string, value: string, hint?: string) => <div className="dp-field" key={label}><label>{label}</label><input className="dp-input" defaultValue={value} readOnly aria-readonly="true" />{hint ? <small>{hint}</small> : null}</div>;
  const s = app.settings;
  return (
    <div className="dp-grid dp-grid--2">
      <section className="dp-panel"><div className="dp-panel__head"><h2 className="dp-h2">Build</h2><span className="dp-spacer" /><Pill tone="info">Detected: Dockerfile</Pill></div><div className="dp-panel__body dp-col">
        {f("Dockerfile", s.dockerfile)}{f("Build context", s.context)}{f("Target stage", "(default)")}{f("Build arguments", s.buildArgs || "(none)")}
        <small className="dp-faint">Secrets are never build arguments; they use BuildKit secret mounts.</small></div></section>
      <section className="dp-panel"><div className="dp-panel__head"><h2 className="dp-h2">Deployment</h2></div><div className="dp-panel__body dp-col">
        {f("Container port", String(s.port ?? ""), "Published on a loopback port chosen by Harpoon")}{f("Health check", s.healthCheck, "Evidence, not a guarantee")}
        {f("Replacement", "Stop old, start new", "Interruption is shown before you confirm and recorded afterwards")}{f("Keep earlier releases", String(s.keepReleases))}</div></section>
      <section className="dp-panel"><div className="dp-panel__head"><h2 className="dp-h2">Automation</h2></div><div className="dp-panel__body dp-col">
        {f("Source", `${app.source.label}`, "Read-only deploy key · host key pinned")}{f("Production branch", s.productionBranch)}{f("Poll interval", `${s.pollSeconds} s`)}
        <div className="dp-evidence"><Warning size={12} />Automatic = while Harpoon is running. Unattended deployment is not offered yet.</div></div></section>
      <section className="dp-panel"><div className="dp-panel__head"><h2 className="dp-h2">Danger zone</h2></div><div className="dp-panel__body dp-col">
        <p className="dp-muted" style={{ margin: 0 }}>Removing the application deletes its records and labelled containers/images. Volumes and unlabelled resources are never deleted.</p>
        <div><button className="btn btn--destructive" disabled title="Not part of this mock"><Trash size={14} /> Remove application…</button></div></div></section>
      <p className="dp-faint" style={{ gridColumn: "1 / -1", fontSize: 11 }}>These fields are read-only in the mock; editing settings is part of the real feature.</p>
    </div>
  );
}

// ------------------------------------------------------------------ deployment detail

export function DeploymentScreen({ c }: { c: Ctx }) {
  const app = c.state.apps[c.route.appId];
  const d = app?.deployments.find((x) => x.id === c.route.depId) ?? app?.deployments[0];
  if (!app || !d) return <Banner tone="warn" title="Deployment not found">This simulated deployment no longer exists in the current scenario.</Banner>;
  const ok = connected(c);
  const r = d.recovery;
  const fail = d.status === "failed" || r?.outcome === "in_progress";
  const rel = d.releaseId ? app.releases.find((x) => x.id === d.releaseId) : null;
  const canRollback = !!rel && rel.id !== app.currentReleaseId;
  return (
    <>
      <div className="dp-row dp-row--wrap" style={{ marginBottom: 12 }}>
        <button className="btn btn--ghost btn--sm" onClick={() => c.setRoute({ screen: "app", tab: "deployments" })}>← {app.name}</button>
        <DeploymentPills d={d} /><span className="mono">{d.revision.sha}</span><span>{d.revision.message}</span><Tag>simulated</Tag><span className="dp-spacer" />
        <div className="dp-actions">
          {r?.outcome === "restore_failed" && btn("btn--primary", () => c.open({ kind: "recover" }), <><ArrowCounterClockwise size={14} /> Retry recovery</>, { disabled: !ok })}
          {(d.status === "failed" || d.status === "interrupted") && btn("btn--primary", () => c.open({ kind: "retry", depId: d.id }), <><ArrowsClockwise size={14} /> Retry deployment</>, { disabled: !ok })}
          {canRollback && btn("btn--secondary", () => c.open({ kind: "rollback", releaseId: rel!.id }), <><ArrowCounterClockwise size={14} /> Roll back to {rel!.id}</>, { disabled: !ok })}
          {btn("btn--secondary", () => c.open({ kind: "redeploy" }), <><RocketLaunch size={14} /> Redeploy this revision</>, { disabled: !ok })}
        </div>
      </div>
      {fail && d.failure && (
        <Banner tone={r?.outcome === "restore_failed" ? "err" : "warn"} title={r ? recoveryHeadline(r.outcome) : "The deployment failed"}>{d.failure}{d.nothingStopped ? " Nothing was stopped." : ""}</Banner>
      )}
      <div className="dp-grid dp-grid--main">
        <div className="dp-col">
          {r && (
            <section className="dp-panel">
              <div className="dp-panel__head"><h2 className="dp-h2">Recovery attempt</h2><span className="dp-spacer" /><Pill tone={recoveryTone(r.outcome)} live={r.outcome === "in_progress"}>{recoveryLabel(r.outcome)}</Pill></div>
              <div className="dp-panel__body">
                <p style={{ margin: "0 0 10px" }}>{r.detail}</p>
                <dl className="dp-kv">
                  <dt>Interruption</dt><dd>{d.interruption ? (d.interruption.ongoing ? (d.interruption.label === "ongoing" ? "ongoing" : `ongoing · ${d.interruption.label}`) : d.interruption.label) : "—"}</dd>
                  <dt>Restoration</dt><dd>{r.outcome === "in_progress" ? "in progress · 23 s of 60 s budget" : r.durationLabel ?? "—"}</dd>
                  {r.reason ? <><dt>Reason</dt><dd>{reasonLabel(r.reason)}</dd></> : null}
                  <dt>Not promised</dt><dd className="dp-muted">Zero downtime or guaranteed recovery. A restored release may be incompatible with data the new release touched.</dd>
                </dl>
                {r.outcome === "in_progress" && <div className="dp-bar dp-bar--live" style={{ marginTop: 10 }}><i style={{ width: "38%" }} /></div>}
              </div>
            </section>
          )}
          <section className="dp-panel"><div className="dp-panel__head"><h2 className="dp-h2">Timeline</h2></div><div className="dp-panel__body"><Steps steps={d.steps} /></div></section>
          <section>
            <div className="dp-h2" style={{ marginBottom: 8 }}>{d.failedAt === "build" ? "Build log — error highlighted" : "Build log"}</div>
            <LogPanel lines={d.buildLog} label="Build log" bar={<><span className="dp-faint">stored with this deployment (simulated) · secrets masked best-effort</span><span className="dp-spacer" /><button className="btn btn--secondary btn--sm" onClick={() => void navigator.clipboard?.writeText(d.buildLog.map((l) => l.text).join("\n"))}><Copy size={14} /> Copy</button></>} />
          </section>
        </div>
        <div className="dp-col">
          <section className="dp-panel"><div className="dp-panel__head"><h2 className="dp-h2">Details</h2></div><div className="dp-panel__body"><dl className="dp-kv">
            <dt>Trigger</dt><dd>{d.trigger}</dd><dt>Revision</dt><dd><span className="mono">{d.revision.sha}</span> on {d.revision.branch}<div className="dp-faint">{d.revision.message} · {d.revision.author}</div></dd>
            <dt>Started</dt><dd>{formatAge(d.ageMin)}</dd><dt>Duration</dt><dd>{d.durationLabel}</dd><dt>Release</dt><dd>{d.releaseId ?? "none (no image was produced)"}</dd><dt>Interruption</dt><dd>{d.interruption ? (d.interruption.ongoing && d.interruption.label !== "ongoing" ? `ongoing · ${d.interruption.label}` : d.interruption.label) : "none"}</dd>
          </dl>{d.note ? <p className="dp-faint" style={{ fontSize: 11, margin: "10px 0 0" }}>{d.note}</p> : null}</div></section>
          <section className="dp-panel"><div className="dp-panel__head"><h2 className="dp-h2">Configuration snapshot</h2></div><div className="dp-panel__body"><dl className="dp-kv">
            <dt>Image</dt><dd className="mono">harpoon/storefront-{app.name}:{d.id}</dd>
            <dt>Variables</dt><dd>{rel ? `${rel.secrets.length} secret${rel.secrets.length === 1 ? "" : "s"} pinned (${rel.secrets.map((s) => `${s.name}@v${s.version}`).join(", ")})` : "snapshot is created when a build succeeds"}</dd>
            <dt>Port</dt><dd>{app.settings.port ?? "—"}</dd></dl>
            <p className="dp-faint" style={{ fontSize: 11, margin: "10px 0 0" }}>Rollback uses this snapshot. If a pinned secret version has been deleted, this release can't be restored — you'll be told before you confirm.</p></div></section>
          <section className="dp-panel"><div className="dp-panel__head"><h2 className="dp-h2">Docker objects</h2></div><div className="dp-panel__body"><div className="dp-muted" style={{ fontSize: 12 }}>Would be labelled <code>io.harpoon.deployment={d.id}</code></div>
            <div style={{ marginTop: 8 }}>{d.releaseId ? <span className="mono" title="Simulated — this container does not exist in your Containers view">{app.projectId}-{app.name}-{d.releaseId}</span> : <span className="dp-faint">none</span>} <Tag>simulated</Tag></div></div></section>
        </div>
      </div>
    </>
  );
}
