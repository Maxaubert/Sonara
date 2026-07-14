# Sonari Phase 2 - Control & Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Global hotkeys + 100% eyes-free option selection (native numeric) for Claude Code, screen off.

**Architecture:** A tiny Swift `hotkeyd` (Carbon RegisterEventHotKey, no macOS permission) registers Ctrl+Cmd combos and writes the mapped JSON control message to the existing speechd Unix socket; ALL keymap logic (key/mod-name → virtual-key-code/Carbon-mask, action → socket message) lives in Python and is unit-tested; speechd gains small additive ops. Phase 1 pipeline untouched.

**Tech Stack:** Python 3 stdlib + pytest (daemon/hooks/keymap/cli), Swift (swiftc, no deps) for hotkeyd, macOS LaunchAgents, Unix-domain-socket newline-JSON protocol.

---

## How to run / conventions (read once)

- **Repo root:** `/Users/Nima.Hakimi/projects/private/claude-tts`. All paths below are relative to it.
- **Tests run with:** `.venv/bin/python -m pytest` (the repo has a venv at `./.venv`; there is no system pytest). Run from the repo root.
- **Test imports:** new daemon tests import `from tests.daemon_helpers import make_daemon` and use `FakeSpeaker` (records `.spoken/.earcons/.cancels/.rates/.voices`). `make_daemon(verbosity="everything", foreground="fg")` returns `(daemon, queue, speaker, sessions, config)`. The config is a deep copy of `DEFAULTS` with `verbosity` overridden.
- **Path monkeypatching pattern:** mirror `tests/test_config.py::_patch_config_paths` - patch the *module-level constant on the module under test* (e.g. `monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")`), not `sonari.paths`.
- **Protocol version stays `PROTOCOL_VERSION = 1`** - every change in this plan is additive.
- **Each task ends with an explicit `git add <exact files>` + `git commit`.** This repo is a git repo; work on the current branch.
- **swiftc:** present at `/usr/bin/swiftc` on this Mac; Swift tests/builds skip gracefully when absent.

## File Structure (what each task creates/modifies)

| File | Responsibility | Task(s) |
|---|---|---|
| `src/sonari/protocol.py` | add `REREAD_OPTIONS`, `CYCLE_VERBOSITY` MsgTypes | 1 |
| `tests/test_protocol.py` | assert new constants; keep the exhaustive set tests in sync | 1 |
| `src/sonari/daemon.py` | relative-rate, cycle_verbosity, option cache + reread, selection cue/notes | 2,3,4,5 |
| `tests/test_daemon_phase2.py` | all new daemon-behavior tests | 2,3,4,5 |
| `src/sonari/paths.py` | `KEYMAP_PATH`, `HOTKEYD_RESOLVED_PATH`, `HOTKEYD_BIN_PATH` | 6 |
| `tests/test_paths.py` | assert the three new path constants | 6 |
| `src/sonari/keymap.py` | ALL key/mod/action resolution + load/write logic (the brain) | 7 |
| `tests/test_keymap.py` | unit tests for resolve/load/write | 7 |
| `hotkeyd/sonari-hotkeyd.swift` | the Swift global-hotkey daemon (thin; reads resolved JSON) | 8 |
| `tests/test_hotkeyd_swift.py` | compile the Swift; assert resolved JSON shape matches Swift's expectations | 8 |
| `src/sonari/cli.py` | hotkeyd install/uninstall/doctor + `keymap` subcommand | 9,10 |
| `tests/test_cli_hotkeyd.py` | plist, build, doctor checks | 9 |
| `tests/test_hotkeyd_contract.py` | every ACTION_MESSAGES dict is a valid speechd command | 9 |
| `commands/sonari:keymap.md` | slash command printing the resolved keymap | 10 |
| `tests/test_commands.py` | include the new command file | 10 |
| `docs/superpowers/phase1-execution-log.md` | append a Phase 2 section | 11 |
| `docs/superpowers/phase2-manual-smoke-checklist.md` | live O-1..O-4 verifications | 11 |

---

## Task 1: Protocol - new message types

**Files:**
- Modify: `src/sonari/protocol.py`
- Test: `tests/test_protocol.py`

Note: `tests/test_protocol.py` has TWO exhaustive tests that enumerate every MsgType
(`test_msgtype_has_every_constant_with_exact_values` and
`test_msgtype_defines_no_extra_string_constants`). Adding constants will break the
"no extra constants" test unless we update its expected set. We update both expected
maps in this task.

- [ ] **Step 1: Write the failing test**

Append to `tests/test_protocol.py`:

```python
def test_reread_options_and_cycle_verbosity_constants():
    assert MsgType.REREAD_OPTIONS == "reread_options"
    assert MsgType.CYCLE_VERBOSITY == "cycle_verbosity"
```

Then add the two new entries to BOTH `expected` dicts already present in the file
(in `test_msgtype_has_every_constant_with_exact_values` and
`test_msgtype_defines_no_extra_string_constants`). In each, add these two lines
right after the `"PING": "ping",` line:

```python
        "REREAD_OPTIONS": "reread_options",
        "CYCLE_VERBOSITY": "cycle_verbosity",
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_protocol.py -v`
Expected: FAIL - `test_reread_options_and_cycle_verbosity_constants` fails with
`AttributeError: type object 'MsgType' has no attribute 'REREAD_OPTIONS'`, and
`test_msgtype_has_every_constant_with_exact_values` fails (asserts the new names exist).

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/protocol.py`, inside `class MsgType`, add the two constants
immediately after `PING = "ping"`:

```python
    PING = "ping"
    REREAD_OPTIONS = "reread_options"
    CYCLE_VERBOSITY = "cycle_verbosity"
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_protocol.py -v`
Expected: PASS (all protocol tests green, including the two exhaustive ones).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/protocol.py tests/test_protocol.py
git commit -m "feat(protocol): add reread_options and cycle_verbosity message types"
```

---

## Task 2: daemon - relative rate (SET_RATE delta) with clamp + spoken confirmation

**Files:**
- Modify: `src/sonari/daemon.py` (add module constants `RATE_MIN`/`RATE_MAX`; extend the `MsgType.SET_RATE` branch)
- Test: `tests/test_daemon_phase2.py` (new file)

Behavior: when a `set_rate` message carries `delta`, compute
`new = clamp(current + delta, RATE_MIN=100, RATE_MAX=400)`, persist it, set the
speaker rate, and enqueue a terse "Rate N." confirmation to the foreground session.
Absolute `set_rate` (carrying `rate`) keeps working unchanged.

Note on assertions: `_enqueue` adds to the queue, not `speaker.spoken`. We assert on
`speaker.rates[-1]` and `config["rate"]` for the numeric effect, and pop the queue
(`queue.pop_next().text`) for the "Rate N." announcement - mirroring how
`test_daemon_control.py` reads enqueued items.

- [ ] **Step 1: Write the failing test**

Create `tests/test_daemon_phase2.py`:

```python
from sonari.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


# ---------------------------------------------------------------------------
# Task 2: relative rate (SET_RATE delta)
# ---------------------------------------------------------------------------

def test_set_rate_delta_increments_and_announces():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 200
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=25))
    assert config["rate"] == 225
    assert speaker.rates[-1] == 225
    # the confirmation is enqueued for the foreground session
    item = queue.pop_next()
    assert item is not None
    assert item.text == "Rate 225."
    assert item.session == "fg"
    assert item.is_decision is False


def test_set_rate_delta_negative_decrements():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 200
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=-25))
    assert config["rate"] == 175
    assert speaker.rates[-1] == 175


def test_set_rate_delta_clamps_at_max():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 390
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=25))
    assert config["rate"] == 400
    assert speaker.rates[-1] == 400


def test_set_rate_delta_clamps_at_min():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 110
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=-25))
    assert config["rate"] == 100
    assert speaker.rates[-1] == 100


def test_set_rate_absolute_still_works():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", rate=300))
    assert config["rate"] == 300
    assert speaker.rates[-1] == 300
    # absolute path does NOT enqueue a confirmation (unchanged behavior)
    assert len(queue) == 0


def test_set_rate_delta_no_foreground_still_updates_rate():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    config["rate"] = 200
    daemon.handle_message(_msg(MsgType.SET_RATE, delta=25))
    assert config["rate"] == 225
    assert speaker.rates[-1] == 225
    # no foreground => nothing enqueued
    assert len(queue) == 0
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_daemon_phase2.py -v`
Expected: FAIL - `test_set_rate_delta_increments_and_announces` fails because the
delta branch does not exist yet: `config["rate"]` stays 200 / `speaker.rates[-1]`
is the absolute `None` path, and `queue.pop_next()` returns `None`.

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/daemon.py`, add module-level constants just after the imports
(before `class SpeechDaemon`):

```python
RATE_MIN = 100
RATE_MAX = 400
```

Then replace the existing `SET_RATE` branch:

```python
        if t == MsgType.SET_RATE:
            rate = msg.get("rate")
            self.config["rate"] = rate
            self.speaker.set_rate(rate)
            save_config(self.config)
            return None
```

with:

```python
        if t == MsgType.SET_RATE:
            if "delta" in msg:
                cur = self.config.get("rate", 200)
                new = max(RATE_MIN, min(RATE_MAX, int(cur) + int(msg.get("delta", 0))))
                self.config["rate"] = new
                self.speaker.set_rate(new)
                save_config(self.config)
                fg = self.sessions.foreground()
                if fg is not None:
                    self._enqueue(fg, "prose", "Rate {0}.".format(new), False)
                return None
            rate = msg.get("rate")
            self.config["rate"] = rate
            self.speaker.set_rate(rate)
            save_config(self.config)
            return None
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_daemon_phase2.py -v`
Expected: PASS (6 tests).

Also run the existing control + settings tests to confirm no regression:
Run: `.venv/bin/python -m pytest tests/test_daemon_control.py tests/test_daemon_settings.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_phase2.py
git commit -m "feat(daemon): relative set_rate delta with clamp and spoken confirmation"
```

---

## Task 3: daemon - cycle_verbosity

**Files:**
- Modify: `src/sonari/daemon.py` (add `MsgType.CYCLE_VERBOSITY` branch)
- Test: `tests/test_daemon_phase2.py`

Behavior: advance `everything → medium → quiet → everything`, persist, and announce
the new level to the foreground session. We enqueue the announcement directly (not
gated by the prose-quiet rule), so the user hears the level even when switching *to*
quiet.

- [ ] **Step 1: Write the failing test**

Append to `tests/test_daemon_phase2.py`:

```python
# ---------------------------------------------------------------------------
# Task 3: cycle_verbosity
# ---------------------------------------------------------------------------

