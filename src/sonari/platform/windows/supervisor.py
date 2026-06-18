"""Windows supervisor backend — zero-admin Task Scheduler autostart, Python
resolution (py-launcher + Store-stub avoidance), exec-form hooks, and the
WinSupervisorBackend ABC implementation.

WINDOWS-only. Every Windows-only stdlib import (winreg, ctypes) is lazy (inside
a method/function) so this module imports cleanly on macOS/Linux for the mock
test suite. "Importable + mock-green" here does NOT mean Windows-verified — the
real gate is docs/superpowers/M2-WINDOWS-ACCEPTANCE.md.

Bodies copied verbatim from docs/superpowers/m2-windows-api-reference.md
(§Windows SupervisorBackend), adapting only: the file/import location to our
layout (src/sonari/platform/windows/...), subclassing the real ABC from
sonari.platform.base, and keeping Windows-only imports lazy.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Optional

from sonari.platform.base import SupervisorBackend

TASK_NAME = "Sonari.Speechd"

# Windows process-creation flags. Defined in subprocess only on win32, so use
# hex literals to keep this module importable on macOS/Linux.
_CREATE_NO_WINDOW = 0x08000000
_DETACHED_PROCESS = 0x00000008
_SPAWN_FLAGS = _CREATE_NO_WINDOW | _DETACHED_PROCESS  # 0x08000008


# ---------------------------------------------------------------------------
# Zero-admin Task Scheduler autostart via hand-authored XML
# ---------------------------------------------------------------------------

# UTF-16 LE with BOM is required by schtasks /xml on older Windows builds.
# Python's encoding='utf-16' produces exactly that.
TASK_XML_TEMPLATE = '''<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2"
  xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>{user_id}</Author>
    <Description>Sonari speech daemon supervisor (autostart on logon)</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user_id}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <Hidden>true</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>PT5M</Interval>
      <Count>5</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{pythonw}</Command>
      <Arguments>"{supervisor_py}"</Arguments>
      <WorkingDirectory>{work_dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
'''


def _current_user_id() -> str:
    """Return DOMAIN\\user or COMPUTERNAME\\user for LogonTrigger/UserId."""
    import ctypes
    buf = ctypes.create_unicode_buffer(256)
    size = ctypes.c_ulong(256)
    ctypes.windll.secur32.GetUserNameExW(2, buf, ctypes.byref(size))  # 2 = NameSamCompatible
    return buf.value


def task_install(pythonw: str, supervisor_py: str) -> int:
    """Register the Task Scheduler task. Returns schtasks exit code (0 = success)."""
    user_id = _current_user_id()
    xml_content = TASK_XML_TEMPLATE.format(
        user_id=user_id,
        pythonw=pythonw,
        supervisor_py=supervisor_py,
        work_dir=str(Path(supervisor_py).parent),
    )
    # Write UTF-16 LE with BOM — required by schtasks /xml
    with tempfile.NamedTemporaryFile(
            mode='w', suffix='.xml', encoding='utf-16',
            delete=False) as fh:
        fh.write(xml_content)
        tmp = fh.name
    try:
        return subprocess.call(
            ["schtasks", "/create", "/tn", TASK_NAME, "/xml", tmp, "/f"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    finally:
        os.unlink(tmp)


def task_uninstall() -> int:
    """Delete the task. /f suppresses confirmation prompt."""
    return subprocess.call(
        ["schtasks", "/delete", "/tn", TASK_NAME, "/f"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )


# KEY GOTCHA: RestartOnFailure is NOT expressible via schtasks CLI flags — XML only.
# The Task Scheduler's RestartOnFailure only restarts the *supervisor* process if
# it crashes (unlikely). The supervisor_loop is the real daemon restarter.


# ---------------------------------------------------------------------------
# Windows Python resolution — py -3 launcher, PATH probe, Store-stub detection
# ---------------------------------------------------------------------------

def _is_store_stub(path: str) -> bool:
    """Return True if *path* is the Windows Store Python stub.

    Fast path: WindowsApps in the normalised path.
    Slow path: run it and check for exit code 9009 (store stub sentinel) or
    empty stdout (the stub prints nothing and exits non-zero).
    """
    if "WindowsApps" in os.path.normcase(path):
        return True
    try:
        result = subprocess.run(
            [path, "-c", "import sys; print(sys.executable)"],
            capture_output=True, text=True, timeout=5,
        )
        return result.returncode == 9009 or not result.stdout.strip()
    except Exception:
        return True   # treat anything broken as a stub


def _find_pythonw(python_real: str) -> "str | None":
    """Return the pythonw.exe sibling of *python_real*, or None."""
    d = os.path.dirname(python_real)
    for candidate in (
        os.path.join(d, "pythonw.exe"),
        os.path.join(d, "Scripts", "pythonw.exe"),   # venv layout
    ):
        if os.path.isfile(candidate):
            return candidate
    return None


def _probe_python_version(candidate: str):
    """Return (major, minor) or None."""
    try:
        out = subprocess.check_output(
            [candidate, "-c",
             "import sys; print('%d.%d' % sys.version_info[:2])"],
            stderr=subprocess.DEVNULL, text=True, timeout=5,
        ).strip()
        major, minor = out.split(".")
        return (int(major), int(minor))
    except Exception:
        return None


def _probe_version_via_launcher(py_exe: str) -> "str | None":
    """Use `py -3 -c 'print(sys.executable)'` to resolve the real interpreter."""
    try:
        real = subprocess.check_output(
            [py_exe, "-3", "-c", "import sys; print(sys.executable)"],
            stderr=subprocess.DEVNULL, text=True, timeout=5,
        ).strip()
        return real if real else None
    except Exception:
        return None


def resolve_python_windows() -> "str | None":
    """Return pythonw.exe path for the best Python 3 >= 3.9, or None.

    Resolution order:
      1. py -3 launcher (works even when python.exe is not on PATH)
      2. 'python' on PATH (skip Microsoft Store stubs)
      3. 'python3' on PATH (skip Microsoft Store stubs)
    Deduped by realpath; prefers the py-launcher result.
    """
    seen_real = set()
    candidates = []   # list of (real_python_path, source_label)

    # 1. Windows Python Launcher
    py = shutil.which("py")
    if py:
        real = _probe_version_via_launcher(py)
        if real and not _is_store_stub(real):
            candidates.append((real, "py-launcher"))

    # 2 & 3. PATH-based names
    for name in ("python", "python3"):
        found = shutil.which(name)
        if found and not _is_store_stub(found):
            try:
                real = subprocess.check_output(
                    [found, "-c", "import sys; print(sys.executable)"],
                    stderr=subprocess.DEVNULL, text=True, timeout=5,
                ).strip()
            except Exception:
                continue
            if real:
                candidates.append((real, name))

    for real, _src in candidates:
        norm = os.path.normcase(os.path.realpath(real))
        if norm in seen_real:
            continue
        seen_real.add(norm)
        ver = _probe_python_version(real)
        if ver and ver >= (3, 9):
            pw = _find_pythonw(real)
            if pw:
                return pw

    return None


# ---------------------------------------------------------------------------
# exec-form hooks.json for Windows (no bash shim) + .gitattributes LF line
# ---------------------------------------------------------------------------

# The resolved pythonw.exe path is baked in at install time by
# WinSupervisorBackend.install(). Claude Code supports separate 'command' +
# 'args' (exec-form) — no bash shim required.
# Event set mirrors hooks/hooks.json (the macOS hooks file), translated to
# exec-form (command + args array) because there is no bash on Windows.
HOOKS_JSON_TEMPLATE = '''{{
  "hooks": {{
    "MessageDisplay": [
      {{
        "matcher": "",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "MessageDisplay"
            ]
          }}
        ]
      }}
    ],
    "PreToolUse": [
      {{
        "matcher": "AskUserQuestion",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "PreToolUse"
            ]
          }}
        ]
      }},
      {{
        "matcher": "ExitPlanMode",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "PreToolUse"
            ]
          }}
        ]
      }},
      {{
        "matcher": "",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "PreToolUse"
            ]
          }}
        ]
      }}
    ],
    "Notification": [
      {{
        "matcher": "permission_prompt",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "Notification"
            ]
          }}
        ]
      }},
      {{
        "matcher": "idle_prompt",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "Notification"
            ]
          }}
        ]
      }}
    ],
    "Stop": [
      {{
        "matcher": "",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "Stop"
            ]
          }}
        ]
      }}
    ],
    "UserPromptSubmit": [
      {{
        "matcher": "",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "UserPromptSubmit"
            ]
          }}
        ]
      }}
    ],
    "SessionStart": [
      {{
        "matcher": "",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "SessionStart"
            ]
          }}
        ]
      }}
    ],
    "SessionEnd": [
      {{
        "matcher": "",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "SessionEnd"
            ]
          }}
        ]
      }}
    ]
  }}
}}'''


def build_hooks_json(pythonw: str, hook_py: str) -> str:
    """Return hooks.json content with backslashes doubled for JSON."""
    return HOOKS_JSON_TEMPLATE.format(
        pythonw=pythonw.replace("\\", "\\\\"),
        hook_py=hook_py.replace("\\", "\\\\"),
    )


# ---------------------------------------------------------------------------
# ~/.claude/settings.json hook delivery (Windows uses exec-form hooks here,
# since the plugin's shell-form manifest cannot spawn the Python hook on win32)
# ---------------------------------------------------------------------------

def claude_settings_path() -> str:
    """Path to the user-scope Claude Code settings.json."""
    return os.path.join(os.path.expanduser("~"), ".claude", "settings.json")


def settings_has_sonari_hooks(settings_path: str) -> bool:
    """True if settings.json contains at least one Sonari hook entry (identified by
    the structured SONARI_HOOK_MARKER sentinel, #23).

    Defensive at every level: a hand-edited settings.json can have any shape
    (hooks a list, an entry a string, args not a list). 'sonari doctor' must never
    crash on it — any unexpected shape simply yields False (M9)."""
    import json
    try:
        with open(settings_path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except Exception:
        return False
    hooks = (data or {}).get("hooks", {}) if isinstance(data, dict) else {}
    if not isinstance(hooks, dict):
        return False
    try:
        for entries in hooks.values():
            if not isinstance(entries, list):
                continue
            for entry in entries:
                if not isinstance(entry, dict):
                    continue
                inner = entry.get("hooks", [])
                if not isinstance(inner, list):
                    continue
                if any(_hook_is_sonari(h) for h in inner):
                    return True
    except Exception:  # noqa: BLE001 - doctor must never raise on malformed input
        return False
    return False


def settings_has_sonari_plugin(settings_path: str) -> bool:
    """True if the Sonari plugin is enabled in settings.json. When it is, the
    plugin's hooks/hooks.json supplies the hooks, so a hand-wired settings.json
    block is not required (and would double-fire). Tolerant of any malformed shape
    (doctor must never crash) — M9."""
    import json
    try:
        with open(settings_path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except Exception:
        return False
    if not isinstance(data, dict):
        return False
    plugins = data.get("enabledPlugins", {})
    if not isinstance(plugins, dict):
        return False
    try:
        for name, enabled in plugins.items():
            # keys look like "sonari@sonari"; match the plugin-name part.
            if enabled and str(name).split("@", 1)[0] == "sonari":
                return True
    except Exception:  # noqa: BLE001 - doctor must never raise on malformed input
        return False
    return False


# Structured, collision-proof sentinel stamped on every hook Sonari writes.
# Identifying our own entries by this key (not by a "sonari-hook" substring scan
# over command+args) means a user's look-alike hook is never false-clobbered, and
# presence-check and removal can never diverge.
SONARI_HOOK_MARKER = "_sonari"


def _sonari_hook_paths(settings_path: str) -> list:
    """The baked script path (args[0]) of every Sonari hook in settings.json.
    Never raises (doctor must not crash); tolerant of any malformed shape."""
    import json
    try:
        with open(settings_path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except Exception:
        return []
    out = []
    hooks = (data or {}).get("hooks", {}) if isinstance(data, dict) else {}
    if not isinstance(hooks, dict):
        return out
    try:
        for entries in hooks.values():
            if not isinstance(entries, list):
                continue
            for entry in entries:
                if not isinstance(entry, dict):
                    continue
                inner = entry.get("hooks", [])
                if not isinstance(inner, list):
                    continue
                for h in inner:
                    if _hook_is_sonari(h):
                        args = h.get("args") or []
                        if isinstance(args, list) and args:
                            out.append(str(args[0]))
    except Exception:  # noqa: BLE001 - doctor must never raise on malformed input
        return out
    return out


def _build_hooks_dict(pythonw: str, hook_py: str) -> dict:
    """Return {event: [entry, ...]} for Sonari's exec-form hooks (from
    build_hooks_json), each hook stamped with the SONARI_HOOK_MARKER sentinel."""
    import json
    hooks = json.loads(build_hooks_json(pythonw, hook_py))["hooks"]
    for entries in hooks.values():
        for entry in entries:
            for h in entry.get("hooks", []):
                h[SONARI_HOOK_MARKER] = True
    return hooks


def _hook_is_sonari(h: dict) -> bool:
    """True if a single hook dict is one Sonari wrote (structured sentinel)."""
    return isinstance(h, dict) and h.get(SONARI_HOOK_MARKER) is True


def _entry_is_sonari(entry: dict, hook_py: str = "") -> bool:
    """True if a settings.json hook entry belongs to Sonari. Keyed on the
    structured sentinel, not a free-text marker (hook_py kept for call-compat)."""
    return any(_hook_is_sonari(h) for h in entry.get("hooks", []))


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
    """Atomically replace settings.json. This is the user's SHARED Claude config,
    so a truncating in-place write that fails mid-serialization would corrupt the
    whole file. Write a temp file in the same dir, fsync, then os.replace (atomic
    on POSIX + Windows). Mirrors the temp+replace pattern in keymap.py."""
    import json
    import tempfile
    parent = os.path.dirname(settings_path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    fd, tmp = tempfile.mkstemp(
        dir=parent or ".", prefix=".sonari-settings-", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(data, fh, indent=2)
            fh.write("\n")
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, settings_path)
    except BaseException:
        try:
            os.remove(tmp)
        except OSError:
            pass
        raise


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


def _validate_hooks_shape(data: dict, settings_path: str) -> None:
    """Raise a friendly ValueError if settings.json has a 'hooks' value we cannot
    safely merge into (must be absent or a dict of event -> list). Without this a
    malformed shape raises a cryptic AttributeError mid-merge."""
    hooks = data.get("hooks")
    if hooks is None:
        return
    if not isinstance(hooks, dict):
        raise ValueError(
            "{0} has a 'hooks' value that is not an object; refusing to modify it. "
            "Fix or remove it, then re-run 'sonari install'.".format(settings_path))
    for event, entries in hooks.items():
        if not isinstance(entries, list):
            raise ValueError(
                "{0} hooks['{1}'] is not a list; refusing to modify it. Fix or "
                "remove it, then re-run 'sonari install'.".format(
                    settings_path, event))


def merge_hooks_into_settings(settings_path: str, pythonw: str, hook_py: str) -> None:
    """Idempotently add Sonari's exec-form hooks to settings.json: drop any prior
    Sonari entries (self-heal across path changes), then append the current ones.
    Preserves all other keys and all non-Sonari hook entries."""
    data = _load_settings(settings_path)
    _validate_hooks_shape(data, settings_path)
    remove_hooks_from_settings(settings_path, hook_py, _data=data)  # in-place prune
    hooks = data.setdefault("hooks", {})
    for event, entries in _build_hooks_dict(pythonw, hook_py).items():
        hooks.setdefault(event, []).extend(entries)
    _write_settings(settings_path, data)


# ---------------------------------------------------------------------------
# Windows launcher (the ~/.local/bin/sonari analogue: a sonari.cmd shim)
# ---------------------------------------------------------------------------

def _local_bin_dir() -> str:
    return os.path.join(os.path.expanduser("~"), ".local", "bin")


def _console_python(pythonw: str) -> str:
    """python.exe sibling of pythonw.exe (console interpreter, for the CLI launcher)."""
    cand = pythonw.replace("pythonw.exe", "python.exe")
    return cand if os.path.isfile(cand) else pythonw


def _hook_py() -> str:
    """Absolute path to the plugin's bin/sonari-hook (pure-Python hook entry)."""
    from sonari import paths
    return os.path.join(paths.repo_root(), "bin", "sonari-hook")


