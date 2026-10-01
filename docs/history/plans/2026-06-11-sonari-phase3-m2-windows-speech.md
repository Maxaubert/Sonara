# Sonari Phase 3 - Milestone 2: Windows OneCore Speech Pipeline - Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the Sonari daemon run on Windows and **speak Claude's output** - a `platform/windows/` backend (OneCore TTS via PyWinRT, `winsound` earcons, Task-Scheduler autostart), the Windows single-instance guard, and exec-form hooks - all built + mock-tested on macOS, with the real on-Windows verification captured as a deferred acceptance checklist. **No hotkeys** (M3); the Windows hotkey backend is a stub.

**Architecture:** Mirror the macOS backend behind the existing four ABCs. Because the code is Windows-only (imports `winrt`/`winsound`/`winreg`/`msvcrt`), it is made **importable and unit-testable on macOS** by injecting fake modules into `sys.modules` (a conftest harness). `get_platform()` gains the `win32` branch. The Mac suite stays green because `get_platform()` never loads `platform/windows` on darwin; the new Windows tests exercise the logic against fakes.

**Tech Stack:** Python 3.9 core; Windows-only deps reached via lazy import - PyWinRT projections (`winrt-runtime` + `winrt-Windows.Media.SpeechSynthesis`/`.Media.Playback`/`.Media.Core`/`.Storage.Streams`), stdlib `winsound`/`winreg`/`msvcrt`/`wave`. pytest with `sys.modules` fakes.

> **VERIFIED CODE SOURCE - read this first.** The backend bodies (TTS `_TtsHandle`/`run`, `winsound` earcon, `.wav` generator, Task XML + supervisor + `resolve_python`, hooks builder) are web-grounded and provided **verbatim** in `docs/superpowers/m2-windows-api-reference.md`. Where a task says *"implement from the verified reference (§Topic)"*, copy that code, adapting ONLY: (a) the file path/import location to our layout (`src/sonari/platform/windows/...`), (b) subclassing the real ABC, (c) keeping Windows-only imports lazy/guarded. Do not re-derive Windows APIs you cannot test.

**Branch:** `phase-3-windows` (M1 already merged to `main`; continue here).

---

## Invariants (hold at EVERY commit)

1. **macOS behavior unchanged.** `get_platform()` returns the macOS backend on darwin and never imports `platform/windows`. The full suite stays green on **both** interpreters:
   ```bash
   cd ~/projects/private/claude-tts
   TMPDIR=/tmp /usr/bin/python3 -m pytest -q     # 3.9
   TMPDIR=/tmp python3.13 -m pytest -q            # 3.13
   ```
   Baseline at M2 start: **436 passing**. Every task keeps all green (count rises with new Windows tests; never red).
2. **"Green" here means the MOCKED contract holds - NOT that it works on Windows.** The real gate (does it speak, autostart, stay single-instance on real Win10/11) is the **deferred acceptance checklist** (Task 10). Every ⚠ item there is escalated to a human-on-Windows, never asserted green from a mock.
3. **No new pip dependency is imported on the macOS path.** PyWinRT is imported lazily, only inside the Windows backend, only reached on win32 (or via the test fakes). The macOS/core import graph gains nothing.
4. **Windows-only stdlib (`winsound`/`winreg`/`msvcrt`) and `winrt` are imported lazily** (inside functions/methods or guarded `try: import ... except ModuleNotFoundError`), never at module top-level on a path the Mac suite imports for real.
5. Work in `~/projects/private/claude-tts` on `phase-3-windows`. Python 3.9 stdlib idioms. `from __future__ import annotations` at the top of every new module so 3.9 accepts `str | None` hints.

---

## File Structure (created this milestone)

```
src/sonari/platform/windows/
  __init__.py          # make_backend() -> WinPlatformBackend
  tts.py               # WinTtsBackend - OneCore via PyWinRT + _TtsHandle proc-adapter
  earcon.py            # WinEarconBackend - winsound + _DoneHandle/_MissingHandle
  hotkeys.py           # WinHotkeyBackend - STUB (M3 implements)
  supervisor.py        # WinSupervisorBackend + Task XML + resolve_python_windows + hooks
  supervisor_loop.py   # thin pythonw supervisor (Popen-restart w/ backoff)
  earcons/
    __init__.py        # default_earcons() via importlib.resources
    generate.py        # stdlib wave/struct/math earcon generator (reproducibility)
    *.wav              # 6 generated CC0 earcon assets (committed, ~90KB)
docs/superpowers/M2-WINDOWS-ACCEPTANCE.md   # the deferred human-on-Windows checklist
tests/_winfakes.py     # the sys.modules fake-injector for winrt/winsound/winreg/msvcrt
tests/test_win_*.py    # mock-based backend tests
```
**Modified:** `src/sonari/platform/__init__.py` (win32 branch), `src/sonari/platform/transport.py` (Windows single-instance), `tests/conftest.py` (load `_winfakes`), `pyproject.toml` (package-data for the `.wav`), `.gitattributes` (new).