def test_cycle_verbosity_everything_to_medium():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY, "fg"))
    assert config["verbosity"] == "medium"
    item = queue.pop_next()
    assert item is not None
    assert item.text == "Verbosity medium."
    assert item.session == "fg"


def test_cycle_verbosity_medium_to_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY, "fg"))
    assert config["verbosity"] == "quiet"
    assert queue.pop_next().text == "Verbosity quiet."


def test_cycle_verbosity_quiet_wraps_to_everything():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY, "fg"))
    assert config["verbosity"] == "everything"
    assert queue.pop_next().text == "Verbosity everything."


def test_cycle_verbosity_unknown_current_defaults_to_everything_step():
    # an out-of-range stored value is treated as the start of the cycle
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["verbosity"] = "bogus"
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY, "fg"))
    assert config["verbosity"] == "everything"


def test_cycle_verbosity_no_foreground_still_persists():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground=None)
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY))
    assert config["verbosity"] == "medium"
    assert len(queue) == 0
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_daemon_phase2.py -k cycle_verbosity -v`
Expected: FAIL - the `cycle_verbosity` branch does not exist, so `config["verbosity"]`
is unchanged and `queue.pop_next()` returns `None`.

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/daemon.py`, add this branch immediately after the `SET_VERBOSITY`
branch (so it sits with the other verbosity handling):

```python
        if t == MsgType.CYCLE_VERBOSITY:
            order = ["everything", "medium", "quiet"]
            cur = self.config.get("verbosity", "everything")
            if cur in order:
                nxt = order[(order.index(cur) + 1) % len(order)]
            else:
                nxt = order[0]
            self.config["verbosity"] = nxt
            save_config(self.config)
            fg = self.sessions.foreground()
            if fg is not None:
                self._enqueue(fg, "prose", "Verbosity {0}.".format(nxt), False)
            return None
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_daemon_phase2.py -k cycle_verbosity -v`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_phase2.py
git commit -m "feat(daemon): cycle_verbosity advances everything->medium->quiet and announces"
```

---

## Task 4: daemon - option caching + reread_options + clearing

**Files:**
- Modify: `src/sonari/daemon.py` (add `self._last_options` in `__init__`; cache it in the CHOICE/PLAN/PERMISSION branches; add `REREAD_OPTIONS` branch; clear it in FLUSH and SESSION_END)
- Test: `tests/test_daemon_phase2.py`

Behavior: whenever speechd builds the spoken text for a CHOICE/PLAN/PERMISSION, it
stores that exact text in `self._last_options`. `reread_options` re-enqueues that
cached text to the foreground session, or says "No options to repeat." when there is
nothing cached. The cache is cleared on `flush` and `session_end`.

This task caches *whatever `text` currently is*. Task 5 later enriches `text` (notes
+ cue) BEFORE the caching line, so re-read will include them once Task 5 lands. To make
that ordering work, caching must happen on the SAME computed `text` variable that gets
enqueued - so in this task we refactor each decision branch to compute `text` into a
local first, cache it, then enqueue.

- [ ] **Step 1: Write the failing test**

Append to `tests/test_daemon_phase2.py`:

```python
# ---------------------------------------------------------------------------
# Task 4: option caching + reread_options + clearing
# ---------------------------------------------------------------------------

def test_reread_after_choice_reenqueues_same_text():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Pick a color", "options": [{"label": "Red"}, {"label": "Blue"}]},
    ]))
    spoken = queue.pop_next().text  # drain the original CHOICE item
    assert "Option 1: Red." in spoken
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    item = queue.pop_next()
    assert item is not None
    assert item.text == spoken
    assert item.kind == "choice"
    assert item.session == "fg"
    assert item.is_decision is False


def test_reread_with_no_prior_says_nothing_to_repeat():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon._last_options is None
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    item = queue.pop_next()
    assert item is not None
    assert item.text == "No options to repeat."
    assert item.kind == "prose"


def test_reread_no_foreground_is_noop():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon._last_options = "Option 1: Red."
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))
    assert len(queue) == 0


def test_plan_and_permission_also_cache_for_reread():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Do the thing."))
    plan_spoken = queue.pop_next().text
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    assert queue.pop_next().text == plan_spoken

    daemon.handle_message(_msg(MsgType.PERMISSION, "fg", action="run rm -rf"))
    perm_spoken = queue.pop_next().text
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    assert queue.pop_next().text == perm_spoken


def test_flush_clears_option_cache():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Q", "options": [{"label": "A"}]},
    ]))
    queue.pop_next()  # drain
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    assert daemon._last_options is None
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    assert queue.pop_next().text == "No options to repeat."


def test_session_end_clears_option_cache():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Q", "options": [{"label": "A"}]},
    ]))
    queue.pop_next()
    daemon.handle_message(_msg(MsgType.SESSION_END, "fg"))
    assert daemon._last_options is None
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_daemon_phase2.py -k "reread or cache" -v`
Expected: FAIL - `AttributeError: 'SpeechDaemon' object has no attribute '_last_options'`
(in `__init__`) and the `REREAD_OPTIONS` branch does not exist.

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/daemon.py`, in `SpeechDaemon.__init__`, add the cache attribute next to
`self._last_spoken`:

```python
        self._last_spoken: str | None = None
        self._last_options: str | None = None
```

Replace the three decision branches:

```python
        if t == MsgType.CHOICE:
            if self.sessions.should_speak(session):
                self._enqueue(session, "choice", self._choice_text(msg), True)
            return None

        if t == MsgType.PLAN:
            if self.sessions.should_speak(session):
                self._enqueue(session, "plan", self._plan_text(msg), True)
            return None

        if t == MsgType.PERMISSION:
            if self.sessions.should_speak(session):
                self._enqueue(session, "permission", self._permission_text(msg), True)
            return None
```

with the cached form (caching the EXACT text that gets enqueued):

```python
        if t == MsgType.CHOICE:
            if self.sessions.should_speak(session):
                text = self._choice_text(msg)
                self._last_options = text
                self._enqueue(session, "choice", text, True)
            return None

        if t == MsgType.PLAN:
            if self.sessions.should_speak(session):
                text = self._plan_text(msg)
                self._last_options = text
                self._enqueue(session, "plan", text, True)
            return None

        if t == MsgType.PERMISSION:
            if self.sessions.should_speak(session):
                text = self._permission_text(msg)
                self._last_options = text
                self._enqueue(session, "permission", text, True)
            return None
```

Add the `REREAD_OPTIONS` branch immediately after the `REPEAT` branch:

```python
        if t == MsgType.REREAD_OPTIONS:
            fg = self.sessions.foreground()
            if self._last_options and fg is not None:
                self._enqueue(fg, "choice", self._last_options, False)
            elif fg is not None:
                self._enqueue(fg, "prose", "No options to repeat.", False)
            return None
```

In the `FLUSH` branch, add the cache clear:

```python
        if t == MsgType.FLUSH:
            self.queue.flush_session(session)
            self.speaker.cancel()
            self._assemblers.pop(session, None)
            self._last_options = None
            return None
```

In the `SESSION_END` branch, add the cache clear:

```python
        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._last_options = None
            return None
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_daemon_phase2.py -v`
Expected: PASS (all Task 2/3/4 tests).

Regression check on decisions + control:
Run: `.venv/bin/python -m pytest tests/test_daemon_decisions.py tests/test_daemon_control.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_phase2.py
git commit -m "feat(daemon): cache last picker text and add reread_options with clearing"
```

---

## Task 5: daemon - selection cue + immediate warning (everything-only) + multiSelect/>9 notes (all modes)

**Files:**
- Modify: `src/sonari/daemon.py` (add `self._warned_immediate` in `__init__`; add `_selection_cue` method and `_choice_notes` staticmethod; wire both into the decision branches BEFORE caching/enqueue)
- Test: `tests/test_daemon_phase2.py`

Behavior (spec §3.2.4 + §6):
- **Selection cue** `"Press the option's number to choose, or Escape to cancel."` is
  appended **only at `everything`** verbosity, on CHOICE/PLAN/PERMISSION.
- **Immediate warning** `" Selecting is immediate."` is appended (to the cue) **once per
  session** at `everything` only.
- **Choice notes** (CHOICE only): a multiSelect note and a `>9`-options note, appended
  **in any verbosity** when those cases occur (the user can't operate them from the
  learned single-select behavior).
- Ordering: notes then cue are appended to `text` BEFORE `self._last_options = text`, so
  re-read (Task 4) replays the enriched text.

- [ ] **Step 1: Write the failing test**

Append to `tests/test_daemon_phase2.py`:

```python
# ---------------------------------------------------------------------------
# Task 5: selection cue + immediate warning + multiSelect/>9 notes
# ---------------------------------------------------------------------------

CUE = "Press the option's number to choose, or Escape to cancel."
WARN = "Selecting is immediate."
MULTI = "Select multiple: press each number, or Space on the highlighted item, then Enter to confirm."
OVER9 = "More than nine options; use arrow keys for ten and up."


def _two_option_choice(session="fg"):
    return _msg(MsgType.CHOICE, session, questions=[
        {"question": "Pick a color", "options": [{"label": "Red"}, {"label": "Blue"}]},
    ])


def test_choice_cue_present_at_everything():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_two_option_choice())
    text = queue.pop_next().text
    assert CUE in text


def test_choice_cue_absent_at_medium():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    daemon.handle_message(_two_option_choice())
    text = queue.pop_next().text
    assert CUE not in text
    assert WARN not in text


def test_choice_cue_absent_at_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
    daemon.handle_message(_two_option_choice())
    text = queue.pop_next().text
    assert CUE not in text
    assert WARN not in text


def test_immediate_warning_fires_once_per_session():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_two_option_choice())
    first = queue.pop_next().text
    assert WARN in first
    daemon.handle_message(_two_option_choice())
    second = queue.pop_next().text
    assert CUE in second          # cue still present every time
    assert WARN not in second     # warning only the first time


def test_immediate_warning_independent_per_session():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_two_option_choice("fg"))
    assert WARN in queue.pop_next().text
    # a different foreground session gets its own first-time warning
    sessions.set_foreground("fg2")
    daemon.handle_message(_two_option_choice("fg2"))
    assert WARN in queue.pop_next().text


def test_multiselect_note_present_in_any_mode():
    for verb in ("everything", "medium", "quiet"):
        daemon, queue, speaker, sessions, config = make_daemon(verbosity=verb, foreground="fg")
        daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
            {"question": "Pick some", "multiSelect": True,
             "options": [{"label": "A"}, {"label": "B"}]},
        ]))
        text = queue.pop_next().text
        assert MULTI in text, verb


def test_over_nine_note_present_in_any_mode():
    opts = [{"label": "Opt {0}".format(i)} for i in range(1, 11)]  # 10 options
    for verb in ("everything", "medium", "quiet"):
        daemon, queue, speaker, sessions, config = make_daemon(verbosity=verb, foreground="fg")
        daemon.handle_message(_msg(MsgType.CHOICE, "fg",
                                   questions=[{"question": "Many", "options": opts}]))
        text = queue.pop_next().text
        assert OVER9 in text, verb


def test_permission_gets_cue_but_not_choice_notes_at_everything():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.PERMISSION, "fg", action="run rm -rf"))
    text = queue.pop_next().text
    assert "run rm -rf" in text
    assert CUE in text
    assert MULTI not in text
    assert OVER9 not in text


def test_plan_gets_cue_at_everything_but_not_at_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Do it."))
    assert CUE in queue.pop_next().text

    daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Do it."))
    assert CUE not in queue.pop_next().text


def test_reread_includes_cue_and_notes():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Pick some", "multiSelect": True,
         "options": [{"label": "A"}, {"label": "B"}]},
    ]))
    spoken = queue.pop_next().text
    assert MULTI in spoken and CUE in spoken
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    assert queue.pop_next().text == spoken
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_daemon_phase2.py -k "cue or warning or note or reread_includes" -v`
Expected: FAIL - `CUE`/`MULTI`/`OVER9` strings are not in the spoken text because the
enrichment helpers and wiring do not exist yet; `_warned_immediate` is missing.

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/daemon.py`, in `SpeechDaemon.__init__`, add next to the other state:

```python
        self._last_options: str | None = None
        self._warned_immediate: set[str] = set()
