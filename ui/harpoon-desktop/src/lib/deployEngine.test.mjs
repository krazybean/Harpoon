import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { appStatus } from "./deployStore.ts";
import { deploymentOf, liveBlock, logLines, observedFrom, toDeployState } from "./deployEngine.ts";

const NOW = 10_000_000;
const GATE = { liveDeploy: false, liveReplacement: false, reason: "closed in this build" };
const step = (name, status, extra = {}) => ({ name, status, startedMs: NOW - 60000, finishedMs: NOW - 50000, note: "", log: false, ...extra });
const dep = (id, extra = {}) => ({
  id, resources: [], status: "succeeded", trigger: "cli", createdMs: NOW - 120000, startedMs: NOW - 119000, finishedMs: NOW - 100000, steps: [step("source", "succeeded"), step("build", "succeeded")],
  cutover: "confirmed", cutoverAtMs: NOW - 101000, failure: null, recovery: null, interruption: null, release: `rel_${id}`, replaces: null, rollbackTo: null, mayBeDown: false, unfinished: false,
  source: { repo: "shop", url: "https://example.com/shop.git", selector: "branch", requested: "main", commit: "0123456789abcdef", trigger: "manual", delivery: null },
  pin: { imageTag: `harpoon/${id}:1`, imageId: "sha256:x", hostPort: 41000, containerPort: 3000, healthPath: "/healthz", envNames: ["MODE"], secrets: [{ name: "DB", id: "app:a/DB", version: 2 }], snapshot: id, build: null },
  config: null, configDiff: [], ...extra,
});
const app = (deployments, extra = {}) => ({ id: "app_1", repo: "shop", deployments, serving: null, pauses: [], events: [], ...extra });
const snap = (apps, extra = {}) => ({
  schema: 1, generatedMs: NOW, runtimeDir: "/x", source: { kind: "own", path: "/x", journalFormat: "sqlite", readOnly: false }, store: "ok", storeError: null, writerHeld: false, gate: GATE, apps, definitions: [],
  repos: [{ name: "shop", url: "https://example.com/shop.git", app: "app_1", project: null, environment: null, branch: "main", filters: [], credential: "none", webhook: null, containerPort: 3000, dockerfile: "Dockerfile", context: null, target: null, healthPath: "/healthz", seen: [] }], ...extra,
});
const st = (s, o = {}) => toDeployState(s, { observed: {}, keychain: "unlocked", ...o });

test("nothing loaded is 'loading', and a failed first load is an error with no invented apps", () => {
  assert.equal(st(null).scenario, "loading");
  const failed = st(null, { loadError: "backend unavailable" });
  assert.deepEqual([Object.keys(failed.apps).length, failed.projects.length, failed.engine.loadError], [0, 0, "backend unavailable"]);
});

test("a missing store is empty, a damaged store shows nothing and carries the reason", () => {
  assert.equal(st(snap([], { store: "missing" })).scenario, "empty");
  const bad = st(snap([{ ...app([dep("dep_1")]) }], { store: "error", storeError: "database disk image is malformed" }));
  assert.deepEqual([Object.keys(bad.apps).length, bad.engine.storeError], [0, "database disk image is malformed"]);
});

test("no seeded or fabricated content: every app, id and release comes from the snapshot", () => {
  const s = st(snap([app([dep("dep_1")])]));
  assert.deepEqual(Object.keys(s.apps), ["app_1"]);
  assert.deepEqual(s.apps.app_1.deployments.map((d) => d.id), ["dep_1"]);
  assert.deepEqual(s.apps.app_1.previews, []);
  assert.equal(s.apps.app_1.resources, null);
  assert.equal(s.job, null);
});

