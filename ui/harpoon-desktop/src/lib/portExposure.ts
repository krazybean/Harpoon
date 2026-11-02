// Two different statements about a published port, kept apart:
//  * what DOCKER reports for a container (`0.0.0.0:38000->8080/tcp`): a mapping inside Harpoon's VM, and
//  * what is actually LISTENING on this Mac (`lsof`), which is what decides who can connect.
// Neither is substituted for the other. When the host check could not run, exposure is "not verified", not "none".

export type PortMapping = { hostIp: string; hostPort: number; containerPort: number; proto: string };
export type HostListener = { port: number; address: string; scope: "loopback" | "wildcard" | "interface"; pid: number; command: string };
export type HostListeners = { verified: boolean; error: string | null; checkedMs: number; listeners: HostListener[] } | null;

/** `docker ps` Ports text: `0.0.0.0:38000->8080/tcp, [::]:38000->8080/tcp, 9000/tcp`. Unpublished entries and ranges are left out. */
export function parseDockerPorts(text: string | null | undefined): PortMapping[] {
  const out: PortMapping[] = [];
  for (const part of String(text ?? "").split(",")) {
    const m = /^\s*(\[[0-9a-fA-F:]+\]|[^:\s]+):(\d+)->(\d+)\/(\w+)\s*$/.exec(part);
    if (m) out.push({ hostIp: m[1], hostPort: Number(m[2]), containerPort: Number(m[3]), proto: m[4] });
  }
  return out;
}

export type HostReach = "none" | "loopback" | "network" | "mixed";
export type Exposure = {
  /** Exactly what Docker reported, one entry per mapping. */
  docker: PortMapping[];
  host: {
    state: "verified" | "unverified";
    error: string | null;
    /** One entry per published host port. Only meaningful when `state` is `verified`. */
    ports: { port: number; reach: HostReach; listeners: HostListener[] }[];
  };
};

export const isWildcardIp = (ip: string) => ip === "0.0.0.0" || ip === "[::]" || ip === "*";

export function exposureOf(mappings: PortMapping[], hl: HostListeners): Exposure {
  const ports = [...new Set(mappings.map((m) => m.hostPort))];
  if (!hl || !hl.verified) {
    return { docker: mappings, host: { state: "unverified", error: hl?.error ?? "The host listeners have not been checked.", ports: [] } };
  }
  return {
    docker: mappings,
    host: {
      state: "verified", error: null,
      ports: ports.map((port) => {
        const listeners = hl.listeners.filter((l) => l.port === port);
        const loop = listeners.filter((l) => l.scope === "loopback").length;
        const reach: HostReach = !listeners.length ? "none" : loop === listeners.length ? "loopback" : loop === 0 ? "network" : "mixed";
        return { port, reach, listeners };
      }),
    },
  };
}

/** Short forms for a card: the same two statements, still apart. */
export function compactExposure(e: Exposure): { docker: string; host: string; tone: "ok" | "warn" | "neutral" } {
  const uniq = [...new Set(e.docker.map((m) => `${m.hostIp}:${m.hostPort}→${m.containerPort}`))];
  const docker = uniq.length ? `Docker: ${uniq.join(" · ")}` : "No published ports";
  if (!e.docker.length) return { docker, host: "", tone: "neutral" };
  if (e.host.state === "unverified") return { docker, host: "Mac listener: not verified", tone: "neutral" };
  const parts = e.host.ports.map((p) => (p.reach === "none" ? `no listener found for ${p.port}` : `${[...new Set(p.listeners.map((l) => l.address))].join(", ")} · ${p.reach === "loopback" ? "loopback only" : p.reach === "network" ? "reachable from the network" : "partly reachable from the network"}`));
  return { docker, host: `Mac listener: ${parts.join("; ")}`, tone: e.host.ports.every((p) => p.reach === "loopback") ? "ok" : "warn" };
}

/** One line per fact, worded so that a Docker report is never taken for host exposure. */
export function describeExposure(e: Exposure): { docker: string; host: string[]; hostTone: "ok" | "warn" | "neutral" } {
  const docker = e.docker.length ? `Docker reports ${e.docker.map((m) => `${m.hostIp}:${m.hostPort}→${m.containerPort}/${m.proto}`).join(", ")} (a mapping inside Harpoon's VM)` : "Docker reports no published ports";
  if (e.host.state === "unverified") return { docker, host: [`Host listener not verified${e.host.error ? `: ${e.host.error}` : ""}`], hostTone: "neutral" };
  if (!e.host.ports.length) return { docker, host: [], hostTone: "neutral" };
  let tone: "ok" | "warn" = "ok";
  const host = e.host.ports.map((p) => {
    const who = p.listeners.map((l) => `${l.command} (pid ${l.pid})`).join(", ");
    const where = p.listeners.map((l) => l.address).join(", ");
    if (p.reach === "none") { tone = "warn"; return `No listener found on this Mac for port ${p.port}`; }
    if (p.reach === "loopback") return `Host listener ${where} · loopback only, reachable from this Mac only · ${who}`;
    tone = "warn";
    return `Host listener ${where} · ${p.reach === "network" ? "reachable from other machines on the network" : "some addresses are reachable from the network"} · ${who}`;
  });
  return { docker, host, hostTone: tone };
}
