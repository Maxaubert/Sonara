# Session-change hotkey (replaces pin)

Status: approved design. Removes the pin feature and adds a `next_session` hotkey
that cycles the active reader between sessions on demand.

## 1. Motivation

The pin feature is being removed. Diagnostic logging proved why it is unfixable in
its current shape: each pin calls `router.repin_reset()` which resets the channel
cursor to 0, and because confirmation cues ("Pinned."/"Auto.") are stored in the
channel at the cursor, every reset **replays stale cues** - one press echoed 2-3
times and old cues resurfaced seconds later (`pin_audit.log` showed one TOGGLE
producing three SPOKE lines, plus phantom cues with no toggle). The cursor-reset
replay is intrinsic to how pin pins/replays, so we remove pin entirely and rely on
auto mode, adding a manual session-change control that fits the cursor model
cleanly.

## 2. Behavior

A single new hotkey action, **`next_session`** (default **Ctrl+Alt+P**, the key pin
freed). It is a **pure round-robin** over all registered sessions in a **fixed
order** (channel insertion order) - the ring never reorders based on queue state,
so "press N times to reach session X" stays predictable. On each press:

- **Advance one slot.** Move the active reader to the **next session after the
  current one** in the fixed ring, wrapping around. With a single session it lands
  on itself.
- **Landing on a read session** (`channel.caught_up()`): **reset its cursor to 0**
  and replay from the start. Announce **"Session changed: {folder}, reading
  again."** + the session-change chime.
- **Landing on an unread session** (`channel.pending() > 0`): resume it **from its
  cursor** (continue where it left off; the manual switch bypasses the minqueue
  gate - any pending counts). Announce **"Session changed: {folder}."** + chime.
- **No session registered at all** (no channels yet): a soft cue **"No session."**

**Read vs unread** is the channel's existing state - no new flag:
- unread = `channel.pending() > 0` (cursor before the end)
- read   = `channel.caught_up()` (cursor at the end; the session was fully heard)

**Leaving the current session** does nothing to it: its cursor stays put, so it
remains unread and reachable on the next time around the ring. A session becomes
"read" only by being fully heard (the speak loop draining its cursor to the end),
never by being left.

**Fixed order:** sessions occupy the ring in the order their channels were first
created (first output). New sessions append at the end; ended sessions drop out.
The order does not shuffle by read/unread state.

## 3. How the switch sticks

The handler sets `router.active = target` directly and calls `speaker.cancel()` for
an immediate switch. The router's existing rule - *"the current reader keeps the
floor until its channel drains"* (`_pick`) - then holds the voice on the target
until it is done, after which normal auto handoff resumes (foreground-first, then
oldest-waiting). No persistent lock, no `repin_reset`.

Rejected alternatives: a sticky "forced session" flag (a lighter pin - reintroduces
the lock/state complexity we are removing); reordering the per-session queues
(heavier, discards the clean cursor model).

## 4. Components

- **`Router`** gains `next_session() -> (target|None, replay: bool)`: pure
  round-robin selection - the next session after `self.active` in `self.channels`
  insertion order (excluding the CONTROL channel), wrapping; with one session it
  returns that session. `replay` is true when the target was caught-up (it then
  resets the target's cursor). Sets `self.active = target` and arms the
  announcement (reusing the existing `_pending_announce` + `session_change` item
  path, with a "reading again" variant when `replay`). Returns `(None, False)` only
  when there are no channels at all.
- **`Router`** loses `repin_reset()`, the `pinned()` checks in `_pick`/`next_item`,
  and any pin bookkeeping.
- **`SessionManager`** loses `pin_toggle()`, `_pinned`, `pinned()`.
- **`daemon.py`**: replace the `PIN_TOGGLE` handler with a `NEXT_SESSION` handler
  (compute via `router.next_session()`, `speaker.cancel()`, emit the "No other
  session." cue on `None`); drop `_DEBOUNCED_HOTKEYS`' `PIN_TOGGLE` entry and add
  `NEXT_SESSION` (debounced - it is a control toggle, a rapid double-tap should be
  one switch).
- **`protocol.py`**: remove `PIN_TOGGLE`, add `NEXT_SESSION = "next_session"`.
- **keymap**: rebind the default `Ctrl+Alt+P` action from `pin_toggle` to
  `next_session`; remove the `pin_toggle` action from the action tables.
- **announce_text**: extend so the router can request the "reading again" variant
  (e.g. pass a flag, or a second formatter).

## 5. Edge cases

- **Single session:** the ring lands on itself - if read, replay ("reading
  again"); if mid-read (unread), it resumes from the cursor (no audible jump, since
  it is already the active reader). `repeat` remains the way to force a replay of a
  mid-read message.
- **No channels yet** (hotkey pressed before any output): "No session." cue.
- **Switch target drains, auto resumes:** after the manually-selected session is
  fully heard, `_pick` falls back to foreground-first auto - expected.
- **Muted (global):** `next_session` still switches and announces (the chime +
  announcement are mute_exempt like other control cues); prose stays muted.
- **Paused:** unchanged - the loop is held; the switch takes effect on resume.
- **Target session ends (SESSION_END) before being served:** `router.drop` already
  clears it; `next_session` re-scans live channels each press.

## 6. Testing

- **`Router.next_session`** (pure): advances one slot in fixed insertion order
  (wrapping); resumes an unread target (no cursor reset); resets + replays a read
  target (replay=True); single session lands on itself; `(None, False)` when there
  are no channels.
- **Daemon-level:** switch-and-resume (continue from cursor, normal announcement);
  revisit-replay ("reading again" + chime + cursor reset); "No session." cue;
  `NEXT_SESSION` is debounced.
- **Removal:** the pin tests are deleted; no test asserts on `pin_toggle`/`_pinned`/
  `repin_reset`/`PIN_TOGGLE` after this change.
- **Integration (multisession):** A reading, press next → B resumes with chime +
  announcement; press again with both read → wrap + "reading again".

## 7. Rollout

Core daemon + keymap change (cross-platform). Single hotkey rebinding (no new key).
Verify live on Windows by replaying the multi-session flow before merge. Remove the
temporary pin/hotkey diagnostic instrumentation (already reverted) as part of this.
