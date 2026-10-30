// Development-only deep links for reviewing simulated states (screenshots, regression checks).
// `#deploy=scenario:cutover-restored,screen:app,tab:logs,runtime:stopped,keychain:locked,dialog:resume`
// Values are whitelisted; anything unknown is ignored. Never used in production builds.
import type { KeychainState, NextOutcome, RuntimeState, ScenarioId } from "./deployTypes.ts";

export const SCENARIOS: ScenarioId[] = ["healthy", "building", "build-failed", "cutover-restored", "restore-failed", "recovering", "rolled-back-paused", "empty", "loading"];
const RUNTIMES: RuntimeState[] = ["connected", "stopped", "docker-not-ready"];
const KEYCHAINS: KeychainState[] = ["unlocked", "locked"];
const OUTCOMES: NextOutcome[] = ["success", "build-failure", "cutover-restored", "restore-failed"];
const SCREENS = ["home", "app", "dep"] as const;
const TABS = ["overview", "deployments", "logs", "metrics", "variables", "settings"] as const;
const ENVS = ["development", "preview", "production"] as const;
const DIALOGS = ["create", "deploy", "redeploy", "promote", "rollback", "resume", "recover", "variable"] as const;

export type DeployPreview = {
  scenario?: ScenarioId; runtime?: RuntimeState; keychain?: KeychainState; outcome?: NextOutcome;
  screen?: (typeof SCREENS)[number]; tab?: (typeof TABS)[number]; env?: (typeof ENVS)[number]; app?: string; dep?: string;
  dialog?: (typeof DIALOGS)[number]; width?: number;
};

const pick = <T extends string>(list: readonly T[], v: string | undefined): T | undefined => (v && (list as readonly string[]).includes(v) ? (v as T) : undefined);

export function parseDeployHash(hash: string): DeployPreview {
  const m = /deploy=([^&]*)/.exec(hash);
  if (!m) return {};
  const kv: Record<string, string> = {};
  for (const part of decodeURIComponent(m[1]).split(",")) {
    const i = part.indexOf(":");
    if (i > 0) kv[part.slice(0, i).trim()] = part.slice(i + 1).trim();
  }
  const w = Number(kv.width);
  return {
    scenario: pick(SCENARIOS, kv.scenario), runtime: pick(RUNTIMES, kv.runtime), keychain: pick(KEYCHAINS, kv.keychain), outcome: pick(OUTCOMES, kv.outcome),
    screen: pick(SCREENS, kv.screen), tab: pick(TABS, kv.tab), env: pick(ENVS, kv.env), dialog: pick(DIALOGS, kv.dialog),
    app: /^[a-z0-9-]{1,32}$/.test(kv.app ?? "") ? kv.app : undefined, dep: /^d-\d{1,6}$/.test(kv.dep ?? "") ? kv.dep : undefined,
    width: Number.isFinite(w) && w >= 480 && w <= 2400 ? w : undefined,
  };
}
