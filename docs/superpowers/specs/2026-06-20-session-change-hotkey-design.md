# Session-change hotkey (replaces pin)

Status: approved design. Removes the pin feature and adds a `next_session` hotkey
that cycles the active reader between sessions on demand.

## 1. Motivation

The pin feature is being removed. Diagnostic logging proved why it is unfixable in
its current shape: each pin calls `router.repin_reset()` which resets the channel
cursor to 0, and because confirmation cues ("Pinned."/"Auto.") are stored in the
channel at the cursor, every reset **replays stale cues** — one press echoed 2-3
times and old cues resurfaced seconds later (`pin_audit.log` showed one TOGGLE
producing three SPOKE lines, plus phantom cues with no toggle). The cursor-reset
replay is intrinsic to how pin pins/replays, so we remove pin entirely and rely on
auto mode, adding a manual session-change control that fits the cursor model
cleanly.

## 2. Behavior

A single new hotkey action, **`next_session`** (default **Ctrl+Alt+P**, the key pin
freed). On each press the daemon moves the active reader to another session:

- **Pass 1 — next unread.** Scan sessions in round-robin order starting *after* the
  current active (wrapping), and switch to the first **other** session that has
  unread content (`channel.pending() > 0`). Resume it **from its cursor** (continue
  where it left off — manual switch bypasses the minqueue gate; any pending counts).
  Announce **"Session changed: {folder}."** + the session-change chime.
- **Pass 2 — revisit a read session.** If no other session is unread, switch to the
  next **read** (caught-up) session, **reset its cursor to 0** (replay from start),
  and announce **"Session changed: {folder}, reading again."** + chime.
- **Pass 3 — nowhere to go.** If there is no other session at all, emit a soft cue
  **"No other session."** (the existing `repeat` hotkey already replays the current
  session, so we do not duplicate that here).

**Read vs unread** is the channel's existing state — no new flag:
- unread = `channel.pending() > 0` (cursor before the end)
- read   = `channel.caught_up()` (cursor at the end; the session was fully heard)

**Leaving the current session** does nothing to it: its cursor stays put, so it
remains unread and round-robin-reachable on a later press. A session becomes "read"
only by being fully heard (the speak loop draining its cursor to the end), never by
being left.

## 3. How the switch sticks

The handler sets `router.active = target` directly and calls `speaker.cancel()` for
an immediate switch. The router's existing rule — *"the current reader keeps the
floor until its channel drains"* (`_pick`) — then holds the voice on the target
until it is done, after which normal auto handoff resumes (foreground-first, then
oldest-waiting). No persistent lock, no `repin_reset`.

Rejected alternatives: a sticky "forced session" flag (a lighter pin — reintroduces
the lock/state complexity we are removing); reordering the per-session queues
(heavier, discards the clean cursor model).

## 4. Components

- **`Router`** gains `next_session() -> (target|None, replay: bool)`: pure
  round-robin selection (pass 1 then pass 2 over `self.channels` in insertion
  order, starting after `self.active`), resets the target's cursor when `replay`,
  sets `self.active = target`, and arms the announcement (reusing the existing
  `_pending_announce` + `session_change` item path, with a "reading again" variant
  when `replay`). Returns `(None, False)` when there is no other session.
- **`Router`** loses `repin_reset()`, the `pinned()` checks in `_pick`/`next_item`,
  and any pin bookkeeping.
- **`SessionManager`** loses `pin_toggle()`, `_pinned`, `pinned()`.
- **`daemon.py`**: replace the `PIN_TOGGLE` handler with a `NEXT_SESSION` handler
  (compute via `router.next_session()`, `speaker.cancel()`, emit the "No other
  session." cue on `None`); drop `_DEBOUNCED_HOTKEYS`' `PIN_TOGGLE` entry and add
  `NEXT_SESSION` (debounced — it is a control toggle, a rapid double-tap should be
  one switch).
- **`protocol.py`**: remove `PIN_TOGGLE`, add `NEXT_SESSION = "next_session"`.
- **keymap**: rebind the default `Ctrl+Alt+P` action from `pin_toggle` to
  `next_session`; remove the `pin_toggle` action from the action tables.
- **announce_text**: extend so the router can request the "reading again" variant
  (e.g. pass a flag, or a second formatter).

## 5. Edge cases

- **Single session, mid-read:** pass 1 finds no *other* unread, pass 2 finds no
  *other* read → "No other session." (use `repeat` to replay the current one).
- **Switch target drains, auto resumes:** after the manually-selected session is
  fully heard, `_pick` falls back to foreground-first auto — expected.
- **Muted (global):** `next_session` still switches and announces (the chime +
  announcement are mute_exempt like other control cues); prose stays muted.
- **Paused:** unchanged — the loop is held; the switch takes effect on resume.
- **Target session ends (SESSION_END) before being served:** `router.drop` already
  clears it; `next_session` re-scans live channels each press.

## 6. Testing

- **`Router.next_session`** (pure): next-unread selection with wrap; pass-2
  read-revisit with cursor reset + replay flag; `(None, False)` for no-other; does
  not pick the current session in pass 1; round-robin order from `active`.
- **Daemon-level:** switch-and-resume (continue from cursor, normal announcement);
  revisit-replay ("reading again" + chime + cursor reset); "No other session." cue;
  `NEXT_SESSION` is debounced.
- **Removal:** the pin tests are deleted; no test asserts on `pin_toggle`/`_pinned`/
  `repin_reset`/`PIN_TOGGLE` after this change.
- **Integration (multisession):** A reading, press next → B resumes with chime +
  announcement; press again with both read → wrap + "reading again".

## 7. Rollout

Core daemon + keymap change (cross-platform). Single hotkey rebinding (no new key).
Verify live on Windows by replaying the multi-session flow before merge. Remove the
temporary pin/hotkey diagnostic instrumentation (already reverted) as part of this.
