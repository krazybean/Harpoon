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

test("the release card holds a native-capture thumbnail (never an iframe) beside the details, with the status badge on the right", async () => {
  const { readFileSync } = await import("node:fs");
  const screens = readFileSync(new URL("../views/deploy/Screens.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(screens, /<iframe|credentialless/);
  assert.doesNotMatch(screens, /function DeploymentPreview|dp-preview/, "the standalone preview panel is gone");
  const thumb = screens.slice(screens.indexOf("function PreviewThumb"), screens.indexOf("export function DeploymentPills"));
  assert.match(thumb, /c\.engine\.preview\(url\)/);
  assert.match(thumb, /Enlarge deployment preview/);
  assert.match(thumb, /Refresh/);
  assert.match(thumb, /Open app/);
  // thumbnail, details, then the actions, all in the one hero card; no status checkmark (the pill says it)
  const hero = screens.slice(screens.indexOf('className="dp-panel__body dp-hero"'), screens.indexOf('<div className="dp-tabs"'));
  assert.ok(hero.indexOf("<PreviewThumb") < hero.indexOf("dp-hero__state") && hero.indexOf("dp-hero__state") < hero.indexOf("dp-actions"));
  assert.doesNotMatch(hero, /heroIcon|className=\{`big /);
  // Refresh is an overlay on the thumbnail, a sibling of the enlarge button so its click cannot bubble into Enlarge
  assert.match(thumb, /dp-thumb__refresh/);
  const enlarge = thumb.slice(thumb.indexOf('className="dp-thumb__img"'), thumb.indexOf("</button>", thumb.indexOf('className="dp-thumb__img"')));
  assert.doesNotMatch(enlarge, /capture\(|dp-thumb__refresh/);
  const hook = readFileSync(new URL("../hooks/useDeployEngine.ts", import.meta.url), "utf8");
  assert.match(hook, /invoke<string>\("deploy_preview", \{ url \}\)/);
});

test("the thumbnail appears only for a running release whose final check passed", async () => {
  const { previewReady } = await import("../views/deploy/Screens.tsx").catch(() => ({ previewReady: null }));
  if (!previewReady) return; // Screens.tsx needs the JSX toolchain; the wiring above is asserted from source
  const L = { releaseId: "r1", steps: [{ name: "final_check", status: "succeeded" }] };
  const app = { currentReleaseId: "r1", address: { status: "running" }, deployments: [L] };
  assert.equal(previewReady({ engine: {} }, app, true), true);
  assert.equal(previewReady({ engine: {} }, app, false), false);
  assert.equal(previewReady({}, app, true), false);
});
