import type { DeployAction, DeployState, Env, RuntimeState } from "../../lib/deployTypes.ts";

export type Tab = "overview" | "deployments" | "logs" | "metrics" | "variables" | "settings";
export type Route = { screen: "home" | "app" | "dep"; appId: string; env: Env; tab: Tab; depId: string | null };

export type DialogState =
  | { kind: "create"; step: 1 | 2 | 3; src: "git" | "folder" | "image" }
  | { kind: "deploy" | "redeploy" | "promote" }
  | { kind: "retry"; depId: string }
  | { kind: "rollback"; releaseId: string }
  | { kind: "resume" }
  | { kind: "recover" }
  | { kind: "variable"; name?: string };

export type Ctx = {
  state: DeployState;
  /** The runtime the views must believe: Harpoon's real status unless a simulation override is set. */
  runtime: RuntimeState;
  runtimeSource: "harpoon" | "simulated";
  route: Route;
  setRoute: (patch: Partial<Route>) => void;
  /** `run` = let the simulated deployment advance by itself. */
  dispatch: (a: DeployAction, run?: boolean) => void;
  open: (d: DialogState | null) => void;
  startRuntime: () => void;
};
