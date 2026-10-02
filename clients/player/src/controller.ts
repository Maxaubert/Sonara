import type { PlayerAction, PlayerClient, PlayerListener, PlayerState, PlayerView } from "./types.js";
import { INITIAL_VIEW, sameView, toView } from "./view.js";
import type { Overrides } from "./view.js";

type Key = keyof Overrides;

/** One value shown ahead of the runtime until the runtime agrees or the wait runs out. */
interface Pending {
  value: boolean | number;
  /** Calls for this key not settled yet. */
  inflight: number;
  timer: ReturnType<typeof setTimeout> | null;
}

/** A coalescing sender for one setting: at most one `set` in flight, the newest value next. */
interface Sender {
  next: number | undefined;
  /** The drain in progress; resolves with whether its last request succeeded. */
  running: Promise<boolean> | null;
}

export interface PlayerControllerOptions {
  /**
   * How long a value the runtime has not confirmed yet (pause, mute,
   * volume, rate) stays shown after its request settled. Default 1000 ms.
   */
  settleMs?: number;
}

/**
 * Headless player: turns a client's `state` snapshots into a view model
 * and buttons into `control` / `set` calls. No DOM, no framework.
 *
 *     const player = new PlayerController(client);
 *     const off = player.subscribe((view) => render(view));
 *     player.toggle();
 *
 * The controller listens to the client while it has at least one
 * subscriber. Actions never reject: a failure is shown as `view.error`.
 * Pause, mute, volume and rate show the new value at once (before the
 * runtime confirms it); rapid volume or rate changes are coalesced into
 * at most one request in flight per setting.
 */
export class PlayerController {
  private readonly client: PlayerClient;
  private readonly settleMs: number;
  private readonly listeners = new Set<PlayerListener>();
  private readonly pending = new Map<Key, Pending>();
  private readonly senders: Record<"volume" | "rate", Sender> = {
    volume: { next: undefined, running: null },
    rate: { next: undefined, running: null },
  };
  private state: PlayerState | null = null;
  private connected = true;
  private error: Error | null = null;
  private current: PlayerView = INITIAL_VIEW;
  private detach: (() => void) | null = null;
  private disposed = false;

  constructor(client: PlayerClient, options: PlayerControllerOptions = {}) {
    if (!client || typeof client.control !== "function" || typeof client.onState !== "function") {
      throw new TypeError("PlayerController needs a client with control, set and onState");
    }
    this.client = client;
    this.settleMs = options.settleMs ?? 1000;
  }

  /** The current view model (a new object after every change). */
  get view(): PlayerView {
    return this.current;
  }

  /** Same as `view`, bound (for `useSyncExternalStore` and the like). */
  readonly getSnapshot = (): PlayerView => this.current;

