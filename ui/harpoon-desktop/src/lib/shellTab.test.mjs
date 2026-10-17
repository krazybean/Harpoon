import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const source = fs.readFileSync(new URL("../views/ShellTab.tsx", import.meta.url), "utf8");

test("shell tab keeps transport events session-scoped and bounded", () => {
  assert.match(source, /event\.sessionId === sessionId/);
  assert.match(source, /MAX_PENDING_OUTPUT = 512 \* 1024/);
  assert.match(source, /open_shell_session/);
  assert.match(source, /resize_shell_session/);
  assert.match(source, /closeSession\(\)/);
  assert.match(source, /requestAnimationFrame\(flush\)/);
  assert.match(source, /resizeObserver\.disconnect\(\)/);
});

test("shell tab does not connect until the explicit action", () => {
  assert.match(source, /connectNonce === 0/);
  assert.match(source, /<button className=\"btn btn--secondary btn--sm\" onClick=\{connect\}/);
  assert.match(source, /connectRequest = 0/);
  assert.match(source, /if \(state === \"connected\" \|\| state === \"connecting\"\) return/);
});

test("shell tab forwards xterm dimensions by name without swapping axes", () => {
  assert.match(source, /shellDimensions\(terminal\.cols, terminal\.rows\)/);
  assert.match(source, /shellDimensions\(cols, rows\)/);
  assert.match(source, /export function shellDimensions\(cols: number, rows: number\)/);
});
