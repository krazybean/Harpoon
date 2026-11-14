import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parseCleanup, parseCache, nothingToClean, freedSummary, groupByApplication, reasonLabel, operationSummary, removable, reconcilable, UNASSOCIATED, shortDeployment } from "./artifactCleanup.ts";

const item = (o) => `item: ${JSON.stringify({ app: "app_A", appName: "collectiv", environment: "production", deployment: "dep_01M7D1AJHSVQ45GXQFTG64ZG5Q", createdMs: 1_700_000_000_000, status: "succeeded", commit: "a68140e", kind: "container", name: "hp-dep_x", verdict: "ready", reason: "superseded", detail: "d", physical: true, ...o })}`;
const report = (...lines) => parseCleanup(lines.join("\n"));

const MIXED = report(
  item({}),
  item({ kind: "image", name: "harpoon/app:dep_x-0123", reason: "superseded" }),
  item({ kind: "port-lease", name: "38000", verdict: "reconcile", reason: "stale-port-lease", physical: null }),
  item({ kind: "container", verdict: "reconcile", reason: "already-absent", physical: false, deployment: "dep_OLD0001" }),
  item({ app: "app_B", appName: "juancastro.me", kind: "image", verdict: "ready", reason: "failed-attempt", deployment: "dep_FAIL001", status: "failed" }),
  item({ app: "app_B", appName: "juancastro.me", kind: "container", verdict: "protected", reason: "serving-release", deployment: "dep_SERV001" }),
  item({ app: "app_Z", appName: null, environment: null, commit: null, kind: "image", verdict: "review", reason: "needs-review", detail: "a protected deployment records something unexpected" }),
  "reclaimed-containers: 1", "reclaimed-images: 2",
);

test("items are grouped by the application the journal names; unknown owners are never guessed into one", () => {
  const groups = groupByApplication(MIXED);
  assert.deepEqual(groups.map((g) => g.title), ["collectiv", "juancastro.me", UNASSOCIATED], "apps with work first, unassociated last");
  const c = groups[0];
  assert.deepEqual([c.ready.containers, c.ready.images, c.reconcile, c.protectedCount, c.review], [1, 1, 2, 0, 0]);
  assert.equal(c.environment, "production");
  const u = groups[2];
  assert.equal(u.unassociated, true);
  assert.equal(u.environment, null);
  assert.equal(u.review, 1);
});

test("two applications with the same deployment history stay apart", () => {
  const r = report(item({ app: "app_1", appName: "one", deployment: "dep_SAME01" }), item({ app: "app_2", appName: "two", deployment: "dep_SAME01" }));
  assert.deepEqual(groupByApplication(r).map((g) => g.title).sort(), ["one", "two"]);
  // the same display name registered for two journal applications is still two groups: grouping is by id
  const dup = report(item({ app: "app_1", appName: "same" }), item({ app: "app_2", appName: "same" }));
  assert.equal(groupByApplication(dup).length, 2);
});

test("every verdict reads as plain language, and an absent object never promises disk space", () => {
  const label = (o) => reasonLabel(parseCleanup(item(o)).items[0]);
  assert.equal(label({}), "Stopped container from an older deployment");
  assert.equal(label({ kind: "image" }), "Unused image from a previous release");
  assert.equal(label({ kind: "port-lease", reason: "stale-port-lease", verdict: "reconcile", physical: null }), "Stale port reservation");
  assert.equal(label({ reason: "already-absent", verdict: "reconcile", physical: false }), "Container already removed; journal needs reconciliation");
  assert.equal(label({ kind: "image", reason: "shared-with-protected", verdict: "protected" }), "Image retained because a protected deployment still references it");
  assert.match(label({ reason: "serving-release", verdict: "protected" }), /^Serving now/);
  assert.match(label({ reason: "needs-review", verdict: "review", detail: "ownership is unclear" }), /^Needs review: ownership is unclear/);
  for (const text of MIXED.items.map(reasonLabel)) assert.doesNotMatch(text, /sha256|dep_|ledger|unreleased resources/, text);
});

