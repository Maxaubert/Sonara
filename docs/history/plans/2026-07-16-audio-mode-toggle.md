# Audio Behavior Mode (Off / Duck / Pause) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Sonara's boolean audio-ducking setting with a three-way audio behavior mode (Off / Duck / Pause) and add a "Pause" behavior that pauses playing media via Windows SMTC while Sonara speaks and resumes it when idle.

**Architecture:** A new `audio_mode` config key drives daemon routing. At playback start the daemon engages the mode's backend (the existing `AudioDucker` for `duck`, a new `MediaPauser` for `pause`) and disengages both at global idle. `MediaPauser` mirrors `AudioDucker`'s best-effort, never-raise, crash-safe-state-file contract, using the `winrt` SMTC API behind patchable module-level seams so tests never touch real WinRT. The settings page swaps its on/off switch for a segmented control identical to the summary-mode control.

**Tech Stack:** Python 3.14 stdlib for the daemon core; `winrt` (already a dependency) for SMTC pause/resume; vanilla JS + the existing settings-page helpers.

## Global Constraints

- Python 3.14, stdlib-only in the daemon core. Pause backend uses `winrt` (already a project dependency).
- No em-dashes in code, comments, copy, or docs. Use en-dashes, commas, or rephrase.
- Pause backend is Windows-only. Non-Windows / missing `winrt` / tests get `NullPauser`.
- Every public method of `MediaPauser` is best-effort and never raises: a pause/resume failure must never break, block, or delay speech.
- All `winrt.*` imports in the pause backend are lazy (inside functions).
- `audio_mode` values are exactly `"off"`, `"duck"`, `"pause"`. Default `"off"`.
- Migration: a persisted config with no `audio_mode` but `audio_control: true` loads as `audio_mode="duck"`; otherwise `"off"`.
- Cue wording on mode change: `off` -> "Audio off.", `duck` -> "Audio ducking.", `pause` -> "Media pause."
- Settings mutations flow through `daemon.handle_message` under the daemon lock (existing webui `_dispatch`).

---

### Task 1: Config default + legacy migration

**Files:**
- Modify: `src/sonara/config.py` (DEFAULTS ~line 16-17; `load_config` ~line 70-83)
- Test: `tests/test_config.py`

**Interfaces:**
- Produces: `DEFAULTS["audio_mode"] == "off"`; `load_config()` returns a dict whose `audio_mode` is migrated from legacy `audio_control` when absent.

- [ ] **Step 1: Write the failing test**

Add to `tests/test_config.py`:

```python
import json
import sonara.config as config_mod
from sonara.config import DEFAULTS, load_config


def test_audio_mode_default_is_off():
    assert DEFAULTS["audio_mode"] == "off"


def test_audio_mode_migrates_from_legacy_audio_control_true(tmp_path, monkeypatch):
    path = tmp_path / "config.json"
    path.write_text(json.dumps({"audio_control": True}), encoding="utf-8")
    monkeypatch.setattr(config_mod, "CONFIG_PATH", path)
    assert load_config()["audio_mode"] == "duck"


def test_audio_mode_off_when_legacy_absent(tmp_path, monkeypatch):
    path = tmp_path / "config.json"
    path.write_text(json.dumps({"rate": 190}), encoding="utf-8")
    monkeypatch.setattr(config_mod, "CONFIG_PATH", path)
    assert load_config()["audio_mode"] == "off"


def test_explicit_audio_mode_is_not_overridden_by_migration(tmp_path, monkeypatch):
    path = tmp_path / "config.json"
    path.write_text(json.dumps({"audio_control": True, "audio_mode": "pause"}),
                    encoding="utf-8")
    monkeypatch.setattr(config_mod, "CONFIG_PATH", path)
    assert load_config()["audio_mode"] == "pause"
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_config.py -k audio_mode -v`
Expected: FAIL (`KeyError: 'audio_mode'` / assertion errors).

- [ ] **Step 3: Add the default**

In `src/sonara/config.py`, in `DEFAULTS`, directly after the `duck_level` line (line 17) add:

```python
    "audio_mode": "off",      # off | duck | pause -- pause pauses SMTC media (#92)
```

- [ ] **Step 4: Add migration to `load_config`**

In `load_config`, replace the final `return _deep_merge(base, persisted)` (line 83) with:

```python
    merged = _deep_merge(base, persisted)
    # Migrate the pre-#92 boolean into the three-way mode when the persisted file
    # predates audio_mode: audio_control True -> "duck", otherwise the default "off".
    if "audio_mode" not in persisted and persisted.get("audio_control"):
        merged["audio_mode"] = "duck"
    return merged
```

- [ ] **Step 5: Run to verify pass**

Run: `python -m pytest tests/test_config.py -k audio_mode -v`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/sonara/config.py tests/test_config.py
git commit -m "feat(config): audio_mode default + migration from legacy audio_control (#92)"
```

---

### Task 2: Protocol SET_AUDIO_MODE

**Files:**
- Modify: `src/sonara/protocol.py` (MsgType, ~line 35)
- Test: `tests/test_protocol.py`

**Interfaces:**
- Produces: `MsgType.SET_AUDIO_MODE == "set_audio_mode"`.

- [ ] **Step 1: Write the failing test**

Add to `tests/test_protocol.py`:

```python
def test_set_audio_mode_type_exists():
    from sonara.protocol import MsgType
    assert MsgType.SET_AUDIO_MODE == "set_audio_mode"
