# Summary Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Opt-in summary mode: when a foreground turn finishes, Sonara reads aloud a 1-2 sentence AI summary produced by a separate throwaway `claude -p` Haiku call, instead of the full narration. The main Claude session is never touched.

**Architecture:** A new pure-ish `summarizer.py` module wraps the headless subprocess call (injectable runner for tests). The daemon gains a `SET_SUMMARY_MODE` toggle, suppresses prose speech when the mode is on (record-to-history only, exactly like `quiet`), and on the `turn_done` earcon dispatches a worker thread that calls the summarizer OFF-lock and enqueues the result (or fires a `summary_failed` earcon). CLI + slash command toggle it; doctor checks the summarizer command resolves.

**Tech Stack:** Python 3.9+ stdlib only (`subprocess`, `shutil`, `threading`). pytest. The summarizer subprocess is `claude -p <instruction> --model <model> --tools ""` with the turn text on stdin — verified working end-to-end on this machine (returns a clean 1-sentence summary; `--tools ""` disables all tools; `haiku` model alias accepted).

## Global Constraints

- Python 3.9+ compatible; stdlib only; no new dependencies.
- No em-dashes in code comments or docs (use en-dashes, commas, or rephrase).
- `summary_mode` is OFF by default; the main Claude session is never modified (no hook injection anywhere).
- Decisions (choice/plan/permission) are still spoken in full when summary mode is on; all earcons still fire.
- Prose is still recorded to history when suppressed (catch-up / re-read must keep working).
- The summarizer subprocess must never run under `self._lock`; only the result enqueue takes the lock.
- Failure (non-zero exit, timeout, empty output, spawn error) means: `summary_failed` earcon, speak nothing, never crash.
- v1 summarizes only the foreground session's turns.
- Run tests from the venv: `./.venv/Scripts/python.exe -m pytest <files> -q` from the repo root.
- The full suite has 19 PRE-EXISTING env-only failures in: test_win_tts, test_winfakes, test_transport, test_paths, test_win_autostart, test_bin_sonara, test_daemon_ducking. They are not yours; do not touch them; add no new failures.

---

### Task 1: `SET_SUMMARY_MODE` protocol type + config defaults

**Files:**
- Modify: `src/sonara/protocol.py` (the `MsgType` class, after `SET_DUCK_LEVEL`)
- Modify: `src/sonara/config.py` (the `DEFAULTS` dict)
- Test: `tests/test_protocol.py` (both snapshot dicts), `tests/test_config.py`

**Interfaces:**
- Produces: `MsgType.SET_SUMMARY_MODE == "set_summary_mode"`; config defaults `summary_mode=False`, `summary_model="haiku"`, `summary_command="claude"`, `summary_timeout=20`. Consumed by Tasks 3-5.

- [ ] **Step 1: Update the protocol snapshot tests**

In `tests/test_protocol.py`, add to BOTH expected dicts (in `test_msgtype_has_every_constant_with_exact_values` and `test_msgtype_defines_no_extra_string_constants`), after the `"SET_DUCK_LEVEL": "set_duck_level",` line in each:

```python
        "SET_SUMMARY_MODE": "set_summary_mode",
```

- [ ] **Step 2: Add config default tests**

Append to `tests/test_config.py`:

```python
def test_summary_mode_defaults():
    from sonara.config import DEFAULTS
    assert DEFAULTS["summary_mode"] is False
    assert DEFAULTS["summary_model"] == "haiku"
    assert DEFAULTS["summary_command"] == "claude"
    assert DEFAULTS["summary_timeout"] == 20
```

- [ ] **Step 3: Run both test files to verify the new tests fail**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_protocol.py tests/test_config.py -q`
Expected: FAIL — missing `SET_SUMMARY_MODE` (two snapshot tests) and `KeyError: 'summary_mode'`.

- [ ] **Step 4: Add the constant and the defaults**

In `src/sonara/protocol.py`, after the `SET_DUCK_LEVEL` line:

```python
    SET_SUMMARY_MODE = "set_summary_mode"     # toggle spoken turn summaries
```

In `src/sonara/config.py`, extend `DEFAULTS` (after the `"duck_level": 30,` line):

```python
    "summary_mode": False,    # speak an AI recap of each finished turn (opt-in)
    "summary_model": "haiku",           # model alias for the throwaway claude -p call
    "summary_command": "claude",        # executable for the summarizer subprocess
    "summary_timeout": 20,              # seconds before a summarizer call is abandoned
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_protocol.py tests/test_config.py -q`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/sonara/protocol.py src/sonara/config.py tests/test_protocol.py tests/test_config.py
git commit -m "feat(protocol,config): SET_SUMMARY_MODE type + summary-mode defaults"
```

