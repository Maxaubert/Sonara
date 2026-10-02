import type { PlayerState, PlayerView } from "./types.js";

/** Values the controller shows ahead of the runtime while a change is on its way. */
export interface Overrides {
  paused?: boolean;
  muted?: boolean;
  volume?: number;
  rate?: number;
}

/** The view before the first state snapshot. */
export const INITIAL_VIEW: PlayerView = Object.freeze({
  ready: false,
  connected: true,
  status: "idle",
  label: null,
  text: "",
  itemId: null,
  chunk: 0,
  chunks: 0,
  progress: 0,
  queued: 0,
  canPrevious: false,
  canNext: false,
  muted: false,
  volume: 100,
  rate: 200,
  voice: null,
  error: null,
}) as PlayerView;

/** Map a protocol `state` snapshot (plus pending overrides) to the view model. */
export function toView(
  state: PlayerState | null,
  overrides: Overrides,
  connected: boolean,
  error: Error | null,
): PlayerView {
  if (!state) {
    return { ...INITIAL_VIEW, connected, error, ...pick(overrides, ["muted", "volume", "rate"]) };
  }
  const np = state.now_playing;
  const paused = overrides.paused ?? state.paused;
  const chunks = np ? Math.max(0, np.chunks) : 0;
  const chunk = np ? Math.min(Math.max(0, np.chunk), Math.max(0, chunks - 1)) : 0;
  return {
    ready: true,
    connected,
    // Idle wins over paused: the runtime never holds a pause with nothing to read.
    status: !np ? "idle" : paused ? "paused" : "playing",
    label: np ? np.label ?? null : null,
    text: np ? np.text : "",
    itemId: np ? np.item_id : null,
    chunk,
    chunks,
    progress: np && chunks > 0 ? (chunk + 1) / chunks : 0,
    queued: state.queued,
    canPrevious: !!np && chunk > 0,
    canNext: !!np && (chunk + 1 < chunks || state.queued > 0),
    muted: overrides.muted ?? state.muted,
    volume: overrides.volume ?? state.volume,
    rate: overrides.rate ?? state.rate,
    voice: state.voice,
    error,
  };
}

/** Shallow equality of two views (every field is a primitive or an Error). */
export function sameView(a: PlayerView, b: PlayerView): boolean {
  const keys = Object.keys(a) as (keyof PlayerView)[];
  return keys.every((k) => Object.is(a[k], b[k]));
}

function pick(o: Overrides, keys: (keyof Overrides)[]): Overrides {
  const out: Overrides = {};
  for (const k of keys) if (o[k] !== undefined) (out as Record<string, unknown>)[k] = o[k];
  return out;
}
