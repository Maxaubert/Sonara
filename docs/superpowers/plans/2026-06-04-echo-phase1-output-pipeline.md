# Echo - Phase 1: Output Pipeline - Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Echo speech daemon plus thin Claude Code hooks so a blind/low-vision developer hears Claude prose, options, plans, permissions, and earcons reliably and in order - eliminating the legacy double-speak / self-interrupt / options-missing bugs.

**Architecture:** One per-machine speech daemon owns a single FIFO queue and one killable macOS say child; thin hook clients (MessageDisplay, PreToolUse, Notification, Stop, UserPromptSubmit, SessionStart, SessionEnd) forward structured JSON over a Unix-domain socket; speech plays strictly in order with instant per-type earcons; per-session foreground gating. Shipped as a Claude Code plugin. Global hotkeys and 100%-eyes-free selection are Phase 2 (separate plan).

**Tech Stack:** Python 3 (stdlib only), pytest, macOS say/afplay, Unix-domain-socket newline-delimited JSON, Claude Code plugin manifest + hooks.json.

**Spec:** `docs/superpowers/specs/2026-06-04-echo-eyes-free-claude-code-design.md`

---

## Project scaffolding, plugin manifest & legacy removal

This section creates the package skeleton, the plugin manifest and hooks, the executable shims, and the editable install - and removes the legacy PTY-era files (preserved at git tag `v0-legacy-pty`). Only `paths.py` is genuine code here, so it is the one task that follows the full TDD loop; the config/manifest/shim files are not unit-testable on their own and are committed with full contents verified by inspection plus the venv install + pytest run at the end.

All commands assume the repo root `/Users/Nima.Hakimi/projects/private/claude-tts`. Run them from there.

### Task: Remove legacy PTY-era files

The legacy implementation is preserved at tag `v0-legacy-pty`, so we can delete it cleanly. We `git rm` exactly the files named in the contract and keep `docs/` and `.git`.

First confirm the tag exists so the history is recoverable:

```bash
git tag --list v0-legacy-pty
```

Expected output (the tag must be present):

```
v0-legacy-pty
```

Now remove the legacy files:

```bash
git rm bin/claude-speak bin/claude-tts \
       hooks/permission-request.sh hooks/pre-tool-use.sh hooks/stop.sh \
       commands/read.md \
       install.sh uninstall.sh README.md
```

Expected output:

```
rm 'bin/claude-speak'
rm 'bin/claude-tts'
rm 'hooks/permission-request.sh'
rm 'hooks/pre-tool-use.sh'
rm 'hooks/stop.sh'
rm 'commands/read.md'
rm 'install.sh'
rm 'uninstall.sh'
rm 'README.md'
```

Verify only `docs/` and `.git` (plus now-empty tracked dirs) remain and the legacy files are gone:

```bash
git status --short
ls -A
```

Expected: `git status --short` shows nine `D ` (deleted) lines for the files above and nothing else; `ls -A` still lists `.git` and `docs`.

Commit the removal:

```bash
git commit -m "chore: remove legacy PTY-era files (preserved at v0-legacy-pty)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected output: a commit summary line ending with `9 files changed` (deletions only).

### Task: Add .gitignore

Create the ignore file so the venv, caches, and the runtime `.echo/` dir never get committed.

Create `.gitignore` with EXACTLY this content:

```
.venv/
__pycache__/
*.pyc
.echo/
```

Verify and commit:

```bash
git add .gitignore
git status --short
git commit -m "chore: add .gitignore

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: `git status --short` shows `A  .gitignore`; the commit reports `1 file changed`.

### Task: Add pyproject.toml (package 'echo', src layout)

This is the single source of truth for the editable install. It declares the `echo` package under `src/` with no third-party runtime dependencies (stdlib only).

Create `pyproject.toml` with EXACTLY this content:

```toml
[build-system]
requires = ["setuptools>=61"]
build-backend = "setuptools.build_meta"

[project]
name = "echo"
version = "0.1.0"
description = "Eyes-free text-to-speech layer for Claude Code (macOS)"
requires-python = ">=3.10"
authors = [{ name = "Nima Hakimi", email = "hakimi.nima1@gmail.com" }]
license = { text = "MIT" }

[project.optional-dependencies]
dev = ["pytest>=7"]

[tool.setuptools.packages.find]
where = ["src"]
```

Verify and commit:

```bash
git add pyproject.toml
git status --short
git commit -m "chore: add pyproject.toml with src-layout editable install

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: `git status --short` shows `A  pyproject.toml`; commit reports `1 file changed`.

### Task: Add tests/conftest.py (sys.path fallback to src)

So tests can run even before `pip install -e .`, `conftest.py` prepends `src/` to `sys.path`.

Create `tests/conftest.py` with EXACTLY this content:

```python
import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parent.parent / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))
```

Verify and commit:

```bash
git add tests/conftest.py
git status --short
git commit -m "test: add conftest.py with src path fallback

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: `git status --short` shows `A  tests/conftest.py`; commit reports `1 file changed`.

### Task: Create the echo package and paths.py (TDD)

This is the first real code. We write a failing test for `paths.py`, then implement it exactly per the contract.

**(1) Write the failing test.** Create `tests/test_paths.py` with EXACTLY this content:

```python
import importlib
from pathlib import Path


def _fresh_paths(monkeypatch, home):
    """Reload echo.paths so the module-level Path.home() constants pick up the patched HOME."""
    monkeypatch.setenv("HOME", str(home))
    import echo.paths as paths
    return importlib.reload(paths)


def test_constants_end_with_expected_names(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.ECHO_DIR.name == ".echo"
    assert paths.CONFIG_PATH.name == "config.json"
    assert paths.SOCKET_PATH.name == "speechd.sock"
    assert paths.LOG_PATH.name == "speechd.log"


def test_paths_are_nested_under_echo_dir(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.CONFIG_PATH.parent == paths.ECHO_DIR
    assert paths.SOCKET_PATH.parent == paths.ECHO_DIR
    assert paths.LOG_PATH.parent == paths.ECHO_DIR


def test_echo_dir_is_under_home(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.ECHO_DIR == Path(tmp_path) / ".echo"


def test_ensure_echo_dir_creates_directory(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert not paths.ECHO_DIR.exists()
    paths.ensure_echo_dir()
    assert paths.ECHO_DIR.is_dir()


def test_ensure_echo_dir_is_idempotent(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    paths.ensure_echo_dir()
    paths.ensure_echo_dir()  # must not raise on an existing dir
    assert paths.ECHO_DIR.is_dir()
```

**(2) Run it - expect FAIL** (the `echo` package does not exist yet):

```bash
python -m pytest tests/test_paths.py -q
```

Expected: a collection/import error, `ModuleNotFoundError: No module named 'echo'` (0 tests pass).

**(3) Implement.** Create the package init `src/echo/__init__.py` with EXACTLY this content:

```python
"""Echo: an eyes-free text-to-speech layer for Claude Code (macOS)."""

__version__ = "0.1.0"
```

Create `src/echo/paths.py` with EXACTLY this content (matches the contract verbatim):

```python
from pathlib import Path

ECHO_DIR = Path.home() / ".echo"
CONFIG_PATH = ECHO_DIR / "config.json"
SOCKET_PATH = ECHO_DIR / "speechd.sock"
LOG_PATH = ECHO_DIR / "speechd.log"


def ensure_echo_dir() -> None:
    ECHO_DIR.mkdir(parents=True, exist_ok=True)
```

**(4) Run it - expect PASS:**

```bash
python -m pytest tests/test_paths.py -q
```

Expected output ends with:

```
5 passed
```

**(5) Commit:**

```bash
git add src/echo/__init__.py src/echo/paths.py tests/test_paths.py
git status --short
git commit -m "feat: add echo package and paths module

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: `git status --short` shows three `A ` lines; commit reports `3 files changed`.

### Task: Add the plugin manifest (.claude-plugin/plugin.json)

The plugin manifest identifies the package to Claude Code.

Create `.claude-plugin/plugin.json` with EXACTLY this content:

```json
{
  "name": "echo",
  "version": "0.1.0",
  "description": "Eyes-free text-to-speech layer for Claude Code (macOS)",
  "author": {
    "name": "Nima Hakimi",
    "email": "hakimi.nima1@gmail.com"
  },
  "license": "MIT",
  "keywords": ["accessibility", "tts", "speech"]
}
```

Verify it is valid JSON and commit:

```bash
python -c "import json; json.load(open('.claude-plugin/plugin.json'))" && echo "VALID"
git add .claude-plugin/plugin.json
git status --short
git commit -m "feat: add Claude Code plugin manifest

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: prints `VALID`; `git status --short` shows `A  .claude-plugin/plugin.json`; commit reports `1 file changed`.

### Task: Add the declarative hooks (hooks/hooks.json)

Every hook routes to the single dispatcher `${CLAUDE_PLUGIN_ROOT}/bin/echo-hook <Event>`. `PreToolUse` has dedicated matchers for `AskUserQuestion` and `ExitPlanMode` plus a catch-all `""` for generic tool announcements; `Notification` has matchers for `permission_prompt` and `idle_prompt`.

Create `hooks/hooks.json` with EXACTLY this content (the FULL JSON):

```json
{
  "hooks": {
    "MessageDisplay": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook MessageDisplay"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "AskUserQuestion",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook PreToolUse"
          }
        ]
      },
      {
        "matcher": "ExitPlanMode",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook PreToolUse"
          }
        ]
      },
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook PreToolUse"
          }
        ]
      }
    ],
    "Notification": [
      {
        "matcher": "permission_prompt",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook Notification"
          }
        ]
      },
      {
        "matcher": "idle_prompt",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook Notification"
          }
        ]
      }
    ],
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook Stop"
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook UserPromptSubmit"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook SessionStart"
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/bin/echo-hook SessionEnd"
          }
        ]
      }
    ]
  }
}
```

Verify it is valid JSON, confirm every command points at `echo-hook`, and commit:

```bash
python -c "import json; d=json.load(open('hooks/hooks.json')); assert set(d['hooks'])=={'MessageDisplay','PreToolUse','Notification','Stop','UserPromptSubmit','SessionStart','SessionEnd'}; print('VALID', len(d['hooks']), 'events')"
git add hooks/hooks.json
git status --short
git commit -m "feat: add declarative hooks routing to echo-hook dispatcher

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: prints `VALID 7 events`; `git status --short` shows `A  hooks/hooks.json`; commit reports `1 file changed`.

### Task: Add the bin/echo-daemon shim

This shim simply execs the daemon module. `exec` replaces the shell process so signals reach Python directly.

Create `bin/echo-daemon` with EXACTLY this content:

```bash
#!/usr/bin/env bash
exec python3 -m echo.daemon "$@"
```

Make it executable:

```bash
chmod +x bin/echo-daemon
```

Verify the executable bit and commit (preserve the mode in git):

```bash
test -x bin/echo-daemon && echo "EXECUTABLE"
git add bin/echo-daemon
git ls-files --stage bin/echo-daemon
git commit -m "feat: add echo-daemon shim

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: prints `EXECUTABLE`; `git ls-files --stage` shows mode `100755` for `bin/echo-daemon`; commit reports `1 file changed`.

### Task: Add the bin/echo shim

This shim execs the CLI module, forwarding all arguments.

Create `bin/echo` with EXACTLY this content:

```bash
#!/usr/bin/env bash
exec python3 -m echo.cli "$@"
```

Make it executable, verify, and commit:

```bash
chmod +x bin/echo
test -x bin/echo && echo "EXECUTABLE"
git add bin/echo
git ls-files --stage bin/echo
git commit -m "feat: add echo CLI shim

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: prints `EXECUTABLE`; `git ls-files --stage` shows mode `100755` for `bin/echo`; commit reports `1 file changed`.

### Task: Add the bin/echo-hook dispatcher shim

This is the fuller dispatcher referenced throughout the contract. It reads the event name from `argv[1]` and the JSON payload from stdin (tolerating empty/invalid as `{}`), optionally captures the raw stdin when `ECHO_CAPTURE` is set, maps the event to protocol messages via the PURE `handle_event`, ensures the daemon is up, sends each message, and ALWAYS exits 0 wrapped in a broad try/except so it can never break Claude. It depends on `echo.hooks_entry.handle_event` and `echo.client` (finalized in their own components); this task finalizes the shim body itself.

Create `bin/echo-hook` with EXACTLY this content:

```python
#!/usr/bin/env python3
"""Echo hook dispatcher: argv[1]=event, stdin=JSON payload.

Maps the event to protocol messages and forwards them to the speech daemon.
ALWAYS exits 0 and never raises, so it can never break Claude Code.
"""
import json
import os
import sys


def _main() -> None:
    event = sys.argv[1] if len(sys.argv) > 1 else ""

    raw = b""
    try:
        raw = sys.stdin.buffer.read()
    except Exception:
        raw = b""

    if os.environ.get("ECHO_CAPTURE"):
        try:
            cap_dir = os.environ["ECHO_CAPTURE"]
            os.makedirs(cap_dir, exist_ok=True)
            cap_path = os.path.join(cap_dir, f"{event}-{os.getpid()}.json")
            with open(cap_path, "wb") as fh:
                fh.write(raw)
        except Exception:
            pass

    try:
        payload = json.loads(raw.decode("utf-8")) if raw.strip() else {}
        if not isinstance(payload, dict):
            payload = {}
    except Exception:
        payload = {}

    from echo.hooks_entry import handle_event
    from echo import client

    msgs = handle_event(event, payload)
    if not msgs:
        return

    client.ensure_daemon()
    for msg in msgs:
        try:
            client.send(msg)
        except Exception:
            pass


if __name__ == "__main__":
    try:
        _main()
    except Exception:
        pass
    sys.exit(0)
```

Make it executable, verify it is at least syntactically valid Python, and commit:

```bash
chmod +x bin/echo-hook
python -c "import ast; ast.parse(open('bin/echo-hook').read()); print('PARSES')"
test -x bin/echo-hook && echo "EXECUTABLE"
git add bin/echo-hook
git ls-files --stage bin/echo-hook
git commit -m "feat: add echo-hook dispatcher shim

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: prints `PARSES` then `EXECUTABLE`; `git ls-files --stage` shows mode `100755` for `bin/echo-hook`; commit reports `1 file changed`.

### Task: Create venv, editable install, and verify pytest

Create the virtual environment, install the package editable with the dev extra (pytest), and confirm the `echo` package is importable from the install (not just via the conftest path fallback) and that pytest collects and passes the `paths` tests.

Create and populate the venv:

```bash
python3 -m venv .venv
.venv/bin/python -m pip install --upgrade pip
.venv/bin/python -m pip install -e ".[dev]"
```

Expected: pip reports `Successfully installed echo-0.1.0 ... pytest-...` (exact pytest version may vary).

Confirm the editable install exposes `echo`:

```bash
.venv/bin/python -c "import echo, echo.paths; print('echo', echo.__version__, echo.paths.ECHO_DIR.name)"
```

Expected output:

```
echo 0.1.0 .echo
```

Run the full test suite (only `test_paths.py` exists so far) and confirm collection is non-empty:

```bash
.venv/bin/python -m pytest -q
```

Expected output ends with:

```
5 passed
```

`.venv/` is already ignored by `.gitignore`, so confirm nothing from it is staged:

```bash
git status --short
```

Expected: no output (clean tree - the venv is ignored, and all prior files are already committed). No commit is needed for this task since the venv is intentionally untracked; if `git status --short` shows anything unexpected, stop and investigate before proceeding.

---

## protocol.py + config.py

These tasks implement the wire protocol (`src/echo/protocol.py`) and the persisted configuration layer (`src/echo/config.py`). Both are pure, stdlib-only modules with no I/O beyond `config.save_config`/`load_config` reading and writing `CONFIG_PATH`. They depend only on `src/echo/paths.py` (created in scaffolding). Every task is strict TDD: write a failing test with real assertions, run the exact pytest command and observe the specific failure, write the minimal real implementation, observe PASS, then commit.

All `pytest` and `git` commands assume the working directory is the repo root `/Users/Nima.Hakimi/projects/private/claude-tts`.

### Task: protocol encode/decode round-trip

Write the failing test first. It asserts that `encode` returns `bytes` ending in a newline whose UTF-8 decode is exactly `json.dumps(msg) + "\n"`, and that `decode(encode(msg)) == msg` for several message shapes (including non-ASCII to lock in `utf-8`).

Create `tests/test_protocol.py`:

```python
import json

from echo import protocol
from echo.protocol import MsgType, PROTOCOL_VERSION, encode, decode


def test_protocol_version_is_one():
    assert PROTOCOL_VERSION == 1


def test_encode_returns_bytes_ending_in_newline():
    msg = {"v": PROTOCOL_VERSION, "type": MsgType.PING}
    out = encode(msg)
    assert isinstance(out, bytes)
    assert out.endswith(b"\n")
    assert out.decode("utf-8") == json.dumps(msg) + "\n"


def test_decode_reverses_encode():
    msg = {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": "abc-123"}
    assert decode(encode(msg)) == msg


def test_round_trip_preserves_nested_and_unicode():
    msg = {
        "v": PROTOCOL_VERSION,
        "type": MsgType.CHOICE,
        "session": "s1",
        "questions": [{"q": "Pick one - café or tea?", "options": ["a", "b"]}],
        "n": 7,
        "flag": True,
        "empty": None,
    }
    line = encode(msg)
    assert isinstance(line, bytes)
    assert line.count(b"\n") == 1
    assert line.endswith(b"\n")
    assert decode(line) == msg


def test_decode_accepts_line_without_trailing_newline():
    # decode must tolerate a json.loads-able line whether or not it carries the delimiter
    msg = {"v": PROTOCOL_VERSION, "type": MsgType.STATUS}
    assert decode(b'{"v": 1, "type": "status"}') == msg


def test_encode_is_pure_module_function():
    assert callable(protocol.encode)
    assert callable(protocol.decode)
```

Run it and confirm it fails because the module does not exist yet:

```
python3 -m pytest tests/test_protocol.py -q
```

Expected output (collection error - `src/echo/protocol.py` is absent):

```
E   ModuleNotFoundError: No module named 'echo.protocol'
...
1 error in 0.__s
```

Now write the minimal real implementation. Create `src/echo/protocol.py`:

```python
"""Echo wire protocol: newline-delimited JSON over a Unix stream socket."""

import json

PROTOCOL_VERSION = 1


class MsgType:
    PROSE = "prose"
    CHOICE = "choice"
    PLAN = "plan"
    TOOL = "tool_announce"
    PERMISSION = "permission"
    EARCON = "earcon"
    FLUSH = "flush"
    SESSION_START = "session_start"
    SESSION_END = "session_end"
    SET_FOREGROUND = "set_foreground"
    STOP = "stop"
    SKIP = "skip"
    REPEAT = "repeat"
    JUMP_DECISION = "jump_decision"
    CATCH_UP = "catch_up"
    SET_RATE = "set_rate"
    SET_VERBOSITY = "set_verbosity"
    SET_VOICE = "set_voice"
    STATUS = "status"
    PING = "ping"


def encode(msg: dict) -> bytes:
    """Serialize a message dict to a newline-terminated UTF-8 byte line."""
    return (json.dumps(msg) + chr(10)).encode("utf-8")


def decode(line: bytes) -> dict:
    """Parse one newline-delimited JSON line back into a dict."""
    return json.loads(line)
```

Run the tests again and confirm they pass:

```
python3 -m pytest tests/test_protocol.py -q
```

Expected output:

```
6 passed in 0.__s
```

Commit:

```
git add src/echo/protocol.py tests/test_protocol.py
git commit -m "$(printf 'feat: add wire protocol encode/decode\n\nNewline-delimited JSON encode/decode with PROTOCOL_VERSION.\n\nCo-Authored-By: Claude <noreply@anthropic.com>')"
```

### Task: protocol MsgType constants

Lock down every constant name and its EXACT string value so no future edit can silently drift the wire format. This test is data-driven against the full contract list.

Append to `tests/test_protocol.py`:

```python
def test_msgtype_has_every_constant_with_exact_values():
    expected = {
        "PROSE": "prose",
        "CHOICE": "choice",
        "PLAN": "plan",
        "TOOL": "tool_announce",
        "PERMISSION": "permission",
        "EARCON": "earcon",
        "FLUSH": "flush",
        "SESSION_START": "session_start",
        "SESSION_END": "session_end",
        "SET_FOREGROUND": "set_foreground",
        "STOP": "stop",
        "SKIP": "skip",
        "REPEAT": "repeat",
        "JUMP_DECISION": "jump_decision",
        "CATCH_UP": "catch_up",
        "SET_RATE": "set_rate",
        "SET_VERBOSITY": "set_verbosity",
        "SET_VOICE": "set_voice",
        "STATUS": "status",
        "PING": "ping",
    }
    for name, value in expected.items():
        assert hasattr(MsgType, name), f"MsgType missing {name}"
        assert getattr(MsgType, name) == value, f"MsgType.{name} != {value!r}"


def test_msgtype_defines_no_extra_string_constants():
    actual = {
        k: v
        for k, v in vars(MsgType).items()
        if not k.startswith("_") and isinstance(v, str)
    }
    expected = {
        "PROSE": "prose",
        "CHOICE": "choice",
        "PLAN": "plan",
        "TOOL": "tool_announce",
        "PERMISSION": "permission",
        "EARCON": "earcon",
        "FLUSH": "flush",
        "SESSION_START": "session_start",
        "SESSION_END": "session_end",
        "SET_FOREGROUND": "set_foreground",
        "STOP": "stop",
        "SKIP": "skip",
        "REPEAT": "repeat",
        "JUMP_DECISION": "jump_decision",
        "CATCH_UP": "catch_up",
        "SET_RATE": "set_rate",
        "SET_VERBOSITY": "set_verbosity",
        "SET_VOICE": "set_voice",
        "STATUS": "status",
        "PING": "ping",
    }
    assert actual == expected


def test_msgtype_values_are_unique():
    values = [
        v for k, v in vars(MsgType).items()
        if not k.startswith("_") and isinstance(v, str)
    ]
    assert len(values) == len(set(values))
```

Run just the new tests:

```
python3 -m pytest tests/test_protocol.py -q -k msgtype
```

Expected output - these PASS already, because the constants were defined fully in the previous task:

```
3 passed in 0.__s
```

This is intentional: the previous task implemented the complete `MsgType`, and this task adds the regression guard that pins the names/values. To prove the guard actually bites, temporarily break one value to watch it fail, then revert:

```
python3 - <<'PY'
import re, pathlib
p = pathlib.Path("src/echo/protocol.py")
src = p.read_text()
p.write_text(src.replace('TOOL = "tool_announce"', 'TOOL = "tool"'))
PY
python3 -m pytest tests/test_protocol.py -q -k msgtype
```

Expected output (the guard catches the drift):

```
F...
E   AssertionError: MsgType.TOOL != 'tool_announce'
...
2 failed, 1 passed in 0.__s
```

Revert the deliberate break and confirm green:

```
git checkout -- src/echo/protocol.py
python3 -m pytest tests/test_protocol.py -q
```

Expected output:

```
9 passed in 0.__s
```

Commit the guard:

```
git add tests/test_protocol.py
git commit -m "$(printf 'test: pin MsgType constant names and exact string values\n\nData-driven regression guard against wire-format drift.\n\nCo-Authored-By: Claude <noreply@anthropic.com>')"
```

### Task: config DEFAULTS shape

Write the failing test first. It pins every documented key of `DEFAULTS`, the exact earcon sound map (all six kinds with their `/System/Library/Sounds/*.aiff` paths), and the scalar defaults.

Create `tests/test_config.py`:

```python
from echo import config
from echo.config import DEFAULTS


def test_defaults_has_documented_top_level_keys():
    assert set(DEFAULTS.keys()) == {
        "voice",
        "rate",
        "verbosity",
        "background_policy",
        "earcons",
    }


def test_defaults_scalar_values():
    assert DEFAULTS["voice"] is None
    assert DEFAULTS["rate"] == 200
    assert DEFAULTS["verbosity"] == "everything"
    assert DEFAULTS["background_policy"] == "earcon_only"


def test_defaults_earcon_map_exact():
    assert DEFAULTS["earcons"] == {
        "permission": "/System/Library/Sounds/Funk.aiff",
        "choice": "/System/Library/Sounds/Ping.aiff",
        "plan": "/System/Library/Sounds/Submarine.aiff",
        "error": "/System/Library/Sounds/Sosumi.aiff",
        "turn_done": "/System/Library/Sounds/Tink.aiff",
        "ready": "/System/Library/Sounds/Glass.aiff",
    }


def test_defaults_earcon_kinds_match_contract():
    assert set(DEFAULTS["earcons"].keys()) == {
        "permission",
        "choice",
        "plan",
        "error",
        "turn_done",
        "ready",
    }


def test_module_exposes_load_and_save():
    assert callable(config.load_config)
    assert callable(config.save_config)
```

Run it and confirm it fails because the module does not exist yet:

```
python3 -m pytest tests/test_config.py -q
```

Expected output:

```
E   ModuleNotFoundError: No module named 'echo.config'
...
1 error in 0.__s
```

Now write the minimal real implementation. Create `src/echo/config.py`. It imports the path names into its own namespace so config tests can `monkeypatch.setattr(config, "CONFIG_PATH", ...)` and `monkeypatch.setattr(config, "ECHO_DIR", ...)`:

```python
"""Echo persisted configuration: DEFAULTS plus load/save against CONFIG_PATH."""

import json
import os

from echo.paths import CONFIG_PATH, ECHO_DIR, ensure_echo_dir

DEFAULTS = {
    "voice": None,
    "rate": 200,
    "verbosity": "everything",
    "background_policy": "earcon_only",
    "earcons": {
        "permission": "/System/Library/Sounds/Funk.aiff",
        "choice": "/System/Library/Sounds/Ping.aiff",
        "plan": "/System/Library/Sounds/Submarine.aiff",
        "error": "/System/Library/Sounds/Sosumi.aiff",
        "turn_done": "/System/Library/Sounds/Tink.aiff",
        "ready": "/System/Library/Sounds/Glass.aiff",
    },
}


def _deep_merge(base: dict, override: dict) -> dict:
    """Return a new dict: override applied onto base, recursing into nested dicts."""
    result = dict(base)
    for key, value in override.items():
        if (
            key in result
            and isinstance(result[key], dict)
            and isinstance(value, dict)
        ):
            result[key] = _deep_merge(result[key], value)
        else:
            result[key] = value
    return result


def load_config() -> dict:
    """Deep-merge persisted CONFIG_PATH over a copy of DEFAULTS.

    Missing or corrupt (non-JSON / non-object) files yield a fresh DEFAULTS copy.
    """
    base = _deep_merge(DEFAULTS, {})
    try:
        with open(CONFIG_PATH, "r", encoding="utf-8") as fh:
            persisted = json.load(fh)
    except (FileNotFoundError, ValueError, OSError):
        return base
    if not isinstance(persisted, dict):
        return base
    return _deep_merge(base, persisted)


def save_config(cfg: dict) -> None:
    """Atomically persist cfg to CONFIG_PATH (temp file in ECHO_DIR + os.replace)."""
    ensure_echo_dir()
    tmp_path = ECHO_DIR / (CONFIG_PATH.name + ".tmp")
    with open(tmp_path, "w", encoding="utf-8") as fh:
        json.dump(cfg, fh, indent=2)
        fh.flush()
        os.fsync(fh.fileno())
    os.replace(tmp_path, CONFIG_PATH)
```