---

### Task 2: `summarizer.py` module

**Files:**
- Create: `src/sonara/summarizer.py`
- Test: `tests/test_summarizer.py` (new)

**Interfaces:**
- Produces: `summarize(text, *, model, command="claude", timeout=20, runner=None) -> str | None` and `build_argv(command, model) -> list`. `runner` is a callable `(argv, text, timeout) -> (returncode, stdout_str)`; the default spawns the real subprocess. Task 4's daemon worker calls `summarize`.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_summarizer.py`:

```python
"""The out-of-band turn summarizer: a throwaway tool-disabled `claude -p` call.
All tests inject a fake runner; nothing here spawns a real process."""
import pytest

from sonara import summarizer


def _ok_runner(result="A short recap."):
    calls = []

    def run(argv, text, timeout):
        calls.append({"argv": argv, "text": text, "timeout": timeout})
        return 0, result
    return run, calls


def test_success_returns_trimmed_stdout():
    run, calls = _ok_runner("  The gist of it.\n")
    out = summarizer.summarize("long text", model="haiku", runner=run)
    assert out == "The gist of it."
    assert calls[0]["text"] == "long text"
    assert calls[0]["timeout"] == 20            # default timeout


def test_argv_is_headless_tool_disabled_call():
    argv = summarizer.build_argv("claude", "haiku")
    assert argv[0] == "claude"
    assert "-p" in argv
    assert "--model" in argv and argv[argv.index("--model") + 1] == "haiku"
    # --tools "" disables every tool: pure text-in/text-out
    assert "--tools" in argv and argv[argv.index("--tools") + 1] == ""


def test_nonzero_exit_returns_none():
    out = summarizer.summarize("t", model="haiku", runner=lambda a, t, s: (1, "oops"))
    assert out is None


def test_empty_stdout_returns_none():
    out = summarizer.summarize("t", model="haiku", runner=lambda a, t, s: (0, "  \n"))
    assert out is None


def test_runner_exception_returns_none():
    def boom(argv, text, timeout):
        raise RuntimeError("spawn failed")
    assert summarizer.summarize("t", model="haiku", runner=boom) is None


def test_empty_text_short_circuits_without_calling_runner():
    def never(argv, text, timeout):
        raise AssertionError("runner must not be called for empty text")
    assert summarizer.summarize("   ", model="haiku", runner=never) is None


def test_command_and_timeout_are_forwarded():
    run, calls = _ok_runner()
    summarizer.summarize("t", model="haiku", command="claude-custom",
                         timeout=5, runner=run)
    assert calls[0]["argv"][0] == "claude-custom"
    assert calls[0]["timeout"] == 5
```

- [ ] **Step 2: Run to verify it fails**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_summarizer.py -q`
Expected: FAIL — `ModuleNotFoundError: No module named 'sonara.summarizer'`.

- [ ] **Step 3: Implement the module**

Create `src/sonara/summarizer.py`:

```python
"""Out-of-band turn summarizer: a throwaway `claude -p` call.

The user's main Claude session is NEVER touched. This module spawns a separate,
tool-disabled headless process whose context contains ONLY the piped turn text
plus a fixed instruction, reads one short summary back, and exits. It reuses the
user's existing Claude Code login; a failure of any kind maps to None (the daemon
then plays a brief cue instead of speaking).
"""
from __future__ import annotations

import os
import shutil
import subprocess

INSTRUCTION = (
    "You turn a coding assistant's finished message into a spoken recap. "
    "Reply with ONLY 1-2 short plain sentences capturing the gist. "
    "No markdown, no preamble."
)


def build_argv(command: str, model: str) -> list:
    """The headless summarizer invocation. --tools "" disables every tool, so the
    call is pure text-in/text-out: it cannot read files or run commands."""
    return [command, "-p", INSTRUCTION, "--model", model, "--tools", ""]