test("recorded serving release is only 'running' when Docker was observed running it", () => {
  const serving = { release: "rel_dep_1", deployment: "dep_1", container: "hp-dep_1", via: "original", sinceMs: NOW - 90000, hostPort: 41000, healthPath: "/healthz" };
  const a = app([dep("dep_1")], { serving });
  const up = st(snap([a]), { observed: { "hp-dep_1": { id: "c1", state: "running", labels: { "io.harpoon.managed": "true", "io.harpoon.app": "app_1", "io.harpoon.deployment": "dep_1" }, ports: [41000], mappings: [] } } }).apps.app_1;
  assert.deepEqual([up.currentReleaseId, up.url], ["rel_dep_1", "http://127.0.0.1:41000"]);
  assert.equal(appStatus(up, "connected").kind, "running");
  const gone = st(snap([a]), { observed: {} }).apps.app_1;
  assert.deepEqual([gone.currentReleaseId, gone.lastKnownReleaseId, gone.url], [null, "rel_dep_1", ""]);
  assert.equal(appStatus(gone, "connected").kind, "down", "recorded but not running is down, not 'running'");
  assert.equal(appStatus(gone, "stopped").kind, "unknown", "a stopped VM makes the state unknown");
  const unknown = st(snap([a]), { observed: null }).apps.app_1;
  assert.equal(unknown.currentReleaseId, null, "an unanswered Docker is never read as running");
});

test("an unfinished deployment with no writer is interrupted; with a writer it is running", () => {
  const d = dep("dep_2", { status: "running", unfinished: true, finishedMs: null, release: null, steps: [step("source", "succeeded"), step("build", "running", { finishedMs: null })] });
  const gone = deploymentOf(d, NOW, false);
  assert.equal(gone.status, "interrupted");
  assert.match(gone.failure, /stopped before this deployment finished/);
  assert.equal(gone.steps[1].status, "warning");
  assert.equal(deploymentOf(d, NOW, true).status, "running");
});

test("an interrupted cutover says the application may be down", () => {
  const d = dep("dep_3", { status: "running", unfinished: true, cutover: "begun", mayBeDown: true, finishedMs: null });
  assert.match(deploymentOf(d, NOW, false).failure, /may be down/);
});

test("failed build: nothing was stopped; failed cutover with a restored release keeps both facts", () => {
  const b = deploymentOf(dep("dep_4", { status: "failed", release: null, failure: { step: "build", kind: "build", message: "RUN failed", timedOut: false } }), NOW, false);
  assert.deepEqual([b.status, b.failedAt, b.nothingStopped], ["failed", "build", true]);
  const c = deploymentOf(dep("dep_5", {
    status: "failed", release: null, cutover: "failed", failure: { step: "cutover", kind: "cutover", message: "new release unhealthy", timedOut: false },
    recovery: { outcome: "restored", reason: null, reasonText: null, startedMs: NOW - 5000, finishedMs: NOW - 2000, budgetMs: 60000 }, interruption: { startedMs: NOW - 9000, endedMs: NOW - 2000 },
  }), NOW, false);
  assert.deepEqual([c.nothingStopped, c.recovery.outcome, c.interruption.label, c.recovery.durationLabel], [false, "restored", "7s", "3s"]);
});

test("a recovery recorded as in progress with no writer is never shown as restoring", () => {
  const d = dep("dep_6", { status: "failed", release: null, recovery: { outcome: "in_progress", reason: null, reasonText: null, startedMs: NOW - 1000, finishedMs: null, budgetMs: 60000 }, interruption: { startedMs: NOW - 2000, endedMs: null } });
  const gone = deploymentOf(d, NOW, false);
  assert.equal(gone.recovery.outcome, "restore_failed");
  assert.match(gone.recovery.detail, /result is unknown/);
  assert.match(gone.interruption.label, /unknown end/);
  assert.equal(deploymentOf(d, NOW, true).recovery.outcome, "in_progress");
});

