export type ContainerTopology = {
  status: "ready" | "stale";
  selected: { name: string; status: string; image: string | null } | null;
  stack: { project: string; service: string | null } | null;
  networks: Array<{ name: string; endpointId: string | null; ipAddress: string | null; gateway: string | null; aliases: string[] }>;
  links: Array<{ source: string; target: string | null }>;
  peerResolution: "unresolved" | "resolved";
};

export function mapContainerTopology(inspect: unknown, expectedId?: string): ContainerTopology;
