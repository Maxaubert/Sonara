# Windows Install via the Platform Seam — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `sonari install`/`uninstall`/`doctor` dispatch through `get_platform()` so they perform correct **Windows** setup (Task Scheduler autostart, exec-form hooks merged into `~/.claude/settings.json`, a `sonari.cmd` launcher, real `pythonw` resolution, a clean voice name) with **no macOS artifacts**, while macOS behavior is byte-for-byte preserved.

**Architecture:** Thin-cli / fat-backend. `cli.py` owns the shared install lifecycle (ensure dir, resolve python via the backend, copy app, keymap, install record, voice check) and dispatches every OS-specific step (autostart, hooks, launcher, hotkeys, doctor rows, combo labels, next-steps) to the platform backend. The macOS install/uninstall body moves out of `cli.py` into `MacSupervisorBackend`; the Windows path extends `WinSupervisorBackend`. See `docs/superpowers/specs/2026-06-16-windows-install-seam-design.md`.

**Tech Stack:** Python 3.9 core (`from __future__ import annotations`); existing `platform/` seam + the four ABCs; the `tests/_winfakes.py` sys.modules harness for Windows-only imports; pytest. Windows-only stdlib (`winreg`, `ctypes`, `subprocess schtasks`) stays lazily imported.

**Branch:** `feat/windows-install-seam` (already created; the design spec is committed there).

---

## Invariants (hold at EVERY commit)

1. **macOS behavior unchanged.** The full suite stays green on macOS, and `sonari install`/`uninstall`/`doctor` produce identical artifacts + identical stdout on macOS. Run after every task:
   ```bash
   cd ~/projects/private/claude-tts   # or the repo root on this machine
   TMPDIR=/tmp /usr/bin/python3 -m pytest -q
   ```
2. **`cli.py` ends OS-agnostic.** After Task 6 there is no `import sonari.platform.macos...` and no `sys.platform` branch in `cli.py`; every OS-specific call goes through `get_platform()`.
3. **Windows-only imports stay lazy** (inside methods / guarded), so the macOS suite imports `platform/windows/*` only through the `_winfakes` harness.
4. **No new pip dependency** on the macOS/core import path.

---

## File Structure

```
src/sonari/platform/base.py              # +2 concrete SupervisorBackend methods (defaults)
src/sonari/platform/windows/tts.py       # best_voice() -> str; new _best_voice_info()
src/sonari/platform/macos/supervisor.py  # fill install()/uninstall(); + post_install_notes/hooks_doctor_row
src/sonari/platform/macos/hotkeys.py     # uninstall() also tears down the hotkeyd LaunchAgent
src/sonari/platform/windows/supervisor.py# settings.json hook-merge helpers; extend install()/uninstall(); +notes/hooks_doctor_row
src/sonari/cli.py                        # OS-agnostic install/uninstall/doctor/_combo_label; drop macOS imports/shims
tests/test_win_tts.py                    # split best_voice/_best_voice_info
tests/test_win_settings_hooks.py         # NEW — settings.json merge/remove
tests/test_win_supervisor.py             # extend: install merges hooks + writes launcher; uninstall reverses
tests/test_cli_install.py / *_uninstall  # repoint to backend dispatch (keep macOS output assertions)
tests/test_cli_doctor.py                 # rows come from the platform supervisor; OS-aware hooks row
docs/superpowers/M2-WINDOWS-ACCEPTANCE.md# extend §1/§7 with the settings.json + launcher checks
```

---

## Task 1: Voice contract fix — `WinTtsBackend.best_voice() -> str`

**Files:**
- Modify: `src/sonari/platform/windows/tts.py:96-136`
- Test: `tests/test_win_tts.py`

- [ ] **Step 1: Update the failing test.** Replace `test_list_and_best_voice` so it asserts the ABC string contract, and add an internal-object test:

```python
def test_best_voice_returns_display_name_string():
    b = WinTtsBackend()
    v = b.best_voice()
    assert isinstance(v, str) and v          # ABC: best_voice() -> str
    assert v == "FakeVoice"                   # the fake voice's display_name

def test_best_voice_info_returns_object_with_id():
    b = WinTtsBackend()
    info = b._best_voice_info()
    assert "speech_onecore" in (info.id or "").lower()
```

- [ ] **Step 2: Run → FAIL.** `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_win_tts.py -q` (old `best_voice().id` test gone; new ones error: no `_best_voice_info`).

- [ ] **Step 3: Split the method.** Rename the current `best_voice` body to `_best_voice_info`, and make `best_voice()` return its `display_name`. Update `_resolve_voice` to call `_best_voice_info()`:

```python
    def _best_voice_info(self, lang_prefix: str = "en-US"):
        """Select a VoiceInformation in priority order:
          1. en-US OneCore (Id contains 'Speech_OneCore'); 2. any en-US; 3. default_voice.
        Raises RuntimeError if no voices are installed at all."""
        from winrt.windows.media.speechsynthesis import SpeechSynthesizer
        voices = self.list_voices()
        if not voices:
            raise RuntimeError(
                "No TTS voices installed. Add a Speech language pack in "
                "Settings -> Time & language -> Speech -> Add voices."
            )
        ll = lang_prefix.lower()

        def _is_onecore(v) -> bool:
            return "speech_onecore" in (v.id or "").lower()

        for v in voices:
            if v.language.lower().startswith(ll) and _is_onecore(v):
                return v
        for v in voices:
            if v.language.lower().startswith(ll):
                return v
        return SpeechSynthesizer.default_voice

    def best_voice(self, lang_prefix: str = "en-US") -> str:
        """ABC contract: return the best installed voice's display NAME (str)."""
        return self._best_voice_info(lang_prefix).display_name
```

In `_resolve_voice`, change the fallback `return self.best_voice()` to `return self._best_voice_info()`.

- [ ] **Step 4: Run → PASS**, then full suite. `TMPDIR=/tmp /usr/bin/python3 -m pytest tests/test_win_tts.py -q && TMPDIR=/tmp /usr/bin/python3 -m pytest -q`

- [ ] **Step 5: Commit**
```bash
git add src/sonari/platform/windows/tts.py tests/test_win_tts.py
git commit -m "fix(windows): WinTtsBackend.best_voice() returns a display-name str per the ABC (internal _best_voice_info keeps the object)"
```

---

## Task 2: ABC additions — `post_install_notes()` + `hooks_doctor_row()`

**Files:**
- Modify: `src/sonari/platform/base.py` (SupervisorBackend)
- Modify: `src/sonari/platform/macos/supervisor.py`, `src/sonari/platform/windows/supervisor.py`
- Test: `tests/test_win_supervisor.py`, `tests/test_macos_supervisor.py` (or the existing macOS supervisor test module)

> Added as **concrete** methods with defaults (not `@abstractmethod`) so no existing SupervisorBackend subclass/test-double breaks; each platform overrides them.

- [ ] **Step 1: Write failing tests.**

```python
# tests/test_win_supervisor.py
def test_post_install_notes_runs(capsys):
    from sonari.platform.windows.supervisor import WinSupervisorBackend
    WinSupervisorBackend().post_install_notes()
    out = capsys.readouterr().out
    assert "sonari doctor" in out and "M3" in out   # next steps + hotkeys deferred

def test_hooks_doctor_row_windows(monkeypatch, tmp_path):
    from sonari.platform.windows import supervisor as sup
    monkeypatch.setattr(sup, "claude_settings_path", lambda: str(tmp_path / "settings.json"))
    name, ok, _ = sup.WinSupervisorBackend().hooks_doctor_row()
    assert name == "hooks installed" and ok is False   # no settings yet
```

```python
# tests/test_macos_supervisor.py (add)
def test_macos_hooks_doctor_row_checks_repo_manifest():
    from sonari.platform.macos.supervisor import MacSupervisorBackend
    name, ok, detail = MacSupervisorBackend().hooks_doctor_row()
    assert name == "hooks installed"
    assert detail.endswith("hooks.json") or "missing" in detail
```

- [ ] **Step 2: Run → FAIL** (methods undefined).

- [ ] **Step 3: Add the ABC defaults** in `base.py` inside `class SupervisorBackend`:

```python
    def post_install_notes(self) -> None:
        """Print OS-specific post-install next steps. Default: nothing."""
        return None

    def hooks_doctor_row(self) -> "tuple":
        """Return a (name, ok, detail) row describing whether Sonari's hooks are
        installed. Default: unknown."""
        return ("hooks installed", False, "unknown")
```