test("pauses, held events and selected revisions are reported; resume needs the full revision", () => {
  const pausedApp = app([dep("dep_7", { status: "failed", release: null, cutover: "failed" })], {
    pauses: [{ origin: "dep_7", kind: "failed_cutover", sinceMs: NOW - 60000, context: "", recorded: true }],
    events: [{ delivery: "delivery-aaaaaaaaaaaa", source: "github", revision: "feedfacefeedface", receivedMs: NOW - 30000, status: "held", decidedMs: null, related: null },
      { delivery: "delivery-bbbbbbbbbbbb", source: "github", revision: "abcdabcdabcd", receivedMs: NOW - 20000, status: "selected", decidedMs: NOW - 10000, related: null },
      { delivery: "delivery-cccccccccccc", source: "github", revision: "0000000", receivedMs: NOW - 10000, status: "dismissed", decidedMs: NOW, related: null }],
  });
  const a = st(snap([pausedApp])).apps.app_1.automation;
  assert.equal(a.state, "paused");
  assert.deepEqual(a.held.map((h) => [h.status, h.revisionFull]), [["held", "feedfacefeedface"]], "held means undecided");
  assert.deepEqual(a.selected.map((h) => [h.status, h.revisionFull]), [["selected", "abcdabcdabcd"]], "a selection is a different state, kept apart");
  assert.equal(a.reason.kind, "failed_cutover");
});

test("secrets in variables and pins are names/ids/versions only", () => {
  const defs = [
    { scope: "app:app_1", name: "DB", kind: "secret", build: false, runtime: true, value: null, secretId: "app:app_1/DB", version: 3, atMs: NOW - 60000 },
    { scope: "app:app_1@production", name: "MODE", kind: "config", build: true, runtime: true, value: "prod", secretId: null, version: null, atMs: NOW - 60000 },
    { scope: "app:other", name: "NOPE", kind: "config", build: false, runtime: true, value: "x", secretId: null, version: null, atMs: NOW },
  ];
  const s = st(snap([app([dep("dep_1")])], { definitions: defs })).apps.app_1;
  assert.deepEqual(s.variables.map((v) => [v.name, v.type, v.scope, v.used, v.value, v.version]), [["DB", "secret", "application", "runtime", undefined, 3], ["MODE", "config", "environment", "build+runtime", "prod", undefined]]);
  assert.deepEqual(s.releases[0].secrets, [{ name: "DB", version: 2 }]);
  assert.doesNotMatch(JSON.stringify(s), /s3cr3t/);
});

test("the shipping gate is surfaced with the engine's own reason", () => {
  assert.equal(liveBlock(st(snap([]))), "closed in this build");
  assert.equal(liveBlock(st(snap([], { gate: { liveDeploy: true, liveReplacement: true, reason: "" } }))), null);
});

test("recorded step logs: withheld output is stated, truncation is stated, text is kept verbatim", () => {
  const lines = logLines([{ step: "build", text: "ok\nerror: boom\n", withheld: false, truncated: true, atMs: 1 }, { step: "source", text: "", withheld: true, truncated: false, atMs: 2 }]);
  assert.match(lines[0].text, /earlier output was cut/);
  assert.deepEqual(lines.slice(1, 3).map((l) => [l.kind, l.text]), [["", "ok"], ["err", "error: boom"]]);
  assert.match(lines.at(-1).text, /withheld/);
});

test("observed Docker state parses names and treats a non-answer as unknown", () => {
  const row = { id: "abc", state: "running", labels: { "io.harpoon.managed": "true", "io.harpoon.app": "app_1" }, ports: [38000], mappings: [{ hostIp: "127.0.0.1", hostPort: 38000, containerPort: 8080, proto: "tcp" }] };
  assert.deepEqual(observedFrom([{ Names: "/a,b", ID: "abc", State: "Running", Labels: "io.harpoon.managed=true,io.harpoon.app=app_1", Ports: "127.0.0.1:38000->8080/tcp" }]), { a: row, b: row });
  assert.equal(observedFrom([{ Names: "x", ID: "1", State: "exited" }]).x.labels, null, "no Labels in Docker's answer means nothing to check, not 'no labels'");
  assert.equal(observedFrom(null), null);
});

test("the engine hook is the only place that calls Tauri; views never do", () => {
  const hook = readFileSync(new URL("../hooks/useDeployEngine.ts", import.meta.url), "utf8");
  assert.match(hook, /deploy_snapshot/);
  assert.match(hook, /deploy_op/);
  assert.doesNotMatch(hook, /\bdeploy_op\b[^\n]*(live|bypass|acceptance)/i);
});

