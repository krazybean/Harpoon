import { settingsProblem, type SourceDetection, type SourceSettings } from "../../lib/sourceSettings.ts";
import { Banner } from "./parts.tsx";

/** Detection result and the overrides that sit on top of it. Empty command fields mean "use what was detected". */
export function SourceSettingsFields({ value, onChange, detection, detectError, networks, readOnly }: {
  value: SourceSettings; onChange: (v: SourceSettings) => void; detection: SourceDetection | null; detectError: string | null; networks: string[]; readOnly?: boolean;
}) {
  const set = (patch: Partial<SourceSettings>) => onChange({ ...value, ...patch });
  const problem = settingsProblem(value);
  const dev = value.style === "development";
  const d = detection && !detection.hasDockerfile ? detection : null;
  return (
    <>
      {detectError ? <Banner tone="warn" title="Harpoon could not detect how to build this folder">{detectError} Add a Dockerfile, or enter the commands below.</Banner> : null}
      {detection?.hasDockerfile ? <Banner tone="info" title="This folder has a Dockerfile">Harpoon builds it as it is. The commands below apply only to a repository without one.</Banner> : null}
      {d ? <Banner tone="ok" title={`Detected ${d.framework}`}>
        <div>Base image <span className="mono">{d.baseImage}</span>. Harpoon writes the build file privately for each build; nothing is added to your repository.</div>
        {(d.notes ?? []).map((n) => <div key={n}>{n}</div>)}
      </Banner> : null}
      <div className="dp-grid dp-grid--2">
        <div className="dp-field"><label htmlFor="ssi">Install command</label><input id="ssi" className="dp-input mono" value={value.install} readOnly={readOnly} placeholder={d?.install ?? "detected"} onChange={(e) => set({ install: e.target.value })} /></div>
        <div className="dp-field"><label htmlFor="ssb">Build command</label><input id="ssb" className="dp-input mono" value={value.build} readOnly={readOnly || dev} placeholder={dev ? "not run in development" : d?.build ?? "none"} onChange={(e) => set({ build: e.target.value })} /></div>
        <div className="dp-field"><label htmlFor="sss">Start command</label><input id="sss" className="dp-input mono" value={value.start} readOnly={readOnly} placeholder={(dev ? d?.devStart ?? d?.start : d?.start) ?? "required"} onChange={(e) => set({ start: e.target.value })} /></div>
        <div className="dp-field"><label htmlFor="ssn">Docker network (optional)</label><input id="ssn" className="dp-input mono" list="ss-networks" value={value.network} readOnly={readOnly} placeholder="Docker's default" onChange={(e) => set({ network: e.target.value })} />
          <datalist id="ss-networks">{networks.map((n) => <option key={n} value={n} />)}</datalist><small>Join an existing network, such as a Compose project's, to reach its services by name.</small></div>
        <div className="dp-field"><label>Style</label><div className="dp-seg"><button type="button" aria-pressed={!dev} disabled={readOnly} onClick={() => set({ style: "production" })}>Production</button><button type="button" aria-pressed={dev} disabled={readOnly} onClick={() => set({ style: "development" })}>Development</button></div>
          <small>{dev ? "Installs and runs the development command; no build step." : "Installs, builds, then runs the production start command."}</small></div>
      </div>
      {problem ? <small style={{ color: "var(--error-fg)" }}>{problem}</small> : null}
    </>
  );
}
