export function containerDestination(context, containerId) {
  return { kind: "container", engine: "docker", context, containerId };
}