---

# GROUP A - Windows test harness + the daemon's Windows single-instance

### Task 1: The `sys.modules` fake harness (lets Windows modules import on macOS)

**Files:** Create `tests/_winfakes.py`; Modify `tests/conftest.py`; Test `tests/test_winfakes.py`

- [ ] **Step 1: Write the failing test**
```python
# tests/test_winfakes.py
def test_winfakes_make_winrt_and_winsound_importable():
    import tests._winfakes as wf
    wf.install()  # idempotent
    import winsound
    assert hasattr(winsound, "PlaySound")
    from winrt.windows.media.speechsynthesis import SpeechSynthesizer
    s = SpeechSynthesizer()
    assert list(SpeechSynthesizer.all_voices)
    from winrt.windows.media.playback import MediaPlayer
    assert hasattr(MediaPlayer(), "add_media_ended")
```

- [ ] **Step 2: Run → FAIL** (`ModuleNotFoundError: tests._winfakes`).
Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_winfakes.py -q`

- [ ] **Step 3: Create `tests/_winfakes.py`** - inject fakes for `winsound`, `winreg`, `msvcrt`, and the `winrt.*` tree. (Verbatim-derived from the verified intel; the `MediaPlayer` fires `media_ended` on a `threading.Timer` so the TTS handle completes.)
```python
"""Fake Windows modules so platform/windows/* imports + unit-tests on macOS/Linux.
install() is idempotent and uses setdefault - a no-op on real Windows."""
import sys, types, threading


def install():
    if sys.platform == "win32":
        return
    # --- winsound ---
    if "winsound" not in sys.modules:
        ws = types.ModuleType("winsound")
        ws.SND_FILENAME = 0x20000; ws.SND_ASYNC = 0x0004
        ws.SND_NODEFAULT = 0x0002; ws.SND_SYNC = 0x0000
        ws._calls = []
        ws.PlaySound = lambda sound, flags: ws._calls.append((sound, flags))
        sys.modules["winsound"] = ws
    # --- winreg ---
    if "winreg" not in sys.modules:
        wr = types.ModuleType("winreg")
        wr.HKEY_LOCAL_MACHINE = 0x80000002
        wr.OpenKey = lambda *a, **k: object()
        def _enum(key, i):
            raise OSError()
        wr.EnumKey = _enum
        sys.modules["winreg"] = wr
    # --- msvcrt (single-instance lock) ---
    # Track locked INODES (not fds): real msvcrt.locking is a system-wide
    # byte-range lock, so two handles to the SAME file conflict across
    # processes. Modelling by inode makes the cross-process test meaningful.
    if "msvcrt" not in sys.modules:
        mc = types.ModuleType("msvcrt")
        mc.LK_NBLCK = 2; mc.LK_UNLCK = 0
        mc._locked = set()
        def _locking(fd, mode, nbytes):
            import os as _os
            ino = _os.fstat(fd).st_ino
            if mode == mc.LK_NBLCK:
                if ino in mc._locked:
                    raise OSError("locked")
                mc._locked.add(ino)
            elif mode == mc.LK_UNLCK:
                mc._locked.discard(ino)
        mc.locking = _locking
        sys.modules["msvcrt"] = mc
    # --- winrt tree ---
    if "winrt" not in sys.modules:
        _install_winrt()


def _install_winrt():
    mk = lambda n: sys.modules.setdefault(n, types.ModuleType(n))
    mk("winrt"); sysmod = mk("winrt.system")
    mk("winrt.windows"); mk("winrt.windows.media")
    play = mk("winrt.windows.media.playback")
    synth = mk("winrt.windows.media.speechsynthesis")

    class Object: pass
    sysmod.Object = Object

    class SpeechAppendedSilence: DEFAULT = 0; MIN = 1
    class SpeechPunctuationSilence: DEFAULT = 0; MIN = 1
    class _Opts:
        appended_silence = 0; punctuation_silence = 0; speaking_rate = 1.0
    class _Stream: pass
    class _AsyncOp:
        def __init__(self, r): self._r = r
        def get(self): return self._r
    class _Voice:
        def __init__(self, id="HKLM\\SOFTWARE\\Microsoft\\Speech_OneCore\\en-US",
                     language="en-US", display_name="FakeVoice"):
            self.id = id; self.language = language; self.display_name = display_name
    class SpeechSynthesizer:
        all_voices = [_Voice()]; default_voice = _Voice()
        def __init__(self): self.voice = None; self.options = _Opts()
        def synthesize_text_to_stream_async(self, t): return _AsyncOp(_Stream())
        def synthesize_ssml_to_stream_async(self, t): return _AsyncOp(_Stream())
    synth.SpeechSynthesizer = SpeechSynthesizer
    synth.SpeechAppendedSilence = SpeechAppendedSilence
    synth.SpeechPunctuationSilence = SpeechPunctuationSilence

    class MediaPlayerAudioCategory: SPEECH = 3
    class MediaPlayer:
        def __init__(self): self._cb = None; self.audio_category = None
        def set_stream_source(self, s): pass
        def add_media_ended(self, cb): self._cb = cb; return 0
        def play(self):
            t = threading.Timer(0.01, lambda: self._cb and self._cb(self, None))
            t.daemon = True; t.start()
        def pause(self): pass
        def close(self): pass
    play.MediaPlayer = MediaPlayer
    play.MediaPlayerAudioCategory = MediaPlayerAudioCategory