def _default_runner(argv, text: str, timeout):
    """Spawn the real subprocess: text on stdin, neutral cwd (the user home, so a
    project CLAUDE.md is never picked up), no console window on Windows. Resolve
    the command via shutil.which because Windows CreateProcess does not apply
    PATHEXT to a bare name like 'claude' (an npm .cmd shim)."""
    exe = shutil.which(argv[0]) or argv[0]
    kwargs = {}
    if os.name == "nt":
        kwargs["creationflags"] = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    proc = subprocess.run(
        [exe] + list(argv[1:]),
        input=text.encode("utf-8"),
        capture_output=True,
        timeout=timeout,
        cwd=os.path.expanduser("~"),
        **kwargs
    )
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def summarize(text, *, model, command: str = "claude", timeout=20, runner=None):
    """A 1-2 sentence spoken summary of *text*, or None on ANY failure
    (non-zero exit, timeout, empty output, spawn error, empty input)."""
    if not (text or "").strip():
        return None
    run = runner or _default_runner
    try:
        code, out = run(build_argv(command, model), text, timeout)
    except Exception:  # noqa: BLE001 - a summarizer failure must never propagate
        return None
    if code != 0:
        return None
    out = (out or "").strip()
    return out or None
```

- [ ] **Step 4: Run to verify it passes**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_summarizer.py -q`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add src/sonara/summarizer.py tests/test_summarizer.py
git commit -m "feat(summarizer): throwaway tool-disabled claude -p recap call"
```

---

### Task 3: daemon toggle + prose gate + status field

**Files:**
- Modify: `src/sonara/daemon.py` — three spots: the PROSE handler's speech gate (`verbosity != "quiet"`, ~line 314), a new `SET_SUMMARY_MODE` branch after the `SET_DUCK_LEVEL` branch (~line 732), and the STATUS reply dict (~line 750).
- Test: `tests/test_daemon_summary_mode.py` (new)

**Interfaces:**
- Consumes: `MsgType.SET_SUMMARY_MODE`, config key `summary_mode` (Task 1).
- Produces: handling of `{"type": "set_summary_mode", "enabled": bool}`; prose suppressed-but-recorded when `summary_mode` is on; `summary_mode` in the STATUS reply. Task 4 builds on the same test file.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_daemon_summary_mode.py`:

```python
"""Summary mode: SET_SUMMARY_MODE toggles + prose is recorded but not spoken."""
from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _prose(session, text, idx=0, final=True):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": session,
            "delta": text, "index": idx, "final": final}


def _set_mode(daemon, enabled):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_SUMMARY_MODE,
                           "enabled": enabled})


def test_set_summary_mode_toggles_and_persists(monkeypatch):
    import sonara.daemon as daemon_module
    saved = []
    monkeypatch.setattr(daemon_module, "save_config", saved.append)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    assert config["summary_mode"] is True
    _set_mode(daemon, False)
    assert config["summary_mode"] is False
    assert len(saved) == 2


def test_set_summary_mode_without_enabled_is_noop(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_SUMMARY_MODE})
    assert config["summary_mode"] is False


def test_summary_mode_suppresses_prose_speech_but_records_history(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "A long explanation. "))
    ch = daemon.router.channel("fg")
    assert ch.pending() == 0                              # nothing queued to speak
    assert daemon.history.unheard("fg")                   # but history recorded it


def test_summary_mode_off_prose_is_spoken_as_today():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_prose("fg", "Hello there. "))
    assert daemon.router.channel("fg").pending() > 0


def test_decisions_still_spoken_with_summary_mode_on(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE,
                           "session": "fg",
                           "questions": [{"question": "Pick one?",
                                          "options": ["a", "b"]}]})
    ch = daemon.router.channel("fg")
    assert any(it.is_decision for it in ch.items[ch.cursor:])


def test_status_reports_summary_mode(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    reply = daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.STATUS})
    assert reply["summary_mode"] is True
```

- [ ] **Step 2: Run to verify the new tests fail**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_daemon_summary_mode.py -q`
Expected: FAIL — `set_summary_mode` unhandled (toggle asserts fail), prose still enqueued, STATUS lacks the key.

- [ ] **Step 3: Implement the three daemon changes**

(a) PROSE gate — in the PROSE handler, change:

```python
                if verbosity != "quiet":
```

to:

```python
                # quiet verbosity AND summary mode both record prose to history
                # without enqueueing speech (summary mode reads a recap at turn
                # end instead; catch_up / re-read still work from history).
                if verbosity != "quiet" and not self.config.get("summary_mode"):
