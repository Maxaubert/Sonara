# Sonari Phase 3 — Milestone 1: Platform Seam + AF_UNIX→TCP — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a `sonari/platform/` abstraction seam (four backends behind one `get_platform()` factory) and migrate IPC from AF_UNIX to localhost TCP — **on macOS, with zero observable behavior change** — so Phase 3 can add a Windows backend later without the core ever branching on the OS.

**Architecture:** A new `platform/` package holds four backend interfaces (`TtsBackend`, `EarconBackend`, `HotkeyBackend`, `SupervisorBackend`) and one `PlatformBackend` bundle returned by `get_platform()` — the only `sys.platform` branch in the codebase. Existing macOS code (`run_say`/`play_earcon`/`best_enhanced_voice`, the keymap code tables, the launchctl/plist/swiftc/python-resolution machinery) **moves** behind the macOS backend; the portable core (`Speaker`, the keymap resolver, `daemon`, `cli` argparse) becomes an OS-agnostic *consumer* of the injected backend. IPC moves to a shared `platform/transport.py` over `127.0.0.1` + ephemeral port + a 256-bit token in a `0o600` lockfile.

**Tech Stack:** Python 3.9 stdlib only (`socket`, `secrets`, `json`, `subprocess`, `threading`, `abc`, `dataclasses`); pytest; Swift/Carbon `sonari-hotkeyd` (one socket-call site changes). Tests must pass under **both** Python 3.9 and 3.13.

**Branch:** `phase-3-windows` (already created off `main`).

---

## Invariants (hold at EVERY commit)

1. **Zero behavior change on macOS.** After each task the daemon narrates, earcons, hotkeys, install/uninstall/doctor all behave exactly as v0.5.0. The only user-visible artifact change is the socket file (`~/.sonari/speechd.sock`) becoming a lockfile (`~/.sonari/daemon.lock`) in Group C.
2. **The full suite is green** under both interpreters. Run every test with `TMPDIR=/tmp` (the macOS 104-char AF_UNIX path limit makes `test_client_send.py` spuriously fail under a long default `TMPDIR`; Group C removes that dependency but keep the habit):
   ```bash
   cd ~/projects/private/claude-tts
   TMPDIR=/tmp /usr/bin/python3 -m pytest -q          # 3.9
   TMPDIR=/tmp python3.13 -m pytest -q                 # 3.13
   ```
   Baseline before starting: ~362 passing.
3. **The portable core never imports `platform/macos` or branches on `sys.platform`.** Only `platform/__init__.py` may read `sys.platform`. Enforced by a test in Task 11.
4. **The daemon loads from `~/.sonari/app`, not the repo.** Code changes reach the live daemon only after `sonari install`. Do NOT run `sonari install` mid-plan (it rebuilds hotkeyd and could disturb the running daemon); the suite is the gate. A single manual `sonari install` + ear-check happens once, at the end (Task 12).

---

## File Structure (what this milestone creates / moves)

**New files:**
- `src/sonari/platform/__init__.py` — `get_platform()` factory (the only `sys.platform` branch).
- `src/sonari/platform/base.py` — the four ABCs + `PlatformBackend` dataclass.
- `src/sonari/platform/transport.py` — shared localhost-TCP IPC (lockfile + token helpers, used by both OSes).
- `src/sonari/platform/macos/__init__.py` — assembles `MacPlatformBackend`.
- `src/sonari/platform/macos/tts.py` — `MacTtsBackend` (was `run_say` + `best_enhanced_voice`).
- `src/sonari/platform/macos/earcon.py` — `MacEarconBackend` (was `play_earcon`).
- `src/sonari/platform/macos/keytables.py` — `KEY_CODES` + `MOD_MASKS` (Carbon values).
- `src/sonari/platform/macos/hotkeys.py` — `MacHotkeyBackend` (was `_build_hotkeyd` / `_hotkeyd_plist` / display tables).
- `src/sonari/platform/macos/supervisor.py` — `MacSupervisorBackend` (was launchctl/plist/`_resolve_python`/launcher/install/uninstall machinery + doctor rows).
- `tests/test_platform_base.py`, `tests/test_platform_factory.py`, `tests/test_transport.py`, `tests/test_macos_tts.py`, `tests/test_macos_earcon.py`, `tests/test_macos_supervisor.py`.

**Modified:** `src/sonari/speaker.py`, `src/sonari/config.py`, `src/sonari/keymap.py`, `src/sonari/paths.py`, `src/sonari/client.py`, `src/sonari/daemon.py`, `src/sonari/cli.py`, `hotkeyd/sonari-hotkeyd.swift`, and the test files enumerated per task.

---

# GROUP A — Scaffold the seam

### Task 1: The four backend interfaces + `PlatformBackend` bundle

**Files:**
- Create: `src/sonari/platform/__init__.py` (empty package marker for now — the factory lands in Task 10)
- Create: `src/sonari/platform/base.py`
- Test: `tests/test_platform_base.py`

- [ ] **Step 1: Write the failing test**

```python
# tests/test_platform_base.py
import abc
import pytest
from sonari.platform import base


def test_backends_are_abstract():
    for cls in (base.TtsBackend, base.EarconBackend,
                base.HotkeyBackend, base.SupervisorBackend):
        assert issubclass(cls, abc.ABC)
        with pytest.raises(TypeError):
            cls()  # cannot instantiate an ABC with abstract methods


def test_platform_backend_bundles_the_four():
    class _Tts(base.TtsBackend):
        def run(self, text, voice, rate): return None
        def best_voice(self): return "x"
        def list_voices(self): return []
    class _Ear(base.EarconBackend):
        def play(self, path): return None
        def default_earcons(self): return {}
    class _Hk(base.HotkeyBackend):
        def install(self): return (True, "")
        def uninstall(self): return None
        def display_combo(self, modifiers, key_code): return ""
    class _Sup(base.SupervisorBackend):
        def install(self, python, app_dir): return None
        def uninstall(self): return None
        def is_running(self): return False
        def is_installed(self): return False
        def resolve_python(self): return None
        def launch_spec(self): return ([], {})
        def doctor_rows(self): return []
    pb = base.PlatformBackend(tts=_Tts(), earcon=_Ear(),
                              hotkey=_Hk(), supervisor=_Sup())
    assert isinstance(pb.tts, base.TtsBackend)
    assert isinstance(pb.supervisor, base.SupervisorBackend)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_platform_base.py -q`
Expected: FAIL — `ModuleNotFoundError: No module named 'sonari.platform'`.

- [ ] **Step 3: Create the package marker and the ABCs**

Create `src/sonari/platform/__init__.py` with a single line:
```python
# sonari.platform — OS abstraction seam. get_platform() lands in Task 10.
```

Create `src/sonari/platform/base.py`:
```python
"""Platform backend interfaces. The portable core depends ONLY on these
abstractions; concrete macOS/Windows implementations live in sibling packages
and are wired in by get_platform() (the only sys.platform branch)."""
from __future__ import annotations

import abc
from dataclasses import dataclass


class TtsBackend(abc.ABC):
    @abc.abstractmethod
    def run(self, text: str, voice, rate: int):
        """Start speaking *text*; return a proc-like handle exposing
        .wait(timeout=None), .terminate(), and .returncode (0 == completed).
        This is the say_runner the Speaker orchestrates."""

    @abc.abstractmethod
    def best_voice(self) -> str:
        """Return the best installed voice name (a sensible default)."""

    @abc.abstractmethod
    def list_voices(self) -> "list[str]":
        """Return installed voice names (may be empty)."""


class EarconBackend(abc.ABC):
    @abc.abstractmethod
    def play(self, path: str):
        """Play the sound at *path* non-blocking; return a proc-like handle
        exposing .poll(), or None on error/missing file."""

    @abc.abstractmethod
    def default_earcons(self) -> "dict":
        """Return the platform's default {kind: sound_path} mapping."""


class HotkeyBackend(abc.ABC):
    @abc.abstractmethod
    def install(self) -> "tuple":
        """Set up the global-hotkey mechanism. Return (ok: bool, detail: str)."""

    @abc.abstractmethod
    def uninstall(self) -> None:
        """Tear down the global-hotkey mechanism."""

    @abc.abstractmethod
    def display_combo(self, modifiers: int, key_code: int) -> str:
        """Human label for a (modifiers, key_code) pair, e.g. 'Ctrl+Cmd+O'."""


class SupervisorBackend(abc.ABC):
    @abc.abstractmethod
    def install(self, python: str, app_dir: str) -> None: ...
    @abc.abstractmethod
    def uninstall(self) -> None: ...
    @abc.abstractmethod
    def is_running(self) -> bool: ...
    @abc.abstractmethod
    def is_installed(self) -> bool:
        """Cheap check the user ran `sonari install` (the launcher/agent exists)."""
    @abc.abstractmethod
    def resolve_python(self): ...
    @abc.abstractmethod
    def launch_spec(self) -> "tuple":
        """Return (argv, spawn_kwargs) to lazily start the daemon process."""
    @abc.abstractmethod
    def doctor_rows(self) -> "list":
        """Return platform-specific [(name, ok, detail), ...] diagnostic rows."""


@dataclass
class PlatformBackend:
    tts: TtsBackend
    earcon: EarconBackend
    hotkey: HotkeyBackend
    supervisor: SupervisorBackend
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_platform_base.py -q`
Expected: PASS (2 passed).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/platform/__init__.py src/sonari/platform/base.py tests/test_platform_base.py
git commit -m "feat(platform): scaffold the four backend ABCs + PlatformBackend bundle"
```

---

# GROUP B — Move the macOS backends behind the seam (zero behavior change)

### Task 2: macOS TTS backend (`run_say` + `best_enhanced_voice`)

**Files:**
- Create: `src/sonari/platform/macos/__init__.py`, `src/sonari/platform/macos/tts.py`
- Test: `tests/test_macos_tts.py`
- Modify: `src/sonari/speaker.py` (`run_say`/`best_enhanced_voice` → delegating shims; `Speaker` class + defaults UNCHANGED), `tests/test_speaker.py` (remove the `best_enhanced_voice` direct tests + their import; relocated to `test_macos_tts.py`)

- [ ] **Step 1: Write the failing test** (the moved unit, patched through the new module)

```python
# tests/test_macos_tts.py
from sonari.platform.macos import tts as mod
from sonari.platform.macos.tts import MacTtsBackend


