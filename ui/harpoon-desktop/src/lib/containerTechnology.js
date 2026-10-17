const TECHNOLOGIES = [
  ["postgres", "PostgreSQL", new Set(["postgres", "postgresql"])],
  ["redis", "Redis", new Set(["redis"])],
  ["nginx", "Nginx", new Set(["nginx"])],
  ["node", "Node.js", new Set(["node"])],
  ["mysql", "MySQL", new Set(["mysql"])],
  ["mariadb", "MariaDB", new Set(["mariadb"])],
  ["mongo", "MongoDB", new Set(["mongo", "mongodb"])],
  ["rabbitmq", "RabbitMQ", new Set(["rabbitmq"])],
  ["traefik", "Traefik", new Set(["traefik"])],
  ["caddy", "Caddy", new Set(["caddy"])],
  ["python", "Python", new Set(["python"])],
  ["alpine", "Alpine Linux", new Set(["alpine"])],
];

const OCI_LABEL_KEYS = new Set([
  "org.opencontainers.image.ref.name",
  "org.opencontainers.image.title",
  "org.opencontainers.image.source",
]);

const unknown = { key: "unknown", label: "Unknown image" };

function imageCandidates(container) {
  const labels = container?.Labels;
  const labelValues = [];
  if (labels && typeof labels === "object" && !Array.isArray(labels)) {
    for (const [key, value] of Object.entries(labels)) {
      if (OCI_LABEL_KEYS.has(key) && typeof value === "string") labelValues.push(value);
    }
  } else if (typeof labels === "string") {
    for (const entry of labels.split(",")) {
      const [key, ...parts] = entry.split("=");
      if (OCI_LABEL_KEYS.has(key?.trim())) labelValues.push(parts.join("=").trim());
    }
  }
  return [
    container?.Image,
    container?.Config?.Image,
    ...(Array.isArray(container?.RepoTags) ? container.RepoTags : []),
    ...(Array.isArray(container?.RepoDigests) ? container.RepoDigests : []),
    ...(Array.isArray(container?.ImageName) ? container.ImageName : []),
    ...labelValues,
  ].filter((candidate) => typeof candidate === "string" && candidate.trim());
}

function normalizeReference(reference) {
  return reference.trim().toLowerCase().replace(/^\/+|\/+$/g, "").split("@", 1)[0].replace(/:[^/]*$/, "");
}

export function inferContainerTechnology(container) {
  for (const candidate of imageCandidates(container)) {
    const normalized = normalizeReference(candidate);
    const repository = normalized.split("/").pop();
    const technology = TECHNOLOGIES.find(([, , names]) => names.has(repository));
    if (technology) return { key: technology[0], label: technology[1] };
  }
  return unknown;
}