test("production shows the engine; the simulation is only reachable through the development preview", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(app, /deployPreviewActive \? \{ sim: deploySim, preview: deployPreview \} : \{ engine: deployEngine \}/);
  assert.match(app, /const deployPreview = import\.meta\.env\.DEV/);
  const view = readFileSync(new URL("../views/deploy/DeployView.tsx", import.meta.url), "utf8");
  assert.match(view, /\{sim \? <>/, "simulation controls render only in simulation mode");
});

import { displayUrl, fileUrlFor } from "./deployEngine.ts";

test("a local folder becomes one safely encoded file URL, and bad paths become none", () => {
  assert.equal(fileUrlFor("/Users/juan/Harpoon UI Test"), "file:///Users/juan/Harpoon%20UI%20Test");
  assert.equal(fileUrlFor("/Users/x/café #1 100%"), "file:///Users/x/caf%C3%A9%20%231%20100%25");
  assert.equal(fileUrlFor("/plain/path-1.2_3~"), "file:///plain/path-1.2_3~");
  assert.equal(fileUrlFor("file:///Users/x/a b"), "file:///Users/x/a%20b", "an existing file URL is encoded once from its path");
  for (const bad of ["relative/path", "", "/a/../b", "/a/./b", "/a/\nb", "/a/\u0000", "/a/\u007f"]) assert.equal(fileUrlFor(bad), null, JSON.stringify(bad));
});

test("an encoded URL never carries a raw space, control character or extra separator", () => {
  for (const p of ["/a b/c d", "/x/\u{1F600}", "/q?x=1#frag", "/p%2e%2e/q"]) {
    const u = fileUrlFor(p);
    assert.match(u, /^file:\/\/\/[A-Za-z0-9/._~%-]*$/, u);
    assert.equal(decodeURIComponent(u.slice(7)), p, "it decodes back exactly once");
  }
  assert.equal(fileUrlFor("/p%2e%2e/q"), "file:///p%252e%252e/q", "a literal percent is encoded, so it is never read as a dot");
});

test("recorded file URLs are shown as folder paths; anything odd is shown as recorded", () => {
  assert.equal(displayUrl("file:///Users/x/Harpoon%20UI%20Test"), "/Users/x/Harpoon UI Test");
  assert.equal(displayUrl("https://github.com/acme/shop.git"), "https://github.com/acme/shop.git");
  assert.equal(displayUrl("file:///a/%zz"), "file:///a/%zz");
  assert.equal(displayUrl("file:///a/%0a"), "file:///a/%0a");
});

test("an existing journal directory with no store is the empty state, not 'cannot read'", () => {
  const s = toDeployState({ schema: 1, generatedMs: 1, runtimeDir: "/x", source: { kind: "own", path: "/x", journalFormat: "none", readOnly: false }, store: "missing", storeError: null, writerHeld: false, gate: GATE, repos: [], apps: [], definitions: [] }, { observed: {}, keychain: "unlocked" });
  assert.deepEqual([s.scenario, s.engine.store, s.engine.storeError], ["empty", "missing", null]);
});

import { allEventKeys, announceHeld, configValueProblem, newHeldEvents, suggestRepoName, summarizePoll, variableNameProblem } from "./deployEngine.ts";
import { heroClassFor } from "./deployPresentation.ts";

const A = "a".repeat(40), B = "b".repeat(40);
const ev = (delivery, revision, status = "held", source = "poll") => ({ delivery, source, revision, receivedMs: NOW - 5000, status, decidedMs: null, related: null });
const withEvents = (events, extra = {}) => snap([app([dep("dep_1")], { events })], extra);