  /** Call `listener` with every new view. Returns an unsubscribe function. */
  readonly subscribe = (listener: PlayerListener): (() => void) => {
    if (this.disposed) return () => undefined;
    this.listeners.add(listener);
    if (!this.detach) this.attach();
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) this.release();
    };
  };

  play(): Promise<void> {
    return this.control("play", "paused", false);
  }

  pause(): Promise<void> {
    return this.control("pause", "paused", true);
  }

  /** Flip play and pause. Sent as the runtime's `toggle`, so rapid presses keep their count. */
  toggle(): Promise<void> {
    return this.control("toggle", "paused", this.current.status !== "paused");
  }

  /** One chunk back; on the first chunk the runtime restarts the item. */
  previous(): Promise<void> {
    return this.control("previous");
  }

  /** One chunk forward; on the last chunk the runtime moves to the next item. */
  next(): Promise<void> {
    return this.control("next");
  }

  /** Back to the current item's first chunk; when idle, replay the last item. */
  restart(): Promise<void> {
    return this.control("restart");
  }

  /** End the current item and start the next queued one. */
  skip(): Promise<void> {
    return this.control("skip");
  }

  /** End the current item and clear the queue. */
  stop(): Promise<void> {
    return this.control("stop");
  }

  mute(): Promise<void> {
    return this.control("mute", "muted", true);
  }

  unmute(): Promise<void> {
    return this.control("unmute", "muted", false);
  }

  toggleMute(): Promise<void> {
    return this.current.muted ? this.unmute() : this.mute();
  }

  /** Volume in percent (rounded, clamped to 0..100). */
  setVolume(volume: number): Promise<void> {
    return this.setting("volume", volume, 0, 100);
  }

  /** Speed in words per minute (rounded, clamped to 100..400). */
  setRate(rate: number): Promise<void> {
    return this.setting("rate", rate, 100, 400);
  }

  /** Stop listening to the client and drop every subscriber. */
  dispose(): void {
    this.disposed = true;
    this.listeners.clear();
    this.release();
    for (const p of this.pending.values()) if (p.timer) clearTimeout(p.timer);
    this.pending.clear();
  }

  private attach(): void {
    const offState = this.client.onState((s) => this.onState(s));
    const offClose = this.client.onClose?.(() => {
      this.connected = false;
      this.update();
    });
    this.detach = () => {
      offState();
      offClose?.();
    };
  }

  private release(): void {
    const detach = this.detach;
    this.detach = null;
    detach?.();
  }

  private onState(state: PlayerState): void {
    if (this.disposed) return;
    this.state = state;
    // A settled change the runtime now confirms needs no override any more.
    for (const [key, p] of this.pending) {
      if (p.inflight === 0 && stateValue(state, key) === p.value) this.clear(key);
    }
    this.update();
  }

  private async control(action: PlayerAction, key?: "paused" | "muted", value?: boolean): Promise<void> {
    if (this.disposed) return;
    // Pausing with nothing to read is a no-op in the runtime, so show nothing.
    const shown = key !== undefined && value !== undefined && (key === "muted" || this.current.status !== "idle");
    if (shown) this.hold(key!, value!);
    this.update();
    let ok = true;
    try {
      await this.client.control(action);
    } catch (err) {
      ok = false;
      this.fail(err);
    }
    if (shown) this.settle(key!, ok);
    else if (ok) this.succeed();
    this.update();
  }

  private async setting(key: "volume" | "rate", raw: number, min: number, max: number): Promise<void> {
    if (this.disposed || typeof raw !== "number" || Number.isNaN(raw)) return;
    const value = Math.min(max, Math.max(min, Math.round(raw)));
    this.hold(key, value);
    this.update();
    const sender = this.senders[key];
    sender.next = value;
    if (!sender.running) {
      sender.running = this.drain(key, sender).then((ok) => {
        sender.running = null;
        return ok;
      });
    }
    const ok = await sender.running;
    this.settle(key, ok);
    this.update();
  }

  /** Send the newest value until none is left (values set meanwhile replace each other). */
  private async drain(key: "volume" | "rate", sender: Sender): Promise<boolean> {
    let ok = true;
    while (sender.next !== undefined && !this.disposed) {
      const value = sender.next;
      sender.next = undefined;
      try {
        await this.client.set(key, value);
        ok = true;
        this.succeed();
      } catch (err) {
        ok = false;
        this.fail(err);
      }
    }
    return ok;
  }

  private hold(key: Key, value: boolean | number): void {
    const p = this.pending.get(key);
    if (p) {
      if (p.timer) clearTimeout(p.timer);
      p.timer = null;
      p.value = value;
      p.inflight += 1;
    } else {
      this.pending.set(key, { value, inflight: 1, timer: null });
    }
  }

  /** One call for `key` settled: drop the override once the runtime agrees, or after `settleMs`. */
  private settle(key: Key, ok: boolean): void {
    const p = this.pending.get(key);
    if (!p) return;
    p.inflight = Math.max(0, p.inflight - 1);
    if (p.inflight > 0) return;
    if (!ok || !this.state || stateValue(this.state, key) === p.value) {
      this.clear(key);
      return;
    }
    // The confirming state event may still be on its way (it travels on
    // another connection than the reply). If none comes, the runtime kept
    // its value (a pause with nothing to read): show that.
    p.timer = setTimeout(() => {
      if (this.pending.get(key) === p && p.inflight === 0) {
        this.pending.delete(key);
        this.update();
      }
    }, this.settleMs);
  }

  private clear(key: Key): void {
    const p = this.pending.get(key);
    if (p?.timer) clearTimeout(p.timer);
    this.pending.delete(key);
  }

  private succeed(): void {
    this.error = null;
  }

  private fail(err: unknown): void {
    this.error = err instanceof Error ? err : new Error(String(err));
  }

  private update(): void {
    if (this.disposed) return;
    const overrides: Overrides = {};
    for (const [key, p] of this.pending) (overrides as Record<string, unknown>)[key] = p.value;
    const next = toView(this.state, overrides, this.connected, this.error);
    if (sameView(next, this.current)) return;
    this.current = next;
    for (const cb of [...this.listeners]) cb(next);
  }
}

function stateValue(state: PlayerState, key: Key): boolean | number {
  return state[key];
}