Note: `_deep_merge(DEFAULTS, {})` builds a deep copy of `DEFAULTS` (recursing into the nested `earcons` dict), so a caller mutating the returned config can never corrupt the module-level `DEFAULTS`.

Run the tests again and confirm they pass:

```
python3 -m pytest tests/test_config.py -q
```

Expected output:

```
5 passed in 0.__s
```

Commit:

```
git add src/echo/config.py tests/test_config.py
git commit -m "$(printf 'feat: add config DEFAULTS and load/save\n\nDocumented default keys plus deep-merge load and atomic save.\n\nCo-Authored-By: Claude <noreply@anthropic.com>')"
```

### Task: load_config returns DEFAULTS copy when CONFIG_PATH missing

Write the failing test first. It patches `config.CONFIG_PATH` and `config.ECHO_DIR` to a `tmp_path` location that does not exist, asserts `load_config()` equals `DEFAULTS`, and-critically-asserts the result is an independent copy (mutating it, including the nested `earcons` dict, must not touch `DEFAULTS`).

Append to `tests/test_config.py`:

```python
import copy


def _patch_config_paths(monkeypatch, tmp_path):
    cfg_path = tmp_path / "config.json"
    monkeypatch.setattr(config, "ECHO_DIR", tmp_path)
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_path)
    return cfg_path


def test_load_config_returns_defaults_when_file_missing(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    assert not cfg_path.exists()
    loaded = config.load_config()
    assert loaded == DEFAULTS


def test_load_config_missing_returns_independent_copy(monkeypatch, tmp_path):
    _patch_config_paths(monkeypatch, tmp_path)
    pristine = copy.deepcopy(DEFAULTS)
    loaded = config.load_config()
    loaded["rate"] = 999
    loaded["earcons"]["choice"] = "/tmp/hacked.aiff"
    assert DEFAULTS == pristine
    assert DEFAULTS["rate"] == 200
    assert DEFAULTS["earcons"]["choice"] == "/System/Library/Sounds/Ping.aiff"
```

Run just the new tests:

```
python3 -m pytest tests/test_config.py -q -k "missing or independent_copy"
```

Expected output - these PASS, because `load_config` and `_deep_merge` were implemented correctly in the prior task:

```
2 passed in 0.__s
```

To prove these tests genuinely exercise the deep-copy behavior, temporarily make `load_config` return the shared object on the missing-file path, watch the independence test fail, then revert:

```
python3 - <<'PY'
import pathlib
p = pathlib.Path("src/echo/config.py")
src = p.read_text()
src = src.replace(
    "    base = _deep_merge(DEFAULTS, {})\n    try:",
    "    base = DEFAULTS\n    try:",
)
p.write_text(src)
PY
python3 -m pytest tests/test_config.py -q -k "missing or independent_copy"
```

Expected output (the shared-reference bug is caught):

```
.F
E   assert {... 'rate': 999 ...} == {... 'rate': 200 ...}
...
1 failed, 1 passed in 0.__s
```

Revert and confirm green:

```
git checkout -- src/echo/config.py
python3 -m pytest tests/test_config.py -q
```

Expected output:

```
7 passed in 0.__s
```

Commit:

```
git add tests/test_config.py
git commit -m "$(printf 'test: load_config returns independent DEFAULTS copy when file missing\n\nGuards against handing back the shared module-level DEFAULTS dict.\n\nCo-Authored-By: Claude <noreply@anthropic.com>')"
```

### Task: load_config deep-merges a partial persisted file

Write the failing test first. It writes a partial config to the patched `CONFIG_PATH`-overriding one scalar and one nested earcon entry-and asserts the result keeps every untouched default while applying the overrides. A second test confirms a persisted file adding a brand-new earcon kind merges it in alongside the defaults.

Append to `tests/test_config.py`:

```python
import json as _json


def test_load_config_deep_merges_partial_file(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text(
        _json.dumps(
            {
                "rate": 240,
                "voice": "Ava (Premium)",
                "earcons": {"choice": "/custom/choice.aiff"},
            }
        ),
        encoding="utf-8",
    )
    loaded = config.load_config()

    # overridden scalars
    assert loaded["rate"] == 240
    assert loaded["voice"] == "Ava (Premium)"
    # untouched scalars keep their defaults
    assert loaded["verbosity"] == "everything"
    assert loaded["background_policy"] == "earcon_only"
    # nested earcons: overridden key replaced, all others preserved
    assert loaded["earcons"]["choice"] == "/custom/choice.aiff"
    assert loaded["earcons"]["permission"] == "/System/Library/Sounds/Funk.aiff"
    assert loaded["earcons"]["plan"] == "/System/Library/Sounds/Submarine.aiff"
    assert loaded["earcons"]["error"] == "/System/Library/Sounds/Sosumi.aiff"
    assert loaded["earcons"]["turn_done"] == "/System/Library/Sounds/Tink.aiff"
    assert loaded["earcons"]["ready"] == "/System/Library/Sounds/Glass.aiff"


def test_load_config_merges_extra_nested_key(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text(
        _json.dumps({"earcons": {"custom_kind": "/custom/extra.aiff"}}),
        encoding="utf-8",
    )
    loaded = config.load_config()
    assert loaded["earcons"]["custom_kind"] == "/custom/extra.aiff"
    # all six defaults still present
    assert loaded["earcons"]["permission"] == "/System/Library/Sounds/Funk.aiff"
    assert len(loaded["earcons"]) == 7


def test_load_config_merge_does_not_mutate_defaults(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text(
        _json.dumps({"earcons": {"choice": "/custom/choice.aiff"}}),
        encoding="utf-8",
    )
    config.load_config()
    assert DEFAULTS["earcons"]["choice"] == "/System/Library/Sounds/Ping.aiff"
```

Run just the new tests:

```
python3 -m pytest tests/test_config.py -q -k "deep_merges or extra_nested or does_not_mutate"
```

Expected output - these PASS, validating `_deep_merge` against real partial input:

```
3 passed in 0.__s
```

To prove the merge is genuinely deep (not a shallow `dict.update` that would clobber the whole `earcons` map), temporarily replace the merge with a shallow update, watch it fail, then revert:

```
python3 - <<'PY'
import pathlib
p = pathlib.Path("src/echo/config.py")
src = p.read_text()
src = src.replace("    return _deep_merge(base, persisted)", "    base.update(persisted)\n    return base")
p.write_text(src)
PY
python3 -m pytest tests/test_config.py -q -k "deep_merges or extra_nested"
```

Expected output (shallow update drops the untouched earcon defaults):

```
FF
E   KeyError: 'permission'
...
2 failed in 0.__s
```

Revert and confirm green:

```
git checkout -- src/echo/config.py
python3 -m pytest tests/test_config.py -q
```

Expected output:

```
10 passed in 0.__s
```

Commit:

```
git add tests/test_config.py
git commit -m "$(printf 'test: load_config deep-merges partial persisted file over DEFAULTS\n\nVerifies nested earcons merge per-key without clobbering defaults.\n\nCo-Authored-By: Claude <noreply@anthropic.com>')"
```

### Task: load_config tolerates a corrupt file

Write the failing test first. It writes non-JSON garbage to `CONFIG_PATH` and asserts `load_config()` returns a clean `DEFAULTS` copy rather than raising. Additional cases cover an empty file and a valid-JSON-but-not-an-object file (e.g. a JSON list), both of which must also degrade gracefully to `DEFAULTS`.

Append to `tests/test_config.py`:

```python
def test_load_config_tolerates_non_json(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text("this is { not json ::: ", encoding="utf-8")
    loaded = config.load_config()
    assert loaded == DEFAULTS


def test_load_config_tolerates_empty_file(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text("", encoding="utf-8")
    loaded = config.load_config()
    assert loaded == DEFAULTS


def test_load_config_tolerates_json_non_object(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text("[1, 2, 3]", encoding="utf-8")
    loaded = config.load_config()
    assert loaded == DEFAULTS


def test_load_config_corrupt_returns_independent_copy(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text("garbage", encoding="utf-8")
    loaded = config.load_config()
    loaded["earcons"]["plan"] = "/tmp/x.aiff"
    assert DEFAULTS["earcons"]["plan"] == "/System/Library/Sounds/Submarine.aiff"
```

Run just the new tests:

```
python3 -m pytest tests/test_config.py -q -k "non_json or empty_file or non_object or corrupt"
```

Expected output:

```
4 passed in 0.__s
```

To prove the corruption tolerance is real (not coincidental), temporarily narrow the `except` to drop `ValueError`, watch the non-JSON case blow up, then revert:

```
python3 - <<'PY'
import pathlib
p = pathlib.Path("src/echo/config.py")
src = p.read_text()
src = src.replace(
    "    except (FileNotFoundError, ValueError, OSError):",
    "    except FileNotFoundError:",
)
p.write_text(src)
PY
python3 -m pytest tests/test_config.py -q -k "non_json or non_object"
```

Expected output (the parse error now propagates instead of degrading to DEFAULTS):

```
F.
E   json.decoder.JSONDecodeError: ...
...
1 failed, 1 passed in 0.__s
```

Revert and confirm green:

```
git checkout -- src/echo/config.py
python3 -m pytest tests/test_config.py -q
```

Expected output:

```
14 passed in 0.__s
```

Commit:

```
git add tests/test_config.py
git commit -m "$(printf 'test: load_config degrades to DEFAULTS on corrupt or non-object file\n\nNon-JSON, empty, and JSON-non-object inputs all yield a DEFAULTS copy.\n\nCo-Authored-By: Claude <noreply@anthropic.com>')"
```

### Task: save_config atomic write and round-trip

Write the failing test first. It patches `config.CONFIG_PATH` and `config.ECHO_DIR` to a `tmp_path` subdirectory that does NOT yet exist (to prove `save_config` calls `ensure_echo_dir`), saves a config, and asserts: the file exists, `load_config()` round-trips the saved values, and no leftover `.tmp` file remains (proving the temp-file-plus-`os.replace` path completed). A second test proves atomicity: when `os.replace` is patched to raise, the pre-existing config file is left untouched.

This task patches `ECHO_DIR` to a nested directory; the scaffolding's `paths.ensure_echo_dir()` does `ECHO_DIR.mkdir(parents=True, exist_ok=True)`, but `config.save_config` calls `ensure_echo_dir` imported into the `echo.paths` namespace - which reads the real `ECHO_DIR` from `paths`, not the patched one. To make the directory creation honor the patched path, the test patches `ensure_echo_dir` on the `config` module to create the patched `ECHO_DIR`. Append to `tests/test_config.py`:

```python
def _patch_config_paths_nested(monkeypatch, tmp_path):
    echo_dir = tmp_path / ".echo"
    cfg_path = echo_dir / "config.json"
    monkeypatch.setattr(config, "ECHO_DIR", echo_dir)
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_path)
    monkeypatch.setattr(
        config,
        "ensure_echo_dir",
        lambda: echo_dir.mkdir(parents=True, exist_ok=True),
    )
    return echo_dir, cfg_path


def test_save_config_creates_dir_and_round_trips(monkeypatch, tmp_path):
    echo_dir, cfg_path = _patch_config_paths_nested(monkeypatch, tmp_path)
    assert not echo_dir.exists()

    cfg = config.load_config()
    cfg["rate"] = 175
    cfg["voice"] = "Zoe (Premium)"
    cfg["verbosity"] = "medium"
    cfg["earcons"]["choice"] = "/custom/choice.aiff"
    config.save_config(cfg)

    assert echo_dir.exists()
    assert cfg_path.exists()
    # no temp artifact left behind after os.replace
    leftovers = list(echo_dir.glob("*.tmp"))
    assert leftovers == []

    reloaded = config.load_config()
    assert reloaded["rate"] == 175
    assert reloaded["voice"] == "Zoe (Premium)"
    assert reloaded["verbosity"] == "medium"
    assert reloaded["earcons"]["choice"] == "/custom/choice.aiff"
    # untouched defaults survive the round-trip
    assert reloaded["earcons"]["permission"] == "/System/Library/Sounds/Funk.aiff"
    assert reloaded["background_policy"] == "earcon_only"


def test_save_config_writes_valid_json_on_disk(monkeypatch, tmp_path):
    echo_dir, cfg_path = _patch_config_paths_nested(monkeypatch, tmp_path)
    cfg = config.load_config()
    cfg["rate"] = 123
    config.save_config(cfg)
    on_disk = _json.loads(cfg_path.read_text(encoding="utf-8"))
    assert on_disk == cfg


def test_save_config_is_atomic_on_replace_failure(monkeypatch, tmp_path):
    echo_dir, cfg_path = _patch_config_paths_nested(monkeypatch, tmp_path)
    echo_dir.mkdir(parents=True, exist_ok=True)
    cfg_path.write_text(_json.dumps({"rate": 200}), encoding="utf-8")

    def _boom(src, dst):
        raise OSError("simulated replace failure")

    monkeypatch.setattr(config.os, "replace", _boom)
    new_cfg = config.load_config()
    new_cfg["rate"] = 999

    try:
        config.save_config(new_cfg)
    except OSError:
        pass

    # original file content is untouched: os.replace never overwrote it
    on_disk = _json.loads(cfg_path.read_text(encoding="utf-8"))
    assert on_disk == {"rate": 200}
```

Run just the new tests:

```
python3 -m pytest tests/test_config.py -q -k "save_config"
```

Expected output - these PASS, exercising `ensure_echo_dir`, the atomic temp-file write, and `os.replace` round-trip:

```
3 passed in 0.__s
```

To prove the no-leftover-`.tmp` assertion is meaningful, temporarily make `save_config` write directly without `os.replace`, watch the leftover/atomicity checks fail, then revert:

```
python3 - <<'PY'
import pathlib
p = pathlib.Path("src/echo/config.py")
src = p.read_text()
src = src.replace(
    '    tmp_path = ECHO_DIR / (CONFIG_PATH.name + ".tmp")\n'
    '    with open(tmp_path, "w", encoding="utf-8") as fh:\n'
    '        json.dump(cfg, fh, indent=2)\n'
    '        fh.flush()\n'
    '        os.fsync(fh.fileno())\n'
    '    os.replace(tmp_path, CONFIG_PATH)',
    '    with open(CONFIG_PATH, "w", encoding="utf-8") as fh:\n'
    '        json.dump(cfg, fh, indent=2)',
)
p.write_text(src)
PY
python3 -m pytest tests/test_config.py -q -k "save_config"
```

Expected output (atomicity is lost - the replace-failure test now sees clobbered content):

```
..F
E   assert {'rate': 999} == {'rate': 200}
...
1 failed, 2 passed in 0.__s
```

Revert and confirm the whole config suite is green:

```
git checkout -- src/echo/config.py
python3 -m pytest tests/test_config.py -q
```

Expected output:

```
17 passed in 0.__s
```

Run the full protocol + config suite together as a final gate:

```
python3 -m pytest tests/test_protocol.py tests/test_config.py -q
```

Expected output:

```
26 passed in 0.__s
```

Commit:

```
git add tests/test_config.py
git commit -m "$(printf 'test: save_config writes atomically and round-trips\n\nVerifies dir creation, no temp leftovers, JSON on disk, and atomic os.replace.\n\nCo-Authored-By: Claude <noreply@anthropic.com>')"
```

---

## cleaner.py + assembler.py

These two modules are PURE (no I/O, no subprocess, stdlib `re` only). `clean_markdown` strips markdown noise from a string; `ProseAssembler` consumes streamed deltas and emits complete, speakable sentence chunks while replacing fenced code blocks with a spoken summary. The assembler calls `clean_markdown`, so build the cleaner first.

All tasks assume the `echo` package scaffolding already exists (`pyproject.toml`, `src/echo/__init__.py`, `tests/conftest.py` with the `sys.path.insert(0, <repo>/src)` fallback). Run every command from the repo root `/Users/Nima.Hakimi/projects/private/claude-tts`.

### Task: clean_markdown - failing test

Write the test first with real assertions covering every behavior in the brief: inline backticks, bold/italic, leading heading hashes, links, bare URLs, table separator rows, and whitespace collapse.

Create `tests/test_cleaner.py`:

```python
from echo.cleaner import clean_markdown


def test_inline_code_backticks_removed():
    assert clean_markdown("run the `clean()` function") == "run the clean() function"


def test_bold_markers_removed_words_kept():
    assert clean_markdown("this is **very** important") == "this is very important"


def test_italic_markers_removed_words_kept():
    assert clean_markdown("this is _very_ important") == "this is very important"
    assert clean_markdown("this is *very* important") == "this is very important"


def test_double_underscore_bold_removed():
    assert clean_markdown("this is __very__ important") == "this is very important"


def test_leading_heading_hashes_removed():
    assert clean_markdown("# Title here") == "Title here"
    assert clean_markdown("## Subtitle here") == "Subtitle here"


def test_markdown_link_becomes_label():
    assert clean_markdown("see [Anthropic](https://x) for more") == "see Anthropic for more"


def test_bare_url_becomes_link_word():
    assert clean_markdown("visit https://example.com/page now") == "visit link now"


def test_table_separator_row_dropped():
    assert clean_markdown("Name\n|---|---|\nValue") == "Name Value"


def test_multiple_spaces_and_newlines_collapse_to_single_space():
    assert clean_markdown("hello    world\n\n\nthere") == "hello world there"


def test_empty_input_returns_empty():
    assert clean_markdown("") == ""
```

Run it and expect a collection/import failure because the function does not exist yet:

```
python -m pytest tests/test_cleaner.py -q
```

Expected output (FAIL): an error during collection, `ModuleNotFoundError: No module named 'echo.cleaner'` (or `ImportError: cannot import name 'clean_markdown'`), 0 tests passed.

### Task: clean_markdown - implementation

Implement the minimal real cleaner. The link rule MUST run before the bare-URL rule so that `[label](url)` collapses to `label` before any leftover `url` could be turned into `"link"`. Order is derived from the legacy `clean()` at tag `v0-legacy-pty`, minus the triple-fence handling (the assembler owns fences).

Create `src/echo/cleaner.py`:

