import { compactExposure, describeExposure, exposureOf, parseDockerPorts, type HostListeners, type PortMapping } from "../lib/portExposure.ts";

/**
 * A container's published ports as two statements that are never merged: what Docker reports (a mapping inside Harpoon's VM,
 * often `0.0.0.0` and `[::]`) and what is actually listening on this Mac (lsof). When the host check has not run or failed it
 * says "not verified"; it does not guess.
 */
export function PortsLine({ ports, mappings, listeners }: { ports?: string | null; mappings?: PortMapping[]; listeners: HostListeners | undefined }) {
  const m = mappings ?? parseDockerPorts(ports);
  if (!m.length) return <span className="text-meta mono port-exposure__none">{ports && ports.trim() ? ports : "No published ports"}</span>;
  const e = exposureOf(m, listeners ?? null);
  const c = compactExposure(e);
  const full = describeExposure(e);
  return (
    <span className="port-exposure" title={[full.docker, ...full.host].join("\n")}>
      <span className="port-exposure__docker text-meta mono">{c.docker}</span>
      <span className="port-exposure__host text-meta" data-tone={c.tone}>{c.host}</span>
    </span>
  );
}