```
In `tests/conftest.py`, add at the very top (before other imports), so every test session has the fakes available:
```python
import tests._winfakes as _winfakes
_winfakes.install()
```

- [ ] **Step 4: Run → PASS.** Then the full suite both interpreters (the fakes must not perturb macOS tests): `TMPDIR=/tmp /usr/bin/python3 -m pytest -q` then `python3.13`.

- [ ] **Step 5: Commit**
```bash
git add tests/_winfakes.py tests/conftest.py tests/test_winfakes.py
git commit -m "test(windows): sys.modules fakes for winrt/winsound/winreg/msvcrt so platform/windows imports on macOS"
```

---

### Task 2: Windows single-instance in `transport.acquire_singleton` (the daemon won't start on Windows without it)

**Files:** Modify `src/sonari/platform/transport.py`; Test `tests/test_transport.py`

> **Why first:** the M1 guard does `import fcntl` (POSIX-only). On Windows that raises → the daemon's `main()` cannot acquire the singleton → it can't run at all. Add a `sys.platform`-branched implementation using `msvcrt.locking` (mirrors the flock-on-a-held-file approach).

- [ ] **Step 1: Write the failing test** (Windows branch selected + exclusivity, via the msvcrt fake)
```python
# add to tests/test_transport.py
def test_acquire_singleton_windows_branch(tmp_path, monkeypatch):
    import importlib, sonari.platform.transport as tr
    monkeypatch.setattr(tr.sys, "platform", "win32")
    lock = tmp_path / "daemon.singleton"
    f1 = tr.acquire_singleton(lock)
    assert f1 is not None
    assert tr.acquire_singleton(lock) is None   # msvcrt fake: 2nd lock on same fd-id fails
    f1.close()
```

- [ ] **Step 2: Run → FAIL** (current code is fcntl-only; `import fcntl` under a win32 monkeypatch still runs fcntl, so the branch/exclusivity assertion fails). Run the one test.

- [ ] **Step 3: Branch `acquire_singleton`** in `transport.py`. Add `import sys` at top. Replace the body:
```python
def acquire_singleton(path):
    """Acquire an exclusive single-instance lock; return the held file object
    (keep a process-lifetime reference) or None if another process holds it.
    POSIX: fcntl.flock (content-independent). Windows: msvcrt.locking on a FIXED
    byte of a NON-truncated file - byte-range locks are system-wide, giving real
    cross-process exclusion; truncating under another holder's lock is undefined,
    and a moving file position would lock the wrong byte. The OS releases the
    lock on process death, so a crash never sticks.

    NOTE: cross-process exclusion on Windows MUST be confirmed on the box
    (M2-WINDOWS-ACCEPTANCE.md). If msvcrt.locking proves unreliable, switch to a
    named mutex (kernel32.CreateMutexW + GetLastError()==ERROR_ALREADY_EXISTS)."""
    fd = os.open(str(path), os.O_RDWR | os.O_CREAT, 0o600)
    fh = os.fdopen(fd, "r+")
    if sys.platform == "win32":
        import msvcrt
        fh.seek(0)
        try:
            msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)   # lock byte [0, 1)
        except OSError:
            fh.close()
            return None
    else:
        import fcntl
        try:
            fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            fh.close()
            return None
    try:
        fh.seek(0); fh.write(str(os.getpid())); fh.flush(); fh.truncate()
    except OSError:
        pass
    return fh
```
(The POSIX path is behaviourally identical to M1's `open("w")` flock - the full gate confirms macOS stays green; `os.open(O_RDWR|O_CREAT)` just makes the open cross-platform-safe.)

- [ ] **Step 4: Run the singleton tests (POSIX + win branch) → PASS**, then full suite both interpreters.
Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_transport.py -q` then the full gate.

- [ ] **Step 5: Commit**
```bash
git add src/sonari/platform/transport.py tests/test_transport.py
git commit -m "feat(transport): Windows single-instance branch (msvcrt.locking) so the daemon can start on win32"
```

---

# GROUP B - The three Windows backends (mock-tested)

