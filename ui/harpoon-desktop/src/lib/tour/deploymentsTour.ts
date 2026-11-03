// The Deployments walkthrough: six short steps, each tied to a stable `data-tour` anchor. The tour never navigates
// and never invents data: a step whose anchor is not on screen right now is shown as a plain, honest explanation.
export type Anchor = "create-project" | "app-list" | "app-status" | "tab-deployments" | "tab-variables" | "recovery-actions" | "replay-tour";

export const anchorSelector = (a: Anchor) => `[data-tour="${a}"]`;

type StepDef = {
  id: string;
  /** The first of these that is on screen is highlighted. */
  anchors: Anchor[];
  title: string;
  /** Said when an anchor is highlighted. */
  here: string;
  /** Said, without a highlight, when none is on screen. */
  elsewhere: string;
};

export const STEPS: readonly StepDef[] = [
  { id: "register", anchors: ["create-project"], title: "Register an application",
    here: "Create a project from a Git repository, a local folder with a Dockerfile, or an image. Registering only records it; nothing is built or run until you deploy.",
    elsewhere: "Projects are created from the Projects list: from a Git repository, a local folder with a Dockerfile, or an image. Registering only records it; nothing is built or run until you deploy." },
  { id: "status", anchors: ["app-status", "app-list"], title: "See what is running",
    here: "Status shows what Harpoon recorded and what Docker reports now. Open an application for its current release, URL and health.",
    elsewhere: "Once a project exists, each application shows its status here: what Harpoon recorded and what Docker reports now. Nothing is shown until there is something real to show." },
  { id: "history", anchors: ["tab-deployments"], title: "Release history",
    here: "Every deployment is listed with its steps, logs and outcome, so you can see exactly what happened and when.",
    elsewhere: "Open an application and choose its Deployments tab to see every deployment with its steps, logs and outcome." },
  { id: "configuration", anchors: ["tab-variables"], title: "Configuration",
    here: "Variables and secrets are set per application. Secret values are never shown again once saved.",
    elsewhere: "Open an application and choose its Variables tab to set variables and secrets. Secret values are never shown again once saved." },
  { id: "recovery", anchors: ["recovery-actions"], title: "Recovery controls",
    here: "Deploy, redeploy and roll back live here. A rollback asks for confirmation first and does not touch your data or volumes. This tour will not press any of them.",
    elsewhere: "On an application's page, Deploy, Redeploy and Roll back sit under its status. A rollback asks for confirmation first and does not touch your data or volumes." },
  { id: "replay", anchors: ["replay-tour"], title: "Come back any time",
    here: "Replay this walkthrough from here whenever you like.",
    elsewhere: "Replay this walkthrough from the Deployments page whenever you like." },
];

export type ResolvedStep = { id: string; anchor: Anchor | null; title: string; text: string };

/** Which anchors exist right now decides, per step, what is highlighted and which wording is honest. */
export function resolveSteps(has: (a: Anchor) => boolean): ResolvedStep[] {
  return STEPS.map((s) => {
    const anchor = s.anchors.find(has) ?? null;
    return { id: s.id, anchor, title: s.title, text: anchor ? s.here : s.elsewhere };
  });
}
