# Audio Ducking ("Audio Control") Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a user-toggleable feature that lowers all other apps' audio while Sonara's TTS speaks and restores it when Sonara goes quiet.

**Architecture:** A Windows-only `AudioDucker` (pycaw per-session volume) is driven by the speak loop's active→idle transition so the duck holds across every sentence and queued session and lifts only at global idle. It is crash-safe via a persisted recovery file restored at daemon startup, exposed behind a test seam, and controlled by two persisted config toggles (`audio_control`, `duck_level`) with CLI + slash commands. Default off.

**Tech Stack:** Python 3.12+, `pycaw` (per-app volume via Core Audio, pure-Python on `comtypes`), existing Sonara daemon/Speaker/config/protocol/CLI.

## Global Constraints

- Windows-only; all `pycaw`/`comtypes` imports are lazy (inside functions) so modules import on any host and in tests.
- Ducking is **best-effort**: every `AudioDucker` method swallows all exceptions and never raises — it must never break or delay speech.
- Default state: `audio_control` = `false` (opt-in); `duck_level` = `20` (target % volume for other apps while ducked), clamped 0-100.
- Never duck Sonara's own audio: exclude the daemon PID and its live earcon-helper PIDs.
- Hold the duck across all sentences AND all queued sessions; restore only at global idle (`router.next_item()` returns `None`), on `daemon.stop()`, on toggle-off, and via the startup crash sweep.
- Follow existing patterns: config `DEFAULTS`, `MsgType` constants, `SET_*` handlers (validate/clamp → `self.config[...] = ...` → `save_config(self.config)` → cue), CLI `_cmd_*` + subparser, `commands/*.md` front-matter.
- Test command: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest`

---

### Task 1: `AudioDucker` module (pycaw, with crash-recovery file)

**Files:**
- Create: `src/sonara/platform/windows/ducking.py`
- Test: `tests/test_ducking.py`

**Interfaces:**
- Consumes: `sonara.paths.SONARA_DIR`, `sonara.paths.ensure_sonara_dir`.
- Produces:
  - `class AudioDucker` with `duck(self, exclude_pids: set[int], level: int) -> None`, `restore(self) -> None`, `is_ducked(self) -> bool`.
  - `class NullDucker` with the same three methods (all no-ops; `is_ducked` returns `False`).
  - `restore_from_state_file() -> None` — startup crash sweep.
  - `_all_sessions()` — the pycaw seam tests monkeypatch.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_ducking.py
import json
import sonara.platform.windows.ducking as ducking
from sonara.platform.windows.ducking import AudioDucker, NullDucker


class _FakeVol:
    def __init__(self, v): self.v = v
    def GetMasterVolume(self): return self.v
    def SetMasterVolume(self, v, ctx): self.v = v


class _FakeProc:
    def __init__(self, name): self._n = name
    def name(self): return self._n


class _FakeSession:
    def __init__(self, pid, vol, name="app.exe"):
        self.ProcessId = pid
        self.SimpleAudioVolume = _FakeVol(vol)
        self.Process = _FakeProc(name)


def _sessions(monkeypatch, sessions):
    monkeypatch.setattr(ducking, "_all_sessions", lambda: sessions)


def test_duck_lowers_non_excluded_sessions_to_level(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    s1, s2 = _FakeSession(100, 0.8), _FakeSession(200, 0.6)
    _sessions(monkeypatch, [s1, s2])
    d = AudioDucker()
    d.duck(exclude_pids=set(), level=20)
    assert d.is_ducked() is True
    assert s1.SimpleAudioVolume.v == 0.2     # 20% of full
    assert s2.SimpleAudioVolume.v == 0.2


def test_duck_skips_excluded_pids(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    own, other = _FakeSession(999, 0.9), _FakeSession(100, 0.8)
    _sessions(monkeypatch, [own, other])
    d = AudioDucker()
    d.duck(exclude_pids={999}, level=20)
    assert own.SimpleAudioVolume.v == 0.9    # excluded -> untouched
    assert other.SimpleAudioVolume.v == 0.2


def test_restore_puts_original_volumes_back(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    s = _FakeSession(100, 0.7)
    _sessions(monkeypatch, [s])
    d = AudioDucker()
    d.duck(exclude_pids=set(), level=10)
    assert s.SimpleAudioVolume.v == 0.1
    d.restore()
    assert s.SimpleAudioVolume.v == 0.7
    assert d.is_ducked() is False


def test_duck_is_idempotent(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    s = _FakeSession(100, 0.8)
    _sessions(monkeypatch, [s])
    d = AudioDucker()
    d.duck(set(), 20)
    s.SimpleAudioVolume.v = 0.5               # someone else changed it
    d.duck(set(), 20)                         # second duck must be a no-op
    assert s.SimpleAudioVolume.v == 0.5


def test_duck_writes_state_file_restore_clears_it(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    _sessions(monkeypatch, [_FakeSession(100, 0.8, "vlc.exe")])
    d = AudioDucker()
    d.duck(set(), 20)
    rec = json.loads(state.read_text(encoding="utf-8"))
    assert rec["sessions"][0]["pid"] == 100 and rec["sessions"][0]["original"] == 0.8
    d.restore()
    assert not state.exists()


def test_restore_from_state_file_restores_matching_live_sessions(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    state.write_text(json.dumps({"sessions": [{"pid": 100, "name": "vlc.exe", "original": 0.9}]}),
                     encoding="utf-8")
    live = _FakeSession(100, 0.2, "vlc.exe")   # currently ducked
    _sessions(monkeypatch, [live])
    ducking.restore_from_state_file()
    assert live.SimpleAudioVolume.v == 0.9
    assert not state.exists()


def test_duck_never_raises_on_pycaw_failure(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    def boom(): raise RuntimeError("no COM")
    monkeypatch.setattr(ducking, "_all_sessions", boom)
    d = AudioDucker()
    d.duck(set(), 20)                          # must swallow
    assert d.is_ducked() is False
    d.restore()                                # must swallow


def test_null_ducker_is_noop():
    n = NullDucker()
    assert n.is_ducked() is False
    n.duck({1, 2}, 20)
    n.restore()
    assert n.is_ducked() is False
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_ducking.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'sonara.platform.windows.ducking'`

