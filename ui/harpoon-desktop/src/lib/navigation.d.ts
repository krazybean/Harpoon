export type ResourceContext = "harpoon";
export type ContainerEngine = "docker";
export type Nav = "overview" | "containers" | "images" | "volumes" | "networks" | "resources" | "diagnostics";
export type ContainerDestination = { kind: "container"; engine: ContainerEngine; context: ResourceContext; containerId: string };
export type Destination = { kind: "nav"; nav: Nav } | ContainerDestination;
export function containerDestination(context: ResourceContext, containerId: string): ContainerDestination;
