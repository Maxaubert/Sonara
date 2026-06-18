# Session Pin Focus — design (#31)

**Status:** approved design, pre-plan
**Issue:** nimkimi/sonari#31

## Goal

A single global hotkey ("pin") that locks Sonari's voice to the session the user
is currently working in, so it keeps speaking that session even when other
sessions submit prompts. Pressing it again on that same session unpins (back to
auto). This is how an eyes-free user with several Claude sessions open directs the
voice to the one they care about.

## Problem

Today the speaking ("foreground") session is simply the last one to submit a
prompt or start (`SET_FOREGROUND` / `SESSION_START`). With multiple sessions open,
a background session's prompt steals the voice, and there is no way to lock it to
a chosen session.

## Why not OS window focus

A Claude session is a process behind a socket, not tied 1:1 to an OS window.
Tabbed terminals (Windows Terminal, VS Code integrated terminal, tmux) host many
sessions inside **one** OS window/process, and the OS exposes no queryable "which
tab is active" that maps back to a session. So Sonari cannot auto-detect "the tab
you're looking at." It *can* pin a specific session reliably, because each session
(each tab) has a unique session id that Sonari already tracks. The pin therefore
sticks to that exact tab's session; only the *selection* is manual (the hotkey),
not OS-driven.

## Behavior

