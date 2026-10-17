import assert from "node:assert/strict";
import test from "node:test";
import { inferContainerTechnology } from "./containerTechnology.js";

test("image repository wins and handles tags, registries, and digests", () => {
  assert.equal(inferContainerTechnology({ Image: "registry.example/team/postgres:17-alpine", Labels: "org.opencontainers.image.title=not-node" }).key, "postgres");
  assert.equal(inferContainerTechnology({ Image: "redis@sha256:abc" }).key, "redis");
  assert.equal(inferContainerTechnology({ RepoDigests: ["docker.io/library/nginx@sha256:abc"] }).key, "nginx");
});

test("recognized OCI labels are a conservative fallback", () => {
  assert.equal(inferContainerTechnology({ Labels: { "org.opencontainers.image.ref.name": "python:3.13" } }).key, "python");
  assert.equal(inferContainerTechnology({ Labels: "com.docker.compose.service=postgres,org.opencontainers.image.title=alpine" }).key, "alpine");
});

test("ambiguous and display-name-only inputs stay generic", () => {
  assert.equal(inferContainerTechnology({ Names: "postgres", Image: "my-company/api:latest" }).key, "unknown");
  assert.equal(inferContainerTechnology({ Image: "node-exporter:latest" }).key, "unknown");
  assert.equal(inferContainerTechnology({ Image: "example/service:latest" }).key, "unknown");
});

test("representative card fixture covers every bundled technology mark", () => {
  const images = ["postgres:17", "redis:7", "nginx:1.27", "node:22", "mysql:9", "mariadb:11", "mongo:8", "rabbitmq:4", "traefik:v3", "caddy:2", "python:3.13", "alpine:3.22"];
  assert.deepEqual(images.map((Image) => inferContainerTechnology({ Image }).key), [
    "postgres", "redis", "nginx", "node", "mysql", "mariadb", "mongo", "rabbitmq", "traefik", "caddy", "python", "alpine",
  ]);
});