- [ ] **Step 3: Implement the module**

```python
# src/sonara/platform/windows/ducking.py
"""Windows audio ducking: lower OTHER apps' volume while Sonara speaks.

Per-app volume via pycaw (Core Audio session API). All pycaw/comtypes imports are
lazy so this module imports anywhere (tests, non-Windows). Best-effort: every
public method swallows pycaw/COM errors and never raises, so a failure to duck can
never break or delay speech.
"""
from __future__ import annotations

import json
import os

from sonara.paths import SONARA_DIR, ensure_sonara_dir

_DUCK_STATE = SONARA_DIR / "duck_state.json"


def _all_sessions():
    """All active audio sessions via pycaw. Lazy import; the test seam patches
    this. Raises if pycaw/COM is unavailable — callers swallow it."""
    from pycaw.pycaw import AudioUtilities
    return AudioUtilities.GetAllSessions()


def _session_name(session) -> str:
    try:
        return session.Process.name() if session.Process else ""
    except Exception:  # noqa: BLE001
        return ""


class AudioDucker:
    """Lower every other app's audio session to a target level, then restore."""

    def __init__(self) -> None:
        self._saved = []          # list[(session, original_scalar)]
        self._ducked = False

    def is_ducked(self) -> bool:
        return self._ducked

    def duck(self, exclude_pids, level: int) -> None:
        if self._ducked:
            return
        try:
            target = max(0, min(100, int(level))) / 100.0
            saved, record = [], []
            for s in _all_sessions():
                vol = s.SimpleAudioVolume
                if vol is None or s.ProcessId in exclude_pids:
                    continue
                original = vol.GetMasterVolume()
                vol.SetMasterVolume(target, None)
                saved.append((s, original))
                record.append({"pid": s.ProcessId, "name": _session_name(s),
                               "original": original})
            self._saved = saved
            self._ducked = True
            _write_state(record)
        except Exception:  # noqa: BLE001 - best-effort; never break speech
            pass

    def restore(self) -> None:
        try:
            for s, original in self._saved:
                try:
                    s.SimpleAudioVolume.SetMasterVolume(original, None)
                except Exception:  # noqa: BLE001 - one bad session must not block the rest
                    pass
        finally:
            self._saved = []
            self._ducked = False
            _clear_state()


class NullDucker:
    """No-op ducker: used on non-Windows, when pycaw is missing, or as the daemon
    default until the real backend ducker is injected."""

    def is_ducked(self) -> bool:
        return False

    def duck(self, exclude_pids, level: int) -> None:
        pass

    def restore(self) -> None:
        pass


def _write_state(record) -> None:
    try:
        ensure_sonara_dir()
        with open(_DUCK_STATE, "w", encoding="utf-8") as f:
            json.dump({"sessions": record}, f)
    except Exception:  # noqa: BLE001
        pass


def _clear_state() -> None:
    try:
        os.unlink(_DUCK_STATE)
    except OSError:
        pass


def restore_from_state_file() -> None:
    """Daemon-startup crash sweep: if a prior daemon died mid-duck, restore any
    live session whose pid or process name matches a recorded entry, then delete
    the file. Best-effort; never raises."""
    try:
        with open(_DUCK_STATE, "r", encoding="utf-8") as f:
            data = json.load(f)
    except (FileNotFoundError, ValueError, OSError):
        return
    try:
        by_pid, by_name = {}, {}
        for e in data.get("sessions", []):
            if "pid" in e:
                by_pid[e["pid"]] = e["original"]
            if e.get("name"):
                by_name[e["name"]] = e["original"]
        for s in _all_sessions():
            try:
                original = by_pid.get(s.ProcessId)
                if original is None:
                    original = by_name.get(_session_name(s))
                if original is not None:
                    s.SimpleAudioVolume.SetMasterVolume(original, None)
            except Exception:  # noqa: BLE001
                pass
    except Exception:  # noqa: BLE001
        pass
    finally:
        _clear_state()
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_ducking.py -v`
Expected: PASS (8 tests)

