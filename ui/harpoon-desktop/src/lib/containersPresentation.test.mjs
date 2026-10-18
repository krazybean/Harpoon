import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const viewSource = readFileSync(new URL("../views/ContainersView.tsx", import.meta.url), "utf8");
const styleSource = readFileSync(new URL("../styles/primitives.css", import.meta.url), "utf8");

test("containers opens on stacks and keeps the gallery as the flex scroll surface", () => {
  assert.match(viewSource, /useState<"table" \| "stacks">\("stacks"\)/);
  assert.match(viewSource, /className="containers-view__content"/);
  assert.match(styleSource, /\.shell-main--containers \{ display: flex; flex-direction: column; min-height: 0; overflow: hidden; \}/);
  assert.match(styleSource, /\.containers-view__content \{ display: flex; min-width: 0; min-height: 0; flex-direction: column;/);
  assert.match(styleSource, /\.stack-gallery \{[^}]*min-height: 0;[^}]*overflow-y: auto;/);
  assert.match(styleSource, /\.stack-workspace \{[^}]*overflow: hidden;/);
});

test("container cards keep long metadata inside the tile and disclose full values", () => {
  assert.match(viewSource, /<strong title=\{name\}>\{name\}<\/strong>/);
  assert.match(viewSource, /title=\{c\.Image \|\| "Image unavailable"\}/);
  assert.match(styleSource, /\.stack-gallery \{[^}]*min-width: 0;[^}]*overflow-x: hidden;/);
  assert.match(styleSource, /\.stack-workspace \{[^}]*min-width: 0;[^}]*overflow: hidden;/);
  assert.match(styleSource, /\.container-tile \{[^}]*box-sizing: border-box;[^}]*min-width: 0;/);
});
