/**
 * The surface side of the Goia plugin bridge — the client of `pluginHost.ts`.
 *
 * This file is the ONLY coupling between godesk's canvas and Goia: it speaks the
 * postMessage protocol the host answers, and exposes it as ordinary async calls.
 * Nothing here imports Goia, and Goia imports nothing here; the contract is this
 * message shape and the allow-listed command names, nothing more. (The same
 * protocol is demonstrated in Goia's `examples/surface-hello/index.html`.)
 *
 *   frame → host : { kind:"call",  reqId, cmd, args }
 *   host  → frame: { kind:"result", reqId, ok, value | error }
 *   host  → frame: { kind:"event", name, payload }      (pty:data, pty:exit)
 *
 * When this bundle runs OUTSIDE Goia (opened directly, or in a test), there is no
 * parent host; calls reject fast so the canvas can show that it is not hosted
 * rather than hang.
 */

/* ---- the shapes Goia's engine returns, kept minimal and local -------------- */

export type SessionState = "working" | "blocked" | "done" | "unknown";
export type SessionStatus = "busy" | "idle" | "unknown";
export type Provider = "claude" | "codex" | "gemini" | "opencode";

export interface Session {
  id: string;
  session_id: string;
  provider: Provider;
  name: string;
  cwd: string;
  status: SessionStatus;
  state: SessionState;
  streamable: boolean;
  project_id?: string | null;
}

/** One styled run of terminal text, as `session_logs` renders it. */
export interface TermSpan {
  text: string;
  fg?: string | null;
  bg?: string | null;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
  inverse?: boolean;
}

export interface SessionLog {
  id: string;
  lines: TermSpan[][];
  error: string | null;
}

/** An inter-agent message, the basis of a detected link between two sessions. */
export interface Beat {
  from?: string | null;
  to?: string | null;
  at?: number;
}

type PtyData = { id: string; data: string };
type PtyExit = { id: string };

/* ---- the channel ----------------------------------------------------------- */

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
  timer: number;
}

const CALL_TIMEOUT_MS = 15_000;

class HostBridge {
  private readonly pending = new Map<number, Pending>();
  private nextReqId = 0;
  private readonly ptyData = new Map<string, Set<(d: PtyData) => void>>();
  private readonly ptyExit = new Map<string, Set<(e: PtyExit) => void>>();
  /** True once we have seen any message from the host; before that a call still
   *  goes out (the host may just not have posted yet), but a total silence is
   *  surfaced as "not hosted" by the timeout. */
  hosted = false;

  constructor() {
    window.addEventListener("message", this.onMessage);
  }

  private onMessage = (e: MessageEvent) => {
    // The host is our parent window. Anything else is ignored; we never trust a
    // message's contents beyond routing it by reqId / event name.
    if (e.source !== window.parent) return;
    const m = e.data as { kind?: string; reqId?: number; ok?: boolean; value?: unknown; error?: string; name?: string; payload?: unknown };
    if (!m || typeof m !== "object") return;
    this.hosted = true;
    if (m.kind === "result" && typeof m.reqId === "number") {
      const p = this.pending.get(m.reqId);
      if (!p) return;
      this.pending.delete(m.reqId);
      window.clearTimeout(p.timer);
      m.ok ? p.resolve(m.value) : p.reject(new Error(m.error ?? "call failed"));
    } else if (m.kind === "event") {
      if (m.name === "pty:data") {
        const d = m.payload as PtyData;
        this.ptyData.get(d.id)?.forEach((cb) => cb(d));
      } else if (m.name === "pty:exit") {
        const x = m.payload as PtyExit;
        this.ptyExit.get(x.id)?.forEach((cb) => cb(x));
      }
    }
  };

  call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const reqId = ++this.nextReqId;
      const timer = window.setTimeout(() => {
        this.pending.delete(reqId);
        reject(new Error(this.hosted ? `timed out: ${cmd}` : "not running inside Goia"));
      }, CALL_TIMEOUT_MS);
      this.pending.set(reqId, { resolve: resolve as (v: unknown) => void, reject, timer });
      window.parent.postMessage({ kind: "call", reqId, cmd, args }, "*");
    });
  }

  onPtyData(id: string, cb: (d: PtyData) => void): () => void {
    let set = this.ptyData.get(id);
    if (!set) this.ptyData.set(id, (set = new Set()));
    set.add(cb);
    return () => set!.delete(cb);
  }

  onPtyExit(id: string, cb: (e: PtyExit) => void): () => void {
    let set = this.ptyExit.get(id);
    if (!set) this.ptyExit.set(id, (set = new Set()));
    set.add(cb);
    return () => set!.delete(cb);
  }
}

const bridge = new HostBridge();

/** Typed calls over the bridge — the allow-listed surface of Goia's engine. */
export const sdk = {
  sessionsList: () => bridge.call<Session[]>("sessionsList"),
  sessionsLogsBulk: (ids: string[], tailLines: number) =>
    bridge.call<SessionLog[]>("sessionsLogsBulk", { ids, tailLines }),
  sessionLogs: (id: string, tailLines: number) =>
    bridge.call<TermSpan[][]>("sessionLogs", { id, tailLines }),
  busRecent: (limit?: number) => bridge.call<Beat[]>("busRecent", { limit }),

  sessionStop: (id: string) => bridge.call<unknown>("sessionStop", { id }),
  sessionSendMessage: (provider: Provider, target: string, text: string) =>
    bridge.call<unknown>("sessionSendMessage", { provider, target, text }),

  ptyOpen: (id: string, cols: number, rows: number) => bridge.call<void>("ptyOpen", { id, cols, rows }),
  ptyWrite: (id: string, data: string) => bridge.call<void>("ptyWrite", { id, data }),
  ptyResize: (id: string, cols: number, rows: number) => bridge.call<void>("ptyResize", { id, cols, rows }),
  ptyClose: (id: string) => bridge.call<void>("ptyClose", { id }),

  stateGet: () => bridge.call<string>("stateGet"),
  stateSet: (state: string) => bridge.call<void>("stateSet", { state }),

  onPtyData: (id: string, cb: (d: PtyData) => void) => bridge.onPtyData(id, cb),
  onPtyExit: (id: string, cb: (e: PtyExit) => void) => bridge.onPtyExit(id, cb),
};
