import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { ArrowCounterClockwise, Cube, Check, FolderSimple, GitBranch, Info, Key, Pause, Play, RocketLaunch, Warning } from "@phosphor-icons/react";
import type { AppModel, VariableScope, VariableType, VariableUse } from "../../lib/deployTypes.ts";
import { appStatus, latestDeployment } from "../../lib/deployStore.ts";
import { liveBlock } from "../../lib/deployEngine.ts";
import { ROLLBACK_WARNING } from "../../lib/deployPresentation.ts";
import { REV } from "../../lib/deployMock.ts";
import type { Ctx, DialogState } from "./context.ts";
import { Banner } from "./parts.tsx";

function Dialog({ title, sub, wide, onClose, children, footer }: { title: string; sub?: string; wide?: boolean; onClose: () => void; children: ReactNode; footer: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>("input, select, button.btn--primary, button")?.focus();
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="dp-scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className={`dp-dialog ${wide ? "dp-dialog--wide" : ""}`} role="dialog" aria-modal="true" aria-label={title} ref={ref}>
        <div className="dp-dialog__head"><h2>{title}</h2>{sub ? <p>{sub}</p> : null}</div>
        <div className="dp-dialog__body">{children}</div>
        <div className="dp-dialog__foot"><button className="btn btn--ghost" onClick={onClose}>Cancel</button>{footer}</div>
      </div>
    </div>
  );
}

const Plan = ({ children }: { children: ReactNode }) => <div className="dp-plan">{children}</div>;
const Row = ({ icon, children }: { icon: ReactNode; children: ReactNode }) => <div>{icon}<span>{children}</span></div>;