- [ ] **Step 5: Commit**

```bash
git add src/sonara/platform/windows/ducking.py tests/test_ducking.py
git commit -m "feat(audio): AudioDucker + NullDucker + crash-recovery sweep"
```

---

### Task 2: Wire the ducker into the backend, daemon, and startup (no speak-loop behavior yet)

**Files:**
- Modify: `src/sonara/platform/base.py` (the `PlatformBackend` dataclass)
- Modify: `src/sonara/platform/windows/__init__.py` (`make_backend`)
- Modify: `src/sonara/daemon.py` (`SpeechDaemon.__init__`, `main()`)
- Modify: `tests/daemon_helpers.py` (inject a `FakeDucker`)
- Test: `tests/test_daemon_ducking.py`

**Interfaces:**
- Consumes: `AudioDucker`, `NullDucker`, `restore_from_state_file` from Task 1.
- Produces:
  - `PlatformBackend.ducker` field (an `AudioDucker`/`NullDucker`).
  - `SpeechDaemon.__init__(self, speaker, sessions, config, ducker=None)` — stores `self.ducker` (defaults to `NullDucker()` when `ducker is None`).
  - `tests/daemon_helpers.py` `FakeDucker` recording `.duck_calls` / `.restore_calls`, attached as `daemon.ducker` and returned/accessible.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_daemon_ducking.py
from tests.daemon_helpers import make_daemon
from sonara.platform.windows.ducking import NullDucker


def test_daemon_defaults_to_null_ducker_when_none_passed():
    from sonara.daemon import SpeechDaemon
    from tests.daemon_helpers import FakeSpeaker
    from sonara.sessions import SessionManager
    from sonara.config import DEFAULTS
    d = SpeechDaemon(FakeSpeaker(), SessionManager(), dict(DEFAULTS))
    assert isinstance(d.ducker, NullDucker)


def test_make_daemon_injects_a_fake_ducker():
    daemon, *_ = make_daemon(foreground="fg")
    assert hasattr(daemon.ducker, "duck_calls")
    assert daemon.ducker.duck_calls == [] and daemon.ducker.restore_calls == 0
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_daemon_ducking.py -v`
Expected: FAIL — `AttributeError: 'SpeechDaemon' object has no attribute 'ducker'`

- [ ] **Step 3: Implement the wiring**

In `src/sonara/platform/base.py`, add the field to the dataclass:

```python
@dataclass
class PlatformBackend:
    tts: TtsBackend
    earcon: EarconBackend
    hotkey: HotkeyBackend
    supervisor: SupervisorBackend
    ducker: object = None     # AudioDucker/NullDucker; duck-typed (duck/restore/is_ducked)
