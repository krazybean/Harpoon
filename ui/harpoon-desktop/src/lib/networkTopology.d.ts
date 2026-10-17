export type NetworkTopology = {
  containers: Array<{ id: string; name: string; fullName: string; status: string; image: string | null; networks: string[] }>;
  networks: Array<{ id: string; dockerId: string | null; name: string; missing: boolean }>;
  layout: { node(id: string): { x: number; y: number }; edges(): unknown[] };
};

export function mapNetworkTopology(networks: unknown[], containers: unknown[]): NetworkTopology;
