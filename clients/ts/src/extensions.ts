/**
 * Extension namespaces (spec sections 4.2 to 4.4). Each method sends one
 * protocol message as is; all behaviour lives in the runtime. A runtime that
 * does not offer the extension (or no client enabled it in `hello`) answers
 * `E_UNSUPPORTED`. `extra` fields are sent along unchanged, for fields a later
 * protocol minor adds.
 */
import type { ControlAction, Reply, SpeakOptions } from "./types.js";

export type Send = (type: string, fields: Record<string, unknown>) => Promise<Reply>;
type Extra = Record<string, unknown>;

/** Drop undefined fields so the runtime sees only what the caller set. */
function defined(fields: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(fields)) if (v !== undefined) out[k] = v;
  return out;
}

export type ChannelPolicy = "latest" | "queue";

export interface ChannelOpenOptions {
  label?: string;
  host_tab?: string;
  policy?: ChannelPolicy;
}

/** `channels`: named sources, each with its own queue and policy. */
export class ChannelsApi {
  constructor(private readonly send: Send) {}

  /** `channel_open`. */
  open(channel: string, opts: ChannelOpenOptions = {}, extra: Extra = {}): Promise<Reply> {
    return this.send("channel_open", defined({ ...extra, ...opts, channel }));
  }

  /** `channel_close`. */
  close(channel: string, extra: Extra = {}): Promise<Reply> {
    return this.send("channel_close", defined({ ...extra, channel }));
  }

  /** `focus`: bring a channel to the front. */
  focus(channel: string, extra: Extra = {}): Promise<Reply> {
    return this.send("focus", defined({ ...extra, channel }));
  }

  /** `speak` on a channel; resolves with the item id. */
  async speak(channel: string, text: string, opts: SpeakOptions = {}, extra: Extra = {}): Promise<number> {
    const r = await this.send("speak", defined({ ...extra, ...opts, text, channel }));
    return r.item_id as number;
  }

  /** `control` scoped to a channel. */
  async control(channel: string, action: ControlAction | "next_channel", extra: Extra = {}): Promise<void> {
    await this.send("control", defined({ ...extra, action, channel }));
  }

  /** `control` `next_channel`. */
  async nextChannel(extra: Extra = {}): Promise<void> {
    await this.send("control", defined({ ...extra, action: "next_channel" }));
  }

  /** `control` `flush`: stop only the session being read (the flush hotkey, #228). */
  flush(extra: Extra = {}): Promise<Reply> {
    return this.send("control", defined({ ...extra, action: "flush" }));
  }
}

export type AskKind = "question" | "permission" | "plan";

export interface StreamMessage {
  channel: string;
  turn: string | number;
  delta: string;
  index: number;
  final: boolean;
  /** Sender start time, so late text from a previous turn is dropped. */
  t?: number;
  [extra: string]: unknown;
}

/** `agent` (needs `channels`): streaming turns, decisions, earcons. */
export class AgentApi {
  constructor(private readonly send: Send) {}

  /** `stream`: one delta of a turn's text. */
  stream(msg: StreamMessage): Promise<Reply> {
    return this.send("stream", defined({ ...msg }));
  }

  /** `turn_start`. */
  turnStart(channel: string, turn: string | number, extra: Extra = {}): Promise<Reply> {
    return this.send("turn_start", defined({ ...extra, channel, turn }));
  }

  /** `turn_end`. */
  turnEnd(channel: string, turn: string | number, extra: Extra = {}): Promise<Reply> {
    return this.send("turn_end", defined({ ...extra, channel, turn }));
  }

  /** `ask`: a question, permission or plan, spoken with priority. */
  ask(channel: string, kind: AskKind, text: string, options?: unknown[], extra: Extra = {}): Promise<Reply> {
    return this.send("ask", defined({ ...extra, channel, kind, text, options }));
  }

  /** `earcon`. */
  earcon(kind: string, extra: Extra = {}): Promise<Reply> {
    return this.send("earcon", defined({ ...extra, kind }));
  }

  /** `set mute_level` 0, 1 or 2. */
  setMuteLevel(level: 0 | 1 | 2): Promise<Reply> {
    return this.send("set", { key: "mute_level", value: level });
  }

  /** `set summaries`. */
  setSummaries(value: Record<string, unknown>): Promise<Reply> {
    return this.send("set", { key: "summaries", value });
  }
}

export type AudioMode = "duck" | "pause" | "off";

/** `system` (Windows): other apps' audio, global hotkeys, the settings page. */
export class SystemApi {
  constructor(private readonly send: Send) {}

  /** `set audio_mode`. */
  setAudioMode(mode: AudioMode): Promise<Reply> {
    return this.send("set", { key: "audio_mode", value: mode });
  }

  /** `set duck_level`. */
  setDuckLevel(level: number): Promise<Reply> {
    return this.send("set", { key: "duck_level", value: level });
  }

  /** `set hotkeys`. */
  setHotkeys(hotkeys: Record<string, unknown>): Promise<Reply> {
    return this.send("set", { key: "hotkeys", value: hotkeys });
  }

  /** `get settings_url`. */
  async settingsUrl(): Promise<string> {
    const r = await this.send("get", { key: "settings_url" });
    return r.value as string;
  }
}