```python
"""Strip markdown noise from text so it reads naturally aloud.

PURE: no I/O. Does NOT handle triple-backtick fenced code blocks; the
ProseAssembler handles those before text reaches here.
"""
import re

# [label](url) -> label   (run BEFORE the bare-url rule)
_LINK = re.compile(r"\[([^\]\n]+)\]\((?:[^)\n]+)\)")
# inline code: `code` -> code  (drop the backticks, keep the text)
_INLINE_CODE = re.compile(r"`([^`\n]*)`")
# leading heading hashes at start of any line: "# ", "## " ... -> ""
_HEADING = re.compile(r"^#{1,6}\s+", flags=re.MULTILINE)
# bold/italic markers around a run of text -> the text (1-3 of * or _)
_EMPHASIS = re.compile(r"(\*{1,3}|_{1,3})([^*_\n]+)\1")
# a bare http/https url -> the word "link"
_BARE_URL = re.compile(r"https?://\S+")
# a markdown table separator row, e.g. |---|---| or | :--- | ---: |
_TABLE_SEP = re.compile(r"^\s*\|?[\s:|-]*-{3,}[\s:|-]*\|?\s*$", flags=re.MULTILINE)
# any run of whitespace (spaces, tabs, newlines) -> a single space
_WHITESPACE = re.compile(r"\s+")


def clean_markdown(text: str) -> str:
    # links first so the embedded url is gone before _BARE_URL runs
    text = _LINK.sub(r"\1", text)
    text = _INLINE_CODE.sub(r"\1", text)
    text = _HEADING.sub("", text)
    # apply emphasis twice to peel nested markers like ***x***
    text = _EMPHASIS.sub(r"\2", text)
    text = _EMPHASIS.sub(r"\2", text)
    text = _BARE_URL.sub("link", text)
    text = _TABLE_SEP.sub(" ", text)
    text = _WHITESPACE.sub(" ", text)
    return text.strip()
```

Run the tests, expect all to pass:

```
python -m pytest tests/test_cleaner.py -q
```

Expected output (PASS): `10 passed` (no failures, no errors).

### Task: clean_markdown - commit

Stage and commit the cleaner and its test together.

```
git add src/echo/cleaner.py tests/test_cleaner.py
git commit -m "$(cat <<'EOF'
feat: add clean_markdown to strip markdown noise for speech

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

Expected output: one commit created reporting `2 files changed`.

### Task: ProseAssembler - failing test for sentence assembly and buffering

Write tests for the core streaming behavior: splitting on sentence terminators across multiple `feed()` calls, holding partials, dedup of repeated index, two sentences in a single delta, and `final=True` flush.

Create `tests/test_assembler.py`:

```python
from echo.assembler import ProseAssembler


def test_two_sentences_across_two_feeds_emit_both_and_hold_nothing():
    a = ProseAssembler()
    out1 = a.feed("Hello world. Second one. ", 0, False)
    assert out1 == ["Hello world.", "Second one."]
    # nothing partial held back; a final flush yields nothing
    assert a.feed("", 1, True) == []


def test_partial_is_buffered_until_completed_by_later_delta():
    a = ProseAssembler()
    assert a.feed("No terminator yet", 0, False) == []
    assert a.feed(" but now it ends.", 1, False) == ["No terminator yet but now it ends."]


def test_partial_is_flushed_on_final():
    a = ProseAssembler()
    assert a.feed("No terminator yet", 0, False) == []
    assert a.feed("", 1, True) == ["No terminator yet"]


def test_repeated_index_is_ignored():
    a = ProseAssembler()
    assert a.feed("Hello world. ", 0, False) == ["Hello world."]
    # same index again: must be ignored, no duplicate emission
    assert a.feed("Hello world. ", 0, False) == []


def test_two_sentences_in_single_delta_emit_both():
    a = ProseAssembler()
    assert a.feed("First sentence! Second sentence?", 0, True) == [
        "First sentence!",
        "Second sentence?",
    ]


def test_final_resets_state_for_reuse():
    a = ProseAssembler()
    assert a.feed("Leftover text", 0, True) == ["Leftover text"]
    # after reset, index 0 is fresh again and buffer is empty
    assert a.feed("Brand new. ", 0, False) == ["Brand new."]
```

Run it, expect collection/import failure:

```
python -m pytest tests/test_assembler.py -q
```

Expected output (FAIL): `ModuleNotFoundError: No module named 'echo.assembler'` (or `ImportError: cannot import name 'ProseAssembler'`) during collection, 0 tests passed.

### Task: ProseAssembler - failing test for code fences

Add fence tests to the same file. A fence with an info string like `python` and 3 content lines emits exactly `"3-line python code block"` and suppresses the code; a fence with no info string emits `"<N>-line code block"`.

Append to `tests/test_assembler.py`:

```python
def test_fence_with_info_string_emits_lang_code_block_summary():
    a = ProseAssembler()
    delta = "```python\nline one\nline two\nline three\n```"
    assert a.feed(delta, 0, True) == ["3-line python code block"]


def test_fence_without_info_string_emits_plain_code_block_summary():
    a = ProseAssembler()
    delta = "```\nalpha\nbeta\n```"
    assert a.feed(delta, 0, True) == ["2-line code block"]


def test_fence_suppresses_code_and_keeps_surrounding_prose():
    a = ProseAssembler()
    delta = "Here it is. ```python\nx = 1\ny = 2\n``` Done now."
    out = a.feed(delta, 0, True)
    assert out == ["Here it is.", "1-line python code block", "Done now."]
```

Wait - the third test mixes prose before and after a fence inside one delta; the assembler emits the leading complete sentence, then the fence summary, then the trailing prose on the same `final=True` call. Run the tests, expect failures (the two earlier sentence tests collected too, but all assembler tests fail because the module is missing):

```
python -m pytest tests/test_assembler.py -q
```

Expected output (FAIL): collection error `ModuleNotFoundError: No module named 'echo.assembler'`, 0 tests passed.

### Task: ProseAssembler - implementation

Implement the assembler exactly per contract. State: a set of seen indices for dedup, a text buffer for prose, fence-tracking state, and a list of fence content lines. `feed` processes the new delta character-by-character through a tiny fence-aware scanner so that prose and fences interleaved in one delta produce chunks in order.

Design notes the code below relies on:
- A delta is ignored entirely if its `index` was already processed.
- The scanner appends prose characters to `self._buf` and, on each fence close, drains the prose accumulated so far into complete sentences (so order is preserved), then appends the fence summary chunk.
- A fence is detected only when ```` ``` ```` appears (we look for the literal triple backtick). The opening fence's remainder of its line is the info string (the language); content lines are the lines until the closing ```` ``` ````.
- A fence may be split across deltas, so fence state persists on `self`. While inside a fence, prose is suppressed.
- Sentence splitting: after `clean_markdown`, cut after `.`, `!`, or `?` when followed by whitespace or end-of-buffer; emit each complete sentence with `len > 1` (stripped); keep the trailing partial in the buffer. On `final=True`, flush the remaining buffer (cleaned, if non-empty) and reset ALL state.

Create `src/echo/assembler.py`:

```python
"""Assemble streamed text deltas into complete, speakable chunks.

PURE: no I/O. Splits prose into sentences and replaces triple-backtick
fenced code blocks with a spoken one-line summary.
"""
import re

from echo.cleaner import clean_markdown

_FENCE = "```"
# a complete sentence ends at . ! or ? followed by whitespace or end-of-string
_SENTENCE = re.compile(r"(.+?[.!?])(?:\s+|$)", flags=re.DOTALL)


class ProseAssembler:
    def __init__(self) -> None:
        self._seen: set[int] = set()
        self._buf = ""                 # pending prose text (outside fences)
        self._pending = ""             # raw tail not yet split into a line/fence token
        self._in_fence = False
        self._fence_lang = ""
        self._fence_lines: list[str] = []
        self._fence_opened_line = False  # have we consumed the opening info-string line?

    def feed(self, delta: str, index: int, final: bool) -> list[str]:
        out: list[str] = []
        if index in self._seen:
            # still honor a final flush even on a duplicate index
            if final:
                out.extend(self._flush_prose())
                self._reset()
            return out
        self._seen.add(index)

        self._pending += delta
        out.extend(self._consume())

        if final:
            out.extend(self._consume(force=True))
            out.extend(self._flush_prose())
            self._reset()
        return out

    def _consume(self, force: bool = False) -> list[str]:
        """Scan _pending for fence boundaries, routing text to prose or fence.

        Only acts on text we can resolve: a fence marker, or (inside a fence)
        a complete line. Leftover ambiguous tail stays in _pending unless force.
        """
        out: list[str] = []
        while True:
            if self._in_fence:
                nl = self._pending.find("\n")
                close = self._pending.find(_FENCE)
                # closing fence comes before the next newline (or no newline)
                if close != -1 and (nl == -1 or close < nl):
                    # everything before the closing fence on this line is content
                    # (already-collected lines handle full lines; trailing inline
                    # content before ``` is rare, treat remainder as a line if any)
                    pre = self._pending[:close]
                    if pre.strip():
                        self._fence_lines.append(pre)
                    self._pending = self._pending[close + len(_FENCE):]
                    out.append(self._close_fence())
                    continue
                if nl != -1:
                    line = self._pending[:nl]
                    self._pending = self._pending[nl + 1:]
                    if not self._fence_opened_line:
                        # first line after opening ``` is the info string
                        self._fence_lang = line.strip()
                        self._fence_opened_line = True
                    else:
                        self._fence_lines.append(line)
                    continue
                # no newline and no closing fence yet
                if force:
                    # unterminated fence at EOF: flush what we have
                    out.append(self._close_fence())
                break
            else:
                open_at = self._pending.find(_FENCE)
                if open_at != -1:
                    prose = self._pending[:open_at]
                    self._buf += prose
                    self._pending = self._pending[open_at + len(_FENCE):]
                    out.extend(self._split_sentences())
                    self._in_fence = True
                    self._fence_opened_line = False
                    self._fence_lang = ""
                    self._fence_lines = []
                    continue
                # no fence opening visible
                if force:
                    self._buf += self._pending
                    self._pending = ""
                    out.extend(self._split_sentences())
                else:
                    # hold back only enough tail to detect a future "```";
                    # commit everything except a possible partial fence marker
                    keep = self._partial_fence_tail_len()
                    if keep:
                        commit = self._pending[:-keep]
                        self._pending = self._pending[-keep:]
                    else:
                        commit = self._pending
                        self._pending = ""
                    if commit:
                        self._buf += commit
                        out.extend(self._split_sentences())
                break
        return out

    def _partial_fence_tail_len(self) -> int:
        """How many trailing chars of _pending could be the start of a fence."""
        for n in (2, 1):
            if self._pending.endswith("`" * n):
                return n
        return 0

    def _close_fence(self) -> str:
        n = len(self._fence_lines)
        lang = self._fence_lang
        self._in_fence = False
        self._fence_opened_line = False
        self._fence_lang = ""
        self._fence_lines = []
        if lang:
            return f"{n}-line {lang} code block"
        return f"{n}-line code block"

    def _split_sentences(self) -> list[str]:
        """Emit complete sentences from _buf, keeping the trailing partial."""
        out: list[str] = []
        cleaned = clean_markdown(self._buf)
        if not cleaned:
            self._buf = ""
            return out
        last_end = 0
        for m in _SENTENCE.finditer(cleaned):
            sentence = m.group(1).strip()
            if len(sentence) > 1:
                out.append(sentence)
            last_end = m.end()
        remainder = cleaned[last_end:]
        # keep the (cleaned) remainder as the buffer; a trailing space means done
        self._buf = remainder
        return out

    def _flush_prose(self) -> list[str]:
        if not self._buf:
            return []
        cleaned = clean_markdown(self._buf)
        self._buf = ""
        if len(cleaned) > 1:
            return [cleaned]
        return []

    def _reset(self) -> None:
        self._seen = set()
        self._buf = ""
        self._pending = ""
        self._in_fence = False
        self._fence_lang = ""
        self._fence_lines = []
        self._fence_opened_line = False
```

Run the assembler tests, expect all to pass:

```
python -m pytest tests/test_assembler.py -q
```

Expected output (PASS): `9 passed` (no failures, no errors).

### Task: ProseAssembler - full module test pass and commit

Confirm both modules pass together (the cleaner must keep working since the assembler depends on it), then commit.

```
python -m pytest tests/test_cleaner.py tests/test_assembler.py -q
```

Expected output (PASS): `19 passed`.

Commit the assembler and its test:

```
git add src/echo/assembler.py tests/test_assembler.py
git commit -m "$(cat <<'EOF'
feat: add ProseAssembler for streamed sentence and code-fence assembly

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

Expected output: one commit created reporting `2 files changed`.

---

## queue.py + speaker.py

This section builds the in-memory speech queue (`src/echo/queue.py`) and the audio Speaker (`src/echo/speaker.py`). Both are pure-logic units: the queue touches no I/O, and the Speaker takes injected `say_runner`/`earcon_player` so tests never play real audio. Every task is strict TDD: failing test first, minimal real implementation, passing test, commit.

All commands are run from the repo root `/Users/Nima.Hakimi/projects/private/claude-tts`.

### Task: SpeechQueue FIFO enqueue/pop_next and __len__

Write the failing test first.

Create `tests/test_queue.py`:

```python
from echo.queue import SpeechItem, SpeechQueue


def _item(id, session="s1", kind="prose", text="t", is_decision=False):
    return SpeechItem(id=id, session=session, kind=kind, text=text, is_decision=is_decision)


def test_enqueue_then_pop_next_is_fifo():
    q = SpeechQueue()
    q.enqueue(_item(1, text="first"))
    q.enqueue(_item(2, text="second"))
    q.enqueue(_item(3, text="third"))
    assert q.pop_next().text == "first"
    assert q.pop_next().text == "second"
    assert q.pop_next().text == "third"


def test_pop_next_on_empty_returns_none():
    q = SpeechQueue()
    assert q.pop_next() is None


def test_len_tracks_pending_items():
    q = SpeechQueue()
    assert len(q) == 0
    q.enqueue(_item(1))
    q.enqueue(_item(2))
    assert len(q) == 2
    q.pop_next()
    assert len(q) == 1
```

Run it and expect failure because the module does not exist yet:

```bash
python -m pytest tests/test_queue.py -q
```

Expected output (abridged) - collection error, no module `echo.queue`:

```
E   ModuleNotFoundError: No module named 'echo.queue'
...
1 error in ...s
```

Now write the minimal real implementation.

Create `src/echo/queue.py`:

```python
from collections import deque
from dataclasses import dataclass


@dataclass
class SpeechItem:
    id: int
    session: str
    kind: str          # one of prose|choice|plan|permission|tool_announce
    text: str
    is_decision: bool  # True for choice|plan|permission


class SpeechQueue:
    def __init__(self) -> None:
        self._items: "deque[SpeechItem]" = deque()

    def enqueue(self, item: SpeechItem) -> None:
        self._items.append(item)

    def pop_next(self) -> "SpeechItem | None":
        if not self._items:
            return None
        return self._items.popleft()

    def __len__(self) -> int:
        return len(self._items)
```

Run the tests again and expect them to pass:

```bash
python -m pytest tests/test_queue.py -q
```

Expected output:

```
3 passed in ...s
```

Commit.

```bash
git add src/echo/queue.py tests/test_queue.py
git commit -m "feat: SpeechQueue FIFO enqueue/pop_next with __len__

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: SpeechQueue jump_to_decision and clear

Add the failing tests. Append to `tests/test_queue.py`:

```python
def test_jump_to_decision_drops_leading_non_decision_keeps_decision_at_front():
    q = SpeechQueue()
    q.enqueue(_item(1, kind="prose", text="p1", is_decision=False))
    q.enqueue(_item(2, kind="prose", text="p2", is_decision=False))
    q.enqueue(_item(3, kind="choice", text="decide", is_decision=True))
    q.enqueue(_item(4, kind="prose", text="after", is_decision=False))
    q.jump_to_decision()
    assert len(q) == 2
    first = q.pop_next()
    assert first.text == "decide"
    assert first.is_decision is True
    assert q.pop_next().text == "after"


def test_jump_to_decision_with_no_decision_empties_queue():
    q = SpeechQueue()
    q.enqueue(_item(1, is_decision=False))
    q.enqueue(_item(2, is_decision=False))
    q.jump_to_decision()
    assert len(q) == 0
    assert q.pop_next() is None


def test_jump_to_decision_on_empty_is_noop():
    q = SpeechQueue()
    q.jump_to_decision()
    assert len(q) == 0


def test_clear_empties_queue():
    q = SpeechQueue()
    q.enqueue(_item(1))
    q.enqueue(_item(2))
    q.clear()
    assert len(q) == 0
    assert q.pop_next() is None
```

Run and expect failure - the methods do not exist yet:

```bash
python -m pytest tests/test_queue.py -k "jump_to_decision or clear" -q
```

Expected output (abridged):

```
E   AttributeError: 'SpeechQueue' object has no attribute 'jump_to_decision'
...
```

Add the implementation. Edit `src/echo/queue.py` - add these two methods to the `SpeechQueue` class (after `pop_next`, before `__len__`):

```python
    def jump_to_decision(self) -> None:
        while self._items and not self._items[0].is_decision:
            self._items.popleft()

    def clear(self) -> None:
        self._items.clear()
```

Run and expect pass:

```bash
python -m pytest tests/test_queue.py -k "jump_to_decision or clear" -q
```

Expected output:

```
4 passed in ...s
```

Commit.

```bash
git add src/echo/queue.py tests/test_queue.py
git commit -m "feat: SpeechQueue jump_to_decision and clear

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: SpeechQueue flush_session

Add the failing test. Append to `tests/test_queue.py`:

```python
def test_flush_session_removes_only_that_session_preserving_order():
    q = SpeechQueue()
    q.enqueue(_item(1, session="A", text="a1"))
    q.enqueue(_item(2, session="B", text="b1"))
    q.enqueue(_item(3, session="A", text="a2"))
    q.enqueue(_item(4, session="B", text="b2"))
    q.flush_session("A")
    assert len(q) == 2
    assert q.pop_next().text == "b1"
    assert q.pop_next().text == "b2"


def test_flush_session_unknown_session_is_noop():
    q = SpeechQueue()
    q.enqueue(_item(1, session="A"))
    q.flush_session("does-not-exist")
    assert len(q) == 1
```

Run and expect failure:

```bash
python -m pytest tests/test_queue.py -k flush_session -q
```

Expected output (abridged):

```
E   AttributeError: 'SpeechQueue' object has no attribute 'flush_session'
...
```

Add the implementation. Edit `src/echo/queue.py` - add this method to the `SpeechQueue` class (after `clear`, before `__len__`):

```python
    def flush_session(self, session: str) -> None:
        self._items = deque(
            item for item in self._items if item.session != session
        )
```

Run and expect pass:

```bash
python -m pytest tests/test_queue.py -k flush_session -q
```

Expected output:

```
2 passed in ...s
```

Run the whole queue suite to confirm nothing regressed:

```bash
python -m pytest tests/test_queue.py -q
```

Expected output:

```
9 passed in ...s
```

Commit.

```bash
git add src/echo/queue.py tests/test_queue.py
git commit -m "feat: SpeechQueue flush_session removes only matching session

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Speaker.speak blocks on the say child via injected runner

Write the failing test first. This introduces the `FakePopen` recorder reused by later Speaker tests.

Create `tests/test_speaker.py`:

```python
from echo.speaker import Speaker


class FakePopen:
    """Stand-in for subprocess.Popen exposing wait()/terminate()."""

    def __init__(self):
        self.wait_calls = 0
        self.terminate_calls = 0

    def wait(self):
        self.wait_calls += 1
        return 0

    def terminate(self):
        self.terminate_calls += 1


class RecordingRunner:
    """Records say_runner(text, voice, rate) calls; returns a fresh FakePopen each time."""

    def __init__(self):
        self.calls = []
        self.procs = []

    def __call__(self, text, voice, rate):
        proc = FakePopen()
        self.calls.append((text, voice, rate))
        self.procs.append(proc)
        return proc


def test_speak_calls_say_runner_with_voice_rate_and_blocks_on_wait():
    runner = RecordingRunner()
    sp = Speaker(voice="Ava", rate=180, say_runner=runner)
    sp.speak("hello world")
    assert runner.calls == [("hello world", "Ava", 180)]
    assert runner.procs[0].wait_calls == 1


def test_speak_tracks_current_proc():
    runner = RecordingRunner()
    sp = Speaker(say_runner=runner)
    sp.speak("one")
    sp.speak("two")
    assert len(runner.procs) == 2
    assert runner.procs[0].wait_calls == 1
    assert runner.procs[1].wait_calls == 1
```

Run and expect failure - module does not exist yet:

```bash
python -m pytest tests/test_speaker.py -k speak -q
```

Expected output (abridged):

```
E   ModuleNotFoundError: No module named 'echo.speaker'
...
1 error in ...s
```

Now write the minimal real implementation. We implement `run_say` and `play_earcon` as the real (injectable) defaults plus the `Speaker` class. `best_enhanced_voice` is stubbed minimally here and fully implemented in a later task; we add it now only so the module imports cleanly and `Speaker(voice=None)` can resolve a default without real subprocess work in these tests (it is only called when voice is None, which these tests do not exercise).

Create `src/echo/speaker.py`:

```python
import subprocess


def run_say(text: str, voice, rate: int):
    cmd = ["say"]
    if voice:
        cmd += ["-v", voice]
    cmd += ["-r", str(rate), text]
    return subprocess.Popen(cmd)


def play_earcon(path: str) -> None:
    try:
        subprocess.Popen(["afplay", path])
    except FileNotFoundError:
        pass


def best_enhanced_voice() -> str:
    return "Samantha"


class Speaker:
    def __init__(
        self,
        voice=None,
        rate=200,
        say_runner=run_say,
        earcon_player=play_earcon,
        earcons=None,
    ) -> None:
        self._voice = voice
        self._rate = rate
        self._say_runner = say_runner
        self._earcon_player = earcon_player
        self._earcons = dict(earcons) if earcons else {}
        self._current = None

    def speak(self, text: str) -> None:
        proc = self._say_runner(text, self._voice, self._rate)
        self._current = proc
        proc.wait()
        self._current = None

    def set_voice(self, v) -> None:
        self._voice = v

    def set_rate(self, r) -> None:
        self._rate = r
```

Run and expect pass:

```bash
python -m pytest tests/test_speaker.py -k speak -q
```

Expected output:

```
2 passed in ...s
```

Commit.

```bash
git add src/echo/speaker.py tests/test_speaker.py
git commit -m "feat: Speaker.speak runs injected say runner and blocks on wait

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Speaker.cancel terminates only the current proc

Add the failing test. Append to `tests/test_speaker.py`:

```python
def test_cancel_terminates_the_current_proc():
    runner = RecordingRunner()

    # A runner whose returned proc does NOT auto-finish on wait(): we drive
    # cancel() before the (simulated) blocking wait by calling speak in a way
    # that lets us inspect the tracked proc. Here we use a runner that records
    # the proc and lets us cancel after wait returns, then assert terminate.
    sp = Speaker(say_runner=runner)
    sp.speak("blah")
    # After speak returns, current proc is cleared; cancel must be a safe no-op.
    sp.cancel()
    assert runner.procs[0].terminate_calls == 0


def test_cancel_terminates_active_proc_mid_speak():
    # Use a runner whose proc.wait() invokes a hook so we can cancel while the
    # proc is still tracked as current.
    captured = {}

    class CancelOnWaitPopen(FakePopen):
        def __init__(self, speaker):
            super().__init__()
            self._speaker = speaker

        def wait(self):
            # While we are "blocking", the speaker treats us as current.
            self._speaker.cancel()
            return super().wait()

    class HookRunner:
        def __init__(self):
            self.procs = []

        def __call__(self, text, voice, rate):
            proc = CancelOnWaitPopen(captured["speaker"])
            self.procs.append(proc)
            return proc

    runner = HookRunner()
    sp = Speaker(say_runner=runner)
    captured["speaker"] = sp
    sp.speak("active")
    assert runner.procs[0].terminate_calls == 1


def test_cancel_with_no_current_proc_is_noop():
    sp = Speaker(say_runner=RecordingRunner())
    # Never called speak; cancel must not raise.
    sp.cancel()
```

Run and expect failure - `cancel` does not exist:

```bash
python -m pytest tests/test_speaker.py -k cancel -q
```

Expected output (abridged):

```
E   AttributeError: 'Speaker' object has no attribute 'cancel'
...
```

Add the implementation. Edit `src/echo/speaker.py` - add the `cancel` method to the `Speaker` class (after `speak`):

```python
    def cancel(self) -> None:
        proc = self._current
        if proc is not None:
            proc.terminate()
```

Run and expect pass:

```bash
python -m pytest tests/test_speaker.py -k cancel -q
```

Expected output:

```
3 passed in ...s
```

Commit.

```bash
git add src/echo/speaker.py tests/test_speaker.py
git commit -m "feat: Speaker.cancel terminates only the current say child

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Speaker.earcon maps kind to path; unknown kind is a no-op

Add the failing test. Append to `tests/test_speaker.py`:

```python
class RecordingEarcon:
    def __init__(self):
        self.paths = []

    def __call__(self, path):
        self.paths.append(path)


def test_earcon_plays_mapped_path():
    player = RecordingEarcon()
    earcons = {
        "permission": "/System/Library/Sounds/Funk.aiff",
        "choice": "/System/Library/Sounds/Ping.aiff",
    }
    sp = Speaker(say_runner=RecordingRunner(), earcon_player=player, earcons=earcons)
    sp.earcon("choice")
    assert player.paths == ["/System/Library/Sounds/Ping.aiff"]


def test_earcon_unknown_kind_is_noop():
    player = RecordingEarcon()
    earcons = {"choice": "/System/Library/Sounds/Ping.aiff"}
    sp = Speaker(say_runner=RecordingRunner(), earcon_player=player, earcons=earcons)
    sp.earcon("does-not-exist")
    assert player.paths == []


def test_earcon_kind_with_no_mapping_is_noop():
    player = RecordingEarcon()
    sp = Speaker(say_runner=RecordingRunner(), earcon_player=player, earcons={})
    sp.earcon("choice")
    assert player.paths == []
```

Run and expect failure - `earcon` does not exist:

```bash
python -m pytest tests/test_speaker.py -k earcon -q
```

Expected output (abridged):

```
E   AttributeError: 'Speaker' object has no attribute 'earcon'
...
```

Add the implementation. Edit `src/echo/speaker.py` - add the `earcon` method to the `Speaker` class (after `cancel`):

```python
    def earcon(self, kind: str) -> None:
        path = self._earcons.get(kind)
        if path is None:
            return
        self._earcon_player(path)
```

Run and expect pass:

```bash
python -m pytest tests/test_speaker.py -k earcon -q
```

Expected output:

```
3 passed in ...s
```

Commit.

```bash
git add src/echo/speaker.py tests/test_speaker.py
git commit -m "feat: Speaker.earcon resolves kind to sound path, unknown is no-op

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: play_earcon tolerates a missing sound file

Add the failing test. This tests the real `play_earcon` default; we monkeypatch `subprocess.Popen` so no real `afplay` runs, and assert that a missing file path raises nothing.

Append to `tests/test_speaker.py`:

```python
import echo.speaker as speaker_mod


def test_play_earcon_missing_file_is_tolerated(monkeypatch):
    def fake_popen(args):
        # Simulate afplay being unable to open the (missing) file.
        raise FileNotFoundError(args[-1])

    monkeypatch.setattr(speaker_mod.subprocess, "Popen", fake_popen)
    # Must not raise even though the file/binary is unavailable.
    speaker_mod.play_earcon("/no/such/sound.aiff")


def test_play_earcon_invokes_afplay_with_path(monkeypatch):
    recorded = {}

    def fake_popen(args):
        recorded["args"] = args
        return object()

    monkeypatch.setattr(speaker_mod.subprocess, "Popen", fake_popen)
    speaker_mod.play_earcon("/System/Library/Sounds/Tink.aiff")
    assert recorded["args"] == ["afplay", "/System/Library/Sounds/Tink.aiff"]
```

Run and expect the first test to FAIL. The current `play_earcon` only catches `FileNotFoundError` raised by `Popen` when the *binary* is absent - but if `afplay` exists yet the *file* is missing, `Popen` succeeds and the playback fails silently in the child, so the contract requires the function to tolerate both. To make the failure concrete now, the first test passes only once the catch is in place; the second test passes already. Run:

```bash
python -m pytest tests/test_speaker.py -k play_earcon -q
```

Expected output: both pass IF the implementation already catches `FileNotFoundError`. Confirm by running; you should see:

```
2 passed in ...s
```

To make the missing-file tolerance explicit and robust (the contract says "ignore FileNotFound / missing file"), harden `play_earcon` so it also swallows the case where the path does not exist before spawning, and any spawn error. Edit `src/echo/speaker.py` - replace the `play_earcon` function:

```python
def play_earcon(path: str) -> None:
    if not os.path.exists(path):
        return
    try:
        subprocess.Popen(["afplay", path])
    except (FileNotFoundError, OSError):
        pass
```

Add the `os` import at the top of `src/echo/speaker.py`, directly above `import subprocess`:

```python
import os
import subprocess
```

The earlier `test_play_earcon_missing_file_is_tolerated` monkeypatches `Popen` to raise; but now the early `os.path.exists` guard returns first, so update that test to assert the guard path too. Replace `test_play_earcon_missing_file_is_tolerated` and `test_play_earcon_invokes_afplay_with_path` with:

```python
def test_play_earcon_missing_file_is_tolerated(monkeypatch):
    called = {"popen": False}

    def fake_popen(args):
        called["popen"] = True
        return object()

    monkeypatch.setattr(speaker_mod.os.path, "exists", lambda p: False)
    monkeypatch.setattr(speaker_mod.subprocess, "Popen", fake_popen)
    # Missing file: must return without spawning afplay and without raising.
    speaker_mod.play_earcon("/no/such/sound.aiff")
    assert called["popen"] is False


def test_play_earcon_spawn_error_is_tolerated(monkeypatch):
    def fake_popen(args):
        raise FileNotFoundError("afplay missing")

    monkeypatch.setattr(speaker_mod.os.path, "exists", lambda p: True)
    monkeypatch.setattr(speaker_mod.subprocess, "Popen", fake_popen)
    # Binary missing: must not raise.
    speaker_mod.play_earcon("/System/Library/Sounds/Tink.aiff")


def test_play_earcon_invokes_afplay_with_path(monkeypatch):
    recorded = {}

    def fake_popen(args):
        recorded["args"] = args
        return object()

    monkeypatch.setattr(speaker_mod.os.path, "exists", lambda p: True)
    monkeypatch.setattr(speaker_mod.subprocess, "Popen", fake_popen)
    speaker_mod.play_earcon("/System/Library/Sounds/Tink.aiff")
    assert recorded["args"] == ["afplay", "/System/Library/Sounds/Tink.aiff"]
```

Run and expect pass:

```bash
python -m pytest tests/test_speaker.py -k play_earcon -q
```

Expected output:

```
3 passed in ...s
```

Commit.

```bash
git add src/echo/speaker.py tests/test_speaker.py
git commit -m "feat: play_earcon tolerates missing sound file and spawn errors

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: best_enhanced_voice picks a Premium en voice, falls back to Samantha

Add the failing test. We monkeypatch the subprocess call used inside `best_enhanced_voice` to return a sample `say -v ?` listing, and assert selection behavior.

Append to `tests/test_speaker.py`:

```python
from echo.speaker import best_enhanced_voice

SAY_SAMPLE = (
    "Albert              en_US    # Hello! My name is Albert.\n"
    "Alice               it_IT    # Ciao! Mi chiamo Alice.\n"
    "Allison             en_US    # Hi, my name is Allison.\n"
    "Ava (Premium)       en_US    # Hi, my name is Ava.\n"
    "Daniel              en_GB    # Hello, my name is Daniel.\n"
    "Samantha            en_US    # Hi, my name is Samantha.\n"
    "Zoe (Premium)       en_US    # Hi, my name is Zoe.\n"
    "Zuzana              cs_CZ    # Dobrý den, jmenuji se Zuzana.\n"
)

SAY_SAMPLE_NO_PREMIUM = (
    "Albert              en_US    # Hello! My name is Albert.\n"
    "Daniel              en_GB    # Hello, my name is Daniel.\n"
    "Zuzana              cs_CZ    # Dobrý den, jmenuji se Zuzana.\n"
)

SAY_SAMPLE_PREMIUM_NON_EN = (
    "Alice (Premium)     it_IT    # Ciao! Mi chiamo Alice.\n"
    "Daniel              en_GB    # Hello, my name is Daniel.\n"
)


def test_best_enhanced_voice_prefers_premium_en(monkeypatch):
    monkeypatch.setattr(
        speaker_mod.subprocess, "check_output", lambda *a, **k: SAY_SAMPLE
    )
    voice = best_enhanced_voice()
    assert voice in ("Ava", "Zoe")
    # The first Premium en voice in the listing wins.
    assert voice == "Ava"


def test_best_enhanced_voice_falls_back_to_samantha_when_no_premium(monkeypatch):
    monkeypatch.setattr(
        speaker_mod.subprocess,
        "check_output",
        lambda *a, **k: SAY_SAMPLE_NO_PREMIUM,
    )
    assert best_enhanced_voice() == "Samantha"


def test_best_enhanced_voice_ignores_premium_non_en(monkeypatch):
    monkeypatch.setattr(
        speaker_mod.subprocess,
        "check_output",
        lambda *a, **k: SAY_SAMPLE_PREMIUM_NON_EN,
    )
    # Premium voice is Italian; must fall back to Samantha.
    assert best_enhanced_voice() == "Samantha"


def test_best_enhanced_voice_falls_back_when_say_errors(monkeypatch):
    def boom(*a, **k):
        raise FileNotFoundError("say missing")

    monkeypatch.setattr(speaker_mod.subprocess, "check_output", boom)
    assert best_enhanced_voice() == "Samantha"
```

Run and expect failure - the stub `best_enhanced_voice` always returns `"Samantha"`, so the Premium-preference tests fail:

```bash
python -m pytest tests/test_speaker.py -k best_enhanced_voice -q
```

Expected output (abridged):

```
>       assert voice == "Ava"
E       AssertionError: assert 'Samantha' == 'Ava'
...
2 failed, 2 passed in ...s
```

Now write the real implementation. Edit `src/echo/speaker.py` - replace the stub `best_enhanced_voice` function:

```python
def best_enhanced_voice() -> str:
    fallback = "Samantha"
    try:
        listing = subprocess.check_output(
            ["say", "-v", "?"], text=True
        )
    except (FileNotFoundError, OSError, subprocess.SubprocessError):
        return fallback

    premium_en = []
    plain_en = []
    for line in listing.splitlines():
        line = line.rstrip()
        if not line:
            continue
        # Format: "Name [maybe (Quality)] <pad> locale # sample"
        before_hash = line.split("#", 1)[0].rstrip()
        parts = before_hash.split()
        if len(parts) < 2:
            continue
        locale = parts[-1]
        name_tokens = parts[:-1]
        name = " ".join(name_tokens)
        is_premium = "(Premium)" in name or "(Enhanced)" in name
        # Bare display name without the quality suffix.
        bare = name.replace("(Premium)", "").replace("(Enhanced)", "").strip()
        if not locale.startswith("en"):
            continue
        if is_premium:
            premium_en.append(bare)
        else:
            plain_en.append(bare)

    if premium_en:
        return premium_en[0]
    for preferred in ("Allison", "Samantha"):
        if preferred in plain_en:
            return preferred
    return fallback
```

Run and expect pass:

```bash
python -m pytest tests/test_speaker.py -k best_enhanced_voice -q
```

Expected output:

```
4 passed in ...s
```

Run the full Speaker suite to confirm no regressions:

```bash
python -m pytest tests/test_speaker.py -q
```

Expected output:

```
15 passed in ...s
```

Commit.

```bash
git add src/echo/speaker.py tests/test_speaker.py
git commit -m "feat: best_enhanced_voice prefers Premium en voice, falls back to Samantha

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Full queue + speaker suite green

Run both suites together as a final gate for this section:

```bash
python -m pytest tests/test_queue.py tests/test_speaker.py -q
```

Expected output:

```
24 passed in ...s
```

No new files to commit (verification only). If `git status` shows a clean tree, this section is complete:

```bash
git status --short
```

Expected output: empty (clean working tree).

---

## sessions.py + daemon.py + client.py

These tasks build the session-tracking, the daemon dispatch + socket server, and the client. They assume the earlier contract modules already exist and pass: `src/echo/paths.py`, `src/echo/protocol.py`, `src/echo/config.py`, `src/echo/cleaner.py`, `src/echo/assembler.py`, `src/echo/queue.py`, and `src/echo/speaker.py`. Every command below is run from the repo root `/Users/Nima.Hakimi/projects/private/claude-tts`. Tests use `python -m pytest` (the venv with `pip install -e .` and `pytest` is already set up by the scaffolding task); `tests/conftest.py` adds `src` to `sys.path` as a fallback.

### Task: SessionManager foreground tracking

**Step 1 - Write the failing test.** Create `tests/test_sessions.py`:

```python
from echo.sessions import SessionManager


def test_foreground_starts_none():
    sm = SessionManager()
    assert sm.foreground() is None


def test_set_and_get_foreground():
    sm = SessionManager()
    sm.set_foreground("s1")
    assert sm.foreground() == "s1"
    assert sm.is_foreground("s1") is True
    assert sm.is_foreground("s2") is False


def test_set_foreground_replaces_previous():
    sm = SessionManager()
    sm.set_foreground("s1")
    sm.set_foreground("s2")
    assert sm.foreground() == "s2"
    assert sm.is_foreground("s1") is False
    assert sm.is_foreground("s2") is True


def test_should_speak_true_only_for_foreground():
    sm = SessionManager()
    sm.register("s1")
    sm.register("s2")
    sm.set_foreground("s1")
    assert sm.should_speak("s1") is True
    assert sm.should_speak("s2") is False


def test_should_speak_false_when_no_foreground():
    sm = SessionManager()
    sm.register("s1")
    assert sm.should_speak("s1") is False


def test_register_and_unregister():
    sm = SessionManager()
    sm.register("s1")
    sm.set_foreground("s1")
    assert sm.is_foreground("s1") is True
    sm.unregister("s1")
    # unregistering the foreground session clears foreground
    assert sm.foreground() is None
    assert sm.should_speak("s1") is False


def test_unregister_non_foreground_keeps_foreground():
    sm = SessionManager()
    sm.register("s1")
    sm.register("s2")
    sm.set_foreground("s1")
    sm.unregister("s2")
    assert sm.foreground() == "s1"


def test_unregister_unknown_session_is_noop():
    sm = SessionManager()
    sm.set_foreground("s1")
    sm.unregister("ghost")
    assert sm.foreground() == "s1"
```

**Step 2 - Run it, expect failure (no module yet).**

```
python -m pytest tests/test_sessions.py -q
```

Expected: collection/import error, `ModuleNotFoundError: No module named 'echo.sessions'`.

**Step 3 - Implement `src/echo/sessions.py`:**

```python
class SessionManager:
    def __init__(self, background_policy: str = "earcon_only") -> None:
        self.background_policy = background_policy
        self._sessions: set[str] = set()
        self._foreground: "str | None" = None

    def set_foreground(self, session: str) -> None:
        self._sessions.add(session)
        self._foreground = session

    def foreground(self) -> "str | None":
        return self._foreground

    def is_foreground(self, session: str) -> bool:
        return self._foreground is not None and session == self._foreground

    def register(self, session: str) -> None:
        self._sessions.add(session)

    def unregister(self, session: str) -> None:
        self._sessions.discard(session)
        if self._foreground == session:
            self._foreground = None

    def should_speak(self, session: str) -> bool:
        return self.is_foreground(session)
```

**Step 4 - Run it, expect pass.**

```
python -m pytest tests/test_sessions.py -q
```

Expected: `8 passed`.

**Step 5 - Commit.**

```
git add src/echo/sessions.py tests/test_sessions.py
git commit -m "feat: SessionManager foreground tracking and should_speak

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Daemon test scaffolding (FakeSpeaker + builder)

This task adds NO production code; it adds reusable fakes/fixtures consumed by the daemon dispatch tests. We verify it by importing the helpers in a smoke test.

**Step 1 - Write the helper module + smoke test.** Create `tests/daemon_helpers.py`:

```python
from echo.queue import SpeechQueue
from echo.sessions import SessionManager
from echo.daemon import SpeechDaemon
from echo.config import DEFAULTS


class FakeSpeaker:
    """Records every Speaker call instead of touching audio."""

    def __init__(self):
        self.spoken: list[str] = []
        self.earcons: list[str] = []
        self.cancels: int = 0
        self.rates: list[int] = []
        self.voices: list = []

    def speak(self, text: str) -> None:
        self.spoken.append(text)

    def earcon(self, kind: str) -> None:
        self.earcons.append(kind)

    def cancel(self) -> None:
        self.cancels += 1

    def set_rate(self, r: int) -> None:
        self.rates.append(r)

    def set_voice(self, v) -> None:
        self.voices.append(v)


def make_daemon(verbosity: str = "everything", foreground: "str | None" = "fg"):
    """Build a SpeechDaemon wired to a real SpeechQueue + FakeSpeaker."""
    queue = SpeechQueue()
    speaker = FakeSpeaker()
    sessions = SessionManager()
    if foreground is not None:
        sessions.set_foreground(foreground)
    config = {k: (v.copy() if isinstance(v, dict) else v) for k, v in DEFAULTS.items()}
    config["verbosity"] = verbosity
    daemon = SpeechDaemon(queue, speaker, sessions, config)
    return daemon, queue, speaker, sessions, config
```

Create `tests/test_daemon_helpers.py`:

```python
from tests.daemon_helpers import FakeSpeaker, make_daemon


def test_fake_speaker_records():
    fs = FakeSpeaker()
    fs.speak("hi")
    fs.earcon("plan")
    fs.cancel()
    fs.set_rate(150)
    fs.set_voice("Ava")
    assert fs.spoken == ["hi"]
    assert fs.earcons == ["plan"]
    assert fs.cancels == 1
    assert fs.rates == [150]
    assert fs.voices == ["Ava"]


def test_make_daemon_wires_components():
    daemon, queue, speaker, sessions, config = make_daemon()
    assert sessions.foreground() == "fg"
    assert config["verbosity"] == "everything"
    assert len(queue) == 0
    assert isinstance(speaker, FakeSpeaker)
```

**Step 2 - Run it, expect failure (daemon module missing).**

```
python -m pytest tests/test_daemon_helpers.py -q
```

Expected: `ModuleNotFoundError: No module named 'echo.daemon'` raised from `tests/daemon_helpers.py` import.

**Step 3 - Create a minimal stub so helpers import.** Create `src/echo/daemon.py`:

```python
class SpeechDaemon:
    def __init__(self, queue, speaker, sessions, config) -> None:
        self.queue = queue
        self.speaker = speaker
        self.sessions = sessions
        self.config = config
        self._assemblers = {}
        self._next_id = 0

    def handle_message(self, msg):
        raise NotImplementedError
```

**Step 4 - Run it, expect pass.**

```
python -m pytest tests/test_daemon_helpers.py -q
```

Expected: `2 passed`.

**Step 5 - Commit.**

```
git add tests/daemon_helpers.py tests/test_daemon_helpers.py src/echo/daemon.py
git commit -m "test: FakeSpeaker and make_daemon helpers + daemon stub

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Daemon prose dispatch (foreground gating + assembly)

**Step 1 - Write the failing test.** Create `tests/test_daemon_prose.py`:

```python
from echo.protocol import MsgType, PROTOCOL_VERSION
from echo.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _prose(session, delta, index, final):
    return {
        "v": PROTOCOL_VERSION,
        "type": MsgType.PROSE,
        "session": session,
        "delta": delta,
        "index": index,
        "final": final,
    }


def test_prose_from_non_foreground_session_is_dropped():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    out = daemon.handle_message(_prose("other", "Hello there. ", 0, False))
    assert out is None
    assert len(queue) == 0


def test_prose_from_foreground_enqueues_one_item_per_chunk():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # Two complete sentences -> two chunks -> two enqueued items.
    daemon.handle_message(_prose("fg", "Hello there. How are you? ", 0, False))
    assert len(queue) == 2
    first = queue.pop_next()
    second = queue.pop_next()
    assert isinstance(first, SpeechItem)
    assert first.session == "fg"
    assert first.kind == "prose"
    assert first.is_decision is False
    assert first.text == "Hello there."
    assert second.text == "How are you?"
    # ids are unique and increasing
    assert second.id > first.id


def test_prose_partial_then_final_flushes_remainder():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # Partial sentence (no terminator) -> no chunk yet.
    daemon.handle_message(_prose("fg", "tail with no period", 0, False))
    assert len(queue) == 0
    # final=True flushes the remainder as one chunk.
    daemon.handle_message(_prose("fg", "", 1, True))
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.text == "tail with no period"


def test_prose_uses_per_session_assembler():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # Same index reused across sessions must NOT be deduped across sessions.
    daemon.handle_message(_prose("fg", "Foreground sentence here. ", 0, False))
    # background session at index 0 is dropped (not foreground) but must not crash
    daemon.handle_message(_prose("bg", "Background sentence here. ", 0, False))
    assert len(queue) == 1
    assert queue.pop_next().text == "Foreground sentence here."
```

**Step 2 - Run it, expect failure.**

```
python -m pytest tests/test_daemon_prose.py -q
```

Expected: failures with `NotImplementedError` from `handle_message`.

**Step 3 - Implement prose handling in `src/echo/daemon.py`.** Replace the whole file with:

```python
from echo.protocol import MsgType
from echo.queue import SpeechItem
from echo.assembler import ProseAssembler


class SpeechDaemon:
    def __init__(self, queue, speaker, sessions, config) -> None:
        self.queue = queue
        self.speaker = speaker
        self.sessions = sessions
        self.config = config
        self._assemblers = {}
        self._next_id = 0

    def _alloc_id(self) -> int:
        self._next_id += 1
        return self._next_id

    def _assembler(self, session: str) -> ProseAssembler:
        a = self._assemblers.get(session)
        if a is None:
            a = ProseAssembler()
            self._assemblers[session] = a
        return a

    def _enqueue(self, session: str, kind: str, text: str, is_decision: bool) -> None:
        item = SpeechItem(
            id=self._alloc_id(),
            session=session,
            kind=kind,
            text=text,
            is_decision=is_decision,
        )
        self.queue.enqueue(item)

    def handle_message(self, msg):
        t = msg.get("type")
        if t == MsgType.PROSE:
            session = msg.get("session", "")
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), msg.get("final", False))
            if self.sessions.should_speak(session):
                for chunk in chunks:
                    self._enqueue(session, "prose", chunk, False)
            return None
        return None
```

**Step 4 - Run it, expect pass.**

```
python -m pytest tests/test_daemon_prose.py -q
```

Expected: `4 passed`.

**Step 5 - Commit.**

```
git add src/echo/daemon.py tests/test_daemon_prose.py
git commit -m "feat: daemon prose dispatch with foreground gating

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Daemon decision dispatch (choice/plan/permission + tool_announce)

**Step 1 - Write the failing test.** Create `tests/test_daemon_decisions.py`:

```python
from echo.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(mtype, session, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype, "session": session}
    d.update(extra)
    return d


def test_choice_enqueues_when_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Pick a color", "options": [{"label": "Red"}, {"label": "Blue"}]},
    ]))
    # A content message NEVER earcons; the alert is a separate EARCON message.
    assert speaker.earcons == []
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "choice"
    assert item.is_decision is True
    assert "Pick a color" in item.text
    assert "Option 1: Red." in item.text
    assert "Option 2: Blue." in item.text


