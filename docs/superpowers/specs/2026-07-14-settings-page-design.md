# Sonara Settings Page — Design Spec

Date: 2026-07-14. Status: approved direction, pending user review of this document.
Visual reference: `docs/superpowers/mockups/codex-b.html` (chosen base), with
`mockup-b.html` and the others kept for comparison.

## Goal

A local browser settings page for Sonara so everyday configuration stops requiring
CLI commands. Optional: the CLI keeps full parity; the page is a friendlier skin
over the same daemon messages.

## Architecture

- **Daemon-served.** The daemon grows a second listener: a small HTTP server
  (Python stdlib `http.server.ThreadingHTTPServer`) bound to `127.0.0.1` on a
  **pinned port** (new config key `settings_port`, default `27431`; falls back
  to ephemeral if taken), recorded in `daemon.lock` as `http_port`. A pinned
  port keeps bookmarks valid and lets the page reconnect across daemon
  restarts.
- **Three endpoints:**
  - `GET /settings` — the page itself (one self-contained HTML file shipped in
    the package, no external resources).
  - `GET /api/state` — JSON: current config values, installed voices grouped by
    engine, engine install status, keymap bindings, daemon status (pid, port,
    uptime, foreground session).
  - `POST /api/set` — `{key, value}`; dispatches through the SAME
    `handle_message` paths the CLI uses (SET_VOICE, SET_RATE, SET_MINQUEUE,
    SET_SUMMARY_MODE, SET_AUDIO_CONTROL, SET_DUCK_LEVEL, keymap writes +
    RELOAD_KEYMAP, SHUTDOWN). No new mutation logic; the page can never drift
    from CLI behavior. Response = fresh state.
  - `POST /api/preview` — speaks a short sample sentence in a named voice
    without changing config (routes to the speaker as a control cue).
- **Security:** every request must carry the token already stored in
  `daemon.lock` (`?token=` query or `X-Sonara-Token` header). Requests without
  it: 403. Binding is localhost-only. This blocks other local users and
  malicious web pages (DNS-rebinding/CSRF) from poking the daemon.
- **Web thread isolation:** the HTTP handler only builds protocol messages and
  calls `handle_message` under the daemon's existing lock, exactly like a TCP
  client. It never touches the speaker, engines, or channels directly.

## Entry points

- `sonara settings` CLI command: reads `daemon.lock`, opens the default browser
  at the tokenized URL. Errors with the standard "not running, run: sonara
  start" hint when the daemon is down.
- `/sonara:settings` slash command wrapping the same.
- `sonara status` prints the URL (convenience; the port changes across daemon
  restarts, so the command is the canonical entry).

## Feature surface (all approved)

Families: **Speech, Summary, Audio, Hotkeys, System.** Instant apply everywhere
(no Save button) with the mockup's "Saved" state dot. Verbosity is deliberately
NOT on the page (stays CLI-only). No playback controls, no live status strip.

- **Speech:** voice picker grouped by engine (Windows / Kokoro / Chatterbox)
  with per-voice ▶ preview (POST /api/preview); speaking rate slider 120–350
  wpm; minimum queue stepper 1–10.
- **Summary:** summary mode toggle; digest model select (haiku/sonnet); digest
  timeout; settle window.
- **Audio:** "Audio duck" toggle (config key stays `audio_control`); duck level
  slider 0–100; synthesis chunk size slider 80–280. No VRAM/idle-unload knobs.
- **Hotkeys:** full editing. Each action row shows its combo as key chips;
  click → "press keys" capture state → writes `keymap.json` via the existing
  keymap module → sends RELOAD_KEYMAP. Per-row unbind (✕). Conflict = replace
  with warning toast. Esc cancels capture.
- **System:** daemon card (status, PID, port, uptime). **Restart** = bare
  SHUTDOWN message (no stop sentinel): the supervisor respawns the daemon and
  the page reconnects via its state polling on the pinned port. **Shut down** =
  a new SHUTDOWN variant that also writes the stop sentinel (mirrors `sonara
  shutdown`); the page then shows the disconnect banner until `sonara start`.
  engine status rows (Kokoro / Chatterbox: Installed or Not installed with the
  `sonara voices install <engine>` command shown for copy-paste — status only,
  no install buttons); version footer.

## Visual design (from codex-b.html)

- macOS-System-Settings-style framed window on a soft tinted backdrop: narrow
  left nav pane (search field, colored icon tiles per section — indigo Speech,
  orange Summary, green Audio, gray Hotkeys/System), content pane right with
  inset grouped lists.
- Font stack `"Segoe UI Variable","Segoe UI",-apple-system,sans-serif`; accent
  `#4F46E5`; rounded 14–18px cards; iOS-style toggles; light/dark theme toggle
  (persisted in localStorage).
- Helper text under every control (keep codex-b's copy quality).
- The search field filters/hides nav sections and highlights matching rows.
- Open question for review: keep or drop the decorative (non-functional)
  traffic-light window dots.

## Failure behavior

- Daemon stops while the page is open: api calls fail → banner "Sonara isn't
  running — run `sonara start`", controls disabled until reconnect (page polls
  /api/state every 3s and recovers automatically).
- Invalid values clamped by the existing handlers (page mirrors the clamps).
- Wrong/missing token: 403 page explaining to relaunch via `sonara settings`.

## Testing

- Unit: HTTP handler auth (403 without token), /api/state shape, /api/set
  dispatch-to-handle_message mapping, preview routing, lockfile http_port field.
- Playwright e2e: start a daemon against a scratch config dir, open the page,
  flip summary mode + change rate + rebind a hotkey, assert config.json and
  STATUS reflect it; kill daemon, assert the disconnect banner appears.

## Out of scope (explicitly)

Verbosity control, voice-pack install/remove actions, playback remote control,
live "now speaking" status, remote (non-localhost) access, multi-user auth.