export function DeployDialogs({ c, dialog }: { c: Ctx; dialog: DialogState }) {
  const close = () => c.open(null);
  const app: AppModel | undefined = c.state.apps[c.route.appId] ?? Object.values(c.state.apps)[0];
  const locked = c.state.keychain === "locked";
  const st = app ? appStatus(app, c.runtime) : null;
  const hasPrev = !!app?.currentReleaseId;

  const gate = c.engine ? liveBlock(c.state) : null;
  const busy = !!c.engine?.busy;
  /** Run an engine operation, close the dialog, and let the page-level result banner show the engine's own answer. */
  const act = (op: Record<string, unknown>, after?: () => void) => { void c.engine!.run(op); close(); after?.(); };
  if (dialog.kind === "create") return c.engine ? <EngineCreateDialog c={c} close={close} /> : <CreateDialog c={c} dialog={dialog} close={close} />;
  if (dialog.kind === "variable") return <VariableDialog c={c} name={dialog.name} close={close} />;
  if (!app) return null;

  if (dialog.kind === "deploy" || dialog.kind === "redeploy" || dialog.kind === "retry" || dialog.kind === "promote") {
    const retryDep = dialog.kind === "retry" ? app.deployments.find((d) => d.id === dialog.depId) : null;
    const branch = app.settings.productionBranch || undefined;
    const revision = dialog.kind === "promote" ? { ...REV.held1, sha: app.previews[0]?.sha ?? REV.held1.sha, message: "preview revision", branch: app.previews[0]?.branch ?? "preview" }
      : retryDep ? retryDep.revision : dialog.kind === "redeploy" ? (app.releases.find((r) => r.id === app.currentReleaseId)?.revision ?? REV.next) : REV.next;
    const title = dialog.kind === "retry" ? `Retry deployment ${retryDep?.id}` : dialog.kind === "promote" ? "Promote to Production" : dialog.kind === "redeploy" ? `Redeploy ${app.name}` : `Deploy ${app.name} to Production`;
    const trigger = dialog.kind === "retry" ? "retry" : dialog.kind === "promote" ? "promote" : dialog.kind === "redeploy" ? "redeploy" : "manual";
    return (
      <Dialog title={title} sub={c.engine ? "Review the plan before anything changes." : "Review the plan before anything changes. (Simulated: confirming starts a mock deployment.)"} onClose={close}
        footer={<button className="btn btn--primary" disabled={locked || !!c.state.job || (!!c.engine && (!!gate || busy || !branch || dialog.kind === "promote"))} title={gate ?? undefined}
          onClick={() => { if (c.engine) { act({ op: "deploy", repo: app.name, branch }, () => c.setRoute({ screen: "app", tab: "logs" })); return; } c.dispatch({ type: "start-deploy", appId: app.id, trigger, revision, env: "production" }, true); c.open(null); c.setRoute({ screen: "app", tab: "logs" }); }}><RocketLaunch size={14} /> {dialog.kind === "promote" ? "Promote" : "Deploy"}</button>}>
        {c.engine && gate && <Banner tone="info" title="This build cannot deploy">{gate}</Banner>}
        <Plan>
          <Row icon={<Cube size={14} />}>{c.engine ? <>Fetch the tip of <span className="mono">{branch ?? "the configured branch"}</span>, then build it from <code>{app.settings.dockerfile}</code> — <b>the running release is not touched</b></> : dialog.kind === "promote" ? <>A <b>new deployment</b> of <span className="mono">{revision.sha}</span> under Production configuration — Production secrets are resolved, not Preview's. The image is <b>rebuilt</b> because a build-time variable differs.</> : <>Build <span className="mono">{revision.sha}</span> from <code>{app.settings.dockerfile}</code> — <b>the running release is not touched</b></>}</Row>
          <Row icon={<Check size={14} />}>Run a preliminary health check on the new build using a temporary port</Row>
          {hasPrev ? <>
            <Row icon={<Warning size={14} />}><b>Brief interruption expected.</b> To switch releases Harpoon stops <b>{app.currentReleaseId}</b> and starts the new one ({c.engine ? "the interruption is measured and recorded" : "about 4 s in this simulation — real timing is not measured yet"}).</Row>
            <Row icon={<ArrowCounterClockwise size={14} />}>If the new release fails after the switch, Harpoon makes <b>one bounded attempt (60 s)</b> to restore {app.currentReleaseId} and tells you the outcome. No guarantee.</Row>
          </> : <Row icon={<Info size={14} />}>Nothing is running yet, so there is no interruption and nothing to restore.</Row>}
        </Plan>
        {locked && <Banner tone="err" icon={<Key size={18} />} title="Keychain is locked">This deployment needs its pinned secrets. It would fail before any change, so it is blocked here.</Banner>}
        {c.state.job && <Banner tone="info" title="Another simulated deployment is running">One deployment runs at a time per runtime. Wait for it to finish.</Banner>}
        {c.engine ? null : <small className="dp-faint">Simulated outcome of this deployment: <b>{c.state.nextOutcome.replace("-", " ")}</b> (change it in the simulation bar).</small>}
      </Dialog>
    );
  }

  if (dialog.kind === "rollback") {
    const rel = app.releases.find((r) => r.id === dialog.releaseId);
    if (!rel) return null;
    return (
      <Dialog title={`Roll back to release ${rel.id}`} sub={`${rel.revision.message} · ${rel.revision.sha}`} onClose={close}
        footer={<button className="btn btn--primary" disabled={!!c.engine && (!!gate || busy)} title={gate ?? undefined} onClick={() => { if (c.engine) { act({ op: "rollback", app: app.id, release: rel.id }, () => c.setRoute({ screen: "app", tab: "overview" })); return; } c.dispatch({ type: "rollback", appId: app.id, releaseId: rel.id }); close(); c.setRoute({ screen: "app", tab: "overview" }); }}><ArrowCounterClockwise size={14} /> Roll back to {rel.id}</button>}>
        {c.engine && gate && <Banner tone="info" title="This build cannot roll back">{gate}</Banner>}
        <Plan>
          <Row icon={<ArrowCounterClockwise size={14} />}>Restart <b>{rel.id}</b> from its retained image with its pinned configuration snapshot ({rel.secrets.map((s) => `${s.name}@v${s.version}`).join(", ") || "no secrets"})</Row>
          <Row icon={<Warning size={14} />}>Brief interruption{c.engine ? "" : " (about 4 s in this simulation)"}: Harpoon stops {app.currentReleaseId ?? "nothing — the application is down"} first.</Row>
          <Row icon={<Pause size={14} />}>Automatic deployments for this application will be <b>paused</b> until you resume them. New pushes are held, not discarded.</Row>
        </Plan>
        <Banner tone="warn" title="Application rollback only">{ROLLBACK_WARNING.replace("Application rollback only. ", "")}</Banner>
      </Dialog>
    );
  }

  if (dialog.kind === "unlock-help") {
    return (
      <Dialog title="Unlocking the Keychain" sub={c.engine ? "Harpoon could not use your login Keychain." : "Simulated help — Harpoon does not touch your real Keychain here."} onClose={close}
        footer={<button className="btn btn--primary" onClick={close}>Close</button>}>
        <Plan>
          <Row icon={<Key size={14} />}>Secrets live in your macOS login Keychain. It locks when you log out or after a timeout set in <b>Keychain Access</b>.</Row>
          <Row icon={<Check size={14} />}>To unlock it, open <b>Keychain Access</b>, select <b>login</b> and choose <b>File → Unlock Keychain</b>, or log out and back in.</Row>
          <Row icon={<Pause size={14} />}>While it is locked nothing is built, stopped or replaced. Come back and deploy again once it is unlocked.</Row>
          {c.engine ? null : <Row icon={<Cube size={14} />}>In this simulation, use the <b>Keychain</b> control in the simulation bar to switch to unlocked.</Row>}
        </Plan>
      </Dialog>
    );
  }

  if (dialog.kind === "resume") {
    return <ResumeDialog c={c} app={app} close={close} />;
  }

  if (dialog.kind === "recover") {
    const d = latestDeployment(app);
    return (
      <Dialog title={`Retry recovery of ${app.lastKnownReleaseId}`} sub="One bounded attempt (60 s). Nothing else changes." onClose={close}
        footer={<><button className="btn btn--secondary" disabled={!!c.engine && (!!gate || busy)} title={gate ?? undefined} onClick={() => { const rb = app.releases.find((r) => r.id !== app.lastKnownReleaseId); if (rb) { if (c.engine) { act({ op: "rollback", app: app.id, release: rb.id }); return; } c.dispatch({ type: "rollback", appId: app.id, releaseId: rb.id }); } close(); }}>Roll back to {app.releases.find((r) => r.id !== app.lastKnownReleaseId)?.deploymentId ?? "an earlier release"} instead</button>
          <button className="btn btn--primary" disabled={!!c.engine && (!!gate || busy)} title={gate ?? undefined} onClick={() => { if (c.engine) { act({ op: "restore", deployment: d?.id }); return; } c.dispatch({ type: "recover", appId: app.id }); close(); }}><ArrowCounterClockwise size={14} /> Retry recovery</button></>}>
        <Banner tone={locked ? "err" : "info"} icon={<Key size={18} />} title={locked ? "The Keychain is still locked" : "Last attempt failed: a pinned secret was unavailable"}>
          {locked ? `Retrying now would fail again. Unlock the Keychain${c.engine ? "" : " (simulation bar)"}, or roll back to a release that pins different secret versions.` : `${d?.recovery?.detail ?? ""} The Keychain is unlocked now, so a retry can succeed.`}
        </Banner>
      </Dialog>
    );
  }
  void st;
  return null;
}

function ResumeDialog({ c, app, close }: { c: Ctx; app: AppModel; close: () => void }) {
  const held = app.automation.held;
  const [pick, setPick] = useState(held.length ? held[held.length - 1].id : "");
  return (
    <Dialog wide title="Resume automatic deployments" sub={app.automation.state === "paused" ? "Paused. Choose explicitly what happens to the events that arrived meanwhile." : "Automation is already active."} onClose={close}
      footer={<>
        <button className="btn btn--secondary" disabled={!!c.engine?.busy} onClick={() => { if (c.engine) { void c.engine.run({ op: "resume", app: app.id, mode: "dismiss" }); close(); return; } c.dispatch({ type: "resume", appId: app.id, mode: "skip" }); close(); }}>Resume without deploying</button>
        <button className="btn btn--primary" disabled={!held.length || !pick || c.state.keychain === "locked" || !!c.state.job || !!c.engine?.busy} title={c.state.keychain === "locked" ? "Keychain is locked — held events are kept until it is unlocked" : c.state.job ? "A deployment is already running — held events are kept" : undefined}
          onClick={() => { if (c.engine) { void c.engine.run({ op: "resume", app: app.id, mode: "revision", revision: held.find((e) => e.id === pick)?.revisionFull }); close(); return; } c.dispatch({ type: "resume", appId: app.id, mode: "deploy", heldEventId: pick }, true); close(); }}><Play size={14} /> {c.engine ? "Resume & select" : "Resume & deploy"}</button>
      </>}>
      <div className="dp-h2">Held events ({held.length})</div>
      <div className="dp-tablewrap"><table className="dp-table"><tbody>
        {held.length ? held.map((e) => (
          <tr key={e.id}><td style={{ width: 30 }}><input type="radio" name="held" checked={pick === e.id} onChange={() => setPick(e.id)} aria-label={`Deploy ${e.revision.sha}`} /></td>
            <td><span className="mono">{e.revision.sha}</span><div className="sub">{e.revision.message}</div></td><td className="dp-faint">{e.ageMin} min ago</td></tr>
        )) : <tr><td className="dp-muted">No held events.</td></tr>}
      </tbody></table></div>
      <Plan>
        <Row icon={<Play size={14} />}>{c.engine ? <><b>Resume &amp; select the revision</b> — it is recorded as chosen and waits for an attended deployment; nothing runs by itself{liveBlock(c.state) ? ", and this build cannot run live deployments" : ""}</> : <><b>Resume &amp; deploy the selected revision</b> — a normal deployment with the interruption notice</>}</Row>
        <Row icon={<Pause size={14} />}><b>Resume without deploying</b> — held events are kept in history as dismissed by you</Row>
      </Plan>
    </Dialog>
  );
}

function CreateDialog({ c, dialog, close }: { c: Ctx; dialog: Extract<DialogState, { kind: "create" }>; close: () => void }) {
  const [step, setStep] = useState(dialog.step);
  const [src, setSrc] = useState(dialog.src);
  const [name, setName] = useState("storefront");
  const srcs: [typeof src, ReactNode, string, string][] = [
    ["git", <GitBranch size={18} key="g" />, "Git repository", "Public HTTPS, or private with a read-only deploy key"],
    ["folder", <FolderSimple size={18} key="f" />, "Local folder", "A directory with a Dockerfile on this Mac"],
    ["image", <Cube size={18} key="i" />, "Docker image", "Run an existing image reference"],
  ];
  return (
    <Dialog wide title="Create project" sub={`Step ${step} of 3 · simulated — nothing is cloned, stored or deployed`} onClose={close}
      footer={<>
        {step > 1 && <button className="btn btn--secondary" onClick={() => setStep((step - 1) as 1 | 2)}>Back</button>}
        {step < 3 ? <button className="btn btn--primary" onClick={() => setStep((step + 1) as 2 | 3)}>Continue</button>
          : <button className="btn btn--primary" onClick={() => { c.dispatch({ type: "create-project", name, source: src }); close(); c.setRoute({ screen: "home" }); }}>Create project</button>}
      </>}>
      <div className="dp-wizard" style={{ padding: 0 }}>{["Source", "Configure", "Review"].map((s, i) => (i + 1 === step ? <b key={s}>{s}</b> : <span key={s}>{s}</span>))}</div>
      {step === 1 && <>
        <div className="dp-field"><label>Source</label>{srcs.map(([k, icon, t, s]) => <button key={k} className="dp-choice" aria-pressed={src === k} onClick={() => setSrc(k)}>{icon}<div><b>{t}</b><span>{s}</span></div></button>)}</div>
        {src === "git" && <div className="dp-field"><label htmlFor="repo">Repository</label><input id="repo" className="dp-input" defaultValue="git@github.com:acme/storefront.git" /><small>Harpoon will pin the host key on first use and keep the deploy key in the macOS Keychain. (Not implemented in this mock.)</small></div>}
      </>}
      {step === 2 && <>
        <Banner tone="ok" icon={<Check size={18} />} title="Detected 2 Dockerfiles">api/Dockerfile and web/Dockerfile — create one application for each? (Simulated detection.)</Banner>
        <div className="dp-grid dp-grid--2">
          <div className="dp-field"><label htmlFor="pname">Project name</label><input id="pname" className="dp-input" value={name} onChange={(e) => setName(e.target.value)} /></div>
          <div className="dp-field"><label>Production branch</label><input className="dp-input" defaultValue="main" /></div>
          <div className="dp-field"><label>Container port (api)</label><input className="dp-input" defaultValue="3000" /></div>
          <div className="dp-field"><label>Health check</label><input className="dp-input" defaultValue="HTTP GET /healthz" /></div>
        </div>
        <small className="dp-faint">You can change build and deployment settings later. Detection is a suggestion, never a requirement.</small>
      </>}
      {step === 3 && <Plan>
        <Row icon={<Check size={14} />}>Create project <b>{name || "storefront"}</b> with applications <b>api</b> and <b>web</b></Row>
        <Row icon={<Check size={14} />}>Environments: Development, Preview, Production (all run locally on this Mac)</Row>
        <Row icon={<Key size={14} />}>The real feature stores the deploy key in the macOS Keychain; this simulation stores nothing</Row>
        <Row icon={<Info size={14} />}>Automatic deployments run <b>while Harpoon is open</b> (polling every 60 s)</Row>
        <Row icon={<RocketLaunch size={14} />}>Nothing deploys until you choose <b>Deploy</b></Row>
      </Plan>}
    </Dialog>
  );
}

function VariableDialog({ c, name, close }: { c: Ctx; name?: string; close: () => void }) {
  const app = c.state.apps[c.route.appId];
  const existing = app?.variables.find((v) => v.name === name);
  const [vname, setVname] = useState(name ?? "");
  const [type, setType] = useState<VariableType>(existing?.type ?? "secret");
  const [used, setUsed] = useState<VariableUse>(existing?.used ?? "runtime");
  const [scope, setScope] = useState<VariableScope>(existing?.scope ?? "environment");
  const [value, setValue] = useState(existing?.type === "config" ? existing.value ?? "" : "");
  return (
    <Dialog title={name ? `${type === "secret" ? "Replace" : "Edit"} ${name}` : "Add variable"} sub="Applies to new deployments only. Running releases keep the snapshot they were built with." onClose={close}
      footer={<button className="btn btn--primary" disabled={!vname.trim() || !app || !!c.engine?.busy || (type === "secret" && !value)}
        onClick={() => {
          // A secret's value never leaves this component: only its name, type and scope are dispatched.
          if (c.engine) {
            const slug = scope === "global" ? "global" : scope === "project" ? `project:${app!.projectId}` : scope === "environment" ? `app:${app!.id}@production` : `app:${app!.id}`;
            const flags = { build: used !== "runtime", runtime: used === "runtime" || used === "build+runtime" };
            // the value goes to the engine (config) or to the Keychain via stdin (secret) and is dropped from this component
            void c.engine.run({ op: type === "secret" ? "set-secret" : "set-config", scope: slug, name: vname.trim(), value, ...flags });
            setValue(""); close(); return;
          }
          c.dispatch({ type: "save-variable", appId: app!.id, name: vname.trim(), varType: type, used, scope, value: type === "config" ? value : undefined });
          setValue(""); close();
        }}>{type === "secret" ? "Save as new version" : "Save"}</button>}>
      <div className="dp-field"><label htmlFor="vn">Name</label><input id="vn" className="dp-input mono" value={vname} onChange={(e) => setVname(e.target.value.toUpperCase())} placeholder="API_KEY" readOnly={!!name} /></div>
      <div className="dp-field"><label>Type</label><div className="dp-seg"><button aria-pressed={type === "config"} onClick={() => setType("config")}>Config</button><button aria-pressed={type === "secret"} onClick={() => setType("secret")}>Secret</button></div>
        <small>{type === "secret" ? (c.engine ? "Secrets are write-only after saving. The value goes to the macOS Keychain and is never shown again." : "Secrets are write-only after saving. This simulation keeps no value at all; the real feature stores it in the macOS Keychain.") : "Config values stay readable."}</small></div>
      <div className="dp-field"><label htmlFor="vv">Value</label><input id="vv" className="dp-input" type={type === "secret" ? "password" : "text"} autoComplete="off" value={value} onChange={(e) => setValue(e.target.value)} placeholder={type === "secret" ? "•••••••••••" : ""} /></div>
      <div className="dp-field"><label>Used in</label><select className="dp-select" value={used} onChange={(e) => setUsed(e.target.value as VariableUse)}><option value="runtime">Runtime</option><option value="build">Build</option><option value="build+runtime">Build + runtime</option><option value="build secret">Build secret (BuildKit mount)</option></select>
        <small>Build-time secrets use BuildKit secret mounts, never build arguments.</small></div>
      <div className="dp-field"><label>Scope</label><select className="dp-select" value={scope} onChange={(e) => setScope(e.target.value as VariableScope)}><option value="environment">Environment · Production</option><option value="application">Application · {app?.name ?? "api"}</option><option value="project">Project · {app?.projectId ?? "storefront"}</option><option value="global">Global</option></select></div>
    </Dialog>
  );
}


/** Register a repository (Git URL or local folder) with the real engine. Nothing deploys until the user chooses Deploy. */
function EngineCreateDialog({ c, close }: { c: Ctx; close: () => void }) {
  const [kind, setKind] = useState<"git" | "folder">("git");
  const [name, setName] = useState("");
  const [location, setLocation] = useState("");
  const [branch, setBranch] = useState("main");
  const [port, setPort] = useState("3000");
  const [dockerfile, setDockerfile] = useState("Dockerfile");
  const [health, setHealth] = useState("/");
  const [project, setProject] = useState("");
  const [username, setUsername] = useState("");
  const [token, setToken] = useState("");
  const url = kind === "folder" ? (location.startsWith("file://") ? location : `file://${location}`) : location;
  const valid = /^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name) && location.trim() !== "" && Number(port) > 0 && Number(port) < 65536 && (kind === "git" || location.startsWith("/") || location.startsWith("file:///"));
  const submit = async () => {
    const r = await c.engine!.run({ op: "add-repo", name, url, project: project || null, branch: branch || null, filters: [], containerPort: Number(port), dockerfile: dockerfile || null, context: null, target: null, healthPath: health || null, username: username || null, knownHost: null, webhook: null, app: null, environment: null });
    // a token is handed to the Keychain through standard input, never kept here
    if (r.ok && username && token) await c.engine!.run({ op: "repo-secret", name, kind: "token", value: token });
    setToken("");
  };
  return (
    <Dialog wide title="Add repository" sub="Registers a source with Harpoon's deployment engine. Nothing is built or deployed until you choose Deploy." onClose={close}
      footer={<button className="btn btn--primary" disabled={!valid || c.engine!.busy} onClick={() => { void submit(); close(); c.setRoute({ screen: "home" }); }}>Add repository</button>}>
      <div className="dp-field"><label>Source</label><div className="dp-seg"><button aria-pressed={kind === "git"} onClick={() => setKind("git")}>Git repository</button><button aria-pressed={kind === "folder"} onClick={() => setKind("folder")}>Local folder (a Git repository on this Mac)</button></div></div>
      <div className="dp-grid dp-grid--2">
        <div className="dp-field"><label htmlFor="rname">Name</label><input id="rname" className="dp-input" value={name} onChange={(e) => setName(e.target.value)} placeholder="storefront" /></div>
        <div className="dp-field"><label htmlFor="rloc">{kind === "git" ? "Repository URL" : "Absolute path"}</label><input id="rloc" className="dp-input" value={location} onChange={(e) => setLocation(e.target.value)} placeholder={kind === "git" ? "https://github.com/acme/storefront.git" : "/Users/you/code/storefront"} /></div>
        <div className="dp-field"><label>Branch</label><input className="dp-input" value={branch} onChange={(e) => setBranch(e.target.value)} /></div>
        <div className="dp-field"><label>Container port</label><input className="dp-input" inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} /></div>
        <div className="dp-field"><label>Dockerfile</label><input className="dp-input" value={dockerfile} onChange={(e) => setDockerfile(e.target.value)} /></div>
        <div className="dp-field"><label>Health check path</label><input className="dp-input" value={health} onChange={(e) => setHealth(e.target.value)} /></div>
        <div className="dp-field"><label>Project (optional)</label><input className="dp-input" value={project} onChange={(e) => setProject(e.target.value)} /></div>
        {kind === "git" && <div className="dp-field"><label>Username (private HTTPS only)</label><input className="dp-input" autoComplete="off" value={username} onChange={(e) => setUsername(e.target.value)} /></div>}
      </div>
      {kind === "git" && username && <div className="dp-field"><label htmlFor="rtok">Access token</label><input id="rtok" className="dp-input" type="password" autoComplete="off" value={token} onChange={(e) => setToken(e.target.value)} /><small>Stored in the macOS Keychain. Never shown again.</small></div>}
      <Plan>
        <Row icon={<Check size={14} />}>Record <b>{name || "the repository"}</b> ({kind === "git" ? "Git" : "local"}) tracking <span className="mono">{branch || "its default branch"}</span></Row>
        <Row icon={<Info size={14} />}>Automatic deployments are attended-only: Harpoon checks for new commits when you ask it to.</Row>
        <Row icon={<RocketLaunch size={14} />}>Nothing deploys until you choose <b>Deploy</b></Row>
      </Plan>
    </Dialog>
  );
}
