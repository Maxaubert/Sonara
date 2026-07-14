# Per-session audio channels

Status: approved design (issue #59). Replaces the daemon's global speech queue +
single voice-owner with per-session channels driven by a router.

## 1. Problem & root cause

The daemon shares **global** state across sessions: one `SpeechQueue` (items
tagged by session), one `_paused` `Event`, one `_voice_owner`. Diagnostic logging
(`queue_audit.log`) during a two-session repro showed the failure:

- Session B paused. Pause is global, so the speak loop froze. The speak loop is
  also the *only* thing that releases `_voice_owner` on drain - so ownership
  stayed pinned to B. When session A then streamed a reply, `_may_speak(A)`
  returned False (owner == B) and A's prose was **captured** (recorded, never
  spoken). Result: A was silent with no way to recover except resuming B.
- Pausing one session silences all sessions (global `_paused`).
- Background sessions' speech is captured into history, not kept as a replayable
  queue - "pin an old session to hear it" is impossible because its speech was
  never queued.
- Pause/mute act on `sessions.foreground()`, which bounces between sessions on
  every prompt, so repeated toggles act on different targets ("says muted twice").

These are all symptoms of one architectural fault: **audio state is global when
the desired model is per-session.**

## 2. Goals / non-goals

**Goals**
- Each session has an independent, replayable queue. One session can never wipe,
  silence, or steal another's audio.
- Auto mode: cooperative hand-off between sessions, announced by folder.
- Pin mode: only the pinned session reads; re-pin replays a session from the start.
- Pause = simple, reliable full-silence + resume. Mute = per-session, consistent.

**Non-goals**
- Mixing/playing two sessions simultaneously (there is one speaker; one reader at
  a time).
- Changing the TTS engine, hooks, or hotkey bindings.
- Cross-machine sync.

## 3. Architecture

Two new units replace `SpeechQueue` + the `_voice_owner`/`_open_msg`/`_captured_msg`
machinery.

### `SessionChannel` (one per session) - pure data, unit-testable

| Field | Meaning |
|---|---|
| `items: list[SpeechItem]` | the session's **current message**, appended as it streams |
| `cursor: int` | index of the next item to speak; items `< cursor` are already spoken. Items are **not discarded** - that is what makes replay-from-start possible |
| `turn_done: bool` | the message is complete (final PROSE / turn_done earcon) |
| `muted: bool` | per-session mute |

Methods (pure): `append(item)`, `pending()` = `len(items) - cursor`,
`ready(minqueue)` = `pending() > 0 and (pending() >= minqueue or turn_done)` (there
is a batch worth reading now), `caught_up()` = `pending() == 0`, `next()` (return
`items[cursor]`, advance cursor), `reset()` (cursor→0, replay), `wipe()`
(items=[], cursor=0, turn_done=False - a new prompt).

### `Router` - decides the active reader, drives the one speaker

Holds `active: session | None` and consults `SessionManager` for pin/foreground.

- **Pin** (`sessions.pinned() is not None`): `active` = the pinned session, always.
  Re-pinning (handled in the pin_toggle path) calls `channel.reset()` so the newly
  pinned session replays from the start.
- **Auto** (`pinned is None`): the current `active` keeps reading while its channel
  is `ready()`. When the active channel is **not** `ready()` (caught up, or pending
  below minqueue and not yet turn_done - i.e. idle), the router picks the next
  reader among channels that are `ready()` and not `muted`, **foreground first,
  then oldest-waiting**. On a change of `active`, the router emits a one-item
  **"Session changed: {folder}."** announcement before that channel's items.
- **Decisions preempt**: a channel that has received a decision item (choice / plan
  / permission) becomes `active` at the next item boundary even if another session
  is mid-message - decisions are user-blocking. (The alert earcon already fires
  immediately and cross-session, unchanged.)
- If no channel is `ready()`, the speaker idles.