# ---------------------------------------------------------------------------
# WinSupervisorBackend — the SupervisorBackend ABC implementation
# ---------------------------------------------------------------------------

class WinSupervisorBackend(SupervisorBackend):

    # --- monkeypatchable thin wrappers ---

    def _schtasks(self, args: list) -> int:
        """Run 'schtasks <args>'. Monkeypatched in tests."""
        return subprocess.call(
            ["schtasks"] + args,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )

    def _probe_python_version(self, candidate: str):
        """Return (major, minor) or None. Monkeypatched in tests."""
        try:
            out = subprocess.check_output(
                [candidate, "-c",
                 "import sys; print('%d.%d' % sys.version_info[:2])"],
                stderr=subprocess.DEVNULL, text=True, timeout=5,
            ).strip()
            major, minor = out.split(".")
            return (int(major), int(minor))
        except Exception:
            return None

    def _list_neural_voices(self) -> list:
        """Return list of neural voice token names. Monkeypatched in tests.

        Registry path: HKLM\\SOFTWARE\\Microsoft\\Speech_OneCore\\Voices\\Tokens
        NOT the legacy Speech\\Voices\\Tokens key (Narrator/OneCore voices only).
        winreg is Windows-only stdlib — imported lazily inside the method.
        """
        import winreg
        key_path = r"SOFTWARE\Microsoft\Speech_OneCore\Voices\Tokens"
        voices = []
        try:
            key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, key_path)
            i = 0
            while True:
                try:
                    voices.append(winreg.EnumKey(key, i))
                    i += 1
                except OSError:
                    break
        except OSError:
            pass
        return voices

    # --- SupervisorBackend ABC ---

    def is_installed(self) -> bool:
        """Return True if the Task Scheduler task exists."""
        return self._schtasks(["/query", "/tn", TASK_NAME]) == 0

    def is_running(self) -> bool:
        """Return True if the daemon socket is accepting connections."""
        from sonari import paths
        return paths.socket_connectable()

    def resolve_python(self) -> Optional[str]:
        """Return pythonw.exe for the best Python >= 3.9, or None."""
        return resolve_python_windows()

    def launch_spec(self) -> tuple:
        """Return (argv, spawn_kwargs) for lazy daemon start."""
        from sonari import paths
        pw = self.resolve_python() or "pythonw.exe"
        argv = [pw, "-m", "sonari.daemon"]
        # The daemon runs in a fresh process; without PYTHONPATH it cannot import
        # 'sonari' -> it exits instantly -> every hook event respawns it (a
        # relaunch storm). Put the plugin's own src/ first so the lazy start
        # resolves the package self-containedly.
        env = dict(os.environ)
        src = os.path.join(paths.repo_root(), "src")
        existing = env.get("PYTHONPATH", "")
        env["PYTHONPATH"] = src + (os.pathsep + existing if existing else "")
        # Route the daemon's stderr to the daemon log (parity with the macOS plist
        # StandardErrorPath) so the speak-loop catch-all traceback survives (#20);
        # DEVNULL made it unrecoverable. Open lazily inside launch_spec.
        paths.ensure_sonari_dir()
        err = open(paths.LOG_PATH, "a")
        kwargs = dict(
            creationflags=_SPAWN_FLAGS,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=err,
            env=env,
        )
        return argv, kwargs

    def doctor_rows(self) -> list:
        """Return Windows-specific [(name, ok, detail), ...] rows.

        Never raises — wrap every external call in try/except so 'sonari doctor'
        always renders (mirrors MacSupervisorBackend).
        """
        rows = []

        # schtasks availability
        schtasks = shutil.which("schtasks")
        rows.append(("schtasks", schtasks is not None,
                     schtasks or "not found (unexpected on Windows)"))

        # Task registered
        task_ok = self.is_installed()
        rows.append(("Task Scheduler task", task_ok,
                     TASK_NAME if task_ok
                     else "not registered (run 'sonari install')"))

        # pythonw.exe
        pw = self.resolve_python()
        rows.append(("pythonw.exe", pw is not None,
                     pw or "no Python >= 3.9 found; install from python.org"))

        # Neural voices (Speech_OneCore)
        try:
            voices = self._list_neural_voices()
            ok = bool(voices)
            detail = voices[0] if ok else (
                "none; install from Settings > Time & language > Speech")
            rows.append(("neural voice", ok, detail))
        except Exception as exc:
            rows.append(("neural voice", False, "error: {0}".format(exc)))

        # PyWinRT (the OneCore TTS engine). Absent -> total no-speech, so a
        # doctor green everywhere else would be dangerously misleading. (#7)
        try:
            from sonari.platform.windows.tts import _winrt_available
            ok = _winrt_available()
            rows.append(("TTS runtime", ok,
                         "PyWinRT ready" if ok else
                         "PyWinRT (winrt) not installed -> no speech. pip install "
                         "winrt-runtime winrt-Windows.Media.SpeechSynthesis "
                         "winrt-Windows.Storage.Streams"))
        except Exception as exc:  # noqa: BLE001 - doctor must always render
            rows.append(("TTS runtime", False, "error: {0}".format(exc)))

        # Daemon running
        running = self.is_running()
        rows.append(("daemon running", running,
                     "accepting connections" if running
                     else "not running (run 'sonari start')"))

        return rows

    def install(self, python: str, app_dir: str) -> None:
        # 1. Exec-form hooks FIRST. This is the step that can fail on a malformed
        #    user settings.json (it raises ValueError); doing it before the Task
        #    Scheduler registration means a failure leaves no orphaned autostart
        #    task behind (partial-install avoidance).
        #
        #    But ONLY when the sonari plugin is NOT enabled: an enabled plugin
        #    already supplies these exact hooks via its hooks/hooks.json, so also
        #    writing them to settings.json fires every event TWICE — each assistant
        #    message is then spoken twice (#44). When the plugin is on, heal any
        #    hooks a prior install left behind and write nothing new.
        settings = claude_settings_path()
        if settings_has_sonari_plugin(settings):
            if settings_has_sonari_hooks(settings):
                remove_hooks_from_settings(settings, _hook_py())
                print("Removed duplicate Sonari hooks from {0}; the enabled sonari "
                      "plugin already supplies them.".format(settings))
            else:
                print("Sonari plugin enabled; hooks come from the plugin "
                      "(nothing written to {0}).".format(settings))
        else:
            merge_hooks_into_settings(settings, python, _hook_py())
            print("Wrote Sonari hooks to: {0}".format(settings))
        # 2. Task Scheduler autostart (pythonw runs the supervisor loop).
        supervisor_py = os.path.join(app_dir, "sonari", "platform",
                                     "windows", "supervisor_loop.py")
        rc = task_install(python, supervisor_py)
        if rc == 0:
            print("Registered Task Scheduler task: {0}".format(TASK_NAME))
        else:
            print("warning: schtasks /create returned {0}; autostart may not be "
                  "registered.".format(rc))
        # 3. sonari.cmd launcher on ~/.local/bin.
        launcher = self._place_launcher(python, app_dir)
        print("Placed launcher: {0}".format(launcher))

    def _place_launcher(self, python: str, app_dir: str) -> str:
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

    def uninstall(self) -> None:
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

    def post_install_notes(self) -> None:
        """Print the Windows post-install next steps."""
        print("")
        print("Sonari is installed. Run 'sonari doctor' to confirm everything is green.")
        # #19: hotkeys now ship and start with the daemon (no longer "M3-pending").
        print("  - Global hotkeys start automatically with the daemon; "
              "run 'sonari keymap' to see the bindings.")
        # The plugin's command files were renamed to NTFS-safe names (status.md,
        # voice.md, ...), so the /sonari:* slash commands now work on Windows too.
        print("  - Enable the 'sonari' plugin for its /sonari:* slash commands "
              "(optional; speech and hotkeys work without it).")

    def hooks_doctor_row(self) -> tuple:
        """Windows: Sonari hooks come from EITHER a hand-wired settings.json block
        (written by 'sonari install') OR the enabled 'sonari' plugin (its
        hooks/hooks.json). For the settings.json path, go RED if a hook is present
        but its baked script path no longer exists — the stale-after-plugin-update
        case that otherwise stops speech silently while doctor stayed green (#8).
        Also go RED when BOTH sources are present: each event then fires twice and
        every message is spoken twice (#44); 're-run sonari install' heals it."""
        path = claude_settings_path()
        if settings_has_sonari_hooks(path) and settings_has_sonari_plugin(path):
            return ("hooks installed", False,
                    "hooks registered TWICE (settings.json + the sonari plugin) — "
                    "every message is spoken twice; re-run 'sonari install' to heal "
                    "(it drops the settings.json copy when the plugin is enabled)")
        if settings_has_sonari_hooks(path):
            missing = [p for p in _sonari_hook_paths(path)
                       if p and not os.path.exists(p)]
            if missing:
                return ("hooks installed", False,
                        "hook script missing: {0} (stale after a plugin update; "
                        "re-run 'sonari install')".format(missing[0]))
            return ("hooks installed", True, "{0} (settings.json)".format(path))
        if settings_has_sonari_plugin(path):
            return ("hooks installed", True, "via the sonari plugin")
        return ("hooks installed", False,
                "no Sonari hooks in {0} and the sonari plugin is not enabled "
                "(run 'sonari install', or enable the sonari plugin)".format(path))