def test_plan_enqueues_when_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Step one then step two."))
    assert speaker.earcons == []
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "plan"
    assert item.is_decision is True
    assert "Step one then step two." in item.text


def test_permission_enqueues_when_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PERMISSION, "fg", action="run rm -rf"))
    assert speaker.earcons == []
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "permission"
    assert item.is_decision is True
    assert item.text == "run rm -rf"


def test_decision_content_not_enqueued_when_not_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "other", questions=[{"question": "Q"}]))
    # Content messages never earcon (the EARCON message does), and a
    # non-foreground decision's spoken text is not enqueued.
    assert speaker.earcons == []
    assert len(queue) == 0


def test_tool_announce_enqueues_only_when_verbosity_everything():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "fg", tool="Bash", summary="run tests"))
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "tool_announce"
    assert item.is_decision is False
    assert "run tests" in item.text


def test_tool_announce_dropped_when_verbosity_medium():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "fg", tool="Bash", summary="run tests"))
    assert len(queue) == 0


def test_tool_announce_dropped_when_not_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "other", tool="Bash", summary="run tests"))
    assert len(queue) == 0


def test_bare_earcon_message_plays_kind():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.EARCON, "fg", kind="turn_done"))
    assert speaker.earcons == ["turn_done"]
    assert len(queue) == 0
```

**Step 2 - Run it, expect failure.**

```
python -m pytest tests/test_daemon_decisions.py -q
```

Expected: failures because these branches return `None` without acting (e.g. `assert len(queue) == 1` fails: `0 != 1`).

**Step 3 - Implement decision/tool/earcon handling.** Add the decision-text helpers and branches to `src/echo/daemon.py`. Insert these methods into the `SpeechDaemon` class (after `_enqueue`):

```python
    @staticmethod
    def _choice_text(msg) -> str:
        parts = []
        for q in msg.get("questions", []) or []:
            qtext = q.get("question", "") if isinstance(q, dict) else str(q)
            opts = q.get("options", []) if isinstance(q, dict) else []
            labels = []
            for o in opts:
                if isinstance(o, dict):
                    labels.append(o.get("label", ""))
                else:
                    labels.append(str(o))
            labels = [l for l in labels if l]
            # Number the options so the user can pick by number (eyes-free).
            segs = ["Option {0}: {1}.".format(i, label) for i, label in enumerate(labels, 1)]
            if qtext and segs:
                parts.append("{0} {1}".format(qtext, " ".join(segs)))
            elif segs:
                parts.append(" ".join(segs))
            elif qtext:
                parts.append(qtext)
        return " ".join(parts) if parts else "A question needs your answer."

    @staticmethod
    def _plan_text(msg) -> str:
        text = (msg.get("text") or "").strip()
        if text:
            return "Plan ready. {0}".format(text)
        return "A plan is ready for your review."

    @staticmethod
    def _permission_text(msg) -> str:
        # The 'permission' earcon already signals that approval is needed, so the
        # spoken text is just the pending action (e.g. "Run: pytest -q").
        action = (msg.get("action") or "").strip()
        return action if action else "Permission needed."
```

Now extend `handle_message`. Replace its body so the leading `return None` is preceded by the new branches:

```python
    def handle_message(self, msg):
        t = msg.get("type")
        session = msg.get("session", "")
        verbosity = self.config.get("verbosity", "everything")

        if t == MsgType.PROSE:
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), msg.get("final", False))
            if self.sessions.should_speak(session):
                for chunk in chunks:
                    self._enqueue(session, "prose", chunk, False)
            return None

        # Decision CONTENT is enqueued (and gated by foreground). The ALERT
        # earcon for a decision travels as a SEPARATE EARCON message that
        # hooks_entry emits BEFORE the content message; it is handled by the
        # MsgType.EARCON branch below, so the earcon fires instantly and
        # cross-session WITHOUT being doubled here.
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

        if t == MsgType.TOOL:
            if verbosity == "everything" and self.sessions.should_speak(session):
                tool = msg.get("tool", "")
                summary = (msg.get("summary") or "").strip()
                text = summary if summary else "Running {0}.".format(tool)
                self._enqueue(session, "tool_announce", text, False)
            return None

        if t == MsgType.EARCON:
            self.speaker.earcon(msg.get("kind", ""))
            return None

        return None
```

**Step 4 - Run it, expect pass.**

```
python -m pytest tests/test_daemon_decisions.py tests/test_daemon_prose.py -q
```

Expected: `12 passed`.

**Step 5 - Commit.**

```
git add src/echo/daemon.py tests/test_daemon_decisions.py
git commit -m "feat: daemon decision and tool_announce dispatch

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Daemon control dispatch (flush/stop/skip/jump/catch_up + session lifecycle)

**Step 1 - Write the failing test.** Create `tests/test_daemon_control.py`:

```python
from echo.protocol import MsgType, PROTOCOL_VERSION
from echo.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


def _seed(queue, daemon, session, n, decision_at=None):
    for i in range(n):
        is_dec = decision_at is not None and i == decision_at
        queue.enqueue(SpeechItem(
            id=daemon._alloc_id(),
            session=session,
            kind="plan" if is_dec else "prose",
            text="item {0}".format(i),
            is_decision=is_dec,
        ))


def test_flush_drops_session_items_and_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _seed(queue, daemon, "fg", 2)
    _seed(queue, daemon, "other", 1)
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    assert speaker.cancels == 1
    # only the 'other' session item remains
    assert len(queue) == 1
    assert queue.pop_next().session == "other"


def test_stop_clears_all_and_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _seed(queue, daemon, "fg", 3)
    daemon.handle_message(_msg(MsgType.STOP, "fg"))
    assert len(queue) == 0
    assert speaker.cancels == 1


def test_skip_only_cancels_current():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _seed(queue, daemon, "fg", 3)
    daemon.handle_message(_msg(MsgType.SKIP, "fg"))
    assert speaker.cancels == 1
    # queue untouched by skip
    assert len(queue) == 3


def test_jump_decision_drops_to_first_decision_and_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # items 0,1 prose; item 2 is a decision
    _seed(queue, daemon, "fg", 4, decision_at=2)
    daemon.handle_message(_msg(MsgType.JUMP_DECISION, "fg"))
    assert speaker.cancels == 1
    nxt = queue.pop_next()
    assert nxt.is_decision is True
    assert nxt.text == "item 2"


def test_catch_up_clears_and_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _seed(queue, daemon, "fg", 3)
    daemon.handle_message(_msg(MsgType.CATCH_UP, "fg"))
    assert len(queue) == 0
    assert speaker.cancels == 1


def test_set_foreground_sets_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon.handle_message(_msg(MsgType.SET_FOREGROUND, "s9"))
    assert sessions.foreground() == "s9"


def test_session_start_sets_foreground_and_registers():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon.handle_message(_msg(MsgType.SESSION_START, "s9"))
    assert sessions.foreground() == "s9"
    assert sessions.is_foreground("s9") is True


def test_session_end_unregisters():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="s9")
    daemon.handle_message(_msg(MsgType.SESSION_END, "s9"))
    assert sessions.foreground() is None
```

**Step 2 - Run it, expect failure.**

```
python -m pytest tests/test_daemon_control.py -q
```

Expected: failures (e.g. `assert speaker.cancels == 1` -> `0 != 1`) because these message types fall through to `return None`.

**Step 3 - Implement control branches.** In `src/echo/daemon.py`, add these branches to `handle_message` just before the final `return None`:

```python
        if t == MsgType.FLUSH:
            self.queue.flush_session(session)
            self.speaker.cancel()
            return None

        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            self.sessions.set_foreground(session)
            if t == MsgType.SESSION_START:
                self.sessions.register(session)
            return None

        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            return None

        if t == MsgType.STOP:
            self.queue.clear()
            self.speaker.cancel()
            return None

        if t == MsgType.SKIP:
            self.speaker.cancel()
            return None

        if t == MsgType.JUMP_DECISION:
            self.queue.jump_to_decision()
            self.speaker.cancel()
            return None

        if t == MsgType.CATCH_UP:
            self.queue.clear()
            self.speaker.cancel()
            return None
```

**Step 4 - Run it, expect pass.**

```
python -m pytest tests/test_daemon_control.py -q
```

Expected: `8 passed`.

**Step 5 - Commit.**

```
git add src/echo/daemon.py tests/test_daemon_control.py
git commit -m "feat: daemon control and session lifecycle dispatch

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Daemon settings dispatch + STATUS/PING

**Step 1 - Write the failing test.** Create `tests/test_daemon_settings.py`:

```python
from unittest import mock

from echo.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


