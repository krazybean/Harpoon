import type { ContainerTechnology } from "./containerTechnology.js";

export type QuakeTargetState = "connected" | "ready" | "stale";

export type QuakeTarget = {
  engine: "docker";
  context: string;
  containerId: string;
  name: string;
  executable: string;
  user: string;
  state: QuakeTargetState;
  technology: ContainerTechnology;
};

export const QUAKE_TARGET_FIXTURES: QuakeTarget[] = [
  {
    engine: "docker",
    context: "harpoon",
    containerId: "8f5f3e5c0c9d4a1b8d8f5f3e5c0c9d4a1b8d8f5f3e5c0c9d4a1b8d8f5f3e5c0c",
    name: "api",
    executable: "/bin/sh",
    user: "root",
    state: "ready",
    technology: { key: "unknown", label: "Unknown image" },
  },
  {
    engine: "docker",
    context: "harpoon",
    containerId: "2b3a4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f70819",
    name: "worker",
    executable: "/bin/sh",
    user: "node",
    state: "ready",
    technology: { key: "unknown", label: "Unknown image" },
  },
  {
    engine: "docker",
    context: "harpoon",
    containerId: "stale-target-000000000000000000000000000000000000000000000000000000000000",
    name: "previous-api",
    executable: "/bin/sh",
    user: "root",
    state: "stale",
    technology: { key: "unknown", label: "Unknown image" },
  },
];

export function isQuakeTargetSafe(target: QuakeTarget | null) {
  return Boolean(target && target.state !== "stale" && target.containerId.length === 64 && target.context === "harpoon");
}

export function quakeFixtureOutput(target: QuakeTarget) {
  return `$ target ${target.name}\n${target.user}@${target.name}:~$ printf 'native PTY pending\\n'\nnative PTY pending\n`;
}