### Task 3: Windows earcons - stdlib `.wav` generator + assets

**Files:** Create `src/sonari/platform/windows/__init__.py` (package marker), `.../earcons/__init__.py`, `.../earcons/generate.py`, the 6 `.wav`; Modify `pyproject.toml`; Test `tests/test_earcon_generator.py`

- [ ] **Step 1: Write the failing test** (pure stdlib - runs on any OS, no mock)
```python
# tests/test_earcon_generator.py
import struct, wave, pathlib
from sonari.platform.windows.earcons.generate import generate_earcon, _EARCON_SPECS

def _hdr(p):
    raw = open(p, "rb").read(44)
    return (raw[0:4], raw[8:12], struct.unpack("<H", raw[20:22])[0],
            struct.unpack("<H", raw[22:24])[0], struct.unpack("<I", raw[24:28])[0],
            struct.unpack("<H", raw[34:36])[0])

def test_generate_writes_valid_pcm_wav(tmp_path):
    p = tmp_path / "x.wav"; generate_earcon(p, 440.0, 0.12)
    riff, wav, fmt, ch, sr, bits = _hdr(p)
    assert riff == b"RIFF" and wav == b"WAVE" and fmt == 1 and ch == 1 and sr == 44100 and bits == 16

def test_all_specs_valid(tmp_path):
    for name, (f, d, wt, f2) in _EARCON_SPECS.items():
        p = tmp_path / (name + ".wav"); generate_earcon(p, f, d, wave_type=wt, freq2=f2)
        assert p.stat().st_size > 0
        with wave.open(str(p)) as w:
            assert abs(w.getnframes() / w.getframerate() - d) < 1e-3
```

- [ ] **Step 2: Run → FAIL** (module missing).

- [ ] **Step 3: Create the package + generator.** `src/sonari/platform/windows/__init__.py`: `# sonari.platform.windows - assembled in Task 7.` Create `.../earcons/__init__.py` and `.../earcons/generate.py` with the **verified stdlib generator** (16-bit PCM mono 44100, trapezoid envelope, `_EARCON_SPECS` for the 6 earcons, `generate_all_earcons`, `python -m ...generate` entry). Implement from the verified reference (§earcon - the stdlib WAV generator).

- [ ] **Step 4: Generate the assets + run tests → PASS**
```bash
TMPDIR=/tmp /usr/bin/python3 -m sonari.platform.windows.earcons.generate \
   src/sonari/platform/windows/earcons
TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_earcon_generator.py -q
```
(Confirm 6 `.wav` written.)

- [ ] **Step 5: `default_earcons()` + package-data.** `.../earcons/__init__.py`: `default_earcons()` resolving the 6 `.wav` via `importlib.resources.files(__package__)` (3.9 `as_file`), raising `FileNotFoundError` if absent. In `pyproject.toml` add:
```toml
[tool.setuptools.package-data]
sonari = ["platform/windows/earcons/*.wav"]
```
Add a test `tests/test_win_earcons_assets.py::test_default_earcons_has_6` asserting `default_earcons()` returns 6 existing paths.

- [ ] **Step 6: Full gate both interpreters → PASS. Commit**
```bash
git add src/sonari/platform/windows/__init__.py src/sonari/platform/windows/earcons \
        pyproject.toml tests/test_earcon_generator.py tests/test_win_earcons_assets.py
git commit -m "feat(windows): stdlib-generated CC0 .wav earcons + default_earcons() (no Apple assets on Windows)"
```

---

### Task 4: `WinEarconBackend` (winsound)

**Files:** Create `src/sonari/platform/windows/earcon.py`; Test `tests/test_win_earcon.py`

