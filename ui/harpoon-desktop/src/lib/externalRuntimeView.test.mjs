// The Deployments view with a runtime attached read-only: the "Open runtime…" workflow, the badges, disabled actions,
// and the recorded-vs-observed panel, rendered by the real components.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test, { after } from "node:test";
import { act, click, createRoot, fakeEngine, find, h, mountView, PortsLine, shutdown, win } from "./viewHarness.mjs";
import { toDeployState } from "./deployEngine.ts";

after(() => shutdown());

const B7 = JSON.parse(readFileSync(new URL("../../src-tauri/tests/fixtures/b7-legacy-snapshot.json", import.meta.url), "utf8"));
const APP = "app_01M6DPQ543ASWBHW7PW17ZAJ3B", DEP = "dep_01M6DPQ543ASWBHW7PW17ZAJ3C", CTR = `hp-${DEP}`;
const CID = "2f70855fd334312f11ae3b2c5cb2ef42af49203820869637c71c02ead50ed936";
const PATH = "/Volumes/evidence/runtime";
const OWN = { schema: 1, generatedMs: B7.generatedMs, runtimeDir: "/own", source: { kind: "own", path: "/own", journalFormat: "none", readOnly: false }, store: "missing", storeError: null, writerHeld: false, gate: B7.gate, repos: [], apps: [], definitions: [] };
const LABELS = { "io.harpoon.managed": "true", "io.harpoon.app": APP, "io.harpoon.deployment": DEP };
const MAP = { hostIp: "0.0.0.0", hostPort: 38000, containerPort: 8080, proto: "tcp" };
const matching = { [CTR]: { id: CID, state: "running", labels: LABELS, ports: [38000], mappings: [MAP] } };
const stateOf = (observed, externals = [{ path: PATH, snapshot: B7, error: null }], hostListeners = null) => toDeployState(OWN, { observed, keychain: "unlocked", externals, hostListeners });
const engine = (state, calls, over) => fakeEngine(state, calls, over);

async function openExternalApp(m) {
  await click(find(m.host, "button.dp-card", "External"));
}

test("'Open runtime…' picks a folder, attaches it, and says why when it cannot", async () => {
  const picked = [];
  const m = await mountView((s, calls) => engine(s, calls, {
    pickFolder: async () => (picked.push(1), "/Volumes/evidence/runtime"),
    attachRuntime: async (p) => { calls.push({ attach: p }); return p.includes("evidence") ? { ok: true } : { ok: false, error: "nope" }; },
  }));
  await m.render(stateOf({}, []));
  const button = find(m.host, "button", "Open runtime…");
  assert.ok(button, "offered on the project list, also when Harpoon has no projects of its own");
  await click(button);
  assert.equal(picked.length, 1);
  assert.deepEqual(m.calls.at(-1), { attach: "/Volumes/evidence/runtime" });
  assert.equal(find(m.host, ".dp-banner", "cannot be opened"), undefined);
  await m.unmount();

  const bad = await mountView((s, calls) => engine(s, calls, { pickFolder: async () => "/somewhere/else", attachRuntime: async () => ({ ok: false, error: "No deploy-journal folder in /somewhere/else." }) }));
  await bad.render(stateOf({}, []));
  await click(find(bad.host, "button", "Open runtime…"));
  assert.match(find(bad.host, ".dp-banner", "cannot be opened").textContent, /No deploy-journal folder in \/somewhere\/else\./);
  await bad.unmount();

  const cancelled = await mountView((s, calls) => engine(s, calls, { pickFolder: async () => null }));
  await cancelled.render(stateOf({}, []));
  await click(find(cancelled.host, "button", "Open runtime…"));
  assert.equal(cancelled.calls.length, 0, "cancelling the picker attaches nothing");
  await cancelled.unmount();
});