def test_run_builds_say_command_with_voice_and_rate(monkeypatch):
    calls = {}
    class _P:  # fake Popen
        def __init__(self, cmd): calls["cmd"] = cmd
    monkeypatch.setattr(mod.subprocess, "Popen", _P)
    MacTtsBackend().run("Hi", "Ava", 220)
    assert calls["cmd"] == ["say", "-v", "Ava", "-r", "220", "Hi"]


def test_best_voice_prefers_premium_en(monkeypatch):
    listing = "Ava (Premium)   en_US  # hi\nDaniel          en_GB  # hi\n"
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    assert MacTtsBackend().best_voice() == "Ava"


def test_best_voice_falls_back_when_say_errors(monkeypatch):
    def boom(*a, **k): raise FileNotFoundError()
    monkeypatch.setattr(mod.subprocess, "check_output", boom)
    assert MacTtsBackend().best_voice() == "Samantha"
```

- [ ] **Step 2: Run it to verify it fails**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_macos_tts.py -q`
Expected: FAIL — `ModuleNotFoundError: No module named 'sonari.platform.macos'`.

- [ ] **Step 3: Create the macOS package + TTS backend (move the verbatim code)**

Create `src/sonari/platform/macos/__init__.py`:
```python
# sonari.platform.macos — the macOS PlatformBackend. Assembled in Task 9.
```

Create `src/sonari/platform/macos/tts.py` — move `run_say` and `best_enhanced_voice` verbatim from `speaker.py` (lines 8–13 and 26–64) into a backend class:
```python
"""macOS TTS backend — wraps the `say` command."""
from __future__ import annotations

import subprocess

from sonari.platform.base import TtsBackend


class MacTtsBackend(TtsBackend):
    def run(self, text: str, voice, rate: int):
        cmd = ["say"]
        if voice:
            cmd += ["-v", voice]
        cmd += ["-r", str(rate), text]
        return subprocess.Popen(cmd)

    def list_voices(self) -> "list[str]":
        try:
            listing = subprocess.check_output(["say", "-v", "?"], text=True)
        except (FileNotFoundError, OSError, subprocess.SubprocessError):
            return []
        names = []
        for line in listing.splitlines():
            before_hash = line.split("#", 1)[0].rstrip()
            parts = before_hash.split()
            if len(parts) >= 2:
                names.append(" ".join(parts[:-1]))
        return names

    def best_voice(self) -> str:
        fallback = "Samantha"
        try:
            listing = subprocess.check_output(["say", "-v", "?"], text=True)
        except (FileNotFoundError, OSError, subprocess.SubprocessError):
            return fallback
        premium_en, plain_en = [], []
        for line in listing.splitlines():
            line = line.rstrip()
            if not line:
                continue
            before_hash = line.split("#", 1)[0].rstrip()
            parts = before_hash.split()
            if len(parts) < 2:
                continue
            locale = parts[-1]
            name = " ".join(parts[:-1])
            is_premium = "(Premium)" in name or "(Enhanced)" in name
            bare = name.replace("(Premium)", "").replace("(Enhanced)", "").strip()
            if not locale.startswith("en"):
                continue
            (premium_en if is_premium else plain_en).append(bare)
        if premium_en:
            return premium_en[0]
        for preferred in ("Allison", "Samantha"):
            if preferred in plain_en:
                return preferred
        return fallback
```

- [ ] **Step 4: Run the new test to verify it passes**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_macos_tts.py -q`
Expected: PASS (3 passed).

- [ ] **Step 5: Make `speaker.py`'s functions delegate to the backend (DELEGATION-UNTIL-FLIP — do NOT change `Speaker`'s signature or defaults here)**

> **Why:** `Speaker`'s defaults (`say_runner=run_say`, `earcon_player=play_earcon`) and the live `cli.py` call sites (`speaker.best_enhanced_voice()` at cli.py:159 in `doctor()` and cli.py:676 in `install()`) must keep working at every commit, or the daemon goes **mute in production** for the rest of Group B (the suite won't catch it — `make_daemon` injects a `FakeSpeaker`). So in T2 we keep the names + defaults intact and only move the *logic* into the backend. The `Speaker`-signature change + daemon injection + shim deletion happen together, atomically, in Task 8.

In `src/sonari/speaker.py`: keep `run_say` and `best_enhanced_voice` as **thin delegating shims** (replace their bodies; keep the names and the `Speaker.__init__` defaults exactly as they are). Add the backend import at the top and replace the two function bodies:
```python
from sonari.platform.macos.tts import MacTtsBackend  # removed in Task 8's flip
_MAC_TTS = MacTtsBackend()

def run_say(text, voice, rate):
    return _MAC_TTS.run(text, voice, rate)

def best_enhanced_voice() -> str:
    return _MAC_TTS.best_voice()
```
`Speaker.__init__` (still `say_runner=run_say`, `earcon_player=play_earcon`), `Speaker.speak`, and `Speaker.cancel` are **unchanged**. `cli.py:159`/`cli.py:676` and the three `mock.patch("sonari.speaker.best_enhanced_voice", ...)` sites (test_cli_doctor.py:10, test_cli_hotkeyd.py:191 and :223) keep resolving because the shim still exists.

In `tests/test_speaker.py`: the four `best_enhanced_voice` tests (lines 279–313) patch `speaker_mod.subprocess.check_output`, which no longer affects the delegated logic — so **delete lines 279–313 AND the now-dangling top-level import `from sonari.speaker import best_enhanced_voice` at line 254** (their coverage is re-expressed in `tests/test_macos_tts.py`). Keep every `Speaker`-orchestration test (they inject `say_runner=`/`earcon_player=`, unaffected). Leave the three `play_earcon`-direct tests for Task 3 (where `play_earcon` becomes a shim).

- [ ] **Step 6: Run the affected suites to verify green**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_speaker.py tests/test_macos_tts.py -q`
Expected: PASS (test_speaker collects cleanly — the dangling import is gone; the moved tests live in test_macos_tts).

- [ ] **Step 7: Commit**

```bash
git add src/sonari/platform/macos/__init__.py src/sonari/platform/macos/tts.py \
        src/sonari/speaker.py tests/test_macos_tts.py tests/test_speaker.py
git commit -m "refactor(platform): move say/best-voice logic into MacTtsBackend; speaker.py delegates (defaults + signature unchanged)"
```

---

### Task 3: macOS earcon backend (`play_earcon`) + config earcon defaults

**Files:**
- Create: `src/sonari/platform/macos/earcon.py`
- Test: `tests/test_macos_earcon.py`
- Modify: `src/sonari/speaker.py` (`play_earcon` → delegating shim), `tests/test_speaker.py` (remove the `play_earcon`-direct tests). **`config.py` is NOT touched here** — the `DEFAULTS` earcon removal is deferred to Task 8's atomic flip.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_macos_earcon.py
from sonari.platform.macos import earcon as mod
from sonari.platform.macos.earcon import MacEarconBackend


