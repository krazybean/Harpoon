import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("./shellSessionStore.ts", import.meta.url), "utf8");

test("shared shell owner is keyed by explicit target identity and detaches without closing", () => {
  assert.match(source, /targetKey\(target: ShellTarget\)/);
  assert.match(source, /context, target\.containerId, target\.executable, target\.user/);
  assert.match(source, /record\.listeners\.add\(listener\)/);
  assert.match(source, /detach: \(\) => record\?\.listeners\.delete\(listener\)/);
  assert.match(source, /close: \(\) => this\.close\(key\)/);
  assert.match(source, /MAX_REPLAY_BYTES = 512 \* 1024/);
});

test("owner scopes native events by the session id and bounds replay", () => {
  assert.match(source, /payload\.sessionId !== record\.sessionId/);
  assert.match(source, /record\.buffer = record\.buffer\.slice\(-MAX_REPLAY_BYTES\)/);
  assert.match(source, /close_shell_session/);
});

test("target remount resets the terminal before replaying shared output", () => {
  const shellTab = readFileSync(new URL("../views/ShellTab.tsx", import.meta.url), "utf8");
  assert.match(shellTab, /setState\("connecting"\); setMessage\(""\); terminal\.reset\(\);/);
  assert.doesNotMatch(shellTab, /terminal\.clear\(\);/);
});