```

(b) New branch, inserted after the `SET_DUCK_LEVEL` branch's `return None` and before `if t == MsgType.CYCLE_VERBOSITY:` (modeled on SET_AUDIO_CONTROL):

```python
        if t == MsgType.SET_SUMMARY_MODE:
            if "enabled" not in msg:
                return None
            enabled = bool(msg.get("enabled"))
            self.config["summary_mode"] = enabled
            save_config(self.config)
            target = self.router.active or self.sessions.foreground()
            self._speak_cue(target,
                            "Summary mode on." if enabled else "Summary mode off.",
                            exempt_mute=True)
            self._wake.set()
            return None
```

(c) STATUS reply — add to the reply dict (next to `"verbosity"`):

```python
                "summary_mode": bool(self.config.get("summary_mode")),
```

- [ ] **Step 4: Run to verify the new tests pass**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_daemon_summary_mode.py -q`
Expected: PASS (6 tests).

- [ ] **Step 5: Run the daemon regression set**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_daemon_prose.py tests/test_daemon_phase2.py tests/test_daemon_decisions.py tests/test_daemon_settings.py -q`
Expected: PASS (summary_mode defaults to False, so nothing changes for existing tests).

- [ ] **Step 6: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_summary_mode.py
git commit -m "feat(daemon): SET_SUMMARY_MODE toggle + prose gate + status field"
```

---

### Task 4: turn-end summary dispatch (worker thread)

**Files:**
- Modify: `src/sonara/daemon.py` — `__init__` (new fields), the EARCON `turn_done` branch (~line 407), and two new methods (`_maybe_summarize`, `_summary_worker`) placed after `_engaged_session`.
- Test: `tests/test_daemon_summary_mode.py` (extend)

**Interfaces:**
- Consumes: `summarizer.summarize` (Task 2), config keys (Task 1), the prose gate (Task 3).
- Produces: on `turn_done` for the foreground session with summary_mode on and non-empty prose, `_maybe_summarize(session)` computes the turn text and calls `self._start_summary_thread(session, gen, text)` (a one-line thread spawner, monkeypatchable in tests). `_summary_worker(session, gen, text)` runs the summarizer OFF-lock, then under the lock enqueues the summary (or fires the `summary_failed` earcon). `SpeechDaemon.__init__` gains an optional `summarize_fn=None` test seam.

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_daemon_summary_mode.py`:

```python
# --- turn-end summary dispatch ------------------------------------------

def _turn_done(daemon, session="fg"):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": session})


def _capture_spawn(daemon, monkeypatch):
    calls = []
    monkeypatch.setattr(daemon, "_start_summary_thread",
                        lambda session, gen, text: calls.append((session, gen, text)))
    return calls


def _enable_and_feed(daemon, monkeypatch, session="fg"):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message(_prose(session, "First part. ", 0, True))
    daemon.handle_message(_prose(session, "Second part. ", 1, True))