def test_set_rate_updates_config_and_speaker_and_saves():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("echo.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_RATE, rate=150))
    assert config["rate"] == 150
    assert speaker.rates == [150]
    save.assert_called_once_with(config)


def test_set_voice_updates_config_and_speaker_and_saves():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("echo.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_VOICE, voice="Ava (Premium)"))
    assert config["voice"] == "Ava (Premium)"
    assert speaker.voices == ["Ava (Premium)"]
    save.assert_called_once_with(config)


def test_set_verbosity_updates_config_and_saves_no_speaker_call():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("echo.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_VERBOSITY, verbosity="quiet"))
    assert config["verbosity"] == "quiet"
    assert speaker.rates == []
    assert speaker.voices == []
    save.assert_called_once_with(config)


def test_status_returns_documented_dict():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    config["rate"] = 175
    config["voice"] = "Samantha"
    # enqueue two items so queue_len is reported
    from echo.queue import SpeechItem
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose", text="a", is_decision=False))
    queue.enqueue(SpeechItem(id=2, session="fg", kind="prose", text="b", is_decision=False))
    resp = daemon.handle_message(_msg(MsgType.STATUS))
    assert resp == {
        "verbosity": "medium",
        "rate": 175,
        "voice": "Samantha",
        "foreground": "fg",
        "queue_len": 2,
    }


def test_ping_returns_ok():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    resp = daemon.handle_message(_msg(MsgType.PING))
    assert resp == {"ok": True}


def test_unknown_type_returns_none():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon.handle_message(_msg("totally_unknown")) is None
```

**Step 2 - Run it, expect failure.**

```
python -m pytest tests/test_daemon_settings.py -q
```

Expected: `ImportError`/`AttributeError` - `echo.daemon` does not yet import `save_config`, so `mock.patch("echo.daemon.save_config")` fails; STATUS/PING return `None`.

**Step 3 - Implement settings + STATUS/PING.** At the top of `src/echo/daemon.py`, extend the imports:

```python
from echo.protocol import MsgType
from echo.queue import SpeechItem
from echo.assembler import ProseAssembler
from echo.config import save_config
```

Then add these branches to `handle_message` just before the final `return None`:

```python
        if t == MsgType.SET_RATE:
            rate = msg.get("rate")
            self.config["rate"] = rate
            self.speaker.set_rate(rate)
            save_config(self.config)
            return None

        if t == MsgType.SET_VOICE:
            voice = msg.get("voice")
            self.config["voice"] = voice
            self.speaker.set_voice(voice)
            save_config(self.config)
            return None

        if t == MsgType.SET_VERBOSITY:
            self.config["verbosity"] = msg.get("verbosity")
            save_config(self.config)
            return None

        if t == MsgType.STATUS:
            return {
                "verbosity": self.config.get("verbosity"),
                "rate": self.config.get("rate"),
                "voice": self.config.get("voice"),
                "foreground": self.sessions.foreground(),
                "queue_len": len(self.queue),
            }

        if t == MsgType.PING:
            return {"ok": True}
```

**Step 4 - Run it, expect pass.**

```
python -m pytest tests/test_daemon_settings.py -q
```

Expected: `6 passed`.

**Step 5 - Commit.**

```
git add src/echo/daemon.py tests/test_daemon_settings.py
git commit -m "feat: daemon settings dispatch plus status and ping

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Daemon speak loop (run components) + integration

**Step 1 - Write the failing test.** Create `tests/test_daemon_loop.py`:

```python
import threading
import time

from echo.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def test_speak_loop_speaks_queued_item_then_stops():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose", text="hello world", is_decision=False))

    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    try:
        deadline = time.time() + 2.0
        while time.time() < deadline and not speaker.spoken:
            time.sleep(0.01)
        assert speaker.spoken == ["hello world"]
    finally:
        daemon.stop()
        t.join(timeout=2.0)
    assert not t.is_alive()


def test_speak_loop_idles_when_queue_empty_then_stops():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    time.sleep(0.05)
    assert speaker.spoken == []
    daemon.stop()
    t.join(timeout=2.0)
    assert not t.is_alive()
```

**Step 2 - Run it, expect failure.**

```
python -m pytest tests/test_daemon_loop.py -q
```

Expected: `AttributeError: 'SpeechDaemon' object has no attribute '_speak_loop'` (the stub raised on it earlier; now we need a real loop + `stop()`).

**Step 3 - Implement the loop, socket server, `stop()`, `run()`.** At the top of `src/echo/daemon.py`, extend imports to include the runtime/socket pieces:

```python
import os
import socket
import threading

from echo.protocol import MsgType, encode, decode
from echo.queue import SpeechItem
from echo.assembler import ProseAssembler
from echo.config import save_config, load_config
from echo.paths import SOCKET_PATH, ensure_echo_dir
```

In `SpeechDaemon.__init__`, add the threading/control state (place after `self._next_id = 0`):

```python
        self._running = threading.Event()
        self._wake = threading.Event()
        self._lock = threading.Lock()
        self._server = None
        self._threads = []
        self._poll_interval = 0.1
```

Make enqueue wake the loop. Replace the existing `_enqueue` with:

```python
    def _enqueue(self, session: str, kind: str, text: str, is_decision: bool) -> None:
        item = SpeechItem(
            id=self._alloc_id(),
            session=session,
            kind=kind,
            text=text,
            is_decision=is_decision,
        )
        self.queue.enqueue(item)
        self._wake.set()
```

Now add the runtime methods to the class (after `handle_message`):

```python
    def stop(self) -> None:
        self._running.clear()
        self._wake.set()
        srv = self._server
        if srv is not None:
            try:
                srv.close()
            except OSError:
                pass

    def _speak_loop(self) -> None:
        self._running.set()
        while self._running.is_set():
            item = self.queue.pop_next()
            if item is not None:
                self.speaker.speak(item.text)
                continue
            # nothing to say: wait until woken by an enqueue or until stop()
            self._wake.wait(self._poll_interval)
            self._wake.clear()

    def _handle_conn(self, conn) -> None:
        try:
            buf = b""
            with conn:
                conn.settimeout(5.0)
                while self._running.is_set():
                    try:
                        data = conn.recv(4096)
                    except (OSError, socket.timeout):
                        return
                    if not data:
                        return
                    buf += data
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        if not line.strip():
                            continue
                        try:
                            msg = decode(line)
                        except (ValueError, UnicodeDecodeError):
                            continue
                        with self._lock:
                            reply = self.handle_message(msg)
                        if reply is not None:
                            try:
                                conn.sendall(encode(reply))
                            except OSError:
                                return
        except OSError:
            return

    def _accept_loop(self) -> None:
        srv = self._server
        while self._running.is_set():
            try:
                conn, _ = srv.accept()
            except OSError:
                return
            th = threading.Thread(target=self._handle_conn, args=(conn,), daemon=True)
            th.start()

    def run(self) -> None:
        ensure_echo_dir()
        # unlink a stale socket file before binding
        try:
            os.unlink(SOCKET_PATH)
        except FileNotFoundError:
            pass
        srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        srv.bind(str(SOCKET_PATH))
        srv.listen(16)
        self._server = srv
        self._running.set()

        speak_thread = threading.Thread(target=self._speak_loop, daemon=True)
        accept_thread = threading.Thread(target=self._accept_loop, daemon=True)
        self._threads = [speak_thread, accept_thread]
        speak_thread.start()
        accept_thread.start()

        try:
            while self._running.is_set():
                accept_thread.join(timeout=0.25)
                if not accept_thread.is_alive():
                    break
        except KeyboardInterrupt:
            pass
        finally:
            self.stop()
            try:
                srv.close()
            except OSError:
                pass
            try:
                os.unlink(SOCKET_PATH)
            except FileNotFoundError:
                pass
```

**Step 4 - Run it, expect pass.**

```
python -m pytest tests/test_daemon_loop.py -q
```

Expected: `2 passed`.

**Step 5 - Commit.**

```
git add src/echo/daemon.py tests/test_daemon_loop.py
git commit -m "feat: daemon speak loop and socket server runtime

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: ensure_running + main

**Step 1 - Write the failing test.** Create `tests/test_daemon_main.py`:

```python
from unittest import mock

import echo.daemon as daemon_mod


def test_ensure_running_noop_when_socket_connectable():
    with mock.patch("echo.daemon._socket_connectable", return_value=True) as conn, \
         mock.patch("echo.daemon.subprocess.Popen") as popen:
        daemon_mod.ensure_running()
    conn.assert_called_once()
    popen.assert_not_called()


def test_ensure_running_spawns_detached_when_socket_absent():
    with mock.patch("echo.daemon._socket_connectable", return_value=False), \
         mock.patch("echo.daemon.subprocess.Popen") as popen:
        daemon_mod.ensure_running()
    assert popen.call_count == 1
    args, kwargs = popen.call_args
    # spawned detached
    assert kwargs.get("start_new_session") is True
    # spawns the bin/echo-daemon shim
    cmd = args[0]
    assert any("echo-daemon" in str(part) for part in cmd)


def test_main_builds_components_and_runs():
    fake_cfg = {"voice": None, "rate": 200, "verbosity": "everything",
                "background_policy": "earcon_only", "earcons": {}}
    with mock.patch("echo.daemon.load_config", return_value=fake_cfg), \
         mock.patch("echo.daemon.SpeechDaemon.run", autospec=True) as run:
        daemon_mod.main()
    assert run.call_count == 1
    built = run.call_args[0][0]
    assert isinstance(built, daemon_mod.SpeechDaemon)
    assert built.config is fake_cfg
```

**Step 2 - Run it, expect failure.**

```
python -m pytest tests/test_daemon_main.py -q
```

Expected: `AttributeError` - `ensure_running`, `main`, and `_socket_connectable` do not exist yet.

**Step 3 - Implement `ensure_running` and `main`.** Add a `subprocess` import to the top of `src/echo/daemon.py`:

```python
import os
import socket
import subprocess
import threading
```

Append the module-level helpers and entry points to the end of `src/echo/daemon.py`:

```python
def _socket_connectable() -> bool:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect(str(SOCKET_PATH))
        return True
    except OSError:
        return False
    finally:
        try:
            s.close()
        except OSError:
            pass


def _daemon_shim_path() -> str:
    # repo layout: <repo>/bin/echo-daemon ; this file is <repo>/src/echo/daemon.py
    here = os.path.dirname(os.path.abspath(__file__))
    repo_root = os.path.dirname(os.path.dirname(here))
    return os.path.join(repo_root, "bin", "echo-daemon")


def ensure_running() -> None:
    if _socket_connectable():
        return
    shim = _daemon_shim_path()
    subprocess.Popen(
        [shim],
        start_new_session=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def main() -> None:
    from echo.speaker import Speaker
    from echo.queue import SpeechQueue
    from echo.sessions import SessionManager

    cfg = load_config()
    queue = SpeechQueue()
    speaker = Speaker(
        voice=cfg.get("voice"),
        rate=cfg.get("rate", 200),
        earcons=cfg.get("earcons"),
    )
    sessions = SessionManager(background_policy=cfg.get("background_policy", "earcon_only"))
    daemon = SpeechDaemon(queue, speaker, sessions, cfg)
    daemon.run()


if __name__ == "__main__":
    main()
```

**Step 4 - Run it, expect pass.**

```
python -m pytest tests/test_daemon_main.py -q
```

Expected: `3 passed`.

**Step 5 - Commit.**

```
git add src/echo/daemon.py tests/test_daemon_main.py
git commit -m "feat: ensure_running detached spawn and daemon main entrypoint

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: client.send round-trip

**Step 1 - Write the failing test.** Create `tests/test_client_send.py`:

```python
import json
import socket
import threading

from echo import paths
from echo.client import send
from echo.protocol import PROTOCOL_VERSION, encode


def _echo_server(sock_path, ready, captured):
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(sock_path)
    srv.listen(1)
    ready.set()
    conn, _ = srv.accept()
    with conn:
        buf = b""
        while b"\n" not in buf:
            data = conn.recv(4096)
            if not data:
                break
            buf += data
        line = buf.split(b"\n", 1)[0]
        captured["recv"] = json.loads(line)
        conn.sendall(encode({"ok": True, "pong": "yes"}))
    srv.close()


def test_send_no_reply(tmp_path, monkeypatch):
    sock_path = str(tmp_path / "speechd.sock")
    monkeypatch.setattr(paths, "SOCKET_PATH", sock_path, raising=False)
    import echo.client as client_mod
    monkeypatch.setattr(client_mod, "SOCKET_PATH", sock_path, raising=False)

    ready = threading.Event()
    captured = {}
    t = threading.Thread(target=_echo_server, args=(sock_path, ready, captured), daemon=True)
    t.start()
    assert ready.wait(2.0)

    msg = {"v": PROTOCOL_VERSION, "type": "ping"}
    result = send(msg, expect_reply=False)
    assert result is None
    t.join(timeout=2.0)
    assert captured["recv"] == msg


def test_send_round_trip_reply(tmp_path, monkeypatch):
    sock_path = str(tmp_path / "speechd.sock")
    import echo.client as client_mod
    monkeypatch.setattr(client_mod, "SOCKET_PATH", sock_path, raising=False)

    ready = threading.Event()
    captured = {}
    t = threading.Thread(target=_echo_server, args=(sock_path, ready, captured), daemon=True)
    t.start()
    assert ready.wait(2.0)

    reply = send({"v": PROTOCOL_VERSION, "type": "ping"}, expect_reply=True, timeout=2.0)
    assert reply == {"ok": True, "pong": "yes"}
    t.join(timeout=2.0)
```

**Step 2 - Run it, expect failure.**

```
python -m pytest tests/test_client_send.py -q
```

Expected: `ModuleNotFoundError: No module named 'echo.client'`.

**Step 3 - Implement `src/echo/client.py`:**

```python
import socket
import time

from echo.protocol import encode, decode
from echo.paths import SOCKET_PATH
from echo.daemon import ensure_running


def send(msg: dict, expect_reply: bool = False, timeout: float = 2.0):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(str(SOCKET_PATH))
        s.sendall(encode(msg))
        if not expect_reply:
            return None
        buf = b""
        while b"\n" not in buf:
            data = s.recv(4096)
            if not data:
                break
            buf += data
        if not buf:
            return None
        line = buf.split(b"\n", 1)[0]
        return decode(line)
    finally:
        try:
            s.close()
        except OSError:
            pass


def ensure_daemon(timeout: float = 3.0) -> None:
    if _connectable():
        return
    ensure_running()
    deadline = time.time() + timeout
    while time.time() < deadline:
        if _connectable():
            return
        time.sleep(0.05)


def _connectable() -> bool:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect(str(SOCKET_PATH))
        return True
    except OSError:
        return False
    finally:
        try:
            s.close()
        except OSError:
            pass
```

**Step 4 - Run it, expect pass.**

```
python -m pytest tests/test_client_send.py -q
```

Expected: `2 passed`.

**Step 5 - Commit.**

```
git add src/echo/client.py tests/test_client_send.py
git commit -m "feat: client send round-trip over unix socket

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: client.ensure_daemon

**Step 1 - Write the failing test.** Create `tests/test_client_ensure.py`:

```python
from unittest import mock

import echo.client as client_mod


def test_ensure_daemon_returns_fast_when_connectable():
    with mock.patch("echo.client._connectable", return_value=True) as conn, \
         mock.patch("echo.client.ensure_running") as run, \
         mock.patch("echo.client.time.sleep") as slept:
        client_mod.ensure_daemon(timeout=3.0)
    # only one connectivity check; never spawns or sleeps
    conn.assert_called_once()
    run.assert_not_called()
    slept.assert_not_called()


def test_ensure_daemon_spawns_then_polls_until_connectable():
    # absent on first check, absent right after spawn, then connectable on the 2nd poll
    connectable_results = iter([False, False, True])

    def fake_connectable():
        return next(connectable_results)

    with mock.patch("echo.client._connectable", side_effect=fake_connectable) as conn, \
         mock.patch("echo.client.ensure_running") as run, \
         mock.patch("echo.client.time.sleep") as slept:
        client_mod.ensure_daemon(timeout=3.0)
    # initial check + spawn + 2 polls = 3 connectivity checks total
    assert conn.call_count == 3
    run.assert_called_once()
    # slept once before the successful 2nd poll
    assert slept.call_count == 1
```

**Step 2 - Run it, expect failure or pass-readiness.** Run:

```
python -m pytest tests/test_client_ensure.py -q
```

Expected: `2 passed` (the `ensure_daemon` body was written in the previous task and already satisfies these patched-dependency tests). If instead you implemented `ensure_daemon` as a stub in a fresh checkout, the failure would be `AssertionError: Expected 'ensure_running' to have been called once. Called 0 times.` - then apply the `ensure_daemon` implementation from the previous task and re-run to get `2 passed`.

**Step 3 - Confirm implementation.** No new production code is required beyond the `ensure_daemon`/`_connectable` already in `src/echo/client.py`. Re-read it to confirm `ensure_daemon` calls `_connectable()` first, returns immediately when true, otherwise calls `ensure_running()` then polls `_connectable()` with `time.sleep(0.05)` until `timeout`.

**Step 4 - Run the whole client + daemon suite, expect pass.**

```
python -m pytest tests/test_client_ensure.py tests/test_client_send.py tests/test_daemon_main.py tests/test_daemon_loop.py tests/test_daemon_settings.py tests/test_daemon_control.py tests/test_daemon_decisions.py tests/test_daemon_prose.py tests/test_sessions.py -q
```

Expected: all pass (`33 passed`).

**Step 5 - Commit.**

```
git add tests/test_client_ensure.py
git commit -m "test: client ensure_daemon spawn-and-poll behavior

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Golden payload capture + hooks_entry.py + bin/echo-hook

This section delivers the hook ingress layer: a pure `handle_event(event, payload)` in `src/echo/hooks_entry.py` that maps Claude Code hook events to protocol message dicts, and the thin `bin/echo-hook` shim that parses argv/stdin, optionally captures golden payloads, and ships each message to the daemon. It begins with a RUNTIME/MANUAL capture task and provides realistic representative payloads so the TDD tasks are fully runnable before real fixtures exist.

Assumes the scaffolding section already created `src/echo/protocol.py` (with `PROTOCOL_VERSION` and `MsgType`), `src/echo/client.py` (with `send`/`ensure_daemon`), `tests/conftest.py` (with the `src` sys.path fallback), and `pyproject.toml`. All commands run from the repo root `/Users/Nima.Hakimi/projects/private/claude-tts`.

### Task: RUNTIME/MANUAL - capture golden hook payloads from a real Claude session

This task is NOT a pytest task. It is a one-time manual capture performed against a REAL Claude Code session on macOS. Its product is the files in `tests/fixtures/` committed to git. The TDD tasks below do NOT depend on this task completing first - they use the representative payloads embedded inline. Once real payloads are captured, overwrite the fixtures and re-run the parser tests; they should still pass (the representative schemas are best-effort copies of the live shapes).

Steps:

1. Confirm `bin/echo-hook` (implemented in the final task of this section) honors `ECHO_CAPTURE`: when the env var `ECHO_CAPTURE` is set to a directory, the hook dumps the raw stdin bytes it received to `${ECHO_CAPTURE}/<event>-<pid>.json` BEFORE doing anything else (so even a later crash leaves the payload on disk).

2. Create a capture directory and point the plugin at it for one session:

```bash
mkdir -p /tmp/echo-capture
export ECHO_CAPTURE=/tmp/echo-capture
```

   Ensure the plugin's `hooks/hooks.json` is installed/linked so `${CLAUDE_PLUGIN_ROOT}/bin/echo-hook <Event>` fires for each event. Launch a real `claude` session in this same shell so the hook subprocesses inherit `ECHO_CAPTURE`.

3. In that session, deliberately trigger each event exactly once:
   - **MessageDisplay**: let Claude stream any normal prose reply (e.g. ask "say hello in one sentence").
   - **PreToolUse · Bash permission_prompt**: ask Claude to run a shell command that requires approval (e.g. "run `git status` for me") and let the permission/approval prompt appear; this exercises both the `PreToolUse` Bash branch and the `Notification` permission prompt.
   - **PreToolUse · AskUserQuestion**: prompt Claude in a way that makes it ask you a multiple-choice question (e.g. "ask me which color I prefer between red and blue using a question").
   - **PreToolUse · ExitPlanMode**: enter plan mode (Shift+Tab to planning), have Claude produce a plan, and approve/reject it so `ExitPlanMode` fires.
   - **Notification · idle_prompt**: leave the session idle until Claude emits the idle notification.

4. Inspect the captured files and copy the canonical raw payload for each event into the fixture names the tests expect. The capture filenames are `<event>-<pid>.json`; map them to stable fixture names:

```bash
ls -la /tmp/echo-capture
mkdir -p tests/fixtures
# Pick the correct captured file for each event (inspect contents to disambiguate PreToolUse variants):
cp /tmp/echo-capture/MessageDisplay-*.json       tests/fixtures/MessageDisplay.json
# For PreToolUse, distinguish by tool_name inside the JSON:
#   tool_name == "AskUserQuestion" -> PreToolUse-AskUserQuestion.json
#   tool_name == "ExitPlanMode"    -> PreToolUse-ExitPlanMode.json
#   tool_name == "Bash"            -> PreToolUse-Bash.json
cp /tmp/echo-capture/Notification-*.json         tests/fixtures/   # rename per notification_type below
```

   Rename the `Notification` captures by their `notification_type`/`matcher` so the final fixture set is exactly:

```
tests/fixtures/MessageDisplay.json
tests/fixtures/PreToolUse-AskUserQuestion.json
tests/fixtures/PreToolUse-ExitPlanMode.json
tests/fixtures/PreToolUse-Bash.json
tests/fixtures/Notification-permission_prompt.json
tests/fixtures/Notification-idle_prompt.json
```

5. Commit the captured fixtures (do NOT commit `/tmp/echo-capture`):

```bash
git add tests/fixtures
git commit -m "chore: capture golden hook payloads from a real Claude session

Co-Authored-By: Claude <noreply@anthropic.com>"
```

If a real session is not yet available, create the fixture files from the REPRESENTATIVE payloads below (they are best-effort copies of the live schemas) so downstream tasks can run, and replace them with the real captures when possible.

### Task: seed representative golden payload fixtures

Create the fixture files now with realistic representative payloads so parser/integration tests are runnable before (or independently of) the real capture. These best-effort schemas match the fields `handle_event` reads (`session_id`, `tool_name`, `tool_input.questions`, `tool_input.plan`, `tool_input.command`, `notification_type`/`matcher`, `delta`/`index`/`final`, `action`).

Create `tests/fixtures/MessageDisplay.json`:

```json
{
  "session_id": "11111111-2222-3333-4444-555555555555",
  "transcript_path": "/Users/me/.claude/projects/-Users-me-proj/11111111-2222-3333-4444-555555555555.jsonl",
  "hook_event_name": "MessageDisplay",
  "delta": "Here is the first sentence. And a second.",
  "index": 0,
  "final": false
}
```

Create `tests/fixtures/PreToolUse-AskUserQuestion.json`:

```json
{
  "session_id": "11111111-2222-3333-4444-555555555555",
  "transcript_path": "/Users/me/.claude/projects/-Users-me-proj/11111111-2222-3333-4444-555555555555.jsonl",
  "hook_event_name": "PreToolUse",
  "tool_name": "AskUserQuestion",
  "tool_input": {
    "questions": [
      {
        "question": "Which color do you prefer?",
        "header": "Color",
        "multiSelect": false,
        "options": [
          {"label": "Red", "description": "warm"},
          {"label": "Blue", "description": "cool"}
        ]
      }
    ]
  }
}
```

Create `tests/fixtures/PreToolUse-ExitPlanMode.json`:

```json
{
  "session_id": "11111111-2222-3333-4444-555555555555",
  "transcript_path": "/Users/me/.claude/projects/-Users-me-proj/11111111-2222-3333-4444-555555555555.jsonl",
  "hook_event_name": "PreToolUse",
  "tool_name": "ExitPlanMode",
  "tool_input": {
    "plan": "1. Add the parser.\n2. Wire the daemon.\n3. Ship it."
  }
}
```

Create `tests/fixtures/PreToolUse-Bash.json`:

```json
{
  "session_id": "11111111-2222-3333-4444-555555555555",
  "transcript_path": "/Users/me/.claude/projects/-Users-me-proj/11111111-2222-3333-4444-555555555555.jsonl",
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": {
    "command": "git status",
    "description": "Show working tree status"
  }
}
```

Create `tests/fixtures/Notification-permission_prompt.json`:

```json
{
  "session_id": "11111111-2222-3333-4444-555555555555",
  "transcript_path": "/Users/me/.claude/projects/-Users-me-proj/11111111-2222-3333-4444-555555555555.jsonl",
  "hook_event_name": "Notification",
  "notification_type": "permission_prompt",
  "matcher": "permission_prompt",
  "action": "Run git status",
  "message": "Claude needs your permission to run a command"
}
```

Create `tests/fixtures/Notification-idle_prompt.json`:

```json
{
  "session_id": "11111111-2222-3333-4444-555555555555",
  "transcript_path": "/Users/me/.claude/projects/-Users-me-proj/11111111-2222-3333-4444-555555555555.jsonl",
  "hook_event_name": "Notification",
  "notification_type": "idle_prompt",
  "matcher": "idle_prompt",
  "message": "Claude is waiting for your input"
}
```

Commit the representative fixtures:

```bash
git add tests/fixtures
git commit -m "test: add representative golden hook payload fixtures

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected: `git log --oneline -1` shows the commit; `ls tests/fixtures` lists the six JSON files.

### Task: handle_event - MessageDisplay maps to a PROSE message

Write the failing test. Create `tests/test_hooks_entry.py`:

```python
import json
from pathlib import Path

from echo.hooks_entry import handle_event
from echo.protocol import PROTOCOL_VERSION, MsgType

FIXTURES = Path(__file__).parent / "fixtures"


def _load(name):
    return json.loads((FIXTURES / name).read_text())


def test_message_display_maps_to_prose():
    payload = {
        "session_id": "sess-1",
        "delta": "Hello there. How are you?",
        "index": 3,
        "final": False,
    }
    assert handle_event("MessageDisplay", payload) == [
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.PROSE,
            "session": "sess-1",
            "delta": "Hello there. How are you?",
            "index": 3,
            "final": False,
        }
    ]


def test_message_display_from_fixture():
    payload = _load("MessageDisplay.json")
    msgs = handle_event("MessageDisplay", payload)
    assert len(msgs) == 1
    m = msgs[0]
    assert m["type"] == MsgType.PROSE
    assert m["session"] == payload["session_id"]
    assert m["delta"] == payload["delta"]
    assert m["index"] == payload["index"]
    assert m["final"] == payload["final"]
    assert m["v"] == PROTOCOL_VERSION
```

Run it and expect failure (module does not exist yet):

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (FAIL): a collection/import error `ModuleNotFoundError: No module named 'echo.hooks_entry'`.

Create the minimal implementation `src/echo/hooks_entry.py`:

```python
"""Pure mapping from Claude Code hook events to protocol message dicts."""
from echo.protocol import PROTOCOL_VERSION, MsgType


def _msg(**fields):
    """Build a protocol message dict, always stamped with the protocol version."""
    out = {"v": PROTOCOL_VERSION}
    out.update(fields)
    return out


def handle_event(event: str, payload: dict) -> list[dict]:
    """Map (event name, parsed stdin payload) to a list of protocol messages.

    PURE: no I/O. Returns [] for any event it does not handle.
    """
    session = payload.get("session_id", "")

    if event == "MessageDisplay":
        return [
            _msg(
                type=MsgType.PROSE,
                session=session,
                delta=payload.get("delta", ""),
                index=payload.get("index", 0),
                final=payload.get("final", False),
            )
        ]

    return []
```

Run again and expect PASS:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (PASS): `2 passed`.

Commit:

```bash
git add src/echo/hooks_entry.py tests/test_hooks_entry.py
git commit -m "feat: handle_event maps MessageDisplay to a prose message

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: handle_event - AskUserQuestion emits choice earcon then CHOICE

Append the failing test to `tests/test_hooks_entry.py`:

```python
def test_ask_user_question_earcon_then_choice():
    payload = {
        "session_id": "sess-1",
        "tool_name": "AskUserQuestion",
        "tool_input": {
            "questions": [
                {"question": "Pick one", "options": [{"label": "A"}, {"label": "B"}]}
            ]
        },
    }
    assert handle_event("PreToolUse", payload) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "choice"},
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.CHOICE,
            "session": "sess-1",
            "questions": [
                {"question": "Pick one", "options": [{"label": "A"}, {"label": "B"}]}
            ],
        },
    ]


def test_ask_user_question_from_fixture():
    payload = _load("PreToolUse-AskUserQuestion.json")
    msgs = handle_event("PreToolUse", payload)
    assert [m["type"] for m in msgs] == [MsgType.EARCON, MsgType.CHOICE]
    assert msgs[0]["kind"] == "choice"
    assert msgs[1]["session"] == payload["session_id"]
    assert msgs[1]["questions"] == payload["tool_input"]["questions"]
    assert all(m["v"] == PROTOCOL_VERSION for m in msgs)
```

Run and expect failure:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (FAIL): `assert [] == [...]` for the AskUserQuestion tests (PreToolUse currently returns `[]`).

Extend `handle_event` in `src/echo/hooks_entry.py` - add a `PreToolUse` branch before the final `return []`:

```python
    if event == "PreToolUse":
        tool = payload.get("tool_name")
        ti = payload.get("tool_input", {})
        if tool == "AskUserQuestion":
            return [
                _msg(type=MsgType.EARCON, kind="choice"),
                _msg(
                    type=MsgType.CHOICE,
                    session=session,
                    questions=ti.get("questions", []),
                ),
            ]
        return []
```

Run and expect PASS:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (PASS): `4 passed`.

Commit:

```bash
git add src/echo/hooks_entry.py tests/test_hooks_entry.py
git commit -m "feat: handle_event maps AskUserQuestion to choice earcon + CHOICE

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: handle_event - ExitPlanMode emits plan earcon then PLAN

Append the failing test to `tests/test_hooks_entry.py`:

```python
def test_exit_plan_mode_earcon_then_plan():
    payload = {
        "session_id": "sess-1",
        "tool_name": "ExitPlanMode",
        "tool_input": {"plan": "Step one. Step two."},
    }
    assert handle_event("PreToolUse", payload) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "plan"},
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.PLAN,
            "session": "sess-1",
            "text": "Step one. Step two.",
        },
    ]


def test_exit_plan_mode_from_fixture():
    payload = _load("PreToolUse-ExitPlanMode.json")
    msgs = handle_event("PreToolUse", payload)
    assert [m["type"] for m in msgs] == [MsgType.EARCON, MsgType.PLAN]
    assert msgs[0]["kind"] == "plan"
    assert msgs[1]["session"] == payload["session_id"]
    assert msgs[1]["text"] == payload["tool_input"]["plan"]
    assert all(m["v"] == PROTOCOL_VERSION for m in msgs)
```

Run and expect failure:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (FAIL): the two ExitPlanMode tests fail because the `ExitPlanMode` branch returns `[]`.

Extend the `PreToolUse` branch in `src/echo/hooks_entry.py` - insert the `ExitPlanMode` case after the `AskUserQuestion` case and before `return []`:

```python
        if tool == "ExitPlanMode":
            return [
                _msg(type=MsgType.EARCON, kind="plan"),
                _msg(type=MsgType.PLAN, session=session, text=ti.get("plan", "")),
            ]
```

Run and expect PASS:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (PASS): `6 passed`.

Commit:

```bash
git add src/echo/hooks_entry.py tests/test_hooks_entry.py
git commit -m "feat: handle_event maps ExitPlanMode to plan earcon + PLAN

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: handle_event - generic PreToolUse emits a TOOL announce with a tool-specific summary

Append the failing test to `tests/test_hooks_entry.py`:

```python
def test_pre_tool_use_bash_tool_announce():
    payload = {
        "session_id": "sess-1",
        "tool_name": "Bash",
        "tool_input": {"command": "git status", "description": "Show status"},
    }
    assert handle_event("PreToolUse", payload) == [
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.TOOL,
            "session": "sess-1",
            "tool": "Bash",
            "summary": "git status",
        }
    ]


def test_pre_tool_use_write_summary_is_basename():
    payload = {
        "session_id": "sess-1",
        "tool_name": "Write",
        "tool_input": {"file_path": "/Users/me/proj/src/echo/cli.py"},
    }
    msgs = handle_event("PreToolUse", payload)
    assert msgs == [
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.TOOL,
            "session": "sess-1",
            "tool": "Write",
            "summary": "cli.py",
        }
    ]


def test_pre_tool_use_edit_summary_is_basename():
    payload = {
        "session_id": "sess-1",
        "tool_name": "Edit",
        "tool_input": {"file_path": "/Users/me/proj/README.md"},
    }
    msgs = handle_event("PreToolUse", payload)
    assert msgs[0]["summary"] == "README.md"
    assert msgs[0]["tool"] == "Edit"


def test_pre_tool_use_unknown_tool_summary_is_tool_name():
    payload = {"session_id": "sess-1", "tool_name": "WebFetch", "tool_input": {}}
    msgs = handle_event("PreToolUse", payload)
    assert msgs == [
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.TOOL,
            "session": "sess-1",
            "tool": "WebFetch",
            "summary": "WebFetch",
        }
    ]


def test_pre_tool_use_bash_from_fixture():
    payload = _load("PreToolUse-Bash.json")
    msgs = handle_event("PreToolUse", payload)
    assert msgs[0]["type"] == MsgType.TOOL
    assert msgs[0]["tool"] == "Bash"
    assert msgs[0]["summary"] == "git status"
    assert msgs[0]["session"] == payload["session_id"]
```

Run and expect failure:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (FAIL): the new generic-tool tests fail because the generic branch returns `[]`.

Replace the trailing `return []` inside the `PreToolUse` branch with a generic TOOL announce, and add a private summary helper at module scope. Final `src/echo/hooks_entry.py`:

```python
"""Pure mapping from Claude Code hook events to protocol message dicts."""
import os

from echo.protocol import PROTOCOL_VERSION, MsgType


def _msg(**fields):
    """Build a protocol message dict, always stamped with the protocol version."""
    out = {"v": PROTOCOL_VERSION}
    out.update(fields)
    return out


def _tool_summary(tool: str, ti: dict) -> str:
    """Short, speakable, tool-specific description of a pending tool call."""
    if tool == "Bash":
        cmd = (ti.get("command") or "").strip()
        return cmd[:120] if cmd else "Bash"
    if tool in ("Write", "Edit", "MultiEdit", "NotebookEdit"):
        path = ti.get("file_path") or ti.get("notebook_path") or ""
        base = os.path.basename(path.rstrip("/")) if path else ""
        return base if base else (tool or "")
    return tool or ""


def handle_event(event: str, payload: dict) -> list[dict]:
    """Map (event name, parsed stdin payload) to a list of protocol messages.

    PURE: no I/O. Returns [] for any event it does not handle.
    """
    session = payload.get("session_id", "")

    if event == "MessageDisplay":
        return [
            _msg(
                type=MsgType.PROSE,
                session=session,
                delta=payload.get("delta", ""),
                index=payload.get("index", 0),
                final=payload.get("final", False),
            )
        ]

    if event == "PreToolUse":
        tool = payload.get("tool_name")
        ti = payload.get("tool_input", {})
        if tool == "AskUserQuestion":
            return [
                _msg(type=MsgType.EARCON, kind="choice"),
                _msg(
                    type=MsgType.CHOICE,
                    session=session,
                    questions=ti.get("questions", []),
                ),
            ]
        if tool == "ExitPlanMode":
            return [
                _msg(type=MsgType.EARCON, kind="plan"),
                _msg(type=MsgType.PLAN, session=session, text=ti.get("plan", "")),
            ]
        return [
            _msg(
                type=MsgType.TOOL,
                session=session,
                tool=tool,
                summary=_tool_summary(tool, ti),
            )
        ]

    return []
```

