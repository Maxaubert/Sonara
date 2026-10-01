# Review Fixes Design (2026-06-17)

Fixes for the 22-finding multi-agent review. Scope is the confirmed, actionable
findings; speculative/negative findings are deferred with rationale (see end).

## Goal

Stop the queue from going silent mid-response, stop spurious skipping, and
repair the two regressions introduced by the recent keymap-reload feature, plus
a batch of robustness fixes - all without changing the protocol wire format.

## Root-cause summary

The two user symptoms ("queue cleared mid-response", "occasional skipping") are
rooted in voice-ownership / foreground gating in `daemon.py`, not in the
decision/permission paths (those append at the tail and drop nothing). The
keymap-reload regressions are from this session's own work.

## Fixes

### H1 - Voice owner stranded mid-response (primary symptom)

`_voice_owner` is released the instant the queue drains to 0 (`note_spoken`,
speak-loop empty branch). Between streamed PROSE chunks of ONE reply the deque
routinely hits 0, so ownership is released mid-message. If another session flips
foreground in that gap (`SET_FOREGROUND`), the original owner fails `_may_speak`
and the rest of its reply is captured to history but never enqueued - it halts.

**Fix:** track sessions with an assistant prose message in flight in
`self._open_msg: set[str]`. Mark on a non-final PROSE; clear on PROSE `final`,
on the Stop turn-boundary (the `turn_done` earcon now carries `session`), on
FLUSH and on SESSION_END. Gate BOTH release sites: release `_voice_owner` only
when the queue is empty AND the owner has no open message. Ownership is held
across inter-chunk drains and released cleanly at the turn boundary.

### H2 - Hotkey reload can dark all Windows hotkeys (regression)

`_reload_hotkeys` calls `stop()` then immediately `start()`. `WinHotkeyBackend.
stop()` signals the pump but does not join it, so the new `RegisterHotKey` calls
collide (GetLastError 1409) with the still-registered chords, get dropped, and
then the old thread's `finally` unregisters everything - all hotkeys dead until
a daemon restart.

**Fix:** `WinHotkeyBackend.stop()` joins the pump thread (bounded timeout) after
posting WM_QUIT, so the old thread's `finally` releases every chord before
`start()` re-registers.

### M2 - Cancel lost in the pop→speak gap (TOCTOU)

The speak loop claims an item under `_lock`, releases it, then `speak()` reads
its cancel baseline under a DIFFERENT lock. A `cancel()` landing in the gap bumps
the epoch first, so `speak()` reads the already-bumped value as its baseline and
plays the cancelled utterance in full.

**Fix:** capture `speaker.cancel_epoch()` inside the claim block (under `_lock`)
and pass it into `speak(text, cancel_epoch=...)` as the baseline. A cancel in the
gap now bumps past the captured baseline and is detected.

### M4 - Decision dropped when another session owns the voice

CHOICE/PLAN/PERMISSION gate on `_may_speak`; if a background session owns the
voice, the earcon fires but the option text is never enqueued.

**Fix:** a decision for the FOREGROUND session reassigns `_voice_owner` to it and
clears its captured flag before enqueuing, so the question is always spoken in the
window the user is looking at.

### M6 - JUMP_DECISION leaks heard-markers (latent; handler unreachable)

`queue.jump_to_decision()` discards queued non-decision items without dropping
their `_pending_heard` entries or marking the cancelled current item heard.

**Fix:** return the dropped list, `_drop_pending` it, and mark the current item
heard (mirrors SKIP). Cheap correctness even though no hotkey currently binds it.

### M7 - keymap reload is a silent no-op on macOS (regression)

On macOS the hotkey backend has no start/stop, the CLI never rewrites the
resolved file, and the Swift hotkeyd reads it once at launch - so `keymap clear`
is not applied live, contrary to the shipped claim.

**Fix:** add a `HotkeyBackend.reload(dispatch)` seam. Default = `stop()` then
`start(dispatch)` (Windows, now safe via H2). macOS overrides it to rewrite the
resolved keymap (`write_resolved()`) and `launchctl kickstart -k` the hotkeyd so
it re-reads the file. `_reload_hotkeys` calls the seam (respecting the
no_hotkeys kill switch). Command help text corrected to match real behavior.