```

In `src/sonara/platform/windows/__init__.py`, import and pass the real ducker:

```python
from sonara.platform.windows.ducking import AudioDucker
# ...
def make_backend() -> PlatformBackend:
    return PlatformBackend(
        tts=WinTtsBackend(),
        earcon=WinEarconBackend(),
        hotkey=WinHotkeyBackend(),
        supervisor=WinSupervisorBackend(),
        ducker=AudioDucker(),
    )
```

In `src/sonara/daemon.py`, change the constructor signature (currently `def __init__(self, speaker, sessions, config) -> None:` at line ~48) and store the ducker. Add the import near the other lazy imports at top of `__init__` is fine, but use a module-level import:

```python
def __init__(self, speaker, sessions, config, ducker=None) -> None:
    self.speaker = speaker
    self.sessions = sessions
    self.config = config
    if ducker is None:
        from sonara.platform.windows.ducking import NullDucker
        ducker = NullDucker()
    self.ducker = ducker
    # ... rest unchanged ...
```

In `src/sonara/daemon.py` `main()`, run the crash sweep at startup and pass the backend ducker (modify the existing block that builds `speaker`/`daemon`):

```python
    from sonara.platform.windows.ducking import restore_from_state_file
    restore_from_state_file()   # un-duck anything a crashed prior daemon left down
    # ... existing speaker construction ...
    daemon = SpeechDaemon(speaker, sessions, cfg, ducker=_backend.ducker)
    daemon.run()
```

In `tests/daemon_helpers.py`, add the `FakeDucker` and inject it in `make_daemon`:

```python
class FakeDucker:
    def __init__(self):
        self.duck_calls = []     # list of (exclude_pids, level)
        self.restore_calls = 0
        self._ducked = False
    def is_ducked(self): return self._ducked
    def duck(self, exclude_pids, level):
        self.duck_calls.append((set(exclude_pids), level)); self._ducked = True
    def restore(self):
        self.restore_calls += 1; self._ducked = False
```

In `make_daemon`, build it and pass it:

```python
    ducker = FakeDucker()
    daemon = SpeechDaemon(speaker, sessions, config, ducker=ducker)
```

(`daemon.ducker` is now the `FakeDucker`; tests reach it via `daemon.ducker`.)

- [ ] **Step 4: Run tests**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_daemon_ducking.py tests/test_win_backend.py -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonara/platform/base.py src/sonara/platform/windows/__init__.py src/sonara/daemon.py tests/daemon_helpers.py tests/test_daemon_ducking.py
git commit -m "feat(audio): wire ducker into backend + daemon + startup sweep"
```

---

### Task 3: Config keys, protocol message types, and command handlers

**Files:**
- Modify: `src/sonara/config.py` (`DEFAULTS`)
- Modify: `src/sonara/protocol.py` (`MsgType`)
- Modify: `src/sonara/daemon.py` (`handle_message` — two new branches; `_speak_cue` helper already exists)
- Test: `tests/test_daemon_ducking.py` (extend)

**Interfaces:**
- Consumes: `self.ducker`, `self._speak_cue`, `save_config`, `self.config` from earlier tasks.
- Produces: config keys `audio_control` (bool) and `duck_level` (int); `MsgType.SET_AUDIO_CONTROL`, `MsgType.SET_DUCK_LEVEL`; handlers that persist + cue + clamp + restore-on-off + re-apply-on-level-change.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_daemon_ducking.py  (append)
from sonara.protocol import MsgType, PROTOCOL_VERSION
from sonara.config import DEFAULTS


def test_config_defaults_have_audio_control_off_and_duck_level_20():
    assert DEFAULTS["audio_control"] is False
    assert DEFAULTS["duck_level"] == 20


def test_set_audio_control_on_persists_and_cues(monkeypatch):
    saved = {}
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: saved.update(c))
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL,
                           "enabled": True})
    assert daemon.config["audio_control"] is True
    assert saved.get("audio_control") is True
    # a spoken confirmation cue was queued on the CONTROL channel
    from sonara.router import CONTROL
    texts = [it.text for it in daemon.router.channel(CONTROL).items]
    assert any("Audio control on" in t for t in texts)


