export type PrototypeContainer = {
  id: string;
  service: string | null;
  fullName: string;
  status: "running" | "stopped" | "unknown";
  image: string | null;
  technology: string;
  networks: string[];
  links: string[];
};

export type PrototypeNetwork = { id: string; name: string };
export type PrototypeTopology = { containers: PrototypeContainer[]; networks: PrototypeNetwork[] };

const services = ["gateway", "api", "worker", "scheduler", "postgres", "redis", "frontend", "proxy", "queue", "metrics"];
const images = ["nginx:alpine", "node:22-alpine", "python:3.13-alpine", "postgres:17-alpine", "redis:7-alpine", "rabbitmq:4-management"];
const networkNames = ["frontend", "backend", "observability"];

export function makeTopology(count: number, state: "populated" | "empty" | "unknown" = "populated"): PrototypeTopology {
  if (state === "empty") return { containers: [], networks: [] };
  const containers = Array.from({ length: count }, (_, index) => {
    const service = services[index % services.length];
    return {
      id: `container-${index + 1}`,
      service: state === "unknown" && index === 0 ? null : service,
      fullName: index === 0 ? "harpoon-ui-stack-test-2026-worker-1-with-a-pathologically-long-compose-generated-name" : `harpoon-ui-stack-test-2026-${service}-${index + 1}`,
      status: state === "unknown" && index === 0 ? "unknown" : index % 7 === 0 ? "stopped" : "running",
      image: state === "unknown" && index === 0 ? null : images[index % images.length],
      technology: state === "unknown" && index === 0 ? "Unknown image" : images[index % images.length].split(":")[0],
      networks: index % 3 === 0 ? ["frontend", "backend"] : index % 4 === 0 ? ["backend", "observability"] : [networkNames[index % networkNames.length]],
      links: index === 1 ? ["/db:/api/db"] : [],
    } satisfies PrototypeContainer;
  });
  return { containers, networks: networkNames.map((name) => ({ id: `network-${name}`, name })) };
}
