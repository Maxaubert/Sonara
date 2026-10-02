import { Connection } from "./connection.js";
import { SonaraError } from "./errors.js";
import { AgentApi, ChannelsApi, SystemApi } from "./extensions.js";
import type {
  ControlAction,
  HelloInfo,
  ItemEvent,
  LogEvent,
  Reply,
  RuntimeInfo,
  SettingKey,
  SpeakOptions,
  State,
  Voice,
} from "./types.js";

export type Unsubscribe = () => void;

type Listener<T> = (value: T) => void;

/** Opens and greets one more connection (the event connection). */
export type Dial = () => Promise<Connection>;

/**
 * A connected Sonara client. Requests use one connection; events
 * (`onState`, `onItem`, `onLog`) arrive on a second one, opened on the first
 * listener and subscribed to every stream.
 */
export class SonaraClient {
  /** The runtime's `hello` reply: version, protocol, capabilities, extensions. */
  readonly info: HelloInfo;
  /** `runtime.json` of the instance this client talks to. */
  readonly runtime: Readonly<RuntimeInfo>;
  /** Extension `channels` (pass-through; needs a runtime that offers it). */
  readonly channels: ChannelsApi;
  /** Extension `agent` (pass-through). */
  readonly agent: AgentApi;
  /** Extension `system` (pass-through). */
  readonly system: SystemApi;

  private readonly conn: Connection;
  private readonly dial: Dial;
  private eventConn: Connection | null = null;
  private eventsOpening: Promise<void> | null = null;
  private eventsFailed = false;
  private closing = false;
  private readonly stateListeners = new Set<Listener<State>>();
  private readonly itemListeners = new Set<Listener<ItemEvent>>();
  private readonly logListeners = new Set<Listener<LogEvent>>();
  private readonly closeListeners = new Set<() => void>();

  constructor(conn: Connection, runtime: RuntimeInfo, info: HelloInfo, dial: Dial) {
    this.conn = conn;
    this.runtime = runtime;
    this.info = info;
    this.dial = dial;
    const send = (type: string, fields: Record<string, unknown>) => this.request(type, fields);
    this.channels = new ChannelsApi(send);
    this.agent = new AgentApi(send);
    this.system = new SystemApi(send);
    conn.onClose = () => {
      this.eventConn?.close();
      for (const cb of [...this.closeListeners]) cb();
    };
  }

  /** True once the connection closed (close(), a takeover or the runtime exited). */
  get closed(): boolean {
    return this.conn.closed;
  }

  /** Send any protocol message; resolves with the `ok: true` reply. */
  request(type: string, fields: Record<string, unknown> = {}): Promise<Reply> {
    return this.conn.request(type, fields);
  }

  /** Add `text` as one item; resolves with its item id. */
  async speak(text: string, opts: SpeakOptions = {}): Promise<number> {
    const fields: Record<string, unknown> = { text };
    if (opts.mode !== undefined) fields.mode = opts.mode;
    if (opts.interrupt !== undefined) fields.interrupt = opts.interrupt;
    if (opts.label !== undefined) fields.label = opts.label;
    const r = await this.request("speak", fields);
    return r.item_id as number;
  }

  /** Playback control: play, pause, toggle, stop, skip, previous, next, restart, mute, unmute. */
  async control(action: ControlAction): Promise<void> {
    await this.request("control", { action });
  }

  /** Change a setting; resolves with the value now in force. */
  async set(key: SettingKey, value: unknown): Promise<unknown> {
    const r = await this.request("set", { key, value });
    return r.value;
  }

  /** Read a setting. */
  async get(key: SettingKey): Promise<unknown> {
    const r = await this.request("get", { key });
    return r.value;
  }

  /** Voices of one engine, or of all. */
  async voices(engine?: string): Promise<Voice[]> {
    const r = await this.request("voices", engine === undefined ? {} : { engine });
    return r.voices as Voice[];
  }

  /** Call `cb` with every `state` snapshot (the first is the current state). */
  onState(cb: Listener<State>): Unsubscribe {
    return this.listen(this.stateListeners, cb);
  }

  /** Call `cb` with every `item` event (started, finished, skipped, failed). */
  onItem(cb: Listener<ItemEvent>): Unsubscribe {
    return this.listen(this.itemListeners, cb);
  }

  /** Call `cb` with every `log` event. */
  onLog(cb: Listener<LogEvent>): Unsubscribe {
    return this.listen(this.logListeners, cb);
  }

  /**
   * Call `cb` once the request connection closes for any reason. A drop of
   * the event connection alone is reported to `onLog` ("event stream
   * closed"); the next listener added opens a new one.
   */
  onClose(cb: () => void): Unsubscribe {
    this.closeListeners.add(cb);
    return () => {
      this.closeListeners.delete(cb);
    };
  }

  /**
   * Resolves once the event connection is subscribed (opened by the first
   * listener), so nothing that follows is missed. Rejects when that failed;
   * the next listener added tries again.
   */
  eventsReady(): Promise<void> {
    return this.eventsOpening ?? Promise.resolve();
  }

  /** Close both connections. The runtime keeps running for other clients and exits on its own when idle. */
  async close(): Promise<void> {
    this.closing = true;
    this.eventConn?.close();
    this.conn.close();
  }

  private listen<T>(set: Set<Listener<T>>, cb: Listener<T>): Unsubscribe {
    set.add(cb);
    // A failed attempt is retried by the next listener.
    if ((!this.eventsOpening || this.eventsFailed) && !this.closing) {
      this.eventsFailed = false;
      this.eventsOpening = this.openEvents();
      // The failure is reported to log listeners; eventsReady() still rejects.
      this.eventsOpening.catch(() => undefined);
    }
    return () => {
      set.delete(cb);
    };
  }

  private async openEvents(): Promise<void> {
    try {
      const conn = await this.dial();
      if (this.closing) {
        conn.close();
        return;
      }
      this.eventConn = conn;
      conn.onEvent = (e) => this.dispatch(e);
      await conn.request("subscribe", { events: ["state", "items", "log"] });
      // The event connection can drop on its own (the request connection
      // reports through onClose). Once subscribed, tell log listeners, and
      // let the next listener open a new one.
      conn.onClose = () => {
        if (this.eventConn === conn) this.eventConn = null;
        if (this.closing || this.conn.closed) return;
        this.eventsFailed = true;
        for (const cb of [...this.logListeners]) cb({ message: "event stream closed" });
      };
    } catch (err) {
      this.eventsFailed = true;
      const message = `event stream failed: ${(err as Error).message}`;
      for (const cb of [...this.logListeners]) cb({ message });
      throw err instanceof SonaraError ? err : new SonaraError("E_CLOSED", message);
    }
  }

  private dispatch(e: Record<string, unknown>): void {
    const { event, ...body } = e;
    if (event === "state") for (const cb of [...this.stateListeners]) cb(body as unknown as State);
    else if (event === "item") for (const cb of [...this.itemListeners]) cb(body as unknown as ItemEvent);
    else if (event === "log") for (const cb of [...this.logListeners]) cb(body as unknown as LogEvent);
  }
}
