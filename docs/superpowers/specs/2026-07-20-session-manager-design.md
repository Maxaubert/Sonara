# Session Manager - Design

Date: 2026-07-20
Status: proposed

## Goal

A "Sessions" tab in the Sonara settings page that lists every known Claude Code
session and lets the user, per session:

- **Name** it (a custom label that replaces the folder name everywhere Sonara
  speaks about the session, e.g. "Session changed: my build box.")
- **Mute** it (that session's speech is silenced; the rest keep speaking)
- **Voice** it (a per-session voice override; other sessions keep the default)
- **Forget** it (remove a stale session from the list)

## Why it fits the existing architecture

Most of the plumbing already exists:

- `SessionChannel.muted` exists and the Router already honors it
  (`Router._ready` refuses a muted channel except for mute-exempt cues).
  Nothing sets it today.
- `Speaker.speak(..., voice=...)` already accepts a per-utterance voice
  override (fast cues use it via `_cue_voice_override`).
- `SessionManager` already tracks session id -> cwd folder name, persists it to
  `~/.sonara/sessions.json` (cap 200), and feeds the "Session changed: {folder}"
  announcement.
- The settings page is a sidebar of pages (`speech`, `summary`, `audio`,
  `hotkeys`, `advanced`, `system`); a `sessions` page slots straight in.

## Components

### 1. `session_prefs.py` (new module)

`SessionPrefs`: a durable map `session id -> {"name": str, "muted": bool,
"voice": str}` persisted to `~/.sonara/session_prefs.json`. Mirrors
`sessions.py`'s storage discipline: opt-in `store_path` (tests stay pure),
atomic best-effort writes (every failure swallowed), cap 200 most-recent
entries, corrupt/missing file tolerated. Accessors `name()`, `muted()`,
`voice()`; mutator `set(session, key, value)` (falsy name/voice clears the
key; name capped at 60 chars); `forget(session)` drops the entry.

Prefs survive daemon restarts and session restarts: if the same session id
comes back, its prefs still apply. Entries die only via Forget or cap
eviction.

### 2. Daemon wiring

- `SpeechDaemon.__init__` gains `prefs=None` (defaults to a memory-only
  `SessionPrefs()`, same pattern as `pauser`); `main()` passes
  `SessionPrefs(store_path=SESSION_PREFS_PATH)`.
- New protocol ops:
  - `SET_SESSION_PREF` `{session, key: name|muted|voice, value}`: writes the
    pref. For `muted` it also applies to the live channel immediately and, if
    the muted session is the one currently speaking, cancels the current
    utterance.
  - `FORGET_SESSION` `{session}`: unregisters the session, drops its prefs,
    and removes its channel. Refused for the current foreground session.

### 3. Announcements use the custom name

`Router` gains a `display_name` resolver (daemon passes
`prefs.name(sid) or sessions.folder(sid)`); the hand-off announcement becomes
"Session changed: {custom name or folder or 'another session'}."

### 4. Mute semantics (matches global Muted, level 1)

- A muted session's speech is silent: digests, live prose, questions.
- Attention earcons (permission/question beeps) still fire, so a muted
  session can still get your attention without a voice.
- Mute-exempt control cues still speak.
- New channels are seeded from prefs on creation (`channel_init` hook on the
  Router), so a mute set while the session is idle sticks when it next talks.
- Interlocks so mute never yields confusing silence:
  - the next-session hotkey's round-robin skips muted sessions (unless every
    other session is muted);
  - catch-up's "other session" picker skips muted sessions.

### 5. Per-session voice

At speak time the voice resolves in priority order:

1. fast-cue override (control cues + session-change announcements keep the
   warm cue voice, unchanged);
2. the session's voice pref, if set;
3. the global default voice.

Any engine's voice can be a per-session override (Kokoro, Chatterbox,
Windows); the existing per-voice routing and fallback chains in `tts.run`
apply unchanged. An override naming a voice that is later uninstalled falls
back through the normal chain (no error surfaced).

### 6. Settings page "Sessions" tab

New sidebar entry Sessions between Audio and Hotkeys. Each session renders as
a row:

- **Name**: text input, placeholder = folder name; blur/Enter saves.
- **Subtitle**: folder name + first 8 chars of the session id; an "Active"
  badge on the foreground session; a pending-items count when non-zero.
- **Mute**: toggle switch.
- **Voice**: dropdown, "Default" + the installed voices grouped by engine
  (same data the Speech page uses).
- **Forget**: removes the row (hidden on the foreground session).

The list is rebuilt from `/api/state` (new `sessions` array) whenever the tab
is opened and after every mutation (`/api/session` POST returns fresh state).

### 7. Web API

- `GET /api/state` gains `"sessions": [{id, folder, name, muted, voice,
  foreground, pending}]`.
- `POST /api/session` with `{id, key, value}` (prefs) or `{id, op: "forget"}`.
  Dispatches through `daemon.handle_message` under the daemon lock, exactly
  like `/api/set`.

## Error handling

- Prefs persistence is best-effort: a failed write never breaks message
  handling (mirrors `sessions.py`).
- Unknown pref keys, non-string session ids, junk values: the op is ignored
  (daemon) / 400 (web API).
- Forgetting the foreground session is refused (400).

## Testing

Pytest throughout, following the existing suites:

- `tests/test_session_prefs.py`: store round-trip, cap, corruption, clearing.
- Daemon tests: SET_SESSION_PREF applies mute to a live channel + cancels
  current speech; voice override reaches `speaker.speak`; announcement uses
  the custom name; FORGET_SESSION cleans all three stores; interlock tests
  (cycle + catch-up skip muted).
- Webui tests: sessions in state; /api/session happy path + validation.

## Out of scope (YAGNI)

- Per-session rate/verbosity/summary-style.
- CLI subcommand for session prefs (the page and hotkeys cover it).
- Reordering or grouping sessions in the UI.
