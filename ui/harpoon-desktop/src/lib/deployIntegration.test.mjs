import assert from "node:assert/strict";
import { dialogAllowed } from "../views/deploy/context.ts";
import { readFileSync, readdirSync } from "node:fs";
import test from "node:test";
import { parseDeployHash, SCENARIOS } from "./deployPreview.ts";

const read = (rel) => readFileSync(new URL(rel, import.meta.url), "utf8");
const deployFiles = () => [
  ...readdirSync(new URL("../views/deploy/", import.meta.url)).filter((f) => /\.(tsx?|css)$/.test(f)).map((f) => `../views/deploy/${f}`),
  "./deployTypes.ts", "./deployMock.ts", "./deployStore.ts", "./deployPresentation.ts", "./deployPreview.ts", "../hooks/useDeploySimulation.ts",
];

test("the Deployments layer is mock-only: no Tauri, no Docker, no network, no storage, no processes", () => {
  for (const f of deployFiles()) {
    const src = read(f);
    assert.doesNotMatch(src, /@tauri-apps|invoke\s*\(|listen\s*\(/, `${f} must not call Tauri`);
    assert.doesNotMatch(src, /\bfetch\s*\(|XMLHttpRequest|WebSocket|EventSource/, `${f} must not use the network`);
    assert.doesNotMatch(src, /localStorage|sessionStorage|indexedDB|document\.cookie/, `${f} must not persist anything`);
    assert.doesNotMatch(src, /child_process|Command\s*\(|docker\s+(run|build|rm)/i, `${f} must not run processes`);
    assert.doesNotMatch(src, /console\.(log|info|debug|warn)\s*\(/, `${f} must not log (secret hygiene)`);
  }
});

test("the view says it is simulated and offers an explicit simulation boundary", () => {
  const view = read("../views/deploy/DeployView.tsx");
  assert.match(view, /SIMULATION_NOTE/);
  assert.match(view, /aria-label="Simulation controls — not part of the product"/);
  assert.match(read("./deployPresentation.ts"), /No deployment engine is connected/);
  const screens = read("../views/deploy/Screens.tsx");
  assert.match(screens, /<Tag>simulated<\/Tag>/);
  assert.match(screens, /does not exist in your real Containers view/);
});

test("simulated containers are never links into the real Containers view", () => {
  const screens = read("../views/deploy/Screens.tsx");
  assert.doesNotMatch(screens, /openContainer|containerDestination|handleNav\(["']containers/);
});

test("secret inputs are masked, never autocompleted, and cleared after saving", () => {
  const d = read("../views/deploy/Dialogs.tsx");
  assert.match(d, /type=\{type === "secret" \? "password" : "text"\}/);
  assert.match(d, /autoComplete="off"/);
  assert.match(d, /setValue\(""\); close\(\)/);
  assert.match(d, /value: type === "config" \? value : undefined/, "a secret value must never be dispatched");
});

test("the app wires Deployments as one additional, lazy-loaded navigation item and leaves the others alone", () => {
  const app = read("../App.tsx");
  assert.match(app, /\["deployments", RocketLaunch, "Deployments"\]/);
  assert.match(app, /lazy\(\(\) => import\("\.\/views\/deploy\/DeployView"\)/);
  for (const nav of ["overview", "containers", "images", "volumes", "networks", "resources", "diagnostics", "contexts"]) {
    assert.match(app, new RegExp(`\\["${nav}", \\w+, "[A-Za-z]+"\\]`), `${nav} nav item must remain`);
    assert.match(app, new RegExp(`active==="${nav}"`), `${nav} view must remain mounted`);
  }
  assert.match(read("./navigation.d.ts"), /"containers" \| "deployments" \| "images"/);
});

test("the Deployments runtime comes from Harpoon's real status unless a simulation override is set", () => {
  const app = read("../App.tsx");
  assert.match(app, /realRuntime: RuntimeState = status\?\.state === "running" \? \(status\.dockerReady \? "connected" : "docker-not-ready"\)/);
  assert.match(read("../views/deploy/DeployView.tsx"), /runtimeOverride \?\? realRuntime/);
});

test("the production build never reads a deep link (preview hash is development-only)", () => {
  assert.match(read("../App.tsx"), /import\.meta\.env\.DEV && typeof window !== "undefined" \? parseDeployHash/);
});

test("deep links are whitelisted and reject anything else", () => {
  assert.deepEqual(parseDeployHash(""), {});
  const p = parseDeployHash("#deploy=scenario:cutover-restored,screen:app,tab:logs,runtime:stopped,keychain:locked,dialog:resume,dep:d-43,width:900,app:api");
  assert.equal(p.scenario, "cutover-restored");
  assert.equal(p.runtime, "stopped");
  assert.equal(p.dialog, "resume");
  assert.equal(p.width, 900);
  const bad = parseDeployHash("#deploy=scenario:rm -rf,screen:../../etc,runtime:<script>,app:../x,dep:d-9;drop,width:99999");
  assert.deepEqual(Object.values(bad).filter((v) => v !== undefined), []);
  for (const s of SCENARIOS) assert.equal(parseDeployHash(`#deploy=scenario:${s}`).scenario, s);
});

test("existing screens keep their own files and are not edited by this feature", () => {
  for (const f of ["ContainersView", "ImagesView", "VolumesView", "NetworksView", "ResourcesView", "DiagnosticsView", "OverviewView", "ContextsView"]) {
    assert.doesNotMatch(read(`../views/${f}.tsx`), /deploy/i, `${f} must not reference the deployment feature`);
  }
});

test("Development route disables every Production-changing action and Keychain help is wired", () => {
  const screens = readFileSync(new URL("../views/deploy/Screens.tsx", import.meta.url), "utf8");
  assert.match(screens, /const prod = env === "production"/);
  assert.equal((screens.match(/!prod/g) ?? []).length >= 4, true);
  assert.doesNotMatch(screens, /btn\("btn--primary", \(\) => \{\}/);
  assert.match(screens, /unlock-help/);
});

test("non-Production routes can never open a Production-changing dialog", () => {
  const mutating = [{ kind: "deploy" }, { kind: "redeploy" }, { kind: "promote" }, { kind: "retry", depId: "d-1" }, { kind: "rollback", releaseId: "r-1" }, { kind: "recover" }, { kind: "resume" }];
  for (const env of ["development", "preview"]) for (const d of mutating) assert.equal(dialogAllowed(d, env), false, `${env}:${d.kind}`);
  for (const d of mutating) assert.equal(dialogAllowed(d, "production"), true);
  for (const env of ["development", "preview", "production"]) for (const d of [{ kind: "variable" }, { kind: "unlock-help" }, { kind: "create", step: 1, src: "git" }]) assert.equal(dialogAllowed(d, env), true);
  const view = read("../views/deploy/DeployView.tsx");
  assert.match(view, /dialogAllowed\(dialog, route\.env\)/);
  assert.match(view, /open: guardedOpen/);
});
