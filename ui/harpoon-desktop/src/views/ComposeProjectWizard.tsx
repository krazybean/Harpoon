import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CheckCircle, FolderOpen, Warning, X, Stack } from "@phosphor-icons/react";

type ComposeFolder = { folder: string; files: string[] };
type ComposeService = { name: string; image?: string | null; build: boolean; ports: string[]; mounts: string[]; profiles: string[] };
type ComposePreview = {
  folder: string; files: string[]; project: string; services: ComposeService[];
  networks: string[]; volumes: string[]; warnings: string[]; existingContainers: number; fingerprint: string;
};
type ComposeLaunchResult = { project: string; detail: string };

const BASE_FILES = ["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"];
const bodyStyle = { display: "flex", flexDirection: "column" as const, gap: 12, maxHeight: "65vh", overflowY: "auto" as const };
const panelStyle = { border: "1px solid var(--border)", borderRadius: 8, padding: 12, background: "var(--panel-header)" };

export function ComposeProjectWizard({ onClose, onLaunched }: {
  onClose: () => void;
  onLaunched: (project: string) => Promise<unknown>;
}) {
  const [folder, setFolder] = useState<ComposeFolder | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [preview, setPreview] = useState<ComposePreview | null>(null);
  const [pending, setPending] = useState<"pick" | "preview" | "launch" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [finished, setFinished] = useState<string | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    closeRef.current?.focus();
    const esc = (event: KeyboardEvent) => { if (event.key === "Escape" && pending !== "launch") onClose(); };
    document.addEventListener("keydown", esc);
    return () => document.removeEventListener("keydown", esc);
  }, [onClose, pending]);

  const inspect = async (source: ComposeFolder, files: string[]) => {
    setSelected(files);
    setPreview(null);
    setError(null);
    setPending("preview");
    try {
      const result = await invoke<ComposePreview>("preview_compose_project", { folder: source.folder, files });
      setPreview(result);
    } catch (err) {
      setError(String(err));
    } finally { setPending(null); }
  };
  const pick = async () => {
    setPending("pick");
    setError(null);
    try {
      const source = await invoke<ComposeFolder | null>("pick_compose_folder");
      if (!source) return;
      setFolder(source);
      setFinished(null);
      await inspect(source, [source.files.find(file => BASE_FILES.includes(file)) ?? source.files[0]]);
    } catch (err) {
      setError(String(err));
      setPending(null);
    }
  };
  const launch = async () => {
    if (!folder || !preview || pending || preview.existingContainers > 0 || preview.warnings.some(w => /not supported|not been verified/i.test(w))) return;
    setPending("launch");
    setError(null);
    try {
      const result = await invoke<ComposeLaunchResult>("launch_compose_project", {
        folder: folder.folder, files: selected, expectedFingerprint: preview.fingerprint,
      });
      setFinished(result.detail);
      await onLaunched(result.project);
    } catch (err) {
      setError(String(err));
    } finally { setPending(null); }
  };

  const possibleOverrides = folder?.files.filter(name => !BASE_FILES.includes(name)) ?? [];
  return (
    <div className="dialog-scrim" role="presentation">
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="compose-import-title" style={{ width: "min(680px, calc(100vw - 32px))", maxWidth: 680, maxHeight: "calc(100vh - 40px)", overflowY: "auto" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <h2 className="dialog-title" id="compose-import-title" style={{ margin: 0 }}>Create from Docker Compose</h2>
          <button ref={closeRef} className="btn btn--ghost btn--sm" onClick={onClose} disabled={pending === "launch"} aria-label="Close Compose project wizard"><X size={16} /></button>
        </div>
        <p className="text-meta" style={{ margin: "8px 0 16px" }}>Inspect a local Compose project, then explicitly create and start its services in the Harpoon Docker context.</p>
        <div style={bodyStyle}>
          <div style={panelStyle}>
            <div className="text-body" style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}><FolderOpen size={16} /> <strong>1 · Select source</strong></div>
            <div className="text-meta" style={{ overflowWrap: "anywhere", marginBottom: 10 }}>{folder?.folder ?? "No folder selected"}</div>
            <button className="btn btn--secondary" onClick={pick} disabled={!!pending}>
              <FolderOpen size={15} /> {pending === "pick" ? "Opening folder picker…" : folder ? "Change folder" : "Browse for folder"}
            </button>
            {folder && <>
              <label className="text-meta" htmlFor="compose-base" style={{ display: "block", marginTop: 12 }}>Compose file</label>
              <select id="compose-base" className="dp-input" style={{ width: "100%", marginTop: 4 }} value={selected[0] ?? ""} disabled={!!pending}
                onChange={event => void inspect(folder, [event.target.value, ...selected.filter(name => !BASE_FILES.includes(name))])}>
                {folder.files.filter(name => BASE_FILES.includes(name)).map(name => <option key={name} value={name}>{name}</option>)}
              </select>
              {possibleOverrides.length > 0 && <fieldset style={{ border: 0, padding: 0, margin: "12px 0 0" }}>
                <legend className="text-meta">Optional override files (not automatically applied)</legend>
                {possibleOverrides.map(name => <label key={name} style={{ display: "block", marginTop: 6 }}>
                  <input type="checkbox" checked={selected.includes(name)} disabled={!!pending}
                    onChange={event => void inspect(folder, event.target.checked ? [...selected, name] : selected.filter(file => file !== name))} /> {name}
                </label>)}
              </fieldset>}
            </>}
          </div>

          {pending === "preview" && <p role="status" className="text-meta">Validating Compose configuration…</p>}
          {preview && <div style={panelStyle}>
            <div className="text-body" style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 10 }}><CheckCircle size={16} color="var(--success-fg)" /> <strong>2 · Inspect configuration</strong></div>
            <div className="text-meta">Project <b>{preview.project}</b> · {preview.services.length} service(s) · Docker context <b>harpoon</b></div>
            {preview.services.map(service => <div key={service.name} style={{ borderTop: "1px solid var(--border)", paddingTop: 9, marginTop: 9 }}>
              <div className="text-body"><strong>{service.name}</strong> · {service.build ? "Build from source" : service.image ?? "Image not specified"}</div>
              {service.ports.map((port, index) => <div key={index} className="text-meta">Port: {port}</div>)}
              {service.mounts.map((mount, index) => <div key={index} className="text-meta" style={{ overflowWrap: "anywhere" }}>Mount: {mount}</div>)}
              {service.profiles.length > 0 && <div className="text-meta">Profiles: {service.profiles.join(", ")} (not started by default)</div>}
            </div>)}
            <div className="text-meta" style={{ marginTop: 12 }}>Networks: {preview.networks.join(", ") || "none declared"} · Volumes: {preview.volumes.join(", ") || "none declared"}</div>
          </div>}
          {preview && <div style={panelStyle}>
            <div className="text-body" style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}><Stack size={16} /> <strong>3 · Review launch</strong></div>
            <p className="text-meta" style={{ marginBottom: 8 }}>Create and Start executes Docker Compose <code>up -d --build --no-recreate</code> against Harpoon only. It may build or pull images and create containers, networks, and volumes. It does not delete resources, prune, switch Docker contexts, or change source files.</p>
            <p className="text-meta">This is standard Compose, not a managed Harpoon deployment. No automatic recovery or rollback is promised.</p>
            {preview.existingContainers > 0 && <p role="alert" style={{ color: "var(--error-fg)", marginTop: 8 }}>An existing stack uses this Compose project name ({preview.existingContainers} containers). Launch is blocked to prevent recreation. Open it in Stacks instead.</p>}
            {preview.warnings.map((warning, index) => <p key={index} role="status" className="text-meta" style={{ display: "flex", gap: 6, marginTop: 8 }}><Warning size={15} />{warning}</p>)}
          </div>}
          {error && <p role="alert" style={{ color: "var(--error-fg)", overflowWrap: "anywhere" }}>{error}</p>}
          {finished && <p role="status" style={{ color: "var(--success-fg)" }}>{finished}</p>}
        </div>
        <div className="dialog-actions" style={{ marginTop: 16 }}>
          <button onClick={onClose} className="btn btn--secondary" disabled={pending === "launch"}>Cancel</button>
          <button onClick={() => void launch()} className="btn btn--primary"
            disabled={!preview || !!pending || preview.existingContainers > 0 || preview.warnings.some(w => /not supported|not been verified/i.test(w))}>
            {pending === "launch" ? "Creating stack…" : "Create and Start"}
          </button>
        </div>
      </div>
    </div>
  );
}