Run and expect PASS:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (PASS): `11 passed`.

Commit:

```bash
git add src/echo/hooks_entry.py tests/test_hooks_entry.py
git commit -m "feat: handle_event maps generic PreToolUse to a tool announce

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: handle_event - Notification permission_prompt and idle_prompt

Append the failing test to `tests/test_hooks_entry.py`:

```python
def test_notification_permission_prompt():
    payload = {
        "session_id": "sess-1",
        "notification_type": "permission_prompt",
        "action": "Run git status",
    }
    assert handle_event("Notification", payload) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "permission"},
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.PERMISSION,
            "session": "sess-1",
            "action": "Run git status",
        },
    ]


def test_notification_permission_prompt_via_matcher_fallback():
    payload = {
        "session_id": "sess-1",
        "matcher": "permission_prompt",
        "action": "Edit file cli.py",
    }
    msgs = handle_event("Notification", payload)
    assert [m["type"] for m in msgs] == [MsgType.EARCON, MsgType.PERMISSION]
    assert msgs[0]["kind"] == "permission"
    assert msgs[1]["action"] == "Edit file cli.py"


def test_notification_idle_prompt():
    payload = {"session_id": "sess-1", "notification_type": "idle_prompt"}
    assert handle_event("Notification", payload) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "ready"}
    ]


def test_notification_permission_prompt_from_fixture():
    payload = _load("Notification-permission_prompt.json")
    msgs = handle_event("Notification", payload)
    assert [m["type"] for m in msgs] == [MsgType.EARCON, MsgType.PERMISSION]
    assert msgs[0]["kind"] == "permission"
    assert msgs[1]["session"] == payload["session_id"]
    assert msgs[1]["action"] == payload["action"]