def test_set_audio_control_off_while_ducked_restores_now(monkeypatch):
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: None)
    daemon, *_ = make_daemon(foreground="fg")
    daemon.config["audio_control"] = True
    daemon.ducker._ducked = True               # pretend currently ducked
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL,
                           "enabled": False})
    assert daemon.config["audio_control"] is False
    assert daemon.ducker.restore_calls == 1


def test_set_duck_level_clamps_and_persists(monkeypatch):
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: None)
    daemon, *_ = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL,
                           "level": 150})
    assert daemon.config["duck_level"] == 100   # clamped
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL,
                           "level": -5})
    assert daemon.config["duck_level"] == 0


def test_set_duck_level_reapplies_when_ducked(monkeypatch):
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: None)
    daemon, *_ = make_daemon(foreground="fg")
    daemon.ducker._ducked = True
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL,
                           "level": 35})
    assert daemon.ducker.restore_calls == 1                 # restored then re-ducked
    assert daemon.ducker.duck_calls[-1][1] == 35
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_daemon_ducking.py -v`
Expected: FAIL — `KeyError: 'audio_control'` / `AttributeError: ... SET_AUDIO_CONTROL`

- [ ] **Step 3: Implement**

In `src/sonara/config.py`, extend `DEFAULTS`:

```python
DEFAULTS = {
    "voice": None,
    "rate": 200,
    "verbosity": "everything",
    "background_policy": "earcon_only",
    "history_cap": 200,
    "minqueue": 1,
    "audio_control": False,   # lower other apps' audio while speaking (opt-in)
    "duck_level": 20,         # target % volume for other apps while ducked (0-100)
}
```

In `src/sonara/protocol.py`, add to `MsgType`:

```python
    SET_AUDIO_CONTROL = "set_audio_control"   # enable/disable audio ducking
    SET_DUCK_LEVEL = "set_duck_level"         # set duck target volume (0-100)
```

In `src/sonara/daemon.py` `handle_message`, add two branches next to the other `SET_*` handlers (after `SET_MINQUEUE`):

```python
        if t == MsgType.SET_AUDIO_CONTROL:
            enabled = bool(msg.get("enabled"))
            self.config["audio_control"] = enabled
            save_config(self.config)
            if not enabled and self.ducker.is_ducked():
                self.ducker.restore()      # un-duck immediately on turn-off
            target = self.router.active or self.sessions.foreground()
            self._speak_cue(target, "Audio control on." if enabled else "Audio control off.",
                            exempt_mute=True)
            self._wake.set()
            return None

        if t == MsgType.SET_DUCK_LEVEL:
            try:
                level = max(0, min(100, int(msg.get("level"))))
            except (TypeError, ValueError):
                return None
            self.config["duck_level"] = level
            save_config(self.config)
            if self.ducker.is_ducked():        # re-apply at the new level
                self.ducker.restore()
                self.ducker.duck(self._duck_exclude_pids(), level)
            target = self.router.active or self.sessions.foreground()
            self._speak_cue(target, "Duck level {0} percent.".format(level), exempt_mute=True)
            self._wake.set()
            return None
```

Note: `_duck_exclude_pids()` is added in Task 4. To keep this task's tests green before Task 4, add a minimal version now (Task 4 fills in the earcon PIDs):

```python
    def _duck_exclude_pids(self) -> "set[int]":
        import os
        pids = {os.getpid()}
        try:
            pids.update(self.speaker.earcon_pids())
        except AttributeError:
            pass
        return pids
```

- [ ] **Step 4: Run tests**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_daemon_ducking.py -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonara/config.py src/sonara/protocol.py src/sonara/daemon.py tests/test_daemon_ducking.py
git commit -m "feat(audio): config keys + SET_AUDIO_CONTROL/SET_DUCK_LEVEL handlers"
```

---

### Task 4: Speak-loop duck/restore hooks (hold-until-idle behavior)

**Files:**
- Modify: `src/sonara/speaker.py` (add `earcon_pids()`)
- Modify: `src/sonara/daemon.py` (`_audio_control_on`, `_duck_level`, finalize `_duck_exclude_pids`; duck in the speak path; restore in the idle path, `stop()`, and the PAUSE handler)
- Test: `tests/test_daemon_ducking.py` (extend)

