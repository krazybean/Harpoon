import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { QUAKE_TARGET_FIXTURES, isQuakeTargetSafe, quakeFixtureOutput, resolveQuakeTarget } from "./quakeShellPrototype.ts";

test("fixture targets carry explicit identity and stale targets are rejected", () => {
  const [api, worker, stale] = QUAKE_TARGET_FIXTURES;
  assert.equal(isQuakeTargetSafe(api), true);
  assert.equal(isQuakeTargetSafe(worker), true);
  assert.equal(isQuakeTargetSafe(stale), false);
  assert.match(quakeFixtureOutput(api), /api/);
});

test("missing target cannot become an executable target", () => {
  assert.equal(isQuakeTargetSafe(null), false);
});

test("production mounts Quake and does not invent a fixture session", () => {
  const appSource = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const componentSource = readFileSync(new URL("../components/QuakeShellPrototype.tsx", import.meta.url), "utf8");
  assert.match(appSource, /import \{ QuakeShellPrototype \} from "\.\/components\/QuakeShellPrototype"/);
  assert.match(componentSource, /lazy\(\(\) => import\("\.\.\/views\/ShellTab"\)/);
  assert.match(componentSource, /<Suspense fallback=\{<div className="quake-prototype__terminal-loading"/);
  assert.doesNotMatch(componentSource, /QUAKE_TARGET_FIXTURES/);
  assert.match(componentSource, /event\.code === "Backquote"/);
});

test("Quake keeps configuration, session actions, and an accessible bounded resizer distinct", () => {
  const componentSource = readFileSync(new URL("../components/QuakeShellPrototype.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../styles/primitives.css", import.meta.url), "utf8");
  assert.match(componentSource, /aria-label="Shell target configuration"/);
  assert.match(componentSource, /id="quake-height" type="range" min="320" max="600"/);
  assert.match(styles, /height: min\(var\(--quake-height\), calc\(100vh - 12px\)\)/);
  assert.match(styles, /grid-template-rows: auto auto auto minmax\(0, 1fr\) auto auto/);
  assert.match(styles, /@media \(max-width: 760px\)/);
});

test("Quake keeps the launcher discoverable and identifies the selected technology", () => {
  const componentSource = readFileSync(new URL("../components/QuakeShellPrototype.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../styles/primitives.css", import.meta.url), "utf8");
  assert.match(componentSource, /inferContainerTechnology\(container\)/);
  assert.match(componentSource, /role="img" aria-label=\{`\$\{target\.technology\.label\} technology`\}/);
  assert.match(componentSource, /\{candidate\.technology\.label\}/);
  assert.doesNotMatch(componentSource, /Connected status and terminal actions are below/);
  assert.match(styles, /opacity: \.48/);
  assert.match(styles, /\.quake-prototype-trigger:hover, \.quake-prototype-trigger:focus-visible/);
  assert.match(styles, /outline: 2px solid var\(--accent-yellow\)/);
  assert.match(styles, /prefers-reduced-motion: reduce.*quake-prototype-trigger/);
});

test("Quake dismisses only on pointerdown outside the panel and launcher", () => {
  const componentSource = readFileSync(new URL("../components/QuakeShellPrototype.tsx", import.meta.url), "utf8");
  assert.match(componentSource, /event\.composedPath\(\)/);
  assert.match(componentSource, /panel && path\.includes\(panel\)/);
  assert.match(componentSource, /trigger && path\.includes\(trigger\)/);
  assert.match(componentSource, /document\.addEventListener\("pointerdown", onPointerDown\)/);
  assert.match(componentSource, /setOpen\(false\);\s*triggerRef\.current\?\.focus\(\);/);
});

test("the selected target follows its container ID across refreshes", () => {
  const [api, worker] = QUAKE_TARGET_FIXTURES;
  assert.equal(resolveQuakeTarget([api, worker], null).target, api);
  assert.equal(resolveQuakeTarget([worker, api], worker.containerId).target, worker);
  // api stopped: input must not fall through to whichever container is now first
  assert.deepEqual(resolveQuakeTarget([worker], api.containerId), { target: null, gone: true });
});
