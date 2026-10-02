/**
 * What the player needs from a client: the core API of `@sonara/client`
 * (`control`, `set`, `onState`, optionally `onClose`). A `SonaraClient` fits
 * as is; so does any object that forwards these calls, such as a bridge from
 * a browser renderer to a client in a Node or Electron main process.
 */
export interface PlayerClient {
  control(action: PlayerAction): Promise<unknown>;
  set(key: "volume" | "rate", value: number): Promise<unknown>;
  /** Every `state` snapshot; the first is the current state. Returns an unsubscribe function. */
  onState(cb: (state: PlayerState) => void): () => void;
  /** Called once the client lost its runtime. */
  onClose?(cb: () => void): () => void;
}

/** The `control` actions the player sends (protocol v1 core). */
export type PlayerAction =
  | "play"
  | "pause"
  | "toggle"
  | "stop"
  | "skip"
  | "previous"
  | "next"
  | "restart"
  | "mute"
  | "unmute";

/** The fields of a protocol v1 `state` event the player reads. */
export interface PlayerState {
  now_playing: {
    item_id: number;
    label: string | null;
    text: string;
    chunk: number;
    chunks: number;
  } | null;
  queued: number;
  paused: boolean;
  muted: boolean;
  volume: number;
  rate: number;
  voice: string | null;
}

export type PlayerStatus = "idle" | "playing" | "paused";

/** The player's view model: everything a player UI renders. */
export interface PlayerView {
  /** A state snapshot has arrived. Before that the values are defaults. */
  ready: boolean;
  /** False once the client reported its runtime gone. */
  connected: boolean;
  status: PlayerStatus;
  /** The current item's label (`speak`'s `label`), `null` without one or when idle. */
  label: string | null;
  /** The chunk (sentence) being read, `""` when idle. */
  text: string;
  /** Id of the current item, `null` when idle. */
  itemId: number | null;
  /** Index of the chunk being read (0-based). */
  chunk: number;
  /** Chunks in the current item, 0 when idle. */
  chunks: number;
  /** Position in the current item, 0..1: `(chunk + 1) / chunks`, 0 when idle. */
  progress: number;
  /** Items waiting after the current one. */
  queued: number;
  /** `previous` steps back a chunk (false on the first one; restart covers that). */
  canPrevious: boolean;
  /** `next` steps forward a chunk or on to the next queued item. */
  canNext: boolean;
  muted: boolean;
  /** Percent, 0..100. */
  volume: number;
  /** Words per minute, 100..400. */
  rate: number;
  voice: string | null;
  /** The last failed action, cleared by the next one that succeeds. */
  error: Error | null;
}

export type PlayerListener = (view: PlayerView) => void;