test("an attached runtime is labelled 'External · read-only' and can be detached", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  assert.match(find(m.host, ".dp-h2", "External").textContent, /External · runtime/);
  assert.ok(find(m.host, ".dp-pill", "External · read-only"));
  assert.ok(find(m.host, "button.dp-card", "External · read-only"), "the application card carries the badge too");
  assert.match(m.host.textContent, /legacy journal\.wal, not migrated/);
  const detach = find(m.host, "button", "Detach runtime");
  assert.match(detach.title, /only this reference.*untouched/);
  assert.equal(find(m.host, "button", /^Close$/), undefined, "nothing here offers to 'close' the folder");
  await click(detach);
  assert.deepEqual(m.calls.at(-1), { detach: PATH });
  await m.unmount();
});

test("an attached runtime that cannot be read is shown as unavailable, kept, and can be retried or detached", async () => {
  const m = await mountView(engine);
  await m.render(stateOf({}, [{ path: "/gone", snapshot: null, error: "Cannot read /gone: No such file or directory (os error 2)" }]));
  const banner = find(m.host, ".dp-banner", "is unavailable");
  assert.match(banner.textContent, /\/gone/);
  assert.match(banner.textContent, /Cannot read \/gone/);
  assert.match(banner.textContent, /keeps the reference and reads the folder again when it is back/);
  await click(find(m.host, ".dp-banner button", "Retry"));
  await click(find(m.host, ".dp-banner button", "Detach runtime"));
  assert.deepEqual(m.calls.at(-1), { detach: "/gone" });
  await m.unmount();
});

test("the external application says what it is, offers no action, and shows recorded vs observed", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  const banner = find(m.host, ".dp-banner", "External · read-only");
  assert.match(banner.textContent, /\/Volumes\/evidence\/runtime/);
  assert.match(banner.textContent, /read where it is and not migrated/);
  assert.match(banner.textContent, /cannot change it/);
  for (const label of ["Deploy", "Redeploy", "Roll back…", "Promote…"]) {
    const b = find(m.host, ".dp-actions button", label);
    assert.ok(b, label);
    assert.equal(b.disabled, true, `${label} is disabled`);
    assert.match(b.title, /attached read-only/, `${label} says why`);
  }
  for (const gone of [/Check for new commits/, /Review held events/, /Review selected commit/]) assert.equal(find(m.host, "button", gone), undefined, String(gone));
  const panel = find(m.host, "section.dp-panel", "Recorded vs observed");
  assert.match(panel.textContent, /Matches Docker/);
  assert.match(panel.textContent, /Recorded.*release rel_01M6DPQ543ASWBHW7PW17ZAJ3C serving in container hp-dep_01M6DPQ543ASWBHW7PW17ZAJ3C \(2f70855fd334\), port lease 38000/);
  assert.match(panel.textContent, /Observed.*container 2f70855fd334 running/);
  assert.match(panel.textContent, /http:\/\/127\.0\.0\.1:38000.*from the recorded port lease/);
  await m.unmount();
});

test("when Docker disagrees with the record the page says so instead of showing the release as running", async () => {
  for (const [name, observed, verdict, note] of [
    ["absent", {}, "Recorded, not in Docker", /no such container/],
    ["another deployment's container", { [CTR]: { id: CID, state: "running", labels: { ...LABELS, "io.harpoon.deployment": "dep_OTHER" }, ports: [38000], mappings: [MAP] } }, "Does not match Docker", /dep_OTHER/],
    ["stopped", { [CTR]: { id: CID, state: "exited", labels: LABELS, ports: [], mappings: [] } }, "Recorded, not running", /exited/],
  ]) {
    const m = await mountView(engine);
    await m.render(stateOf(observed));
    await openExternalApp(m);
    const panel = find(m.host, "section.dp-panel", "Recorded vs observed");
    assert.match(panel.textContent, new RegExp(verdict), name);
    assert.match(panel.textContent, note, name);
    assert.doesNotMatch(panel.textContent, /Loopback URL/, `${name}: no URL for something not confirmed`);
    assert.ok(!find(m.host, ".dp-hero .dp-pill", "Running"), `${name}: not shown as running`);
    await m.unmount();
  }
});