test("variable names follow the engine's rules, and the reserved prefix is explained", () => {
  assert.equal(variableNameProblem(""), null);
  assert.equal(variableNameProblem("DATABASE_URL"), null);
  assert.equal(variableNameProblem("_private1"), null);
  assert.match(variableNameProblem("HARPOON_ANYTHING"), /reserved for Harpoon/);
  assert.match(variableNameProblem("HARPOON_DEPLOYMENT_MARKER"), /reserved/);
  assert.equal(variableNameProblem("HARPOONX"), null, "only the HARPOON_ prefix is reserved");
  assert.match(variableNameProblem("1ABC"), /start with a letter/);
  assert.match(variableNameProblem("MY-VAR"), /letters, digits and underscores/);
  assert.match(variableNameProblem("MY VAR"), /letters, digits and underscores/);
  assert.match(variableNameProblem("A".repeat(129)), /at most 128/);
  assert.equal(variableNameProblem("A".repeat(128)), null);
});

test("config values follow the engine's limits", () => {
  assert.equal(configValueProblem("ui-test"), null);
  assert.equal(configValueProblem(""), null);
  assert.match(configValueProblem("line1\nline2"), /control characters/);
  assert.match(configValueProblem("tab\there"), /control characters/);
  assert.match(configValueProblem("x".repeat(4097)), /at most 4096/);
  assert.equal(configValueProblem("x".repeat(4096)), null);
});

test("held events carry the full commit, status, source and time for the review dialog", () => {
  const s = st(withEvents([ev("d1", A), ev("d2", B, "selected", "github")]));
  const { held, selected } = s.apps.app_1.automation;
  assert.deepEqual(held.map((h) => [h.revisionFull, h.status, h.source, h.receivedMs]), [[A, "held", "poll", NOW - 5000]]);
  assert.deepEqual(selected.map((h) => [h.revisionFull, h.status, h.source, h.receivedMs]), [[B, "selected", "github", NOW - 5000]]);
  const decided = st(withEvents([ev("d1", A, "dismissed"), ev("d2", B, "executed")])).apps.app_1.automation;
  assert.deepEqual([decided.held, decided.selected], [[], []], "decided events are history, neither held nor selected");
});

test("a new held commit is announced once; opening the app and decided events are not", () => {
  // first snapshot only seeds: an event that was already there is not news
  let r = announceHeld(withEvents([ev("d1", A)]), null);
  assert.deepEqual(r.fresh, []);
  // the same snapshot again: still nothing
  r = announceHeld(withEvents([ev("d1", A)]), r.seen);
  assert.deepEqual(r.fresh, []);
  // a new commit lands
  r = announceHeld(withEvents([ev("d1", A), ev("d2", B)]), r.seen);
  assert.deepEqual(r.fresh.map((n) => [n.app, n.sha]), [["shop", B.slice(0, 7)]]);
  // and is not announced again on the next polls
  r = announceHeld(withEvents([ev("d1", A), ev("d2", B)]), r.seen);
  assert.deepEqual(r.fresh, []);
  // a decision on it is never news
  r = announceHeld(withEvents([ev("d1", A, "dismissed"), ev("d2", B, "selected")]), r.seen);
  assert.deepEqual(r.fresh, []);
});

test("announcements survive a store that starts out empty and an unreadable one", () => {
  let r = announceHeld(snap([], { store: "missing" }), null);
  assert.deepEqual([r.fresh, r.seen.size], [[], 0]);
  r = announceHeld(withEvents([ev("d1", A)]), r.seen); // the store appears with an event: that is news
  assert.equal(r.fresh.length, 1);
  const bad = announceHeld(snap([], { store: "error", storeError: "x" }), r.seen);
  assert.deepEqual(bad.fresh, []);
  assert.equal(bad.seen, r.seen, "an unreadable store forgets nothing");
  assert.deepEqual(newHeldEvents(st(withEvents([ev("d1", A)])), new Set(allEventKeys(withEvents([ev("d1", A)])))), []);
});

test("a manual check is summarised in plain words from the engine's own lines", () => {
  const queued = "watching 1 repository (one pass): …\nLATE: shop branch main moved aaa -> bbb while Harpoon was not running; queued as poll:shop:bbb\n";
  assert.match(summarizePoll(true, queued), /^1 new commit recorded as held event\. Nothing was deployed\./);
  assert.match(summarizePoll(true, queued + queued.split("\n")[1] + "\n"), /^2 new commits recorded as held events/);
  assert.match(summarizePoll(true, "shop branch main first seen at aaa (baseline; nothing is deployed for history that already existed)\n"), /Starting point recorded for 1 repository/);
  assert.equal(summarizePoll(true, "watching 1 repository (one pass)\n"), "No new commits since the last check.");
  assert.match(summarizePoll(false, ""), /did not complete/);
});