def test_turn_done_dispatches_summary_for_foreground(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    assert len(calls) == 1
    session, gen, text = calls[0]
    assert session == "fg"
    assert "First part." in text and "Second part." in text


def test_turn_done_does_not_dispatch_when_mode_off(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    daemon.handle_message(_prose("fg", "Text. "))
    _turn_done(daemon)
    assert calls == []


def test_turn_done_does_not_dispatch_for_background_session(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch, session="bg")
    _turn_done(daemon, session="bg")
    assert calls == []


def test_turn_done_with_no_prose_does_not_dispatch(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _set_mode(daemon, True)
    _turn_done(daemon)                                   # decision-only / empty turn
    assert calls == []


def test_worker_success_enqueues_summary(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The gist."
    daemon._summary_worker(*calls[0])                    # run inline, outside the lock
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert "The gist." in texts


def test_worker_failure_fires_cue_and_enqueues_nothing(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: None
    daemon._summary_worker(*calls[0])
    assert speaker.earcons[-1] == "summary_failed"
    assert daemon.router.channel("fg").pending() == 0


def test_superseded_worker_result_is_dropped(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # gen 1
    daemon.handle_message(_prose("fg", "More text. ", 2, True))
    _turn_done(daemon)                                   # gen 2 supersedes
    daemon._summarize_fn = lambda text, **kw: "Stale summary."
    daemon._summary_worker(*calls[0])                    # gen-1 result arrives late
    ch = daemon.router.channel("fg")
    assert "Stale summary." not in [it.text for it in ch.items[ch.cursor:]]


def test_worker_forwards_config_to_summarizer(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    config["summary_model"] = "haiku"
    config["summary_command"] = "claude"
    config["summary_timeout"] = 20
    _turn_done(daemon)
    seen = {}

    def fake(text, **kw):
        seen.update(kw)
        return "Recap."
    daemon._summarize_fn = fake
    daemon._summary_worker(*calls[0])
    assert seen == {"model": "haiku", "command": "claude", "timeout": 20}
```

- [ ] **Step 2: Run to verify the new tests fail**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_daemon_summary_mode.py -q`
Expected: FAIL — `_start_summary_thread` does not exist (monkeypatch AttributeError).

- [ ] **Step 3: Implement the dispatch**

(a) In `SpeechDaemon.__init__`, after the `self._hotkey_q` line:

```python
        # Summary mode: per-session generation counter; a new turn-end supersedes
        # any older in-flight summarizer so a stale result is dropped, not spoken.
        self._summary_gen: dict = {}
        self._summarize_fn = None      # test seam; None -> sonara.summarizer.summarize
```

(b) In the EARCON handler's `turn_done` branch, after `self._wake.set()` and before its `return None`:

```python
                self._maybe_summarize(session)
```

(c) Add the two methods after `_engaged_session`:

```python
    def _maybe_summarize(self, session: str) -> None:
        """Summary mode: on turn end, recap the foreground session's prose via a
        separate throwaway claude -p call (see summarizer.py). Runs under the
        daemon lock, so it only gathers text and spawns the worker thread; the
        subprocess itself runs OFF-lock in _summary_worker."""
        if not self.config.get("summary_mode"):
            return
        if not self.sessions.is_foreground(session):
            return                       # v1: foreground turns only
        texts = []
        for mid in self.history.message_ids(session):
            for e in self.history.entries_for_message(session, mid):
                if e.kind == "prose":
                    texts.append(e.text)
        text = " ".join(texts).strip()
        if not text:
            return                       # decision-only / empty turn: nothing to recap
        gen = self._summary_gen.get(session, 0) + 1
        self._summary_gen[session] = gen
        self._start_summary_thread(session, gen, text)

    def _start_summary_thread(self, session: str, gen: int, text: str) -> None:
        threading.Thread(target=self._summary_worker, args=(session, gen, text),
                         name="sonara-summary", daemon=True).start()

    def _summary_worker(self, session: str, gen: int, text: str) -> None:
        """Run the summarizer subprocess OFF-lock, then apply the result under the
        lock: enqueue the spoken summary, or fire the failure cue. A result whose
        generation was superseded by a newer turn end is dropped silently."""
        from sonara import summarizer
        fn = self._summarize_fn or summarizer.summarize
        try:
            summary = fn(text,
                         model=self.config.get("summary_model", "haiku"),
                         command=self.config.get("summary_command", "claude"),
                         timeout=self.config.get("summary_timeout", 20))
        except Exception:  # noqa: BLE001 - a summary failure must never crash the daemon
            summary = None
        with self._lock:
            if self._summary_gen.get(session) != gen:
                return                   # superseded: a newer turn owns the voice
            if summary:
                entry = self.history.record(session, "prose", summary)
                self._enqueue(session, "prose", summary, False, entry=entry)
                self.router.channel(session).turn_done = True
                self._wake.set()
            else:
                self._earcon("summary_failed")
```

- [ ] **Step 4: Run to verify the tests pass**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_daemon_summary_mode.py -q`
Expected: PASS (14 tests).

- [ ] **Step 5: Run the daemon regression set**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_daemon_prose.py tests/test_daemon_loop.py tests/test_daemon_channels.py tests/test_daemon_multisession.py tests/test_hotkeyd_contract.py -q`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_summary_mode.py
git commit -m "feat(daemon): turn-end summary dispatch via off-lock worker"
```

---

### Task 5: CLI command, slash command, doctor check

**Files:**
- Modify: `src/sonara/cli.py` — new `_cmd_summary` (after `_cmd_duck_level`), parser registration in `_build_parser` (after the `duck-level` block), doctor row in `doctor()` (after the `keymap resolves` block).
- Create: `commands/summary.md`
- Test: `tests/test_cli_control.py` (extend), `tests/test_cli_doctor.py` (extend)

**Interfaces:**
- Consumes: `MsgType.SET_SUMMARY_MODE` (Task 1), config keys via `load_config`.
- Produces: `sonara summary on|off` sends the toggle; bare `sonara summary` prints the persisted state; doctor row `summary command` (ok when mode off, or when the command resolves on PATH).

- [ ] **Step 1: Write the failing CLI tests**

Append to `tests/test_cli_control.py`:

```python
def test_summary_on_sends_set_summary_mode():
    from unittest import mock
    from sonara import cli
    sent = []
    with mock.patch("sonara.client.send", side_effect=lambda m, **k: sent.append(m)):
        rc = cli.main(["summary", "on"])
    assert rc == 0
    assert sent[-1]["type"] == "set_summary_mode" and sent[-1]["enabled"] is True


def test_summary_off_sends_disabled():
    from unittest import mock
    from sonara import cli
    sent = []
    with mock.patch("sonara.client.send", side_effect=lambda m, **k: sent.append(m)):
        rc = cli.main(["summary", "off"])
    assert rc == 0
    assert sent[-1]["enabled"] is False


def test_bare_summary_prints_state(capsys, monkeypatch, tmp_path):
    from sonara import cli, config
    monkeypatch.setattr(config, "CONFIG_PATH", tmp_path / "config.json")
    rc = cli.main(["summary"])
    assert rc == 0
    assert "off" in capsys.readouterr().out.lower()      # default state
```

Append to `tests/test_cli_doctor.py`:

```python
def test_doctor_summary_row_ok_when_mode_off(monkeypatch, tmp_path):
    from sonara import cli, config
    monkeypatch.setattr(config, "CONFIG_PATH", tmp_path / "config.json")
    rows = {name: (ok, detail) for name, ok, detail in cli.doctor()}
    ok, detail = rows["summary command"]
    assert ok is True and "off" in detail


def test_doctor_summary_row_fails_when_command_missing(monkeypatch, tmp_path):
    import json
    from sonara import cli, config
    cfg_path = tmp_path / "config.json"
    cfg_path.write_text(json.dumps({"summary_mode": True,
                                    "summary_command": "definitely-not-a-cmd-xyz"}),
                        encoding="utf-8")
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_path)
    rows = {name: (ok, detail) for name, ok, detail in cli.doctor()}
    ok, detail = rows["summary command"]
    assert ok is False
```

- [ ] **Step 2: Run to verify they fail**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_cli_control.py tests/test_cli_doctor.py -q`
Expected: FAIL — argparse rejects the unknown `summary` subcommand; doctor has no `summary command` row.

- [ ] **Step 3: Implement CLI + doctor**

(a) `_cmd_summary`, after `_cmd_duck_level`:

```python
def _cmd_summary(args) -> int:
    if not args.state:
        from sonara.config import load_config
        on = bool(load_config().get("summary_mode"))
        print("Summary mode is {0}.".format("on" if on else "off"))
        return 0
    enabled = args.state == "on"
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_SUMMARY_MODE,
           "enabled": enabled})
    print("Summary mode {0}.".format("on" if enabled else "off"))
    return 0
```

(b) Parser registration in `_build_parser`, after the `duck-level` block:

```python
    sp = sub.add_parser(
        "summary", help="speak an AI recap of each finished turn (on|off)")
    sp.add_argument("state", nargs="?", choices=["on", "off"])
    sp.set_defaults(func=_cmd_summary)
```

(c) Doctor row in `doctor()`, after the `keymap resolves` block:

```python
    # Summary mode: the summarizer command must resolve when the mode is on
    # (the daemon spawns it per turn; a missing command means a failure cue
    # on every turn with no visible cause).
    try:
        from sonara.config import load_config as _load_cfg
        _cfg = _load_cfg()
        if not _cfg.get("summary_mode"):
            results.append(("summary command", True, "summary mode off"))
        else:
            import shutil as _shutil
            _cmd = _cfg.get("summary_command", "claude")
            _found = _shutil.which(_cmd)
            results.append(("summary command", bool(_found),
                            _found or "'{0}' not found on PATH".format(_cmd)))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("summary command", False, f"error: {exc}"))
```

(d) Create `commands/summary.md`:

```markdown
---
description: Toggle Summary mode (speak an AI recap of each finished turn)
argument-hint: [on|off]
---

Run the Sonara summary command with the Bash tool, forwarding any arguments:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" summary $ARGUMENTS
```

Print the command's output to the user verbatim. With no arguments it prints
whether summary mode is on. Note: summary mode sends each finished message to a
separate local `claude -p` call to produce the spoken recap; it is off by default.
```

- [ ] **Step 4: Run to verify they pass**

Run: `./.venv/Scripts/python.exe -m pytest tests/test_cli_control.py tests/test_cli_doctor.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonara/cli.py commands/summary.md tests/test_cli_control.py tests/test_cli_doctor.py
git commit -m "feat(cli): sonara summary on|off + /sonara:summary + doctor row"
```

---

### Task 6: docs (README + PRIVACY)

**Files:**
- Modify: `README.md` (slash-command table + a new Summary mode section after the Verbosity section)
- Modify: `PRIVACY.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Add the command row**

In the README slash-command table, after the `/sonara:minqueue` row:

```
| `/sonara:summary [on\|off]` | `sonara summary [on\|off]` | Speak an AI recap of each finished turn instead of full narration (off = full narration; bare prints state) |
```

- [ ] **Step 2: Add the Summary mode section**

Insert after the `## Verbosity` section:

```markdown
## Summary mode

`sonara summary on` switches Sonara to a recap style: instead of narrating a whole
response, Sonara waits for the message to finish and reads a 1-2 sentence summary.
Decisions (questions, plans, permission prompts) are still read in full, every
earcon still fires, and the full text stays in history, so `sonara repeat` and
catch-up can still read everything.

How it works: when a turn finishes, Sonara runs a separate, throwaway
`claude -p` call (default model: Haiku, tool-disabled) with only that message's
text and speaks the result. Your main Claude session is untouched, and nothing is
added to its context. The recap call reuses your existing Claude Code login and its
tokens count against your plan (one small call per finished message); expect a few
seconds between the message finishing and the recap being spoken. If the call
fails (offline, timeout), Sonara plays a brief cue and stays quiet - the full text
remains available via catch-up. Summary mode is off by default.
```

- [ ] **Step 3: Update PRIVACY.md**

Read `PRIVACY.md` first, then append a section (adapting to its existing tone):

```markdown
## Summary mode (opt-in)

With summary mode ON (`sonara summary on`, off by default), each finished
assistant message is sent to a separate local `claude -p` process to produce the
short spoken recap. That call is made with your own Claude Code login and is
subject to Anthropic's terms, exactly like the Claude Code session that produced
the message; Sonara itself still stores nothing and operates no servers. With
summary mode OFF (the default), Sonara sends nothing anywhere, as described above.
```

Also update any absolute "sends nothing over the network" claims in PRIVACY.md and the README Privacy section to carve out opt-in summary mode (e.g. "sends nothing over the network (except, if you opt in, summary mode's local `claude -p` call described below)").

- [ ] **Step 4: Verify docs consistency**

Run: `git grep -n "summary" README.md PRIVACY.md | head -20`
Expected: the new rows/sections present; no em-dashes in the added text (en-dashes/commas only).

- [ ] **Step 5: Commit**

```bash
git add README.md PRIVACY.md
git commit -m "docs: summary mode section + privacy carve-out"
```

---

## Self-Review

**Spec coverage:** toggle + persistence (T1/T3), summarizer module with isolation (`--tools ""`, neutral cwd, `shutil.which` for the Windows .cmd shim) (T2), prose suppressed-but-recorded (T3), decisions unchanged (T3 test), turn-end foreground-only dispatch with empty-turn guard (T4), off-lock subprocess + lock-only enqueue (T4), failure earcon `summary_failed` with silent-no-op wav convention (T4; no asset shipped, matching existing user-supplied earcon convention), supersede/no-pile-up via generation counter (T4), CLI + slash + doctor (T5), README + PRIVACY (T6). Feasibility of the exact subprocess invocation was verified live on this machine before planning. ✓

**Placeholder scan:** none — every step has exact code/text. ✓

**Type consistency:** `summarize(text, *, model, command, timeout, runner)` matches the worker's call (`model`/`command`/`timeout` kwargs, asserted in `test_worker_forwards_config_to_summarizer`); `_start_summary_thread(session, gen, text)` matches the monkeypatch seam; `_summarize_fn` name is consistent across T4 code and tests; `SET_SUMMARY_MODE`/`"set_summary_mode"`/`summary_mode` consistent across T1/T3/T5. ✓

**Note for the implementer of T4:** `handle_message` already holds `self._lock` when `_maybe_summarize` runs — that is why the method must not call the summarizer inline and why tests invoke `_summary_worker` directly (outside the lock) rather than through a real thread.
