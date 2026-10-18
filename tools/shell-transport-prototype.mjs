import { Buffer } from "node:buffer";

export const FRAME_TYPES = new Set([
  "open", "stdin", "resize", "data", "exit", "error", "cancel",
]);

export function encodeFrame(frame) {
  if (!frame || typeof frame !== "object" || !FRAME_TYPES.has(frame.type)) {
    throw new Error("invalid shell frame type");
  }
  const body = Buffer.from(JSON.stringify(frame), "utf8");
  const header = Buffer.allocUnsafe(4);
  header.writeUInt32BE(body.length, 0);
  return Buffer.concat([header, body]);
}

export function decodeFrames(buffer) {
  const frames = [];
  let offset = 0;
  while (buffer.length - offset >= 4) {
    const length = buffer.readUInt32BE(offset);
    if (buffer.length - offset - 4 < length) break;
    const body = buffer.subarray(offset + 4, offset + 4 + length);
    const frame = JSON.parse(body.toString("utf8"));
    if (!frame || typeof frame !== "object" || !FRAME_TYPES.has(frame.type)) {
      throw new Error("invalid shell frame");
    }
    frames.push(frame);
    offset += 4 + length;
  }
  return { frames, rest: buffer.subarray(offset) };
}

export function terminalData(data) {
  return { type: "data", data: Buffer.from(data).toString("base64") };
}

function validContainerId(id) {
  return typeof id === "string" && /^[0-9a-f]{12,64}$/i.test(id);
}

export class ShellTransport {
  constructor({ engine = "docker", context = "harpoon", spawn }) {
    if (engine !== "docker" || context !== "harpoon") {
      throw new Error("shell transport requires the Harpoon Docker engine context");
    }
    if (typeof spawn !== "function") throw new Error("spawn adapter is required");
    this.spawn = spawn;
    this.sessions = new Map();
  }

  async open({ sessionId, containerId, command = ["/bin/sh"] }) {
    if (!sessionId || this.sessions.has(sessionId)) throw new Error("duplicate shell session");
    if (!validContainerId(containerId)) throw new Error("full stable container ID required");
    const process = await this.spawn({
      engine: "docker",
      context: "harpoon",
      args: ["--context", "harpoon", "exec", "--interactive", "--tty", containerId, ...command],
    });
    this.sessions.set(sessionId, { containerId, process });
    return encodeFrame({ type: "open", sessionId, containerId, engine: "docker", context: "harpoon" });
  }

  async send(sessionId, frame) {
    const session = this.sessions.get(sessionId);
    if (!session) throw new Error("unknown shell session");
    switch (frame.type) {
      case "stdin":
        await session.process.write(Buffer.from(frame.data, "base64"));
        return;
      case "resize":
        if (!Number.isInteger(frame.cols) || !Number.isInteger(frame.rows) || frame.cols < 1 || frame.rows < 1) {
          throw new Error("invalid terminal size");
        }
        await session.process.resize({ cols: frame.cols, rows: frame.rows });
        return;
      case "cancel":
        await session.process.cancel();
        this.sessions.delete(sessionId);
        return;
      default:
        throw new Error(`unsupported client frame: ${frame.type}`);
    }
  }

  finish(sessionId, code = 0) {
    if (!this.sessions.delete(sessionId)) return null;
    return encodeFrame({ type: "exit", sessionId, code });
  }
}