test("a repository name is suggested from the folder, in the characters the engine accepts", () => {
  assert.equal(suggestRepoName("/Users/juan/Harpoon UI Test"), "Harpoon-UI-Test");
  assert.equal(suggestRepoName("/Users/juan/code/shop/"), "shop");
  assert.equal(suggestRepoName("/Users/juan/.hidden repo"), "hidden-repo");
  assert.match(suggestRepoName("/a/café ☕"), /^caf-?$/);
  assert.equal(suggestRepoName("/"), "");
});

test("only something running gets the green badge; 'No release' and 'unknown' are neutral", () => {
  assert.equal(heroClassFor("running"), "");
  assert.equal(heroClassFor("building"), "");
  assert.equal(heroClassFor("idle"), "idle");
  assert.equal(heroClassFor("unknown"), "idle");
  assert.equal(heroClassFor("down"), "err");
  assert.equal(heroClassFor("recovering"), "info");
  // the real status of an application with a repository and no release is 'idle'
  const idle = st(snap([app([])])).apps.app_1;
  assert.equal(appStatus(idle, "connected").kind, "idle");
  assert.equal(heroClassFor(appStatus(idle, "connected").kind), "idle");
});

// ---- stable application address -------------------------------------------------------------------------------
import { addressOf } from "./deployEngine.ts";
const ROUTER = { domain: "harpoon.localhost", port: 7480, error: null };
const withHost = (route, extra = {}) => app([dep("d1")], { hostname: "abc124xyz0.harpoon.localhost", route, ...extra });

test("the address is the application's name plus the router port, and never contains a release's port", () => {
  const a = addressOf(withHost("serving"), ROUTER, true, false);
  assert.deepEqual([a.url, a.status], ["http://abc124xyz0.harpoon.localhost:7480", "running"]);
  assert.doesNotMatch(a.url, /41000|127\.0\.0\.1/);
});

test("the address is identical for release A and release B: only the serving release changes, not the name", () => {
  const serving = (rel, hostPort) => ({ release: rel, deployment: "d", container: "c", via: "original", sinceMs: NOW, hostPort, healthPath: "/healthz" });
  const urls = [["rel_a", 38001], ["rel_b", 38002]].map(([r, p]) => {
    const s = st(snap([withHost("serving", { serving: serving(r, p) })], { router: ROUTER }));
    return Object.values(s.apps)[0].address.url;
  });
  assert.equal(urls[0], urls[1]);
});

test("a stopped application keeps its address but says it is not responding", () => {
  const a = addressOf(withHost("serving"), ROUTER, false, false);
  assert.equal(a.status, "stopped");
  assert.match(a.note, /not observed running/);
  assert.ok(a.url);
});

test("nothing true to show means no address: idle, external, no router data or an old backend", () => {
  assert.equal(addressOf(withHost("idle"), ROUTER, false, false), undefined);
  assert.equal(addressOf(withHost("external"), ROUTER, true, true), undefined);
  assert.equal(addressOf(withHost("serving"), undefined, true, false), undefined);
  assert.equal(addressOf(app([dep("d1")]), ROUTER, true, false), undefined);
});

test("a collision or an unavailable router is reported, with no URL to copy or open", () => {
  const c = addressOf(withHost("conflict"), ROUTER, true, false);
  assert.deepEqual([c.status, c.url], ["conflict", ""]);
  const u = addressOf(withHost("serving"), { ...ROUTER, port: null, error: "cannot listen on 127.0.0.1:7480: Address already in use" }, true, false);
  assert.deepEqual([u.status, u.url], ["unavailable", ""]);
  assert.match(u.note, /already in use/);
});

