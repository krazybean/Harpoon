import type { AppModel } from "./deployTypes.ts";
import { latestDeployment } from "./deployStore.ts";

/** A preview is taken only of a running release whose final health check passed; anything else has nothing true to show. */
export function previewReady(engine: boolean, app: AppModel, running: boolean): boolean {
  const L = latestDeployment(app);
  return engine && running && app.address?.status === "running" && !!L && L.releaseId === app.currentReleaseId && L.steps.some((step) => step.name === "final_check" && step.status === "succeeded");
}
