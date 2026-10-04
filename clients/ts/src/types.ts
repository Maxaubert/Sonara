/** Queue mode of `speak`: after everything queued, or drop the unread items first. */
export type SpeakMode = "append" | "replace";

/** Core playback controls (protocol v1, `control`). */
export type ControlAction =
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

/** Core setting keys (protocol v1, `set` / `get`). */
export type SettingKey = "volume" | "rate" | "voice" | "engine";

export interface SpeakOptions {
  /** `append` (default) or `replace` (drop unread items, never the current one). */
  mode?: SpeakMode;
  /** Also cut the current item and start this one now. */
  interrupt?: boolean;
  /** Shown in `state.now_playing.label`. */
  label?: string;
}

export interface NowPlaying {
  item_id: number;
  label: string | null;
  /** The chunk being read. */
  text: string;
  chunk: number;
  chunks: number;
  [extra: string]: unknown;
}

/** A `state` event: a full snapshot, sent on change only. */
export interface State {
  seq: number;
  now_playing: NowPlaying | null;
  queued: number;
  paused: boolean;
  muted: boolean;
  volume: number;
  rate: number;
  voice: string | null;
  engine_status: EngineStatus;
  [extra: string]: unknown;
}

/**
 * `state.engine_status`. Protocol 1.1 adds readiness: Kokoro is `loading`,
 * `downloading` (with `progress`), `waiting` to retry or `unavailable`
 * until it is `ready`, and names the engine speaking meanwhile in
 * `fallback`. A 1.0 runtime sends only `engine`.
 */
export interface EngineStatus {
  engine: string;
  ready?: boolean;
  status?: "ready" | "loading" | "downloading" | "waiting" | "unavailable";
  progress?: { done: number; total: number };
  fallback?: string;
  message?: string;
  /**
   * Protocol 1.2: why an external engine is not speaking itself
   * (`no_key`, `auth`, `quota`, `rate_limited`, `network`, `timeout`,
   * `server`, `bad_voice`, `bad_config`, `format`).
   */
  reason?: string;
  [extra: string]: unknown;
}

export type ItemPhase = "started" | "finished" | "skipped" | "failed";

/** An `item` event. */
export interface ItemEvent {
  item_id: number;
  phase: ItemPhase;
  [extra: string]: unknown;
}

/** A `log` event. */
export interface LogEvent {
  message: string;
  [extra: string]: unknown;
}

export interface Voice {
  id: string;
  name: string;
  language: string;
  engine: string;
  license_class: "permissive" | "os" | "external";
  installed: boolean;
  [extra: string]: unknown;
}

/** The `hello` reply: what the runtime offers. */
export interface HelloInfo {
  version: string;
  protocol: { major: number; minor: number };
  capabilities: string[];
  extensions: string[];
  /** Requested extensions this runtime lacks. */
  unavailable: string[];
  [extra: string]: unknown;
}

/** Contents of `runtime.json` (protocol v1, Discovery). */
export interface RuntimeInfo {
  pid: number;
  port: number;
  http_port: number;
  token: string;
  version: string;
  protocol: { major: number; minor: number };
  capabilities: string[];
  started_at: string;
  [extra: string]: unknown;
}

/** Any reply with `ok: true`; the fields depend on the request. */
export type Reply = Record<string, unknown>;