**Interfaces:**
- Consumes: `self.ducker`, `self.config`, `self.router.next_item`, `FakeDucker`.
- Produces: `Speaker.earcon_pids() -> list[int]`; daemon helpers `_audio_control_on()`, `_duck_level()`; duck/restore wired into the loop.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_daemon_ducking.py  (append)
from sonara.queue import SpeechItem


def _prose_item(session, text):
    return SpeechItem(id=0, session=session, kind="prose", text=text, is_decision=False)


def test_no_duck_when_audio_control_off():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.config["audio_control"] = False
    queue.enqueue(_prose_item("fg", "Hello."))
    daemon._speak_loop_once()                       # speaks the item
    assert daemon.ducker.duck_calls == []


def test_duck_once_then_restore_only_at_global_idle():
    # The hold/no-flap behavior: many queued items => exactly ONE duck and ONE
    # restore (at global idle), not one per item. (Restore fires only when
    # next_item() returns None, which is also true across multiple sessions, since
    # the idle condition is global — so one session proves the mechanism without
    # the session-change announcements that would make the count non-deterministic.)
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.config["audio_control"] = True
    queue.enqueue(_prose_item("fg", "One."))
    queue.enqueue(_prose_item("fg", "Two."))
    queue.enqueue(_prose_item("fg", "Three."))
    for _ in range(3):                               # drain every queued item
        daemon._speak_loop_once()
    assert len(daemon.ducker.duck_calls) == 1        # ducked once at first speak
    assert daemon.ducker.restore_calls == 0          # still speaking -> held
    daemon._speak_loop_once()                        # next_item() now None -> idle
    assert daemon.ducker.restore_calls == 1          # restored only at global idle


def test_duck_excludes_daemon_and_earcon_pids():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.config["audio_control"] = True
    speaker._earcon_pids = [4242]                    # see Speaker.earcon_pids() below
    queue.enqueue(_prose_item("fg", "Hi."))
    daemon._speak_loop_once()
    import os
    exclude = daemon.ducker.duck_calls[0][0]
    assert os.getpid() in exclude and 4242 in exclude


def test_stop_restores_if_ducked():
    daemon, *_ = make_daemon(foreground="fg")
    daemon.ducker._ducked = True
    daemon.stop()
    assert daemon.ducker.restore_calls == 1
```

Add an `earcon_pids` test:

```python
# tests/test_speaker.py  (append, or create if absent)
from sonara.speaker import Speaker


class _P:
    def __init__(self, pid, alive): self.pid = pid; self._a = alive
    def poll(self): return None if self._a else 0


def test_earcon_pids_returns_live_helper_pids():
    s = Speaker(say_runner=lambda *a: None)
    s._earcon_procs = [_P(11, True), _P(22, False), _P(33, True)]
    assert set(s.earcon_pids()) == {11, 33}     # only live ones
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_daemon_ducking.py tests/test_speaker.py -v`
Expected: FAIL — `AttributeError: 'Speaker' object has no attribute 'earcon_pids'` and duck not called.

- [ ] **Step 3: Implement**

In `src/sonara/speaker.py`, add the method (uses the existing `self._earcon_procs`; supports a test-injected `_earcon_pids` list):

```python
    def earcon_pids(self) -> "list[int]":
        """PIDs of the live earcon helper subprocesses (so the ducker excludes
        Sonara's own beeps). Test seam: a `_earcon_pids` attribute overrides."""
        injected = getattr(self, "_earcon_pids", None)
        if injected is not None:
            return list(injected)
        return [p.pid for p in self._earcon_procs if p.poll() is None]
```

Also add `earcon_pids()` to the test `FakeSpeaker` in `tests/daemon_helpers.py` (the daemon tests use it, not the real `Speaker`), with the same injection seam:

```python
    def earcon_pids(self):
        return list(getattr(self, "_earcon_pids", []))
