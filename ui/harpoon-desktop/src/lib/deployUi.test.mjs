import test from "node:test";
import assert from "node:assert/strict";
import { copyText, visibleLogLines } from "./deployUi.ts";

test("visibleLogLines matches the exact text shown by the log panel", () => {
  const lines = [{ kind: "", text: "build started" }, { kind: "", text: "DATABASE ready" }, { kind: "", text: "done" }];
  assert.deepEqual(visibleLogLines(lines, " database ").map((line) => line.text), ["DATABASE ready"]);
  assert.deepEqual(visibleLogLines(lines, "").map((line) => line.text), lines.map((line) => line.text));
});

test("copyText preserves the selected log text and reports clipboard failures", async () => {
  const copied = [];
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { clipboard: { writeText: async (text) => copied.push(text) } } });
  await copyText("one\ntwo");
  assert.deepEqual(copied, ["one\ntwo"]);
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { clipboard: { writeText: async () => { throw new Error("denied"); } } } });
  // a refused async write falls back to the selection copy; with no document either, the failure is reported
  await assert.rejects(() => copyText("secret-free log"), /clipboard is unavailable/);
});

test("copyText falls back to the selection copy when the async clipboard refuses", async () => {
  const made = [];
  const area = { style: {}, setAttribute() {}, select() {}, remove() {}, set value(v) { made.push(v); } };
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { clipboard: { writeText: async () => { throw new Error("NotAllowedError"); } } } });
  globalThis.document = { createElement: () => area, body: { appendChild() {} }, execCommand: (c) => c === "copy" };
  try { await copyText("line1\nline2"); } finally { delete globalThis.document; }
  assert.deepEqual(made, ["line1\nline2"]);
});

test("the Overview preview is a native capture, never an iframe (WebKit has no credentialless frames and apps may forbid framing)", async () => {
  const { readFileSync } = await import("node:fs");
  const screens = readFileSync(new URL("../views/deploy/Screens.tsx", import.meta.url), "utf8");
  const preview = screens.slice(screens.indexOf("function DeploymentPreview"), screens.indexOf("export function DeploymentPills"));
  assert.doesNotMatch(preview, /<iframe|credentialless/);
  assert.match(preview, /c\.engine\.preview\(url\)/);
  const hook = readFileSync(new URL("../hooks/useDeployEngine.ts", import.meta.url), "utf8");
  assert.match(hook, /invoke<string>\("deploy_preview", \{ url \}\)/);
});
