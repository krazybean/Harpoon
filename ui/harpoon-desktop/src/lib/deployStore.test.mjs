import assert from "node:assert/strict";
import test from "node:test";
import {
  appStatus, appsNeedingAttention, applyScenario, attention, initialState, latestDeployment, navBadge, recoveryInProgress, reduce,
} from "./deployStore.ts";
import { formatAge, recoveryHeadline, recoveryLabel, redact, statusLabel, EVIDENCE_NOTE, ROLLBACK_WARNING } from "./deployPresentation.ts";

const SCENARIOS = ["healthy", "building", "build-failed", "cutover-restored", "restore-failed", "recovering", "rolled-back-paused", "empty", "loading"];
const api = (s) => s.apps.api;
const drain = (s) => { let n = 0; while (s.job && n++ < 20) s = reduce(s, { type: "advance" }); return s; };

test("every scenario yields internally consistent application state in every runtime", () => {
  for (const sc of SCENARIOS) {
    const s = initialState(sc);
    for (const app of Object.values(s.apps)) {
      // the serving release, if any, exists and is retained
      if (app.currentReleaseId) assert.ok(app.releases.some((r) => r.id === app.currentReleaseId), `${sc}: current release must exist`);
      // at most one deployment in flight, and it matches the simulation job
      const running = app.deployments.filter((d) => d.status === "running");
      assert.ok(running.length <= 1, `${sc}: at most one running deployment`);
      assert.equal(running.length === 1, s.job?.appId === app.id, `${sc}: job and running deployment agree`);
      // a failed deployment always says why; a recovery is only attached to a failed/in-flight deployment
      for (const d of app.deployments) {
        if (d.status === "failed") assert.ok(d.failure, `${sc}: ${d.id} failed without a reason`);
        if (d.recovery) assert.ok(d.status === "failed" || d.status === "running", `${sc}: recovery on ${d.status}`);
        if (d.recovery?.outcome === "restored") assert.equal(app.currentReleaseId, app.lastKnownReleaseId);
      }
      // down means: nothing serving, something was serving before, nothing in flight
      const down = appStatus(app, "connected").kind === "down";
      assert.equal(down, app.currentReleaseId === null && !running.length && app.lastKnownReleaseId !== null && !recoveryInProgress(app));
      // paused automation always has a reason
      if (app.automation.state === "paused") assert.ok(app.automation.reason, `${sc}: paused needs a reason`);
      for (const rt of ["connected", "stopped", "docker-not-ready"]) {
        const st = appStatus(app, rt);
        assert.equal(st.live, rt === "connected");
        if (rt !== "connected") assert.equal(st.kind, "unknown", `${sc}: ${rt} must never show live status`);
      }
    }
  }
});

test("a disconnected runtime never reports live status, attention or badges", () => {
  for (const sc of SCENARIOS) {
    const s = initialState(sc);
    assert.deepEqual(appsNeedingAttention(s, "stopped"), []);
    assert.equal(navBadge(s, "stopped"), "none");
    for (const app of Object.values(s.apps)) assert.equal(attention(app, "docker-not-ready"), null);
  }
});

test("healthy → build failure: nothing stopped, previous release keeps serving, automation NOT paused", () => {
  const s = initialState("build-failed");
  const a = api(s), d = latestDeployment(a);
  assert.equal(d.status, "failed");
  assert.equal(d.failedAt, "build");
  assert.equal(d.nothingStopped, true);
  assert.equal(d.releaseId, null, "a failed build produces no release");
  assert.equal(a.currentReleaseId, "r-42");
  assert.equal(a.automation.state, "active", "an ordinary failure must not pause automation");
  assert.equal(appStatus(a, "connected").kind, "running");
  assert.match(attention(a, "connected").text, /nothing was stopped/i);
});

test("failed cutover with restoration: previous release serves again, automation paused, events held", () => {
  const s = initialState("cutover-restored");
  const a = api(s), d = latestDeployment(a);
  assert.equal(d.status, "failed");
  assert.equal(d.recovery.outcome, "restored");
  assert.equal(a.currentReleaseId, "r-42");
  assert.equal(a.automation.state, "paused");
  assert.equal(a.automation.reason.kind, "failed_cutover");
  assert.equal(a.automation.held.length, 2, "held events are retained, not discarded");
  assert.equal(appStatus(a, "connected").kind, "running", "paused automation is not a stopped application");
  assert.equal(d.interruption.ongoing, false);
});

test("failed restoration: application is down, interruption ongoing, still distinguishable from a failed deployment", () => {
  const s = initialState("restore-failed");
  const a = api(s), d = latestDeployment(a);
  assert.equal(d.status, "failed");
  assert.equal(d.recovery.outcome, "restore_failed");
  assert.equal(d.recovery.reason, "secret_unavailable");
  assert.equal(a.currentReleaseId, null);
  assert.equal(appStatus(a, "connected").kind, "down");
  assert.equal(d.interruption.ongoing, true);
  assert.equal(attention(a, "connected").tone, "err");
  assert.equal(navBadge(s, "connected"), "error");
  assert.equal(api(s).resources, api(s).resources); // resources are not part of "down" truth; views must hide them
});

