// A runtime attached read-only (the B7 acceptance session's legacy journal): the adapter, the recorded-vs-observed
// comparison against Docker, and the rule that nothing can operate on it. The snapshot is the exact JSON the Rust
// backend produces for the checked-in B7 fixture (a golden file both sides test against).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { externalOpProblem, isExternalAppId, observedFrom, toDeployState } from "./deployEngine.ts";
import { appStatus, navBadge } from "./deployStore.ts";
import { dialogAllowedFor } from "../views/deploy/context.ts";

const B7 = JSON.parse(readFileSync(new URL("../../src-tauri/tests/fixtures/b7-legacy-snapshot.json", import.meta.url), "utf8"));
const APP = "app_01M6DPQ543ASWBHW7PW17ZAJ3B", DEP = "dep_01M6DPQ543ASWBHW7PW17ZAJ3C", CTR = `hp-${DEP}`;
const CID = "2f70855fd334312f11ae3b2c5cb2ef42af49203820869637c71c02ead50ed936";
const OWN = { schema: 1, generatedMs: B7.generatedMs, runtimeDir: "/own", source: { kind: "own", path: "/own", journalFormat: "none", readOnly: false }, store: "missing", storeError: null, writerHeld: false,
  gate: B7.gate, repos: [], apps: [], definitions: [] };
const LABELS = { "io.harpoon.managed": "true", "io.harpoon.app": APP, "io.harpoon.deployment": DEP };
const row = (over = {}) => ({ id: CID, state: "running", labels: LABELS, ports: [38000], ...over });
const stateWith = (observed, own = OWN) => toDeployState(own, { observed, keychain: "unlocked", externals: [{ path: "/Volumes/evidence/runtime", snapshot: B7, error: null }] });
const app = (s) => s.apps[`ext0/${APP}`];

test("the golden snapshot really is a read-only legacy external source with the recorded ledger", () => {
  assert.deepEqual([B7.source.kind, B7.source.journalFormat, B7.source.readOnly, B7.store], ["external", "legacy", true, "ok"]);
  const d = B7.apps[0].deployments[0];
  assert.deepEqual(d.resources.filter((r) => r.kind === "port").map((r) => [r.name, r.state]), [["39000", "released"], ["38000", "created"]]);
});

test("an attached runtime appears as its own read-only group, with ids that cannot collide with Harpoon's own", () => {
  const s = stateWith({});
  assert.deepEqual(Object.keys(s.apps), [`ext0/${APP}`]);
  assert.equal(isExternalAppId(`ext0/${APP}`), true);
  assert.equal(isExternalAppId(APP), false);
  assert.deepEqual(s.projects.map((p) => [p.id, p.name, p.sourceLabel]), [["ext0", "External · runtime", "read-only · legacy journal.wal, not migrated"]]);
  assert.deepEqual(app(s).external, { path: "/Volumes/evidence/runtime", format: "legacy" });
  assert.deepEqual(s.engine.externals, [{ id: "ext0", path: "/Volumes/evidence/runtime", format: "legacy", store: "ok", error: null }]);
  assert.equal(s.scenario, "healthy", "an attached runtime is shown even though Harpoon's own store is empty");
  // the deployment as recorded, with the ledger's objects and ids
  const dep = app(s).deployments[0];
  assert.deepEqual([dep.id, dep.status, dep.steps.at(-1).status], [DEP, "succeeded", "skipped"]);
  assert.deepEqual(dep.recorded.filter((r) => r.kind === "container").map((r) => [r.name, r.state, r.id.slice(0, 8)]), [[`${CTR}-pre`, "released", "6b31ddb6"], [CTR, "created", "2f70855f"]]);
});

test("recorded and observed agree: running, owned by this application and deployment, on the leased port", () => {
  const a = app(stateWith({ [CTR]: row() }));
  assert.equal(a.observation.kind, "match");
  assert.deepEqual(a.observation.notes, []);
  assert.deepEqual([a.currentReleaseId, a.url, a.urlSource], ["rel_01M6DPQ543ASWBHW7PW17ZAJ3C", "http://127.0.0.1:38000", "lease"], "no pinned port in a pre-configuration journal: the recorded lease is the source, and says so");
  assert.match(a.observation.recorded, /release rel_01M6DPQ543ASWBHW7PW17ZAJ3C serving in container hp-dep_01M6DPQ543ASWBHW7PW17ZAJ3C \(2f70855fd334\), port lease 38000/);
  assert.match(a.observation.observed, /container 2f70855fd334 running, ownership labels checked, published 38000/);
  assert.equal(appStatus(a, "connected").kind, "running");
});