def test_play_invokes_afplay_with_path(monkeypatch):
    seen = {}
    monkeypatch.setattr(mod.os.path, "exists", lambda p: True)
    monkeypatch.setattr(mod.subprocess, "Popen", lambda args: seen.setdefault("args", args))
    MacEarconBackend().play("/x/Funk.aiff")
    assert seen["args"] == ["afplay", "/x/Funk.aiff"]


def test_play_missing_file_is_none(monkeypatch):
    monkeypatch.setattr(mod.os.path, "exists", lambda p: False)
    assert MacEarconBackend().play("/nope.aiff") is None


def test_default_earcons_are_macos_system_sounds():
    d = MacEarconBackend().default_earcons()
    assert d["permission"] == "/System/Library/Sounds/Funk.aiff"
    assert set(d) == {"permission", "choice", "plan", "error", "turn_done", "ready"}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_macos_earcon.py -q`
Expected: FAIL — `ModuleNotFoundError: ... earcon`.

- [ ] **Step 3: Create the earcon backend (move `play_earcon` + own the defaults)**

Create `src/sonari/platform/macos/earcon.py`:
```python
"""macOS earcon backend — wraps `afplay` + the System Sounds defaults."""
from __future__ import annotations

import os
import subprocess

from sonari.platform.base import EarconBackend

_DEFAULTS = {
    "permission": "/System/Library/Sounds/Funk.aiff",
    "choice":     "/System/Library/Sounds/Ping.aiff",
    "plan":       "/System/Library/Sounds/Submarine.aiff",
    "error":      "/System/Library/Sounds/Sosumi.aiff",
    "turn_done":  "/System/Library/Sounds/Tink.aiff",
    "ready":      "/System/Library/Sounds/Glass.aiff",
}


class MacEarconBackend(EarconBackend):
    def play(self, path: str):
        if not os.path.exists(path):
            return None
        try:
            return subprocess.Popen(["afplay", path])
        except (FileNotFoundError, OSError):
            return None

    def default_earcons(self) -> dict:
        return dict(_DEFAULTS)
```

- [ ] **Step 4: Run the new test to verify it passes**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_macos_earcon.py -q`
Expected: PASS (3 passed).

- [ ] **Step 5: Make `play_earcon` delegate (DELEGATION-UNTIL-FLIP — keep `config.DEFAULTS` earcons for now)**

> **Why:** removing the `.aiff` block from `config.DEFAULTS` now (before the daemon backfills from the backend, which lands in Task 8) leaves any user without a persisted `earcons` block with **no earcons in production** for the rest of Group B — another silent invariant-#1 violation the suite won't catch. So `MacEarconBackend.default_earcons()` is created here (additive, harmless) but the `config.DEFAULTS` deletion is deferred to the atomic Task 8 flip.

In `src/sonari/speaker.py`: keep `play_earcon` as a **delegating shim** (replace its body; keep the name and the `earcon_player=play_earcon` default). Reuse the Task 2 backend import block by adding the earcon backend:
```python
from sonari.platform.macos.earcon import MacEarconBackend  # removed in Task 8's flip
_MAC_EARCON = MacEarconBackend()

def play_earcon(path):
    return _MAC_EARCON.play(path)
```
**Do NOT touch `config.py` in this task** — `DEFAULTS["earcons"]` stays as-is.

In `tests/test_speaker.py`: the three `play_earcon`-direct tests (lines 142–176) patch `speaker_mod.os.path`/`speaker_mod.subprocess`, which no longer affect the delegated logic — **delete those three tests** (re-expressed in `tests/test_macos_earcon.py`). Keep the earcon-*orchestration* tests that inject `earcon_player=` (lines 113, 124, 132, 208, 237) — they're unaffected.

- [ ] **Step 6: Run affected suites**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_speaker.py tests/test_macos_earcon.py -q`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/sonari/platform/macos/earcon.py src/sonari/speaker.py \
        tests/test_macos_earcon.py tests/test_speaker.py
git commit -m "refactor(platform): move afplay into MacEarconBackend; speaker.py delegates; config DEFAULTS unchanged"
```

---

### Task 4: macOS key tables (`KEY_CODES` / `MOD_MASKS`)