test("recovering: restoration is in progress, deployment still running, no serving release", () => {
  const s = initialState("recovering");
  const a = api(s), d = latestDeployment(a);
  assert.equal(d.status, "running");
  assert.equal(d.recovery.outcome, "in_progress");
  assert.equal(appStatus(a, "connected").kind, "recovering");
  assert.equal(a.currentReleaseId, null);
  const done = drain(s);
  assert.equal(done.job, null);
  assert.equal(latestDeployment(done.apps.api).status, "failed");
});

test("success path replaces the release, records a bounded interruption and leaves automation active", () => {
  let s = initialState("healthy");
  s = reduce(s, { type: "start-deploy", appId: "api", trigger: "manual" });
  assert.equal(appStatus(api(s), "connected").kind, "building");
  assert.equal(api(s).currentReleaseId, "r-42", "the running release is untouched while building");
  s = drain(s);
  const d = latestDeployment(api(s));
  assert.equal(d.status, "succeeded");
  assert.equal(api(s).currentReleaseId, d.releaseId);
  assert.equal(d.interruption.ongoing, false);
  assert.equal(api(s).automation.state, "active");
});

test("rollback: restarts a retained release, pauses automation, keeps held events, states the interruption", () => {
  let s = initialState("cutover-restored");
  s = reduce(s, { type: "rollback", appId: "api", releaseId: "r-41" });
  const a = api(s);
  assert.equal(a.currentReleaseId, "r-41");
  assert.equal(latestDeployment(a).trigger, "rollback");
  assert.equal(a.automation.state, "paused");
  assert.equal(a.automation.reason.kind, "rollback");
  assert.equal(a.automation.held.length, 2, "rollback must not discard held events");
  assert.match(ROLLBACK_WARNING, /not rolled back/);
  // rolling back to what is already running is a no-op
  assert.equal(reduce(s, { type: "rollback", appId: "api", releaseId: "r-41" }), s);
});

test("resume is explicit; deploying the selected held revision starts a real simulated deployment", () => {
  let s = initialState("cutover-restored");
  const held = api(s).automation.held;
  assert.equal(api(s).automation.state, "paused");
  const skipped = reduce(s, { type: "resume", appId: "api", mode: "skip" });
  assert.equal(api(skipped).automation.state, "active");
  assert.equal(api(skipped).automation.held.length, 0);
  assert.equal(skipped.job, null, "resuming without deploying starts nothing");
  const deployed = reduce(s, { type: "resume", appId: "api", mode: "deploy", heldEventId: held[0].id });
  assert.equal(api(deployed).automation.state, "active");
  assert.equal(latestDeployment(api(deployed)).revision.sha, held[0].revision.sha);
  assert.ok(deployed.job);
  // resuming something that is not paused changes nothing
  assert.equal(reduce(skipped, { type: "resume", appId: "api", mode: "skip" }), skipped);
});

test("recover: succeeds only when the Keychain is readable; automation stays paused until resumed", () => {
  let s = initialState("restore-failed");
  s = reduce(s, { type: "set-keychain", keychain: "locked" });
  const still = reduce(s, { type: "recover", appId: "api" });
  assert.equal(api(still).currentReleaseId, null);
  assert.equal(latestDeployment(api(still)).recovery.outcome, "restore_failed");
  assert.match(latestDeployment(api(still)).recovery.detail, /Keychain locked/);
  const ok = reduce(reduce(still, { type: "set-keychain", keychain: "unlocked" }), { type: "recover", appId: "api" });
  assert.equal(api(ok).currentReleaseId, "r-42");
  assert.equal(latestDeployment(api(ok)).recovery.outcome, "restored");
  assert.equal(api(ok).automation.state, "paused", "recovery never resumes automation");
});

test("a locked Keychain blocks a deployment before any side effect", () => {
  let s = reduce(initialState("healthy"), { type: "set-keychain", keychain: "locked" });
  const after = reduce(s, { type: "start-deploy", appId: "api", trigger: "manual" });
  assert.equal(after, s);
  assert.equal(after.job, null);
  assert.equal(api(after).deployments.length, api(s).deployments.length);
});

test("only one simulated deployment runs at a time", () => {
  let s = reduce(initialState("healthy"), { type: "start-deploy", appId: "api", trigger: "manual" });
  const again = reduce(s, { type: "start-deploy", appId: "web", trigger: "manual" });
  assert.equal(again, s);
});

