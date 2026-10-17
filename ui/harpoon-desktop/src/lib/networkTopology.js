import { layoutTopology } from "./topologyPrototypeLayout.ts";

const text = (value) => typeof value === "string" && value.trim() ? value.trim() : null;

export function mapNetworkTopology(networks, containers) {
  const knownNetworks = new Map((Array.isArray(networks) ? networks : []).map((network) => {
    const name = text(network?.Name) ?? text(network?.name);
    return name ? [name, { id: `network-${name}`, dockerId: text(network?.ID), name, missing: false }] : null;
  }).filter(Boolean));
  const attached = [];
  const seenContainers = new Set();
  const ensureNetwork = (name) => {
    if (!knownNetworks.has(name)) knownNetworks.set(name, { id: `network-${name}`, dockerId: null, name, missing: true });
    return knownNetworks.get(name);
  };

  (Array.isArray(containers) ? containers : []).forEach((container, index) => {
    const names = Array.isArray(container?.NetworkNames) ? container.NetworkNames.filter(text) : [];
    if (!names.length) return;
    const id = text(container?.ID) ?? text(container?.Id) ?? `container-${index + 1}`;
    if (seenContainers.has(id)) return;
    seenContainers.add(id);
    const name = (text(container?.Names) ?? text(container?.Name) ?? id).replace(/^\//, "");
    const composeService = text(container?.ComposeService);
    const composeNumber = text(container?.ComposeNumber);
    const label = composeService
      ? `${composeService}${composeNumber && composeNumber !== "1" ? ` #${composeNumber}` : ""}`
      : name;
    const status = text(container?.State) ?? (text(container?.Status)?.toLowerCase().startsWith("up") ? "running" : text(container?.Status)) ?? "unknown";
    const attachments = Array.isArray(container?.NetworkAttachments) ? container.NetworkAttachments : [];
    attached.push({ id: `container-${id}`, name, label, fullName: name, status: status.toLowerCase(), image: text(container?.Image), networks: [...new Set(names)], attachments });
    names.forEach(ensureNetwork);
  });

  return { containers: attached, networks: [...knownNetworks.values()], layout: layoutTopology({ containers: attached.map((container) => ({ ...container, service: null, technology: "unknown", links: [] })), networks: [...knownNetworks.values()] }) };
}