test("a Mac that cannot resolve *.harpoon.localhost gets no broken URL, only the reason; unknown is not a claim", () => {
  const no = addressOf(withHost("serving"), { ...ROUTER, resolves: false, resolveError: "this Mac does not resolve probe.harpoon.localhost: nodename nor servname provided" }, true, false);
  assert.deepEqual([no.status, no.url], ["unresolvable", ""]);
  assert.match(no.note, /does not resolve/);
  assert.match(no.note, /direct address/);
  for (const resolves of [true, null, undefined]) assert.ok(addressOf(withHost("serving"), { ...ROUTER, resolves }, true, false).url);
});

test("the address note says it needs Harpoon open (the router lives in the desktop app)", () => {
  assert.match(addressOf(withHost("serving"), ROUTER, true, false).note, /Harpoon is open/);
  assert.match(addressOf(withHost("serving"), ROUTER, false, false).note, /needs Harpoon open/);
});

test("Copy and Open act on exactly the URL shown, and the primary view hides the internal port", () => {
  const src = readFileSync(new URL("../views/deploy/Screens.tsx", import.meta.url), "utf8");
  assert.match(src, /writeText\(a\.url\)/);
  assert.match(src, /openUrl\(a\.url\)/);
  assert.match(readFileSync(new URL("../App.tsx", import.meta.url), "utf8"), /invoke\("open_app_url", \{ url \}\)/);
  assert.match(src, /app\.address \? null : <>\{app\.url/, "the direct 127.0.0.1 URL is only the fallback when there is no address");
  assert.match(src, /Direct URL \(diagnostic\)/);
});

// ---- attended deployment: the confirmation flow (Milestone B gate opening)

import { CUTOVER_ACKNOWLEDGEMENT, deployConfirmation, failureText } from "./deployEngine.ts";

const read = (rel) => readFileSync(new URL(rel, import.meta.url), "utf8");

test("a first deployment confirms only the Docker changes; a replacement needs its own cutover acknowledgement", () => {
  assert.deepEqual(deployConfirmation({ replacing: false, cutoverAcknowledged: false }), { liveChanges: true, cutover: false });
  assert.deepEqual(deployConfirmation({ replacing: false, cutoverAcknowledged: true }), { liveChanges: true, cutover: false }, "nothing to switch on a first deployment");
  // a replacement without the acknowledgement may send nothing at all
  assert.equal(deployConfirmation({ replacing: true, cutoverAcknowledged: false }), null);
  assert.deepEqual(deployConfirmation({ replacing: true, cutoverAcknowledged: true }), { liveChanges: true, cutover: true });
  // "prepare only" builds and checks the candidate and never switches, so it needs no acknowledgement and never sends a cutover
  assert.deepEqual(deployConfirmation({ replacing: true, cutoverAcknowledged: false, prepareOnly: true }), { liveChanges: true, cutover: false });
  assert.deepEqual(deployConfirmation({ replacing: true, cutoverAcknowledged: true, prepareOnly: true }), { liveChanges: true, cutover: false });
  assert.match(CUTOVER_ACKNOWLEDGEMENT, /unavailable/);
});

test("a failed operation is shown in plain words, never as a bare exit code", () => {
  const r = (over) => ({ ok: false, exitCode: 6, summary: "this changes Docker state and needs the explicit opt-in --authorize-live-docker-changes=…", detail: "", refusedByGate: false, ...over });
  assert.equal(failureText(r({ hint: "Harpoon needs your confirmation before it changes Docker." })), "Harpoon needs your confirmation before it changes Docker.");
  assert.equal(failureText(r({ hint: "  " })), r({}).summary, "a blank hint falls back to the engine's own first line");
  assert.match(failureText(r({ summary: "", hint: null })), /Nothing was retried/);
  assert.doesNotMatch(failureText(r({ hint: "Harpoon refused this before changing anything: x" })), /exit code|exitCode/i);
});

test("with attended deployment open no gate banner shows; a closed gate still shows the engine's reason", () => {
  const open = st(snap([], { gate: { liveDeploy: true, liveReplacement: true, liveTriggered: false, reason: "closed" } }));
  assert.equal(liveBlock(open), null);
  assert.equal(liveBlock(st(snap([], { gate: { liveDeploy: true, liveReplacement: false, reason: "replacement closed" } }))), "replacement closed");
});

test("Cancel only closes: the shared dialog's Cancel never runs an operation", () => {
  const d = read("../views/deploy/Dialogs.tsx");
  assert.match(d, /<button className="btn btn--ghost" onClick=\{onClose\}>Cancel<\/button>/);
  // closing by the scrim or Escape is the same function
  assert.match(d, /onMouseDown=\{\(e\) => \{ if \(e\.target === e\.currentTarget\) onClose\(\); \}\}/);
});

test("only an explicit confirmation sends a confirmation, and a replacement needs the separate acknowledgement first", () => {
  const d = read("../views/deploy/Dialogs.tsx");
  // the confirmation constant for rollback and recovery exists exactly once
  assert.equal((d.match(/liveChanges: true/g) ?? []).length, 1, "liveChanges: true appears only in CONFIRM_LIVE");
  // every Docker-changing call in engine mode passes a confirmation as the third argument, and no other call does
  for (const op of ["rollback", "restore"]) assert.match(d, new RegExp(`act\\(\\{ op: "${op}"[^;]*CONFIRM_LIVE\\)`));
  assert.match(d, /act\(\{ op: "deploy", repo: app\.name, branch \}, goLogs, confirmation\)/);
  assert.doesNotMatch(d, /run\(\{ op: "(deploy|rollback|restore)"/, "no dialog runs a Docker-changing operation without going through act(op, after, confirmation)");
  // the replacement button cannot be pressed until the acknowledgement is ticked, and a null confirmation sends nothing
  assert.match(d, /disabled=\{engineBlocked \|\| !cutoverAck\}/);
  assert.match(d, /if \(!confirmation\) return;/);
  // the acknowledgement is per dialog and application: never carried over, never pre-ticked
  assert.match(d, /const ackKey = `\$\{dialog\.kind\}:\$\{app\.id\}`/);
  assert.match(d, /useState<string \| null>\(null\)/);
  // resume, poll and configuration dialogs never confirm Docker changes
  for (const op of ["resume", "poll", "set-config", "set-secret", "add-repo", "repo-secret"]) assert.doesNotMatch(d, new RegExp(`op: "${op}"[^;]*(liveChanges|CONFIRM_LIVE)`));
});

test("the engine hook sends the confirmation beside the operation and only when one was given; the phrase never exists in the front end", () => {
  const hook = read("../hooks/useDeployEngine.ts");
  assert.match(hook, /confirmation \? \{ op, confirmation \} : \{ op \}/);
  for (const f of ["../hooks/useDeployEngine.ts", "../lib/deployEngine.ts", "../views/deploy/Dialogs.tsx", "../views/deploy/DeployView.tsx", "../views/deploy/Screens.tsx"]) {
    assert.doesNotMatch(read(f), /authorize-live|I-UNDERSTAND/, `${f} must not hold the engine's authorization phrase`);
  }
});

test("a failed operation's banner uses the plain-language hint", () => {
  const v = read("../views/deploy/DeployView.tsx");
  assert.match(v, /failureText\(last\)/);
  assert.doesNotMatch(v, /Refused by the shipping gate/);
});

test("a test namespace is shown, comes only from the backend, and never changes what the front end sends", () => {
  const hook = read("../hooks/useDeployEngine.ts");
  assert.match(hook, /invoke<string \| null>\("test_namespace"\)/);
  const view = read("../views/deploy/DeployView.tsx");
  assert.match(view, /engine\?\.api\.testNamespace &&/);
  for (const f of ["../hooks/useDeployEngine.ts", "../lib/deployEngine.ts", "../views/deploy/Dialogs.tsx", "../views/deploy/DeployView.tsx"]) {
    assert.doesNotMatch(read(f), /HARPOON_TEST_TMPDIR/, `${f}: the front end never reads or sends the namespace variable`);
  }
});