```

Add these two helpers to the class (place them right after `_permission_text`):

```python
    def _selection_cue(self, session: str, verbosity: str) -> str:
        if verbosity != "everything":
            return ""
        cue = "Press the option's number to choose, or Escape to cancel."
        if session not in self._warned_immediate:
            self._warned_immediate.add(session)
            cue += " Selecting is immediate."
        return cue

    @staticmethod
    def _choice_notes(msg) -> str:
        notes = []
        questions = msg.get("questions", []) or []
        if any(isinstance(q, dict) and q.get("multiSelect") for q in questions):
            notes.append(
                "Select multiple: press each number, or Space on the "
                "highlighted item, then Enter to confirm."
            )
        if any(
            isinstance(q, dict) and len(q.get("options", []) or []) > 9
            for q in questions
        ):
            notes.append("More than nine options; use arrow keys for ten and up.")
        return " ".join(notes)
```

Now wire them into the three decision branches. Replace the branches from Task 4 with:

```python
        if t == MsgType.CHOICE:
            if self.sessions.should_speak(session):
                text = self._choice_text(msg)
                extras = [e for e in (
                    self._choice_notes(msg),
                    self._selection_cue(session, verbosity),
                ) if e]
                if extras:
                    text = "{0} {1}".format(text, " ".join(extras))
                self._last_options = text
                self._enqueue(session, "choice", text, True)
            return None

        if t == MsgType.PLAN:
            if self.sessions.should_speak(session):
                text = self._plan_text(msg)
                cue = self._selection_cue(session, verbosity)
                if cue:
                    text = "{0} {1}".format(text, cue)
                self._last_options = text
                self._enqueue(session, "plan", text, True)
            return None

        if t == MsgType.PERMISSION:
            if self.sessions.should_speak(session):
                text = self._permission_text(msg)
                cue = self._selection_cue(session, verbosity)
                if cue:
                    text = "{0} {1}".format(text, cue)
                self._last_options = text
                self._enqueue(session, "permission", text, True)
            return None
```

Note: the multiSelect note string is split across two source lines above for line
length, but it concatenates to exactly:
`"Select multiple: press each number, or Space on the highlighted item, then Enter to confirm."`
which equals the `MULTI` constant in the test.

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_daemon_phase2.py -v`
Expected: PASS (all Task 2-5 tests).

**This behavior intentionally changes two existing tests** (they assert the OLD,
un-cued decision text, and both run at `everything` where the cue is now appended). Update
both - do NOT relax the new behavior to match them.

(1) `tests/test_daemon_decisions.py` - the permission test uses *exact equality* on the
bare action. Change:
```python
    assert item.text == "run rm -rf"
```
to a substring assertion:
```python
    assert "run rm -rf" in item.text
```
(The choice and plan asserts in that file already use `in`, so they keep passing.)

(2) `tests/test_e2e_pipeline.py::test_scripted_session_full_ordering` runs at `everything`
(its local `make_daemon` sets `cfg["verbosity"]="everything"`), so the choice and
permission spoken text now carry the cue. The choice is the session's FIRST decision so it
also carries the once-per-session `"Selecting is immediate."`; the later permission does
NOT (the session was already warned). Replace the expected `log` list with:
```python
    assert log == [
        ("earcon", "choice"),
        ("text", "Let me check the files."),
        ("text", "I will start now."),
        ("text", "Which approach? Option 1: Refactor. Option 2: Rewrite. Press the option's number to choose, or Escape to cancel. Selecting is immediate."),
        ("earcon", "permission"),
        ("text", "Applying the change now."),
        ("text", "Run: pytest -q Press the option's number to choose, or Escape to cancel."),
        ("earcon", "turn_done"),
    ]
```
(The `test_background_session_is_earcon_only` test is unaffected: gated-out text is never
spoken, so no cue is produced.)

- [ ] **Step 4b: Run the updated regression tests**

Run: `.venv/bin/python -m pytest tests/test_daemon_decisions.py tests/test_e2e_pipeline.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_phase2.py tests/test_daemon_decisions.py tests/test_e2e_pipeline.py
git commit -m "feat(daemon): selection cue + once-per-session warning + multiSelect/>9 notes"
```

---

## Task 6: paths - new path constants

**Files:**
- Modify: `src/sonari/paths.py` (add `KEYMAP_PATH`, `HOTKEYD_RESOLVED_PATH`, `HOTKEYD_BIN_PATH`)
- Test: `tests/test_paths.py`

- [ ] **Step 1: Write the failing test**

Append to `tests/test_paths.py`:

```python
def test_phase2_path_constants_names(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.KEYMAP_PATH.name == "keymap.json"
    assert paths.HOTKEYD_RESOLVED_PATH.name == "hotkeyd.resolved.json"
    assert paths.HOTKEYD_BIN_PATH.name == "sonari-hotkeyd"


def test_phase2_paths_nested_under_sonari_dir(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.KEYMAP_PATH.parent == paths.SONARI_DIR
    assert paths.HOTKEYD_RESOLVED_PATH.parent == paths.SONARI_DIR
    assert paths.HOTKEYD_BIN_PATH.parent == paths.SONARI_DIR
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_paths.py -k phase2 -v`
Expected: FAIL - `AttributeError: module 'sonari.paths' has no attribute 'KEYMAP_PATH'`.

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/paths.py`, add after the existing `LOG_PATH` line:

```python
LOG_PATH = SONARI_DIR / "speechd.log"
KEYMAP_PATH = SONARI_DIR / "keymap.json"
HOTKEYD_RESOLVED_PATH = SONARI_DIR / "hotkeyd.resolved.json"
HOTKEYD_BIN_PATH = SONARI_DIR / "sonari-hotkeyd"
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_paths.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/paths.py tests/test_paths.py
git commit -m "feat(paths): add keymap, hotkeyd-resolved, and hotkeyd-binary path constants"
```

---

## Task 7: keymap module - all logic + unit tests

**Files:**
- Create: `src/sonari/keymap.py`
- Test: `tests/test_keymap.py`

This module is the brain. The Swift binary stays dumb: it just reads the resolved JSON
array and registers/sends. Everything that needs reasoning (name→code, name→mask,
action→message, defaults, merge, atomic write) lives here and is unit-tested.

Key facts baked into the constants (macOS ANSI virtual key codes + Carbon modifier masks):
- `kVK_ANSI_S = 1`, `R = 15`, `D = 2`, `L = 37`, `V = 9`, `O = 31`,
  `Period = 47`, `RightBracket = 30`, `LeftBracket = 33`.
- Carbon masks: `cmdKey = 256`, `shiftKey = 512`, `optionKey = 2048`, `controlKey = 4096`.
- So `ctrl+cmd` = `4096 | 256 = 4352`.

- [ ] **Step 1: Write the failing test**

Create `tests/test_keymap.py`:

```python
import json

import pytest

from sonari import keymap


def _patch_keymap_paths(monkeypatch, tmp_path):
    km = tmp_path / "keymap.json"
    resolved = tmp_path / "hotkeyd.resolved.json"
    monkeypatch.setattr(keymap, "KEYMAP_PATH", km)
    monkeypatch.setattr(keymap, "HOTKEYD_RESOLVED_PATH", resolved)
    monkeypatch.setattr(keymap, "SONARI_DIR", tmp_path)
    monkeypatch.setattr(keymap, "ensure_sonari_dir",
                        lambda: tmp_path.mkdir(parents=True, exist_ok=True))
    return km, resolved


# --- constants ---------------------------------------------------------------

def test_key_codes_cover_default_keys():
    for k in ("s", "r", "d", "l", "v", "o", ".", "]", "["):
        assert k in keymap.KEY_CODES
    assert keymap.KEY_CODES["s"] == 1
    assert keymap.KEY_CODES["."] == 47
    assert keymap.KEY_CODES["]"] == 30
    assert keymap.KEY_CODES["["] == 33


def test_mod_masks_values():
    assert keymap.MOD_MASKS["cmd"] == 256
    assert keymap.MOD_MASKS["shift"] == 512
    assert keymap.MOD_MASKS["opt"] == 2048
    assert keymap.MOD_MASKS["ctrl"] == 4096


def test_action_messages_faster_has_delta_25():
    assert keymap.ACTION_MESSAGES["faster"] == {"type": "set_rate", "delta": 25}
    assert keymap.ACTION_MESSAGES["slower"] == {"type": "set_rate", "delta": -25}


def test_default_keymap_has_nine_actions():
    assert set(keymap.DEFAULT_KEYMAP.keys()) == {
        "stop", "repeat", "skip", "jump_decision", "catch_up",
        "faster", "slower", "cycle_verbosity", "reread_options",
    }
    assert keymap.DEFAULT_KEYMAP["stop"]["key"] == "s"
    assert keymap.DEFAULT_KEYMAP["stop"]["mods"] == ["ctrl", "cmd"]
    assert keymap.DEFAULT_KEYMAP["skip"]["key"] == "."
    assert keymap.DEFAULT_KEYMAP["faster"]["key"] == "]"
    assert keymap.DEFAULT_KEYMAP["slower"]["key"] == "["


