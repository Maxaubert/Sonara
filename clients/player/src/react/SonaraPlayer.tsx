import { useMemo } from "react";
import type { CSSProperties, ReactElement } from "react";
import { PlayerController } from "../controller.js";
import type { PlayerClient, PlayerView } from "../types.js";
import { usePlayerView } from "./hooks.js";
import { Icon } from "./icons.js";
import type { IconName } from "./icons.js";
import { PLAYER_CSS } from "./styles.js";

/** Every text the player shows or announces, for translation. */
export interface PlayerLabels {
  player: string;
  play: string;
  pause: string;
  previous: string;
  next: string;
  restart: string;
  stop: string;
  mute: string;
  volume: string;
  progress: string;
  idle: string;
  reading: string;
  paused: string;
  connecting: string;
  disconnected: string;
  /** Spoken position, for example "Sentence 2 of 5" (`chunk` is 1-based). */
  position: (chunk: number, chunks: number) => string;
}

export const DEFAULT_LABELS: PlayerLabels = {
  player: "Sonara player",
  play: "Play",
  pause: "Pause",
  previous: "Previous sentence",
  next: "Next sentence",
  restart: "Restart",
  stop: "Stop",
  mute: "Mute",
  volume: "Volume",
  progress: "Progress",
  idle: "Nothing playing",
  reading: "Reading",
  paused: "Paused",
  connecting: "Connecting",
  disconnected: "Not connected",
  position: (chunk, chunks) => `Sentence ${chunk} of ${chunks}`,
};

export interface SonaraPlayerProps {
  /** A connected `@sonara/client` (or any `PlayerClient`). Give this or `controller`. */
  client?: PlayerClient;
  /** A controller you created, to share it with other UI. */
  controller?: PlayerController;
  /** `auto` (default) follows `prefers-color-scheme`. */
  theme?: "auto" | "light" | "dark";
  /** Leave out the built-in stylesheet (style the `sonara-player__*` classes yourself). */
  unstyled?: boolean;
  labels?: Partial<PlayerLabels>;
  className?: string;
  style?: CSSProperties;
}

/**
 * A compact player bar: restart, previous, play/pause, next, stop, the
 * now-playing label with the sentence being read and its progress, mute
 * and volume. Every control is a native button or slider in reading
 * order; a control that does not apply right now stays focusable and
 * says so with `aria-disabled`, so focus never jumps away.
 */
export function SonaraPlayer(props: SonaraPlayerProps): ReactElement {
  const { client, controller, theme = "auto", unstyled = false, className, style } = props;
  const own = useMemo(
    () => (controller ? null : client ? new PlayerController(client) : null),
    [client, controller],
  );
  const player = controller ?? own;
  if (!player) throw new TypeError("SonaraPlayer needs a client or a controller");
  const view = usePlayerView(player);
  const labels = useMemo(() => ({ ...DEFAULT_LABELS, ...props.labels }), [props.labels]);

  const live = view.connected && view.ready;
  const hasItem = live && view.status !== "idle";
  const playing = view.status === "playing";
  const classes = ["sonara-player", className].filter(Boolean).join(" ");

  return (
    <div
      role="group"
      aria-label={labels.player}
      className={classes}
      style={style}
      data-theme={theme === "auto" ? undefined : theme}
      data-status={view.status}
    >
      {unstyled ? null : <style>{PLAYER_CSS}</style>}
      <div className="sonara-player__group">
        <Button icon="restart" label={labels.restart} enabled={live} onPress={() => player.restart()} />
        <Button icon="previous" label={labels.previous} enabled={live && view.canPrevious} onPress={() => player.previous()} />
        <Button
          icon={playing ? "pause" : "play"}
          label={playing ? labels.pause : labels.play}
          enabled={hasItem}
          primary
          onPress={() => player.toggle()}
        />
        <Button icon="next" label={labels.next} enabled={live && view.canNext} onPress={() => player.next()} />
        <Button icon="stop" label={labels.stop} enabled={hasItem} onPress={() => player.stop()} />
      </div>
      <NowPlaying view={view} labels={labels} />
      <div className="sonara-player__group">
        <Button
          icon={view.muted ? "muted" : "volume"}
          label={labels.mute}
          pressed={view.muted}
          enabled={live}
          onPress={() => player.toggleMute()}
        />
        <input
          type="range"
          className="sonara-player__volume"
          min={0}
          max={100}
          step={1}
          value={view.volume}
          aria-label={labels.volume}
          aria-valuetext={`${view.volume}%`}
          aria-disabled={live ? undefined : true}
          onChange={(e) => {
            if (live) void player.setVolume(Number(e.currentTarget.value));
          }}
        />
      </div>
      {/* Always in the DOM: a live region that appears with its text
          already in it goes unannounced in several screen readers. */}
      <p className="sonara-player__error" role="status">
        {view.error ? view.error.message : ""}
      </p>
    </div>
  );
}

function NowPlaying({ view, labels }: { view: PlayerView; labels: PlayerLabels }): ReactElement {
  let title: string;
  let status: string | null = null;
  if (!view.connected) title = labels.disconnected;
  else if (!view.ready) title = labels.connecting;
  else if (view.status === "idle") title = labels.idle;
  else {
    title = view.label || labels.reading;
    if (view.status === "paused") status = labels.paused;
  }
  const active = view.connected && view.ready && view.status !== "idle";
  const position = active ? view.chunk + 1 : 0;
  return (
    <div className="sonara-player__now">
      {/* Announced on change: the item and whether it is paused, never each sentence. */}
      <p className="sonara-player__title" aria-live="polite" aria-atomic="true">
        <span className="sonara-player__label">{title}</span>
        {status ? <span className="sonara-player__status">{status}</span> : null}
      </p>
      <p className="sonara-player__text" title={active ? view.text : undefined}>
        {active ? view.text : " "}
      </p>
      <div className="sonara-player__meter">
        <div
          className="sonara-player__progress"
          role="progressbar"
          aria-label={labels.progress}
          aria-valuemin={0}
          aria-valuemax={Math.max(1, view.chunks)}
          aria-valuenow={position}
          aria-valuetext={active ? labels.position(position, view.chunks) : labels.idle}
        >
          <div className="sonara-player__progress-fill" style={{ transform: `scaleX(${Math.round(view.progress * 1000) / 1000})` }} />
        </div>
        <span className="sonara-player__position" aria-hidden="true">
          {active ? `${position} / ${view.chunks}` : ""}
        </span>
      </div>
    </div>
  );
}

interface ButtonProps {
  icon: IconName;
  label: string;
  enabled: boolean;
  onPress: () => void;
  pressed?: boolean;
  primary?: boolean;
}

function Button({ icon, label, enabled, onPress, pressed, primary }: ButtonProps): ReactElement {
  return (
    <button
      type="button"
      className={primary ? "sonara-player__button sonara-player__button--primary" : "sonara-player__button"}
      aria-label={label}
      aria-pressed={pressed}
      aria-disabled={enabled ? undefined : true}
      onClick={() => {
        if (enabled) onPress();
      }}
    >
      <Icon name={icon} />
    </button>
  );
}