```

If `tests/test_protocol.py` has a pinned-set test enumerating all message types, add `"set_audio_mode"` to that expected set as well.

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_protocol.py -v`
Expected: FAIL (`AttributeError: SET_AUDIO_MODE`, plus the pinned-set test if present).

- [ ] **Step 3: Add the type**

In `src/sonara/protocol.py`, directly after the `SET_AUDIO_CONTROL` line (line 35) add:

```python
    SET_AUDIO_MODE = "set_audio_mode"         # off | duck | pause (#92)
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_protocol.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonara/protocol.py tests/test_protocol.py
git commit -m "feat(protocol): SET_AUDIO_MODE message type (#92)"
```

---

### Task 3: MediaPauser backend (SMTC)

**Files:**
- Create: `src/sonara/platform/windows/pausing.py`
- Modify: `src/sonara/platform/base.py` (PlatformBackend dataclass, ~line 130)
- Modify: `src/sonara/platform/windows/__init__.py` (make_backend, ~line 8-18)
- Test: `tests/test_pausing.py`

**Interfaces:**
- Produces:
  - `class MediaPauser` with `is_paused() -> bool`, `pause() -> None`, `resume() -> None`.
  - `class NullPauser` with the same three no-op methods.
  - `resume_from_state_file() -> None`.
  - Patchable module seams: `_session_manager()`, `_sessions(mgr)`, `_is_playing(session) -> bool`, `_pause(session)`, `_play(session)`, `_app_id(session) -> str`, and module var `_PAUSE_STATE`.
  - `PlatformBackend.pauser`; `make_backend(...)` sets `pauser=MediaPauser()`.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_pausing.py`:

```python
import json
import sonara.platform.windows.pausing as pausing
from sonara.platform.windows.pausing import MediaPauser, NullPauser


class _FakeSession:
    def __init__(self, app_id, playing):
        self.app_id = app_id
        self.playing = playing
        self.paused = False
        self.played = False


def _wire(monkeypatch, sessions, tmp_path):
    monkeypatch.setattr(pausing, "_PAUSE_STATE", tmp_path / "pause_state.json")
    monkeypatch.setattr(pausing, "_session_manager", lambda: object())
    monkeypatch.setattr(pausing, "_sessions", lambda mgr: sessions)
    monkeypatch.setattr(pausing, "_is_playing", lambda s: s.playing)
    monkeypatch.setattr(pausing, "_pause", lambda s: setattr(s, "paused", True))
    monkeypatch.setattr(pausing, "_play", lambda s: setattr(s, "played", True))
    monkeypatch.setattr(pausing, "_app_id", lambda s: s.app_id)


def test_pause_pauses_only_playing_sessions(monkeypatch, tmp_path):
    playing, stopped = _FakeSession("spotify", True), _FakeSession("game", False)
    _wire(monkeypatch, [playing, stopped], tmp_path)
    p = MediaPauser()
    p.pause()
    assert p.is_paused() is True
    assert playing.paused is True
    assert stopped.paused is False


def test_resume_plays_only_previously_paused(monkeypatch, tmp_path):
    playing, stopped = _FakeSession("spotify", True), _FakeSession("game", False)
    _wire(monkeypatch, [playing, stopped], tmp_path)
    p = MediaPauser()
    p.pause()
    p.resume()
    assert p.is_paused() is False
    assert playing.played is True
    assert stopped.played is False


def test_resume_skips_a_session_that_vanished(monkeypatch, tmp_path):
    a, b = _FakeSession("a", True), _FakeSession("b", True)
    _wire(monkeypatch, [a, b], tmp_path)
    p = MediaPauser()
    p.pause()
    monkeypatch.setattr(pausing, "_sessions", lambda mgr: [b])  # 'a' is gone
    p.resume()
    assert b.played is True                       # still resumes the survivor


def test_pause_writes_state_resume_clears_it(monkeypatch, tmp_path):
    state = tmp_path / "pause_state.json"
    s = _FakeSession("spotify", True)
    _wire(monkeypatch, [s], tmp_path)
    p = MediaPauser()
    p.pause()
    assert json.loads(state.read_text())["apps"] == ["spotify"]
    p.resume()
    assert not state.exists()


def test_pause_never_raises_on_backend_failure(monkeypatch, tmp_path):
    monkeypatch.setattr(pausing, "_PAUSE_STATE", tmp_path / "pause_state.json")

    def boom():
        raise RuntimeError("winrt down")

    monkeypatch.setattr(pausing, "_session_manager", boom)
    p = MediaPauser()
    p.pause()                                     # must not raise
    assert p.is_paused() is False