# --- resolve_keymap ----------------------------------------------------------

def test_resolve_stop_entry_exact():
    resolved = keymap.resolve_keymap({"stop": {"key": "s", "mods": ["ctrl", "cmd"]}})
    assert resolved == [{
        "action": "stop",
        "keyCode": 1,
        "modifiers": 4352,  # 4096 | 256
        "message": '{"type": "stop"}',
    }]


def test_resolve_faster_message_is_json_with_delta():
    resolved = keymap.resolve_keymap({"faster": {"key": "]", "mods": ["ctrl", "cmd"]}})
    entry = resolved[0]
    assert entry["keyCode"] == 30
    assert entry["modifiers"] == 4352
    assert json.loads(entry["message"]) == {"type": "set_rate", "delta": 25}


def test_resolve_default_keymap_has_nine_entries():
    resolved = keymap.resolve_keymap(keymap.DEFAULT_KEYMAP)
    assert len(resolved) == 9
    actions = {e["action"] for e in resolved}
    assert actions == set(keymap.DEFAULT_KEYMAP.keys())


def test_resolve_unknown_key_raises():
    with pytest.raises(ValueError):
        keymap.resolve_keymap({"stop": {"key": "zzz", "mods": ["ctrl", "cmd"]}})


def test_resolve_unknown_mod_raises():
    with pytest.raises(ValueError):
        keymap.resolve_keymap({"stop": {"key": "s", "mods": ["hyper"]}})


def test_resolve_unknown_action_raises():
    with pytest.raises(ValueError):
        keymap.resolve_keymap({"frobnicate": {"key": "s", "mods": ["ctrl", "cmd"]}})


# --- load_keymap -------------------------------------------------------------

def test_load_keymap_returns_defaults_when_missing(monkeypatch, tmp_path):
    _patch_keymap_paths(monkeypatch, tmp_path)
    loaded = keymap.load_keymap()
    assert loaded == keymap.DEFAULT_KEYMAP
    # must be an independent copy
    loaded["stop"]["key"] = "x"
    assert keymap.DEFAULT_KEYMAP["stop"]["key"] == "s"


def test_load_keymap_merges_user_override(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({"stop": {"key": "x", "mods": ["cmd"]}}), encoding="utf-8")
    loaded = keymap.load_keymap()
    assert loaded["stop"] == {"key": "x", "mods": ["cmd"]}
    # untouched actions keep defaults
    assert loaded["repeat"] == keymap.DEFAULT_KEYMAP["repeat"]