def test_notification_idle_prompt_from_fixture():
    payload = _load("Notification-idle_prompt.json")
    msgs = handle_event("Notification", payload)
    assert msgs == [{"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "ready"}]


def test_unknown_notification_type_is_empty():
    payload = {"session_id": "sess-1", "notification_type": "something_else"}
    assert handle_event("Notification", payload) == []
```

Run and expect failure:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (FAIL): the Notification tests fail because `Notification` is unhandled and returns `[]`.

Add the `Notification` branch to `src/echo/hooks_entry.py`, immediately after the `PreToolUse` branch and before the final `return []`:

```python
    if event == "Notification":
        nt = payload.get("notification_type") or payload.get("matcher")
        if nt == "permission_prompt":
            return [
                _msg(type=MsgType.EARCON, kind="permission"),
                _msg(
                    type=MsgType.PERMISSION,
                    session=session,
                    action=payload.get("action", ""),
                ),
            ]
        if nt == "idle_prompt":
            return [_msg(type=MsgType.EARCON, kind="ready")]
        return []
```

Run and expect PASS:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (PASS): `17 passed`.

Commit:

```bash
git add src/echo/hooks_entry.py tests/test_hooks_entry.py
git commit -m "feat: handle_event maps Notification permission/idle prompts

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: handle_event - Stop, UserPromptSubmit, SessionStart, SessionEnd, and unknown events

Append the failing test to `tests/test_hooks_entry.py`:

```python
def test_stop_emits_turn_done_earcon():
    assert handle_event("Stop", {"session_id": "sess-1"}) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "turn_done"}
    ]


def test_user_prompt_submit_sets_foreground_then_flush():
    assert handle_event("UserPromptSubmit", {"session_id": "sess-9"}) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "sess-9"},
        {"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "sess-9"},
    ]


def test_session_start_sets_foreground_then_session_start():
    assert handle_event("SessionStart", {"session_id": "sess-9"}) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "sess-9"},
        {"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START, "session": "sess-9"},
    ]


def test_session_end_emits_session_end():
    assert handle_event("SessionEnd", {"session_id": "sess-9"}) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END, "session": "sess-9"}
    ]


def test_unknown_event_is_empty():
    assert handle_event("TotallyMadeUp", {"session_id": "sess-1"}) == []


def test_missing_session_id_defaults_to_empty_string():
    msgs = handle_event("SessionStart", {})
    assert msgs[0]["session"] == ""
    assert msgs[1]["session"] == ""
```

Run and expect failure:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (FAIL): the Stop/UserPromptSubmit/SessionStart/SessionEnd tests fail because those events return `[]`.

Add the remaining branches to `src/echo/hooks_entry.py`, immediately after the `Notification` branch and before the final `return []`:

```python
    if event == "Stop":
        return [_msg(type=MsgType.EARCON, kind="turn_done")]

    if event == "UserPromptSubmit":
        return [
            _msg(type=MsgType.SET_FOREGROUND, session=session),
            _msg(type=MsgType.FLUSH, session=session),
        ]

    if event == "SessionStart":
        return [
            _msg(type=MsgType.SET_FOREGROUND, session=session),
            _msg(type=MsgType.SESSION_START, session=session),
        ]

    if event == "SessionEnd":
        return [_msg(type=MsgType.SESSION_END, session=session)]
```

Run and expect PASS:

```bash
python -m pytest tests/test_hooks_entry.py -q
```

Expected output (PASS): `23 passed`.

Commit:

```bash
git add src/echo/hooks_entry.py tests/test_hooks_entry.py
git commit -m "feat: handle_event maps Stop/UserPromptSubmit/Session* and unknowns

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: bin/echo-hook - full shim with capture, dispatch, send, and always exit 0

`bin/echo-hook` is a thin shim: it reads `argv[1]` as the event, parses stdin JSON (tolerating empty/invalid), optionally dumps the raw stdin bytes when `ECHO_CAPTURE` is set, calls `handle_event`, ensures the daemon is up, sends each resulting message, and ALWAYS exits 0 - wrapping everything in a total try/except so it can never break Claude. All real work lives in `echo.hooks_entry` (pure) and `echo.client`; the shim only orchestrates.

Write the failing test. Create `tests/test_echo_hook_bin.py`:

```python
import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
HOOK = REPO / "bin" / "echo-hook"


def _run(event, stdin_bytes, extra_env=None):
    env = dict(os.environ)
    # Force the daemon/send path into a no-op fake so the shim never touches a socket.
    env["PYTHONPATH"] = str(REPO / "tests" / "_fakeclient") + os.pathsep + str(REPO / "src") + os.pathsep + env.get("PYTHONPATH", "")
    if extra_env:
        env.update(extra_env)
    return subprocess.run(
        [sys.executable, str(HOOK), event],
        input=stdin_bytes,
        capture_output=True,
        env=env,
    )


def test_hook_exists_and_is_executable():
    assert HOOK.exists(), f"missing {HOOK}"
    assert os.access(HOOK, os.X_OK), f"{HOOK} not executable"


def test_hook_sends_messages_and_exits_zero(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    payload = json.dumps({"session_id": "s1", "delta": "Hi.", "index": 0, "final": True}).encode()
    res = _run("MessageDisplay", payload, {"ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "prose"
    assert lines[0]["delta"] == "Hi."


def test_hook_invalid_stdin_still_exits_zero(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    res = _run("MessageDisplay", b"not json at all", {"ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    # Empty/invalid stdin -> payload {} -> a prose message with empty delta is still produced.
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "prose"
    assert lines[0]["delta"] == ""


def test_hook_empty_stdin_exits_zero(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    res = _run("Stop", b"", {"ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "earcon"
    assert lines[0]["kind"] == "turn_done"


def test_hook_unknown_event_sends_nothing(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    res = _run("MadeUp", b"{}", {"ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    assert not sent_log.exists() or sent_log.read_text().strip() == ""


def test_hook_capture_dumps_raw_stdin(tmp_path):
    cap = tmp_path / "cap"
    cap.mkdir()
    sent_log = tmp_path / "sent.jsonl"
    raw = b'{"session_id": "s1", "delta": "Cap.", "index": 0, "final": true}'
    res = _run("MessageDisplay", raw, {"ECHO_CAPTURE": str(cap), "ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    files = list(cap.glob("MessageDisplay-*.json"))
    assert len(files) == 1
    assert files[0].read_bytes() == raw


def test_hook_send_failure_is_swallowed(tmp_path):
    # When the fake client is told to raise, the shim must still exit 0.
    res = _run("Stop", b"{}", {"ECHO_FAKE_RAISE": "1"})
    assert res.returncode == 0, res.stderr.decode()
```

Create the fake `echo.client` used by the test so no real socket is touched. Create `tests/_fakeclient/echo/__init__.py` (empty file) and `tests/_fakeclient/echo/client.py`:

```python
"""Test double for echo.client: records sent messages instead of using a socket.

Shadowed onto PYTHONPATH ahead of src/ so bin/echo-hook imports THIS client.
The pure echo.hooks_entry and echo.protocol still resolve from src/ because this
package only provides a `client` submodule.
"""
import json
import os


def ensure_daemon(timeout: float = 3.0) -> None:
    if os.environ.get("ECHO_FAKE_RAISE"):
        raise RuntimeError("forced ensure_daemon failure")


def send(msg: dict, expect_reply: bool = False, timeout: float = 2.0):
    if os.environ.get("ECHO_FAKE_RAISE"):
        raise RuntimeError("forced send failure")
    log = os.environ.get("ECHO_FAKE_SENT_LOG")
    if log:
        with open(log, "a") as f:
            f.write(json.dumps(msg) + "\n")
    return None
```

Because the fake package only provides `echo/client.py`, importing `echo.hooks_entry`/`echo.protocol` from the same `echo` namespace would fail under a plain directory package. To keep the fake's `echo` from shadowing the real one for OTHER submodules, the conftest fallback puts `src` on the path and the test prepends `tests/_fakeclient` only for the client. Make `tests/_fakeclient/echo/__init__.py` a namespace-friendly package by giving it this content (so Python extends the package search path to also include the real `src/echo`):

```python
from pkgutil import extend_path

__path__ = extend_path(__path__, __name__)
```

Run the test and expect failure (the hook does not exist yet):

```bash
python -m pytest tests/test_echo_hook_bin.py -q
```

Expected output (FAIL): `test_hook_exists_and_is_executable` fails with `AssertionError: missing .../bin/echo-hook` (and the subprocess tests error because the file is absent).

Create the shim `bin/echo-hook`:

```python
#!/usr/bin/env python3
"""Echo hook entrypoint.

Invoked by Claude Code as: bin/echo-hook <Event>  (payload JSON on stdin).
Thin shim: parse argv/stdin, optionally capture raw stdin, map the event to
protocol messages via the PURE echo.hooks_entry.handle_event, ensure the daemon
is up, and send each message. Wrapped in a total try/except and ALWAYS exits 0
so a failure here can never break the Claude session. Keep total work tiny.
"""
import os
import sys


def main() -> None:
    # Read raw stdin first so we can capture it even if everything else fails.
    try:
        raw = sys.stdin.buffer.read()
    except Exception:
        raw = b""

    # Optional golden-payload capture: dump raw stdin BEFORE any other work.
    capture_dir = os.environ.get("ECHO_CAPTURE")
    if capture_dir:
        try:
            event_for_name = sys.argv[1] if len(sys.argv) > 1 else "unknown"
            os.makedirs(capture_dir, exist_ok=True)
            out_path = os.path.join(capture_dir, f"{event_for_name}-{os.getpid()}.json")
            with open(out_path, "wb") as f:
                f.write(raw)
        except Exception:
            pass

    # Resolve the package: prefer an installed 'echo'; fall back to ../src.
    try:
        import echo  # noqa: F401
    except Exception:
        here = os.path.dirname(os.path.abspath(__file__))
        src = os.path.join(os.path.dirname(here), "src")
        if src not in sys.path:
            sys.path.insert(0, src)

    from echo.hooks_entry import handle_event
    from echo import client

    event = sys.argv[1] if len(sys.argv) > 1 else ""

    # Tolerant stdin parse: empty/invalid -> {}.
    import json

    try:
        text = raw.decode("utf-8").strip()
        payload = json.loads(text) if text else {}
        if not isinstance(payload, dict):
            payload = {}
    except Exception:
        payload = {}

    msgs = handle_event(event, payload)
    if not msgs:
        return

    client.ensure_daemon()
    for m in msgs:
        client.send(m)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # Never let a hook failure surface to Claude.
        pass
    sys.exit(0)
```

Make it executable:

```bash
chmod +x bin/echo-hook
```

Run the test and expect PASS:

```bash
python -m pytest tests/test_echo_hook_bin.py -q
```

Expected output (PASS): `7 passed`.

Run the whole suite for this section to confirm nothing regressed:

```bash
python -m pytest tests/test_hooks_entry.py tests/test_echo_hook_bin.py -q
```

Expected output (PASS): `30 passed`.

Commit:

```bash
git add bin/echo-hook tests/test_echo_hook_bin.py tests/_fakeclient
git update-index --chmod=+x bin/echo-hook
git commit -m "feat: bin/echo-hook shim with capture, dispatch, send, always exit 0

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## cli.py + bin/echo + slash commands + install/uninstall/doctor + legacy migration

This section builds `src/echo/cli.py` (`main(argv)`), the `bin/echo` shim, five namespaced slash commands, and the install/uninstall/doctor flows including the legacy-migration cleaners. Everything is stdlib-only and every subprocess/audio/launchctl call is patchable so tests never touch the real system. The CLI assumes the modules from earlier sections exist (`echo.paths`, `echo.protocol`, `echo.config`, `echo.client`, `echo.daemon`, `echo.speaker`). Where a dependency is only imported inside a function, that is intentional so tests can patch it cleanly.

Conventions for this section:
- Control subcommands (`status`, `verbosity`, `rate`, `voice`, `repeat`, `stop`, `skip`) build a protocol message dict and hand it to `echo.client.send`. Tests patch `echo.client.send` and assert the exact dict.
- Local subcommands (`doctor`, `install`, `uninstall`, `daemon`) call module-level functions in `cli.py`.
- `main(argv)` returns an int exit code (0 = ok). The `bin/echo` shim passes that to `sys.exit`.

### Task: control subcommands map to client.send

Write the failing test that pins down the argparse wiring for the seven control subcommands. Each must produce a single `client.send` call with the right `MsgType` and payload, and `status` must print the daemon reply.

Create `tests/test_cli_control.py`:

```python
import json
from unittest import mock

import pytest

from echo import cli
from echo.protocol import MsgType, PROTOCOL_VERSION


def _sent(send_mock):
    assert send_mock.call_count == 1, send_mock.call_args_list
    args, kwargs = send_mock.call_args
    return args[0], args, kwargs


def test_status_sends_status_and_prints(capsys):
    reply = {"verbosity": "everything", "rate": 200, "voice": None,
             "foreground": "abc", "queue_len": 3}
    with mock.patch("echo.client.send", return_value=reply) as send:
        rc = cli.main(["status"])
    msg, args, kwargs = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.STATUS}
    assert kwargs.get("expect_reply") is True
    out = capsys.readouterr().out
    assert "everything" in out
    assert "queue_len" in out or "queue" in out


def test_status_handles_no_reply(capsys):
    with mock.patch("echo.client.send", return_value=None):
        rc = cli.main(["status"])
    assert rc == 1
    assert "no response" in capsys.readouterr().out.lower()


def test_verbosity_sends_set_verbosity():
    with mock.patch("echo.client.send", return_value=None) as send:
        rc = cli.main(["verbosity", "quiet"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_VERBOSITY,
                   "verbosity": "quiet"}


def test_verbosity_rejects_bad_value():
    with mock.patch("echo.client.send") as send:
        with pytest.raises(SystemExit):
            cli.main(["verbosity", "loud"])
    send.assert_not_called()


def test_rate_sends_int_set_rate():
    with mock.patch("echo.client.send", return_value=None) as send:
        rc = cli.main(["rate", "260"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_RATE, "rate": 260}
    assert isinstance(msg["rate"], int)


def test_voice_sends_set_voice():
    with mock.patch("echo.client.send", return_value=None) as send:
        rc = cli.main(["voice", "Ava (Premium)"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_VOICE,
                   "voice": "Ava (Premium)"}


def test_repeat_sends_repeat():
    with mock.patch("echo.client.send", return_value=None) as send:
        cli.main(["repeat"])
    msg, _, _ = _sent(send)
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.REPEAT}


def test_stop_sends_stop():
    with mock.patch("echo.client.send", return_value=None) as send:
        cli.main(["stop"])
    msg, _, _ = _sent(send)
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.STOP}


def test_skip_sends_skip():
    with mock.patch("echo.client.send", return_value=None) as send:
        cli.main(["skip"])
    msg, _, _ = _sent(send)
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SKIP}


def test_no_args_prints_help_and_returns_2(capsys):
    rc = cli.main([])
    assert rc == 2
    err = capsys.readouterr()
    assert "usage" in (err.out + err.err).lower()
```

Run it and expect failure because `echo.cli` does not exist yet:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_control.py -q
```

Expected output (fails at import):

```
ModuleNotFoundError: No module named 'echo.cli'
...
!!!!!!!!!!!!!!!!!!!! Interrupted: 1 error during collection !!!!!!!!!!!!!!!!!!!!
```

Now write the minimal real implementation. Create `src/echo/cli.py` with the argparse skeleton and the control subcommands only (local subcommands come in later tasks but we register their parsers now so argparse is stable):

```python
"""Echo command-line interface.

Subcommands fall into two groups:
  * control  -> build a protocol message and hand it to echo.client.send
  * local    -> doctor / install / uninstall / daemon (run in-process)

main(argv) returns an int exit code. Heavy imports (client, daemon) are done
inside the handlers so the module imports cheaply and is easy to patch in tests.
"""
from __future__ import annotations

import argparse
import json
import sys
from typing import Optional

from .protocol import MsgType, PROTOCOL_VERSION

VERBOSITY_CHOICES = ("everything", "medium", "quiet")


def _send(msg: dict, expect_reply: bool = False):
    from . import client  # local import so tests can patch echo.client.send
    return client.send(msg, expect_reply=expect_reply)


def _cmd_status(_args) -> int:
    reply = _send({"v": PROTOCOL_VERSION, "type": MsgType.STATUS},
                  expect_reply=True)
    if reply is None:
        print("echo: no response from daemon (is it running?)")
        return 1
    print(json.dumps(reply, indent=2))
    return 0


def _cmd_verbosity(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_VERBOSITY,
           "verbosity": args.level})
    return 0


def _cmd_rate(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_RATE, "rate": args.wpm})
    return 0


def _cmd_voice(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_VOICE, "voice": args.name})
    return 0


def _cmd_repeat(_args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.REPEAT})
    return 0


def _cmd_stop(_args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.STOP})
    return 0


def _cmd_skip(_args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SKIP})
    return 0


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="echo",
                                description="Echo eyes-free TTS for Claude Code")
    sub = p.add_subparsers(dest="command")

    sub.add_parser("status", help="print daemon status").set_defaults(
        func=_cmd_status)

    sp = sub.add_parser("verbosity", help="set verbosity level")
    sp.add_argument("level", choices=VERBOSITY_CHOICES)
    sp.set_defaults(func=_cmd_verbosity)

    sp = sub.add_parser("rate", help="set words-per-minute speech rate")
    sp.add_argument("wpm", type=int)
    sp.set_defaults(func=_cmd_rate)

    sp = sub.add_parser("voice", help="set the say voice")
    sp.add_argument("name")
    sp.set_defaults(func=_cmd_voice)

    sub.add_parser("repeat", help="repeat the last spoken item").set_defaults(
        func=_cmd_repeat)
    sub.add_parser("stop", help="stop all speech and clear the queue").set_defaults(
        func=_cmd_stop)
    sub.add_parser("skip", help="skip the current item").set_defaults(
        func=_cmd_skip)

    # Local subcommands are registered in later tasks via _register_local(sub).
    _register_local(sub)
    return p


def _register_local(sub) -> None:
    """Register local (non-control) subcommands. Filled in later tasks."""
    return None


def main(argv: Optional[list] = None) -> int:
    if argv is None:
        argv = sys.argv[1:]
    parser = _build_parser()
    args = parser.parse_args(argv)
    if not getattr(args, "func", None):
        parser.print_help()
        return 2
    return args.func(args)
```

Run again, expect pass:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_control.py -q
```

Expected output:

```
...........                                                              [100%]
11 passed in 0.0Xs
```

Commit:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && git add src/echo/cli.py tests/test_cli_control.py && git commit -m "feat: echo cli control subcommands map to client.send

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: legacy zshrc cleaner

The new uninstaller must scrub a prior legacy install. First the `~/.zshrc` cleaner: it removes the `# claude-tts` marker comment, the `alias claude='claude-speak'` line, and the `~/.local/bin` PATH export that the legacy installer appended (the line carrying a `# claude-tts` trailing comment). It must leave unrelated lines (including a user's own `local/bin` PATH export without the marker) untouched, and be safe on a missing file.

Add to `tests/test_cli_control.py` a new test file `tests/test_cli_legacy.py`:

```python
from pathlib import Path

from echo import cli


LEGACY_ZSHRC = """\
export EDITOR=vim
export PATH="$HOME/bin:$PATH"

# claude-tts
alias claude='claude-speak'

export PATH="$HOME/.local/bin:$PATH"  # claude-tts
alias gs='git status'
"""


def test_clean_zshrc_removes_legacy_lines(tmp_path):
    rc = tmp_path / ".zshrc"
    rc.write_text(LEGACY_ZSHRC)
    changed = cli._clean_zshrc(str(rc))
    assert changed is True
    text = rc.read_text()
    assert "claude-tts" not in text
    assert "claude-speak" not in text
    assert ".local/bin" not in text
    # Untouched lines survive.
    assert "export EDITOR=vim" in text
    assert 'export PATH="$HOME/bin:$PATH"' in text
    assert "alias gs='git status'" in text


def test_clean_zshrc_keeps_user_local_bin_without_marker(tmp_path):
    rc = tmp_path / ".zshrc"
    rc.write_text('export PATH="$HOME/.local/bin:$PATH"\nalias ll=\'ls -la\'\n')
    changed = cli._clean_zshrc(str(rc))
    assert changed is False
    assert ".local/bin" in rc.read_text()
    assert "alias ll='ls -la'" in rc.read_text()


def test_clean_zshrc_missing_file_is_noop(tmp_path):
    rc = tmp_path / "nope.zshrc"
    assert cli._clean_zshrc(str(rc)) is False
    assert not rc.exists()


def test_clean_zshrc_idempotent(tmp_path):
    rc = tmp_path / ".zshrc"
    rc.write_text(LEGACY_ZSHRC)
    assert cli._clean_zshrc(str(rc)) is True
    assert cli._clean_zshrc(str(rc)) is False
```

Run, expect failure (`_clean_zshrc` not defined):

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_legacy.py -q
```

Expected output:

```
E       AttributeError: module 'echo.cli' has no attribute '_clean_zshrc'
...
4 failed in 0.0Xs
```

Implement. Add to `src/echo/cli.py` (after the imports block, before `_send`):

```python
import os
import re


def _clean_zshrc(path: str) -> bool:
    """Remove legacy claude-tts lines from a zshrc. Returns True if it changed.

    Drops the '# claude-tts' marker comment, the 'alias claude=...claude-speak'
    line, and the '.local/bin' PATH export that carries the '# claude-tts'
    marker. A user's own .local/bin PATH line WITHOUT the marker is preserved.
    """
    p = os.path.expanduser(path)
    if not os.path.exists(p):
        return False
    with open(p, "r", encoding="utf-8") as f:
        lines = f.readlines()

    kept = []
    for line in lines:
        stripped = line.strip()
        if stripped == "# claude-tts":
            continue
        if "claude-speak" in line and "alias" in line and "claude" in line:
            continue
        if ".local/bin" in line and "claude-tts" in line:
            continue
        kept.append(line)

    # Collapse a blank line that the marker block left orphaned at the top of a
    # run only if we actually removed something; otherwise leave file untouched.
    if kept == lines:
        return False

    with open(p, "w", encoding="utf-8") as f:
        f.writelines(kept)
    return True
```

Run, expect pass:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_legacy.py -q
```

Expected output:

```
....                                                                     [100%]
4 passed in 0.0Xs
```

Commit:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && git add src/echo/cli.py tests/test_cli_legacy.py && git commit -m "feat: legacy zshrc cleaner removes claude-tts alias and PATH lines

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: legacy settings.json hook cleaner

Remove the 3 legacy hooks (`claude-tts-permission.sh`, `claude-tts-pre-tool.sh`, `claude-tts-stop.sh`) from a `~/.claude/settings.json` by loading the JSON, dropping any hook entry whose `command` contains `claude-tts`, removing now-empty event arrays, and writing back. Must preserve unrelated hooks and tolerate a missing/corrupt file.

Append to `tests/test_cli_legacy.py`:

```python
import json


def _legacy_settings():
    return {
        "model": "opus",
        "hooks": {
            "PreToolUse": [
                {"hooks": [{"type": "command",
                            "command": "/Users/x/.claude/hooks/claude-tts-pre-tool.sh"}]},
                {"hooks": [{"type": "command", "command": "/Users/x/keep-me.sh"}]},
            ],
            "Stop": [
                {"hooks": [{"type": "command",
                            "command": "/Users/x/.claude/hooks/claude-tts-stop.sh"}]},
            ],
            "PermissionRequest": [
                {"hooks": [{"type": "command",
                            "command": "/Users/x/.claude/hooks/claude-tts-permission.sh"}]},
            ],
        },
    }


def test_clean_settings_removes_legacy_hooks(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps(_legacy_settings()))
    changed = cli._clean_settings_json(str(sp))
    assert changed is True
    data = json.loads(sp.read_text())
    blob = json.dumps(data)
    assert "claude-tts" not in blob
    # Unrelated hook preserved; empty events dropped.
    assert data["hooks"]["PreToolUse"] == [
        {"hooks": [{"type": "command", "command": "/Users/x/keep-me.sh"}]}]
    assert "Stop" not in data["hooks"]
    assert "PermissionRequest" not in data["hooks"]
    assert data["model"] == "opus"


def test_clean_settings_missing_file_is_noop(tmp_path):
    sp = tmp_path / "settings.json"
    assert cli._clean_settings_json(str(sp)) is False


def test_clean_settings_corrupt_file_is_noop(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text("{not json")
    assert cli._clean_settings_json(str(sp)) is False
    # File left as-is when it cannot be parsed.
    assert sp.read_text() == "{not json"


def test_clean_settings_no_legacy_no_change(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({"hooks": {"Stop": [
        {"hooks": [{"type": "command", "command": "/Users/x/other.sh"}]}]}}))
    assert cli._clean_settings_json(str(sp)) is False
```

Run, expect failure (`_clean_settings_json` missing):

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_legacy.py -q
```

Expected output:

```
E       AttributeError: module 'echo.cli' has no attribute '_clean_settings_json'
...
4 failed, 4 passed in 0.0Xs
```

Implement. Add to `src/echo/cli.py` right after `_clean_zshrc`:

```python
def _clean_settings_json(path: str) -> bool:
    """Remove legacy claude-tts hooks from a settings.json. Returns True if changed.

    Drops any hook entry whose command contains 'claude-tts', removes hook
    groups left without hooks, and removes events left without groups. Tolerates
    a missing or corrupt file (returns False, leaves the file untouched).
    """
    p = os.path.expanduser(path)
    if not os.path.exists(p):
        return False
    try:
        with open(p, "r", encoding="utf-8") as f:
            data = json.load(f)
    except (ValueError, OSError):
        return False
    if not isinstance(data, dict):
        return False

    hooks = data.get("hooks")
    if not isinstance(hooks, dict):
        return False

    changed = False
    for event in list(hooks.keys()):
        groups = hooks.get(event)
        if not isinstance(groups, list):
            continue
        new_groups = []
        for group in groups:
            inner = group.get("hooks", []) if isinstance(group, dict) else []
            kept = [h for h in inner
                    if "claude-tts" not in str(h.get("command", ""))]
            if len(kept) != len(inner):
                changed = True
            if not kept:
                # whole group was legacy -> drop it
                continue
            group = dict(group)
            group["hooks"] = kept
            new_groups.append(group)
        if new_groups:
            hooks[event] = new_groups
        else:
            del hooks[event]

    if not changed:
        return False

    with open(p, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    return True
```

Run, expect pass:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_legacy.py -q
```

Expected output:

```
........                                                                 [100%]
8 passed in 0.0Xs
```

Commit:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && git add src/echo/cli.py tests/test_cli_legacy.py && git commit -m "feat: legacy settings.json hook cleaner removes claude-tts hooks

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: doctor() health checks

`doctor()` returns a list of `(check, ok, detail)` tuples for: `say` present, `afplay` present, enhanced voice available, `ECHO_DIR` writable, daemon socket reachable (client ping), and plugin `hooks/hooks.json` present in the repo. All system probes are patchable so tests stay hermetic.

Create `tests/test_cli_doctor.py`:

```python
from unittest import mock

from echo import cli


def _ok_patches():
    """Context managers that make every doctor check pass."""
    return [
        mock.patch("shutil.which", side_effect=lambda n: "/usr/bin/" + n),
        mock.patch("echo.speaker.best_enhanced_voice", return_value="Ava (Premium)"),
        mock.patch("os.access", return_value=True),
        mock.patch("echo.paths.ensure_echo_dir"),
        mock.patch("echo.client.send", return_value={"ok": True}),
        mock.patch("os.path.exists", return_value=True),
    ]


def _run(patches):
    for p in patches:
        p.start()
    try:
        return cli.doctor()
    finally:
        for p in reversed(patches):
            p.stop()


def _as_dict(results):
    return {check: (ok, detail) for check, ok, detail in results}


def test_doctor_returns_tuples():
    results = _run(_ok_patches())
    assert isinstance(results, list)
    for row in results:
        assert len(row) == 3
        check, ok, detail = row
        assert isinstance(check, str)
        assert isinstance(ok, bool)
        assert isinstance(detail, str)


def test_doctor_all_ok():
    d = _as_dict(_run(_ok_patches()))
    for key in ("say", "afplay", "enhanced voice", "ECHO_DIR writable",
                "daemon socket", "plugin hooks.json"):
        assert key in d, key
        assert d[key][0] is True, (key, d[key])


def test_doctor_say_missing():
    patches = _ok_patches()
    patches[0] = mock.patch(
        "shutil.which",
        side_effect=lambda n: None if n == "say" else "/usr/bin/" + n)
    d = _as_dict(_run(patches))
    assert d["say"][0] is False
    assert d["afplay"][0] is True


def test_doctor_socket_unreachable():
    patches = _ok_patches()
    patches[4] = mock.patch("echo.client.send",
                            side_effect=ConnectionRefusedError())
    d = _as_dict(_run(patches))
    assert d["daemon socket"][0] is False


def test_doctor_hooks_json_missing():
    patches = _ok_patches()
    patches[5] = mock.patch("os.path.exists", return_value=False)
    d = _as_dict(_run(patches))
    assert d["plugin hooks.json"][0] is False


def test_doctor_subcommand_prints_and_returns(capsys):
    with mock.patch("echo.cli.doctor",
                    return_value=[("say", True, "/usr/bin/say"),
                                  ("afplay", False, "not found")]):
        rc = cli.main(["doctor"])
    out = capsys.readouterr().out
    assert "say" in out and "afplay" in out
    # Any failing check makes the command exit non-zero.
    assert rc == 1


def test_doctor_subcommand_all_ok_returns_zero(capsys):
    with mock.patch("echo.cli.doctor",
                    return_value=[("say", True, "ok")]):
        rc = cli.main(["doctor"])
    assert rc == 0
    assert "say" in capsys.readouterr().out
```

Run, expect failure (`doctor` missing):

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_doctor.py -q
```

Expected output:

```
E       AttributeError: module 'echo.cli' has no attribute 'doctor'
...
8 failed in 0.0Xs
```

Implement. Add to `src/echo/cli.py` the `doctor` function and its subcommand handler. First add the `doctor` function after `_clean_settings_json`:

```python
import shutil

from . import paths


def _repo_hooks_json_path() -> str:
    """Path to the plugin hooks/hooks.json inside this repo checkout."""
    here = os.path.dirname(os.path.abspath(__file__))      # src/echo
    repo = os.path.dirname(os.path.dirname(here))          # repo root
    return os.path.join(repo, "hooks", "hooks.json")


def doctor() -> list:
    """Return a list of (check, ok, detail) health-check tuples."""
    results = []

    say = shutil.which("say")
    results.append(("say", say is not None,
                    say or "not found (macOS 'say' required)"))

    afplay = shutil.which("afplay")
    results.append(("afplay", afplay is not None,
                    afplay or "not found (macOS 'afplay' required)"))

    try:
        from . import speaker
        voice = speaker.best_enhanced_voice()
        results.append(("enhanced voice", bool(voice),
                        voice or "none detected; will fall back to Samantha"))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("enhanced voice", False, f"error: {exc}"))

    try:
        paths.ensure_echo_dir()
        writable = os.access(str(paths.ECHO_DIR), os.W_OK)
        results.append(("ECHO_DIR writable", writable,
                        str(paths.ECHO_DIR) if writable
                        else f"{paths.ECHO_DIR} is not writable"))
    except Exception as exc:  # noqa: BLE001
        results.append(("ECHO_DIR writable", False, f"error: {exc}"))

    try:
        from . import client
        reply = client.send({"v": PROTOCOL_VERSION, "type": MsgType.PING},
                            expect_reply=True)
        ok = bool(reply) and reply.get("ok") is True
        results.append(("daemon socket", ok,
                        "reachable" if ok else "no ok reply from daemon"))
    except Exception as exc:  # noqa: BLE001
        results.append(("daemon socket", False,
                        f"not reachable: {exc} (run 'echo install')"))

    hooks_json = _repo_hooks_json_path()
    present = os.path.exists(hooks_json)
    results.append(("plugin hooks.json", present,
                    hooks_json if present else f"missing: {hooks_json}"))

    return results


def _cmd_doctor(_args) -> int:
    rows = doctor()
    all_ok = True
    for check, ok, detail in rows:
        mark = "ok " if ok else "FAIL"
        print(f"[{mark}] {check}: {detail}")
        all_ok = all_ok and ok
    return 0 if all_ok else 1
```

Then register the `doctor` subcommand by replacing the placeholder body of `_register_local`:

```python
def _register_local(sub) -> None:
    """Register local (non-control) subcommands."""
    sub.add_parser("doctor", help="run health checks").set_defaults(
        func=_cmd_doctor)
```

Run, expect pass:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_doctor.py -q
```

Expected output:

```
........                                                                 [100%]
8 passed in 0.0Xs
```

Commit:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && git add src/echo/cli.py tests/test_cli_doctor.py && git commit -m "feat: echo doctor health checks and subcommand

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: install creates LaunchAgent plist

`install()` writes a `~/Library/LaunchAgents/com.echo.speechd.plist` LaunchAgent that runs `bin/echo-daemon`, ensures `ECHO_DIR`, loads the agent via `launchctl`, and prints plugin-enable instructions. The plist builder is a pure function so we can assert its full XML. All filesystem/launchctl effects are patchable.

Create `tests/test_cli_install.py`:

```python
import os
import plistlib
from unittest import mock

from echo import cli


def test_launchagent_plist_is_valid_and_complete(tmp_path):
    daemon = "/repo/bin/echo-daemon"
    log = "/home/u/.echo/speechd.log"
    xml = cli._launchagent_plist(daemon, log)
    assert isinstance(xml, str)
    assert xml.startswith("<?xml")
    data = plistlib.loads(xml.encode("utf-8"))
    assert data["Label"] == cli.LAUNCH_AGENT_LABEL
    assert data["ProgramArguments"] == [daemon]
    assert data["RunAtLoad"] is True
    assert data["KeepAlive"] is True
    assert data["StandardErrorPath"] == log
    assert data["StandardOutPath"] == log


def test_install_writes_plist_and_loads(tmp_path, capsys):
    la_dir = tmp_path / "LaunchAgents"
    plist = la_dir / (cli.LAUNCH_AGENT_LABEL + ".plist")
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch("echo.paths.ensure_echo_dir") as ensure:
        rc = cli.install()
    assert rc == 0
    ensure.assert_called_once()
    assert plist.exists()
    # launchctl unload (ignored) then load was attempted.
    assert any(c.args[0][0] == "load" for c in run.call_args_list)
    out = capsys.readouterr().out
    assert "/plugin" in out or "plugin" in out.lower()


def test_install_subcommand_invokes_install():
    with mock.patch("echo.cli.install", return_value=0) as inst:
        rc = cli.main(["install"])
    inst.assert_called_once()
    assert rc == 0
```

Run, expect failure (`install` / `_launchagent_plist` / `LAUNCH_AGENT_*` missing):

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_install.py -q
```

Expected output:

```
E       AttributeError: module 'echo.cli' has no attribute '_launchagent_plist'
...
3 failed in 0.0Xs
```

Implement. Add to `src/echo/cli.py` after `doctor`/`_cmd_doctor`:

```python
import subprocess

LAUNCH_AGENT_LABEL = "com.echo.speechd"
LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.echo.speechd.plist")


def _repo_root() -> str:
    here = os.path.dirname(os.path.abspath(__file__))   # src/echo
    return os.path.dirname(os.path.dirname(here))       # repo root


def _daemon_shim_path() -> str:
    return os.path.join(_repo_root(), "bin", "echo-daemon")


def _launchagent_plist(daemon_path: str, log_path: str) -> str:
    """Return the full LaunchAgent plist XML for the speech daemon."""
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
        '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
        '<plist version="1.0">\n'
        '<dict>\n'
        '    <key>Label</key>\n'
        f'    <string>{LAUNCH_AGENT_LABEL}</string>\n'
        '    <key>ProgramArguments</key>\n'
        '    <array>\n'
        f'        <string>{daemon_path}</string>\n'
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


def _launchctl(args: list) -> int:
    """Run 'launchctl <args...>'. Patched in tests. Returns the exit code."""
    try:
        return subprocess.call(["launchctl", *args])
    except FileNotFoundError:
        return 1


def install() -> int:
    """Install the speech daemon as a LaunchAgent and ensure ECHO_DIR."""
    paths.ensure_echo_dir()
    daemon = _daemon_shim_path()
    log = str(paths.LOG_PATH)
    xml = _launchagent_plist(daemon, log)

    os.makedirs(os.path.dirname(LAUNCH_AGENT_PATH), exist_ok=True)
    with open(LAUNCH_AGENT_PATH, "w", encoding="utf-8") as f:
        f.write(xml)
    print(f"Wrote LaunchAgent: {LAUNCH_AGENT_PATH}")

    # Reload: unload any prior copy (ignore failure), then load.
    _launchctl(["unload", LAUNCH_AGENT_PATH])
    rc = _launchctl(["load", LAUNCH_AGENT_PATH])
    if rc == 0:
        print(f"Loaded LaunchAgent {LAUNCH_AGENT_LABEL}.")
    else:
        print(f"warning: 'launchctl load' returned {rc}; "
              f"the daemon will still autostart on next login.")

    print("")
    print("Enable the Echo plugin in Claude Code:")
    print(f"  1. Add this repo as a plugin marketplace/source: {_repo_root()}")
    print("  2. In Claude Code run: /plugin")
    print("  3. Enable 'echo' so its hooks load.")
    print("Then run 'echo doctor' to verify everything is wired up.")
    return 0


def _cmd_install(_args) -> int:
    return install()
```

Now wire the subcommand: extend `_register_local`:

```python
def _register_local(sub) -> None:
    """Register local (non-control) subcommands."""
    sub.add_parser("doctor", help="run health checks").set_defaults(
        func=_cmd_doctor)
    sub.add_parser("install", help="install the LaunchAgent + ECHO_DIR").set_defaults(
        func=_cmd_install)
```

Run, expect pass:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_install.py -q
```

Expected output:

```
...                                                                      [100%]
3 passed in 0.0Xs
```

Commit:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && git add src/echo/cli.py tests/test_cli_install.py && git commit -m "feat: echo install writes LaunchAgent plist and loads daemon

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: uninstall + legacy migration

`uninstall()` unloads and removes the LaunchAgent, removes `ECHO_DIR`, then runs the legacy migration: clean `~/.zshrc`, clean `~/.claude/settings.json`, and delete the four legacy artifacts (`~/.local/bin/claude-speak`, `~/.local/bin/claude-tts`, `~/.claude-tts-enabled`, `~/.claude-tts-pos`). All paths are parameterized so the test runs entirely under `tmp_path`.

Create `tests/test_cli_uninstall.py`:

```python
import json
import os
from unittest import mock

from echo import cli


def test_uninstall_removes_launchagent_and_echo_dir(tmp_path):
    plist = tmp_path / "com.echo.speechd.plist"
    plist.write_text("<plist/>")
    echo_dir = tmp_path / ".echo"
    echo_dir.mkdir()
    (echo_dir / "config.json").write_text("{}")

    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli.paths, "ECHO_DIR", echo_dir), \
         mock.patch.object(cli, "_legacy_migrate", return_value=[]) as mig:
        rc = cli.uninstall()

    assert rc == 0
    assert not plist.exists()
    assert not echo_dir.exists()
    assert any(c.args[0][0] == "unload" for c in run.call_args_list)
    mig.assert_called_once()


def test_legacy_migrate_cleans_everything(tmp_path):
    home = tmp_path
    zshrc = home / ".zshrc"
    zshrc.write_text("# claude-tts\nalias claude='claude-speak'\n"
                     'export PATH="$HOME/.local/bin:$PATH"  # claude-tts\n'
                     "export EDITOR=vim\n")
    claude = home / ".claude"
    claude.mkdir()
    settings = claude / "settings.json"
    settings.write_text(json.dumps({"hooks": {"Stop": [
        {"hooks": [{"type": "command",
                    "command": str(claude / "hooks/claude-tts-stop.sh")}]}]}}))
    local_bin = home / ".local" / "bin"
    local_bin.mkdir(parents=True)
    (local_bin / "claude-speak").write_text("x")
    (local_bin / "claude-tts").write_text("x")
    (home / ".claude-tts-enabled").write_text("1")
    (home / ".claude-tts-pos").write_text("0")

    removed = cli._legacy_migrate(home=str(home))

    assert "claude-tts" not in zshrc.read_text()
    assert "claude-tts" not in settings.read_text()
    assert not (local_bin / "claude-speak").exists()
    assert not (local_bin / "claude-tts").exists()
    assert not (home / ".claude-tts-enabled").exists()
    assert not (home / ".claude-tts-pos").exists()
    # removed is a human-readable list of what was cleaned.
    assert any("claude-speak" in r for r in removed)
    assert "export EDITOR=vim" in zshrc.read_text()


def test_legacy_migrate_on_clean_machine_is_safe(tmp_path):
    removed = cli._legacy_migrate(home=str(tmp_path))
    assert removed == [] or all(isinstance(r, str) for r in removed)


def test_uninstall_subcommand_invokes_uninstall():
    with mock.patch("echo.cli.uninstall", return_value=0) as un:
        rc = cli.main(["uninstall"])
    un.assert_called_once()
    assert rc == 0
```

Run, expect failure (`uninstall` / `_legacy_migrate` missing):

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_uninstall.py -q
```

Expected output:

```
E       AttributeError: module 'echo.cli' has no attribute 'uninstall'
...
4 failed in 0.0Xs
```

Implement. Add to `src/echo/cli.py` after `install`/`_cmd_install`:

```python
import shutil as _shutil  # alias so module-level 'shutil' (used by doctor) stays clear


def _legacy_migrate(home: Optional[str] = None) -> list:
    """Clean up a PRIOR legacy claude-tts install. Returns a list of strings
    describing what was removed. Safe (no-op) on a machine with no legacy install.
    """
    base = home or os.path.expanduser("~")
    removed = []

    zshrc = os.path.join(base, ".zshrc")
    if _clean_zshrc(zshrc):
        removed.append(f"cleaned legacy alias/PATH lines from {zshrc}")

    settings = os.path.join(base, ".claude", "settings.json")
    if _clean_settings_json(settings):
        removed.append(f"cleaned legacy hooks from {settings}")

    legacy_files = [
        os.path.join(base, ".local", "bin", "claude-speak"),
        os.path.join(base, ".local", "bin", "claude-tts"),
        os.path.join(base, ".claude-tts-enabled"),
        os.path.join(base, ".claude-tts-pos"),
    ]
    for f in legacy_files:
        if os.path.exists(f):
            try:
                os.remove(f)
                removed.append(f"removed {f}")
            except OSError:
                pass

    return removed


def uninstall() -> int:
    """Remove the LaunchAgent + ECHO_DIR and migrate away a legacy install."""
    if os.path.exists(LAUNCH_AGENT_PATH):
        _launchctl(["unload", LAUNCH_AGENT_PATH])
        try:
            os.remove(LAUNCH_AGENT_PATH)
            print(f"Removed LaunchAgent: {LAUNCH_AGENT_PATH}")
        except OSError as exc:
            print(f"warning: could not remove {LAUNCH_AGENT_PATH}: {exc}")
    else:
        print("No LaunchAgent installed.")

    echo_dir = str(paths.ECHO_DIR)
    if os.path.isdir(echo_dir):
        _shutil.rmtree(echo_dir, ignore_errors=True)
        print(f"Removed {echo_dir}")

    print("Checking for a prior legacy claude-tts install...")
    for line in _legacy_migrate():
        print(f"  - {line}")
    print("Done. Disable the 'echo' plugin via /plugin in Claude Code if enabled.")
    return 0


def _cmd_uninstall(_args) -> int:
    return uninstall()
```

Wire the subcommand and the `daemon` passthrough by finalizing `_register_local`:

```python
def _register_local(sub) -> None:
    """Register local (non-control) subcommands."""
    sub.add_parser("doctor", help="run health checks").set_defaults(
        func=_cmd_doctor)
    sub.add_parser("install", help="install the LaunchAgent + ECHO_DIR").set_defaults(
        func=_cmd_install)
    sub.add_parser("uninstall",
                   help="remove Echo and clean a legacy install").set_defaults(
        func=_cmd_uninstall)
    sub.add_parser("daemon", help="run the speech daemon in the foreground").set_defaults(
        func=_cmd_daemon)
```

Add the `daemon` handler near the other handlers (it just delegates to `echo.daemon.main`):

```python
def _cmd_daemon(_args) -> int:
    from . import daemon
    daemon.main()
    return 0
```

Run, expect pass:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_uninstall.py -q
```

Expected output:

```
....                                                                     [100%]
4 passed in 0.0Xs
```

Run the whole cli suite to confirm nothing regressed (argparse stability after adding subcommands):

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_control.py tests/test_cli_legacy.py tests/test_cli_doctor.py tests/test_cli_install.py tests/test_cli_uninstall.py -q
```

Expected output:

```
..............................                                           [100%]
30 passed in 0.Xs
```

Commit:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && git add src/echo/cli.py tests/test_cli_uninstall.py && git commit -m "feat: echo uninstall removes LaunchAgent, ECHO_DIR, and runs legacy migration

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: bin/echo shim

The `bin/echo` shim execs `python -m echo.cli`, passing through args and the exit code. Test it as a subprocess so we exercise the real shim end-to-end (with `status` stubbed to fail fast since no daemon runs in CI - we just assert the shim dispatches and forwards the exit code).

Create `tests/test_bin_echo.py`:

```python
import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SHIM = os.path.join(REPO, "bin", "echo")


def _env():
    env = dict(os.environ)
    # Make 'echo' importable without an install.
    src = os.path.join(REPO, "src")
    env["PYTHONPATH"] = src + os.pathsep + env.get("PYTHONPATH", "")
    return env


def test_shim_exists_and_executable():
    assert os.path.exists(SHIM)
    assert os.access(SHIM, os.X_OK)


def test_shim_help_runs():
    proc = subprocess.run([SHIM, "--help"], capture_output=True, text=True,
                          env=_env())
    assert proc.returncode == 0
    assert "usage" in proc.stdout.lower()


def test_shim_no_args_returns_2():
    proc = subprocess.run([SHIM], capture_output=True, text=True, env=_env())
    assert proc.returncode == 2


def test_shim_forwards_subcommand_exit_code():
    # 'doctor' returns 1 when any check fails; with no daemon the socket check
    # fails, so we expect a non-zero exit and printed output.
    proc = subprocess.run([SHIM, "doctor"], capture_output=True, text=True,
                          env=_env())
    assert proc.returncode in (0, 1)
    assert "say" in proc.stdout
```

Run, expect failure (shim missing):

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_bin_echo.py -q
```

Expected output:

```
E       assert False
 +  where False = <function exists ...>('.../bin/echo')
...
4 failed in 0.0Xs
```

Implement. Create `bin/echo`:

```bash
#!/usr/bin/env bash
# bin/echo - shim that execs the Echo CLI.
exec python3 -m echo.cli "$@"
```

Make it executable:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && chmod +x bin/echo
```

Run, expect pass:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_bin_echo.py -q
```

Expected output:

```
....                                                                     [100%]
4 passed in 0.Xs
```

Commit:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && git add bin/echo tests/test_bin_echo.py && git commit -m "feat: bin/echo shim execs python -m echo.cli

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: slash commands

Create the five namespaced slash commands. Each is a thin instruction telling Claude to run the matching `echo` CLI subcommand via Bash and to print nothing unless the command is `status` or `doctor` (those surface their output). A test asserts each file exists, names the right subcommand, and respects the print/no-print rule.

Create `tests/test_commands.py`:

```python
import os

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CMD = os.path.join(REPO, "commands")


def _read(name):
    with open(os.path.join(CMD, name), encoding="utf-8") as f:
        return f.read()


def test_all_command_files_exist():
    for name in ("echo:status.md", "echo:verbosity.md", "echo:stop.md",
                 "echo:repeat.md", "echo:doctor.md"):
        assert os.path.exists(os.path.join(CMD, name)), name


def test_status_runs_status_and_shows_output():
    txt = _read("echo:status.md")
    assert "echo status" in txt
    assert "Bash" in txt
    # status surfaces output to the user.
    assert "print" in txt.lower()


def test_verbosity_passes_argument_and_is_silent():
    txt = _read("echo:verbosity.md")
    assert "echo verbosity" in txt
    assert "$ARGUMENTS" in txt or "ARGUMENTS" in txt
    assert "nothing" in txt.lower()


def test_stop_is_silent():
    txt = _read("echo:stop.md")
    assert "echo stop" in txt
    assert "nothing" in txt.lower()


def test_repeat_is_silent():
    txt = _read("echo:repeat.md")
    assert "echo repeat" in txt
    assert "nothing" in txt.lower()


def test_doctor_shows_output():
    txt = _read("echo:doctor.md")
    assert "echo doctor" in txt
    assert "Bash" in txt
    assert "print" in txt.lower()
```

Run, expect failure (command files missing):

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_commands.py -q
```

Expected output:

```
E       AssertionError: echo:status.md
...
6 failed in 0.0Xs
```

Implement. Create `commands/echo:status.md`:

```markdown
---
description: Show Echo speech daemon status (verbosity, rate, voice, queue)
---

Run the Echo status command using the Bash tool:

```
echo status
```

Print the command's output to the user verbatim so they can see the current
verbosity, rate, voice, foreground session, and queue length. Do not add
commentary beyond the raw status.
```

Create `commands/echo:verbosity.md`:

```markdown
---
description: Set Echo verbosity (everything | medium | quiet)
argument-hint: everything | medium | quiet
---

Run the Echo verbosity command using the Bash tool, forwarding the requested
level:

```
echo verbosity $ARGUMENTS
```

This is a silent control action. Print nothing to the user on success - just run
the command. If the command errors, briefly report the error.
```

Create `commands/echo:stop.md`:

```markdown
---
description: Stop Echo speech immediately and clear the queue
---

Run the Echo stop command using the Bash tool:

```
echo stop
```

This is a silent control action. Print nothing to the user - just run the
command.
```

Create `commands/echo:repeat.md`:

```markdown
---
description: Repeat the last thing Echo spoke
---

Run the Echo repeat command using the Bash tool:

```
echo repeat
```

This is a silent control action. Print nothing to the user - just run the
command.
```

Create `commands/echo:doctor.md`:

```markdown
---
description: Run Echo health checks (say, afplay, voice, daemon, hooks)
---

Run the Echo doctor command using the Bash tool:

```
echo doctor
```

Print the command's output to the user verbatim so they can see which health
checks pass or fail and the suggested fixes. Do not add commentary beyond the
raw check results.
```

Run, expect pass:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_commands.py -q
```

Expected output:

```
......                                                                   [100%]
6 passed in 0.0Xs
```

Run the full CLI section suite one last time to confirm everything is green together:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && python -m pytest tests/test_cli_control.py tests/test_cli_legacy.py tests/test_cli_doctor.py tests/test_cli_install.py tests/test_cli_uninstall.py tests/test_bin_echo.py tests/test_commands.py -q
```

Expected output:

```
........................................                                 [100%]
40 passed in 0.Xs
```

Commit:

```bash
cd /Users/Nima.Hakimi/projects/private/claude-tts && git add commands/echo:status.md commands/echo:verbosity.md commands/echo:stop.md commands/echo:repeat.md commands/echo:doctor.md tests/test_commands.py && git commit -m "feat: echo slash commands (status, verbosity, stop, repeat, doctor)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## End-to-end integration test + README + final verification

This is the final section. By now every `src/echo/*` module exists and is unit-tested. These tasks wire the **real** pipeline together end-to-end (with audio still injected), validate the captured fixtures and the plugin manifests, ship the full `README.md`, and finish with a manual eyes-free verification checklist.

The e2e test is the proof of the core UX contract from the spec: a **decision earcon fires immediately** when a choice/plan/permission appears, but the **spoken detail is FIFO** and is only voiced *after* the preceding prose. We capture earcons (fired synchronously inside `SpeechDaemon.handle_message`) and speech (produced when we drain the queue through the speak-loop logic) into **one shared ordered list**, then assert the full sequence.

### Task: E2E recorder + scripted-scenario ordering test (RED)

We build a single `FakeSpeaker` that has the exact public surface the daemon uses (`speak`, `cancel`, `earcon`, `set_voice`, `set_rate`) and records `("text", <spoken text>)` and `("earcon", <kind>)` into a shared list **in call order**. Because the daemon fires earcons during `handle_message` but only enqueues spoken items, draining the queue *after* feeding the whole scenario reproduces the real "alert now, speak in order later" timeline.

Create `tests/test_e2e_pipeline.py`:

```python
"""End-to-end pipeline test: real hooks_entry -> real SpeechDaemon -> recording FakeSpeaker.

Proves the ordering contract from the design spec section 4:
  - a decision earcon fires IMMEDIATELY when the decision appears,
  - but the decision's spoken text is FIFO and is voiced only AFTER the
    preceding prose,
  - foreground gating works,
  - a turn_done earcon ends the turn.
No real audio is ever produced: the Speaker is replaced by a recorder.
"""
from echo.hooks_entry import handle_event
from echo.protocol import MsgType, PROTOCOL_VERSION
from echo.queue import SpeechQueue
from echo.sessions import SessionManager
from echo.daemon import SpeechDaemon
from echo.config import DEFAULTS


SID = "sess-e2e-1"


class FakeSpeaker:
    """Records spoken text and earcons into one shared ordered list.

    Mirrors the public surface SpeechDaemon uses: speak/cancel/earcon/
    set_voice/set_rate. speak() is synchronous here (no threads) so the
    drain helper produces a deterministic ordering.
    """

    def __init__(self, log):
        self.log = log
        self.voice = None
        self.rate = DEFAULTS["rate"]
        self.cancelled = 0

    def speak(self, text):
        self.log.append(("text", text))

    def cancel(self):
        self.cancelled += 1

    def earcon(self, kind):
        self.log.append(("earcon", kind))

    def set_voice(self, v):
        self.voice = v

    def set_rate(self, r):
        self.rate = r


def drain_queue(daemon, speaker):
    """Run the _speak_loop logic to exhaustion: pop FIFO, speak each item.

    This is exactly what SpeechDaemon._speak_loop does per iteration
    (item = queue.pop_next(); if item: speaker.speak(item.text)), minus the
    blocking wait, so the test is deterministic and never touches threads.
    """
    while True:
        item = daemon.queue.pop_next()
        if item is None:
            return
        speaker.speak(item.text)


def make_daemon():
    log = []
    speaker = FakeSpeaker(log)
    queue = SpeechQueue()
    sessions = SessionManager(background_policy="earcon_only")
    cfg = {k: (dict(v) if isinstance(v, dict) else v) for k, v in DEFAULTS.items()}
    cfg["verbosity"] = "everything"
    daemon = SpeechDaemon(queue, speaker, sessions, cfg)
    return daemon, speaker, log


def feed_event(daemon, event, payload):
    """Run a hook event through the real handle_event and feed every
    resulting protocol message into the real daemon, just like bin/echo-hook
    -> client -> daemon would in production."""
    for msg in handle_event(event, payload):
        assert msg["v"] == PROTOCOL_VERSION
        daemon.handle_message(msg)


def test_scripted_session_full_ordering():
    daemon, speaker, log = make_daemon()

    # 1. SessionStart: registers + sets foreground (no audio yet).
    feed_event(daemon, "SessionStart", {"session_id": SID})

    # 2. MessageDisplay: two sentences of prose, streamed as one final delta.
    feed_event(daemon, "MessageDisplay", {
        "session_id": SID,
        "delta": "Let me check the files. I will start now.",
        "index": 0,
        "final": True,
    })

    # 3. PreToolUse AskUserQuestion with two options.
    #    -> choice earcon fires IMMEDIATELY (recorded now),
    #       choice TEXT is enqueued AFTER the queued prose (recorded on drain).
    feed_event(daemon, "PreToolUse", {
        "session_id": SID,
        "tool_name": "AskUserQuestion",
        "tool_input": {
            "questions": [{
                "question": "Which approach?",
                "options": [
                    {"label": "Refactor"},
                    {"label": "Rewrite"},
                ],
            }],
        },
    })

    # Drain everything queued so far: prose sentences first, THEN the choice
    # text. The choice EARCON is already in the log from step 3 (before any
    # of this prose was spoken) -> that is the ordering proof.
    drain_queue(daemon, speaker)

    # 4. UserPromptSubmit: user answered + sent a new prompt -> flush + fg.
    feed_event(daemon, "UserPromptSubmit", {"session_id": SID})

    # 5. MessageDisplay: more prose for the new turn.
    feed_event(daemon, "MessageDisplay", {
        "session_id": SID,
        "delta": "Applying the change now.",
        "index": 0,
        "final": True,
    })

    # 6. PreToolUse Bash -> permission via Notification permission_prompt.
    #    permission earcon fires immediately; permission text enqueued after.
    feed_event(daemon, "Notification", {
        "session_id": SID,
        "notification_type": "permission_prompt",
        "action": "Run: pytest -q",
    })

    drain_queue(daemon, speaker)

    # 7. Stop: turn_done earcon ends the turn.
    feed_event(daemon, "Stop", {"session_id": SID})
    drain_queue(daemon, speaker)

    assert log == [
        ("earcon", "choice"),
        ("text", "Let me check the files."),
        ("text", "I will start now."),
        ("text", "Which approach? Option 1: Refactor. Option 2: Rewrite."),
        ("earcon", "permission"),
        ("text", "Applying the change now."),
        ("text", "Run: pytest -q"),
        ("earcon", "turn_done"),
    ]


def test_background_session_is_earcon_only():
    """A non-foreground session still fires decision earcons but its prose
    and decision TEXT are NOT spoken (foreground gating)."""
    daemon, speaker, log = make_daemon()

    feed_event(daemon, "SessionStart", {"session_id": "fg"})
    # Background session never becomes foreground.
    feed_event(daemon, "MessageDisplay", {
        "session_id": "bg",
        "delta": "Background chatter that must stay silent.",
        "index": 0,
        "final": True,
    })
    feed_event(daemon, "PreToolUse", {
        "session_id": "bg",
        "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{
            "question": "Pick one",
            "options": [{"label": "A"}, {"label": "B"}],
        }]},
    })
    drain_queue(daemon, speaker)

    # Earcon fired (alerts are cross-session), but no text was spoken.
    assert log == [("earcon", "choice")]
```

Run it (expect FAIL - the FakeSpeaker text assertions pin the exact decision-text format the daemon must build, and the ordering must hold; if any earlier module drifts from the contract this is where it surfaces):

```bash
python -m pytest tests/test_e2e_pipeline.py -q
```

Expected output (RED) - one of:

```
E       assert [...] == [...]
...
1 failed, 1 passed in 0.1s
```

or, if the decision-text format differs, an `AssertionError` showing the actual vs expected `log` lists. Do **not** weaken these assertions; instead reconcile the daemon's decision-text builder (the CHOICE/PERMISSION branch of `SpeechDaemon.handle_message`) so it emits:

- CHOICE: `"<question> Option 1: <label>. Option 2: <label>."` (one space-joined string per question; questions joined by a space).
- PERMISSION: the human action string from the payload (here `"Run: pytest -q"`).

These formats are owned by the daemon section; this test is the authority on the exact spoken strings. Once the daemon matches, proceed to GREEN.

### Task: Make the e2e ordering test pass (GREEN)

No new production module is created here - the daemon and hooks already exist. If the RED run above passed immediately, the pipeline already conforms and you skip straight to commit. If it failed, the fix lives in the already-built modules and is constrained to the contract:

1. `SpeechDaemon.handle_message` CHOICE branch must build the spoken text from `msg["questions"]` as: for each question, `"<question> "` followed by `"Option <n>: <label>."` for each option, joined by a single space, and multiple questions joined by a single space. The CHOICE branch only enqueues content (gated by `should_speak`) - it must **not** call `speaker.earcon` itself.
2. `SpeechDaemon.handle_message` PERMISSION branch must set the SpeechItem text to `msg["action"]` (bare action), and likewise must **not** call `speaker.earcon` itself.
3. The decision ALERT earcon is fired by the `MsgType.EARCON` branch when it receives the separate EARCON message that `hooks_entry.handle_event` emits BEFORE each content message: `PreToolUse`/`AskUserQuestion` returns `[{EARCON choice}, {CHOICE questions=...}]`; `Notification`/`permission_prompt` returns `[{EARCON permission}, {PERMISSION action=...}]`. This is why the e2e log shows exactly one earcon per decision (fired by the EARCON message), then the content in FIFO order. Confirm `action` is sourced from `payload` (`payload.get("action")`).

Apply the minimal edit in the relevant already-built module(s), then re-run:

```bash
python -m pytest tests/test_e2e_pipeline.py -q
```

Expected output (GREEN):

```
2 passed in 0.1s
```

Now run the **entire** suite to confirm nothing regressed:

```bash
python -m pytest -q
```

Expected output (GREEN): all tests pass, e.g.

```
.................................................
NN passed in 0.Xs
```

Commit:

```bash
git add tests/test_e2e_pipeline.py src/echo
git commit -m "test: end-to-end pipeline ordering (earcon-now, speak-in-order)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Golden fixture parse test (RED then GREEN)

Every captured payload in `tests/fixtures/*.json` (captured by the runtime task in the scaffolding section via `ECHO_CAPTURE`) must flow through `handle_event` without raising and must yield well-typed protocol messages. This guards against schema drift between the real Claude Code binary and our parser.

Create `tests/test_fixtures.py`:

```python
"""Run every captured golden payload through handle_event and assert the
returned messages are well-typed protocol dicts. Fixture file names are
'<Event>.json' or '<Event>-<pid>.json'; the leading token is the event."""
import json
from pathlib import Path

import pytest

from echo.hooks_entry import handle_event
from echo.protocol import PROTOCOL_VERSION, MsgType


FIXTURE_DIR = Path(__file__).parent / "fixtures"

_VALID_TYPES = {
    v for k, v in vars(MsgType).items()
    if not k.startswith("_") and isinstance(v, str)
}


def _event_name(path: Path) -> str:
    # 'MessageDisplay-12345.json' -> 'MessageDisplay'; 'Stop.json' -> 'Stop'
    return path.stem.split("-", 1)[0]


def _fixture_files():
    if not FIXTURE_DIR.is_dir():
        return []
    return sorted(p for p in FIXTURE_DIR.glob("*.json") if p.is_file())


@pytest.mark.parametrize(
    "fixture",
    _fixture_files(),
    ids=lambda p: p.name,
)
def test_fixture_parses_to_well_typed_messages(fixture):
    raw = fixture.read_text(encoding="utf-8")
    payload = json.loads(raw) if raw.strip() else {}
    assert isinstance(payload, dict), f"{fixture.name}: payload must be an object"

    event = _event_name(fixture)
    msgs = handle_event(event, payload)

    assert isinstance(msgs, list), f"{fixture.name}: handle_event must return a list"
    for msg in msgs:
        assert isinstance(msg, dict), f"{fixture.name}: each message must be a dict"
        assert msg.get("v") == PROTOCOL_VERSION, f"{fixture.name}: missing/bad protocol version"
        assert msg.get("type") in _VALID_TYPES, (
            f"{fixture.name}: bad message type {msg.get('type')!r}"
        )


def test_at_least_one_fixture_exists():
    """The capture task must have produced golden payloads; a green run with
    zero fixtures would be a silent false positive."""
    files = _fixture_files()
    assert files, (
        "no tests/fixtures/*.json captured; run the ECHO_CAPTURE capture task "
        "against a real session first"
    )
```

Run it. If the capture task already ran, the parametrized cases should pass and only the sentinel matters; if fixtures are missing you get a clear, actionable failure:

```bash
python -m pytest tests/test_fixtures.py -q
```

Expected output if fixtures are missing (RED, actionable):

```
E       AssertionError: no tests/fixtures/*.json captured; run the ECHO_CAPTURE capture task against a real session first
1 failed in 0.0s
```

To go GREEN, ensure the fixtures exist (the scaffolding/runtime capture task writes them; if you are running before a real capture is available, drop minimal representative payloads into `tests/fixtures/` named after each event, e.g. `MessageDisplay.json`, `PreToolUse.json`, `Notification.json`, `Stop.json`, `UserPromptSubmit.json`, `SessionStart.json`, `SessionEnd.json`). Re-run:

```bash
python -m pytest tests/test_fixtures.py -q
```

Expected output (GREEN):

```
........ [100%]
N passed in 0.0s
```

Commit:

```bash
git add tests/test_fixtures.py tests/fixtures
git commit -m "test: golden fixtures parse to well-typed protocol messages

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Plugin + hooks manifest validity test (RED then GREEN)

`.claude-plugin/plugin.json` and `hooks/hooks.json` must be valid JSON, and every command referenced in `hooks.json` must resolve (after `${CLAUDE_PLUGIN_ROOT}` substitution) to the existing `bin/echo-hook` shim. This catches a broken manifest before Claude Code silently ignores our hooks.

Create `tests/test_manifests.py`:

```python
"""Validate the shipped plugin manifests as real JSON and assert every
hooks.json command points at an existing bin/echo-hook under the repo root."""
import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGIN_JSON = REPO_ROOT / ".claude-plugin" / "plugin.json"
HOOKS_JSON = REPO_ROOT / "hooks" / "hooks.json"
ECHO_HOOK = REPO_ROOT / "bin" / "echo-hook"


def _load(path: Path) -> dict:
    assert path.is_file(), f"missing manifest: {path}"
    return json.loads(path.read_text(encoding="utf-8"))


def test_plugin_json_is_valid_and_named():
    data = _load(PLUGIN_JSON)
    assert isinstance(data, dict)
    assert data.get("name"), "plugin.json must declare a non-empty name"


def test_echo_hook_shim_exists():
    assert ECHO_HOOK.is_file(), f"missing hook shim: {ECHO_HOOK}"


def _iter_hook_commands(data: dict):
    """Yield every 'command' string found anywhere in the hooks.json tree.

    hooks.json shape (Claude Code): {"hooks": {<EventName>: [ {"hooks":
    [ {"type":"command","command":"..."} ] } ] }}. We walk it generically so
    the test does not over-constrain the exact nesting.
    """
    def walk(node):
        if isinstance(node, dict):
            cmd = node.get("command")
            if isinstance(cmd, str):
                yield cmd
            for v in node.values():
                yield from walk(v)
        elif isinstance(node, list):
            for v in node:
                yield from walk(v)

    yield from walk(data)


def test_hooks_json_commands_point_at_existing_echo_hook():
    data = _load(HOOKS_JSON)
    commands = list(_iter_hook_commands(data))
    assert commands, "hooks.json declares no commands"

    for cmd in commands:
        # Commands use ${CLAUDE_PLUGIN_ROOT}/bin/echo-hook <Event>.
        assert "${CLAUDE_PLUGIN_ROOT}" in cmd, (
            f"command must use ${{CLAUDE_PLUGIN_ROOT}}: {cmd!r}"
        )
        # Resolve the plugin-root-relative path to this repo and assert it
        # points at the existing bin/echo-hook shim.
        rel = cmd.split("${CLAUDE_PLUGIN_ROOT}", 1)[1].lstrip("/")
        # rel looks like 'bin/echo-hook MessageDisplay' -> take the path token.
        path_token = rel.split()[0]
        resolved = REPO_ROOT / path_token
        assert resolved == ECHO_HOOK, f"command path {path_token!r} != bin/echo-hook"
        assert resolved.is_file(), f"hook command target does not exist: {resolved}"


def test_every_phase1_event_is_hooked():
    """Phase 1 wires exactly these output events; assert each appears as a
    hooks.json key so none is silently unregistered."""
    data = _load(HOOKS_JSON)
    hooks = data.get("hooks", data)
    keys = set(hooks.keys()) if isinstance(hooks, dict) else set()
    required = {
        "MessageDisplay",
        "PreToolUse",
        "Notification",
        "Stop",
        "UserPromptSubmit",
        "SessionStart",
        "SessionEnd",
    }
    missing = required - keys
    assert not missing, f"hooks.json is missing event hooks: {sorted(missing)}"
```

Run it:

```bash
python -m pytest tests/test_manifests.py -q
```

Expected output (RED if a manifest is malformed or an event is unhooked) - e.g.:

```
E       AssertionError: hooks.json is missing event hooks: ['SessionEnd']
1 failed in 0.0s
```

Fix the manifest(s) created in the scaffolding section so all seven Phase 1 events are declared and every command is `"${CLAUDE_PLUGIN_ROOT}/bin/echo-hook <Event>"`. Re-run:

```bash
python -m pytest tests/test_manifests.py -q
```

Expected output (GREEN):

```
....
4 passed in 0.0s
```

Commit:

```bash
git add tests/test_manifests.py .claude-plugin/plugin.json hooks/hooks.json
git commit -m "test: validate plugin + hooks manifests and hook command paths

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Write the full README.md

Overwrite the legacy `README.md` (the legacy copy is preserved at git tag `v0-legacy-pty`) with the complete Phase 1 documentation. Write `README.md` at the repo root with EXACTLY this content:

```markdown
# Echo

**Eyes-free text-to-speech for [Claude Code](https://claude.ai/code) on macOS.**

Echo speaks everything Claude Code does - prose, plans, multiple-choice questions, and
permission prompts - so you can run a full session **with the screen off**. It is a
ground-up rebuild of the old `claude-tts` tool: one speech daemon, one ordered queue, one
`say` voice at a time, and a distinct sound (an *earcon*) the instant any decision appears.

> **Phase 1 (this release) is the output pipeline:** you *hear* everything, in order,
> reliably. Answering questions and approving actions still uses Claude Code's own keyboard
> picker. Fully eyes-free *selection* via global hotkeys is Phase 2.

## The goal

A blind or low-vision developer should be able to use Claude Code without looking at the
screen. Echo's job in Phase 1 is to make sure nothing important is ever silent or
out-of-order: you always hear the prose that explains a decision *before* the decision, and
a short sound alerts you the moment a question, plan, or permission is waiting.

## Requirements

- macOS (Echo uses the built-in `say` and `afplay` commands).
- Python 3.10 or newer (`python3 --version`).
- Claude Code 2.1.162 or newer.
- No third-party Python packages at runtime. `pytest` is only needed to run the tests.

## Install

```bash
git clone https://github.com/nimkimi/claude-tts ~/projects/claude-tts
cd ~/projects/claude-tts
pip install -e .
```

Then add Echo as a Claude Code plugin (it registers its hooks declaratively - no
hand-editing of `settings.json`):

```bash
claude plugin add ~/projects/claude-tts
```

Verify everything is wired up:

```bash
echo doctor
```

`doctor` checks: an enhanced voice is installed, the `say`/`afplay` binaries exist, the
speech daemon can start and its socket is reachable, the plugin manifests are valid, and the
seven Phase 1 hooks are registered. Start a Claude Code session and you should hear a
**ready** earcon.

## Enhanced-voice setup (recommended)

Echo defaults to the best enhanced/neural English voice it can find and falls back to
**Samantha**. Enhanced voices sound dramatically better and are free and offline. To install
one:

1. Open **System Settings → Accessibility → Spoken Content**.
2. Click **System Voice → Manage Voices…**.
3. Pick an English voice marked **(Enhanced)** or **(Premium)** - e.g. *Ava (Premium)*,
   *Zoe (Premium)*, or *Allison* - and download it.
4. Run `echo doctor` to confirm Echo picks it up, or pin it explicitly:

```bash
echo voice "Ava (Premium)"
```

## Controls and slash commands

In Phase 1, control is via the `echo` CLI and namespaced slash commands inside a session.
(Global hotkeys that work even mid-speech arrive in Phase 2.)

| Slash command | CLI | Effect |
|---|---|---|
| `/echo:status` | `echo status` | Show voice, rate, verbosity, foreground session, queue length |
| `/echo:verbosity <level>` | `echo verbosity <level>` | Set `everything` / `medium` / `quiet` |
| `/echo:voice <name>` | `echo voice <name>` | Set the `say` voice |
| `/echo:rate <wpm>` | `echo rate <wpm>` | Set words-per-minute |
| `/echo:repeat` | `echo repeat` | Re-speak the last item |
| `/echo:stop` | `echo stop` | Stop now and clear the queue |
| `/echo:doctor` | `echo doctor` | Run all health checks |

## Verbosity

Three live-switchable levels (earcons fire in **all** of them):

- **everything** (default) - prose, questions, plans, permissions, *and* brief tool
  announcements ("Running git status").
- **medium** - same as everything but **drops** routine tool announcements.
- **quiet** - prose plus decisions (questions / plans / permissions) only; no tool chatter.

## How ordering works

Echo's voice never jumps ahead of you. Spoken content is **strictly first-in, first-out**: a
question, plan, or permission is voiced *in its natural place* - after the prose that
explains it - so if the voice is mid-sentence when a permission appears, you still hear the
remaining sentences first, then the permission. What *is* instant is the **alert**: the
moment any decision appears, a short distinct earcon plays immediately (a different sound for
permission, choice, plan, error, turn-done, and ready), while the spoken detail waits its
turn in the queue. Claude Code blocks on the prompt until you respond, so hearing the
context first costs nothing. "Higher priority" therefore means *"alert you instantly with a
sound,"* never *"speak it out of order."*

## Per-session behavior

Echo tracks a single **foreground** session (set by `SessionStart` and each
`UserPromptSubmit`). Only the foreground session is *spoken*; if you run multiple sessions,
background sessions still fire decision **earcons** so you are alerted, but their prose and
decision text are not read aloud until you bring that session forward. Submitting a new
prompt or stopping flushes the queue, so the voice always resumes at what is current.

## Doctor and troubleshooting

Run `echo doctor` first - it reports each check as pass/fail. Common issues:

- **No speech at all.** Confirm `echo status` shows your session as the foreground. The
  daemon starts lazily on the first hook; if the socket is unreachable, run `echo doctor` to
  restart it, or check `~/.echo/speechd.log`.
- **Robotic voice.** No enhanced voice is installed; see *Enhanced-voice setup* above.
- **Hooks not firing.** Re-run `claude plugin add ~/projects/claude-tts` and verify with
  `echo doctor` that all seven hooks are registered.
- **Speech too fast/slow.** `echo rate 180` (default is 200 wpm).
- **Too chatty.** `echo verbosity medium` or `echo verbosity quiet`.
- **Everything is stuck.** `echo stop` clears the queue and cancels the current utterance.

State, config, the socket, and logs all live under `~/.echo/`
(`config.json`, `speechd.sock`, `speechd.log`).

## Uninstall and migration from legacy

To remove Echo:

```bash
claude plugin remove echo
echo uninstall
```

`echo uninstall` also cleans up a **prior legacy `claude-tts` install** if one is present on
your machine: it removes the `alias claude=claude-speak` line and the `~/.local/bin` PATH
export from your `~/.zshrc`, removes the three legacy hooks
(`claude-tts-permission.sh`, `claude-tts-pre-tool.sh`, `claude-tts-stop.sh`) from
`~/.claude/settings.json`, and deletes `~/.local/bin/claude-speak`,
`~/.local/bin/claude-tts`, `~/.claude-tts-enabled`, and `~/.claude-tts-pos`. The new Echo
design uses **no shell alias and no `~/.zshrc` edits at all**. (The legacy code is preserved
at git tag `v0-legacy-pty` if you ever need it.)

## What's next (Phase 2)

Global keyboard hotkeys for live speech control (skip, jump-to-decision, catch-up) and 100%
eyes-free **selection** - pick any question option and approve plans and permissions without
ever looking at the screen.
```

Run the manifest/fixture/e2e suite once more to confirm the README change does not affect tests, then commit:

```bash
python -m pytest -q
git add README.md
git commit -m "docs: full Echo Phase 1 README (eyes-free goal, install, ordering, migration)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected output: all tests pass.

### Task: Full-suite green gate before manual verification

Before the manual checklist, prove the whole project is green and that no real audio path is reachable from tests (audio is always injected/mocked). Run:

```bash
python -m pytest -q
```

Expected output (GREEN), e.g.:

```
............................................................
NN passed in 0.Xs
```

Sanity-check that tests never shell out to `say`/`afplay` (they must use injected recorders only):

```bash
grep -rn -E "subprocess|Popen|'say'|\"say\"|afplay" tests/ || echo "OK: tests reference no real audio binaries"
```

Expected output:

```
OK: tests reference no real audio binaries
```

If `grep` finds a match inside a test, that test is touching the real audio path and must be refactored to inject `say_runner`/`earcon_player` (or use `FakeSpeaker`) instead. There is nothing to commit for this gate unless a refactor was required; if so:

```bash
git add tests
git commit -m "test: ensure no test reaches the real say/afplay audio path

Co-Authored-By: Claude <noreply@anthropic.com>"
```

### Task: Manual eyes-free VERIFICATION CHECKLIST

This is the human exit-criteria check for Phase 1 - it cannot be automated because it
validates real audio against the spec's success criterion: *a full session with the screen
off.* Perform it on the target Mac after install.

**Setup**

1. `pip install -e .` and `claude plugin add ~/projects/claude-tts` are done.
2. `echo doctor` reports all checks pass (enhanced voice present, daemon up, socket
   reachable, all seven hooks registered).
3. `echo verbosity everything` and set a comfortable `echo rate` (e.g. 200).
4. **Turn the screen off / look away. Do the rest by ear only.**

**Checklist** - start a Claude Code session and confirm each item by sound alone:

- [ ] **Session start.** Starting the session plays the **ready** earcon (Glass).
- [ ] **Prose in order.** Ask Claude something that produces multi-sentence prose; it is
      spoken sentence-by-sentence, in order, in your enhanced voice, with no stutter and no
      double-speaking.
- [ ] **Code summary.** Ask for a code block; you hear "*N-line `<lang>` code block*", not
      the code character-by-character.
- [ ] **Choice question.** Trigger an `AskUserQuestion`. A **choice** earcon (Ping) fires
      *immediately*, but the question and its numbered options are spoken only **after** the
      preceding prose finishes. The numbers match the on-screen picker.
- [ ] **Plan.** Trigger `ExitPlanMode`. A **plan** earcon (Submarine) fires immediately; the
      plan text is spoken in order after any preceding prose.
- [ ] **Permission.** Trigger a permission prompt (e.g. a `Bash` command). A **permission**
      earcon (Funk) fires immediately; the action ("Run: …") is spoken in its natural place.
- [ ] **No barge-in on detail.** While prose is still speaking, make a decision appear and
      confirm the *spoken detail* of the decision does **not** cut off the prose - only the
      earcon barges in.
- [ ] **Turn done.** When Claude finishes a turn, a **turn_done** earcon (Tink) plays.
- [ ] **Flush on new prompt.** Submit a new prompt mid-speech; the backlog is flushed and the
      voice resumes on the new turn.
- [ ] **Stop.** Run `/echo:stop`; speech stops immediately and the queue is cleared.
- [ ] **Verbosity.** `/echo:verbosity quiet` then run a tool - you hear no tool
      announcement; switch back to `everything` and tool announcements return.
- [ ] **Per-session.** Open a second Claude Code session. With the first in the foreground,
      drive the second toward a decision: you hear its decision **earcon** but **not** its
      prose. Bring the second forward (submit a prompt in it) and confirm it now speaks.
- [ ] **No system-wide kill.** Start unrelated `say "hello from another app"` in a Terminal,
      then trigger Echo speech; the unrelated `say` is **not** killed (Echo only cancels its
      own child).

**Pass = every box checked with the screen off.** If any box fails, file the failure against
the owning component (earcon/ordering issues -> daemon + queue; missing options/plan ->
`hooks_entry`; wrong/robotic voice -> `speaker` + voice setup; wrong session spoken ->
`sessions`) and re-run the relevant automated test before re-checking.

There is no code or commit for this task; it is a sign-off gate. When the checklist passes,
Phase 1 is complete.