**Files:**
- Create: `src/sonari/platform/macos/keytables.py`
- Modify: `src/sonari/keymap.py` (import the tables from the backend instead of defining them), `tests/test_cli_hotkeyd.py` (the cross-check test's import target)

- [ ] **Step 1: Write the failing test**

```python
# add to tests/test_macos_tts.py is wrong location; create assertion in test_keymap.py
# tests/test_keymap.py — add:
def test_keytables_live_in_macos_backend():
    from sonari.platform.macos import keytables
    assert keytables.KEY_CODES["o"] == 31
    assert keytables.MOD_MASKS["cmd"] == 256
    # keymap re-exports them for backward compatibility
    import sonari.keymap as keymap
    assert keymap.KEY_CODES is keytables.KEY_CODES
    assert keymap.MOD_MASKS is keytables.MOD_MASKS
```

- [ ] **Step 2: Run it to verify it fails**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_keymap.py::test_keytables_live_in_macos_backend -q`
Expected: FAIL — `ModuleNotFoundError: ... keytables`.

- [ ] **Step 3: Create keytables.py; re-export from keymap.py**

Create `src/sonari/platform/macos/keytables.py` (move keymap.py lines 20–43 verbatim):
```python
"""macOS Carbon key-code + modifier-mask tables (used to resolve the keymap
into the form the Swift hotkeyd reads)."""

KEY_CODES = {
    "s": 1, "r": 15, "d": 2, "l": 37, "v": 9, "o": 31,
    "period": 47, ".": 47,
    "rightbracket": 30, "]": 30,
    "leftbracket": 33, "[": 33,
}

MOD_MASKS = {
    "cmd": 256, "shift": 512,
    "opt": 2048, "option": 2048,
    "ctrl": 4096, "control": 4096,
}
```
In `src/sonari/keymap.py`, replace the inline `KEY_CODES`/`MOD_MASKS` definitions (lines 20–43) with a re-export so `keymap.KEY_CODES` keeps working for every existing consumer:
```python
from sonari.platform.macos.keytables import KEY_CODES, MOD_MASKS
```
(Place this import with the other imports near the top. `resolve_keymap()` already references the module-level `KEY_CODES`/`MOD_MASKS`, which now resolve to the re-exported names — no further change.)

> **The re-export is load-bearing — do NOT delete `keymap.KEY_CODES`/`keymap.MOD_MASKS`.** Pre-existing tests still read them via the `keymap` module: `test_keymap.py:21–34` (`test_key_codes_cover_default_keys`, `test_mod_masks_values`) and `test_cli_hotkeyd.py:256–269` (the display-table cross-check imports `sonari.keymap as _keymap` and compares `_keymap.KEY_CODES.values()`). Keeping the re-export means those tests pass unchanged.

- [ ] **Step 4: Run to verify it passes**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_keymap.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/platform/macos/keytables.py src/sonari/keymap.py tests/test_keymap.py
git commit -m "refactor(platform): move Carbon KEY_CODES/MOD_MASKS into macos/keytables; keymap re-exports"
```

---

### Task 5: macOS hotkey backend (`_build_hotkeyd`, `_hotkeyd_plist`, display tables)

**Files:**
- Create: `src/sonari/platform/macos/hotkeys.py`
- Modify: `src/sonari/cli.py` (delegate `_combo_label`/`_KEYCODE_DISPLAY`/`_MOD_DISPLAY`/`_build_hotkeyd`/`_hotkeyd_plist` to the backend), `tests/test_cli_hotkeyd.py` (patch targets move to `platform.macos.hotkeys`)

> **Note on scope:** to preserve zero behavior change and keep `test_cli_hotkeyd.py`'s existing patch points working with minimal churn, `cli.py` keeps thin module-level wrappers that delegate to the backend (e.g. `_build_hotkeyd = _mac_hotkeys.build`). The *logic* moves; the `cli.`-level names remain so install/uninstall and their tests are undisturbed. The display tables are derived from `keytables` to kill the duplicated Carbon constants.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_macos_hotkeys.py
from sonari.platform.macos.hotkeys import MacHotkeyBackend


def test_display_combo_labels_ctrl_cmd_o():
    hk = MacHotkeyBackend()
    # 4096 (ctrl) | 256 (cmd) == 4352 ; key 31 == 'O'
    assert hk.display_combo(4352, 31) == "Ctrl+Cmd+O"


def test_display_tables_cover_every_keycode_and_modifier():
    from sonari.platform.macos import keytables
    hk = MacHotkeyBackend()
    for code in keytables.KEY_CODES.values():
        assert code in hk._keycode_display
    for mask in keytables.MOD_MASKS.values():
        assert mask in {m for m, _ in hk._mod_display}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_macos_hotkeys.py -q`
Expected: FAIL — module missing.

- [ ] **Step 3: Create the hotkey backend (move build + display logic)**

Create `src/sonari/platform/macos/hotkeys.py`. Move `_build_hotkeyd` (cli.py 541–577) and `_hotkeyd_plist` logic here, and build the display tables from `keytables`:
```python
"""macOS hotkey backend — compiles + supervises the Swift Carbon hotkeyd."""
from __future__ import annotations

import hashlib
import os
import shutil
import subprocess

from sonari import paths
from sonari.platform.base import HotkeyBackend

LAUNCH_AGENT_LABEL = "com.sonari.hotkeyd"
LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.sonari.hotkeyd.plist")

_KEYCODE_DISPLAY = {1: "S", 15: "R", 2: "D", 37: "L", 9: "V", 31: "O",
                    47: ".", 30: "]", 33: "["}
_MOD_DISPLAY = [(4096, "Ctrl"), (256, "Cmd"), (2048, "Opt"), (512, "Shift")]


class MacHotkeyBackend(HotkeyBackend):
    _keycode_display = _KEYCODE_DISPLAY
    _mod_display = _MOD_DISPLAY

    def display_combo(self, modifiers: int, key_code: int) -> str:
        parts = [name for mask, name in self._mod_display if modifiers & mask]
        parts.append(self._keycode_display.get(key_code, "key{0}".format(key_code)))
        return "+".join(parts)

    def build(self):
        """Compile sonari-hotkeyd if swiftc is present and the source changed.
        Returns (ok: bool, detail: str). (Verbatim move of cli._build_hotkeyd.)"""
        if shutil.which("swiftc") is None:
            return (False, "swiftc not found")
        src = os.path.join(paths.repo_root(), "hotkeyd", "sonari-hotkeyd.swift")
        try:
            with open(src, "rb") as fh:
                src_hash = hashlib.sha256(fh.read()).hexdigest()
        except OSError as exc:
            return (False, "cannot read hotkeyd source: {0}".format(exc))
        hash_path = str(paths.SONARI_DIR / ".hotkeyd.srchash")
        if os.path.exists(str(paths.HOTKEYD_BIN_PATH)):
            try:
                with open(hash_path, "r", encoding="utf-8") as fh:
                    if fh.read().strip() == src_hash:
                        return (True, "{0} (unchanged; kept to preserve any "
                                "permission grants)".format(paths.HOTKEYD_BIN_PATH))
            except OSError:
                pass
        rc = subprocess.call(["swiftc", src, "-o", str(paths.HOTKEYD_BIN_PATH)])
        if rc == 0:
            try:
                with open(hash_path, "w", encoding="utf-8") as fh:
                    fh.write(src_hash)
            except OSError:
                pass
            return (True, str(paths.HOTKEYD_BIN_PATH))
        return (False, "swiftc exited {0}".format(rc))

    def install(self):
        return self.build()

    def uninstall(self) -> None:
        try:
            os.remove(str(paths.HOTKEYD_BIN_PATH))
        except OSError:
            pass
```

- [ ] **Step 4: Delegate from cli.py; update the cross-check test**

In `src/sonari/cli.py`, replace the bodies of `_combo_label`, `_build_hotkeyd`, and the display tables with delegations to a module-level backend instance (add near the top: `from sonari.platform.macos.hotkeys import MacHotkeyBackend; _mac_hotkeys = MacHotkeyBackend()`):
```python
_KEYCODE_DISPLAY = _mac_hotkeys._keycode_display
_MOD_DISPLAY = _mac_hotkeys._mod_display

def _combo_label(modifiers: int, key_code: int) -> str:
    return _mac_hotkeys.display_combo(modifiers, key_code)

def _build_hotkeyd():
    return _mac_hotkeys.build()
```
Keep `HOTKEYD_LAUNCH_AGENT_LABEL`/`HOTKEYD_LAUNCH_AGENT_PATH` in cli.py (Task 7 moves install/uninstall wholesale; these constants can re-point to the backend then). In `tests/test_cli_hotkeyd.py`, the cross-check test `test_display_tables_cover_every_keycode_and_modifier` (line 256) still passes because `cli._KEYCODE_DISPLAY`/`cli._MOD_DISPLAY` now alias the backend tables and `keymap.KEY_CODES` re-exports `keytables.KEY_CODES`. The `test_build_hotkeyd_*` tests patch `shutil.which`/`subprocess.call`/`cli.paths.*`; update any that patch `cli.shutil`/`cli.subprocess` to patch `sonari.platform.macos.hotkeys.shutil`/`.subprocess` instead (the build logic now lives there).

- [ ] **Step 5: Run to verify green**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_macos_hotkeys.py tests/test_cli_hotkeyd.py -q`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/sonari/platform/macos/hotkeys.py src/sonari/cli.py \
        tests/test_macos_hotkeys.py tests/test_cli_hotkeyd.py
git commit -m "refactor(platform): move hotkeyd build + display tables into MacHotkeyBackend; cli delegates"
```

---

### Task 6: macOS supervisor backend (python resolution, launchctl/plist, launcher, doctor rows)

**Files:**
- Create: `src/sonari/platform/macos/supervisor.py`
- Modify: `src/sonari/cli.py` (delegate `_resolve_python`/`_PYTHON_CANDIDATE_NAMES`/`_launchctl`/`_plist`/`_launchagent_plist`/`_place_launcher` + the macOS `doctor()` rows to the backend), `src/sonari/daemon.py` (`_launcher_present` → `is_installed`), `tests/test_cli_resolve_python.py`, `tests/test_cli_doctor.py` (patch targets)

> **Note:** This is the largest move. Keep thin `cli.`-level delegating wrappers so install/uninstall (Task 7 leaves them in cli.py for now, calling the backend) and their existing tests' patch points keep working. `_probe_python_version` moves with `_resolve_python`.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_macos_supervisor.py
from sonari.platform.macos.supervisor import MacSupervisorBackend


def test_resolve_python_prefers_usr_bin(monkeypatch):
    sup = MacSupervisorBackend()
    monkeypatch.setattr(sup, "_probe_python_version", lambda c: (3, 11))
    monkeypatch.setattr("sonari.platform.macos.supervisor.shutil.which",
                        lambda n: "/opt/homebrew/bin/python3")
    monkeypatch.setattr("sonari.platform.macos.supervisor.os.path.realpath",
                        lambda p: p)
    assert sup.resolve_python() == "/usr/bin/python3"


def test_launch_spec_uses_start_new_session():
    argv, kwargs = MacSupervisorBackend().launch_spec()
    assert argv[-1].endswith("sonari-daemon")
    assert kwargs.get("start_new_session") is True


def test_doctor_rows_include_say_and_swiftc(monkeypatch):
    monkeypatch.setattr("sonari.platform.macos.supervisor.shutil.which",
                        lambda n: "/usr/bin/" + n)
    names = [r[0] for r in MacSupervisorBackend().doctor_rows()]
    assert "say" in names and "swiftc" in names
```

- [ ] **Step 2: Run it to verify it fails**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_macos_supervisor.py -q`
Expected: FAIL — module missing.

- [ ] **Step 3: Create the supervisor backend**

Create `src/sonari/platform/macos/supervisor.py`. Move `_PYTHON_CANDIDATE_NAMES` (cli.py 292–295), `_probe_python_version` (298–311), `_resolve_python` (314–343), `_launchctl` (580–585), `_plist` (462–513), `_launchagent_plist` (516–530), `_place_launcher` (401–417) verbatim into methods, and provide `launch_spec`, `is_installed`, `is_running`, and `doctor_rows` (the macOS rows). Use `paths` + `repo_root` for the daemon shim path:
```python
"""macOS supervisor backend — launchd/launchctl install + python resolution."""
from __future__ import annotations

import os
import shutil
import subprocess

from sonari import paths
from sonari.platform.base import SupervisorBackend

LAUNCH_AGENT_LABEL = "com.sonari.speechd"
LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.sonari.speechd.plist")
_PYTHON_CANDIDATE_NAMES = (
    "python3", "python3.13", "python3.12", "python3.11", "python3.10", "python3.9",
)


def _launcher_path() -> str:
    return os.path.join(os.path.expanduser("~"), ".local", "bin", "sonari")


class MacSupervisorBackend(SupervisorBackend):
    # --- python resolution (verbatim move of cli._resolve_python et al.) ---
    def _probe_python_version(self, candidate):
        # (verbatim move of cli._probe_python_version, lines 298-311)
        ...

    def resolve_python(self):
        candidates = ["/usr/bin/python3"]
        for name in _PYTHON_CANDIDATE_NAMES:
            found = shutil.which(name)
            if found:
                candidates.append(found)
        seen, qualifying = set(), []
        for cand in candidates:
            real = os.path.realpath(cand)
            if real in seen:
                continue
            seen.add(real)
            ver = self._probe_python_version(cand)
            if ver is not None and ver >= (3, 9):
                qualifying.append((real, cand == "/usr/bin/python3"))
        if not qualifying:
            return None
        for real, was_usr_bin in qualifying:
            if was_usr_bin:
                return real
        return qualifying[0][0]

    # --- launchd helpers (verbatim moves) ---
    def launchctl(self, args):
        try:
            return subprocess.call(["launchctl", *args])
        except FileNotFoundError:
            return 1

    def plist(self, label, program_args, log_path, env=None):
        ...  # verbatim move of cli._plist

    # --- lifecycle ---
    def launch_spec(self):
        shim = os.path.join(paths.repo_root(), "bin", "sonari-daemon")
        return ([shim], {"start_new_session": True,
                         "stdin": subprocess.DEVNULL,
                         "stdout": subprocess.DEVNULL,
                         "stderr": subprocess.DEVNULL})

    def is_installed(self) -> bool:
        return os.path.exists(_launcher_path())

    def is_running(self) -> bool:
        from sonari import paths as _p
        return _p.socket_connectable()

    def install(self, python, app_dir): ...   # filled when Task 7 moves install
    def uninstall(self): ...

    def doctor_rows(self):
        rows = []
        rows.append(("say", shutil.which("say") is not None, shutil.which("say") or "missing"))
        rows.append(("afplay", shutil.which("afplay") is not None, shutil.which("afplay") or "missing"))
        rows.append(("swiftc", shutil.which("swiftc") is not None,
                     shutil.which("swiftc") or "missing (hotkeys unavailable)"))
        # ... the remaining macOS rows: hotkeyd binary, resolved keymap,
        #     speechd/hotkeyd LaunchAgent loaded, sonari launcher (verbatim
        #     move of those checks from cli.doctor()) ...
        return rows
```

> Fill the elided bodies (`_probe_python_version`, `plist`, the remaining doctor rows) by moving the exact code from `cli.py` (lines cited above) and `cli.doctor()` rows 1,2,3,7,8,9,13,14,15. Keep the row *names* and ok/detail tuples byte-identical so `test_cli_doctor.py`'s key list is unchanged.

- [ ] **Step 4: Delegate from cli.py + daemon.py**

In `cli.py` add `from sonari.platform.macos.supervisor import MacSupervisorBackend; _mac_sup = MacSupervisorBackend()` and repoint the wrappers:
```python
_PYTHON_CANDIDATE_NAMES = supervisor._PYTHON_CANDIDATE_NAMES  # via import
def _resolve_python(): return _mac_sup.resolve_python()
def _probe_python_version(c): return _mac_sup._probe_python_version(c)
def _launchctl(args): return _mac_sup.launchctl(args)
def _plist(*a, **k): return _mac_sup.plist(*a, **k)
```
In `cli.doctor()`, replace the macOS rows with `rows.extend(_mac_sup.doctor_rows())` while keeping the neutral rows (SONARI_DIR writable, daemon socket, plugin hooks.json, plugin path resolved, keymap resolves) inline. In `src/sonari/daemon.py`, change `_launcher_present()` (lines 203–208) to consult the supervisor backend. **Use the direct backend import here — `get_platform()` does not exist until Task 8.** Task 8 swaps this to the factory form.
```python
@staticmethod
def _launcher_present() -> bool:
    from sonari.platform.macos.supervisor import MacSupervisorBackend
    return MacSupervisorBackend().is_installed()
```
(The existing tests at `tests/test_daemon_setup_health.py:19–73` monkeypatch the `_launcher_present` static method wholesale, so they stay green regardless of its body — no test churn here.)

Update `tests/test_cli_resolve_python.py` to patch `sonari.platform.macos.supervisor` symbols (or keep patching `cli._probe_python_version` — the wrapper still exists, so these tests pass unchanged). Update `tests/test_cli_doctor.py` `_ok_patches()` to patch `sonari.platform.macos.supervisor.shutil.which` for say/afplay/swiftc and `..supervisor`'s launchctl; the asserted key list is unchanged because row names didn't change.

- [ ] **Step 5: Run to verify green**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_macos_supervisor.py tests/test_cli_resolve_python.py tests/test_cli_doctor.py -q`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/sonari/platform/macos/supervisor.py src/sonari/cli.py src/sonari/daemon.py \
        tests/test_macos_supervisor.py tests/test_cli_resolve_python.py tests/test_cli_doctor.py
git commit -m "refactor(platform): move python-resolution/launchd/doctor-rows into MacSupervisorBackend; cli+daemon delegate"
```

---

### Task 7: Assemble `MacPlatformBackend`

**Files:**
- Modify: `src/sonari/platform/macos/__init__.py`
- Test: extend `tests/test_macos_supervisor.py` (or a new `tests/test_macos_backend.py`)

- [ ] **Step 1: Write the failing test**

```python
# tests/test_macos_backend.py
from sonari.platform.macos import make_backend
from sonari.platform import base


def test_make_backend_returns_full_bundle():
    pb = make_backend()
    assert isinstance(pb, base.PlatformBackend)
    assert isinstance(pb.tts, base.TtsBackend)
    assert isinstance(pb.earcon, base.EarconBackend)
    assert isinstance(pb.hotkey, base.HotkeyBackend)
    assert isinstance(pb.supervisor, base.SupervisorBackend)
```

- [ ] **Step 2: Run it to verify it fails** — `ImportError: cannot import name 'make_backend'`.

- [ ] **Step 3: Implement the assembler**

In `src/sonari/platform/macos/__init__.py`:
```python
from sonari.platform.base import PlatformBackend
from sonari.platform.macos.tts import MacTtsBackend
from sonari.platform.macos.earcon import MacEarconBackend
from sonari.platform.macos.hotkeys import MacHotkeyBackend
from sonari.platform.macos.supervisor import MacSupervisorBackend


def make_backend() -> PlatformBackend:
    return PlatformBackend(
        tts=MacTtsBackend(),
        earcon=MacEarconBackend(),
        hotkey=MacHotkeyBackend(),
        supervisor=MacSupervisorBackend(),
    )
```

- [ ] **Step 4: Run to verify it passes.**

- [ ] **Step 5: Commit**

```bash
git add src/sonari/platform/macos/__init__.py tests/test_macos_backend.py
git commit -m "feat(platform): assemble MacPlatformBackend"
```

---

### Task 8: The `get_platform()` factory + THE ATOMIC FLIP

> **This is the single commit where the delegation-until-flip lands.** Everything that would have broken production or tests if done piecemeal in T2/T3 happens here, together: the `Speaker` signature change, the daemon's backend injection, the earcon backfill + `config.DEFAULTS` deletion, the `cli` best-voice repoint, the shim deletion, and the matching test-patch repoints. Before this commit the suite is green AND production is byte-identical; after it, the seam is live. Run the **full dual-interpreter gate** at the end (not a narrow suite) — these changes touch the whole composition root.

**Files:**
- Modify: `src/sonari/platform/__init__.py` (the factory), `src/sonari/speaker.py` (signature → injected, delete shims), `src/sonari/config.py` (drop `DEFAULTS["earcons"]`), `src/sonari/daemon.py` (inject + backfill + `_launcher_present` via factory), `src/sonari/cli.py` (best-voice via backend)
- Test: `tests/test_platform_factory.py` (new), `tests/test_cli_doctor.py` + `tests/test_cli_hotkeyd.py` (repoint the `best_enhanced_voice` patch target), `tests/test_config.py` (if it asserts `DEFAULTS["earcons"]`)

- [ ] **Step 1: Write the failing test**

```python
# tests/test_platform_factory.py
import sonari.platform as platform
from sonari.platform import base


def test_get_platform_returns_macos_backend_on_darwin(monkeypatch):
    monkeypatch.setattr(platform.sys, "platform", "darwin")
    platform._CACHE = None
    pb = platform.get_platform()
    assert isinstance(pb, base.PlatformBackend)


def test_get_platform_rejects_unknown_os(monkeypatch):
    monkeypatch.setattr(platform.sys, "platform", "sunos5")
    platform._CACHE = None
    import pytest
    with pytest.raises(RuntimeError):
        platform.get_platform()
```

- [ ] **Step 2: Run it to verify it fails.**

- [ ] **Step 3: Implement the factory (the ONLY sys.platform branch)**

`src/sonari/platform/__init__.py`:
```python
"""get_platform() — the single OS dispatch point for Sonari."""
from __future__ import annotations

import sys

from sonari.platform.base import PlatformBackend

_CACHE = None


def get_platform() -> PlatformBackend:
    global _CACHE
    if _CACHE is not None:
        return _CACHE
    if sys.platform == "darwin":
        from sonari.platform.macos import make_backend
    elif sys.platform == "win32":
        raise RuntimeError("Windows backend lands in Milestone 2.")
    else:
        raise RuntimeError("Unsupported platform: {0}".format(sys.platform))
    _CACHE = make_backend()
    return _CACHE
```

- [ ] **Step 4: The atomic flip (all of the following in ONE commit)**

**(a) `src/sonari/speaker.py` — make `Speaker` truly portable; delete the shims.** Change the defaults to `None` and add the no-op guards, then **delete** the Task 2/3 delegating shims (`run_say`, `best_enhanced_voice`, `play_earcon`) and their `from sonari.platform.macos...` imports so `speaker.py` no longer imports any backend:
```python
class Speaker:
    def __init__(self, voice=None, rate=200, say_runner=None,
                 earcon_player=None, earcons=None,
                 _wait_timeout: float = _DEFAULT_WAIT_TIMEOUT) -> None:
        self._voice = voice
        self._rate = rate
        self._say_runner = say_runner
        self._earcon_player = earcon_player
        self._earcons = dict(earcons) if earcons else {}
        self._current = None
        self._current_lock = threading.Lock()
        self._earcon_procs: list = []
        self._wait_timeout = _wait_timeout

    def speak(self, text: str) -> bool:
        if self._say_runner is None:
            return False
        proc = self._say_runner(text, self._voice, self._rate)
        # ... rest of the existing body unchanged ...
```
In the earcon-playing method, add `if self._earcon_player is None: return` at the top (before it calls `self._earcon_player(...)`).

**(b) `src/sonari/config.py` — drop the `.aiff` defaults** (now backfilled from the backend). New `DEFAULTS`:
```python
DEFAULTS = {
    "voice": None, "rate": 200, "verbosity": "everything",
    "background_policy": "earcon_only", "history_cap": 200,
}
```

**(c) `src/sonari/daemon.py` — inject the backend + backfill earcons + factory-ify `_launcher_present`.** At the `Speaker(...)` site (line 612):
```python
from sonari.platform import get_platform
_backend = get_platform()
cfg = load_config()
if "earcons" not in cfg:
    cfg["earcons"] = _backend.earcon.default_earcons()
speaker = Speaker(
    voice=cfg.get("voice"),
    rate=cfg.get("rate", 200),
    say_runner=_backend.tts.run,
    earcon_player=_backend.earcon.play,
    earcons=cfg.get("earcons"),
)
```
And change `_launcher_present` (set to the direct-import form in Task 6) to the factory form: `from sonari.platform import get_platform; return get_platform().supervisor.is_installed()`.

**(d) `src/sonari/cli.py` — best-voice via the backend.** Repoint the two `speaker.best_enhanced_voice()` call sites — cli.py:159 (the `doctor()` "enhanced voice" row) and cli.py:676 (the `install()` voice check) — to `get_platform().tts.best_voice()` (add `from sonari.platform import get_platform`). Remove the now-unused `from . import speaker` there if nothing else needs it.

**(e) Repoint the `best_enhanced_voice` test patches.** In `tests/test_cli_doctor.py:10` (`_ok_patches`) and `tests/test_cli_hotkeyd.py:191` + `:223`, change `mock.patch("sonari.speaker.best_enhanced_voice", return_value=...)` to patch the symbol cli now invokes: `mock.patch("sonari.platform.macos.tts.MacTtsBackend.best_voice", return_value="Ava")`. If `tests/test_config.py` asserts `DEFAULTS["earcons"]`, change it to assert `"earcons" not in DEFAULTS` (grep `tests/test_config.py` for `earcons` first; if absent, no change).

- [ ] **Step 5: Run the FULL suite on both interpreters**

Run:
```bash
TMPDIR=/tmp /usr/bin/python3 -m pytest -q
TMPDIR=/tmp python3.13 -m pytest -q
```
Expected: PASS on both. This is the gate that catches anything the narrow per-task commands in T2–T7 could not. Investigate every red before committing.

- [ ] **Step 6: Commit**

```bash
git add src/sonari/platform/__init__.py src/sonari/speaker.py src/sonari/config.py \
        src/sonari/daemon.py src/sonari/cli.py tests/test_platform_factory.py \
        tests/test_cli_doctor.py tests/test_cli_hotkeyd.py tests/test_config.py
git commit -m "feat(platform): get_platform() factory + atomic flip — Speaker takes injected backend callables; daemon injects the macOS backend; remove shims + .aiff DEFAULTS; cli best-voice via backend (the only sys.platform branch)"
```

---

# GROUP C — Migrate IPC from AF_UNIX to localhost TCP

> After this group, `~/.sonari/speechd.sock` is gone; `~/.sonari/daemon.lock` (JSON: host/port/token/pid, mode 0o600) replaces it. The wire protocol (`protocol.encode`/`decode`) is unchanged. A mandatory token gate replaces the filesystem permission the AF_UNIX socket provided.

### Task 9: Shared `transport.py` (lockfile + token + connect/serve helpers)

**Files:**
- Create: `src/sonari/platform/transport.py`
- Modify: `src/sonari/paths.py` (`SOCKET_PATH` → `LOCK_PATH`; `socket_connectable` delegates to transport)
- Test: `tests/test_transport.py`, `tests/test_paths.py` (the `.name`/`socket_connectable` assertions)

- [ ] **Step 1: Write the failing test**

```python
# tests/test_transport.py
import json, socket, threading
from sonari.platform import transport


def test_write_then_read_lockfile_roundtrips(tmp_path):
    lock = tmp_path / "daemon.lock"
    transport.write_lockfile(lock, "127.0.0.1", 54321, "deadbeef", 4242)
    info = transport.read_lockfile(lock)
    assert info == {"host": "127.0.0.1", "port": 54321,
                    "token": "deadbeef", "pid": 4242}
    assert oct(lock.stat().st_mode)[-3:] == "600"


def test_read_lockfile_missing_returns_none(tmp_path):
    assert transport.read_lockfile(tmp_path / "absent.lock") is None


def test_connectable_true_against_a_live_listener(tmp_path):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    port = srv.getsockname()[1]
    lock = tmp_path / "daemon.lock"
    transport.write_lockfile(lock, "127.0.0.1", port, "tok", 999999)
    # PID 999999 is unlikely-live; connectable must NOT depend on PID when the
    # socket actually accepts — it returns True because connect() succeeds.
    t = threading.Thread(target=lambda: srv.accept(), daemon=True)
    t.start()
    assert transport.connectable(lock) is True
    srv.close()


def test_connectable_false_when_lockfile_absent(tmp_path):
    assert transport.connectable(tmp_path / "absent.lock") is False
```

- [ ] **Step 2: Run it to verify it fails** — module missing.

- [ ] **Step 3: Implement the transport**

Create `src/sonari/platform/transport.py`:
```python
"""Shared localhost-TCP transport for the Sonari daemon <-> clients.

A lockfile (JSON: host/port/token/pid, mode 0o600) advertises the daemon's
ephemeral port + a 256-bit token. Loopback TCP has no filesystem ACL, so the
token is MANDATORY: a connection must send the token as its first line before
any message is processed."""
from __future__ import annotations

import json
import os
import socket

HOST = "127.0.0.1"


def make_token() -> str:
    import secrets
    return secrets.token_hex(32)  # 256-bit


def write_lockfile(path, host, port, token, pid) -> None:
    data = {"host": host, "port": int(port), "token": token, "pid": int(pid)}
    tmp = str(path) + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.chmod(tmp, 0o600)
    os.replace(tmp, str(path))


def read_lockfile(path):
    try:
        with open(str(path), "r", encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def connect(path, timeout=2.0):
    """Return a connected, authenticated socket, or raise OSError."""
    info = read_lockfile(path)
    if not info:
        raise OSError("daemon lockfile missing")
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect((info["host"], info["port"]))
    s.sendall((info["token"] + "\n").encode("utf-8"))   # token handshake first
    return s


def connectable(path) -> bool:
    try:
        s = connect(path, timeout=1.0)
    except OSError:
        return False
    try:
        s.close()
    except OSError:
        pass
    return True
```

In `src/sonari/paths.py`: replace `SOCKET_PATH = SONARI_DIR / "speechd.sock"` with `LOCK_PATH = SONARI_DIR / "daemon.lock"`, and rewrite `socket_connectable()` to delegate:
```python
def socket_connectable() -> bool:
    """Return True if the daemon is accepting connections (TCP lockfile)."""
    from sonari.platform import transport
    return transport.connectable(LOCK_PATH)
```
In `tests/test_paths.py`: change the `.name == "speechd.sock"` assertion to `.name == "daemon.lock"` and `.parent` likewise; rewrite the two `socket_connectable` mock tests to patch `sonari.platform.transport.connectable` (true/false) instead of the raw `socket.socket`.

- [ ] **Step 4: Run to verify green**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_transport.py tests/test_paths.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/platform/transport.py src/sonari/paths.py tests/test_transport.py tests/test_paths.py
git commit -m "feat(transport): shared localhost-TCP lockfile+token transport; paths uses LOCK_PATH"
```

---

### Task 10: Daemon binds TCP + token-gates connections; client connects via transport

**Files:**
- Modify: `src/sonari/daemon.py` (bind/accept/run, `_handle_conn` token gate, `ensure_running` via `launch_spec`, single-instance guard), `src/sonari/client.py` (`send` via transport)
- Test: `tests/test_daemon_loop.py`, `tests/test_client_send.py`, `tests/conftest.py`, `tests/test_cli_control.py`

- [ ] **Step 1: Rewrite the daemon transport tests (AF_INET + token)**

In `tests/test_daemon_loop.py`, change `_make_unix_socket_daemon` and the ping/status round-trips to AF_INET + token-first:
```python
def _make_inet_daemon(tmp_path):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(4)
    host, port = srv.getsockname()
    daemon._server = srv
    daemon._token = "testtoken"          # daemon checks this as the first line
    daemon._running.set()
    # ... start speak/accept threads as before ...
    return daemon, (host, port), [speak_t, accept_t], speaker

# client side of each round-trip:
client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
client.connect((host, port))
client.sendall(b"testtoken\n")           # token handshake first
client.sendall(encode({"type": "ping"}))
```

- [ ] **Step 2: Run to verify they fail** (daemon has no `_token`, doesn't gate). Expected: FAIL.

- [ ] **Step 3: Implement TCP bind + token gate in daemon.py**

In `src/sonari/daemon.py` `run()` (lines 544–579):
```python
import secrets
from sonari.paths import LOCK_PATH   # replace SOCKET_PATH import
from sonari.platform import transport

def run(self) -> None:
    ensure_sonari_dir()
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((transport.HOST, 0))
    srv.listen(16)
    port = srv.getsockname()[1]
    self._token = secrets.token_hex(32)
    transport.write_lockfile(LOCK_PATH, transport.HOST, port, self._token, os.getpid())
    self._server = srv
    self._running.set()
    # ... unchanged speak/accept thread startup ...
    finally:
        self.stop()
        try: srv.close()
        except OSError: pass
        try: os.unlink(LOCK_PATH)
        except FileNotFoundError: pass
```
In `_handle_conn` (lines 503–532), read and validate the token as the first newline-delimited line before entering the message loop:
```python
def _handle_conn(self, conn) -> None:
    try:
        buf = b""
        with conn:
            conn.settimeout(5.0)
            # --- token handshake ---
            while b"\n" not in buf:
                data = conn.recv(4096)
                if not data:
                    return
                buf += data
            token_line, buf = buf.split(b"\n", 1)
            if token_line.decode("utf-8", "replace") != getattr(self, "_token", None):
                return  # reject unauthenticated peer
            # --- existing message loop continues, starting with leftover buf ---
            while self._running.is_set():
                ...
```
Replace `ensure_running()` (lines 586–596) to use the supervisor's `launch_spec`:
```python
def ensure_running() -> None:
    if socket_connectable():
        return
    from sonari.platform import get_platform
    argv, kwargs = get_platform().supervisor.launch_spec()
    subprocess.Popen(argv, **kwargs)
```
The `main()` single-instance guard already calls `socket_connectable()` — now TCP-aware, no change.

- [ ] **Step 4: Rewrite `client.send` to use the transport**

In `src/sonari/client.py`, replace the AF_UNIX block in `send()` with:
```python
from sonari.paths import LOCK_PATH
from sonari.platform import transport

def send(msg: dict, expect_reply: bool = False, timeout: float = 2.0):
    try:
        s = transport.connect(LOCK_PATH, timeout=timeout)
    except OSError as exc:
        raise DaemonNotRunning("Sonari daemon is not running. Run: sonari install") from exc
    try:
        s.sendall(encode(msg))
        if not expect_reply:
            return None
        buf = b""
        while b"\n" not in buf:
            data = s.recv(4096)
            if not data:
                break
            buf += data
        return decode(buf.split(b"\n", 1)[0]) if buf else None
    finally:
        try: s.close()
        except OSError: pass
```
(`transport.connect` already sends the token first.)

- [ ] **Step 5: Update conftest.py + test_client_send.py**

In `tests/conftest.py`, replace the `SOCKET_PATH` monkeypatch (line 38) with **two** repoints (keep the `APP_DIR` repoint at line 36): `monkeypatch.setattr(paths, "LOCK_PATH", sonari_dir / "daemon.lock", raising=False)` **and** `import sonari.client as client_mod; monkeypatch.setattr(client_mod, "LOCK_PATH", sonari_dir / "daemon.lock", raising=False)`. The second is essential: `client.send` does `from sonari.paths import LOCK_PATH` (a by-value bind), so an isolated test that calls the real `send` would otherwise read the developer's **real** `~/.sonari/daemon.lock` — exactly why `test_client_send.py` already patches both `SOCKET_PATH` names today. (Also confirm no other module bound `LOCK_PATH` by value that an isolated test exercises.)

In `tests/test_cli_control.py`, update `test_client_send_raises_daemon_not_running_on_connection_refused` (line ~159): change the `monkeypatch.setattr(client_mod, "SOCKET_PATH", ...)` at line 166 to `monkeypatch.setattr(client_mod, "LOCK_PATH", tmp_path / "daemon.lock", raising=False)` pointing at a **nonexistent** lockfile, so `transport.connect` raises `OSError` ("lockfile missing") and `send` deterministically raises `DaemonNotRunning`.

In `tests/test_client_send.py`, change `_reply_server` to bind AF_INET, capture its port, and write a lockfile with a known token; have the server read+discard the token line before the payload:
```python
def _reply_server(lock_path, ready, captured, token="tok"):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    from sonari.platform import transport
    transport.write_lockfile(lock_path, "127.0.0.1", srv.getsockname()[1], token, 1)
    ready.set()
    conn, _ = srv.accept()
    data = conn.recv(65536)
    # strip the token handshake line, capture the rest
    _, _, rest = data.partition(b"\n")
    captured["msg"] = rest
    ...
```
and monkeypatch `paths.LOCK_PATH` + `client_mod.LOCK_PATH` to `tmp_path / "daemon.lock"`.

- [ ] **Step 6: Run the IPC suites + full suite (both interpreters)**

Run:
```bash
TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_daemon_loop.py tests/test_client_send.py tests/test_paths.py -q
TMPDIR=/tmp /usr/bin/python3 -m pytest -q
TMPDIR=/tmp python3.13 -m pytest -q
```
Expected: PASS on both interpreters.

- [ ] **Step 7: Commit**

```bash
git add src/sonari/daemon.py src/sonari/client.py tests/test_daemon_loop.py \
        tests/test_client_send.py tests/conftest.py
git commit -m "feat(transport): daemon binds 127.0.0.1 + token-gates conns; client connects via transport"
```

---

### Task 11: Point the Swift hotkeyd at TCP; update uninstall artifact list

**Files:**
- Modify: `hotkeyd/sonari-hotkeyd.swift` (read the lockfile, connect AF_INET, send token+message), `src/sonari/cli.py` uninstall (remove `LOCK_PATH` instead of `SOCKET_PATH`)
- Test: `tests/test_hotkeyd_swift.py` (compile gate), `tests/test_cli_uninstall.py` (artifact list)

- [ ] **Step 1: Rewrite the Swift `sendMessage` for TCP + lockfile**

In `hotkeyd/sonari-hotkeyd.swift`: delete `socketPath()` (lines 34–36). Add a tiny lockfile reader and rewrite `sendMessage` (lines 66–94) to read `~/.sonari/daemon.lock`, parse `host`/`port`/`token`, connect `AF_INET` to `127.0.0.1:port`, and write `token + "\n" + message + "\n"`:
```swift
func lockInfo() -> (port: UInt16, token: String)? {
    let path = (sonariDir() as NSString).appendingPathComponent("daemon.lock")
    guard let data = FileManager.default.contents(atPath: path),
          let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let port = obj["port"] as? Int,
          let token = obj["token"] as? String else { return nil }
    return (UInt16(port), token)
}

func sendMessage(_ message: String) {
    guard let info = lockInfo() else { return }
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    if fd < 0 { return }
    defer { close(fd) }
    var addr = sockaddr_in()
    addr.sin_family = sa_family_t(AF_INET)
    addr.sin_port = info.port.bigEndian
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr)
    let connected = withUnsafePointer(to: &addr) { aptr -> Int32 in
        aptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sptr in
            connect(fd, sptr, socklen_t(MemoryLayout<sockaddr_in>.size))
        }
    }
    if connected != 0 { return }
    let line = info.token + "\n" + message + "\n"
    _ = line.withCString { write(fd, $0, strlen($0)) }
}
```

- [ ] **Step 2: Verify it still compiles (the existing gate)**

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_hotkeyd_swift.py -q`
Expected: PASS (`test_swift_source_compiles` runs `swiftc`; if `swiftc` is absent it skips). If it fails to compile, fix the Swift until `swiftc hotkeyd/sonari-hotkeyd.swift -o /tmp/hk` exits 0 with no warnings.

- [ ] **Step 3: Update the uninstall artifact list**

In `src/sonari/cli.py` `uninstall()` (the artifact list ~line 732), replace `paths.SOCKET_PATH` with `paths.LOCK_PATH`. In `tests/test_cli_uninstall.py`, change the `cli.paths.SOCKET_PATH` monkeypatch + the removed-artifact assertion to `cli.paths.LOCK_PATH` / `daemon.lock`.

- [ ] **Step 4: Run uninstall + swift suites + full suite**

Run:
```bash
TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_cli_uninstall.py tests/test_hotkeyd_swift.py -q
TMPDIR=/tmp /usr/bin/python3 -m pytest -q
TMPDIR=/tmp python3.13 -m pytest -q
```
Expected: PASS on both.

- [ ] **Step 5: Commit**

```bash
git add hotkeyd/sonari-hotkeyd.swift src/sonari/cli.py tests/test_cli_uninstall.py
git commit -m "feat(transport): Swift hotkeyd reads the lockfile and sends token+message over TCP; uninstall drops the lockfile"
```

---

# GROUP D — Guard the invariant + live verification

### Task 12: Enforce "no OS branch in core" + final dual-interpreter gate + live ear-check

**Files:**
- Test: `tests/test_no_os_branch_in_core.py`

- [ ] **Step 1: Write the guard test**

```python
# tests/test_no_os_branch_in_core.py
import pathlib

CORE = ["assembler.py", "cleaner.py", "queue.py", "history.py", "sessions.py",
        "protocol.py", "hooks_entry.py", "speaker.py", "keymap.py"]
SRC = pathlib.Path(__file__).resolve().parents[1] / "src" / "sonari"


def test_core_modules_have_no_sys_platform_branch():
    for name in CORE:
        text = (SRC / name).read_text(encoding="utf-8")
        assert "sys.platform" not in text, f"{name} branches on sys.platform"


def test_core_modules_do_not_import_macos_backend():
    for name in CORE:
        text = (SRC / name).read_text(encoding="utf-8")
        assert "platform.macos" not in text, f"{name} imports a concrete backend"


def test_only_factory_branches_on_platform():
    factory = (SRC / "platform" / "__init__.py").read_text(encoding="utf-8")
    assert "sys.platform" in factory  # the one allowed branch
```

> Note: `keymap.py` imports `KEY_CODES` from `platform.macos.keytables` (Task 4) — that's a concrete-backend import in a "core" module, which this test would flag. Resolve by having `keymap.py` import the tables via the factory (`get_platform()` does not expose keytables, so instead) — simpler: move the `keytables` import OUT of `keymap.py` and have `resolve_keymap()` accept the tables as parameters injected by the macOS hotkey backend's resolver, OR drop `keymap.py` from the CORE list and document it as a macOS-coupled module pending the Windows keytables in M3. **Decision: drop `keymap.py` from CORE for M1** (it legitimately needs platform keytables; M3 adds the Windows table + a resolver injection). Update the CORE list to exclude `keymap.py` and add a comment.

- [ ] **Step 2: Run it** — adjust the CORE list per the note until green.

Run: `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_no_os_branch_in_core.py -q`
Expected: PASS.

- [ ] **Step 3: Full dual-interpreter gate**

Run:
```bash
TMPDIR=/tmp /usr/bin/python3 -m pytest -q
TMPDIR=/tmp python3.13 -m pytest -q
```
Expected: PASS on both, count ≥ the original 362 (new tests added, the moved tests relocated, none net-lost).

- [ ] **Step 4: Commit**

```bash
git add tests/test_no_os_branch_in_core.py
git commit -m "test(platform): enforce no sys.platform branch / no macos import in the portable core"
```

- [ ] **Step 5: Live ear-check on macOS (the zero-behavior-change proof)**

```bash
cd ~/projects/private/claude-tts && ./bin/sonari install   # rebuilds app dir + hotkeyd + reloads agents
sonari doctor                                               # all green
```
Then in a real `claude` session, confirm by ear (escalate to Nima — this is the ⚠ human listen-test): prose narrates, an earcon plays, `Ctrl+Cmd+O` re-reads options, `Ctrl+Cmd+S` stops. Behavior must be **indistinguishable** from v0.5.0. If anything regressed, do NOT proceed — fix forward.

---

## Self-Review checklist (run before handing off)

- **Spec coverage (§2 of the design):** seam package ✅(T1,7,8), four ABCs ✅(T1), `TtsBackend` logic ✅(T2), `EarconBackend` ✅(T3), keytables ✅(T4), `HotkeyBackend` ✅(T5), `SupervisorBackend` + doctor rows + `is_installed` ✅(T6), `get_platform()` single branch + Speaker injection + earcon backfill ✅(T8 flip), `transport.py` TCP + mandatory token ✅(T9,T10,T11), Swift hotkeyd → TCP ✅(T11), `ensure_running` via `launch_spec` ✅(T10), `_launcher_present`→`is_installed` ✅(T6 direct, T8 factory). **Out of M1 scope (correctly deferred):** the Windows backends, `bin/*` shim Windows equivalents, `_resolve_python` Windows split — those are M2.
- **Zero-behavior-change invariant — DELEGATION-UNTIL-FLIP (the key correctness mechanism, post-review):** T2–T7 only *add* backends + make `speaker.py`/`cli.py` *delegate* to them; `Speaker`'s signature/defaults, `config.DEFAULTS`, and every live call site stay byte-identical, so each intermediate commit is genuinely behavior-preserving (narrow per-task test commands are therefore safe). **All the breaking cleanups** — `Speaker` defaults→None, daemon injection, earcon backfill + `.aiff` removal, `cli` best-voice repoint, shim deletion, test-patch repoints — land **together in the single T8 flip**, gated by the **full dual-interpreter suite**. T11/T12 re-run the full gate; T12 adds the live ear-check. ✅
- **No placeholders:** the only elided bodies are explicit "verbatim move of cli.<fn> lines N–M" with the source lines cited (T6 `_probe_python_version`, `plist`, remaining doctor rows) — the executor copies exact existing code. Acceptable per "repeat the code" only-where-it's-a-pure-move.
- **Type/name consistency:** backend method names are consistent across T1 (ABCs) and T2–T8 (impls): `TtsBackend.run/best_voice/list_voices`, `EarconBackend.play/default_earcons`, `HotkeyBackend.install/uninstall/display_combo`, `SupervisorBackend.install/uninstall/is_running/is_installed/resolve_python/launch_spec/doctor_rows`. Transport API consistent across T9–T11: `make_token/write_lockfile/read_lockfile/connect/connectable`, `LOCK_PATH`, `daemon._token`.

---

## Execution Handoff

Two execution options:
1. **Subagent-Driven (recommended)** — fresh subagent per task, two-stage review (spec compliance then code quality) between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session with checkpoints.