def test_load_keymap_tolerates_corrupt_file(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text("{ not json", encoding="utf-8")
    assert keymap.load_keymap() == keymap.DEFAULT_KEYMAP


# --- write_default_keymap_if_absent -----------------------------------------

def test_write_default_keymap_if_absent_writes_once(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    assert not km.exists()
    assert keymap.write_default_keymap_if_absent() is True
    assert km.exists()
    on_disk = json.loads(km.read_text(encoding="utf-8"))
    assert on_disk == keymap.DEFAULT_KEYMAP
    # second call is a no-op
    assert keymap.write_default_keymap_if_absent() is False


# --- write_resolved ----------------------------------------------------------

def test_write_resolved_emits_array_of_nine(monkeypatch, tmp_path):
    km, resolved = _patch_keymap_paths(monkeypatch, tmp_path)
    out_path = keymap.write_resolved()
    assert out_path == str(resolved)
    data = json.loads(resolved.read_text(encoding="utf-8"))
    assert isinstance(data, list)
    assert len(data) == 9
    for entry in data:
        assert isinstance(entry["keyCode"], int)
        assert isinstance(entry["modifiers"], int)
        assert isinstance(entry["message"], str)


def test_write_resolved_no_tmp_leftover(monkeypatch, tmp_path):
    km, resolved = _patch_keymap_paths(monkeypatch, tmp_path)
    keymap.write_resolved()
    assert list(tmp_path.glob("*.tmp")) == []
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_keymap.py -v`
Expected: FAIL - `ModuleNotFoundError: No module named 'sonari.keymap'`.

- [ ] **Step 3: Write minimal implementation**

Create `src/sonari/keymap.py`:

```python
"""Sonari Phase 2 keymap: ALL hotkey logic lives here (the Swift binary is dumb).

Maps key names -> macOS virtual key codes, modifier names -> Carbon masks, and
actions -> speechd protocol messages. Produces the resolved JSON array that the
Swift hotkeyd reads, registers, and sends on fire.
"""

import json
import os

from sonari.paths import (
    KEYMAP_PATH,
    HOTKEYD_RESOLVED_PATH,
    SONARI_DIR,
    ensure_sonari_dir,
)

# macOS ANSI virtual key codes (Carbon kVK_ANSI_*).
KEY_CODES = {
    "s": 1,
    "r": 15,
    "d": 2,
    "l": 37,
    "v": 9,
    "o": 31,
    "period": 47,
    ".": 47,
    "rightbracket": 30,
    "]": 30,
    "leftbracket": 33,
    "[": 33,
}

# Carbon modifier masks.
MOD_MASKS = {
    "cmd": 256,
    "shift": 512,
    "opt": 2048,
    "option": 2048,
    "ctrl": 4096,
    "control": 4096,
}

# action -> the speechd protocol message it sends.
ACTION_MESSAGES = {
    "stop": {"type": "stop"},
    "repeat": {"type": "repeat"},
    "skip": {"type": "skip"},
    "jump_decision": {"type": "jump_decision"},
    "catch_up": {"type": "catch_up"},
    "faster": {"type": "set_rate", "delta": 25},
    "slower": {"type": "set_rate", "delta": -25},
    "cycle_verbosity": {"type": "cycle_verbosity"},
    "reread_options": {"type": "reread_options"},
}

# Default bindings (modifier Ctrl+Cmd, chosen to avoid VoiceOver's Ctrl+Opt).
DEFAULT_KEYMAP = {
    "stop": {"key": "s", "mods": ["ctrl", "cmd"]},
    "repeat": {"key": "r", "mods": ["ctrl", "cmd"]},
    "skip": {"key": ".", "mods": ["ctrl", "cmd"]},
    "jump_decision": {"key": "d", "mods": ["ctrl", "cmd"]},
    "catch_up": {"key": "l", "mods": ["ctrl", "cmd"]},
    "faster": {"key": "]", "mods": ["ctrl", "cmd"]},
    "slower": {"key": "[", "mods": ["ctrl", "cmd"]},
    "cycle_verbosity": {"key": "v", "mods": ["ctrl", "cmd"]},
    "reread_options": {"key": "o", "mods": ["ctrl", "cmd"]},
}


def _copy_keymap(km: dict) -> dict:
    """Deep-ish copy: each action maps to a fresh {key, mods[...]} dict."""
    out = {}
    for action, binding in km.items():
        out[action] = {
            "key": binding.get("key"),
            "mods": list(binding.get("mods", [])),
        }
    return out


def resolve_keymap(keymap=None) -> list:
    """Resolve an action->binding map into the Swift-facing array.

    Each output entry: {action, keyCode, modifiers, message}. Raises ValueError
    on an unknown key name, unknown modifier name, or unknown action.
    """
    if keymap is None:
        keymap = DEFAULT_KEYMAP
    resolved = []
    for action, binding in keymap.items():
        if action not in ACTION_MESSAGES:
            raise ValueError("unknown action: {0}".format(action))
        key = (binding.get("key") or "").lower()
        if key not in KEY_CODES:
            raise ValueError("unknown key: {0}".format(binding.get("key")))
        mask = 0
        for mod in binding.get("mods", []):
            m = (mod or "").lower()
            if m not in MOD_MASKS:
                raise ValueError("unknown modifier: {0}".format(mod))
            mask |= MOD_MASKS[m]
        resolved.append({
            "action": action,
            "keyCode": KEY_CODES[key],
            "modifiers": mask,
            "message": json.dumps(ACTION_MESSAGES[action]),
        })
    return resolved


def load_keymap() -> dict:
    """Merge the user's KEYMAP_PATH over a copy of DEFAULT_KEYMAP.

    Missing or corrupt files yield a fresh DEFAULT_KEYMAP copy. A user entry
    fully replaces the default binding for that action.
    """
    merged = _copy_keymap(DEFAULT_KEYMAP)
    try:
        with open(KEYMAP_PATH, "r", encoding="utf-8") as fh:
            user = json.load(fh)
    except (FileNotFoundError, ValueError, OSError):
        return merged
    if not isinstance(user, dict):
        return merged
    for action, binding in user.items():
        if isinstance(binding, dict):
            merged[action] = {
                "key": binding.get("key"),
                "mods": list(binding.get("mods", [])),
            }
    return merged


def write_default_keymap_if_absent() -> bool:
    """Write DEFAULT_KEYMAP to KEYMAP_PATH if it does not exist. Returns True
    iff it wrote the file."""
    if os.path.exists(KEYMAP_PATH):
        return False
    ensure_sonari_dir()
    with open(KEYMAP_PATH, "w", encoding="utf-8") as fh:
        json.dump(DEFAULT_KEYMAP, fh, indent=2)
        fh.flush()
        os.fsync(fh.fileno())
    return True


def write_resolved(keymap=None) -> str:
    """Atomically write the resolved array to HOTKEYD_RESOLVED_PATH; return its
    path. Uses load_keymap() when no explicit keymap is given."""
    if keymap is None:
        keymap = load_keymap()
    data = json.dumps(resolve_keymap(keymap))
    ensure_sonari_dir()
    tmp_path = SONARI_DIR / (HOTKEYD_RESOLVED_PATH.name + ".tmp")
    with open(tmp_path, "w", encoding="utf-8") as fh:
        fh.write(data)
        fh.flush()
        os.fsync(fh.fileno())
    os.replace(tmp_path, HOTKEYD_RESOLVED_PATH)
    return str(HOTKEYD_RESOLVED_PATH)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_keymap.py -v`
Expected: PASS (all keymap tests).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/keymap.py tests/test_keymap.py
git commit -m "feat(keymap): key/mod/action resolution, load/merge, default + resolved writers"
```

---

## Task 8: Swift hotkeyd binary

**Files:**
- Create: `hotkeyd/sonari-hotkeyd.swift`
- Test: `tests/test_hotkeyd_swift.py`

The Swift binary reads `~/.sonari/hotkeyd.resolved.json` (the array from Task 7),
installs ONE keyboard event handler, registers each entry with `RegisterEventHotKey`,
and on fire writes `message + "\n"` to the AF_UNIX socket at `~/.sonari/speechd.sock`
(best-effort; ignore errors). Headless via `.accessory` policy + `NSApplication.run`.

- [ ] **Step 1: Write the failing test**

Create `tests/test_hotkeyd_swift.py`:

```python
import json
import os
import shutil
import subprocess

import pytest

from sonari import keymap

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SWIFT_SRC = os.path.join(REPO_ROOT, "hotkeyd", "sonari-hotkeyd.swift")


def test_swift_source_exists():
    assert os.path.isfile(SWIFT_SRC), SWIFT_SRC


@pytest.mark.skipif(shutil.which("swiftc") is None, reason="swiftc not available")
def test_swift_source_compiles(tmp_path):
    out = tmp_path / "sonari-hotkeyd"
    proc = subprocess.run(
        ["swiftc", SWIFT_SRC, "-o", str(out)],
        capture_output=True, text=True,
    )
    assert proc.returncode == 0, proc.stderr
    assert out.exists()


def test_resolved_json_shape_matches_swift_contract(monkeypatch, tmp_path):
    # The Swift reads entries with int keyCode, int modifiers, str message.
    resolved = tmp_path / "hotkeyd.resolved.json"
    monkeypatch.setattr(keymap, "HOTKEYD_RESOLVED_PATH", resolved)
    monkeypatch.setattr(keymap, "SONARI_DIR", tmp_path)
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    monkeypatch.setattr(keymap, "ensure_sonari_dir",
                        lambda: tmp_path.mkdir(parents=True, exist_ok=True))
    keymap.write_resolved()
    data = json.loads(resolved.read_text(encoding="utf-8"))
    assert isinstance(data, list) and data
    for entry in data:
        assert set(entry.keys()) >= {"keyCode", "modifiers", "message"}
        assert isinstance(entry["keyCode"], int)
        assert isinstance(entry["modifiers"], int)
        assert isinstance(entry["message"], str)
        # message is itself a JSON object speechd understands
        assert isinstance(json.loads(entry["message"]), dict)
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_hotkeyd_swift.py -v`
Expected: FAIL - `test_swift_source_exists` fails (the `.swift` file does not exist).

- [ ] **Step 3: Write minimal implementation**

Create `hotkeyd/sonari-hotkeyd.swift`:

```swift
// sonari-hotkeyd.swift
// Sonari Phase 2 global-hotkey daemon.
//
// Reads ~/.sonari/hotkeyd.resolved.json - an array of
//   { "action": String, "keyCode": Int, "modifiers": Int, "message": String }
// produced by sonari.keymap.write_resolved(). For each entry it registers a
// Carbon global hotkey (RegisterEventHotKey: fires system-wide, consumes only
// the registered combo, needs NO macOS permission). On fire it writes the
// entry's `message` plus a newline to the speechd Unix socket at
// ~/.sonari/speechd.sock (best-effort; errors ignored).
//
// Build: swiftc hotkeyd/sonari-hotkeyd.swift -o ~/.sonari/sonari-hotkeyd
// Run:   the com.sonari.hotkeyd LaunchAgent (Aqua session, .accessory policy).

import Carbon
import Cocoa

let kHotKeySignature: OSType = 0x534F4E49  // 'SONI'

struct HotkeyEntry {
    let keyCode: UInt32
    let modifiers: UInt32
    let message: String
}

func sonariDir() -> String {
    return (NSHomeDirectory() as NSString).appendingPathComponent(".sonari")
}

func resolvedPath() -> String {
    return (sonariDir() as NSString).appendingPathComponent("hotkeyd.resolved.json")
}

func socketPath() -> String {
    return (sonariDir() as NSString).appendingPathComponent("speechd.sock")
}

// Parse the resolved JSON array into HotkeyEntry values.
func loadEntries() -> [HotkeyEntry] {
    guard let data = FileManager.default.contents(atPath: resolvedPath()) else {
        FileHandle.standardError.write(
            "hotkeyd: cannot read \(resolvedPath())\n".data(using: .utf8)!)
        return []
    }
    guard let parsed = try? JSONSerialization.jsonObject(with: data),
          let array = parsed as? [[String: Any]] else {
        FileHandle.standardError.write("hotkeyd: malformed resolved JSON\n".data(using: .utf8)!)
        return []
    }
    var entries: [HotkeyEntry] = []
    for obj in array {
        guard let keyCode = obj["keyCode"] as? Int,
              let modifiers = obj["modifiers"] as? Int,
              let message = obj["message"] as? String else {
            continue
        }
        entries.append(HotkeyEntry(
            keyCode: UInt32(keyCode),
            modifiers: UInt32(modifiers),
            message: message))
    }
    return entries
}

// Best-effort: connect to the speechd Unix socket and write one newline-JSON line.
func sendMessage(_ message: String) {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    if fd < 0 { return }
    defer { close(fd) }

    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    let path = socketPath()
    let maxLen = MemoryLayout.size(ofValue: addr.sun_path)
    path.withCString { cstr in
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            ptr.withMemoryRebound(to: CChar.self, capacity: maxLen) { dst in
                strncpy(dst, cstr, maxLen - 1)
            }
        }
    }
    let size = socklen_t(MemoryLayout<sockaddr_un>.size)
    let connected = withUnsafePointer(to: &addr) { aptr -> Int32 in
        aptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sptr in
            connect(fd, sptr, size)
        }
    }
    if connected != 0 { return }

    let line = message + "\n"
    _ = line.withCString { cstr in
        write(fd, cstr, strlen(cstr))
    }
}

// Index entries by their hotkey id so the handler can look up the message.
var entriesByID: [UInt32: HotkeyEntry] = [:]

let hotKeyHandler: EventHandlerUPP = { (_ nextHandler, _ theEvent, _ userData) -> OSStatus in
    var hkID = EventHotKeyID()
    let status = GetEventParameter(
        theEvent,
        EventParamName(kEventParamDirectObject),
        EventParamType(typeEventHotKeyID),
        nil,
        MemoryLayout<EventHotKeyID>.size,
        nil,
        &hkID
    )
    if status == noErr && hkID.signature == kHotKeySignature {
        if let entry = entriesByID[hkID.id] {
            sendMessage(entry.message)
        }
    }
    return noErr
}

// 1. Install the keyboard event handler for hotkey-pressed events.
var eventType = EventTypeSpec(
    eventClass: OSType(kEventClassKeyboard),
    eventKind: UInt32(kEventHotKeyPressed)
)
let installStatus = InstallEventHandler(
    GetApplicationEventTarget(),
    hotKeyHandler,
    1,
    &eventType,
    nil,
    nil
)
guard installStatus == noErr else {
    FileHandle.standardError.write(
        "hotkeyd: InstallEventHandler failed: \(installStatus)\n".data(using: .utf8)!)
    exit(1)
}

// 2. Register each resolved entry. Keep the refs alive for the process lifetime.
var hotKeyRefs: [EventHotKeyRef?] = []
let entries = loadEntries()
for (index, entry) in entries.enumerated() {
    let id = UInt32(index)
    entriesByID[id] = entry
    var ref: EventHotKeyRef?
    let hotKeyID = EventHotKeyID(signature: kHotKeySignature, id: id)
    let regStatus = RegisterEventHotKey(
        entry.keyCode,
        entry.modifiers,
        hotKeyID,
        GetApplicationEventTarget(),
        0,
        &ref
    )
    if regStatus != noErr {
        // A claimed combo: log and continue with the rest.
        FileHandle.standardError.write(
            "hotkeyd: RegisterEventHotKey failed for id \(id) (status \(regStatus))\n"
                .data(using: .utf8)!)
        continue
    }
    hotKeyRefs.append(ref)
}

FileHandle.standardError.write(
    "hotkeyd: registered \(hotKeyRefs.count)/\(entries.count) hotkeys\n".data(using: .utf8)!)

// 3. Run the Carbon event loop headlessly (no Dock icon).
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.run()
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_hotkeyd_swift.py -v`
Expected: PASS - `test_swift_source_exists` and `test_resolved_json_shape_...` pass;
`test_swift_source_compiles` passes on this Mac (swiftc present) with returncode 0.
(If swiftc were absent it would `SKIP`, not fail.)

- [ ] **Step 5: Commit**

```bash
git add hotkeyd/sonari-hotkeyd.swift tests/test_hotkeyd_swift.py
git commit -m "feat(hotkeyd): Swift Carbon hotkey daemon reading resolved keymap, sending to socket"
```

---

## Task 9: cli - install/uninstall/doctor for hotkeyd + protocol-contract test

**Files:**
- Modify: `src/sonari/cli.py` (constants, `_hotkeyd_plist`, `_build_hotkeyd`, extend `install`/`uninstall`/`doctor`; import `keymap`)
- Test: `tests/test_cli_hotkeyd.py` (new)
- Test: `tests/test_hotkeyd_contract.py` (new)

The contract test is the safety net for the whole Swift→speechd link: it proves that
every `ACTION_MESSAGES` dict, fed straight into the daemon, does the intended thing -
so the bytes the Swift sends are valid speechd commands.

- [ ] **Step 1: Write the failing test**

Create `tests/test_cli_hotkeyd.py`:

```python
import os
import plistlib
from unittest import mock

from sonari import cli


def test_hotkeyd_plist_is_valid_and_complete(tmp_path):
    binary = "/Users/u/.sonari/sonari-hotkeyd"
    log = "/Users/u/.sonari/hotkeyd.log"
    xml = cli._hotkeyd_plist(binary, log)
    assert isinstance(xml, str)
    assert xml.startswith("<?xml")
    data = plistlib.loads(xml.encode("utf-8"))
    assert data["Label"] == cli.HOTKEYD_LAUNCH_AGENT_LABEL
    assert data["ProgramArguments"] == [binary]
    assert data["RunAtLoad"] is True
    assert data["KeepAlive"] is True
    assert data["StandardErrorPath"] == log
    assert data["StandardOutPath"] == log
    assert data["ProcessType"] == "Interactive"


def test_build_hotkeyd_missing_swiftc_returns_false():
    with mock.patch("shutil.which", return_value=None):
        ok, detail = cli._build_hotkeyd()
    assert ok is False
    assert "swiftc" in detail.lower()


def test_build_hotkeyd_compiles_when_swiftc_present(tmp_path):
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=0) as call, \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"):
        ok, detail = cli._build_hotkeyd()
    assert ok is True
    # swiftc was invoked with the repo's swift source and the bin output path
    args = call.call_args.args[0]
    assert args[0] == "swiftc"
    assert args[1].endswith(os.path.join("hotkeyd", "sonari-hotkeyd.swift"))
    assert args[-1] == str(tmp_path / "sonari-hotkeyd")


def test_build_hotkeyd_nonzero_returncode_is_failure(tmp_path):
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=1), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"):
        ok, _ = cli._build_hotkeyd()
    assert ok is False


def test_install_writes_hotkeyd_plist_and_keymap(tmp_path, capsys):
    speechd_plist = tmp_path / "com.sonari.speechd.plist"
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    km = tmp_path / "keymap.json"
    resolved = tmp_path / "hotkeyd.resolved.json"
    binp = tmp_path / "sonari-hotkeyd"
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(speechd_plist)), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", km), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", km), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir",
                           lambda: tmp_path.mkdir(parents=True, exist_ok=True)), \
         mock.patch("sonari.paths.ensure_sonari_dir"), \
         mock.patch.object(cli, "_build_hotkeyd", return_value=(True, "built")):
        rc = cli.install()
    assert rc == 0
    assert hotkeyd_plist.exists()
    assert km.exists()
    assert resolved.exists()
    data = plistlib.loads(hotkeyd_plist.read_text().encode("utf-8"))
    assert data["ProgramArguments"] == [str(binp)]
    # hotkeyd agent reloaded
    loads = [c.args[0] for c in run.call_args_list]
    assert any(a[0] == "load" and a[1] == str(hotkeyd_plist) for a in loads)


def test_install_build_failure_is_nonfatal(tmp_path, capsys):
    speechd_plist = tmp_path / "com.sonari.speechd.plist"
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    km = tmp_path / "keymap.json"
    resolved = tmp_path / "hotkeyd.resolved.json"
    binp = tmp_path / "sonari-hotkeyd"
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(speechd_plist)), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", km), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", km), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir",
                           lambda: tmp_path.mkdir(parents=True, exist_ok=True)), \
         mock.patch("sonari.paths.ensure_sonari_dir"), \
         mock.patch.object(cli, "_build_hotkeyd",
                           return_value=(False, "swiftc not found")):
        rc = cli.install()
    assert rc == 0  # speechd still installed; build failure only warns
    out = capsys.readouterr().out
    assert "warning" in out.lower() or "swiftc" in out.lower()


def test_uninstall_removes_hotkeyd_agent_and_binary(tmp_path):
    speechd_plist = tmp_path / "com.sonari.speechd.plist"
    speechd_plist.write_text("<plist/>")
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    hotkeyd_plist.write_text("<plist/>")
    sonari_dir = tmp_path / ".sonari"
    sonari_dir.mkdir()
    binp = sonari_dir / "sonari-hotkeyd"
    binp.write_text("binary")
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(speechd_plist)), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli.paths, "SONARI_DIR", sonari_dir), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp), \
         mock.patch.object(cli, "_legacy_migrate", return_value=[]):
        rc = cli.uninstall()
    assert rc == 0
    assert not hotkeyd_plist.exists()
    unloads = [c.args[0] for c in run.call_args_list]
    assert any(a[0] == "unload" and a[1] == str(hotkeyd_plist) for a in unloads)


def _doctor_ok_patches(tmp_path):
    binp = tmp_path / "sonari-hotkeyd"
    binp.write_text("x")
    resolved = tmp_path / "hotkeyd.resolved.json"
    resolved.write_text("[]")
    return [
        mock.patch("shutil.which", side_effect=lambda n: "/usr/bin/" + n),
        mock.patch("sonari.speaker.best_enhanced_voice", return_value="Ava (Premium)"),
        mock.patch("os.access", return_value=True),
        mock.patch("sonari.paths.ensure_sonari_dir"),
        mock.patch("sonari.client.send", return_value={"ok": True}),
        mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp),
        mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved),
    ]


def test_doctor_includes_hotkeyd_checks(tmp_path):
    patches = _doctor_ok_patches(tmp_path)
    for p in patches:
        p.start()
    try:
        rows = cli.doctor()
    finally:
        for p in reversed(patches):
            p.stop()
    checks = {check for check, _, _ in rows}
    assert "swiftc" in checks
    assert "hotkeyd binary" in checks
    assert "hotkeyd resolved keymap" in checks
    assert "keymap resolves" in checks


def test_doctor_hotkeyd_binary_missing_fails(tmp_path):
    missing = tmp_path / "nope" / "sonari-hotkeyd"
    resolved = tmp_path / "hotkeyd.resolved.json"
    resolved.write_text("[]")
    patches = [
        mock.patch("shutil.which", side_effect=lambda n: "/usr/bin/" + n),
        mock.patch("sonari.speaker.best_enhanced_voice", return_value="V"),
        mock.patch("os.access", return_value=True),
        mock.patch("sonari.paths.ensure_sonari_dir"),
        mock.patch("sonari.client.send", return_value={"ok": True}),
        mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", missing),
        mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved),
    ]
    for p in patches:
        p.start()
    try:
        d = {check: ok for check, ok, _ in cli.doctor()}
    finally:
        for p in reversed(patches):
            p.stop()
    assert d["hotkeyd binary"] is False
```

Create `tests/test_hotkeyd_contract.py`:

```python
"""Every keymap.ACTION_MESSAGES dict must be a valid speechd command: feeding
it straight into a daemon's handle_message must produce the intended effect.
This proves the bytes the Swift hotkeyd sends are real protocol commands."""

from sonari import keymap
from sonari.protocol import MsgType
from tests.daemon_helpers import make_daemon


def _msg(action_message, session="fg"):
    d = dict(action_message)
    d["session"] = session
    return d


def test_all_action_messages_are_known_msgtypes():
    valid_types = {
        v for k, v in vars(MsgType).items()
        if not k.startswith("_") and isinstance(v, str)
    }
    for action, message in keymap.ACTION_MESSAGES.items():
        assert message["type"] in valid_types, action


def test_stop_message_clears_and_cancels():
    from sonari.queue import SpeechItem
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose",
                             text="x", is_decision=False))
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["stop"]))
    assert len(queue) == 0
    assert speaker.cancels == 1


def test_skip_message_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["skip"]))
    assert speaker.cancels == 1


def test_repeat_message_reenqueues_last_spoken():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._last_spoken = "Hello."
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["repeat"]))
    assert queue.pop_next().text == "Hello."


def test_faster_message_bumps_rate_by_25():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 200
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["faster"]))
    assert config["rate"] == 225


def test_slower_message_drops_rate_by_25():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 200
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["slower"]))
    assert config["rate"] == 175


def test_cycle_verbosity_message_advances():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["cycle_verbosity"]))
    assert config["verbosity"] == "medium"


def test_reread_options_message_works():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._last_options = "Option 1: A."
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["reread_options"]))
    assert queue.pop_next().text == "Option 1: A."


def test_jump_decision_message_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["jump_decision"]))
    assert speaker.cancels == 1


def test_catch_up_message_clears_and_cancels():
    from sonari.queue import SpeechItem
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose",
                             text="x", is_decision=False))
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["catch_up"]))
    assert len(queue) == 0
    assert speaker.cancels == 1
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_cli_hotkeyd.py tests/test_hotkeyd_contract.py -v`
Expected: FAIL - `tests/test_cli_hotkeyd.py` fails with
`AttributeError: module 'sonari.cli' has no attribute '_hotkeyd_plist'` (and
`HOTKEYD_LAUNCH_AGENT_LABEL`, `_build_hotkeyd`, `cli.keymap`). The contract test passes
already (it only depends on Tasks 1-5 + 7), confirming the actions are wired.

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/cli.py`, add `keymap` to the package imports near the top
(`from . import paths` is already there):

```python
from . import paths
from . import keymap
```

Add the hotkeyd constants next to the existing `LAUNCH_AGENT_*` constants:

```python
HOTKEYD_LAUNCH_AGENT_LABEL = "com.sonari.hotkeyd"
HOTKEYD_LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.sonari.hotkeyd.plist")
```

Add the plist builder and the build helper (place after `_launchagent_plist`):

```python
def _hotkeyd_plist(binary_path: str, log_path: str) -> str:
    """Return the full LaunchAgent plist XML for the hotkey daemon.

    Runs the compiled Swift binary directly. RunAtLoad + KeepAlive keep it alive
    in the Aqua (GUI) session; ProcessType Interactive so it participates in the
    foreground session that Carbon hotkeys require.
    """
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
        '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
        '<plist version="1.0">\n'
        '<dict>\n'
        '    <key>Label</key>\n'
        f'    <string>{HOTKEYD_LAUNCH_AGENT_LABEL}</string>\n'
        '    <key>ProgramArguments</key>\n'
        '    <array>\n'
        f'        <string>{binary_path}</string>\n'
        '    </array>\n'
        '    <key>RunAtLoad</key>\n'
        '    <true/>\n'
        '    <key>KeepAlive</key>\n'
        '    <true/>\n'
        '    <key>StandardErrorPath</key>\n'
        f'    <string>{log_path}</string>\n'
        '    <key>StandardOutPath</key>\n'
        f'    <string>{log_path}</string>\n'
        '    <key>ProcessType</key>\n'
        '    <string>Interactive</string>\n'
        '</dict>\n'
        '</plist>\n'
    )


def _build_hotkeyd():
    """Compile hotkeyd/sonari-hotkeyd.swift to paths.HOTKEYD_BIN_PATH.

    Returns (ok, detail). Non-fatal: if swiftc is absent we return
    (False, "swiftc not found") and the caller warns but still installs speechd.
    """
    if shutil.which("swiftc") is None:
        return (False, "swiftc not found")
    src = os.path.join(paths.repo_root(), "hotkeyd", "sonari-hotkeyd.swift")
    rc = subprocess.call(["swiftc", src, "-o", str(paths.HOTKEYD_BIN_PATH)])
    if rc == 0:
        return (True, str(paths.HOTKEYD_BIN_PATH))
    return (False, f"swiftc exited {rc}")
```

Extend `install()` - add this block just before the final
`print("Enable the Sonari plugin in Claude Code:")` block (after the speechd
LaunchAgent is loaded):

```python
    # --- Phase 2: hotkeyd ---------------------------------------------------
    keymap.write_default_keymap_if_absent()
    keymap.write_resolved()
    ok, detail = _build_hotkeyd()
    if ok:
        hk_log = os.path.join(os.path.dirname(str(paths.LOG_PATH)), "hotkeyd.log")
        hk_xml = _hotkeyd_plist(str(paths.HOTKEYD_BIN_PATH), hk_log)
        os.makedirs(os.path.dirname(HOTKEYD_LAUNCH_AGENT_PATH), exist_ok=True)
        with open(HOTKEYD_LAUNCH_AGENT_PATH, "w", encoding="utf-8") as f:
            f.write(hk_xml)
        print(f"Wrote LaunchAgent: {HOTKEYD_LAUNCH_AGENT_PATH}")
        _launchctl(["unload", HOTKEYD_LAUNCH_AGENT_PATH])
        hrc = _launchctl(["load", HOTKEYD_LAUNCH_AGENT_PATH])
        if hrc == 0:
            print(f"Loaded LaunchAgent {HOTKEYD_LAUNCH_AGENT_LABEL}.")
        else:
            print(f"warning: 'launchctl load' returned {hrc} for the hotkey daemon.")
    else:
        print(f"warning: hotkey daemon not built ({detail}); "
              f"global hotkeys disabled, but speech still works.")
```

Extend `uninstall()` - add this block right after the speechd LaunchAgent removal
(after the `else: print("No LaunchAgent installed.")` block, before the SONARI_DIR
removal):

```python
    if os.path.exists(HOTKEYD_LAUNCH_AGENT_PATH):
        _launchctl(["unload", HOTKEYD_LAUNCH_AGENT_PATH])
        try:
            os.remove(HOTKEYD_LAUNCH_AGENT_PATH)
            print(f"Removed LaunchAgent: {HOTKEYD_LAUNCH_AGENT_PATH}")
        except OSError as exc:
            print(f"warning: could not remove {HOTKEYD_LAUNCH_AGENT_PATH}: {exc}")
    if os.path.exists(str(paths.HOTKEYD_BIN_PATH)):
        try:
            os.remove(str(paths.HOTKEYD_BIN_PATH))
            print(f"Removed hotkey daemon binary: {paths.HOTKEYD_BIN_PATH}")
        except OSError:
            pass
```

Note: `uninstall()` removes the whole SONARI_DIR afterward anyway; removing the binary
explicitly is harmless and keeps uninstall correct if SONARI_DIR removal is later made
conditional. keymap.json is intentionally left when SONARI_DIR is preserved (spec §5).

Extend `doctor()` - add these checks just before `return results`:

```python
    swiftc = shutil.which("swiftc")
    results.append(("swiftc", swiftc is not None,
                    swiftc or "not found (needed to build the hotkey daemon)"))

    hk_bin = str(paths.HOTKEYD_BIN_PATH)
    hk_exists = os.path.exists(hk_bin)
    results.append(("hotkeyd binary", hk_exists,
                    hk_bin if hk_exists else f"missing: {hk_bin} (run 'sonari install')"))

    try:
        with open(paths.HOTKEYD_RESOLVED_PATH, "r", encoding="utf-8") as fh:
            parsed = json.load(fh)
        ok = isinstance(parsed, list)
        results.append(("hotkeyd resolved keymap", ok,
                        str(paths.HOTKEYD_RESOLVED_PATH) if ok
                        else "not a JSON list"))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("hotkeyd resolved keymap", False, f"unreadable: {exc}"))

    try:
        keymap.resolve_keymap(keymap.load_keymap())
        results.append(("keymap resolves", True, "ok"))
    except Exception as exc:  # noqa: BLE001
        results.append(("keymap resolves", False, f"error: {exc}"))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_cli_hotkeyd.py tests/test_hotkeyd_contract.py -v`
Expected: PASS (all).

**You MUST update two existing CLI tests in this task** (not "if it fails"). Extending
`install()`/`uninstall()` makes them touch the REAL machine unless re-mocked:
`install()` now compiles a real binary via `swiftc` and writes a real
`~/Library/LaunchAgents/com.sonari.hotkeyd.plist` + `~/.sonari/keymap.json`;
`uninstall()` now `os.remove`s the real hotkeyd plist and the real
`~/.sonari/sonari-hotkeyd` binary (which could be the user's actual installed binary).

(1) In `tests/test_cli_install.py::test_install_writes_plist_and_loads`, add these mocks to
its `with` block (mirroring `test_install_writes_hotkeyd_plist_and_keymap`):
```python
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(tmp_path / "com.sonari.hotkeyd.plist")),
         mock.patch.object(cli, "_build_hotkeyd", return_value=(True, "built")),
         mock.patch.object(cli.paths, "KEYMAP_PATH", tmp_path / "keymap.json"),
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"),
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"),
         mock.patch.object(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json"),
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"),
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path),
         mock.patch.object(cli.keymap, "ensure_sonari_dir", lambda: tmp_path.mkdir(parents=True, exist_ok=True)),
