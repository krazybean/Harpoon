import assert from "node:assert/strict";
import test from "node:test";
import { NO_SETTINGS, detectionLines, hasSettings, parseSettings, sameSettings, settingsJson, settingsProblem } from "./sourceSettings.ts";
import { registrationEdit, registrationFormOf } from "./deployEngine.ts";

const REPO = { name: "shop", url: "file:///Users/me/shop", app: "app_1", project: null, branch: "main", containerPort: 3000, dockerfile: null, healthPath: null, settings: null, credential: "none", registeredMs: 1, seen: [] };

test("only what differs from the defaults is sent, and nothing is the empty object", () => {
  assert.equal(settingsJson(NO_SETTINGS), "{}");
  assert.equal(hasSettings(NO_SETTINGS), false);
  const s = { ...NO_SETTINGS, start: "  npm start ", style: "development", network: "proj_default" };
  assert.deepEqual(JSON.parse(settingsJson(s)), { start: "npm start", style: "development", network: "proj_default" });
  assert.ok(sameSettings(parseSettings(settingsJson(s)), s));
});

test("unreadable stored settings are no settings, never a guess", () => {
  assert.deepEqual(parseSettings("not json"), NO_SETTINGS);
  assert.deepEqual(parseSettings(null), NO_SETTINGS);
});

test("the form refuses what the engine would refuse", () => {
  assert.match(settingsProblem({ ...NO_SETTINGS, start: "a\nb" }), /one line/);
  assert.match(settingsProblem({ ...NO_SETTINGS, network: "host" }), /user-defined/);
  assert.equal(settingsProblem({ ...NO_SETTINGS, network: "collectiv-harpoon-test_default" }), null);
});

test("changing a build setting is an edit; leaving it is not; clearing sends the empty object", () => {
  const form = registrationFormOf(REPO);
  assert.equal(registrationEdit(REPO, form, false).op, null);
  const e = registrationEdit(REPO, { ...form, settings: { ...NO_SETTINGS, start: "npm start" } }, false);
  assert.equal(e.op.settings, '{"start":"npm start"}');
  assert.deepEqual(e.changed, ["build settings"]);
  const had = { ...REPO, settings: '{"start":"npm start"}' };
  const cleared = registrationEdit(had, { ...registrationFormOf(had), settings: NO_SETTINGS }, false);
  assert.equal(cleared.op.settings, "{}");
});

test("detection is described in plain lines", () => {
  assert.match(detectionLines({ hasDockerfile: true })[0], /Dockerfile/);
  const l = detectionLines({ hasDockerfile: false, framework: "Next.js", baseImage: "node:22-slim", install: "npm ci", build: "npm run build", start: "npm start", notes: ["n"] });
  assert.match(l[0], /Next\.js on node:22-slim/);
  assert.match(l[1], /Install: npm ci · Build: npm run build · Start: npm start/);
});

test("the branch dropdown keeps the configured branch selectable, flagged when the folder lacks it", async () => {
  const { branchChoices } = await import("./deployEngine.ts");
  assert.deepEqual(branchChoices(["main", "dev"], "dev"), { options: ["main", "dev"], missing: false });
  // a refresh that no longer lists the chosen branch must not swap it for another
  assert.deepEqual(branchChoices(["main"], "release"), { options: ["release", "main"], missing: true });
  assert.deepEqual(branchChoices(["main"], ""), { options: ["main"], missing: false });
});

test("choosing another branch records only the branch; the serving release is not part of the edit", () => {
  const form = { ...registrationFormOf(REPO), branch: "dev" };
  const edit = registrationEdit(REPO, form, true);
  assert.deepEqual(edit.changed, ["branch"]);
  assert.equal(edit.op.branch, "dev");
  assert.equal(registrationEdit(REPO, registrationFormOf(REPO), true).op, null, "an untouched dropdown changes nothing");
});
