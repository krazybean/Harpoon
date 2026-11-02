import type { DeployAction, DeployState, Env, RuntimeState } from "../../lib/deployTypes.ts";
import type { EngineApi } from "../../lib/deployEngine.ts";

export type Tab = "overview" | "deployments" | "logs" | "metrics" | "variables" | "settings";
export type Route = { screen: "home" | "app" | "dep"; appId: string; env: Env; tab: Tab; depId: string | null };

export type DialogState =
  | { kind: "create"; step: 1 | 2 | 3; src: "git" | "folder" | "image" }
  | { kind: "deploy" | "redeploy" | "promote" }
  | { kind: "retry"; depId: string }
  | { kind: "rollback"; releaseId: string }
  | { kind: "resume" }
  | { kind: "unlock-help" }
  | { kind: "recover" }
  | { kind: "variable"; name?: string }
  | { kind: "result" };

export type Ctx = {
  state: DeployState;
  /** The runtime the views must believe: Harpoon's real status unless a simulation override is set. */
  runtime: RuntimeState;
  runtimeSource: "harpoon" | "simulated";
  /** Present only when the data is the real engine's. Absent in the explicit simulation preview. */
  engine?: EngineApi;
  route: Route;
  setRoute: (patch: Partial<Route>) => void;
  /** `run` = let the simulated deployment advance by itself. */
  dispatch: (a: DeployAction, run?: boolean) => void;
  open: (d: DialogState | null) => void;
  startRuntime: () => void;
  /** Leave Deployments for the existing Containers view (real engine mode). */
  openContainers?: () => void;
};

/** Dialogs that start or change Production state; the simulation models Production only. */
const PRODUCTION_ONLY: DialogState["kind"][] = ["deploy", "redeploy", "promote", "retry", "rollback", "recover", "resume"];
export const dialogAllowed = (d: DialogState, env: Env): boolean => env === "production" || !PRODUCTION_ONLY.includes(d.kind);