```
Its existing speechd assertions stay valid (`rc==0`, the speechd plist is written, the
speechd `_launchctl load` happens).

(2) In `tests/test_cli_uninstall.py::test_uninstall_removes_launchagent_and_sonari_dir`,
add to its `with` block so uninstall targets only temp paths:
```python
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(tmp_path / "com.sonari.hotkeyd.plist")),
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"),
```
NOTE on the spec's "uninstall leaves keymap.json": if the current `uninstall()` removes the
whole `SONARI_DIR`, that also removes `keymap.json`. Do NOT add new SONARI_DIR-removal; if
the existing behavior already nukes the dir, leave a one-line code comment noting the spec
prefers preserving `keymap.json` and let the code-quality reviewer decide - do not expand
scope here.

The existing doctor "all ok" test only checks that its known keys are True, so the extra
hotkeyd rows do not break it.

Then run the existing CLI suites green:
Run: `.venv/bin/python -m pytest tests/test_cli_install.py tests/test_cli_uninstall.py tests/test_cli_doctor.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_hotkeyd.py tests/test_hotkeyd_contract.py tests/test_cli_install.py tests/test_cli_uninstall.py
git commit -m "feat(cli): build+install+uninstall+doctor for hotkeyd; protocol-contract test"
```

---

## Task 10: slash command + `keymap` subcommand + manifests

**Files:**
- Create: `commands/sonari:keymap.md`
- Modify: `src/sonari/cli.py` (add `_cmd_keymap` + register `keymap` subcommand)
- Test: `tests/test_commands.py`
- Test: `tests/test_cli_hotkeyd.py` (add the subcommand test there)

The slash command runs `sonari keymap`, which prints the resolved keymap (all 9
actions with their combos) so the user can confirm/learn bindings eyes-free.

- [ ] **Step 1: Write the failing test**

Append to `tests/test_commands.py`:

```python
def test_keymap_command_file_exists_and_runs_sonari_keymap():
    assert os.path.exists(os.path.join(CMD, "sonari:keymap.md"))
    txt = _read("sonari:keymap.md")
    assert "sonari keymap" in txt
    assert "Bash" in txt
    assert "print" in txt.lower()
