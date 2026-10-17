import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("../views/ContainerDetailView.tsx", import.meta.url), "utf8");

test("container detail exposes inspect-backed network and storage workspaces", () => {
  assert.match(source, /type DetailTab = .*"networks".*"storage"/);
  assert.match(source, /NetworkSettings\?\.Networks/);
  assert.match(source, /detail\?\.Mounts/);
  assert.match(source, /Endpoint ID/);
  assert.match(source, /Propagation/);
  assert.match(source, /onNavigate\("networks"\)/);
  assert.match(source, /onNavigate\("volumes"\)/);
});