- [ ] **Step 1: Write the failing test** (uses the winsound fake)
```python
# tests/test_win_earcon.py
import wave, struct, math
from sonari.platform.windows.earcon import WinEarconBackend

def _wav(tmp_path):
    p = tmp_path / "e.wav"
    with wave.open(str(p), "w") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(44100)
        w.writeframes(b"".join(struct.pack("<h", int(math.sin(i/10)*1000)) for i in range(4410)))
    return p

def test_play_existing_returns_done_handle(tmp_path):
    import winsound; winsound._calls.clear()
    h = WinEarconBackend().play(str(_wav(tmp_path)))
    assert h.poll() == 0
    assert len(winsound._calls) == 1
    assert winsound._calls[0][1] == (winsound.SND_FILENAME | winsound.SND_ASYNC)

def test_play_missing_returns_none_handle(tmp_path):
    import winsound; winsound._calls.clear()
    h = WinEarconBackend().play(str(tmp_path / "nope.wav"))
    assert h.poll() is None and winsound._calls == []

def test_default_earcons_six():
    assert len(WinEarconBackend().default_earcons()) == 6
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** `src/sonari/platform/windows/earcon.py` with the verified `_DoneHandle`/`_MissingHandle` + lazy `winsound` import (guarded `try: import winsound except ModuleNotFoundError`), `play()`, and `default_earcons()` delegating to `.earcons.default_earcons()`. Subclass `EarconBackend`. Implement from the verified reference (§earcon - winsound play()).

- [ ] **Step 4: Run → PASS, full gate, commit**
```bash
git add src/sonari/platform/windows/earcon.py tests/test_win_earcon.py
git commit -m "feat(windows): WinEarconBackend via winsound (poll()-able handles)"
```

---

### Task 5: `WinTtsBackend` (OneCore via PyWinRT) - the `_TtsHandle` proc-adapter

**Files:** Create `src/sonari/platform/windows/tts.py`; Test `tests/test_win_tts.py`

> **The crux.** OneCore has no subprocess, so `run()` returns `_TtsHandle` - a proc-like object whose `.wait(timeout)` blocks on a `threading.Event` (raising `subprocess.TimeoutExpired` on timeout), `.terminate()` pauses playback + sets `returncode=1`, `.returncode` is `0` on `MediaEnded`. Uses `IAsyncOperation.get()` (NOT asyncio - no loop in the daemon thread). Holds GC-refs to stream/synth/callback.

- [ ] **Step 1: Write the failing test** (the fake MediaPlayer fires media_ended on a timer)
```python
# tests/test_win_tts.py
import subprocess, pytest
from sonari.platform.windows.tts import WinTtsBackend, wpm_to_speaking_rate

def test_list_and_best_voice():
    b = WinTtsBackend()
    assert isinstance(b.list_voices(), list) and b.list_voices()
    assert "speech_onecore" in (b.best_voice().id or "").lower()

def test_run_completes_returns_zero():
    h = WinTtsBackend().run("hello", None, 200)
    assert h.wait(timeout=2.0) == 0

def test_terminate_sets_returncode_one():
    h = WinTtsBackend().run("hello", None, 200)
    h.terminate()
    assert h.returncode == 1

def test_wait_timeout_raises(monkeypatch):
    # a player that never fires media_ended → wait must raise TimeoutExpired
    import winrt.windows.media.playback as pb
    monkeypatch.setattr(pb.MediaPlayer, "play", lambda self: None)
    h = WinTtsBackend().run("hello", None, 200)
    with pytest.raises(subprocess.TimeoutExpired):
        h.wait(timeout=0.05)

def test_wpm_maps_to_multiplier():
    assert abs(wpm_to_speaking_rate(200) - 1.0) < 1e-6
    assert wpm_to_speaking_rate(400) > 1.0 and wpm_to_speaking_rate(100) < 1.0

def test_run_falls_back_when_voice_name_unknown():
    # a stale/foreign voice name (e.g. macOS "Samantha") must not be assigned
    # as-is to synth.voice - run() resolves it or falls back to best_voice().
    h = WinTtsBackend().run("hi", "Samantha", 200)  # fake has no such voice
    assert h.wait(timeout=2.0) == 0   # did not crash on an unresolved name
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** `src/sonari/platform/windows/tts.py` with the **verified** code: lazy `winrt.*` imports inside `run()`/`best_voice()`/`list_voices()` (so the module imports even without the fakes, though tests use fakes); `wpm_to_speaking_rate(wpm)` = `max(0.5, min(6.0, wpm/200.0))`; `best_voice()` (en-US OneCore by id → any en-US → `default_voice` → `RuntimeError`); `run()` (options `appended_silence`/`punctuation_silence` MIN, `speaking_rate` try/AttributeError→SSML fallback, `synthesize_text_to_stream_async(text).get()`, `MediaPlayer` SPEECH category, `set_stream_source`, `_TtsHandle`, `play()`); `_TtsHandle` (`wait` raises `subprocess.TimeoutExpired`, `terminate` pause/close + `returncode=1`, GC-refs). Subclass `TtsBackend`. Implement from the verified reference (§Windows OneCore TTS - the full synth→stream→play pattern + `_TtsHandle`).
> The ABC's `run(self, text, voice, rate)` takes the Sonari wpm `rate` (int) - map it internally via `wpm_to_speaking_rate`.
>
> **CRITICAL deviation from the reference - voice resolution.** `Speaker` passes `voice` as a voice-NAME **string** (from config) or `None` - but `synthesizer.voice` requires a `VoiceInformation` **object**. The reference's `synth.voice = voice` is wrong for our contract. `run()` must resolve: if `voice` is a non-empty string, find the matching `VoiceInformation` in `all_voices` by `display_name` (case-insensitive); if not found (e.g. a stale macOS voice name like `"Samantha"`), fall back to `best_voice()`; if `voice is None`, use `best_voice()`. Add a helper `_resolve_voice(name)`.