```

Also update the existing `test_all_command_files_exist` tuple in `tests/test_commands.py`
to include the new file:

```python
def test_all_command_files_exist():
    for name in ("sonari:status.md", "sonari:verbosity.md", "sonari:stop.md",
                 "sonari:repeat.md", "sonari:doctor.md", "sonari:keymap.md"):
        assert os.path.exists(os.path.join(CMD, name)), name
```

Append to `tests/test_cli_hotkeyd.py`:

```python
def test_keymap_subcommand_prints_all_nine_actions(capsys):
    rc = cli.main(["keymap"])
    assert rc == 0
    out = capsys.readouterr().out
    for action in ("stop", "repeat", "skip", "jump_decision", "catch_up",
                   "faster", "slower", "cycle_verbosity", "reread_options"):
        assert action in out
    # human-readable combos appear (Ctrl+Cmd default)
    assert "Ctrl" in out and "Cmd" in out
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_commands.py -k keymap tests/test_cli_hotkeyd.py::test_keymap_subcommand_prints_all_nine_actions -v`
Expected: FAIL - the command file does not exist, and `cli.main(["keymap"])` returns 2
(unknown subcommand → `print_help`) so its assertions fail.

- [ ] **Step 3: Write minimal implementation**

Create `commands/sonari:keymap.md`:

```markdown
---
description: Show the active Sonari global hotkey bindings
---

Run the Sonari keymap command using the Bash tool:

```
sonari keymap
```

Print the command's output to the user verbatim so they can see every action and
its global hotkey combo. Do not add commentary beyond the raw keymap.
```

In `src/sonari/cli.py`, add a reverse lookup for human-readable modifier/key names and
the command handler. Place these helpers near the other `_cmd_*` functions:

```python
_MOD_DISPLAY = [
    (4096, "Ctrl"),
    (256, "Cmd"),
    (2048, "Opt"),
    (512, "Shift"),
]
_KEYCODE_DISPLAY = {
    1: "S", 15: "R", 2: "D", 37: "L", 9: "V", 31: "O",
    47: ".", 30: "]", 33: "[",
}


def _combo_label(modifiers: int, key_code: int) -> str:
    parts = [name for mask, name in _MOD_DISPLAY if modifiers & mask]
    parts.append(_KEYCODE_DISPLAY.get(key_code, "key{0}".format(key_code)))
    return "+".join(parts)


def _cmd_keymap(_args) -> int:
    try:
        resolved = keymap.resolve_keymap(keymap.load_keymap())
    except ValueError as exc:
        print(f"sonari: invalid keymap: {exc}", file=sys.stderr)
        return 1
    for entry in resolved:
        combo = _combo_label(entry["modifiers"], entry["keyCode"])
        print("{0:<16} {1}".format(entry["action"], combo))
    return 0
```

Register the subcommand inside `_register_local(sub)`:

```python
    sub.add_parser("keymap",
                   help="print the active global hotkey bindings").set_defaults(
        func=_cmd_keymap)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_commands.py tests/test_cli_hotkeyd.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add commands/sonari:keymap.md src/sonari/cli.py tests/test_commands.py tests/test_cli_hotkeyd.py
git commit -m "feat(cli): /sonari:keymap slash command and 'sonari keymap' subcommand"
```

---

## Task 11: docs + manual smoke checklist + final review

**Files:**
- Modify: `docs/superpowers/phase1-execution-log.md` (append a Phase 2 section)
- Create: `docs/superpowers/phase2-manual-smoke-checklist.md`

No production code changes here - this is documentation + the final full-suite gate.

- [ ] **Step 1: Append the Phase 2 section to the execution log**

Append to the END of `docs/superpowers/phase1-execution-log.md`:

```markdown

---

## Phase 2 - Control & Selection (built)

**Spec:** `docs/superpowers/specs/2026-06-05-sonari-phase2-control-selection-design.md`
**Plan:** `docs/superpowers/plans/2026-06-05-sonari-phase2-control-selection.md`
**Manual checklist:** `docs/superpowers/phase2-manual-smoke-checklist.md`

### What was built
- **Protocol:** added `reread_options` and `cycle_verbosity` message types (additive;
  PROTOCOL_VERSION stays 1). `set_rate` now accepts an optional `delta`.