- **Default state: auto.** Nothing pinned -> foreground = last session to submit a
  prompt / start (today's behavior, unchanged).
- **Pin toggle** (`pin_toggle` action; default chord+`P` = **P**in, see Keybind. To free `P`, `pause` moved to `S`. NOT `F`: macOS chord is Ctrl+Cmd and Ctrl+Cmd+F is the system "Enter Full Screen" shortcut):
  - Let `cur` = the current foreground session at the moment of the press.
  - `cur is None` (no active session) -> announce "No active session"; no state change.
  - A session is already pinned **and** it equals `cur` -> **unpin** (-> auto); announce "Auto".
  - Otherwise -> **pin** `cur`; announce "Pinned <folder>" (folder = cwd basename).
  - Net effect: press `P` in a tab to pin it; press `P` again in that same tab to
    unpin; press `P` while a different session is active to move the pin there.
- **While pinned:** a `SET_FOREGROUND` from any *other* session does not change who
  speaks. The pinned session owns the voice.
- **Pinned session ends** (`SESSION_END` / unregister of the pinned session) ->
  fall back to **auto**.

## Session identity / announcement

- Sessions are announced by **folder name** = `os.path.basename(cwd)`.
- `cwd` is available in the Claude Code hook payload (`payload.get("cwd")`), already
  parsed in `hooks_entry.py`. It is added to the `SESSION_START` and
  `SET_FOREGROUND` messages and stored in `SessionManager`.
- Fallback if `cwd` is absent (older hook / missing field): announce a generic
  label ("a session"); the pin still works (it keys on session id, not folder).

## Architecture / seams (branch off merged `main`)

The key insight: **a pin overrides what "foreground" means.** `_may_speak` and
`_claim_for_decision` already gate on `sessions.is_foreground(session)`. Making the
effective foreground be `pinned if pinned else _foreground` means the voice-
ownership logic needs **no changes** — the pin flows through the existing gate.

### `src/sonari/sessions.py` (SessionManager)
- Replace `_sessions: set[str]` with an **insertion-ordered** `dict[str, str]`
  (session id -> cwd basename). Ordering is stable and future-proofs a list/cycle.
- Add `_pinned: str | None` (None = auto).
- `register(session, cwd=None)` and `set_foreground(session, cwd=None)` record the
  cwd basename when provided (never overwrite a known basename with an empty one).
- `pin_toggle() -> tuple[str, str | None]`: applies the toggle rules above against
  the current `_foreground`; returns `(action, folder)` with
  `action in {"pinned", "unpinned", "none"}` for the daemon to announce.
- `effective_foreground()` / `is_foreground(session)` honor the pin:
  effective = `_pinned if _pinned is not None else _foreground`.
- `unregister(session)`: if `_pinned == session`, clear the pin (-> auto). Existing
  `_foreground` clearing is unchanged.
- `pinned() -> str | None` accessor (for STATUS / tests).

### `src/sonari/protocol.py`
- New `MsgType.PIN_TOGGLE = "pin_toggle"` (hotkey -> daemon). `PROTOCOL_VERSION`
  stays 1 (additive; unknown types are already ignored by older peers).

### `src/sonari/daemon.py`
- Handle `PIN_TOGGLE`: call `sessions.pin_toggle()`, then enqueue an announcement
  through the existing speech queue:
  - `"pinned"`   -> earcon + "Pinned <folder>"
  - `"unpinned"` -> earcon + "Auto"
  - `"none"`     -> error earcon only (no spoken text)
- The "pinned"/"unpinned" announcement is enqueued as owned by the **effective
  foreground after the toggle** (which is always a real session: `cur`), so the
  existing `_may_speak` gate lets it through — it is a system confirmation the user
  must hear. The "none" case has no session to speak through, so it is an earcon
  only (the press still gives audible feedback that nothing was pinnable).
- `SET_FOREGROUND` / `SESSION_START` handlers: pass `cwd` from the message payload
  through to `sessions.set_foreground(... , cwd=...)` / `register`.
- No change to `_may_speak` / `_claim_for_decision` bodies — they already key on
  `is_foreground`, which now honors the pin.

### `src/sonari/keymap.py` + hotkey backends
- New action `pin_toggle`. Default binding chord+`P` (macOS Ctrl+Cmd+P / Windows
  Ctrl+Shift+Alt+P); `pause` moved from `P` to `S` to free it. Routed like the other hotkey actions: the
  backend sends a `PIN_TOGGLE` message to the daemon.
- Windows in-process hotkey backend + macOS hotkeyd both register it.
- `keymap <action> clear` unbind already covers the new action generically.

### `src/sonari/hooks_entry.py` (the cwd addition)
- Add `cwd=payload.get("cwd", "")` to the `SESSION_START` message and to the
  `SET_FOREGROUND` messages. One field; backward-compatible (absent -> "").

## Edge cases

- One session open: `P` pins it; `P` again unpins. Harmless.
- Zero sessions: `P` -> error earcon, no-op (nothing to speak through).
- Pinned session ends while another is foreground -> auto resumes following the
  last prompt.
- Two tabs in the same folder: both announce the same folder name, but the pin
  keys on the distinct session id, so the correct tab is pinned even though the
  spoken name is ambiguous. (A numeric disambiguator is a possible later polish,
  out of scope here.)
- Rapid double-press: deterministic — second press sees the state the first left.

## Testing (TDD)

- **SessionManager:** ordered registration; `pin_toggle` pins current foreground;
  toggle-again on the same session unpins; pressing while another session is
  foreground moves the pin; `is_foreground`/`effective_foreground` honor the pin;
  `unregister` of the pinned session -> auto; cwd basename recorded and not clobbered
  by a later empty cwd.
- **daemon:** `PIN_TOGGLE` enqueues the correct announcement per action; while
  pinned, a `SET_FOREGROUND` from another session does not change who speaks; the
  pinned session's prose still speaks; pinned `SESSION_END` -> auto; `cwd` from the
  message reaches the SessionManager.
- **keymap:** `pin_toggle` is a known action with the default binding, is
  clearable, and routes to a `PIN_TOGGLE` message.
- **hotkey backends:** `skipif` the Win32 real-pump tests; macOS resolution uses
  `opt`/`option`, never hardcoded `alt`.
- Pure-core tests run on both OSes; platform pump tests are guarded.

## Out of scope (this PR)

- Cycle / list-sessions hotkeys (dropped per product decision — single `P` only).
- OS window/tab auto-detection (infeasible for tabbed terminals; documented above).
- A `sonari sessions` CLI listing command (optional, low priority; can follow).

## Ownership / CONTRIBUTING

- Shared core (`sessions` / `daemon` / `protocol` / `hooks_entry`) + both hotkey
  backends -> **both approve**.
- New default keybind (chord+`P`) + moving `pause` to `S` is a **behavior change** -> raise with Nima before
  finalizing the default. The user's personal binding is separate (local keymap).
- `skipif` cross-OS tests; `opt` not `alt` on macOS; per-platform human acceptance.
