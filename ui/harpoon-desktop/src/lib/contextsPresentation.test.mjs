import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../views/ContextsView.tsx", import.meta.url), "utf8");

test("contexts view presents honest discovery states and a visible read-only boundary", () => {
  for (const state of ["connected", "authentication_needed", "unavailable", "detected"]) assert.match(source, new RegExp(state));
  assert.match(source, /Discovery only/);
  assert.match(source, /does not switch contexts/);
  assert.match(source, /unsupported in discovery/);
  assert.match(source, /context-card__name-text/);
  assert.match(source, /title=\{context\.name\}/);
  assert.match(source, /tabIndex=\{0\}/);
});

test("contexts view refreshes only on entry when the cache is absent or stale", () => {
  assert.match(source, /if \(hasCache && !stale\) return;/);
  assert.match(source, /\}, \[refresh\]\);/);
  assert.doesNotMatch(source, /setInterval/);
});