test("variables of an attached runtime cannot be added or edited", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  await click(find(m.host, '[role="tab"]', "Variables"));
  const add = find(m.host, "button", "Add variable");
  assert.equal(add.disabled, true);
  assert.match(add.title, /attached read-only/);
  await click(add);
  assert.equal(m.host.querySelector('[role="dialog"]'), null, "no dialog opens");
  await m.unmount();
});

test("an external deployment shows what the ledger recorded, with ids, and asks the store for no logs", async () => {
  let asked = 0;
  const m = await mountView((s, calls) => engine(s, calls, { stepLogs: async () => (asked++, []) }));
  await m.render(stateOf(matching));
  await openExternalApp(m);
  await click(find(m.host, '[role="tab"]', /Deployments/));
  await click(find(m.host, "tr.is-click", DEP));
  const docker = find(m.host, "section.dp-panel", "Docker objects");
  assert.match(docker.textContent, /container.*hp-dep_01M6DPQ543ASWBHW7PW17ZAJ3C.*2f70855fd334.*created/);
  assert.match(docker.textContent, /port.*38000.*created/);
  assert.match(docker.textContent, /port.*39000.*released/);
  for (const label of [/Retry deployment/, /Roll back to/, /Redeploy this revision/]) {
    const b = find(m.host, ".dp-actions button", label);
    if (b) assert.equal(b.disabled, true, String(label));
  }
  assert.equal(asked, 0, "Harpoon's own step-log store is never asked about another runtime's deployment");
  await m.unmount();
});


// ---- the Configuration panel and the Settings tab: recorded facts only --------------------------------------------------------

const LOOPBACK = { verified: true, error: null, checkedMs: 1, listeners: [{ port: 38000, address: "127.0.0.1:38000", scope: "loopback", pid: 67874, command: "harpoon" }] };
const dd = (host, label) => [...host.querySelectorAll(".dp-kv dt")].find((e) => e.textContent === label)?.nextElementSibling;

test("the Configuration panel shows only what was recorded, each value with its origin, and 'Not recorded' otherwise", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  const panel = find(m.host, "section.dp-panel", "Configuration");
  const row = (label) => dd(panel, label).textContent.replace(/\s+/g, " ").trim();
  assert.equal(row("Source"), "Not recorded");
  assert.equal(row("Build file"), "Not recorded");
  assert.equal(row("Health check"), "Not recorded");
  assert.equal(row("Container port"), "8080 · observed in Docker, not in the journal (Docker maps 38000→8080; the journal does not record it)");
  assert.equal(row("Host port"), "38000 · from the journal (recorded port lease)");
  assert.equal(row("Replacement"), "Nothing was replaced (first deployment) · from the journal (cutover not required)");
  assert.match(row("Image"), /^harpoon\/app_01m6dpq543aswbhw7pw17zaj3b:dep_01M6DPQ543….*image 02693c38b1e5 · from the journal/);
  // the values the old panel invented
  for (const invented of [/folder · app_/, /HTTP GET \//, /stop old → start new/, /Dockerfile\s/]) assert.doesNotMatch(panel.textContent, invented, String(invented));
  await m.unmount();
});

test("with Docker's answer missing, the container port is 'Not recorded' rather than guessed", async () => {
  const m = await mountView(engine);
  await m.render(stateOf({}));
  await openExternalApp(m);
  assert.equal(dd(find(m.host, "section.dp-panel", "Configuration"), "Container port").textContent.trim(), "Not recorded");
  await m.unmount();
});

test("an external deployment offers no Edit, no settings link and no other way to change configuration", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  const head = find(m.host, "section.dp-panel", "Configuration").querySelector(".dp-panel__head");
  assert.equal(head.querySelector("button"), null, "no button at all in the Configuration panel");
  assert.equal(find(m.host, "button", /^Edit$/), undefined);
  await m.unmount();
});