- **speechd (daemon.py):** relative rate (`delta`, clamped 100–400, speaks "Rate N.");
  `cycle_verbosity` (everything→medium→quiet→everything, persisted, announced); caches
  the last picker's spoken text for re-read and clears it on flush/session_end;
  `reread_options` re-speaks the cache (or "No options to repeat."); a selection cue
  ("Press the option's number to choose, or Escape to cancel.") + a once-per-session
  "Selecting is immediate." warning are appended at `everything` only; multiSelect and
  >9-options notes appear in any verbosity.
- **keymap (keymap.py):** all key/mod/action resolution + load/merge + writers; emits
  `~/.sonari/hotkeyd.resolved.json`.
- **hotkeyd (hotkeyd/sonari-hotkeyd.swift):** Swift + Carbon `RegisterEventHotKey`;
  reads the resolved JSON, registers each combo, writes the mapped message to
  `~/.sonari/speechd.sock` on fire. No macOS permission required.
- **cli.py:** `install` builds hotkeyd (swiftc), writes default keymap + resolved JSON,
  writes & loads `com.sonari.hotkeyd` LaunchAgent (build failure is non-fatal);
  `uninstall` removes the agent + binary (keeps keymap.json); `doctor` adds swiftc /
  binary / resolved-keymap / keymap-resolves checks; new `sonari keymap` subcommand and
  `/sonari:keymap` slash command print the active bindings.

### Default keymap (Ctrl+Cmd; rebindable in ~/.sonari/keymap.json)
| Combo | Action |
|---|---|
| Ctrl+Cmd+S | stop |
| Ctrl+Cmd+R | repeat |
| Ctrl+Cmd+. | skip |
| Ctrl+Cmd+D | jump_decision |
| Ctrl+Cmd+L | catch_up |
| Ctrl+Cmd+] | faster (+25 wpm) |
| Ctrl+Cmd+[ | slower (-25 wpm) |
| Ctrl+Cmd+V | cycle_verbosity |
| Ctrl+Cmd+O | reread_options |

### Using / rebinding / install
- Hear options: a picker reads its numbered options; press the digit (1–9) to select,
  Esc to cancel. Press Ctrl+Cmd+O to re-read.
- Rebind: edit `~/.sonari/keymap.json` (action → {"key","mods"}), then
  `sonari install` (re-resolves + reloads the agent), or run `sonari keymap` to view.
- Install/uninstall/doctor: `sonari install` / `sonari uninstall` / `sonari doctor`.
  If swiftc is missing, speech still works; only global hotkeys are disabled.
```

- [ ] **Step 2: Create the manual smoke checklist**

Create `docs/superpowers/phase2-manual-smoke-checklist.md`:

```markdown
# Sonari Phase 2 - Manual Smoke Checklist (screen-off, live)

Run these on the real machine after `sonari install`. The deterministic Python suite
covers daemon/keymap/cli logic and Swift compilation; this covers the Carbon runtime
behavior and Claude Code picker behavior, which cannot be unit-tested. Resolves spec
open questions O-1..O-4.

## Setup
- [ ] `sonari install` succeeded; `sonari doctor` is all-ok (incl. swiftc, hotkeyd
  binary, resolved keymap, keymap resolves).
- [ ] `sonari keymap` prints all 9 actions with Ctrl+Cmd combos.
- [ ] Confirm the hotkey daemon is running: `launchctl list | grep com.sonari.hotkeyd`.

## O-4 - hotkeys fire, no character leak / no beep
For each terminal - **Terminal.app**, **iTerm2**, **VS Code integrated terminal**:
- [ ] Focus the terminal at a shell prompt. Press each combo; confirm NO character is
  inserted at the prompt and NO system beep:
  Ctrl+Cmd+S, +R, +. , +D, +L, +] , +[ , +V, +O.
- [ ] Each fire produces the right speechd reaction (stop silences; faster/slower speak
  "Rate N."; cycle_verbosity speaks the level; reread re-speaks the last picker).
- [ ] LaunchAgent parity: the agent-launched daemon registers identically to a
  shell-launched `~/.sonari/sonari-hotkeyd` (kill the agent's process, run the binary in
  a shell, repeat one combo; behavior matches).

## Numeric selection - AskUserQuestion / permission / plan
- [ ] Trigger a real **AskUserQuestion** picker. Sonari reads "Question … Option 1 … "
  then the cue. Press a digit (1–9) → it selects immediately. Press Esc on another →
  it cancels.
- [ ] Trigger a **permission** prompt. Confirm 1 = first/proceed, Esc = deny, and the
  cue is read at `everything`.
- [ ] Trigger an **ExitPlanMode** plan. Confirm the plan text + cue are read; 1 accepts,
  Esc keeps planning.

## O-1 - multiSelect keys
- [ ] On a multiSelect AskUserQuestion: Sonari reads the "Select multiple…" note.
  Verify the EXACT working keys: digit-toggle vs Space-on-highlighted + Enter to confirm.
  Record the verified behavior; if the narration wording is wrong, fix `_choice_notes`.

## O-2 - multi-question AskUserQuestion
- [ ] On a multi-question picker: confirm a digit selects within the CURRENT sub-question
  and Tab advances to the next (vs submits). Record the result; consider a "Tab moves to
  the next question." note if useful.

## >9 options
- [ ] On a picker with 10+ options: confirm Sonari reads "More than nine options; use
  arrow keys for ten and up." and that arrows reach options 10+.

## Re-read
- [ ] After any picker, press Ctrl+Cmd+O → the exact same options (incl. cue/notes) are
  re-spoken. After Esc/submit + a new prompt (flush), Ctrl+Cmd+O says "No options to
  repeat."

## O-3 - permission_prompt payload
- [ ] Capture a golden `Notification permission_prompt` hook payload. Does it carry the
  option list? Record yes/no. If yes, consider enriching `_permission_text` to read the
  numbered options in a follow-up.
```

- [ ] **Step 3: Run the FULL suite (final green gate)**

Run: `.venv/bin/python -m pytest -q`
Expected: PASS - entire suite green (Phase 1 + all Phase 2 tasks). Investigate and fix
any failure before committing (do NOT relax assertions to force green).

- [ ] **Step 4: Confirm the tree is clean of stray artifacts**

Run: `git status --short`
Expected: only the two doc files staged/modified (all code from Tasks 1-10 already
committed). No `*.tmp`, no built binary, no `~/.sonari` artifacts in the repo.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/phase1-execution-log.md docs/superpowers/phase2-manual-smoke-checklist.md
git commit -m "docs: Phase 2 execution log section + manual smoke checklist"
```

---

## Self-Review - Spec coverage

Mapping each spec section to the task(s) that implement it (spec:
`2026-06-05-sonari-phase2-control-selection-design.md`):

| Spec section | Requirement | Task(s) |
|---|---|---|
| §3.1 hotkeyd | Swift Carbon RegisterEventHotKey daemon, reads keymap, sends to socket, .accessory | 8 (binary), 7 (keymap logic), 9 (build+LaunchAgent) |
| §3.1 keymap | key/mod name → code/mask, action → message, default keymap, registration-conflict tolerance | 7 (resolve/load/defaults), 8 (per-combo continue-on-failure) |
| §3.2.1 reread_options | cache CHOICE/PLAN/PERMISSION text, re-speak, "no options", clear on flush/session_end | 4 |
| §3.2.2 relative rate | `delta`, clamp 100–400, terse "Rate N." confirmation, absolute still works | 2 |
| §3.2.3 cycle_verbosity | everything→medium→quiet→everything, persist, announce | 3 |
| §3.2.4 cue + warning | cue + once-per-session "Selecting is immediate." at `everything` only | 5 |
| §4 protocol | add REREAD_OPTIONS, CYCLE_VERBOSITY; SET_RATE delta; no version bump | 1 (types), 2 (delta) |
| §5 install/uninstall/doctor | build hotkeyd, write keymap+resolved, LaunchAgent, doctor checks; /sonari:keymap | 9 (install/uninstall/doctor), 10 (keymap subcommand + slash command) |
| §6 >9 options | "More than nine options; use arrow keys for ten and up." (any mode) | 5 |
| §6 multiSelect | "Select multiple: press each number, or Space…then Enter" (any mode) | 5 (narration), 11 (live key verification O-1) |
| §6 multi-question Tab | live-verify Tab behavior | 11 (O-2) |
| §6 permission payload (O-3) | does payload carry options? capture golden | 11 (O-3) |
| §6 hotkey conflict | announce/log which combo failed; rest still work | 8 |
| §7 deterministic tests | reread/clear, rate-clamp+confirm, cycle, cue/warning gating + once-per-session, >9/multiSelect notes | 2,3,4,5 |
| §7 hotkeyd logic tests | keymap name→code/mask, action→JSON golden strings, resolved-shape contract | 7, 8, 9 (contract) |
| §7 live smoke checklist | each hotkey, numeric selection, multiSelect, Tab, >9, re-read, no leak/beep, LaunchAgent parity | 11 |
| §7 doctor as smoke test | swiftc / binary / resolved / keymap-resolves | 9 |
| §8 O-1..O-4 | resolved empirically during the build | 11 (checklist) |

**Placeholder scan:** no "TODO"/"similar to Task N"/"handle edge cases" - every code/test
step pastes complete code.

**Type/name consistency (verified across tasks):**
- `RATE_MIN`/`RATE_MAX` defined in Task 2, reused implicitly by Task 9's contract test
  (faster/slower assert 225/175 within bounds).
- `self._last_options` (Task 4) cached in the SAME `text` local that Task 5 enriches, so
  re-read replays cue+notes - verified by Task 5's `test_reread_includes_cue_and_notes`.
- `self._warned_immediate` (Task 5) keyed by session; matches the per-session warning test.
- `keymap.ACTION_MESSAGES` (Task 7) message dicts == the protocol the daemon handles
  (Tasks 1-5), proven by Task 9's `test_hotkeyd_contract.py`.
- `keymap.resolve_keymap` output keys `action/keyCode/modifiers/message` (Task 7) == the
  keys the Swift `HotkeyEntry` parses (Task 8) == the keys Task 9 doctor + Task 10
  `_combo_label` read.
- `HOTKEYD_LAUNCH_AGENT_LABEL`/`HOTKEYD_LAUNCH_AGENT_PATH`, `_hotkeyd_plist`,
  `_build_hotkeyd` (Task 9) are the exact names the Task 9/10 tests reference.
- Default combos in `DEFAULT_KEYMAP` (Task 7) == the table in the spec §3.1 and the
  execution-log table (Task 11).
```