- [ ] **Step 4: Run → PASS, full gate, commit**
```bash
git add src/sonari/platform/windows/tts.py tests/test_win_tts.py
git commit -m "feat(windows): WinTtsBackend - OneCore via PyWinRT with a subprocess-like playback handle"
```

---

### Task 6: `WinSupervisorBackend` + Task XML + `resolve_python` + supervisor loop

**Files:** Create `src/sonari/platform/windows/supervisor.py`, `.../supervisor_loop.py`; Test `tests/test_win_supervisor.py`

- [ ] **Step 1: Write the failing tests** - the verified suite: Task XML asserted via `ElementTree` (LogonTrigger/UserId, RestartOnFailure PT5M, RunLevel LeastPrivilege), `launch_spec` creationflags (`& 0x08000000`, `& 0x00000008`, no `start_new_session`), `is_installed` calls `schtasks /query /tn`, `doctor_rows` include "Task Scheduler task" + "neural voice", `_is_store_stub` WindowsApps fast-path, `_SPAWN_FLAGS == 0x08000008`. Use the verified test file from the reference (§supervisor - mock strategy), importing from `sonari.platform.windows.supervisor`.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** `supervisor.py` with the verified code: `TASK_NAME`, `TASK_XML_TEMPLATE` (UTF-16/`InteractiveToken`/`LeastPrivilege`/`RestartOnFailure`/`Hidden`), `_current_user_id` (lazy `ctypes`), `task_install`/`task_uninstall`/`task_is_installed`, `resolve_python_windows` (+`_is_store_stub`/`_find_pythonw`/`_probe_python_version`/`_probe_version_via_launcher`), `build_hooks_json` + `_GITATTRIBUTES_LINE`, and `WinSupervisorBackend` (thin `_schtasks`/`_probe_python_version`/`_list_neural_voices` wrappers [lazy `winreg`]; `is_installed`/`is_running`/`resolve_python`/`launch_spec`/`doctor_rows`/`install`/`uninstall`). Create `supervisor_loop.py` (verified `launch_spec`/`run_supervisor_loop` with backoff; `__main__` entry). All Windows-only imports lazy. Implement from the verified reference (§supervisor - Task XML, resolve_python, supervisor loop, WinSupervisorBackend).

- [ ] **Step 4: Run → PASS, full gate, commit**
```bash
git add src/sonari/platform/windows/supervisor.py src/sonari/platform/windows/supervisor_loop.py tests/test_win_supervisor.py
git commit -m "feat(windows): WinSupervisorBackend - Task Scheduler XML (no admin) + py-launcher resolution + supervisor loop"
```

---

### Task 7: `WinHotkeyBackend` stub + assemble `WinPlatformBackend` + factory branch

**Files:** Create `src/sonari/platform/windows/hotkeys.py`; Modify `src/sonari/platform/windows/__init__.py`, `src/sonari/platform/__init__.py`; Test `tests/test_win_backend.py`

- [ ] **Step 1: Write the failing test**
```python
# tests/test_win_backend.py
from sonari.platform import base

def test_make_windows_backend_full_bundle():
    from sonari.platform.windows import make_backend
    pb = make_backend()
    assert isinstance(pb, base.PlatformBackend)
    for part, cls in [(pb.tts, base.TtsBackend), (pb.earcon, base.EarconBackend),
                      (pb.hotkey, base.HotkeyBackend), (pb.supervisor, base.SupervisorBackend)]:
        assert isinstance(part, cls)

def test_hotkey_stub_reports_deferred():
    from sonari.platform.windows.hotkeys import WinHotkeyBackend
    ok, detail = WinHotkeyBackend().install("log", "agent", lambda a: 0)
    assert ok is False and "M3" in detail

def test_get_platform_win32(monkeypatch):
    import sonari.platform as platform
    monkeypatch.setattr(platform.sys, "platform", "win32")
    platform._CACHE = None
    pb = platform.get_platform()
    assert isinstance(pb, base.PlatformBackend)
    platform._CACHE = None
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement.** `hotkeys.py`: `WinHotkeyBackend(HotkeyBackend)` - `install(log_path, agent_path, launchctl_fn)` returns `(False, "Windows hotkeys land in Milestone 3.")`; `uninstall()` no-op; `display_combo(modifiers, key_code)` returns a simple `"Ctrl+Shift+Alt+<key>"`-style label (a minimal Windows VK display map, or `"keyN"` fallback - full M3). `windows/__init__.py`:
```python
from sonari.platform.base import PlatformBackend
from sonari.platform.windows.tts import WinTtsBackend
from sonari.platform.windows.earcon import WinEarconBackend
from sonari.platform.windows.hotkeys import WinHotkeyBackend
from sonari.platform.windows.supervisor import WinSupervisorBackend

def make_backend() -> PlatformBackend:
    return PlatformBackend(tts=WinTtsBackend(), earcon=WinEarconBackend(),
                           hotkey=WinHotkeyBackend(), supervisor=WinSupervisorBackend())
