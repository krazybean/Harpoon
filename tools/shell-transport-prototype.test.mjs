import assert from "node:assert/strict";
import test from "node:test";
import { ShellTransport, decodeFrames, encodeFrame, terminalData } from "./shell-transport-prototype.mjs";

function mockSpawn(calls) {
  return async (request) => {
    const process = {
      writes: [],
      sizes: [],
      cancelled: false,
      async write(data) { this.writes.push(data.toString("utf8")); },
      async resize(size) { this.sizes.push(size); },
      async cancel() { this.cancelled = true; },
    };
    calls.push({ request, process });
    return process;
  };
}

test("frames survive partial reads and preserve arbitrary terminal bytes", () => {
  const encoded = encodeFrame(terminalData(Buffer.from([0, 10, 255])));
  const split = decodeFrames(encoded.subarray(0, 5));
  assert.deepEqual(split.frames, []);
  const decoded = decodeFrames(Buffer.concat([split.rest, encoded.subarray(5)]));
  assert.equal(Buffer.from(decoded.frames[0].data, "base64")[2], 255);
  assert.equal(decoded.rest.length, 0);
});

test("sessions use one Harpoon Docker context and remain independently cancellable", async () => {
  const calls = [];
  const transport = new ShellTransport({ spawn: mockSpawn(calls) });
  const one = await transport.open({ sessionId: "one", containerId: "a".repeat(64) });
  const two = await transport.open({ sessionId: "two", containerId: "b".repeat(64), command: ["/bin/ash"] });
  assert.equal(decodeFrames(one).frames[0].context, "harpoon");
  assert.equal(decodeFrames(two).frames[0].containerId, "b".repeat(64));
  assert.deepEqual(calls[0].request.args, ["--context", "harpoon", "exec", "--interactive", "--tty", "a".repeat(64), "/bin/sh"]);

  await transport.send("one", { type: "stdin", data: Buffer.from("echo one\n").toString("base64") });
  await transport.send("one", { type: "resize", cols: 120, rows: 40 });
  await transport.send("one", { type: "cancel" });
  assert.deepEqual(calls[0].process.writes, ["echo one\n"]);
  assert.deepEqual(calls[0].process.sizes, [{ cols: 120, rows: 40 }]);
  assert.equal(calls[0].process.cancelled, true);
  assert.equal(calls[1].process.cancelled, false);
  assert.deepEqual(decodeFrames(transport.finish("two", 137)).frames[0], { type: "exit", sessionId: "two", code: 137 });
});

test("rejects names, short IDs, invalid sizes, and duplicate sessions", async () => {
  const transport = new ShellTransport({ spawn: mockSpawn([]) });
  await assert.rejects(() => transport.open({ sessionId: "one", containerId: "web" }), /full stable container ID/);
  await transport.open({ sessionId: "one", containerId: "c".repeat(64) });
  await assert.rejects(() => transport.open({ sessionId: "one", containerId: "c".repeat(64) }), /duplicate/);
  await assert.rejects(() => transport.send("one", { type: "resize", cols: 0, rows: 24 }), /terminal size/);
});