```

In `src/sonara/daemon.py`, add the remaining helpers next to the `_duck_exclude_pids` you added in Task 3 (that one is unchanged; add the four others):

```python
    def _audio_control_on(self) -> bool:
        return bool(self.config.get("audio_control"))

    def _duck_level(self) -> int:
        try:
            return max(0, min(100, int(self.config.get("duck_level", 20))))
        except (TypeError, ValueError):
            return 20

    def _duck_exclude_pids(self) -> "set[int]":
        import os
        pids = {os.getpid()}
        try:
            pids.update(self.speaker.earcon_pids())
        except AttributeError:
            pass
        return pids

    def _maybe_duck(self) -> None:
        if self._audio_control_on() and not self.ducker.is_ducked():
            self.ducker.duck(self._duck_exclude_pids(), self._duck_level())

    def _maybe_restore(self) -> None:
        if self.ducker.is_ducked():
            self.ducker.restore()
```

In `_speak_loop_once`, in the **normal speak path**, call `_maybe_duck()` just before speaking a real item. Find the block (after the `if muted: return` / `if item.kind == "session_change":` handling, right before `completed = self.speaker.speak(...)`) and insert:

```python
        self._maybe_duck()
        try:
            completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch)
```

In `_speak_loop_once`, in the **idle path** (the `if item is None:` block that runs after `next_item()` returns `None`), restore before waiting:

```python
        if item is None:
            self._maybe_restore()
            self._wake.wait(self._poll_interval)
            self._wake.clear()
            return
```

In `stop()` (line ~687), add a restore so shutdown un-ducks:

```python
    def stop(self) -> None:
        self._running.clear()
        self._wake.set()
        self._hotkey_q.put(None)
        self._maybe_restore()       # never leave other apps' audio ducked
        self._stop_hotkeys()
        # ... existing server close ...
```

In the `MsgType.PAUSE` handler, when pausing (the branch that sets `self._paused`), restore too (a paused Sonara is "quiet"); the next speak after resume re-ducks. Add `self._maybe_restore()` in the pause-set branch (not the resume branch).

- [ ] **Step 4: Run tests**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_daemon_ducking.py tests/test_speaker.py -v`
Expected: PASS

Then run the speak-loop regression set: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_daemon_loop.py tests/test_daemon_pause_mute.py -q`
Expected: PASS (no regressions).

- [ ] **Step 5: Commit**

```bash
git add src/sonara/speaker.py src/sonara/daemon.py tests/test_daemon_ducking.py tests/test_speaker.py
git commit -m "feat(audio): duck on speak, restore at global idle/stop/pause"
```

---

### Task 5: CLI commands, slash commands, and the pycaw dependency

**Files:**
- Modify: `src/sonara/cli.py` (`_cmd_audio_control`, `_cmd_duck_level`, subparsers, dep list)
- Modify: `pyproject.toml` (`[windows]` extra)
- Create: `commands/audio-control.md`, `commands/duck-level.md`
- Test: `tests/test_cli_ducking.py`, `tests/test_commands.py` (extend `COMMANDS`)

**Interfaces:**
- Consumes: `MsgType.SET_AUDIO_CONTROL`, `MsgType.SET_DUCK_LEVEL`, `_send`.
- Produces: `audio-control on|off` and `duck-level <0-100>` CLI subcommands + their command files; `pycaw` in the install dep set.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_cli_ducking.py
import sonara.cli as cli
from sonara.protocol import MsgType


def test_audio_control_on_sends_enabled_true(monkeypatch):
    sent = {}
    monkeypatch.setattr(cli, "_send", lambda m, expect_reply=False: sent.update(m))
    assert cli.main(["audio-control", "on"]) == 0
    assert sent["type"] == MsgType.SET_AUDIO_CONTROL and sent["enabled"] is True


def test_audio_control_off_sends_enabled_false(monkeypatch):
    sent = {}
    monkeypatch.setattr(cli, "_send", lambda m, expect_reply=False: sent.update(m))
    assert cli.main(["audio-control", "off"]) == 0
    assert sent["enabled"] is False


def test_duck_level_forwards_integer(monkeypatch):
    sent = {}
    monkeypatch.setattr(cli, "_send", lambda m, expect_reply=False: sent.update(m))
    assert cli.main(["duck-level", "35"]) == 0
    assert sent["type"] == MsgType.SET_DUCK_LEVEL and sent["level"] == 35
```

