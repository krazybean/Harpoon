const value = (entry) => typeof entry === "string" && entry.trim() ? entry.trim() : null;

function labelsOf(inspect) {
  const labels = inspect?.Config?.Labels ?? inspect?.Labels;
  if (!labels || typeof labels !== "object" || Array.isArray(labels)) return {};
  return labels;
}

export function mapContainerTopology(inspect, expectedId) {
  const actualId = value(inspect?.Id);
  if (!inspect || (expectedId && (!actualId || !(actualId === expectedId || actualId.startsWith(expectedId))))) {
    return { status: "stale", selected: null, stack: null, networks: [], links: [], peerResolution: "unresolved" };
  }

  const labels = labelsOf(inspect);
  const project = value(labels["com.docker.compose.project"]);
  const service = value(labels["com.docker.compose.service"]);
  const networks = inspect?.NetworkSettings?.Networks;
  const attachments = networks && typeof networks === "object" && !Array.isArray(networks)
    ? Object.entries(networks).map(([name, network]) => ({
      name,
      endpointId: value(network?.EndpointID),
      ipAddress: value(network?.IPAddress),
      gateway: value(network?.Gateway),
      aliases: Array.isArray(network?.Aliases) ? network.Aliases.filter(value) : [],
    }))
    : [];
  const links = Array.isArray(inspect?.HostConfig?.Links)
    ? inspect.HostConfig.Links.filter(value).map((link) => {
      const [source, target] = link.split(":");
      return { source: source.replace(/^\//, ""), target: value(target) };
    })
    : [];

  return {
    status: "ready",
    selected: {
      name: value(inspect?.Name)?.replace(/^\//, "") ?? "Container",
      status: value(inspect?.State?.Status) ?? (inspect?.State?.Running ? "running" : "unknown"),
      image: value(inspect?.Config?.Image),
    },
    stack: project ? { project, service } : null,
    networks: attachments,
    links,
    peerResolution: "unresolved",
  };
}
