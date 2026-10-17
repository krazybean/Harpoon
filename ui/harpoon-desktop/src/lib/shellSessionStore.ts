import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ShellTarget = { context: string; containerId: string; executable: string; user: string };
export type SharedShellEvent = { sessionId: string; kind: "output" | "exit" | "error"; data?: string; code?: number; error?: string };
type Listener = (event: SharedShellEvent) => void;
type Record = { target: ShellTarget; sessionId: string | null; opening: Promise<void>; listeners: Set<Listener>; buffer: string; unlisten: UnlistenFn | null };

const MAX_REPLAY_BYTES = 512 * 1024;

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
      record = { target, sessionId: null, opening: Promise.resolve(), listeners: new Set(), buffer: "", unlisten: null };
      this.sessions.set(key, record);
      record.opening = this.open(record, cols, rows, key);
    }
    record.listeners.add(listener);
    await record.opening;
    if (replay) listener({ sessionId: record.sessionId ?? "", kind: "output", data: replay });
    const sessionId = record.sessionId;
    if (!sessionId) throw new Error("shell session is unavailable");
    return {
      sessionId,
      detach: () => record?.listeners.delete(listener),
      write: (input: string) => this.invokeFn("write_shell_input", { sessionId, input }),
      resize: (nextCols: number, nextRows: number) => this.invokeFn("resize_shell_session", { sessionId, cols: nextCols, rows: nextRows }),
      close: () => this.close(key),
    };
  }

  private async open(record: Record, cols: number, rows: number, key: string) {
    try {
      record.unlisten = await this.listenFn<SharedShellEvent>("shell-event", ({ payload }) => {
        if (!record.sessionId || payload.sessionId !== record.sessionId) return;
        if (payload.kind === "output") {
          record.buffer += payload.data ?? "";
          if (record.buffer.length > MAX_REPLAY_BYTES) record.buffer = record.buffer.slice(-MAX_REPLAY_BYTES);
        }
        for (const listener of record.listeners) listener(payload);
        if (payload.kind === "exit") {
          record.unlisten?.();
          record.unlisten = null;
          this.sessions.delete(key);
        }
      });
      const session = await this.invokeFn<{ sessionId: string }>("open_shell_session", { context: record.target.context, containerId: record.target.containerId, cols, rows });
      record.sessionId = session.sessionId;
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