```python
# tests/test_commands.py  -- extend the COMMANDS tuple
COMMANDS = ("status", "verbosity", "doctor", "keymap", "voice", "rate",
            "uninstall", "audio-control", "duck-level")
ARG_COMMANDS = ("verbosity", "voice", "rate", "keymap", "duck-level")
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_cli_ducking.py tests/test_commands.py -v`
Expected: FAIL — unknown CLI subcommand / missing command files.

- [ ] **Step 3: Implement**

In `src/sonara/cli.py`, add the two command functions (near `_cmd_rate`):

```python
def _cmd_audio_control(args) -> int:
    enabled = args.state == "on"
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL, "enabled": enabled})
    print("Audio control {0}.".format("on" if enabled else "off"))
    return 0


def _cmd_duck_level(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL, "level": args.level})
    print("Duck level set to {0} percent.".format(args.level))
    return 0
```

In `src/sonara/cli.py` `main()` (the subparser block near the `rate`/`minqueue` parsers), register them:

```python
    ap = sub.add_parser("audio-control", help="duck other apps' audio while speaking")
    ap.add_argument("state", choices=["on", "off"])
    ap.set_defaults(func=_cmd_audio_control)

    dp = sub.add_parser("duck-level", help="set duck target volume (0-100)")
    dp.add_argument("level", type=int)
    dp.set_defaults(func=_cmd_duck_level)
```

In `src/sonara/cli.py`, add `pycaw` to the install dependency list `_WINRT_PACKAGES` (rename-agnostic: append so install provisions it):

```python
_WINRT_PACKAGES = (
    "winrt-runtime",
    "winrt-Windows.Media.SpeechSynthesis",
    "winrt-Windows.Storage.Streams",
    "pycaw",     # per-app volume control for audio ducking
)
```

In `pyproject.toml`, add `pycaw` to the `[windows]` extra:

```toml
windows = [
    "winrt-runtime; sys_platform == 'win32'",
    "winrt-Windows.Media.SpeechSynthesis; sys_platform == 'win32'",
    "winrt-Windows.Storage.Streams; sys_platform == 'win32'",
    "pycaw; sys_platform == 'win32'",
]
```

Create `commands/audio-control.md`:

```markdown
---
description: Toggle Audio Control (duck other apps' audio while Sonara speaks)
argument-hint: on|off
---

Run the Sonara audio-control command with the Bash tool, forwarding on or off:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" audio-control $ARGUMENTS
```

Print the command's output to the user verbatim. If the command errors, report it briefly.
```

Create `commands/duck-level.md`:

```markdown
---
description: Set how far Audio Control lowers other apps' audio (0-100 percent)
argument-hint: <0-100>
---

Run the Sonara duck-level command with the Bash tool, forwarding the percent value:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" duck-level $ARGUMENTS
```

Print the command's output to the user verbatim. If the command errors (for example
a value outside 0-100), report the error briefly.
```

- [ ] **Step 4: Run tests**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_cli_ducking.py tests/test_commands.py -v`
Expected: PASS

Then the full suite (expect only the known Windows-env failures):
Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest -q`

- [ ] **Step 5: Commit**

```bash
git add src/sonara/cli.py pyproject.toml commands/audio-control.md commands/duck-level.md tests/test_cli_ducking.py tests/test_commands.py
git commit -m "feat(audio): audio-control + duck-level CLI/slash commands + pycaw dep"
```

---

## Final verification (after all tasks)

- [ ] Full suite green except the known Windows-env failures (bin-shim WinError 193, `~/.sonara` home assertions, transport lockfile, winsound fake, autostart).
- [ ] Manual smoke on the deployed daemon (see the deploy runbook): set `audio-control on`, play music, trigger a read → music ducks to `duck_level`% and restores when Sonara finishes; `audio-control off` while speaking restores immediately; kill the daemon mid-speech and restart → startup sweep restores the music.

## Notes for the implementer

- The `~/.sonara/duck_state.json` recovery file is keyed by process identity (pid + name) because in-memory pycaw session objects do not survive a restart.
- `pycaw` pulls `comtypes` (already installed) and `psutil`. Both are pure-Python wheels.
- Do not re-duck per utterance — duck once at batch start, restore at global idle. The single `is_ducked()` flag enforces this.
- Deploy/restart the live daemon via the documented runbook (stop task + kill + clear singleton + copy to `~/.sonara/app/sonara` + restart) — editing repo src alone does nothing.