def test_resume_from_state_file_plays_recorded_and_deletes(monkeypatch, tmp_path):
    state = tmp_path / "pause_state.json"
    state.write_text(json.dumps({"apps": ["spotify"]}), encoding="utf-8")
    survivor = _FakeSession("spotify", False)
    monkeypatch.setattr(pausing, "_PAUSE_STATE", state)
    monkeypatch.setattr(pausing, "_session_manager", lambda: object())
    monkeypatch.setattr(pausing, "_sessions", lambda mgr: [survivor])
    monkeypatch.setattr(pausing, "_play", lambda s: setattr(s, "played", True))
    monkeypatch.setattr(pausing, "_app_id", lambda s: s.app_id)
    pausing.resume_from_state_file()
    assert survivor.played is True
    assert not state.exists()


def test_null_pauser_is_noop():
    n = NullPauser()
    assert n.is_paused() is False
    n.pause(); n.resume()                         # no error, no state
    assert n.is_paused() is False
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_pausing.py -v`
Expected: FAIL (`ModuleNotFoundError: sonara.platform.windows.pausing`).

- [ ] **Step 3: Create the module**

Create `src/sonara/platform/windows/pausing.py`:

```python
"""Windows media pause: pause OTHER apps' playing media while Sonara speaks (#92).

Uses the System Media Transport Controls (SMTC) API via winrt to pause exactly
the media sessions that are Playing, then resume exactly those. Mirrors
ducking.py's contract: all winrt imports are lazy, every public method swallows
errors and never raises, and a state file makes a mid-pause crash recoverable.

Only real media (apps that register SMTC transport controls) is affected. Audio
that is not an SMTC session (game SFX, calls, notifications) is left untouched.
"""
from __future__ import annotations

import json
import os
import threading

from sonara.paths import SONARA_DIR, ensure_sonara_dir

_PAUSE_STATE = SONARA_DIR / "pause_state.json"


# --- WinRT seams (patched wholesale in tests; never imported off Windows) ------

def _run_async(op):
    """Block on a WinRT IAsyncOperation from a sync context. Best-effort."""
    import asyncio

    async def _await():
        return await op

    loop = asyncio.new_event_loop()
    try:
        return loop.run_until_complete(_await())
    finally:
        loop.close()


def _session_manager():
    from winrt.windows.media.control import (
        GlobalSystemMediaTransportControlsSessionManager as Manager,
    )
    return _run_async(Manager.request_async())


def _sessions(mgr):
    return list(mgr.get_sessions())


def _is_playing(session) -> bool:
    from winrt.windows.media.control import (
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    )
    info = session.get_playback_info()
    return info.playback_status == Status.PLAYING


def _pause(session) -> None:
    _run_async(session.try_pause_async())


def _play(session) -> None:
    _run_async(session.try_play_async())


def _app_id(session) -> str:
    return session.source_app_user_model_id


# --- Public API ----------------------------------------------------------------

