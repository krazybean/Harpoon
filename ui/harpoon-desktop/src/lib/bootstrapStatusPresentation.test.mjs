import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("../styles/primitives.css", import.meta.url), "utf8");

test("startup status is the only live bootstrap message and replays on replacement", () => {
  assert.match(app, /<BootstrapStatus message=\{bootstrapStatus\} trace=\{bootstrapTrace\} \/>/);
  assert.match(app, /key=\{`current-\$\{current\}`\}/);
  assert.match(app, /if \(timer\.current !== null\) window\.clearTimeout\(timer\.current\);/);
  assert.doesNotMatch(app, /phaseDetail\(/);
  assert.doesNotMatch(app, /Docker ready.*binary resolved/);
});

test("startup status exit is perceptible and remains accessible to reduced motion", () => {
  assert.match(styles, /bootstrap-status-out 360ms/);
  assert.match(styles, /translateY\(20px\)/);
  assert.match(styles, /prefers-reduced-motion: reduce/);
  assert.match(styles, /bootstrap-status__message--leaving\s*\{ animation: none; \}/);
});

test("authoritative runtime failures stay structured and do not auto-restart", () => {
  assert.match(app, /failure\?: RuntimeFailure/);
  assert.match(app, /status\?\.failure\?\.category === "vm_terminated"/);
  assert.match(app, /The runtime is stopped; no automatic restart was attempted/);
  assert.match(app, /View underlying diagnostics/);
  assert.match(app, /if \(s0\.state === "failed"\)/);
});
