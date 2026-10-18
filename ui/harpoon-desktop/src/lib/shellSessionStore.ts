import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ShellTarget = { context: string; containerId: string; executable: string; user: string };
export type SharedShellEvent = { sessionId: string; kind: "output" | "exit" | "error"; data?: string; code?: number; error?: string };
type Listener = (event: SharedShellEvent) => void;
type Record = { target: ShellTarget; sessionId: string | null; opening: Promise<void>; listeners: Set<Listener>; buffer: string; unlisten: UnlistenFn | null; ended: SharedShellEvent | null; failure: string | null };

const MAX_REPLAY_BYTES = 512 * 1024;
// The backend reader starts emitting as soon as the PTY exists, which can be
// before open_shell_session resolves here. Hold those events until the session
// id is known so a fast exit or the first prompt is never dropped.
const MAX_EARLY_EVENTS = 256;

function targetKey(target: ShellTarget) {
  return [target.context, target.containerId, target.executable, target.user].join("\u0000");
}

export class ShellSessionStore {
  private sessions = new Map<string, Record>();
  private invokeFn: typeof invoke;
  private listenFn: typeof listen;

  constructor({ invokeFn = invoke, listenFn = listen }: { invokeFn?: typeof invoke; listenFn?: typeof listen } = {}) {
    this.invokeFn = invokeFn;
    this.listenFn = listenFn;
  }

  async attach(target: ShellTarget, cols: number, rows: number, listener: Listener) {
    const key = targetKey(target);
    let record = this.sessions.get(key);
    const replay = record?.buffer ?? "";
    if (!record) {
      record = { target, sessionId: null, opening: Promise.resolve(), listeners: new Set(), buffer: "", unlisten: null, ended: null, failure: null };
      this.sessions.set(key, record);
      record.opening = this.open(record, cols, rows, key);
    }
    record.listeners.add(listener);
    await record.opening;
    if (replay) listener({ sessionId: record.sessionId ?? "", kind: "output", data: replay });
    const sessionId = record.sessionId;
    if (!sessionId) throw new Error("shell session is unavailable");
    if (record.ended) {
      record.listeners.delete(listener);
      const code = typeof record.ended.code === "number" ? ` with code ${record.ended.code}` : "";
      throw new Error(record.failure ?? `Shell exited${code} before it was ready.`);
    }
    return {
      sessionId,
      detach: () => record?.listeners.delete(listener),
      write: (input: string) => this.invokeFn("write_shell_input", { sessionId, input }),
      resize: (nextCols: number, nextRows: number) => this.invokeFn("resize_shell_session", { sessionId, cols: nextCols, rows: nextRows }),
      close: () => this.close(key),
    };
  }

  private async open(record: Record, cols: number, rows: number, key: string) {
    const early: SharedShellEvent[] = [];
    const deliver = (payload: SharedShellEvent) => {
      if (payload.sessionId !== record.sessionId || record.ended) return;
      if (payload.kind === "output") {
        record.buffer += payload.data ?? "";
        if (record.buffer.length > MAX_REPLAY_BYTES) record.buffer = record.buffer.slice(-MAX_REPLAY_BYTES);
      }
      if (payload.kind === "error") record.failure = payload.error ?? null;
      for (const listener of record.listeners) listener(payload);
      if (payload.kind === "exit") {
        record.ended = payload;
        record.unlisten?.();
        record.unlisten = null;
        if (this.sessions.get(key) === record) this.sessions.delete(key);
      }
    };
    try {
      record.unlisten = await this.listenFn<SharedShellEvent>("shell-event", ({ payload }) => {
        if (record.sessionId) deliver(payload);
        else if (early.length < MAX_EARLY_EVENTS) early.push(payload);
      });
      const session = await this.invokeFn<{ sessionId: string }>("open_shell_session", { context: record.target.context, containerId: record.target.containerId, cols, rows });
      record.sessionId = session.sessionId;
      for (const payload of early.splice(0)) deliver(payload);
    } catch (error) {
      record.unlisten?.();
      this.sessions.delete(key);
      throw error;
    }
  }

  private async close(key: string) {
    const record = this.sessions.get(key);
    if (!record) return;
    this.sessions.delete(key);
    record.unlisten?.();
    if (record.sessionId) await this.invokeFn("close_shell_session", { sessionId: record.sessionId }).catch(() => undefined);
  }
}

export const shellSessions = new ShellSessionStore();
