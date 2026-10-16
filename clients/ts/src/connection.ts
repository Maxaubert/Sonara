import * as net from "node:net";
import { SonaraError } from "./errors.js";
import type { Reply, RequestOptions } from "./types.js";

/** Longest line the runtime accepts (bytes, newline included); a longer one closes the connection. */
export const MAX_LINE = 1 << 20;

interface Pending {
  resolve: (reply: Reply) => void;
  reject: (err: Error) => void;
}

/**
 * One TCP JSON-lines connection to the runtime. Requests carry an `id`;
 * replies (with `ok`) settle the matching promise, events (with `event`) go
 * to `onEvent`.
 */
export class Connection {
  onEvent: ((event: Record<string, unknown>) => void) | null = null;
  onClose: (() => void) | null = null;
  private readonly sock: net.Socket;
  private buf = "";
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private closedFlag = false;

  private constructor(sock: net.Socket) {
    this.sock = sock;
    sock.setEncoding("utf8");
    sock.setNoDelay(true);
    sock.on("data", (chunk: string) => this.onData(chunk));
    sock.on("error", () => {
      // "close" follows and settles everything.
    });
    sock.on("close", () => this.finish());
  }

  /** Connect to `127.0.0.1:<port>`; rejects after `timeoutMs`. */
  static open(port: number, timeoutMs: number): Promise<Connection> {
    return new Promise((resolve, reject) => {
      const sock = net.connect({ host: "127.0.0.1", port });
      const timer = setTimeout(() => {
        sock.destroy();
        reject(new SonaraError("E_CLOSED", `no connection to 127.0.0.1:${port} within ${timeoutMs} ms`));
      }, timeoutMs);
      sock.once("connect", () => {
        clearTimeout(timer);
        sock.removeAllListeners("error");
        resolve(new Connection(sock));
      });
      sock.once("error", (err) => {
        clearTimeout(timer);
        reject(new SonaraError("E_CLOSED", `cannot connect to 127.0.0.1:${port}: ${err.message}`));
      });
    });
  }

  get closed(): boolean {
    return this.closedFlag;
  }

  /** Send `{type, ...fields}`; resolves with the `ok: true` reply, rejects with a SonaraError. */
  request(type: string, fields: Record<string, unknown> = {}, opts: RequestOptions = {}): Promise<Reply> {
    if (this.closedFlag) {
      return Promise.reject(new SonaraError("E_CLOSED", "the connection is closed"));
    }
    const id = this.nextId++;
    const line = JSON.stringify({ ...fields, type, id }) + "\n";
    const bytes = Buffer.byteLength(line, "utf8");
    if (bytes > MAX_LINE) {
      // Refused here, so the shared connection stays open (#54).
      return Promise.reject(new SonaraError("E_BAD_REQUEST", `line too long: ${bytes} bytes, at most ${MAX_LINE}`));
    }
    return new Promise((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const done = () => {
        if (timer !== undefined) clearTimeout(timer);
      };
      this.pending.set(id, {
        resolve: (r) => {
          done();
          resolve(r);
        },
        reject: (e) => {
          done();
          reject(e);
        },
      });
      if (opts.timeoutMs !== undefined) {
        timer = setTimeout(() => {
          // Replies carry their id, so a late one is dropped (onLine).
          if (this.pending.delete(id)) {
            reject(new SonaraError("E_TIMEOUT", `no reply to ${type} within ${opts.timeoutMs} ms`));
          }
        }, opts.timeoutMs);
      }
      this.sock.write(line);
    });
  }

  close(): void {
    this.sock.end();
    this.sock.destroy();
    this.finish();
  }

  private onData(chunk: string): void {
    this.buf += chunk;
    let nl: number;
    while ((nl = this.buf.indexOf("\n")) >= 0) {
      const line = this.buf.slice(0, nl).trim();
      this.buf = this.buf.slice(nl + 1);
      if (line) this.onLine(line);
    }
  }

  private onLine(line: string): void {
    let msg: unknown;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (typeof msg !== "object" || msg === null) return;
    const m = msg as Record<string, unknown>;
    if (typeof m.event === "string") {
      this.onEvent?.(m);
      return;
    }
    if (!("ok" in m)) return;
    const id = typeof m.id === "number" ? m.id : undefined;
    // Replies come in request order, so a reply without our id (the runtime
    // could not parse the request) belongs to the oldest pending request.
    // One with an id no longer pending answers a request that timed out.
    if (id !== undefined && !this.pending.has(id)) return;
    const key = id !== undefined ? id : this.pending.keys().next().value;
    if (key === undefined) return;
    const p = this.pending.get(key)!;
    this.pending.delete(key);
    if (m.ok === true) {
      p.resolve(m);
    } else {
      const err = (m.error ?? {}) as { code?: string; message?: string; reason?: string };
      p.reject(new SonaraError(err.code ?? "E_BAD_REQUEST", err.message ?? "request failed", err.reason));
    }
  }

  private finish(): void {
    if (this.closedFlag) return;
    this.closedFlag = true;
    for (const p of this.pending.values()) {
      p.reject(new SonaraError("E_CLOSED", "the connection closed before the reply"));
    }
    this.pending.clear();
    this.onClose?.();
  }
}
