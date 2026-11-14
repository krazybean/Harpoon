// How a repository without a Dockerfile is built and run. Detection happens in the engine (read-only); the choices
// made here are stored with the registration as one JSON object and sent as `--settings`.

export type SourceSettings = { install: string; build: string; start: string; style: "production" | "development"; kind: "service" | "job"; network: string };

export const NO_SETTINGS: SourceSettings = { install: "", build: "", start: "", style: "production", kind: "service", network: "" };

/** What the engine detected for a folder. `hasDockerfile` means Harpoon builds that file and detects nothing. */
export type SourceDetection = {
  hasDockerfile: boolean;
  framework?: string; baseImage?: string; install?: string; build?: string | null; start?: string | null; devStart?: string | null;
  suggestedPort?: number; notes?: string[];
};

/** The stored text (or nothing) as form values; an unreadable value is treated as no settings, never guessed at. */
export function parseSettings(text: string | null | undefined): SourceSettings {
  if (!text) return NO_SETTINGS;
  try {
    const o = JSON.parse(text) as Record<string, unknown>;
    const str = (k: string) => (typeof o[k] === "string" ? (o[k] as string) : "");
    return {
      install: str("install"), build: str("build"), start: str("start"), network: str("network"),
      style: o.style === "development" ? "development" : "production", kind: o.kind === "job" ? "job" : "service",
    };
  } catch { return NO_SETTINGS; }
}

const trimmed = (s: SourceSettings): SourceSettings => ({ ...s, install: s.install.trim(), build: s.build.trim(), start: s.start.trim(), network: s.network.trim() });

/** Only what differs from the defaults is sent; "{}" clears every setting. */
export function settingsJson(s: SourceSettings): string {
  const t = trimmed(s);
  const o: Record<string, string> = {};
  if (t.install) o.install = t.install;
  if (t.build) o.build = t.build;
  if (t.start) o.start = t.start;
  if (t.style !== "production") o.style = t.style;
  if (t.kind !== "service") o.kind = t.kind;
  if (t.network) o.network = t.network;
  return JSON.stringify(o);
}

export const sameSettings = (a: SourceSettings, b: SourceSettings): boolean => settingsJson(a) === settingsJson(b);
export const hasSettings = (s: SourceSettings): boolean => settingsJson(s) !== "{}";

/** A problem the form can show before the engine is asked (the engine checks again). */
export function settingsProblem(s: SourceSettings): string | null {
  const t = trimmed(s);
  for (const [label, v] of [["Install", t.install], ["Build", t.build], ["Start", t.start]] as const) {
    if (/[\u0000-\u001f\u007f]/.test(v)) return `${label} command must be one line.`;
    if (v.length > 1024) return `${label} command is longer than 1024 characters.`;
  }
  if (t.network && (!/^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$/.test(t.network) || ["host", "none", "bridge"].includes(t.network))) return "Network must be a user-defined Docker network name (not host, none or bridge).";
  return null;
}

/** One line per detected fact, for the registration form. */
export function detectionLines(d: SourceDetection): string[] {
  if (d.hasDockerfile) return ["This folder has a Dockerfile. Harpoon builds it as it is."];
  const out = [`Detected ${d.framework} on ${d.baseImage}.`];
  out.push(`Install: ${d.install}${d.build ? ` · Build: ${d.build}` : ""} · Start: ${d.start ?? "not found, enter one"}`);
  for (const n of d.notes ?? []) out.push(n);
  return out;
}

export function parseDetection(text: string): SourceDetection {
  return JSON.parse(text) as SourceDetection;
}
