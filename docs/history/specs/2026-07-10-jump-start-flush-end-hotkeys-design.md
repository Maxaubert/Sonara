# Jump-to-start / flush-to-end hotkeys (+ Ctrl+Alt modifier)

Date: 2026-07-10
Branch context: `deploy/per-session-channels`

## Summary

Add two paired global hotkeys that extend Sonara's directional nav cluster, both
operating on the **engaged session** (the one the user is currently hearing):

- **Ctrl+Alt+Up** - jump to the start of the current turn and replay from the top.
- **Ctrl+Alt+Down** - flush: skip all pending items for that session and go
  silent/idle, keeping the skipped items recoverable via catch-up / repeat.

At the same time, change the Windows default hotkey modifier from `Ctrl+Shift+Alt`
to **`Ctrl+Alt`** (drop Shift) for *all* default bindings.

## Motivation

Today the arrow keys step one item at a time (`Ctrl+Shift+Alt+Left/Right` =
prev/next). There is no single-press way to jump to the very start of a turn or to
skip everything queued and go quiet. `stop` clears *every* session's queue and
`skip` only skips the current utterance; neither is a per-session "I've heard
enough, skip the rest" or "replay this from the top." These two hotkeys fill that
gap and round out the arrow cluster (Up/Down alongside Left/Right).

## Behavior

Both actions target `_engaged_session()` (`router.active or router._last_active or
sessions.foreground()`) - the session the user HEARS - consistent with how
nav/repeat/reread/jump-decision already resolve their target.

### Ctrl+Alt+Up - jump to start (`nav_start`)

Jump to the beginning of the current turn's message history and replay from the
top. **Reuses the existing `_nav(session, "first")`** in `daemon.py`, which already
implements seek-and-play from the first message forward (`to == "first"` sets the
cursor to message 0 and replays that message and every later one). No new playback
logic is written; only the action → message → key wiring is added.

The daemon message is the existing `NAV`: `{"type": "nav", "to": "first"}`.

On success the existing NAV handler plays the `nav` earcon; at an edge / nothing to
navigate it plays `nav_edge` (unchanged behavior).

### Ctrl+Alt+Down - flush to end (`flush`)

Skip all pending items for the engaged session and go silent/idle. Modeled on the
existing `JUMP_DECISION` handler, but instead of advancing the cursor to the next
decision it advances the cursor all the way to `len(ch.items)`.

Steps (in the new `FLUSH_SESSION` handler):

1. Resolve `fg = self._engaged_session()`; if `None`, no-op (optionally an edge
   earcon).
2. If `self._current_item` belongs to `fg`, `self.speaker.cancel()` so the
   in-progress utterance stops (reuse the per-session cancel guard pattern used by
   the existing FLUSH handler: `cur is not None and cur.session == fg`).
3. Advance the channel cursor to the end, dropping each skipped item's heard-marker
   so a later `CATCH_UP` can still replay them and they are not marked heard:

   ```
   ch = self.router.channel(fg)
   while ch.cursor < len(ch.items):
       skipped = ch.items[ch.cursor]
       self._pending_heard.pop(skipped.id, None)
       ch.cursor += 1
   ```
   (After the loop `ch.caught_up()` is True and `ch.has_decision` should be cleared
   to match `SessionChannel.next()`'s own drain semantics.)
4. Fire an earcon (the existing `nav` earcon on a flush that skipped ≥1 item,
   `nav_edge` when there was nothing pending) and `self._wake.set()`.

Skipped items stay **unheard** in `history` (their `_pending_heard` entry is popped
before `note_spoken` could flip it), so `sonara catch_up` / repeat can bring them
back - matching the "non-destructive, recoverable" requirement. Nothing is wiped;
this is a cursor move, not a `wipe()`.

Not debounced - matches the directional nav keys (`_DEBOUNCED_HOTKEYS` covers only
PAUSE/MUTE/NEXT_SESSION/CYCLE_VERBOSITY). A repeated flush press is harmless (the
second press finds nothing pending → edge earcon).

### Modifier change: Ctrl+Shift+Alt → Ctrl+Alt

The default chord modifier is produced by the Windows backend's `default_mods()`
(consumed by `keymap.default_keymap()` for every action). Change it to `Ctrl+Alt`.
This moves **all** default bindings off Shift: Left, Right, Up, Down, M
(mute), P (next_session).

User `keymap.json` overrides are untouched - `load_keymap()` merges user entries
over the defaults, so anyone who set an explicit modifier keeps it.

## Components / files touched

1. **`src/sonara/protocol.py`** - add `FLUSH_SESSION = "flush_session"` to
   `MsgType`. (Up reuses the existing `NAV`.)

2. **`src/sonara/keymap.py`**
   - `ACTION_MESSAGES`: add
     `"nav_start": {"type": "nav", "to": "first"}` and
     `"flush": {"type": "flush_session"}`.
   - `_DEFAULT_KEYS`: add `"nav_start": "up"`, `"flush": "down"`.

3. **`src/sonara/platform/windows/`** - change `default_mods()` to return
   `Ctrl+Alt` (drop Shift). Confirm `up` / `down` exist in the key-code table
   (`keytables.py`); add them if missing.

4. **`src/sonara/daemon.py`** - add the `FLUSH_SESSION` branch to
   `handle_message()` (per "Ctrl+Alt+Down" above). Up needs no daemon change (NAV
   already handles `to == "first"`).

5. **Docs** - `README.md` hotkey table + the per-session section, and
   `commands/keymap.md`. Every `Ctrl+Shift+Alt` reference in `README.md` becomes
   `Ctrl+Alt`; add the two new rows (Up = start of turn, Down = flush to end).

## Naming decisions

- Action names surfaced by `sonara keymap`: **`nav_start`** (Up) and **`flush`**
  (Down).
- **Hotkey-only**, no CLI commands - mirrors `nav_prev` / `nav_next`, which have no
  CLI surface either.

## Testing (TDD - write tests first)

- **protocol**: extend the `MsgType` snapshot test to include `FLUSH_SESSION`
  (mirrors the existing SET_AUDIO_CONTROL / SET_DUCK_LEVEL snapshot coverage).
- **keymap**:
  - `nav_start` resolves to `{"type": "nav", "to": "first"}` and `flush` to
    `{"type": "flush_session"}`.
  - default keys: `nav_start → up`, `flush → down`.
  - default modifier is now `Ctrl+Alt` (no Shift) for a representative action.
- **daemon `FLUSH_SESSION` handler**:
  - after flush, `ch.cursor == len(ch.items)` (nothing pending) for the engaged
    session.
  - the current utterance is cancelled iff `_current_item.session` is the engaged
    session.
  - skipped items remain unheard, so `history.unheard(fg)` still returns them and a
    following `CATCH_UP` replays them.
  - flushing with nothing pending is a safe no-op (edge earcon).
- **wiring**: pressing `nav_start` dispatches `{"type": "nav", "to": "first"}`
  through the same `handle_message` path hotkeys use.

## Out of scope (YAGNI)

- No CLI commands for either action.
- No cross-turn history jump - "start of history" means the start of the current
  turn (history resets each prompt, per `_nav`'s existing contract).
- No changes to `stop` / `skip` / `pause` / `mute` semantics.
- No new earcon wav assets - reuse the existing `nav` / `nav_edge` earcons.