### Speak loop (rewritten, same locking discipline)

```
while running:
    if paused: wait; continue
    item = router.next_item(minqueue)   # under lock: may switch active + yield a
                                        # "Session changed" cue, or None
    if item is None: wait; continue
    speak(item)                         # outside lock; cancel-epoch as today
```

`router.next_item` runs under `self._lock` (same as the current pop+claim), returns
the next `SpeechItem` to speak (an announcement cue, or the active channel's `next()`),
or None when everything is idle/paused.

## 4. Behaviors

- **New prompt** (UserPromptSubmit → FLUSH): `channel[session].wipe()`. No other
  channel is touched. If `session` was the active reader, the router re-evaluates.
- **Prose / tool / decision** (per session): append to that session's channel.
  minqueue batching is expressed by `ready()` (read only when a batch exists or the
  turn is done), so the standalone `_prose_buffer` goes away.
- **turn_done / final**: set `channel.turn_done = True`, flushing the sub-minqueue
  remainder into readability.
- **Pause**: one `_paused` flag halts the speak loop (full silence). Resume continues
  from each channel's cursor. Nothing is lost because channels persist.
- **Mute**: the mute hotkey toggles **the active reader's** channel `muted` (not the
  bouncing foreground). The "Session muted/unmuted" confirmation is spoken even when
  muting (an exempt cue). A muted channel is skipped by the router in auto.
- **Pin / re-pin**: `pin_toggle` pins/unpins the last-prompt session; on a *change*
  of pinned target the new channel resets its cursor (replay). "Pinned {folder}." /
  "Auto." cues unchanged.

## 5. What is removed / replaced

- `SpeechQueue` (global deque) → per-session `SessionChannel`s + `Router`.
- `_voice_owner`, `_may_speak`, `_claim_for_decision`, `_captured_msg`, `_open_msg`,
  `_owner_mid_reply` → subsumed by the router's active-reader logic.
- `_prose_buffer` (minqueue) → channel `ready()` batching.
- `_nav_cursor` + nav/repeat/catch_up → adapted to read from channels/history; see §7.

## 6. Edge cases

- **Flush the active reader**: wiping the active channel mid-read → router falls
  through to the next ready channel (or idle), no crash, no stuck speaker.
- **Decision while another session reads**: preempt to the decision's session; the
  interrupted reader's cursor is preserved and resumes after.
- **All channels muted / caught up**: speaker idles silently.
- **Single-session use**: exactly one channel; auto never hands off; behavior is
  identical to today (no announcements, no regressions). This is the common case and
  must stay byte-for-byte in feel.
- **Session ends**: drop its channel; if it was active, re-evaluate.

## 7. Nav / repeat / catch-up (scope note)

The channel holds the current message with a cursor, which overlaps with what
`history` + `_nav_cursor` provide. To bound risk, the first implementation keeps
`history` for past-message nav/repeat and routes **live** reading through channels;
`catch_up` ("hear another session") is largely superseded by re-pin but is kept
working. Folding nav fully into channels is a follow-up, not part of this change.

## 8. Testing

- **`SessionChannel`** (pure): append/pending/ready/caught_up/next/reset/wipe,
  minqueue threshold, turn_done flush, cursor never loses items.
- **`Router`** (pure, fake clock-free): auto hand-off selection (foreground-first,
  oldest-waiting), mute-skip, decision-preempt, pin lock, re-pin reset, idle.
- **Daemon-level** (reproduce the bug report): pause B → A still reachable and
  speaks; submit A then B → both heard in turn, nothing lost; re-pin replays from
  start; single-session unchanged.
- The existing suite must stay green (single-session paths unchanged in feel).

## 9. Rollout

Core daemon change (cross-platform). Develop on a branch off `main`; verify the
multi-session flows live on Windows by cherry-picking onto the running daemon
branch and restarting. Remove the temporary `queue_audit.log` instrumentation as
part of landing.