test("on every tab of an external application no control that could change anything is enabled", async () => {
  const MUTATING = /^(Edit|Replace|Add variable|Deploy|Redeploy|Roll back|Promote|Retry|Check for new commits|Review|Dismiss all|Select this revision|Save|Remove|Resume|Create project|Add repository)/;
  const m = await mountView(engine);
  await m.render(stateOf(matching, undefined, LOOPBACK));
  await openExternalApp(m);
  for (const tab of ["Overview", "Deployments", "Logs", "Metrics", "Variables", "Settings"]) {
    await click(find(m.host, '[role="tab"]', tab));
    const live = [...m.host.querySelectorAll('button:not(:disabled):not([role="tab"])')].map((b) => b.textContent.replace(/\s+/g, " ").trim()).filter((t) => MUTATING.test(t));
    assert.deepEqual(live, [], `${tab}: enabled controls that could change something`);
  }
  // the deployment page too
  await click(find(m.host, '[role="tab"]', /Deployments/));
  await click(find(m.host, "tr.is-click", DEP));
  const live = [...m.host.querySelectorAll('button:not(:disabled):not([role="tab"])')].map((b) => b.textContent.replace(/\s+/g, " ").trim()).filter((t) => MUTATING.test(t) || /Retry|Roll back|Redeploy/.test(t));
  assert.deepEqual(live, [], "deployment page");
  assert.equal(m.calls.filter((c) => c.op).length, 0, "and nothing was sent to the engine");
  await m.unmount();
});

test("the Settings tab of an external application shows recorded facts and none of the invented text", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  await click(find(m.host, '[role="tab"]', "Settings"));
  const body = m.host.textContent;
  for (const invented of [/Detected: Dockerfile/, /Published on a loopback port chosen by Harpoon/, /Stop old, start new/, /Remove application/, /\(default\)/]) assert.doesNotMatch(body, invented, String(invented));
  const input = (label) => [...m.host.querySelectorAll(".dp-field")].find((f) => f.querySelector("label")?.textContent === label)?.querySelector("input");
  assert.equal(input("Dockerfile").value, "Not recorded");
  assert.equal(input("Health check").value, "Not recorded");
  assert.equal(input("Host port").value, "38000");
  assert.equal(input("Container port").value, "8080");
  assert.match(body, /Read-only\. These are the values that the external journal/);
  await m.unmount();
});

// ---- recorded vs observed: exposure and ownership -----------------------------------------------------------------------------

