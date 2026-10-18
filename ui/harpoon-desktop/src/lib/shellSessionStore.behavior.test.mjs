import assert from "node:assert/strict";
import test from "node:test";
import { ShellSessionStore } from "./shellSessionStore.ts";

const target = { context: "harpoon", containerId: "a".repeat(64), executable: "/bin/sh", user: "default" };

test("one target owns one session while views detach and reattach with output", async () => {
  const calls = [];
  let eventListener;
  const store = new ShellSessionStore({
    invokeFn: async (command, args) => {
      calls.push({ command, args });
      return command === "open_shell_session" ? { sessionId: "session-1" } : undefined;
    },
    listenFn: async (_event, callback) => { eventListener = callback; return () => { eventListener = undefined; }; },
  });
  const firstEvents = [];
  const first = await store.attach(target, 80, 24, (event) => firstEvents.push(event));
  const second = await store.attach(target, 120, 40, () => undefined);
  assert.equal(calls.filter(({ command }) => command === "open_shell_session").length, 1);
  eventListener({ payload: { sessionId: "session-1", kind: "output", data: "alive\n" } });
  first.detach();
  second.detach();
  assert.equal(calls.filter(({ command }) => command === "close_shell_session").length, 0);
  const reopened = await store.attach(target, 100, 30, (event) => firstEvents.push(event));
  assert.equal(firstEvents.at(-1).data, "alive\n");
  await reopened.write("echo ok\n");
  await reopened.resize(100, 30);
  assert.deepEqual(calls.slice(-2).map(({ command, args }) => [command, args]), [
    ["write_shell_input", { sessionId: "session-1", input: "echo ok\n" }],
    ["resize_shell_session", { sessionId: "session-1", cols: 100, rows: 30 }],
  ]);
  await reopened.close();
  assert.equal(calls.at(-1).command, "close_shell_session");
});

test("reopened views replay only bounded output from the same owner", async () => {
  let eventListener;
  const store = new ShellSessionStore({
    invokeFn: async (command) => command === "open_shell_session" ? { sessionId: "session-bounded" } : undefined,
    listenFn: async (_event, callback) => { eventListener = callback; return () => { eventListener = undefined; }; },
  });
  const first = await store.attach(target, 80, 24, () => undefined);
  eventListener({ payload: { sessionId: first.sessionId, kind: "output", data: "x".repeat(600 * 1024) } });
  first.detach();
  let replay = "";
  const reopened = await store.attach(target, 80, 24, (event) => { replay += event.data ?? ""; });
  assert.equal(reopened.sessionId, first.sessionId);
  assert.equal(replay.length, 512 * 1024);
  await reopened.close();
});

test("different stable identities cannot share a session", async () => {
  let opens = 0;
  const store = new ShellSessionStore({
    invokeFn: async (command) => command === "open_shell_session" ? { sessionId: `session-${++opens}` } : undefined,
    listenFn: async () => () => undefined,
  });
  const one = await store.attach(target, 80, 24, () => undefined);
  const two = await store.attach({ ...target, containerId: "b".repeat(64) }, 80, 24, () => undefined);
  assert.equal(one.sessionId, "session-1");
  assert.equal(two.sessionId, "session-2");
  await one.close();
  await two.close();
});

test("A to B to A keeps output and ownership isolated", async () => {
  const listeners = new Map();
  let opens = 0;
  const store = new ShellSessionStore({
    invokeFn: async (command) => command === "open_shell_session" ? { sessionId: `session-${++opens}` } : undefined,
    listenFn: async (_event, callback) => {
      const id = `listener-${listeners.size + 1}`;
      listeners.set(id, callback);
      return () => listeners.delete(id);
    },
  });
  const targetB = { ...target, containerId: "b".repeat(64) };
  const firstA = await store.attach(target, 80, 24, () => undefined);
  const b = await store.attach(targetB, 80, 24, () => undefined);
  listeners.get("listener-1")({ payload: { sessionId: firstA.sessionId, kind: "output", data: "A prompt\n" } });
  listeners.get("listener-2")({ payload: { sessionId: b.sessionId, kind: "output", data: "B prompt\n" } });
  firstA.detach();
  b.detach();
  let replay = "";
  const secondA = await store.attach(target, 80, 24, (event) => { replay += event.data ?? ""; });
  assert.equal(secondA.sessionId, firstA.sessionId);
  assert.equal(replay, "A prompt\n");
  assert.equal(opens, 2);
  await secondA.close();
  await b.close();
});

test("output and exit emitted before open resolves are delivered, and reconnect opens a fresh session", async () => {
  let eventListener;
  let opens = 0;
  const store = new ShellSessionStore({
    invokeFn: async (command) => {
      if (command !== "open_shell_session") return undefined;
      const sessionId = `session-early-${++opens}`;
      if (opens === 1) {
        eventListener({ payload: { sessionId: "someone-else", kind: "output", data: "not mine" } });
        eventListener({ payload: { sessionId, kind: "output", data: "prompt$ " } });
        eventListener({ payload: { sessionId, kind: "error", code: 127, error: "Shell is not available in this container." } });
        eventListener({ payload: { sessionId, kind: "exit", code: 127 } });
      }
      return { sessionId };
    },
    listenFn: async (_event, callback) => { eventListener = callback; return () => { eventListener = undefined; }; },
  });
  const seen = [];
  await assert.rejects(store.attach(target, 80, 24, (event) => seen.push([event.kind, event.data ?? event.code])), /not available/);
  assert.deepEqual(seen, [["output", "prompt$ "], ["error", 127], ["exit", 127]]);
  const reconnected = await store.attach(target, 80, 24, () => undefined);
  assert.equal(reconnected.sessionId, "session-early-2");
  assert.equal(opens, 2);
  await reconnected.close();
});