```
In `platform/__init__.py`, replace the `win32` raise:
```python
    elif sys.platform == "win32":
        from sonari.platform.windows import make_backend
```

- [ ] **Step 4: Run → PASS, full gate, commit**
```bash
git add src/sonari/platform/windows/hotkeys.py src/sonari/platform/windows/__init__.py \
        src/sonari/platform/__init__.py tests/test_win_backend.py
git commit -m "feat(windows): WinHotkeyBackend stub (M3) + WinPlatformBackend + get_platform() win32 branch"
```

---

# GROUP C - Windows install glue (mock-tested) + the deferred acceptance gate

### Task 8: Exec-form hooks + `.gitattributes`

**Files:** Modify `src/sonari/platform/windows/supervisor.py` (hooks writer, if not done in T6); Create `.gitattributes`; Test `tests/test_win_hooks.py`

> The macOS `hooks.json` (shell-form `bin/sonari-hook`) is untouched - `sonari install` on Windows writes the **exec-form** hook config (`command` = resolved `pythonw`, `args` = `[hook.py, EventName]`) with backslashes JSON-escaped. The exact Claude-Code-hooks location on Windows is a deferred acceptance item (Task 10) - this task delivers + tests the *content builder*.

- [ ] **Step 1: Write the failing test**
```python
# tests/test_win_hooks.py
import json
from sonari.platform.windows.supervisor import build_hooks_json

def test_hooks_json_is_exec_form_with_escaped_paths():
    s = build_hooks_json(r"C:\u\.sonari\pythonw.exe", r"C:\plug\hook.py")
    data = json.loads(s)  # valid JSON (backslashes doubled)
    md = data["hooks"]["MessageDisplay"][0]["hooks"][0]
    assert md["type"] == "command"
    assert md["command"].endswith("pythonw.exe")
    assert md["args"][0].endswith("hook.py") and md["args"][-1] == "MessageDisplay"
```

- [ ] **Step 2: Run → FAIL** (if `build_hooks_json` from T6 doesn't yet cover MessageDisplay/Stop shape).

- [ ] **Step 3: Implement / confirm** `build_hooks_json(pythonw, hook_py)` produces the verified exec-form JSON (MessageDisplay + Stop + the other Sonari events Sonari actually consumes - match the macOS `hooks/hooks.json` event set). Create `.gitattributes` at repo root:
```
hooks/*.py text eol=lf
src/sonari/**/*.py text eol=lf
```

- [ ] **Step 4: Run → PASS, full gate, commit**
```bash
git add src/sonari/platform/windows/supervisor.py .gitattributes tests/test_win_hooks.py
git commit -m "feat(windows): exec-form hooks.json builder + .gitattributes LF enforcement"
```

---

### Task 9: Doctor wiring - Windows rows reachable via the seam (sanity)

**Files:** Verify `src/sonari/cli.py doctor()` composes `get_platform().supervisor.doctor_rows()`; Test `tests/test_win_doctor_rows.py`

> M1 already made `doctor()` pull platform rows from the supervisor backend. This task confirms the Windows rows flow through under a win32 monkeypatch (no cli changes expected; if `doctor()` hardcodes anything macOS, fix it).

- [ ] **Step 1: Write the test**
```python
# tests/test_win_doctor_rows.py
def test_windows_supervisor_doctor_rows(monkeypatch):
    from sonari.platform.windows.supervisor import WinSupervisorBackend
    sup = WinSupervisorBackend()
    monkeypatch.setattr(sup, "_schtasks", lambda a: 0)
    monkeypatch.setattr(sup, "resolve_python", lambda: r"C:\Py\pythonw.exe")
    monkeypatch.setattr(sup, "_list_neural_voices", lambda: ["Microsoft Aria"])
    monkeypatch.setattr("sonari.paths.socket_connectable", lambda: True)
    names = [r[0] for r in sup.doctor_rows()]
    assert {"Task Scheduler task", "pythonw.exe", "neural voice", "daemon running"} <= set(names)
