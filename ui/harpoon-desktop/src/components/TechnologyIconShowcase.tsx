import { ContainerTechnologyIcon } from "./ContainerTechnologyIcon";
import { inferContainerTechnology } from "../lib/containerTechnology.js";
import { TECHNOLOGY_ICONS } from "../lib/technologyIcons.js";

const CASES = [
  ["postgres:17-alpine", { Image: "postgres:17-alpine" }],
  ["redis:7-alpine", { Image: "redis:7-alpine" }],
  ["nginx:alpine", { Image: "nginx:alpine" }],
  ["node:22-alpine", { Image: "node:22-alpine" }],
  ["mysql:9", { Image: "mysql:9" }],
  ["mariadb:11", { Image: "mariadb:11" }],
  ["mongo:8", { Image: "mongo:8" }],
  ["rabbitmq:4", { Image: "rabbitmq:4" }],
  ["traefik:v3", { Image: "traefik:v3" }],
  ["caddy:2", { Image: "caddy:2" }],
  ["python:3.13-alpine", { Image: "python:3.13-alpine" }],
  ["alpine:3.22", { Image: "alpine:3.22" }],
  ["registry/team/postgres:17-alpine", { Image: "registry.example/team/postgres:17-alpine" }],
  ["ambiguous service", { Image: "example/service:latest", Names: "postgres" }],
  ["unknown", { Image: "example/worker:latest" }],
] as const;

export function TechnologyIconShowcase() {
  return <main className="technology-showcase" aria-labelledby="technology-showcase-title">
    <header className="technology-showcase__header">
      <div>
        <span className="overview-eyebrow">Developer fixture · offline</span>
        <h1 id="technology-showcase-title">Technology marks</h1>
        <p>Synthetic image metadata only. Curated local Simple Icons marks; no Docker pulls or runtime network fetches.</p>
      </div>
      <span className="badge badge--info">DEV ONLY</span>
    </header>
    <div className="technology-showcase__grid">
      {CASES.map(([name, metadata]) => {
        const technology = inferContainerTechnology(metadata);
        return <article className="technology-showcase__card" key={name}>
          <div className="technology-showcase__mark"><ContainerTechnologyIcon technology={technology} size={34} /></div>
          <div className="technology-showcase__copy"><strong>{technology.label}</strong><code title={metadata.Image}>{name}</code></div>
          <span className="technology-showcase__key">{technology.key}<br />{TECHNOLOGY_ICONS[technology.key]?.color ?? "generic"}</span>
        </article>;
      })}
    </div>
  </main>;
}
