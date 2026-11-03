// node --test docs/ui/stable-urls/stub.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { STUB_SOURCE, fixtureUrl } from "./stub.mjs";

const run = (scene) => {
  const win = { URLSearchParams };
  const ctx = vm.createContext({ ...win, location: { search: new URL(fixtureUrl("http://x/", scene)).search }, window: win, URLSearchParams });
  vm.runInContext(STUB_SOURCE, ctx);
  return win;
};

test("hostile strings in a scene stay data and never execute", () => {
  const evil = `"});globalThis.PWNED=1;({"\`\${globalThis.PWNED=2}</script> `;
  const win = run({ snap: { x: evil }, containers: [], now: 1 });
  assert.equal(win.PWNED, undefined);
  assert.equal(globalThis.PWNED, undefined);
  return win.__TAURI_INTERNALS__.invoke("deploy_snapshot").then((s) => assert.equal(s.x, evil));
});

test("the stub answers only fixtures and refuses unknown commands (no engine, no journal)", async () => {
  const win = run({ snap: {}, containers: [], now: 1 });
  await assert.rejects(win.__TAURI_INTERNALS__.invoke("start_harpoon"), /no answer/);
});

test("the stub source is a constant and shoot.mjs builds no code from data", () => {
  assert.ok(!/\$\{[^}]*(scene|NOW)/.test(STUB_SOURCE));
  const shoot = readFileSync(new URL("./shoot.mjs", import.meta.url), "utf8");
  assert.ok(!/addScriptToEvaluateOnNewDocument", \{ source: (?!STUB_SOURCE)/.test(shoot));
});