Implement in `MacSupervisorBackend` (preserve today's macOS doctor string — the repo `hooks/hooks.json` check that lives in `cli.doctor()` today):

```python
    def post_install_notes(self) -> None:
        plugin_root = os.path.realpath(paths.repo_root())
        print("")
        print("Enable the Sonari plugin in Claude Code, then run 'sonari doctor'.")
        print(f"  - Per session: claude --plugin-dir {plugin_root}")
        print("  - Or enable 'sonari' from the /plugin menu (local marketplace).")
        if not _local_bin_on_path():
            print('Add ~/.local/bin to your PATH so `sonari` works in every shell:')
            print('  export PATH="$HOME/.local/bin:$PATH"')

    def hooks_doctor_row(self) -> tuple:
        hooks_json = os.path.join(paths.repo_root(), "hooks", "hooks.json")
        present = os.path.exists(hooks_json)
        return ("hooks installed", present,
                hooks_json if present else "missing: {0}".format(hooks_json))
```

Implement in `WinSupervisorBackend` (uses the settings.json helper added in Task 3 — define `claude_settings_path` + `settings_has_sonari_hooks` now as module functions so this task is self-contained; Task 3 builds the merge on top):

```python
def claude_settings_path() -> str:
    return os.path.join(os.path.expanduser("~"), ".claude", "settings.json")


def settings_has_sonari_hooks(settings_path: str) -> bool:
    import json
    try:
        with open(settings_path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except Exception:
        return False
    hooks = (data or {}).get("hooks", {})
    for entries in hooks.values():
        for entry in entries:
            for h in entry.get("hooks", []):
                args = h.get("args") or []
                blob = (h.get("command", "") + " " + " ".join(map(str, args)))
                if "sonari-hook" in blob:
                    return True
    return False
```

```python
    # inside WinSupervisorBackend
    def post_install_notes(self) -> None:
        print("")
        print("Sonari is installed. Run 'sonari doctor' to confirm everything is green.")
        print("  - Enable the 'sonari' plugin for its slash commands (optional).")
        print("  - Global hotkeys arrive in Milestone 3 (M3); speech works without them.")

    def hooks_doctor_row(self) -> tuple:
        path = claude_settings_path()
        ok = settings_has_sonari_hooks(path)
        return ("hooks installed", ok,
                path if ok else "no Sonari hooks in {0} (run 'sonari install')".format(path))
```

- [ ] **Step 4: Run → PASS**, then full suite.

- [ ] **Step 5: Commit**
```bash
git add src/sonari/platform/base.py src/sonari/platform/macos/supervisor.py \
        src/sonari/platform/windows/supervisor.py tests/test_win_supervisor.py tests/test_macos_supervisor.py
git commit -m "feat(platform): SupervisorBackend.post_install_notes() + hooks_doctor_row() (per-OS)"
```

---

## Task 3: Windows `~/.claude/settings.json` hook merge/remove helpers

**Files:**
- Modify: `src/sonari/platform/windows/supervisor.py`
- Test: `tests/test_win_settings_hooks.py` (NEW)

- [ ] **Step 1: Write the failing tests** (pure stdlib + tmp files — no winrt fakes needed):

```python
# tests/test_win_settings_hooks.py
import json
from sonari.platform.windows.supervisor import (
    merge_hooks_into_settings, remove_hooks_from_settings, settings_has_sonari_hooks,
)

PW = r"C:\Py\pythonw.exe"
HOOK = r"C:\plug\bin\sonari-hook"

def _read(p):
    with open(p, encoding="utf-8") as fh:
        return json.load(fh)

def test_merge_creates_file_with_sonari_hooks(tmp_path):
    sp = str(tmp_path / "settings.json")
    merge_hooks_into_settings(sp, PW, HOOK)
    data = _read(sp)
    md = data["hooks"]["MessageDisplay"][0]["hooks"][0]
    assert md["command"] == PW and md["args"] == [HOOK, "MessageDisplay"]
    assert settings_has_sonari_hooks(sp)

def test_merge_preserves_unrelated_keys_and_hooks(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({
        "theme": "dark",
        "hooks": {"Stop": [{"matcher": "", "hooks": [
            {"type": "command", "command": "other.exe", "args": ["x"]}]}]},
    }), encoding="utf-8")
    merge_hooks_into_settings(str(sp), PW, HOOK)
    data = _read(str(sp))
    assert data["theme"] == "dark"
    stop_cmds = [h["command"] for e in data["hooks"]["Stop"] for h in e["hooks"]]
    assert "other.exe" in stop_cmds and PW in stop_cmds   # unrelated kept, sonari added

def test_merge_is_idempotent(tmp_path):
    sp = str(tmp_path / "settings.json")
    merge_hooks_into_settings(sp, PW, HOOK)
    merge_hooks_into_settings(sp, PW, HOOK)
    data = _read(sp)
    sonari = [h for e in data["hooks"]["MessageDisplay"] for h in e["hooks"]
              if "sonari-hook" in (h.get("command","") + " ".join(h.get("args", [])))]
    assert len(sonari) == 1   # not duplicated

def test_remove_drops_only_sonari_entries(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({"hooks": {"Stop": [{"matcher": "", "hooks": [
        {"type": "command", "command": "other.exe", "args": ["x"]}]}]}}), encoding="utf-8")
    merge_hooks_into_settings(str(sp), PW, HOOK)
    remove_hooks_from_settings(str(sp), HOOK)
    data = _read(str(sp))
    assert not settings_has_sonari_hooks(str(sp))
    stop_cmds = [h["command"] for e in data["hooks"]["Stop"] for h in e["hooks"]]
    assert stop_cmds == ["other.exe"]   # unrelated survived

def test_invalid_json_aborts_without_clobber(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text("{ not json", encoding="utf-8")
    import pytest
    with pytest.raises(ValueError):
        merge_hooks_into_settings(str(sp), PW, HOOK)
    assert sp.read_text(encoding="utf-8") == "{ not json"   # untouched
```

- [ ] **Step 2: Run → FAIL** (functions undefined).

- [ ] **Step 3: Implement** in `windows/supervisor.py`. Reuse the existing `build_hooks_json` to derive the per-event entries, then merge/remove by the `sonari-hook` marker:

```python
def _build_hooks_dict(pythonw: str, hook_py: str) -> dict:
    """Return {event: [entry, ...]} for Sonari's exec-form hooks."""
    import json
    return json.loads(build_hooks_json(pythonw, hook_py))["hooks"]


def _entry_is_sonari(entry: dict, hook_py: str) -> bool:
    marker = os.path.basename(hook_py) or "sonari-hook"
    for h in entry.get("hooks", []):
        blob = (h.get("command", "") + " " + " ".join(map(str, h.get("args") or [])))
        if marker in blob or "sonari-hook" in blob:
            return True
    return False


def _load_settings(settings_path: str) -> dict:
    """Read settings.json tolerantly. Missing/empty -> {}. Unparseable -> ValueError
    (never clobber a file we cannot understand)."""
    import json
    if not os.path.exists(settings_path):
        return {}
    try:
        with open(settings_path, "r", encoding="utf-8") as fh:
            text = fh.read().strip()
    except OSError as exc:
        raise ValueError("cannot read {0}: {1}".format(settings_path, exc))
    if not text:
        return {}
    try:
        data = json.loads(text)
    except ValueError as exc:
        raise ValueError(
            "{0} is not valid JSON ({1}); refusing to overwrite. Fix or remove it, "
            "then re-run 'sonari install'.".format(settings_path, exc))
    return data if isinstance(data, dict) else {}


def _write_settings(settings_path: str, data: dict) -> None:
    import json
    os.makedirs(os.path.dirname(settings_path), exist_ok=True)
    with open(settings_path, "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2)
        fh.write("\n")


def merge_hooks_into_settings(settings_path: str, pythonw: str, hook_py: str) -> None:
    """Idempotently add Sonari's exec-form hooks to settings.json: drop any prior
    Sonari entries (self-heal across path changes), then append the current ones.
    Preserves all other keys and all non-Sonari hook entries."""
    data = _load_settings(settings_path)
    remove_hooks_from_settings(settings_path, hook_py, _data=data)  # in-place prune
    hooks = data.setdefault("hooks", {})
    for event, entries in _build_hooks_dict(pythonw, hook_py).items():
        hooks.setdefault(event, []).extend(entries)
    _write_settings(settings_path, data)


def remove_hooks_from_settings(settings_path: str, hook_py: str, _data=None) -> None:
    """Remove only Sonari's hook entries; prune emptied events / hooks. When *_data*
    is given, prune it in place and DO NOT write (used by merge)."""
    data = _data if _data is not None else _load_settings(settings_path)
    hooks = data.get("hooks", {})
    for event in list(hooks.keys()):
        hooks[event] = [e for e in hooks[event] if not _entry_is_sonari(e, hook_py)]
        if not hooks[event]:
            del hooks[event]
    if not hooks and "hooks" in data:
        del data["hooks"]
    if _data is None:
        _write_settings(settings_path, data)
```

- [ ] **Step 4: Run → PASS**, then full suite.

- [ ] **Step 5: Commit**
```bash
git add src/sonari/platform/windows/supervisor.py tests/test_win_settings_hooks.py
git commit -m "feat(windows): idempotent ~/.claude/settings.json hook merge/remove (Sonari-only, marker-identified)"
```

---

## Task 4: macOS backend — fill `install()`/`uninstall()` (behavior-preserving move)

**Files:**
- Modify: `src/sonari/platform/macos/supervisor.py` (`install`/`uninstall`)
- Modify: `src/sonari/platform/macos/hotkeys.py` (`uninstall` also tears down the hotkeyd LaunchAgent)
- Test: `tests/test_macos_supervisor.py`

> Move today's `cli.install()`/`cli.uninstall()` macOS bodies into the backend **verbatim** (same prints, same artifacts). `cli` is not switched yet (Task 6); this task adds the backend implementation + its tests. The shared steps (copy app, install record, keymap, voice check) stay in `cli`.

- [ ] **Step 1: Write failing tests** (patch `launchctl`/hotkey build so nothing real runs):

```python
# tests/test_macos_supervisor.py
def test_install_writes_and_loads_launchagent(tmp_path, monkeypatch):
    from sonari.platform.macos.supervisor import MacSupervisorBackend, LAUNCH_AGENT_PATH
    calls = []
    sup = MacSupervisorBackend()
    monkeypatch.setattr(sup, "launchctl", lambda a: calls.append(a) or 0)
    monkeypatch.setattr(MacSupervisorBackend, "_LAUNCH_AGENT_PATH", str(tmp_path / "speechd.plist"), raising=False)
    # ... (use monkeypatch on module LAUNCH_AGENT_PATH); assert plist written + load called
```

(Use the existing macOS install test as the template; it already patches these — repoint its target from `cli` to `MacSupervisorBackend`.)

- [ ] **Step 2: Run → FAIL** (`install` is still `pass`).

- [ ] **Step 3: Implement `MacSupervisorBackend.install`** — the macOS-specific portion of today's `cli.install()` (everything EXCEPT the shared resolve-python / copy-app / install-record / keymap / voice steps, which stay in cli):

```python
    def install(self, python, app_dir):
        """Write + load the speechd LaunchAgent and place the ~/.local/bin launcher.
        (Hotkeyd is installed separately via MacHotkeyBackend.install in cli.)"""
        if shutil.which("swiftc") is None:
            print("Xcode Command Line Tools not found; global hotkeys disabled. "
                  "Install them with:  xcode-select --install   then re-run: "
                  "sonari install")
        log = str(paths.LOG_PATH)
        xml = self.launchagent_plist(python_executable=python, src_path=app_dir,
                                     log_path=log)
        os.makedirs(os.path.dirname(LAUNCH_AGENT_PATH), exist_ok=True)
        with open(LAUNCH_AGENT_PATH, "w", encoding="utf-8") as f:
            f.write(xml)
        print(f"Wrote LaunchAgent: {LAUNCH_AGENT_PATH}")
        self.launchctl(["unload", LAUNCH_AGENT_PATH])
        rc = self.launchctl(["load", LAUNCH_AGENT_PATH])
        if rc == 0:
            print(f"Loaded LaunchAgent {LAUNCH_AGENT_LABEL}.")
        else:
            print(f"warning: 'launchctl load' returned {rc}; "
                  f"the daemon will still autostart on next login.")
        launcher = self.place_launcher(os.path.realpath(paths.repo_root()))
        print(f"Placed launcher: {launcher}")
```

Implement `MacSupervisorBackend.uninstall` — the speechd-LaunchAgent + artifact-cleanup portion of today's `cli.uninstall()` (the hotkeyd teardown moves to `MacHotkeyBackend.uninstall`; the app-copy removal + config preservation stay in cli):

```python
    def uninstall(self):
        """Unload + remove the speechd LaunchAgent and the ~/.local/bin launcher."""
        if os.path.exists(LAUNCH_AGENT_PATH):
            self.launchctl(["unload", LAUNCH_AGENT_PATH])
            try:
                os.remove(LAUNCH_AGENT_PATH)
                print(f"Removed LaunchAgent: {LAUNCH_AGENT_PATH}")
            except OSError as exc:
                print(f"warning: could not remove {LAUNCH_AGENT_PATH}: {exc}")
        else:
            print("No LaunchAgent installed.")
        path = _launcher_path()
        if os.path.exists(path):
            try:
                os.remove(path)
                print(f"Removed launcher: {path}")
            except OSError:
                pass
```

- [ ] **Step 4: Implement `MacHotkeyBackend.uninstall`** to also unload + remove the hotkeyd LaunchAgent (today done inline in `cli.uninstall`), keeping the binary removal:

```python
    def uninstall(self) -> None:
        from sonari.platform.macos.supervisor import MacSupervisorBackend
        agent = LAUNCH_AGENT_PATH                      # hotkeyd plist path (module const)
        if os.path.exists(agent):
            MacSupervisorBackend().launchctl(["unload", agent])
            try:
                os.remove(agent)
                print(f"Removed LaunchAgent: {agent}")
            except OSError as exc:
                print(f"warning: could not remove {agent}: {exc}")
        try:
            os.remove(str(paths.HOTKEYD_BIN_PATH))
            print(f"Removed hotkey daemon binary: {paths.HOTKEYD_BIN_PATH}")
        except OSError:
            pass
```

- [ ] **Step 5: Run macOS supervisor + hotkey tests → PASS**, then full suite.

- [ ] **Step 6: Commit**
```bash
git add src/sonari/platform/macos/supervisor.py src/sonari/platform/macos/hotkeys.py tests/test_macos_supervisor.py
git commit -m "refactor(macos): move install/uninstall orchestration into the backend (behavior-preserving)"
```

---

## Task 5: Windows backend — extend `install()`/`uninstall()` (hooks + launcher)

**Files:**
- Modify: `src/sonari/platform/windows/supervisor.py` (`install`/`uninstall` + a `_place_launcher` helper)
- Test: `tests/test_win_supervisor.py`

- [ ] **Step 1: Write failing tests** (spy schtasks + settings + launcher via tmp paths):

```python
# tests/test_win_supervisor.py (add)
def test_install_registers_task_merges_hooks_and_places_launcher(tmp_path, monkeypatch):
    from sonari.platform.windows import supervisor as sup
    calls = []
    monkeypatch.setattr(sup, "task_install", lambda pw, spy: calls.append(("task", pw)) or 0)
    monkeypatch.setattr(sup, "claude_settings_path", lambda: str(tmp_path / "settings.json"))
    monkeypatch.setattr(sup, "_local_bin_dir", lambda: str(tmp_path / "bin"))
    monkeypatch.setattr("sonari.paths.repo_root", lambda: str(tmp_path / "plug"))
    s = sup.WinSupervisorBackend()
    s.install(r"C:\Py\pythonw.exe", str(tmp_path / "app"))
    assert ("task", r"C:\Py\pythonw.exe") in calls
    assert sup.settings_has_sonari_hooks(str(tmp_path / "settings.json"))
    assert (tmp_path / "bin" / "sonari.cmd").exists()

def test_uninstall_removes_task_hooks_and_launcher(tmp_path, monkeypatch):
    from sonari.platform.windows import supervisor as sup
    monkeypatch.setattr(sup, "task_uninstall", lambda: 0)
    monkeypatch.setattr(sup, "claude_settings_path", lambda: str(tmp_path / "settings.json"))
    monkeypatch.setattr(sup, "_local_bin_dir", lambda: str(tmp_path / "bin"))
    monkeypatch.setattr("sonari.paths.repo_root", lambda: str(tmp_path / "plug"))
    s = sup.WinSupervisorBackend()
    s.install(r"C:\Py\pythonw.exe", str(tmp_path / "app"))
    s.uninstall()
    assert not sup.settings_has_sonari_hooks(str(tmp_path / "settings.json"))
    assert not (tmp_path / "bin" / "sonari.cmd").exists()
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** the launcher helper + extended `install`/`uninstall`. The console interpreter for the launcher is `pythonw.exe`'s `python.exe` sibling:

```python
def _local_bin_dir() -> str:
    return os.path.join(os.path.expanduser("~"), ".local", "bin")


def _console_python(pythonw: str) -> str:
    """python.exe sibling of pythonw.exe (console, for the CLI launcher)."""
    cand = pythonw.replace("pythonw.exe", "python.exe")
    return cand if os.path.isfile(cand) else pythonw


def _hook_py() -> str:
    return os.path.join(paths.repo_root(), "bin", "sonari-hook")
```

```python
    # WinSupervisorBackend
    def install(self, python, app_dir):
        # 1. Task Scheduler autostart (pythonw runs the supervisor loop).
        supervisor_py = os.path.join(app_dir, "sonari", "platform",
                                     "windows", "supervisor_loop.py")
        rc = task_install(python, supervisor_py)
        if rc == 0:
            print("Registered Task Scheduler task: {0}".format(TASK_NAME))
        else:
            print("warning: schtasks /create returned {0}; autostart may not be "
                  "registered.".format(rc))
        # 2. Exec-form hooks merged into the user's Claude settings.json.
        settings = claude_settings_path()
        merge_hooks_into_settings(settings, python, _hook_py())
        print("Wrote Sonari hooks to: {0}".format(settings))
        # 3. sonari.cmd launcher on ~/.local/bin.
        launcher = self._place_launcher(python, app_dir)
        print("Placed launcher: {0}".format(launcher))

    def _place_launcher(self, python, app_dir) -> str:
        path = os.path.join(_local_bin_dir(), "sonari.cmd")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        body = (
            "@echo off\r\n"
            'set "PYTHONPATH={app}"\r\n'
            '"{py}" -m sonari.cli %*\r\n'
        ).format(app=app_dir, py=_console_python(python))
        with open(path, "w", encoding="utf-8", newline="") as fh:
            fh.write(body)
        return path

    def uninstall(self):
        rc = task_uninstall()
        print("Removed Task Scheduler task: {0}".format(TASK_NAME) if rc == 0
              else "No Task Scheduler task to remove.")
        remove_hooks_from_settings(claude_settings_path(), _hook_py())
        print("Removed Sonari hooks from: {0}".format(claude_settings_path()))
        launcher = os.path.join(_local_bin_dir(), "sonari.cmd")
        if os.path.exists(launcher):
            try:
                os.remove(launcher)
                print("Removed launcher: {0}".format(launcher))
            except OSError:
                pass
```

- [ ] **Step 4: Run → PASS**, then full suite.

- [ ] **Step 5: Commit**
```bash
git add src/sonari/platform/windows/supervisor.py tests/test_win_supervisor.py
git commit -m "feat(windows): install/uninstall register the Task, merge settings.json hooks, and place sonari.cmd"
```

---

## Task 6: `cli.py` — OS-agnostic dispatch (the switch)

**Files:**
- Modify: `src/sonari/cli.py`
- Test: `tests/test_cli_install.py`, `tests/test_cli_uninstall.py`, `tests/test_cli_doctor.py` (repoint patches; keep macOS output assertions)

> This commit makes `cli` go through `get_platform()`. macOS output stays identical because the backend prints the same strings (Task 4). The dormant macOS body in `cli` is deleted here.

- [ ] **Step 1: Update the cli tests** to patch the platform backend rather than `cli._launchctl`/`cli._build_hotkeyd`. Add a Windows-path test:

```python
# tests/test_cli_install.py
def test_install_dispatches_through_platform(monkeypatch):
    import sonari.cli as cli
    seq = []
    class FakeSup:
        def resolve_python(self): return "PYEXE"
        def _probe_python_version(self, p): return (3, 12)
        def install(self, py, app): seq.append(("sup.install", py, app))
        def post_install_notes(self): seq.append(("notes",))
    class FakeHK:
        def install(self, **k): seq.append(("hk.install",)); return (False, "M3")
    class FakeTts:
        def best_voice(self): return "Aria"
    import types
    pb = types.SimpleNamespace(supervisor=FakeSup(), hotkey=FakeHK(), tts=FakeTts())
    monkeypatch.setattr(cli, "_platform", lambda: pb)
    monkeypatch.setattr(cli, "_copy_app", lambda root: "APPDIR")
    monkeypatch.setattr(cli, "_write_install_record", lambda **k: None)
    monkeypatch.setattr("sonari.keymap.write_default_keymap_if_absent", lambda: None)
    monkeypatch.setattr("sonari.keymap.write_resolved", lambda: None)
    assert cli.install() == 0
    assert ("sup.install", "PYEXE", "APPDIR") in seq and ("notes",) in seq
```

- [ ] **Step 2: Run → FAIL** (cli still macOS-coupled).

- [ ] **Step 3: Rewrite `cli.py`.** Remove the top-level macOS imports (lines 23-35) and the macOS-only shims/constants (`LAUNCH_AGENT_*`, `_plist`, `_launchagent_plist`, `_launchctl`, `_build_hotkeyd`, `_place_launcher`, `_launcher_path`, `_local_bin_on_path`, `_remove_launcher`, `_daemon_shim_path`). Add the cached accessor and rewrite the functions:

```python
from sonari.platform import get_platform

_PLATFORM = None
def _platform():
    global _PLATFORM
    if _PLATFORM is None:
        _PLATFORM = get_platform()
    return _PLATFORM


def _resolve_python():
    return _platform().supervisor.resolve_python()

def _probe_python_version(path: str):
    return _platform().supervisor._probe_python_version(path)

def _combo_label(modifiers: int, key_code: int) -> str:
    return _platform().hotkey.display_combo(modifiers, key_code)


def install() -> int:
    paths.ensure_sonari_dir()
    sup = _platform().supervisor
    python = sup.resolve_python()
    if python is None:
        print("No suitable Python >= 3.9 found. Install Python 3.9+ "
              "(python.org) and re-run: sonari install")
        return 1
    ver = sup._probe_python_version(python)
    py_ver = "{0}.{1}".format(*ver) if ver else "3.9"
    print(f"Using interpreter: {python} (Python {py_ver})")

    plugin_root = os.path.realpath(paths.repo_root())
    try:
        app_dir = _copy_app(plugin_root)
    except OSError as exc:
        print(f"Could not copy the runtime to ~/.sonari/app: {exc}. "
              f"Check that ~/.sonari is writable.")
        return 1
    print(f"Copied runtime to: {app_dir}")

    keymap.write_default_keymap_if_absent()
    keymap.write_resolved()

    plugin_version = _read_plugin_version(plugin_root)
    _write_install_record(python=python, python_version=py_ver,
                          plugin_root=plugin_root, app_path=app_dir,
                          plugin_version=plugin_version)

    sup.install(python, app_dir)

    hk_log = os.path.join(os.path.dirname(str(paths.LOG_PATH)), "hotkeyd.log")
    ok, detail = _platform().hotkey.install(
        log_path=hk_log,
        agent_path=None,                # macOS backend owns its plist path
        launchctl_fn=sup.launchctl if hasattr(sup, "launchctl") else (lambda a: 0),
    )
    if not ok:
        print(f"note: global hotkeys not enabled ({detail}); speech still works.")

    try:
        voice = _platform().tts.best_voice()
        print(f"Voice: {voice}." if voice else "Voice: default.")
    except Exception:  # noqa: BLE001 - voice check must never break install
        pass

    sup.post_install_notes()
    return 0


def uninstall() -> int:
    sup = _platform().supervisor
    sup.uninstall()
    try:
        _platform().hotkey.uninstall()
    except Exception:  # noqa: BLE001
        pass

    # Shared runtime-artifact cleanup (preserve config.json + keymap.json).
    sonari_dir = paths.SONARI_DIR
    for artifact in (paths.LOCK_PATH, paths.LOG_PATH, paths.HOTKEYD_RESOLVED_PATH,
                     paths.INSTALL_RECORD_PATH, sonari_dir / "hotkeyd.log"):
        if os.path.exists(str(artifact)):
            try:
                os.remove(str(artifact))
            except OSError:
                pass
    if os.path.isdir(str(paths.APP_DIR)):
        try:
            shutil.rmtree(str(paths.APP_DIR))
            print(f"Removed app copy: {paths.APP_DIR}")
        except OSError:
            pass

    preserved = []
    if os.path.exists(str(paths.KEYMAP_PATH)):
        preserved.append("keymap.json")
    if os.path.exists(str(paths.CONFIG_PATH)):
        preserved.append("config.json")
    if preserved:
        print(f"Preserved your settings: {', '.join(preserved)}")
    print(f"Removed Sonari runtime files from {sonari_dir} "
          f"(keymap.json and config.json left in place).")
    print("Done.")
    return 0
```

Update `doctor()`: replace `results.extend(_mac_sup.doctor_rows())` with `results.extend(_platform().supervisor.doctor_rows())`, and replace the inline `plugin hooks.json` row (`_repo_hooks_json_path()`) with `results.append(_platform().supervisor.hooks_doctor_row())`. Delete `_repo_hooks_json_path` if now unused.

- [ ] **Step 4: Run the cli tests → PASS**, then the full suite on macOS (output identical).

- [ ] **Step 5: Commit**
```bash
git add src/sonari/cli.py tests/test_cli_install.py tests/test_cli_uninstall.py tests/test_cli_doctor.py
git commit -m "refactor(cli): install/uninstall/doctor dispatch through get_platform() (no macOS coupling)"
```

---

## Task 7: Extend the Windows acceptance checklist

**Files:**
- Modify: `docs/superpowers/M2-WINDOWS-ACCEPTANCE.md`

- [ ] **Step 1: Add** under §1 (Install) and §7 (Hooks), with exact commands + expected output:
  - `sonari install` writes Sonari hooks into `%USERPROFILE%\.claude\settings.json` (exec-form: `command` = resolved `pythonw.exe`, `args` = `[<...>\bin\sonari-hook, "MessageDisplay"]`); confirm other keys/hooks preserved.
  - The **double-fire constraint**: do NOT also enable the plugin's shell-form manifest hooks on Windows (each event would fire twice). Sonari hooks come from `settings.json` only.
  - `%USERPROFILE%\.local\bin\sonari.cmd` exists and `sonari doctor` runs through it.
  - `sonari uninstall` removes the Task, the Sonari hook entries (only), and `sonari.cmd`; `config.json` + `keymap.json` survive.

- [ ] **Step 2: Commit**
```bash
git add docs/superpowers/M2-WINDOWS-ACCEPTANCE.md
git commit -m "docs(windows): acceptance checks for settings.json hooks, launcher, and uninstall"
```

---

## Self-Review (completed)

- **Spec coverage:** §3.1 cli de-macOS → T6; §3.2 macOS move → T4; §3.3 Windows install/uninstall → T5 (+hooks merge T3, launcher T5); §3.4 voice contract → T1; §3.5 settings.json merge → T3; §3.6 double-fire → T7 doc; §2 doctor OS-aware row + dispatch → T2 (`hooks_doctor_row`) + T6. Notes/next-steps → T2 (`post_install_notes`).
- **Type consistency:** `best_voice() -> str` / `_best_voice_info() -> VoiceInformation` (T1) used by `cli` voice line (T6). `merge_hooks_into_settings(path, pythonw, hook_py)` / `remove_hooks_from_settings(path, hook_py)` / `settings_has_sonari_hooks(path)` / `claude_settings_path()` consistent across T2/T3/T5. `supervisor.install(python, app_dir)` ABC signature honored by both backends + the `cli` call site.
- **Ordering:** T1-T3 additive (suite green). T4 fills macOS backend (dormant). T5 fills Windows backend. T6 flips `cli` to dispatch + deletes the dormant macOS body. T7 docs.
- **macOS preservation:** backend `install/uninstall/post_install_notes` reproduce today's exact prints; `hooks_doctor_row` (macOS) returns the same repo `hooks/hooks.json` string the cli row used.
