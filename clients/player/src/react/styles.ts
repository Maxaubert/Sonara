/**
 * The player's stylesheet. Every colour, the radius and the font come from
 * a `--sonara-player-*` custom property with a light and a dark default, so
 * a host themes the player by setting those properties on it or on any
 * ancestor. Selectors use `:where()` (zero specificity), so a host's own
 * rules always win. `unstyled` leaves this sheet out.
 */
export const PLAYER_CSS = `
:where(.sonara-player) {
  --_bg: var(--sonara-player-bg, #fbfbfa);
  --_fg: var(--sonara-player-fg, #1c1c1f);
  --_muted: var(--sonara-player-muted-fg, #5d5d66);
  --_accent: var(--sonara-player-accent, #2f56c9);
  --_on-accent: var(--sonara-player-on-accent, #ffffff);
  --_track: var(--sonara-player-track, #e1e1e6);
  --_border: var(--sonara-player-border, #d8d8de);
  --_hover: var(--sonara-player-hover, #ececf0);
  --_focus: var(--sonara-player-focus, #2f56c9);
  --_danger: var(--sonara-player-danger, #b42318);
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 12px;
  box-sizing: border-box;
  min-height: 52px;
  padding: 6px 10px;
  border: 1px solid var(--_border);
  border-radius: var(--sonara-player-radius, 10px);
  background: var(--_bg);
  color: var(--_fg);
  color-scheme: light;
  font: var(--sonara-player-font, 14px/1.35 system-ui, -apple-system, "Segoe UI", sans-serif);
}
@media (prefers-color-scheme: dark) {
  :where(.sonara-player:not([data-theme="light"])) {
    --_bg: var(--sonara-player-bg, #1c1c20);
    --_fg: var(--sonara-player-fg, #ececf0);
    --_muted: var(--sonara-player-muted-fg, #a4a4ae);
    --_accent: var(--sonara-player-accent, #8ea7ff);
    --_on-accent: var(--sonara-player-on-accent, #10131c);
    --_track: var(--sonara-player-track, #35353d);
    --_border: var(--sonara-player-border, #34343b);
    --_hover: var(--sonara-player-hover, #2a2a31);
    --_focus: var(--sonara-player-focus, #8ea7ff);
    --_danger: var(--sonara-player-danger, #ff8a80);
    color-scheme: dark;
  }
}
:where(.sonara-player[data-theme="dark"]) {
  --_bg: var(--sonara-player-bg, #1c1c20);
  --_fg: var(--sonara-player-fg, #ececf0);
  --_muted: var(--sonara-player-muted-fg, #a4a4ae);
  --_accent: var(--sonara-player-accent, #8ea7ff);
  --_on-accent: var(--sonara-player-on-accent, #10131c);
  --_track: var(--sonara-player-track, #35353d);
  --_border: var(--sonara-player-border, #34343b);
  --_hover: var(--sonara-player-hover, #2a2a31);
  --_focus: var(--sonara-player-focus, #8ea7ff);
  --_danger: var(--sonara-player-danger, #ff8a80);
  color-scheme: dark;
}
:where(.sonara-player) *, :where(.sonara-player) *::before, :where(.sonara-player) *::after {
  box-sizing: border-box;
}
:where(.sonara-player__group) {
  display: flex;
  align-items: center;
  gap: 2px;
}
:where(.sonara-player__button) {
  display: inline-grid;
  place-items: center;
  width: 36px;
  height: 36px;
  margin: 0;
  padding: 0;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: inherit;
  cursor: pointer;
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  transition: background-color 120ms ease, color 120ms ease, opacity 120ms ease, transform 80ms ease-out;
}
:where(.sonara-player__button:hover:not([aria-disabled="true"])) {
  background: var(--_hover);
}
:where(.sonara-player__button:active:not([aria-disabled="true"])) {
  transform: scale(0.94);
}
:where(.sonara-player__button[aria-disabled="true"]) {
  opacity: 0.38;
  cursor: default;
}
:where(.sonara-player__button--primary) {
  width: 40px;
  height: 40px;
  border-radius: 999px;
  background: var(--_accent);
  color: var(--_on-accent);
}
:where(.sonara-player__button--primary:hover:not([aria-disabled="true"])) {
  background: var(--_accent);
  filter: brightness(1.08);
}
:where(.sonara-player__button[aria-pressed="true"]) {
  color: var(--_accent);
}
:where(.sonara-player__button:focus-visible, .sonara-player__volume:focus-visible) {
  outline: 2px solid var(--_focus);
  outline-offset: 2px;
}
:where(.sonara-player__now) {
  flex: 1 1 12rem;
  min-width: 0;
  display: grid;
  gap: 2px;
}
:where(.sonara-player__title) {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin: 0;
  min-width: 0;
  font-weight: 600;
}
:where(.sonara-player__label) {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
:where(.sonara-player__status) {
  flex: none;
  font-size: 0.8em;
  font-weight: 500;
  color: var(--_muted);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
:where(.sonara-player__text) {
  margin: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--_muted);
}
:where(.sonara-player__meter) {
  display: flex;
  align-items: center;
  gap: 8px;
}
:where(.sonara-player__progress) {
  position: relative;
  flex: 1;
  height: 4px;
  border-radius: 2px;
  background: var(--_track);
  overflow: hidden;
}
:where(.sonara-player__progress-fill) {
  position: absolute;
  inset: 0;
  border-radius: inherit;
  background: var(--_accent);
  transform-origin: left center;
  transition: transform 240ms ease-out;
}
:where(.sonara-player__position) {
  flex: none;
  min-width: 3.5em;
  text-align: right;
  font-size: 0.8em;
  color: var(--_muted);
  font-variant-numeric: tabular-nums;
}
:where(.sonara-player__volume) {
  width: 96px;
  margin: 0 4px;
  accent-color: var(--_accent);
  cursor: pointer;
}
:where(.sonara-player__volume[aria-disabled="true"]) {
  opacity: 0.38;
  cursor: default;
}
:where(.sonara-player__error) {
  flex-basis: 100%;
  margin: 0;
  font-size: 0.85em;
  color: var(--_danger);
}
:where(.sonara-player__error:empty) {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip-path: inset(50%);
}
:where(.sonara-player__icon) {
  display: block;
}
@media (prefers-reduced-motion: reduce) {
  :where(.sonara-player__button, .sonara-player__progress-fill) {
    transition: none;
  }
  :where(.sonara-player__button:active:not([aria-disabled="true"])) {
    transform: none;
  }
}
@media (forced-colors: active) {
  :where(.sonara-player__progress) {
    border: 1px solid CanvasText;
  }
  :where(.sonara-player__progress-fill) {
    background: Highlight;
  }
  :where(.sonara-player__button--primary) {
    border: 1px solid ButtonText;
  }
}
`;