```

- [ ] **Step 2-4:** Run → PASS (likely first try). If `cli.doctor()` needs a tweak to stay OS-agnostic, make it + keep the macOS doctor tests green. Full gate. Commit:
```bash
git add tests/test_win_doctor_rows.py src/sonari/cli.py
git commit -m "test(windows): doctor rows compose from WinSupervisorBackend through the seam"
```

---

### Task 10: The deferred Windows acceptance checklist (escalate-the-unverifiable)

**Files:** Create `docs/superpowers/M2-WINDOWS-ACCEPTANCE.md`

> Per the iterate-and-verify doctrine: what a mock can't verify must be **handed to a human-on-Windows**, never asserted green. This file is the gate M2 cannot close on the Mac.

- [ ] **Step 1: Author the checklist** covering, with exact commands + what to listen for:
  - **Install:** `pip install` the PyWinRT projection set; `sonari install` registers the Task (no UAC prompt - confirm non-admin); `schtasks /query /tn Sonari.Speechd` shows it.
  - ⚠ **Speech:** in a real `claude` session, Claude's prose is spoken by a **OneCore** voice; rate is usable (the 750ms-silence hardening applied).
  - ⚠ **Interrupt:** `stop`/`skip` cut speech mid-utterance (`_TtsHandle.terminate` → `MediaPlayer.pause`).
  - ⚠ **Earcons:** each message type plays its distinct generated `.wav`.
  - ⚠ **Single-instance:** rapid hook activity yields **one** daemon (the `msvcrt.locking` guard) - `tasklist | findstr python`.
  - ⚠ **Autostart + restart:** log off/on → daemon returns; kill it → the supervisor restarts it (backoff).
  - ⚠ **Hooks fire:** confirm the exec-form hook actually reaches the daemon (resolve the exact Claude-Code Windows hooks path).
  - **RISKS to probe explicitly (mock-blind):** (a) **SAPI/MediaPlayer audio from a `DETACHED_PROCESS|CREATE_NO_WINDOW` Task-Scheduler process** - the #1 risk: neural `Speak()`/`MediaPlayer` may emit no audio or hang without an STA + `CoInitializeEx(COINIT_APARTMENTTHREADED)`; if so, add a defensive `CoInitializeEx` (via `ctypes.windll.ole32`) in the daemon/TTS thread startup. (b) **`IAsyncOperation.get()` actually blocks** and returns the stream from a plain daemon thread (vs requiring `await`). (c) **Single-instance truly excludes across processes** - spawn two daemons, confirm one survives; if `msvcrt.locking` doesn't exclude, switch to a named mutex (`kernel32.CreateMutexW` + `ERROR_ALREADY_EXISTS`). (d) `schtasks /xml` UTF-16 acceptance + non-admin LogonTrigger registration (no UAC). (e) Store-stub avoidance on a machine where only Store Python exists. (f) `importlib.resources.as_file` temp-path lifetime for wheel installs. (g) `winsound` rapid-earcon truncation (two earcons in quick succession). (h) PyWinRT projection availability for the target arch (win-amd64 confirmed; **win-arm64** may be unavailable → document).
  - **Residual:** Nima is low-vision (magnifier) - a fully-blind + NVDA pass is a separate pre-GA step.

- [ ] **Step 2: Commit**
```bash
git add docs/superpowers/M2-WINDOWS-ACCEPTANCE.md
git commit -m "docs(windows): M2 acceptance checklist - the deferred human-on-Windows verification gate"
```

---

## Self-Review checklist (run before handoff)

- **Spec §3 coverage:** OneCore default ✅(T5), winsound earcons + bundled `.wav` ✅(T3,T4), Task-Scheduler-XML no-admin autostart ✅(T6), `py`-launcher + Store-stub resolution ✅(T6), exec-form hooks + `.gitattributes` ✅(T8), Windows single-instance ✅(T2), hotkeys stubbed ✅(T7), factory win32 branch ✅(T7), doctor rows ✅(T9). Out of M2 (M3): real Windows hotkeys.
- **The Mac suite never imports `platform/windows` for real:** `get_platform()` darwin → macos; the win modules only load under the `_winfakes` harness or win32. Guard via the existing `tests/test_no_os_branch_in_core.py` (core unaffected) + the win tests' reliance on fakes.
- **Every ⚠ behavior is in the Task-10 checklist, not asserted from a mock.** The single biggest mock-blind risk (SAPI/COM audio under DETACHED_PROCESS) is called out explicitly.
- **No placeholders:** load-bearing code (the `_winfakes` harness, the singleton branch, the factory wiring, the test bodies) is inlined; the larger verified backend bodies are provided **verbatim** in `docs/superpowers/m2-windows-api-reference.md` (committed alongside this plan) and each task points at the exact section - the executor copies them, adapting only file paths + ABC subclassing.
- **Type/name consistency:** backends subclass the as-built ABCs (`TtsBackend.run/best_voice/list_voices`; `EarconBackend.play/default_earcons`; `HotkeyBackend.install(log_path, agent_path, launchctl_fn)/uninstall/display_combo`; `SupervisorBackend.install(python, app_dir)/uninstall/is_running/is_installed/resolve_python/launch_spec/doctor_rows`). `_TtsHandle` honors the proc contract (`wait`→`TimeoutExpired`, `terminate`, `returncode`).

---

## Execution Handoff

Subagent-driven, same as M1 - but the green-gate is **mock-based** (it proves the contracts, not Windows behavior). The real gate is `M2-WINDOWS-ACCEPTANCE.md`, run when the M0 Windows box exists.