### M8 - Connection-semaphore permit leak on spawn failure

`_spawn_conn_handler` acquires a permit then starts the handler thread; if
`Thread.start()` raises, the permit is never released and capacity bleeds to
zero.

**Fix:** wrap create+start; on failure release the permit and close the conn.

### M9 - `sonari doctor` crashes on malformed settings.json

`settings_has_sonari_hooks` / `settings_has_sonari_plugin` assume nested dict/list
shapes; a hand-malformed settings.json (e.g. `hooks` a list) raises instead of
returning False.

**Fix:** isinstance guards on every level and a containing try/except so any
shape yields False, never an exception.

### M10 - sonari-hook.cmd silent-mute risk

`pythonw "%~dp0sonari-hook" %* 2>nul` hardcodes `pythonw` and routes stderr to
nul; if `pythonw` is absent/wrong the hook fails invisibly and the daemon goes
mute.

**Fix:** resolve `pythonw`, fall back to the windowless `pyw -3` launcher, and
append stderr to `~/.sonari/hook.log` instead of discarding it. Still always
`exit /b 0` (a hook must never fail loudly).

### L2 - PAUSE re-queue TOCTOU resurrects a flushed item

The `not completed and self._paused.is_set()` check runs outside `_lock`, then
`enqueue_front` runs unconditionally inside it - a FLUSH racing in between can
resurrect a flushed item.

**Fix:** move the `_paused.is_set()` re-check inside `_lock`; only `enqueue_front`
while still paused, else fall through to `note_spoken`.

### L3 - Live prose dropped during nav (multi-session)

`_nav` cancels + re-enqueues history for the foreground session but does not make
it the voice owner, so subsequent live deltas can be captured instead of spoken
when a background session owns the voice.

**Fix:** `_nav` sets `_voice_owner` to the navigating foreground session and
clears its captured flag.

## Deferred (with rationale)

- **M3 (FLUSH overtakes trailing prose):** needs an end-to-end turn id in the
  MessageDisplay payload; the stateless hook can't supply one, and a daemon-only
  counter cannot tell a late old-turn delta from a new-turn delta. A half-fix
  would be fragile. Defer until a protocol turn-id is justified.
- **M5 (decision interrupt-class):** reordering/cancelling queued prose to
  front-run a decision risks the exact "skipping" the user dislikes. M4 already
  guarantees delivery; FIFO (prose, then the question that follows it) matches
  natural reading order.
- **L1 (`_wake` lost wakeup):** latency only (≤ the 0.1s poll), not correctness;
  a Condition rewrite of the hot loop carries more risk than the payoff.
- **L4:** subsumed by H1 - holding ownership during an open message is the
  intended resolution of "SET_FOREGROUND doesn't release the outgoing owner".
- **L5 / L6 / L7:** dead-config and negative/doc findings; no code action.

## Post-implementation review (adversarial, 7 agents over the diff)

Two confirmed findings, both addressed:

1. **`_nav` seized the voice unconditionally** - could strand a different session
   still streaming, inconsistent with the conservative M4 rule. Fixed: nav now
   claims only a free/stale/own voice (`owner == session or owner not in
   _open_msg`).
2. **Reload `join()` ran while holding the daemon `_lock`** the hotkey pump thread
   needs to dispatch - a hotkey firing during a reload could stall the daemon and,
   on join timeout, re-create the H2 dark-hotkey race. Fixed: RELOAD_KEYMAP runs
   the reload on a short-lived thread off the lock, serialized by `_reload_lock`.

## Testing

Unit tests per fix (TDD). Key new tests: two-session strand (H1), Windows
stop()-joins-before-start (H2), cancel-in-gap (M2), decision-while-background-owns
(M4), malformed-settings doctor (M9), pause/flush race (L2), nav-sets-owner (L3).
Full platform-independent suite must stay green; the known Windows env failure
(`test_bin_shims` shim exec) is unchanged.
