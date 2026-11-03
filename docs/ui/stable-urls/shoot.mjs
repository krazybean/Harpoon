// Screenshots of the real (engine-mode) Deployments UI against FIXTURE snapshots: Tauri's IPC is replaced by a stub in
// the page, so no Harpoon engine, Docker or journal is contacted. Requires the Vite dev server on :1420 and Chrome.
//   node docs/ui/stable-urls/shoot.mjs
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";
import { STUB_SOURCE, fixtureUrl } from "./stub.mjs";

const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const OUT = new URL("./shots/", import.meta.url).pathname;
mkdirSync(OUT, { recursive: true });
const NOW = Date.now();
const APP = "app_01KJ3WQ7V9D2M5N8R4T6XABC12";
const HOST = `${APP.split("_")[1].slice(-10).toLowerCase()}.harpoon.localhost`;
const DEP = (n) => `dep_01KJ3WQ7V9D2M5N8R4T6XD000${n}`;
const step = (name) => ({ name, status: "succeeded", startedMs: NOW - 90000, finishedMs: NOW - 80000, note: "", log: false });
const dep = (n, port) => ({
  id: DEP(n), resources: [], status: "succeeded", trigger: "cli", createdMs: NOW - 600000 + n * 1000, startedMs: NOW - 599000, finishedMs: NOW - 590000 + n * 1000,
  steps: ["source", "build", "preliminary", "cutover", "final"].map(step), cutover: "confirmed", cutoverAtMs: NOW - 591000, failure: null, recovery: null, interruption: null,
  release: `rel_01KJ3WQ7V9D2M5N8R4T6XD000${n}`, replaces: null, rollbackTo: null, mayBeDown: false, unfinished: false,
  source: { repo: "shop", url: "https://example.com/shop.git", selector: "branch", requested: "main", commit: "0123456789abcdef", trigger: "manual", delivery: null },
  pin: { imageTag: `harpoon/shop:${n}`, imageId: "sha256:x", hostPort: port, containerPort: 3000, healthPath: "/healthz", envNames: [], secrets: [], snapshot: null, build: null }, config: null, configDiff: [],
});
const snapshot = (o = {}) => ({
  schema: 1, generatedMs: NOW, runtimeDir: "/fixture", source: { kind: "own", path: "/fixture", journalFormat: "sqlite", readOnly: false }, store: "ok", storeError: null, writerHeld: false,
  gate: { liveDeploy: false, liveReplacement: false, reason: "closed in this build" }, router: { domain: "harpoon.localhost", port: 7480, error: null },
  repos: [{ name: "shop", url: "https://example.com/shop.git", app: APP, project: null, environment: null, branch: "main", filters: [], credential: "none", webhook: null, containerPort: 3000, dockerfile: "Dockerfile", context: null, target: null, healthPath: "/healthz", seen: [] }],
  apps: [{ id: APP, repo: "shop", hostname: HOST, route: "serving", deployments: [dep(1, 38001), dep(2, 38002)],
    serving: { release: `rel_${DEP(2).slice(4)}`, deployment: DEP(2), container: `hp-${DEP(2)}`, via: "original", sinceMs: NOW - 590000, hostPort: 38002, healthPath: "/healthz" }, pauses: [], events: [], ...o.app }],
  definitions: [], ...o.snap,
});
const SCENES = {
  "01-running": { containers: [{ ID: "c1", Names: `hp-${DEP(2)}`, State: "running", Ports: "127.0.0.1:38002->3000/tcp", Labels: null }], snap: snapshot() },
  "02-stopped": { containers: [], snap: snapshot() },
  "03-hostname-collision": { containers: [], snap: snapshot({ app: { route: "conflict" } }) },
  "04-router-unavailable": { containers: [{ ID: "c1", Names: `hp-${DEP(2)}`, State: "running", Ports: "", Labels: null }], snap: snapshot({ snap: { router: { domain: "harpoon.localhost", port: null, error: "cannot listen on 127.0.0.1:7480: Address already in use" } } }) },
};

const proc = spawn(CHROME, ["--headless=new", "--disable-gpu", "--hide-scrollbars", "--remote-debugging-port=9444", "--window-size=1440,1000", "--user-data-dir=/tmp/harpoon-stable-url-shots", "about:blank"], { stdio: "ignore" });
let id = 0; const pending = new Map(); let ws;
const send = (method, params = {}) => new Promise((res, rej) => { const i = ++id; pending.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); });
const js = async (expression) => { const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true }); if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails)); return r.result.value; };
const waitFor = async (expr, ms = 15000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { if (await js(expr).catch(() => false)) return true; await sleep(250); } return false; };
// sel/text are passed as call arguments (data), never spliced into the evaluated source.
const CLICK_TEXT = `function (sel, text) { const b = [...document.querySelectorAll(sel)].find((x) => x.innerText.replace(/\\s+/g, " ").trim().includes(text)); if (!b) return false; b.click(); return true; }`;
const clickText = async (sel, text) => {
  const fn = await send("Runtime.evaluate", { expression: `(${CLICK_TEXT})` });
  const r = await send("Runtime.callFunctionOn", { objectId: fn.result.objectId, functionDeclaration: "function (...a) { return this(...a); }", arguments: [{ value: sel }, { value: text }], returnByValue: true });
  return r.result.value;
};
try {
  let tabs; for (let i = 0; i < 40 && !tabs; i++) { try { tabs = await (await fetch("http://localhost:9444/json")).json(); } catch { await sleep(250); } }
  ws = new WebSocket(tabs.find((t) => t.type === "page").webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  ws.onmessage = (m) => { const d = JSON.parse(m.data); if (d.method === "Runtime.consoleAPICalled" && d.params.type === "error" && process.env.SHOW_ERRORS) console.log("console.error:", d.params.args.map((a) => a.value ?? a.description).join(" ").slice(0, 600)); const p = pending.get(d.id); if (p) { pending.delete(d.id); d.error ? p.rej(new Error(JSON.stringify(d.error))) : p.res(d.result); } };
  await send("Page.enable"); await send("Runtime.enable");
  let script;
  for (const [name, scene] of Object.entries(SCENES)) {
    if (script) await send("Page.removeScriptToEvaluateOnNewDocument", { identifier: script });
    script = (await send("Page.addScriptToEvaluateOnNewDocument", { source: STUB_SOURCE })).identifier;
    await send("Page.navigate", { url: fixtureUrl("http://localhost:1420/", { ...scene, now: NOW }) });
    await sleep(1500);
    await waitFor(`!!document.querySelector('nav, aside, [role=navigation]')`);
    await clickText("button, a, [role=button], li", "Deployments");
    const ok = await waitFor(`!!document.querySelector('button.dp-card')`);
    await clickText("button.dp-card", "shop");
    await waitFor(`!!document.querySelector('.dp-hero')`, 8000);
    await sleep(800);
    const text = await js(`(document.querySelector('.dp-address')?.innerText ?? 'NO ADDRESS BAR').replace(/\\s+/g, ' ')`);
    console.log(name.padEnd(24), ok ? "" : "(app list not found)", "|", text);
    const shot = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(OUT + name + ".png", Buffer.from(shot.data, "base64"));
  }
} finally { proc.kill(); }