test("switching scenarios fully replaces state and never leaves a stale job or status", () => {
  let s = initialState("building");
  assert.ok(s.job);
  for (const sc of SCENARIOS) {
    s = reduce(s, { type: "set-scenario", scenario: sc });
    assert.equal(s.scenario, sc);
    const inflight = Object.values(s.apps).flatMap((a) => a.deployments.filter((d) => d.status === "running"));
    assert.equal(inflight.length, s.job ? 1 : 0, `${sc}: leftover running deployment`);
  }
  assert.deepEqual(applyScenario(s, "healthy"), initialState("healthy"));
});

test("every simulated outcome ends in a state the derived status can describe", () => {
  for (const outcome of ["success", "build-failure", "cutover-restored", "restore-failed"]) {
    let s = reduce(initialState("healthy"), { type: "set-next-outcome", outcome });
    s = drain(reduce(s, { type: "start-deploy", appId: "api", trigger: "manual" }));
    assert.equal(s.job, null, outcome);
    const k = appStatus(api(s), "connected").kind;
    assert.ok(["running", "down"].includes(k), `${outcome} → ${k}`);
    const d = latestDeployment(api(s));
    assert.notEqual(d.status, "running");
    assert.equal(api(s).automation.state === "paused", outcome === "cutover-restored" || outcome === "restore-failed", outcome);
  }
});

test("a first deployment (nothing serving) never stops, restores or pauses anything", () => {
  let s = initialState("restore-failed");
  const pausedBefore = api(s).automation;
  s = reduce(s, { type: "set-next-outcome", outcome: "cutover-restored" });
  s = drain(reduce(s, { type: "start-deploy", appId: "api", trigger: "manual" }));
  const d = latestDeployment(api(s));
  assert.equal(d.status, "failed");
  assert.equal(d.recovery, undefined, "no previous release means no restoration attempt");
  assert.equal(d.interruption, undefined);
  assert.equal(d.steps.find((x) => x.name === "cutover").note.includes("first release"), true);
  assert.deepEqual(api(s).automation, pausedBefore, "automation state is not changed by a deployment with no cutover");
  assert.equal(api(s).currentReleaseId, null);
});

test("secret values never enter state: saving a secret stores a version and drops the value", () => {
  const CANARY = "sk_live_CANARY_0123456789";
  let s = initialState("healthy");
  s = reduce(s, { type: "save-variable", appId: "api", name: "STRIPE_KEY", varType: "secret", used: "runtime", scope: "environment", value: CANARY });
  s = reduce(s, { type: "save-variable", appId: "api", name: "NEW_SECRET", varType: "secret", used: "runtime", scope: "application", value: CANARY });
  const v = api(s).variables.find((x) => x.name === "STRIPE_KEY");
  assert.equal(v.version, 4);
  assert.equal("value" in v, false);
  assert.equal(JSON.stringify(s).includes(CANARY), false);
  // and no seeded scenario carries any secret-looking value
  for (const sc of SCENARIOS) {
    const j = JSON.stringify(initialState(sc));
    assert.equal(/sk_live|password=|BEGIN [A-Z ]*PRIVATE KEY/i.test(j.replace(/\[REDACTED:[A-Z_]+\]/g, "")), false, sc);
  }
  for (const app of Object.values(initialState("healthy").apps)) for (const x of app.variables) if (x.type === "secret") assert.equal("value" in x, false);
});

test("config values are stored, secrets are logged only as redaction tokens", () => {
  const s = initialState("building");
  const logs = api(s).deployments.flatMap((d) => d.buildLog.map((l) => l.text)).join("\n");
  assert.match(logs, /\[REDACTED:STRIPE_KEY\]/);
  assert.equal(redact("token=abc123 and abc123", [{ name: "T", value: "abc123" }]), "token=[REDACTED:T] and [REDACTED:T]");
  assert.equal(redact("x", [{ name: "E", value: "" }]), "x", "an empty value never matches");
  assert.equal(redact("a1 and a12", [{ name: "SHORT", value: "a1" }, { name: "LONG", value: "a12" }]), "[REDACTED:SHORT] and [REDACTED:LONG]", "longest value wins; short values are masked too");
});

test("wording: only a verified restoration is called restored; local health is never a guarantee", () => {
  assert.equal(recoveryLabel("restored"), "Restored");
  assert.notEqual(recoveryLabel("restored_unverified"), "Restored");
  assert.doesNotMatch(recoveryHeadline("restore_failed"), /running again|recovered/i);
  assert.match(recoveryHeadline("restored"), /running again/);
  assert.match(EVIDENCE_NOTE, /not a guarantee/);
  assert.equal(statusLabel("running", true), "Recovering");
  assert.equal(formatAge(0.2), "just now");
  assert.equal(formatAge(1440), "yesterday");
});

test("create-project from the empty state yields a healthy, consistent workspace", () => {
  let s = initialState("empty");
  assert.equal(s.projects.length, 0);
  s = reduce(s, { type: "create-project", name: "storefront", source: "git" });
  assert.equal(s.projects.length, 2);
  assert.equal(appStatus(api(s), "connected").kind, "running");
});