test("every way the record and Docker can disagree is reported, and none of them is called current", () => {
  const cases = {
    absent: [{}, "absent", /no such container/],
    stopped: [{ [CTR]: row({ state: "exited" }) }, "stopped", /exited/],
    "another container id": [{ [CTR]: row({ id: "f".repeat(64) }) }, "mismatch", /container id differs/],
    "another deployment's label": [{ [CTR]: row({ labels: { ...LABELS, "io.harpoon.deployment": "dep_OTHER" } }) }, "mismatch", /io\.harpoon\.deployment label is "dep_OTHER"/],
    "another application's label": [{ [CTR]: row({ labels: { ...LABELS, "io.harpoon.app": "app_OTHER" } }) }, "mismatch", /io\.harpoon\.app label is "app_OTHER"/],
    "not managed": [{ [CTR]: row({ labels: { "io.harpoon.app": APP, "io.harpoon.deployment": DEP } }) }, "mismatch", /not labelled io\.harpoon\.managed=true/],
    "another port": [{ [CTR]: row({ ports: [39999] }) }, "mismatch", /port lease 38000 is not among the published ports \(39999\)/],
    "docker unreachable": [null, "unknown", /cannot be compared/],
  };
  for (const [name, [observed, kind, note]] of Object.entries(cases)) {
    const a = app(stateWith(observed));
    assert.equal(a.observation.kind, kind, name);
    assert.match(a.observation.notes.join(" "), note, name);
    assert.equal(a.currentReleaseId, null, `${name}: the record is not treated as current`);
    assert.equal(a.url, "", `${name}: no URL is offered for something not confirmed`);
    assert.equal(a.lastKnownReleaseId, "rel_01M6DPQ543ASWBHW7PW17ZAJ3C", `${name}: it is still shown as last known`);
  }
});

test("when Docker's answer carries no labels the record is running but ownership is said to be unchecked", () => {
  const a = app(stateWith({ [CTR]: row({ labels: null }) }));
  assert.equal(a.observation.kind, "unverified");
  assert.match(a.observation.notes[0], /no labels/);
  assert.equal(a.currentReleaseId, "rel_01M6DPQ543ASWBHW7PW17ZAJ3C");
});

test("docker ps rows are read for id, state, labels and published ports", () => {
  const o = observedFrom([{ ID: CID, Names: CTR, State: "running", Labels: Object.entries(LABELS).map(([k, v]) => `${k}=${v}`).join(","), Ports: "127.0.0.1:38000->8080/tcp, [::]:38000->8080/tcp" }]);
  assert.deepEqual(o[CTR], { id: CID, state: "running", labels: LABELS, ports: [38000, 38000] });
  assert.equal(app(stateWith(o)).observation.kind, "match");
});

test("Harpoon's own applications get the same comparison", () => {
  const own = { ...OWN, store: "ok", source: { ...OWN.source, journalFormat: "sqlite" }, apps: B7.apps.map((a) => ({ ...a, id: "app_OWN", serving: { ...a.serving, deployment: "dep_OWN", container: "hp-dep_OWN" }, deployments: a.deployments.map((d) => ({ ...d, id: "dep_OWN", resources: d.resources.map((r) => (r.kind === "container" && r.state === "created" ? { ...r, name: "hp-dep_OWN" } : r)) })) })) };
  const s = toDeployState(own, { observed: { "hp-dep_OWN": row({ labels: { "io.harpoon.managed": "true", "io.harpoon.app": "app_OWN", "io.harpoon.deployment": "dep_OWN" } }) }, keychain: "unlocked" });
  assert.equal(s.apps.app_OWN.observation.kind, "match");
  assert.equal(s.apps.app_OWN.external, undefined);
  const wrong = toDeployState(own, { observed: { "hp-dep_OWN": row({ labels: LABELS }) }, keychain: "unlocked" });
  assert.equal(wrong.apps.app_OWN.observation.kind, "mismatch", "a container that merely has the right name is not enough");
});

test("an unreadable attached runtime is reported and contributes nothing; it never raises Harpoon's own alerts", () => {
  const s = toDeployState(OWN, { observed: {}, keychain: "unlocked", externals: [{ path: "/gone", snapshot: null, error: "Cannot read /gone: No such file or directory" }] });
  assert.deepEqual(s.projects, []);
  assert.deepEqual(s.engine.externals.map((e) => [e.path, e.error]), [["/gone", "Cannot read /gone: No such file or directory"]]);
  // a recorded-but-absent external deployment is "down" on its own page, yet raises no navigation badge
  assert.equal(navBadge(stateWith({}), "connected"), "none");
});

test("nothing can be aimed at an attached runtime from the frontend either", () => {
  assert.match(externalOpProblem({ op: "resume", app: `ext0/${APP}`, mode: "dismiss" }), /attached read-only/);
  assert.match(externalOpProblem({ op: "deploy", app: "ext12/x" }), /attached read-only/);
  assert.equal(externalOpProblem({ op: "resume", app: APP, mode: "dismiss" }), null);
  assert.equal(externalOpProblem({ op: "poll" }), null);
  const ext = app(stateWith({}));
  for (const kind of ["deploy", "redeploy", "promote", "retry", "rollback", "recover", "resume", "variable"]) {
    assert.equal(dialogAllowedFor({ kind }, "production", ext), false, kind);
  }
  assert.equal(dialogAllowedFor({ kind: "deploy" }, "production", { ...ext, external: undefined }), true, "the same dialog is fine for an application of Harpoon's own");
  assert.equal(dialogAllowedFor({ kind: "create", step: 1, src: "git" }, "production", ext), true, "creating a project is not an operation on it");
});
