// Manual end-to-end check of the SIMULATED Deployments UI in headless Chrome (CDP). No test framework needed.
// Requires: Vite dev server on :1420 and Google Chrome.   node docs/ui/deploy-integrated/click-through.mjs
import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const URL_ = "http://localhost:1420/#deploy=scenario:healthy,screen:app,tab:overview,runtime:connected";
const proc = spawn(CHROME, ["--headless=new", "--disable-gpu", "--remote-debugging-port=9333", "--window-size=1440,1000", "--user-data-dir=/tmp/harpoon-deploy-cdp", URL_], { stdio: "ignore" });
let id = 0;
const pending = new Map();
let ws;
const send = (method, params = {}) => new Promise((res, rej) => { const i = ++id; pending.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); });
const evalJs = async (expr) => { const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }); if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails.exception?.description ?? r.exceptionDetails)); return r.result.value; };
const text = () => evalJs("document.querySelector('[data-testid=deployments-view]')?.innerText ?? ''");
// `label` is matched against the whole, whitespace-normalized button text (so "Deploy" never hits "Deployments").
const click = (label, scope = "document") => evalJs(`(() => { const want = ${JSON.stringify(label)}; const norm = (x) => x.innerText.replace(/\\s+/g,' ').trim(); const bs = [...${scope}.querySelectorAll('button')].filter(x => !x.disabled); const b = bs.find(x => norm(x) === want) || bs.find(x => norm(x).startsWith(want)); if (!b) return false; b.click(); return true; })()`);
const setSelect = (label, value) => evalJs(`(() => { const l = [...document.querySelectorAll('.dp-sim label')].find(x => x.innerText.trim().startsWith(${JSON.stringify(label)})); const s = l.querySelector('select'); const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set; set.call(s, ${JSON.stringify(value)}); s.dispatchEvent(new Event('change', {bubbles:true})); return s.value; })()`);
const results = [];
const check = async (name, ok, detail = "") => { results.push({ name, ok }); console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok ? "" : "  → " + (detail || (await text()).replace(/\s+/g, " ").replace(/^.*?Reset /, "").slice(0, 700))}`); };
const waitFor = async (pred, ms = 15000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { if (await pred()) return true; await sleep(250); } return false; };

try {
  let tabs;
  for (let i = 0; i < 40 && !tabs; i++) { try { tabs = await (await fetch("http://localhost:9333/json")).json(); } catch { await sleep(250); } }
  const page = tabs.find((t) => t.type === "page");
  ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  ws.onmessage = (m) => { const d = JSON.parse(m.data); const p = pending.get(d.id); if (p) { pending.delete(d.id); d.error ? p.rej(new Error(JSON.stringify(d.error))) : p.res(d.result); } };
  await send("Runtime.enable");
  await waitFor(async () => (await text()).includes("Latest deployment"), 20000);

  let t = await text();
  await check("healthy: running release r-42, automation active", /RUNNING/i.test(t) && t.includes("r-42") && /Active/i.test(t));

  // 1. Deploy → failed cutover (restored)
  await setSelect("Next deployment", "cutover-restored");
  await check("deploy dialog opens", await click("Deploy", "document.querySelector('.dp-hero')") && await waitFor(async () => (await text()).includes("Review the plan before anything changes")));
  t = await text();
  await check("plan states the interruption and one bounded 60 s restoration, with no guarantee", /Brief interruption expected/.test(t) && /one bounded attempt \(60 s\)/.test(t) && /No guarantee/.test(t));
  await click("Deploy", "document.querySelector('.dp-dialog')");
  await check("running release untouched while building", await waitFor(async () => /Deploying d-43/.test(await text()), 8000) && /release r-42/.test(await text()));
  await check("failed cutover ends restored + paused + 2 held events", await waitFor(async () => /Automatic deployments are paused/.test(await text()), 20000));
  await click("Overview", "document.querySelector('.dp-tabs')");
  t = await text();
  await check("recovery is 'Restored' and application still RUNNING r-42 (paused ≠ stopped)", /Restored/i.test(t) && /release r-42/.test(t) && /2 pushes held/.test(t));

  // 2. Resume without deploying keeps nothing silently
  await click("Review & resume");
  await waitFor(async () => (await text()).includes("Held events (2)"));
  await check("resume dialog lists held events and both explicit choices", /Resume & deploy/.test(await text()) && /Resume without deploying/.test(await text()));
  await click("Resume without deploying", "document.querySelector('.dp-dialog')");
  await check("resume clears the pause", await waitFor(async () => !(await text()).includes("Automatic deployments are paused")));

  // 3. Rollback pauses again and warns about data
  await click("Roll back…", "document.querySelector('.dp-hero')");
  await waitFor(async () => (await text()).includes("Application rollback only"));
  await check("rollback dialog warns data/schema are not rolled back and that automation will pause", /not<\/b> rolled back|not rolled back/.test(await text()) && /paused/.test(await text()));
  await click("Roll back to r-41", "document.querySelector('.dp-dialog')");
  await check("after rollback: running r-41, automation paused with rollback reason", await waitFor(async () => /release r-41/.test(await text()) && /Automatic deployments are paused/.test(await text())));

  // 4. Failed restoration → down; locked keychain blocks retry; unlock → restored
  await setSelect("Scenario", "restore-failed");
  await check("restore failed: Application down", await waitFor(async () => /Application down/.test(await text())) && /Restore failed/i.test(await text()));
  await setSelect("Keychain", "locked");
  await click("Retry recovery", "document.querySelector('.dp-banner')");
  await waitFor(async () => (await text()).includes("Keychain is still locked"));
  await click("Retry recovery", "document.querySelector('.dp-dialog')");
  await check("locked Keychain: retry leaves the app down", await waitFor(async () => /Application down/.test(await text())));
  await setSelect("Keychain", "unlocked");
  await click("Retry recovery", "document.querySelector('.dp-banner')");
  await waitFor(async () => (await text()).includes("The Keychain is unlocked now"));
  await click("Retry recovery", "document.querySelector('.dp-dialog')");
  await check("unlocked: retry restores r-42, automation still paused until resumed", await waitFor(async () => /release r-42/.test(await text()) && /Automatic deployments are paused/.test(await text())));

  // 5. Deploy blocked while locked
  await setSelect("Keychain", "locked");
  await click("Deploy", "document.querySelector('.dp-hero')");
  await waitFor(async () => (await text()).includes("Keychain is locked"));
  const blocked = await evalJs("(() => { const b = [...document.querySelectorAll('.dp-dialog button')].find(x => /^\\s*Deploy\\s*$/.test(x.innerText)); return b ? b.disabled : null; })()");
  await check("locked Keychain blocks Deploy before any side effect", blocked === true, String(blocked));
  await evalJs("document.dispatchEvent(new KeyboardEvent('keydown', {key:'Escape'}))");

  // 6. Disconnected runtime never shows live state
  await setSelect("Keychain", "unlocked");
  await setSelect("Scenario", "healthy");
  await setSelect("Runtime", "stopped");
  await waitFor(async () => /Harpoon is stopped/.test(await text()));
  t = await text();
  await check("stopped runtime: UNKNOWN / NOT LIVE, no RUNNING pill, resources not shown as current", /UNKNOWN/.test(t) && /NOT LIVE/i.test(t) && !/RUNNING/.test(await evalJs("document.querySelector('.dp-hero')?.innerText ?? ''")) && /unavailable while Harpoon is stopped/.test(t));
  const disabled = await evalJs("[...document.querySelectorAll('.dp-hero button')].every(b => b.disabled)");
  await check("stopped runtime: deploy/redeploy/rollback/promote are disabled", disabled === true);
  await click("End simulated outage");
  await check("ending the simulated outage restores connected view", await waitFor(async () => /RUNNING/.test(await text())));

  // 7. Existing navigation still works
  await evalJs("[...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label') === 'Containers').click()");
  await sleep(800);
  const onContainers = await evalJs("document.querySelector('.shell-header__title .text-title')?.innerText");
  await check("Containers navigation still works", onContainers === "Containers", String(onContainers));
  await evalJs("[...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label') === 'Deployments').click()");
  await sleep(500);
  await check("Deployments state survives navigating away and back", /Latest deployment|Projects/.test(await text()));
  // PR #105 round 2: no visible entry point in Development or Preview may start a Production mutation.
  await setSelect("Runtime", "connected");
  const MUTATING = /^(Deploy|Redeploy|Roll back…|Roll back to .*|Promote…|Retry recovery|Retry deployment|Redeploy this revision|Review & resume…|Deploy from folder…)$/;
  const entryPoints = (env) => evalJs(`(() => { const re = ${MUTATING}; return [...document.querySelectorAll('[data-testid=deployments-view] button')].filter(b => re.test(b.innerText.replace(/\\s+/g,' ').trim()) && !b.disabled).map(b => b.innerText.trim()); })()`);
  const dialogOpen = () => evalJs("!!document.querySelector('[role=dialog]')");
  let leaked = [];
  for (const scenario of ["healthy", "cutover-restored", "restore-failed", "rolled-back-paused"]) {
    await setSelect("Scenario", scenario);
    await sleep(300);
    for (const env of ["Development", "Preview"]) {
      await evalJs("[...document.querySelectorAll('.dp-card')].find(x => /api/.test(x.innerText))?.click()");
      await sleep(200);
      await click(env);
      await sleep(200);
      for (const tab of ["Overview", "Deployments", "Logs", "Metrics", "Variables", "Settings"]) {
        await evalJs(`[...document.querySelectorAll('.dp-tabs button')].find(b => b.innerText.trim().startsWith(${JSON.stringify(tab)}))?.click()`);
        await sleep(150);
        const open = await entryPoints(env);
        if (open.length) leaked.push(`${scenario}/${env}/${tab}: ${open.join(", ")}`);
        if (await dialogOpen()) leaked.push(`${scenario}/${env}/${tab}: dialog open`);
      }
      // the deployment detail screen, reached from the Deployments tab, with the environment still non-Production
      await evalJs("[...document.querySelectorAll('.dp-tabs button')].find(b => b.innerText.trim().startsWith('Deployments'))?.click()");
      await sleep(150);
      await evalJs("document.querySelector('.dp-table tr.is-click')?.click()");
      await sleep(200);
      const detail = await entryPoints(env);
      if (detail.length) leaked.push(`${scenario}/${env}/detail: ${detail.join(", ")}`);
      // leave and come back: the Deployments view remounts on its Home screen
      await evalJs("[...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label') === 'Containers').click()");
      await sleep(300);
      await evalJs("[...document.querySelectorAll('.shell-nav__item')].find(b => b.getAttribute('aria-label') === 'Deployments').click()");
      await sleep(300);
    }
  }
  await check("Development/Preview expose no enabled Production-mutating entry point (4 scenarios × 2 envs × 6 tabs + detail)", leaked.length === 0, leaked.slice(0, 6).join(" | "));
  await setSelect("Scenario", "healthy");
  // PR #105 review corrections: Development cannot act on Production; Keychain help is real.
  await setSelect("Runtime", "connected");
  await sleep(300);
  await evalJs("[...document.querySelectorAll('.dp-card')].find(x => /api/.test(x.innerText))?.click()");
  await sleep(300);
  await click("Development");
  await sleep(300);
  const devDisabled = await evalJs("(() => { const bs = [...document.querySelectorAll('.dp-actions button')]; return bs.length > 0 && bs.every(b => b.disabled); })()");
  await check("Development route: Deploy/Redeploy/Roll back/Promote are all disabled", devDisabled);
  await click("Production");
  await sleep(300);
  await evalJs("[...document.querySelectorAll('.dp-tabs button')].find(b => /^Variables/.test(b.innerText.trim()))?.click()");
  await setSelect("Keychain", "locked");
  await sleep(400);
  await check("locked Keychain: 'How to unlock' opens simulated help", (await click("How to unlock")) && await waitFor(async () => /does not touch your real Keychain/.test(await evalJs("document.body.innerText"))));
  await click("Close");
  await setSelect("Keychain", "unlocked");
} catch (e) {
  console.log("ERROR", e.message);
  results.push({ name: "script", ok: false });
} finally {
  proc.kill();
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed`);
  process.exit(failed ? 1 : 0);
}