class MediaPauser:
    """Pause every OTHER app's currently-playing media session, then resume it."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._paused_ids: list[str] = []
        self._paused = False

    def is_paused(self) -> bool:
        with self._lock:
            return self._paused

    def pause(self) -> None:
        with self._lock:
            if self._paused:
                return
            try:
                mgr = _session_manager()
                ids = []
                for s in _sessions(mgr):
                    try:
                        if _is_playing(s):
                            _pause(s)
                            ids.append(_app_id(s))
                    except Exception:  # noqa: BLE001 - skip a bad session, keep the rest
                        continue
                self._paused_ids = ids
                self._paused = True
                _write_state(ids)
            except Exception:  # noqa: BLE001 - best-effort; never break speech
                pass

    def resume(self) -> None:
        with self._lock:
            try:
                wanted = set(self._paused_ids)
                if wanted:
                    try:
                        mgr = _session_manager()
                        for s in _sessions(mgr):
                            try:
                                if _app_id(s) in wanted:
                                    _play(s)
                            except Exception:  # noqa: BLE001 - one bad session must not block the rest
                                continue
                    except Exception:  # noqa: BLE001
                        pass
            finally:
                self._paused_ids = []
                self._paused = False
                _clear_state()


class NullPauser:
    """No-op pauser: non-Windows, missing winrt, or the daemon default until the
    real backend is injected. Mirrors NullDucker."""

    def is_paused(self) -> bool:
        return False

    def pause(self) -> None:
        pass

    def resume(self) -> None:
        pass


def _write_state(ids) -> None:
    try:
        ensure_sonara_dir()
        with open(_PAUSE_STATE, "w", encoding="utf-8") as f:
            json.dump({"apps": list(ids)}, f)
    except Exception:  # noqa: BLE001
        pass


def _clear_state() -> None:
    try:
        os.unlink(_PAUSE_STATE)
    except OSError:
        pass


def resume_from_state_file() -> None:
    """Daemon-startup crash sweep: if a prior daemon died mid-pause, resume any
    live SMTC session whose app id matches a recorded entry, then delete the
    file. Best-effort; never raises."""
    try:
        with open(_PAUSE_STATE, "r", encoding="utf-8") as f:
            data = json.load(f)
    except Exception:  # noqa: BLE001 - no/unreadable state -> nothing to resume
        return
    try:
        wanted = set(data.get("apps", []))
        if wanted:
            mgr = _session_manager()
            for s in _sessions(mgr):
                try:
                    if _app_id(s) in wanted:
                        _play(s)
                except Exception:  # noqa: BLE001
                    continue
    except Exception:  # noqa: BLE001
        pass
    finally:
        _clear_state()
```

- [ ] **Step 4: Add the PlatformBackend field**

In `src/sonara/platform/base.py`, directly after the `ducker` field (line 130) add:

```python
    pauser: object = None     # MediaPauser/NullPauser; duck-typed (pause/resume/is_paused)
```

- [ ] **Step 5: Inject in make_backend**

In `src/sonara/platform/windows/__init__.py`, add the import after line 8:

```python
from sonara.platform.windows.pausing import MediaPauser
```

and add to the `PlatformBackend(...)` call (after `ducker=AudioDucker(),`):

```python
        pauser=MediaPauser(),
```

- [ ] **Step 6: Run to verify pass**

Run: `python -m pytest tests/test_pausing.py -v`
Expected: PASS (8 tests).

- [ ] **Step 7: Commit**

```bash
git add src/sonara/platform/windows/pausing.py src/sonara/platform/base.py src/sonara/platform/windows/__init__.py tests/test_pausing.py
git commit -m "feat(pausing): SMTC MediaPauser backend + NullPauser + crash-safe state (#92)"
```

---

### Task 4: Daemon accepts a pauser (plumbing + test seam)

**Files:**
- Modify: `src/sonara/daemon.py` (`__init__` ~line 100-107; `main()` construction ~line 2511-2525)
- Modify: `tests/daemon_helpers.py` (add `FakePauser`; wire into `make_daemon`)
- Test: `tests/test_daemon_audio_mode.py` (new)

**Interfaces:**
- Consumes: `NullPauser` (Task 3).
- Produces: `SpeechDaemon(speaker, sessions, config, ducker=None, pauser=None)` stores `self.pauser` (defaulting to `NullPauser()`); `make_daemon(...)` injects a `FakePauser` reachable as `daemon.pauser`.
- `FakePauser`: `is_paused()`, `pause()`, `resume()`, `.pause_calls`, `.resume_calls`.

- [ ] **Step 1: Write the failing test**

Create `tests/test_daemon_audio_mode.py`:

```python
from sonara.daemon import SpeechDaemon
from sonara.sessions import SessionManager
from sonara.config import DEFAULTS
from tests.daemon_helpers import make_daemon, FakePauser


def test_daemon_defaults_to_null_pauser():
    from sonara.platform.windows.pausing import NullPauser
    cfg = {k: v for k, v in DEFAULTS.items()}
    d = SpeechDaemon(object(), SessionManager(), cfg)   # no pauser passed
    assert isinstance(d.pauser, NullPauser)


def test_make_daemon_injects_fake_pauser():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert isinstance(daemon.pauser, FakePauser)
    assert daemon.pauser.pause_calls == 0
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_daemon_audio_mode.py -v`
Expected: FAIL (`ImportError: FakePauser`; `AttributeError: pauser`).

- [ ] **Step 3: Add FakePauser to daemon_helpers**

In `tests/daemon_helpers.py`, after the `FakeDucker` class add:

```python
class FakePauser:
    def __init__(self):
        self.pause_calls = 0
        self.resume_calls = 0
        self._paused = False

    def is_paused(self): return self._paused

    def pause(self):
        self.pause_calls += 1; self._paused = True

    def resume(self):
        self.resume_calls += 1; self._paused = False
```

In `make_daemon`, replace the construction lines:

```python
    ducker = FakeDucker()
    daemon = SpeechDaemon(speaker, sessions, config, ducker=ducker)
```

with:

```python
    ducker = FakeDucker()
    pauser = FakePauser()
    daemon = SpeechDaemon(speaker, sessions, config, ducker=ducker, pauser=pauser)
```

- [ ] **Step 4: Add the pauser param to the daemon**

In `src/sonara/daemon.py`, change the `__init__` signature (line 100) and the ducker default block (lines 104-107) to:

```python
    def __init__(self, speaker, sessions, config, ducker=None, pauser=None) -> None:
        self.speaker = speaker
        self.sessions = sessions
        self.config = config
        if ducker is None:
            from sonara.platform.windows.ducking import NullDucker
            ducker = NullDucker()
        self.ducker = ducker
        if pauser is None:
            from sonara.platform.windows.pausing import NullPauser
            pauser = NullPauser()
        self.pauser = pauser
```

- [ ] **Step 5: Wire the real backend + startup resume in main()**

In `src/sonara/daemon.py`, after the ducking startup sweep (lines 2511-2512) add:

```python
    from sonara.platform.windows.pausing import resume_from_state_file as _resume_paused
    _resume_paused()   # resume anything a crashed prior daemon left paused
```

Change the daemon construction (line 2525) to:

```python
    daemon = SpeechDaemon(speaker, sessions, cfg,
                          ducker=_backend.ducker, pauser=_backend.pauser)
```

- [ ] **Step 6: Run to verify pass**

Run: `python -m pytest tests/test_daemon_audio_mode.py -v`
Expected: PASS (2 tests).

- [ ] **Step 7: Commit**

```bash
git add src/sonara/daemon.py tests/daemon_helpers.py tests/test_daemon_audio_mode.py
git commit -m "feat(daemon): accept a pauser backend, inject real MediaPauser + startup resume (#92)"
```

---

### Task 5: Mode-aware engage / restore routing

**Files:**
- Modify: `src/sonara/daemon.py` (`_audio_control_on` ~line 1959-1960; `_maybe_duck`/`_maybe_restore` ~line 2090-2096; `_speak_loop_once` on_play var ~line 2164 and restore call sites ~lines 1040, 2104, 2146-2147)
- Test: `tests/test_daemon_audio_mode.py`

**Interfaces:**
- Consumes: `self.ducker`, `self.pauser`, `self._duck_exclude_pids()`, `self._duck_level()`.
- Produces: `_audio_mode() -> str`, `_audio_duck_on() -> bool`, `_audio_pause_on() -> bool`, `_maybe_engage_audio() -> None`, `_maybe_restore_audio() -> None`.

- [ ] **Step 1: Write the failing tests**

Add to `tests/test_daemon_audio_mode.py`:

```python
from sonara.queue import SpeechItem


def _seed_item(daemon, text="Hello there.", session="fg"):
    ch = daemon.router.channel(session)
    ch.append(SpeechItem(id=1, session=session, kind="prose", text=text,
                         is_decision=False))
    ch.turn_done = True


def test_off_mode_engages_neither_backend():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "off"
    _seed_item(daemon)
    daemon._speak_loop_once()
    assert daemon.ducker.duck_calls == []
    assert daemon.pauser.pause_calls == 0


def test_duck_mode_ducks_at_playback():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "duck"
    _seed_item(daemon)
    daemon._speak_loop_once()
    assert daemon.ducker.duck_calls
    assert daemon.pauser.pause_calls == 0


def test_pause_mode_pauses_at_playback():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "pause"
    _seed_item(daemon)
    daemon._speak_loop_once()
    assert daemon.pauser.pause_calls == 1
    assert daemon.ducker.duck_calls == []


def test_pause_mode_resumes_at_global_idle():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "pause"
    _seed_item(daemon)
    daemon._speak_loop_once()                     # speaks -> paused
    assert daemon.pauser.is_paused() is True
    daemon._speak_loop_once()                     # nothing left -> idle restore
    assert daemon.pauser.resume_calls == 1
    assert daemon.pauser.is_paused() is False


def test_session_change_announcement_engages_neither():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    config["audio_mode"] = "pause"
    daemon.router._last_active = "a"
    _seed_item(daemon, text="The digest body.", session="b")
    daemon.router._replay_authorized.add("b")
    daemon._speak_loop_once()                     # the announcement
    assert daemon.pauser.pause_calls == 0
    assert daemon.ducker.duck_calls == []
    daemon._speak_loop_once()                     # the content
    assert daemon.pauser.pause_calls == 1
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_daemon_audio_mode.py -v`
Expected: FAIL (`_maybe_engage_audio` not wired; pause never called).

- [ ] **Step 3: Replace the mode helper**

In `src/sonara/daemon.py`, replace `_audio_control_on` (lines 1959-1960):

```python
    def _audio_control_on(self) -> bool:
        return bool(self.config.get("audio_control"))
```

with:

```python
    def _audio_mode(self) -> str:
        mode = self.config.get("audio_mode", "off")
        return mode if mode in ("off", "duck", "pause") else "off"

    def _audio_duck_on(self) -> bool:
        return self._audio_mode() == "duck"

    def _audio_pause_on(self) -> bool:
        return self._audio_mode() == "pause"
```

- [ ] **Step 4: Replace engage/restore**

In `src/sonara/daemon.py`, replace `_maybe_duck` and `_maybe_restore` (lines 2090-2096):

```python
    def _maybe_duck(self) -> None:
        if self._audio_control_on() and not self.ducker.is_ducked():
            self.ducker.duck(self._duck_exclude_pids(), self._duck_level())

    def _maybe_restore(self) -> None:
        if self.ducker.is_ducked():
            self.ducker.restore()
```

with:

```python
    def _maybe_engage_audio(self) -> None:
        mode = self._audio_mode()
        if mode == "duck":
            if not self.ducker.is_ducked():
                self.ducker.duck(self._duck_exclude_pids(), self._duck_level())
        elif mode == "pause":
            if not self.pauser.is_paused():
                self.pauser.pause()

    def _maybe_restore_audio(self) -> None:
        # Disengage BOTH backends defensively: a mid-speech mode switch can leave
        # the other backend engaged, and idle must never leave media ducked OR paused.
        if self.ducker.is_ducked():
            self.ducker.restore()
        if self.pauser.is_paused():
            self.pauser.resume()
```

- [ ] **Step 5: Update the on_play var and restore call sites**

In `_speak_loop_once`, change the on_play line (line 2164) from:

```python
        on_play = None if item.kind == "session_change" else self._maybe_duck
```

to:

```python
        on_play = None if item.kind == "session_change" else self._maybe_engage_audio
```

Replace the three `self._maybe_restore()` calls with `self._maybe_restore_audio()`:
- `stop()` (line 1040)
- the paused branch (line 2104)
- the idle branch (line 2147)

Verify none remain:

Run: `grep -n "_maybe_restore()\|_maybe_duck\b\|_audio_control_on" src/sonara/daemon.py`
Expected: only the `SET_DUCK_LEVEL` handler's `_audio_control_on()` remains (fixed in Task 6); no `_maybe_duck`/`_maybe_restore()` left.

- [ ] **Step 6: Run to verify pass**

Run: `python -m pytest tests/test_daemon_audio_mode.py tests/test_daemon_duck_timing.py tests/test_daemon_duck_announcement.py -v`
Expected: PASS (duck-timing and announcement tests still green: duck mode preserves old behavior).

- [ ] **Step 7: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_audio_mode.py
git commit -m "feat(daemon): route audio engage/restore by audio_mode (off/duck/pause) (#92)"
```

---

### Task 6: Mode handlers + duck-level guard

**Files:**
- Modify: `src/sonara/daemon.py` (`SET_AUDIO_CONTROL` handler ~line 947-959; `SET_DUCK_LEVEL` handler ~line 961-975; add `SET_AUDIO_MODE` handler + `_apply_audio_mode`)
- Test: `tests/test_daemon_audio_mode.py`

**Interfaces:**
- Consumes: `_maybe_restore_audio`, `_audio_duck_on`, `MsgType.SET_AUDIO_MODE`.
- Produces: `_apply_audio_mode(mode: str) -> None`; `SET_AUDIO_MODE` and `SET_AUDIO_CONTROL` both route through it.

- [ ] **Step 1: Write the failing tests**

Add to `tests/test_daemon_audio_mode.py`:

```python
from sonara.protocol import MsgType, PROTOCOL_VERSION


def _msg(daemon, **kw):
    kw.setdefault("v", PROTOCOL_VERSION)
    return daemon.handle_message(kw)


def test_set_audio_mode_persists_and_cues():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _msg(daemon, type=MsgType.SET_AUDIO_MODE, mode="pause")
    assert config["audio_mode"] == "pause"
    assert any("Media pause." in t for t in speaker.spoken)


def test_set_audio_mode_disengages_previous_backend():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "duck"
    _seed_item(daemon)
    daemon._speak_loop_once()                     # ducked now
    assert daemon.ducker.is_ducked() is True
    _msg(daemon, type=MsgType.SET_AUDIO_MODE, mode="pause")
    assert daemon.ducker.is_ducked() is False     # old backend released on switch


def test_set_audio_mode_ignores_unknown_value():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "duck"
    _msg(daemon, type=MsgType.SET_AUDIO_MODE, mode="bogus")
    assert config["audio_mode"] == "duck"         # unchanged


def test_audio_control_shim_maps_to_mode():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _msg(daemon, type=MsgType.SET_AUDIO_CONTROL, enabled=True)
    assert config["audio_mode"] == "duck"
    _msg(daemon, type=MsgType.SET_AUDIO_CONTROL, enabled=False)
    assert config["audio_mode"] == "off"


def test_duck_level_reapplies_only_in_duck_mode():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "pause"
    daemon.pauser.pause()                          # pretend paused
    _msg(daemon, type=MsgType.SET_DUCK_LEVEL, level=50)
    assert daemon.ducker.duck_calls == []         # not in duck mode -> no re-duck
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_daemon_audio_mode.py -k "audio_mode or shim or duck_level" -v`
Expected: FAIL (`SET_AUDIO_MODE` unhandled; shim still sets `audio_control`).

- [ ] **Step 3: Add `_apply_audio_mode` and rewrite handlers**

In `src/sonara/daemon.py`, replace the `SET_AUDIO_CONTROL` and `SET_DUCK_LEVEL` handlers (lines 947-975) with:

```python
        if t == MsgType.SET_AUDIO_MODE:
            mode = msg.get("mode")
            if mode not in ("off", "duck", "pause"):
                return None
            self._apply_audio_mode(mode)
            return None

        if t == MsgType.SET_AUDIO_CONTROL:
            # Pre-#92 compat shim: enabled -> duck, disabled -> off.
            if "enabled" not in msg:
                return None
            self._apply_audio_mode("duck" if bool(msg.get("enabled")) else "off")
            return None

        if t == MsgType.SET_DUCK_LEVEL:
            try:
                level = max(0, min(100, int(msg.get("level"))))
            except (TypeError, ValueError):
                return None
            self.config["duck_level"] = level
            save_config(self.config)
            if self._audio_duck_on() and self.ducker.is_ducked():  # re-apply at the new level
                self.ducker.restore()
                self.ducker.duck(self._duck_exclude_pids(), level)
            target = self.router.active or self.sessions.foreground()
            self._speak_cue(target, "Duck level {0} percent.".format(level),
                            exempt_mute=True, pause_exempt=True)
            self._wake.set()
            return None
```

Add the shared setter as a method near `_maybe_restore_audio` (in the methods block, e.g. after `_maybe_restore_audio`):

```python
    def _apply_audio_mode(self, mode: str) -> None:
        """Persist the audio behavior mode, disengage whatever backend was
        engaged (so a switch never leaves other apps ducked or paused), and
        speak the mode cue. Shared by SET_AUDIO_MODE and the SET_AUDIO_CONTROL
        compat shim."""
        if mode not in ("off", "duck", "pause"):
            return
        self.config["audio_mode"] = mode
        save_config(self.config)
        self._maybe_restore_audio()
        target = self.router.active or self.sessions.foreground()
        cue = {"off": "Audio off.", "duck": "Audio ducking.",
               "pause": "Media pause."}[mode]
        self._speak_cue(target, cue, exempt_mute=True, pause_exempt=True)
        self._wake.set()
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_daemon_audio_mode.py -v`
Expected: PASS (all tests in the file).

- [ ] **Step 5: Run the ducking regression set**

Run: `python -m pytest tests/test_daemon_ducking.py -v`
Expected: PASS except the known baseline `test_config_defaults_have_audio_control_off_and_duck_level_20` (unrelated `duck_level==20` vs 30). If a test in that file asserts old `audio_control`-driven behavior that this task changed, update it to drive `audio_mode` and note it in the commit.

- [ ] **Step 6: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_audio_mode.py
git commit -m "feat(daemon): SET_AUDIO_MODE handler + audio_control shim + duck-level guard (#92)"
```

---

### Task 7: CLI audio-mode subcommand

**Files:**
- Modify: `src/sonara/cli.py` (`_cmd_audio_control` ~line 100-103; subparsers ~line 224-230)
- Test: `tests/test_cli_ducking.py`

**Interfaces:**
- Produces: `sonara audio-mode {off,duck,pause}` sends `{"type":"set_audio_mode","mode":...}`.

- [ ] **Step 1: Write the failing test**

Add to `tests/test_cli_ducking.py`:

```python
def test_audio_mode_command_sends_set_audio_mode(monkeypatch):
    import sonara.cli as cli
    sent = {}
    monkeypatch.setattr(cli, "_send", lambda m: sent.update(m))
    cli.main(["audio-mode", "pause"])
    assert sent["type"] == "set_audio_mode"
    assert sent["mode"] == "pause"
```

(If `test_cli_ducking.py` invokes the CLI differently, mirror its existing invocation style; the assertion on the sent message is the point.)

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_cli_ducking.py -k audio_mode -v`
Expected: FAIL (`invalid choice: 'audio-mode'`).

- [ ] **Step 3: Add the command handler**

In `src/sonara/cli.py`, after `_cmd_duck_level` (line 109) add:

```python
def _cmd_audio_mode(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_MODE, "mode": args.mode})
    print("Audio mode set to {0}.".format(args.mode))
    return 0
```

- [ ] **Step 4: Register the subparser**

In `src/sonara/cli.py`, after the `duck-level` subparser block (line 230) add:

```python
    am = sub.add_parser("audio-mode", help="off | duck | pause (pause media while speaking)")
    am.add_argument("mode", choices=["off", "duck", "pause"])
    am.set_defaults(func=_cmd_audio_mode)
```

- [ ] **Step 5: Run to verify pass**

Run: `python -m pytest tests/test_cli_ducking.py -k audio_mode -v`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/sonara/cli.py tests/test_cli_ducking.py
git commit -m "feat(cli): audio-mode subcommand (#92)"
```

---

### Task 8: Settings page + webui plumbing

**Files:**
- Modify: `src/sonara/webui.py` (`_PAGE_KEYS` ~line 20-26; `_MSG_KEYS` ~line 28-35)
- Modify: `src/sonara/settings.html` (audio pref block ~line 205; `render` gate ~line 472-476; click handler ~line 620-622)
- Test: `tests/test_webui.py`

**Interfaces:**
- Consumes: `MsgType.SET_AUDIO_MODE`, `audio_mode` config key.
- Produces: page writes to `audio_mode` dispatch a `set_audio_mode` message; the Audio section shows an Off/Duck/Pause segmented control gating the duck-level row.

- [ ] **Step 1: Write the failing test**

Add to `tests/test_webui.py` (mirror the existing key-write test style in that file):

```python
def test_audio_mode_write_dispatches_set_audio_mode():
    from sonara.webui import _MSG_KEYS, _PAGE_KEYS
    assert "audio_mode" in _PAGE_KEYS
    msg = _MSG_KEYS["audio_mode"]("pause")
    assert msg == {"type": "set_audio_mode", "mode": "pause"}
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_webui.py -k audio_mode -v`
Expected: FAIL (`KeyError: 'audio_mode'`).

- [ ] **Step 3: Add the webui plumbing**

In `src/sonara/webui.py`, add `"audio_mode"` to the `_PAGE_KEYS` tuple (alongside `"audio_control", "duck_level"`). In `_MSG_KEYS`, add:

```python
    "audio_mode":    lambda v: {"type": "set_audio_mode", "mode": str(v)},
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_webui.py -k audio_mode -v`
Expected: PASS.

- [ ] **Step 5: Replace the settings markup**

In `src/sonara/settings.html`, replace the Audio-duck pref block (the `<div class="pref">...#duck-switch...</div>` at line 205) with:

```html
          <div class="pref"><div class="pref-copy"><strong>Audio</strong><div class="hint">What happens to other apps while Sonara speaks.</div></div><div class="control"><div class="segments" id="audio-seg" role="tablist"><button data-mode="off">Off</button><button data-mode="duck">Duck</button><button data-mode="pause">Pause</button></div><div class="hint" id="audio-mode-hint">Other apps play at full volume.</div></div></div>
```

- [ ] **Step 6: Replace the render logic**

In `src/sonara/settings.html`, in `render(s)`, replace the audio-duck lines (the `gateRow("duck-row", ...)`, `setSwitch("duck-switch", ...)`, and the `duck` `setVal`/`textContent` block, lines ~472-476) with:

```javascript
  const AUDIO_HINTS = {off: "Other apps play at full volume.",
                       duck: "Lower other apps' volume while speaking.",
                       pause: "Pause music and video while speaking."};
  const audioMode = s.config.audio_mode || "off";
  document.querySelectorAll("#audio-seg button").forEach(b =>
    b.classList.toggle("active", b.dataset.mode === audioMode));
  document.getElementById("audio-mode-hint").textContent = AUDIO_HINTS[audioMode];
  gateRow("duck-row", audioMode !== "duck", ["duck"]);
  setVal("duck", s.config.duck_level);
  document.getElementById("duck-out").textContent = s.config.duck_level + "%";
```

- [ ] **Step 7: Replace the click handler**

In `src/sonara/settings.html`, replace the `#duck-switch` click handler (lines 620-622):

```javascript
document.getElementById("duck-switch").addEventListener("click", function () {
  set("audio_control", this.getAttribute("aria-checked") !== "true");
});
```

with:

```javascript
document.querySelectorAll("#audio-seg button").forEach(b =>
  b.addEventListener("click", () => set("audio_mode", b.dataset.mode)));
```

- [ ] **Step 8: Manual smoke check**

Run: `python -m pytest tests/test_webui.py -v`
Expected: PASS. Then deploy locally (see below) and open the settings page: the Audio section shows Off / Duck / Pause; selecting Duck reveals the Duck level slider, Off and Pause gray it out; each selection persists across a page refresh (tab persistence already handled).

- [ ] **Step 9: Commit**

```bash
git add src/sonara/webui.py src/sonara/settings.html tests/test_webui.py
git commit -m "feat(settings): Off/Duck/Pause segmented audio control + gating (#92)"
```

---

## Final verification

- [ ] Run the full suite: `python -m pytest -q`
  Expected: all pass except the documented pre-existing environment baseline failures (test_bin_sonara.py WinError 193 x3, test_daemon_ducking `duck_level==20`, test_paths x2, test_transport, test_win_tts x2 + 1 error). No new failures.
- [ ] Deploy to the live daemon and verify Pause live:
  `./bin/sonara shutdown` -> `robocopy` mirror `src/sonara` into `$HOME/.sonara/app/sonara` (via PowerShell with native paths) -> `./bin/sonara start`. Set mode to Pause, play music, trigger speech: music pauses when the voice starts and resumes when Sonara goes idle; the session-change announcement does not pause early.

---

## Self-Review

**Spec coverage:**
- Config `audio_mode` + migration -> Task 1. Protocol `SET_AUDIO_MODE` -> Task 2. `MediaPauser`/`NullPauser`/`resume_from_state_file` + crash state -> Task 3. Injection (`base.py`, `__init__.py`, daemon `__init__`, `main()` + startup resume) -> Tasks 3-4. Helpers + engage/restore + timing + #90 announcement carve-out -> Task 5. `SET_AUDIO_MODE` handler + `SET_AUDIO_CONTROL` shim + duck-level guard -> Task 6. CLI -> Task 7. Settings UI + webui + gating -> Task 8. All spec sections covered.
- Non-goals (no allow/deny list, no duck fallback in pause, no cross-fade, no non-Windows backend, no retroactive mid-utterance engage) are respected: no task implements them.

**Placeholder scan:** No TBD/TODO. Every code step shows complete code. The one judgment area (test_daemon_ducking rows that assert legacy `audio_control` behavior) is handled explicitly in Task 6 Step 5 with instructions, not left vague.

**Type consistency:** `_audio_mode`/`_audio_duck_on`/`_audio_pause_on`, `_maybe_engage_audio`/`_maybe_restore_audio`, `_apply_audio_mode(mode)` names are used identically across Tasks 5-8. `MediaPauser.pause()/resume()/is_paused()` and the `_session_manager`/`_sessions`/`_is_playing`/`_pause`/`_play`/`_app_id`/`_PAUSE_STATE` seams match between Task 3's module and its tests. `FakePauser` (`pause_calls`/`resume_calls`) is consistent between Task 4's definition and its uses in Tasks 5-6. webui `audio_mode` message shape matches the protocol type and handler.
