// Runtime-status reliability: one logical refresh is one status command, concurrent callers share it, a failed read is
// reported as a failed *read* (never as a failed startup) and recovers, and a late old answer cannot undo a newer one.
// Real useShelf and the real <App/> run in jsdom; only the Tauri bridge is replaced. Everything is ordered with promises we resolve.
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { deferred, loadApp, newBridge, loadShelf, shutdown, until, win } from "./statusHarness.mjs";

const mounted = [];
afterEach(async () => { while (mounted.length) await mounted.pop()(); }); // also when an assertion failed: no App keeps running timers
after(() => shutdown());

const RUNNING = { state: "running", dockerReady: true, mgmtReady: true, pid: 67874, cpus: 2, memoryMiB: 4096, diskPath: "/d.img", socketPath: "/tmp/harpoon-docker.sock", lockHeld: true, sockExists: true };
const STOPPED = { ...RUNNING, state: "stopped", dockerReady: false, pid: null };

// ---- useShelf -----------------------------------------------------------------------------------------------------------

async function shelf(fetcher) {
  const { useShelf, createRoot, act, createElement: h } = await loadShelf();
  const api = { current: null };
  const errors = [];
  const orig = console.error; console.error = (...a) => errors.push(a.join(" "));
  const Probe = () => { api.current = useShelf(fetcher); return h("i", null, ""); };
  const host = win.document.createElement("div"); win.document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(h(Probe)));
  return { api, act, errors, unmount: () => act(async () => { root.unmount(); console.error = orig; }), restore: () => { console.error = orig; } };
}

test("two simultaneous callers share one read and one result", async () => {
  const d = deferred(); let calls = 0;
  const s = await shelf(() => (calls++, d.promise));
  let a, b;
  await s.act(async () => { a = s.api.current.refresh(); b = s.api.current.refresh(); });
  assert.equal(calls, 1, "one logical refresh is one underlying command");
  await s.act(async () => d.resolve(RUNNING));
  assert.deepEqual([await a, await b], [RUNNING, RUNNING], "the duplicate caller gets the value, not null");
  assert.equal(s.api.current.data, RUNNING);
  assert.equal(s.api.current.refreshing, false);
  await s.act(async () => { void s.api.current.refresh(); });
  assert.equal(calls, 2, "after it settles the next refresh is a new read");
  await s.unmount();
});

test("a genuine failure reaches every caller as a failure, is not cached, and a later refresh recovers", async () => {
  const first = deferred(); const second = deferred(); const queue = [first, second]; let calls = 0;
  const s = await shelf(() => queue[calls++].promise);
  let a, b, r;
  await s.act(async () => { a = s.api.current.refresh(); b = s.api.current.refreshResult(); r = s.api.current.refreshResult(); });
  await s.act(async () => first.reject(new Error("spawn harpoon: boom")));
  assert.equal(await a, null);
  assert.deepEqual(await b, { ok: false, error: "Error: spawn harpoon: boom" });
  assert.equal(await r, await b, "all callers got the same settled result");
  assert.equal(calls, 1);
  assert.match(s.api.current.error, /boom/, "the failure stays visible");
  assert.equal(s.api.current.refreshing, false);
  await s.act(async () => { a = s.api.current.refresh(); });
  assert.equal(calls, 2, "the rejected promise was not kept");
  await s.act(async () => second.resolve(RUNNING));
  assert.equal(await a, RUNNING);
  assert.equal(s.api.current.error, null, "success clears the stale error");
  assert.equal(s.api.current.data, RUNNING);
  await s.unmount();
});

test("a fetcher that throws before returning a promise is a failure too, and is not cached", async () => {
  let calls = 0;
  const s = await shelf(() => { calls++; if (calls === 1) throw new Error("sync"); return Promise.resolve(RUNNING); });
  let r;
  await s.act(async () => { r = await s.api.current.refreshResult(); });
  assert.equal(r.ok, false);
  await s.act(async () => { r = await s.api.current.refreshResult(); });
  assert.deepEqual(r, { ok: true, value: RUNNING });
  await s.unmount();
});

test("an old answer arriving after a newer published state does not replace it", async () => {
  const d = deferred();
  const s = await shelf(() => d.promise);
  let p;
  await s.act(async () => { p = s.api.current.refresh(); });
  await s.act(async () => s.api.current.publish(STOPPED, null)); // the worker reports the VM stopped while the read is running
  await s.act(async () => d.resolve(RUNNING)); // the old read lands late
  assert.equal(s.api.current.data.state, "stopped", "the newer observation wins");
  assert.equal((await p).state, "stopped", "and the caller is told the newer one");
  await s.unmount();
});

test("an old failure after a newer success neither raises an error nor discards the data", async () => {
  const d = deferred();
  const s = await shelf(() => d.promise);
  let p;
  await s.act(async () => { p = s.api.current.refresh(); });
  await s.act(async () => s.api.current.publish(RUNNING, null));
  await s.act(async () => d.reject(new Error("late")));
  assert.equal(s.api.current.error, null);
  assert.equal(await p, RUNNING);
  await s.unmount();
});