test("the confirmation counts Docker objects, journal-only records and protected items separately, with no byte estimate", () => {
  const s = operationSummary(MIXED);
  assert.deepEqual([s.containers, s.images, s.journalOnly, s.protectedCount, s.review], [1, 2, 2, 1, 1]);
  assert.deepEqual(s.apps.sort(), ["collectiv", "juancastro.me"].sort());
  assert.equal(removable(MIXED).length, 3);
  assert.equal(reconcilable(MIXED).length, 2);
  const src = readFileSync(new URL("../components/DiskReclaimCard.tsx", import.meta.url), "utf8");
  // the confirmation's wording is exercised by the component tests in reviewOldReleases.test.mjs
  assert.doesNotMatch(src, /\bMB\b.*estimate|nominal/i);
});

test("a protected release alone is nothing to clean, and a repeated run (idempotent) is exactly that", () => {
  const again = report(item({ verdict: "protected", reason: "serving-release" }), item({ kind: "image", verdict: "protected", reason: "serving-release" }), "reclaimed-containers: 0", "reclaimed-images: 0");
  assert.equal(nothingToClean(again), true);
  assert.equal(nothingToClean(MIXED), false);
  assert.equal(nothingToClean(report(item({ kind: "port-lease", verdict: "reconcile", reason: "stale-port-lease", physical: null }))), false, "a stale reservation is still worth reconciling");
});

test("raw identifiers live only behind the Technical details disclosure", () => {
  const src = readFileSync(new URL("../components/DiskReclaimCard.tsx", import.meta.url), "utf8");
  const group = src.slice(src.indexOf("function ArtifactGroup"), src.indexOf("/** BuildKit"));
  const [shown, technical] = group.split("<details>");
  const visible = shown.replace(/key=\{`[^`]*`\}/g, ""); // keys are not displayed
  assert.ok(technical, "a details element exists");
  assert.match(technical, /<summary[^>]*>Technical details<\/summary>/);
  for (const raw of ["i.name", "i.app}", "i.detail", "i.deployment}"]) assert.ok(technical.includes(raw), `${raw} is in the disclosure`);
  for (const raw of ["i.name", "i.detail", "i.app}"]) assert.ok(!visible.includes(raw), `${raw} is not shown by default`);
  assert.match(visible, /shortDeployment\(i\.deployment\)/, "only the short deployment id is shown");
  assert.equal(shortDeployment("dep_01M7D1AJHSVQ45GXQFTG64ZG5Q"), "dep…64ZG5Q");
});

test("malformed or unknown lines are skipped, not guessed at", () => {
  const r = report("item: {not json", `item: ${JSON.stringify({ verdict: "ready", kind: "volume", deployment: "d", app: "a" })}`, "item: {\"verdict\":\"ready\"}", item({}));
  assert.equal(r.items.length, 1);
});

test("freed space is the guest filesystem's own before/after, never a sum of image sizes", () => {
  const applied = report("guest-free-before-bytes: 1000", "guest-free-after-bytes: 4000");
  const size = (b) => `${b}B`;
  assert.equal(freedSummary(applied, size), "Guest free space: 1000B before, 4000B after (+3000B).");
  assert.match(freedSummary(report("guest-free: not measurable"), size), /could not be measured/);
});

test("the build cache report carries Docker's figure and the guest's measured free space separately", () => {
  assert.deepEqual(parseCache("build-cache-reclaimable-bytes: 1250"), { reclaimable: 1250, reclaimed: null, guestFreeBefore: null, guestFreeAfter: null });
  const done = parseCache("build-cache-reclaimed-bytes: 1250\nguest-free-before-bytes: 10\nguest-free-after-bytes: 20");
  assert.equal(done.reclaimed, 1250);
  assert.equal(done.guestFreeAfter - done.guestFreeBefore, 10);
});