test("exposure: Docker's wildcard mapping and the verified loopback listener are shown side by side", async () => {
  const wild = { [CTR]: { id: CID, state: "running", labels: LABELS, ports: [38000], mappings: [MAP, { ...MAP, hostIp: "[::]" }] } };
  const m = await mountView(engine);
  await m.render(stateOf(wild, undefined, LOOPBACK));
  await openExternalApp(m);
  const exp = dd(find(m.host, "section.dp-panel", "Recorded vs observed"), "Exposure").textContent.replace(/\s+/g, " ");
  assert.match(exp, /Docker reports 0\.0\.0\.0:38000→8080\/tcp, \[::\]:38000→8080\/tcp \(a mapping inside Harpoon's VM\)/);
  assert.match(exp, /Host listener 127\.0\.0\.1:38000 · loopback only, reachable from this Mac only · harpoon \(pid 67874\)/);
  await m.unmount();

  const none = await mountView(engine);
  await none.render(stateOf(wild, undefined, { verified: false, error: "lsof did not finish in 5 s", checkedMs: 1, listeners: [] }));
  await openExternalApp(none);
  const unv = dd(find(none.host, "section.dp-panel", "Recorded vs observed"), "Exposure").textContent.replace(/\s+/g, " ");
  assert.match(unv, /Host listener not verified: lsof did not finish in 5 s/);
  assert.doesNotMatch(unv, /loopback only|reachable from/, "no exposure is claimed when it could not be verified");
  assert.match(unv, /Docker reports 0\.0\.0\.0:38000/, "Docker's own report is still shown");
  await none.unmount();
});

test("a running container without ownership labels is shown as unverified, in the status and the panel", async () => {
  const m = await mountView(engine);
  await m.render(stateOf({ [CTR]: { id: CID, state: "running", labels: null, ports: [38000], mappings: [MAP] } }));
  await openExternalApp(m);
  assert.ok(find(m.host, ".dp-hero .dp-pill", "Running · ownership unverified"));
  assert.equal(find(m.host, ".dp-hero .dp-pill", /^Running$/), undefined, "not the plain, fully verified 'Running'");
  const panel = find(m.host, "section.dp-panel", "Recorded vs observed");
  assert.match(panel.textContent, /Running, ownership unchecked/);
  const checks = [...panel.querySelectorAll(".dp-checks li")].map((li) => [li.dataset.result, li.textContent.replace(/\s+/g, " ").trim()]);
  assert.deepEqual(checks.filter(([r]) => r === "unavailable").map(([, t]) => t.match(/Managed by Harpoon|Belongs to this application|Created by this deployment/)?.[0]), ["Managed by Harpoon", "Belongs to this application", "Created by this deployment"]);
  assert.ok(checks.every(([r, t]) => r !== "unavailable" || /not checked/.test(t)));
  await m.unmount();
});

test("the panel lists what was compared, and each failed check by name", async () => {
  const m = await mountView(engine);
  await m.render(stateOf({ [CTR]: { id: CID, state: "running", labels: { ...LABELS, "io.harpoon.app": "app_OTHER" }, ports: [38000], mappings: [MAP] } }));
  await openExternalApp(m);
  const lis = [...m.host.querySelectorAll(".dp-checks li")];
  assert.deepEqual(lis.map((li) => li.dataset.result), ["ok", "ok", "ok", "fail", "ok", "ok"]);
  assert.match(lis[3].textContent, /Belongs to this application.*io\.harpoon\.app label is "app_OTHER", not app_01M6DPQ543ASWBHW7PW17ZAJ3B/);
  assert.match(find(m.host, "section.dp-panel", "Recorded vs observed").textContent, /from the legacy journal\.wal: what was done, not what is true now/);
  await m.unmount();
});

// ---- long identifiers: tooltips and copy ---------------------------------------------------------------------------------------

test("a long identifier keeps its complete value in a tooltip and can be copied", async () => {
  const copied = [];
  Object.defineProperty(win.navigator, "clipboard", { value: { writeText: async (t) => copied.push(t) }, configurable: true });
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  await click(find(m.host, '[role="tab"]', /Deployments/));
  const idCell = find(m.host, "td b", DEP);
  assert.equal(idCell.title, DEP, "the deployment id's tooltip is the whole id");
  await click(find(m.host, "tr.is-click", DEP));
  const copy = m.host.querySelector('.dp-id__copy[aria-label^="Copy sha256:02693c38b1e5"], .dp-id__copy[aria-label^="Copy 2f70855f"]');
  assert.ok(copy, "recorded object ids have a copy button");
  const full = copy.getAttribute("aria-label").replace(/^Copy /, "");
  assert.ok(full.length >= 64, "the button carries the complete id, not the shortened one shown");
  await click(copy);
  assert.deepEqual(copied, [full]);
  const shown = copy.parentElement.querySelector(".dp-id__text");
  assert.equal(shown.title, full);
  assert.ok(shown.textContent.length < full.length, "what is shown is shortened");
  await m.unmount();
});

test("the ports line keeps Docker's mapping and the Mac-side listener on separate lines", async () => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(h(PortsLine, { ports: "0.0.0.0:38000->8080/tcp, [::]:38000->8080/tcp", listeners: LOOPBACK })));
  const spans = [...host.querySelectorAll(".port-exposure > span")].map((x) => x.textContent);
  assert.deepEqual(spans, ["Docker: 0.0.0.0:38000→8080 · [::]:38000→8080", "Mac listener: 127.0.0.1:38000 · loopback only"]);
  assert.match(host.querySelector(".port-exposure").title, /Docker reports .*\(a mapping inside Harpoon's VM\)\nHost listener 127\.0\.0\.1:38000/);
  await act(async () => root.render(h(PortsLine, { ports: "0.0.0.0:38000->8080/tcp", listeners: undefined })));
  assert.equal(host.querySelector(".port-exposure__host").textContent, "Mac listener: not verified");
  await act(async () => root.render(h(PortsLine, { ports: "", listeners: undefined })));
  assert.equal(host.textContent, "No published ports");
  await act(async () => root.unmount());
});

test("Resources & health do not call a running container 'no running container' or its recorded check 'healthy'", async () => {
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  const panel = find(m.host, "section.dp-panel", "Resources & health");
  assert.doesNotMatch(panel.textContent, /No running container/);
  assert.match(panel.textContent, /Per-container resource figures are not collected on this screen/);
  assert.ok(find(panel, ".dp-pill", "Final check passed (recorded)"));
  assert.equal(find(panel, ".dp-pill", /healthy/i), undefined, "a deploy-time check is not presented as current health");
  await m.unmount();
  const gone = await mountView(engine);
  await gone.render(stateOf({}));
  await openExternalApp(gone);
  assert.match(find(gone.host, "section.dp-panel", "Resources & health").textContent, /No running container/, "and when there really is none, it still says so");
  await gone.unmount();
});

test("a restored attachment is shown without opening it again, and an unreadable list is reported", async () => {
  const m = await mountView(engine);
  // the state comes straight from the remembered references: no pickFolder / attachRuntime call is involved
  await m.render(stateOf(matching));
  assert.ok(find(m.host, ".dp-pill", "External · read-only"));
  assert.deepEqual(m.calls, []);
  await m.render(stateOf({}, []), { engine: { ...engine(stateOf({}, []), m.calls), api: { ...engine(stateOf({}, []), m.calls).api, externalsError: "The list of attached runtimes is damaged. It was left as it is." } } });
  assert.match(find(m.host, ".dp-banner", "list of attached runtimes cannot be read").textContent, /left as it is/);
  await m.unmount();
});

test("a long image name is readable in the Configuration panel and its full reference stays reachable", async () => {
  const copied = [];
  Object.defineProperty(win.navigator, "clipboard", { value: { writeText: async (t) => copied.push(t) }, configurable: true });
  const m = await mountView(engine);
  await m.render(stateOf(matching));
  await openExternalApp(m);
  const row = [...m.host.querySelectorAll("dt")].find((d) => d.textContent === "Image").nextElementSibling;
  const image = row.querySelector(".dp-image");
  const full = image.querySelector(".dp-id__text").title;
  assert.match(full, /^harpoon\/.+:dep_01M6DPQ543ASWBHW7PW17ZAJ3C-[0-9a-f]+ \(02693c38b1e5\)$/, "the tooltip carries the whole recorded reference");
  assert.match(image.querySelector(".dp-image__tag").textContent, /…/, "the long tag is shortened in the middle");
  assert.ok(image.querySelector(".dp-image__tag").title.startsWith(DEP) && full.includes(image.querySelector(".dp-image__tag").title), "the complete tag is in its tooltip");
  assert.match(image.querySelector(".dp-image__id").textContent, /^image [0-9a-f]{12}$/, "the id is on its own line");
  await click(image.querySelector(".dp-id__copy"));
  assert.deepEqual(copied, [full]);
  await m.unmount();
});