test("finishing after unmount updates nothing and does not warn", async () => {
  const d = deferred();
  const s = await shelf(() => d.promise);
  let p;
  await s.act(async () => { p = s.api.current.refresh(); });
  await s.unmount();
  d.resolve(RUNNING);
  assert.equal(await p, RUNNING);
  assert.deepEqual(s.errors, []);
});

// ---- the real App ---------------------------------------------------------------------------------------------------------

async function mountApp() {
  const bridge = newBridge();
  const { App, createRoot, act, createElement: h } = await loadApp(bridge);
  Object.defineProperty(win, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  const status = [];
  bridge.handlers.get_status = () => { const d = deferred(); status.push(d); return d.promise; };
  const host = win.document.createElement("div"); win.document.body.appendChild(host);
  const root = createRoot(host);
  mounted.push(() => act(async () => { root.unmount(); host.remove(); }));
  const text = () => host.textContent.replace(/\s+/g, " ");
  const wait = (cond, what) => until(act, cond, what).catch((e) => { e.message += " :: " + text().slice(0, 400); throw e; });
  await act(async () => root.render(h(App)));
  return {
    host, text, wait, status, act, bridge, count: (cmd) => bridge.calls.filter((c) => c === cmd).length,
    poll: () => act(async () => { win.document.dispatchEvent(new win.Event("visibilitychange")); }), // what a background poll does
    click: (label) => act(async () => { [...host.querySelectorAll("button")].find((b) => b.textContent.includes(label)).dispatchEvent(new win.MouseEvent("click", { bubbles: true, cancelable: true })); }),
    emit: (key, value, ok = true) => act(async () => { for (const f of bridge.listeners["runtime-refresh"] ?? []) f({ payload: { key, value, ok, generation: 7 } }); }),
  };
}

test("startup overlapping a background poll shares the poll's read: one status command, no false failure, consistent screen", async () => {
  const m = await mountApp();
  await m.poll(); // a background poll is already reading the status ...
  assert.equal(m.status.length, 1);
  await m.wait(() => /Checking runtime status|Phase:/.test(m.text()), "bootstrap asking for the status"); // ... when startup asks for it
  assert.doesNotMatch(m.text(), /Phase:|Could not read Harpoon status/, "startup was not told 'nothing' while the poll was reading");
  assert.equal(m.status.length, 1, "startup joined the poll's read instead of running a second command");
  await m.act(async () => m.status[0].resolve(RUNNING));
  await m.wait(() => /running/i.test(m.text()) && !m.text().includes("Reading runtime"), "startup finished");
  const t = m.text();
  assert.doesNotMatch(t, /could not start|Could not read Harpoon status|Phase:/i, "no failure anywhere");
  assert.match(t, /Docker: ready/);
  assert.equal(m.host.querySelector('[role="alert"]'), null);
  assert.equal(m.count("get_status"), 1);
});

test("a rejected status command is reported as a failed read, not as a failed start, and Check again recovers", async () => {
  const m = await mountApp();
  await m.wait(() => m.status.length === 1, "first status command");
  await m.act(async () => m.status[0].reject(new Error("failed to execute harpoon status --json: Resource busy")));
  await m.wait(() => m.text().includes("Phase:"), "failure panel");
  const t = m.text();
  assert.match(t, /Could not read Harpoon status/);
  assert.match(t, /Resource busy/, "the underlying reason is shown");
  assert.doesNotMatch(t, /Harpoon could not start|VM failed/, "a status read failure is not a startup failure");
  assert.match(t, /does not mean Harpoon failed to start/);
  await m.click("Check again");
  await m.wait(() => m.status.length === 2, "the retry reads the status again (the failed read was not kept)");
  await m.act(async () => m.status[1].resolve(RUNNING));
  await m.wait(() => !m.text().includes("Phase:"), "failure panel gone");
  assert.doesNotMatch(m.text(), /Could not read Harpoon status|status error/);
  assert.match(m.text(), /Docker: ready/);
});

test("when a later background read succeeds the stale failure clears by itself", async () => {
  const m = await mountApp();
  await m.wait(() => m.status.length === 1, "first status command");
  await m.act(async () => m.status[0].reject(new Error("transient")));
  await m.wait(() => m.text().includes("Phase:"), "failure panel");
  await m.wait(() => (m.bridge.listeners["runtime-refresh"] ?? []).length > 0 && m.count("start_runtime_refresh_worker") > 0, "refresh worker listening");
  await m.emit("get_status", RUNNING); // the Rust refresh worker publishes a good status
  await m.wait(() => !m.text().includes("Phase:"), "failure panel gone");
  assert.doesNotMatch(m.text(), /Could not read Harpoon status|status error/);
  assert.match(m.text(), /Docker: ready/);
});

test("an authoritative startup failure is still shown as one", async () => {
  const m = await mountApp();
  await m.wait(() => m.status.length === 1, "first status command");
  await m.act(async () => m.status[0].resolve({ ...STOPPED, state: "failed", failure: { message: "Virtualization.framework refused the VM", category: "vm_start", source: "vz", occurredAt: 0 } }));
  await m.wait(() => m.text().includes("Phase:"), "failure panel");
  assert.match(m.text(), /Virtualization\.framework refused the VM/);
  assert.doesNotMatch(m.text(), /Could not read Harpoon status/);
});
