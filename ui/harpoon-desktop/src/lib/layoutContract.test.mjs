// A SOURCE-LEVEL contract for the overflow fixes. jsdom has no layout engine, so this only guards that the rules which fixed the
// problem stay in place; the layout itself was measured in a real browser (see docs/results/DEPLOY_MILESTONE_E_DESKTOP.md, section 13).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const css = (rel) => readFileSync(new URL(rel, import.meta.url), "utf8");
const deploy = css("../views/deploy/deploy.css");
const prim = css("../styles/primitives.css");
const rule = (text, selector) => {
  const i = text.lastIndexOf(selector + " {");
  assert.ok(i >= 0, `a rule for ${selector}`);
  return text.slice(i, text.indexOf("}", i));
};

test("Deployments: children of rows, cards, banners and panel heads may shrink, and long text breaks instead of pushing out", () => {
  assert.match(rule(deploy, ".dp-root"), /overflow-wrap: break-word/);
  assert.match(rule(deploy, ".dp-row > *"), /min-width: 0/);
  assert.match(rule(deploy, ".dp-card"), /min-width: 0/);
  assert.match(rule(deploy, ".dp-card > .dp-row"), /flex-wrap: wrap/);
  assert.match(rule(deploy, ".dp-card > .dp-row > b"), /text-overflow: ellipsis/);
  assert.match(rule(deploy, ".dp-banner > div:not(.dp-banner__actions)"), /min-width: 0/);
  assert.match(rule(deploy, ".dp-hero__state > div:not(.big)"), /min-width: 0/);
  assert.match(rule(deploy, ".dp-actions"), /flex-wrap: wrap/);
  assert.match(rule(deploy, ".dp-tablewrap"), /overflow-x: auto/, "a wide table scrolls instead of being clipped");
  assert.match(rule(deploy, ".dp-id__text"), /overflow-wrap: anywhere/);
});

test("badges keep their size and never shrink into neighbours", () => {
  assert.match(rule(deploy, ".dp-pill, .dp-tag"), /flex: 0 0 auto/);
  assert.match(rule(deploy, ".dp-pill, .dp-tag"), /white-space: nowrap/);
  assert.match(rule(prim, ".container-tile__state, .stack-card__summary .badge"), /flex: 0 0 auto/);
});

test("Containers: image hashes and port lines are truncated inside their card and span its width", () => {
  assert.match(rule(prim, ".container-tile > .text-meta, .stack-card__heading .text-meta"), /text-overflow: ellipsis/);
  assert.match(rule(prim, ".container-tile > .text-meta, .stack-card__heading .text-meta"), /min-width: 0/);
  assert.match(rule(prim, ".port-exposure > span"), /text-overflow: ellipsis/);
  assert.match(rule(prim, ".stack-services .stack-services__ports"), /grid-column: 1 \/ -1/);
});

test("what is truncated keeps its full value reachable", () => {
  const cv = readFileSync(new URL("../views/ContainersView.tsx", import.meta.url), "utf8");
  assert.match(cv, /title=\{c\.Image \|\| "Image unavailable"\}/, "the image tooltip");
  const pl = readFileSync(new URL("../views/PortsLine.tsx", import.meta.url), "utf8");
  assert.match(pl, /title=\{\[full\.docker, \.\.\.full\.host\]\.join/, "the ports tooltip carries Docker's report and the host listener in full");
});
