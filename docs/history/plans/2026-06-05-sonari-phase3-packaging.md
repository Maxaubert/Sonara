# Sonari Phase 3 - Self-Contained Packaging & Installer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Sonari install as a self-contained, zero-dependency Claude Code plugin on macOS system python3 (>=3.9) - no pip/PyPI/Homebrew/notarization.

**Architecture:** Ship the stdlib-only src/sonari inside the plugin; run it via PYTHONPATH=<plugin>/src on a resolved absolute python3>=3.9; write that interpreter + plugin paths into the LaunchAgents; build hotkeyd locally via swiftc; add a ~/.local/bin/sonari launcher. Runtime data flow is unchanged.

**Tech Stack:** Python 3 stdlib + pytest (run under BOTH /usr/bin/python3 3.9 and the 3.13 venv), Swift (swiftc) for hotkeyd, macOS LaunchAgents, Unix-socket protocol.

---

## How to run tests

- **Fast per-task iteration (3.13):** `.venv/bin/python -m pytest -q`
- **3.9 compatibility gate:** `.venv39/bin/python -m pytest -q`
  - The controller pre-creates `.venv39` before execution begins via
    `/usr/bin/python3 -m venv .venv39 && .venv39/bin/pip install pytest`.
    **Assume `.venv39` already exists.** Only the final task (Task 12) runs the
    full dual-interpreter gate; earlier tasks may add a 3.9 invocation when they
    touch syntax-sensitive code, but the canonical fast loop is the 3.13 venv.
- Run a single test: `.venv/bin/python -m pytest tests/test_foo.py::test_bar -v`

## Safety rule (git)

Each task ends by staging the **exact** files it touched and committing. The
only git operations allowed in this plan are `git add <exact files>` and
`git commit`. Do **not** run `git reset`, `git checkout`, `git rebase`,
`git commit --amend`, `git stash`, `git clean`, `git rm`, or `git push`. The
repo is on branch `rebuild-echo`; all commits land there.

## File structure (what each task creates / modifies)

- `src/sonari/*.py` - add `from __future__ import annotations` to 13 modules (Task 1).
- `pyproject.toml` - `requires-python = ">=3.9"` (Task 1); `version = "0.3.0"` (Task 12).
- `src/sonari/cli.py` - `_resolve_python` (Task 2); `_xml_escape` + `_plist(env=)` +
  `_launchagent_plist` signature (Task 4); `install()` rewrite (Task 7); `uninstall()`
  change (Task 8); `doctor()` new checks (Task 9); `_dev_install_migrate` (Task 10).
- `src/sonari/paths.py` - `INSTALL_RECORD_PATH` (Task 5).
- `bin/sonari`, `bin/sonari-daemon`, `bin/sonari-hook` - self-locating rewrites (Task 3).
- `commands/sonari:voice.md`, `commands/sonari:rate.md`, `commands/sonari:skip.md` - new (Task 11).
- `.claude-plugin/plugin.json`, `.claude-plugin/marketplace.json` - version bump (Task 12).
- `README.md` - Requirements/Install rewrite (Task 12).
- `docs/superpowers/phase3-manual-smoke-checklist.md` - new fresh-install checklist (Task 12).
- New tests: `tests/test_py39_compat.py` (Task 1), `tests/test_cli_resolve_python.py` (Task 2),
  `tests/test_cli_launcher.py` (Task 6), `tests/test_cli_dev_migrate.py` (Task 10).
- Updated tests: `tests/test_sonari_hook_bin.py` + `tests/test_bin_sonari.py` (Task 3),
  `tests/test_cli_install.py` (Tasks 4, 5, 7), `tests/test_cli_uninstall.py` (Tasks 5, 8),
  `tests/test_cli_doctor.py` (Task 9), `tests/test_commands.py` (Task 11).

## Conventions to mirror (read before starting)

- Tests live in `tests/`; `tests/conftest.py` autouse-isolates every `~/.sonari` path to a
  per-test `tmp_path`, repointing `paths.*`, `config.*`, and `keymap.*` module copies.
- `cli._launchctl` is the single patch point for `launchctl`.
- Plists are parsed with `plistlib.loads(xml.encode("utf-8"))`.
- Install/doctor tests monkeypatch module-level path constants on `cli`, `cli.paths`,
  and `cli.keymap` (paths are bound by value at import time in `keymap.py`).
- `tests/daemon_helpers.py` provides `FakeSpeaker`/`make_daemon` for daemon tests (not used
  here, but the monkeypatch style matches).

---

## Task 1: `from __future__ import annotations` sweep + `requires-python >= 3.9`

**Files:**
- Test: `tests/test_py39_compat.py` (create)
- Modify: `src/sonari/__init__.py`, `src/sonari/assembler.py`, `src/sonari/cleaner.py`,
  `src/sonari/client.py`, `src/sonari/config.py`, `src/sonari/daemon.py`,
  `src/sonari/hooks_entry.py`, `src/sonari/keymap.py`, `src/sonari/paths.py`,
  `src/sonari/protocol.py`, `src/sonari/queue.py`, `src/sonari/sessions.py`,
  `src/sonari/speaker.py`
- Modify: `pyproject.toml:9`

Notes on placement: `from __future__ import annotations` must be the **first
statement** - after a module docstring if one exists, before any other import.
Modules WITH a docstring (insert future-import on the line after the closing `"""`):
`__init__.py`, `assembler.py`, `cleaner.py`, `config.py`, `hooks_entry.py`,
`keymap.py`, `protocol.py`. Modules WITHOUT a docstring (insert at line 1):
`client.py`, `daemon.py`, `paths.py`, `queue.py`, `sessions.py`, `speaker.py`.
`cli.py` already has the import - do not touch it.

- [ ] **Step 1: Write the failing test**

Create `tests/test_py39_compat.py`:

```python
"""Guard the public 3.9 target: every shipped module future-imports annotations,
and pyproject declares requires-python >= 3.9.

PEP 563 (the future import) defers annotation evaluation so `X | Y`-style hints
never run on 3.9, where they would raise at import time. cli.py already has it;
this test makes the rest of the package keep it.
"""
import os

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(REPO, "src", "sonari")

FUTURE = "from __future__ import annotations"


def _first_code_line(path):
    """Return the first non-blank, non-docstring source line of a module."""
    with open(path, encoding="utf-8") as f:
        text = f.read()
    lines = text.splitlines()
    i = 0
    # Skip leading blanks.
    while i < len(lines) and lines[i].strip() == "":
        i += 1
    # Skip a module docstring if present (single- or triple-quoted).
    if i < len(lines):
        s = lines[i].lstrip()
        for q in ('"""', "'''", '"', "'"):
            if s.startswith(q):
                # Find the closing quote (may be on the same line).
                rest = s[len(q):]
                if q in (('"""', "'''")) and rest.endswith(q) and len(rest) >= len(q):
                    i += 1
                elif q in ('"', "'") and rest.endswith(q):
                    i += 1
                else:
                    j = i + 1
                    while j < len(lines) and q not in lines[j]:
                        j += 1
                    i = j + 1
                break
    while i < len(lines) and lines[i].strip() == "":
        i += 1
    return lines[i].strip() if i < len(lines) else ""


def test_every_module_has_future_annotations():
    for name in os.listdir(SRC):
        if not name.endswith(".py"):
            continue
        path = os.path.join(SRC, name)
        assert _first_code_line(path) == FUTURE, (
            f"{name}: first code line must be {FUTURE!r}")


def test_pyproject_requires_python_39():
    pyproject = os.path.join(REPO, "pyproject.toml")
    with open(pyproject, encoding="utf-8") as f:
        text = f.read()
    assert 'requires-python = ">=3.9"' in text, (
        "pyproject must declare requires-python >= 3.9")
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_py39_compat.py -v`
Expected: FAIL - both tests fail (modules lack the future import; pyproject says `>=3.10`).

- [ ] **Step 3: Add the future import to all 13 modules and lower requires-python**

For each module WITHOUT a docstring, the first line becomes the future import.
`src/sonari/paths.py` head becomes:

```python
from __future__ import annotations

import os
import socket
from pathlib import Path
```

`src/sonari/daemon.py` head becomes:

```python
from __future__ import annotations

import os
import socket
import subprocess
import threading
```

`src/sonari/client.py` head becomes:

```python
from __future__ import annotations

import socket
import time
```

`src/sonari/queue.py` head becomes:

```python
from __future__ import annotations

from collections import deque
from dataclasses import dataclass
```

`src/sonari/sessions.py` head becomes:

```python
from __future__ import annotations


class SessionManager:
```

`src/sonari/speaker.py` head becomes:

```python
from __future__ import annotations

import os
import subprocess
import threading
```

For each module WITH a docstring, insert the future import on the line directly
after the closing `"""`. `src/sonari/__init__.py` becomes:

```python
"""Sonari: an eyes-free text-to-speech layer for Claude Code (macOS)."""
from __future__ import annotations

__version__ = "0.1.0"
```

`src/sonari/assembler.py` head becomes:

```python
"""Assemble streamed text deltas into complete, speakable chunks.

PURE: no I/O. Splits prose into sentences and replaces triple-backtick
fenced code blocks with a spoken one-line summary.
"""
from __future__ import annotations

import re
```

`src/sonari/cleaner.py` head becomes:

```python
"""Strip markdown noise from text so it reads naturally aloud.

PURE: no I/O. Does NOT handle triple-backtick fenced code blocks; the
ProseAssembler handles those before text reaches here.
"""
from __future__ import annotations

import re
```

`src/sonari/config.py` head becomes:

```python
"""Sonari persisted configuration: DEFAULTS plus load/save against CONFIG_PATH."""
from __future__ import annotations

import json
import os
```

`src/sonari/hooks_entry.py` head becomes:

```python
"""Pure mapping from Claude Code hook events to protocol message dicts."""
from __future__ import annotations

import os
```

`src/sonari/keymap.py` head becomes:

```python
"""Sonari Phase 2 keymap: ALL hotkey logic lives here (the Swift binary is dumb).

Maps key names -> macOS virtual key codes, modifier names -> Carbon masks, and
actions -> speechd protocol messages. Produces the resolved JSON array that the
Swift hotkeyd reads, registers, and sends on fire.
"""
from __future__ import annotations
```

`src/sonari/protocol.py` head becomes:

```python
"""Sonari wire protocol: newline-delimited JSON over a Unix stream socket."""
from __future__ import annotations

import json
```

Then in `pyproject.toml`, change line 9:

```toml
requires-python = ">=3.9"
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_py39_compat.py -v`
Expected: PASS (both tests green).

- [ ] **Step 5: Run the full suite under BOTH interpreters (this task changes module heads)**

Run: `.venv/bin/python -m pytest -q`
Expected: PASS, 0 failures.
Run: `.venv39/bin/python -m pytest -q`
Expected: PASS, 0 failures (proves the 3.9 target imports cleanly).

- [ ] **Step 6: Commit**

```bash
git add tests/test_py39_compat.py pyproject.toml \
  src/sonari/__init__.py src/sonari/assembler.py src/sonari/cleaner.py \
  src/sonari/client.py src/sonari/config.py src/sonari/daemon.py \
  src/sonari/hooks_entry.py src/sonari/keymap.py src/sonari/paths.py \
  src/sonari/protocol.py src/sonari/queue.py src/sonari/sessions.py \
  src/sonari/speaker.py
git commit -m "feat: future-annotations sweep + requires-python>=3.9 for self-contained 3.9 target"
```

---

## Task 2: `_resolve_python()` interpreter resolver

**Files:**
- Test: `tests/test_cli_resolve_python.py` (create)
- Modify: `src/sonari/cli.py` (add `_resolve_python` after `_daemon_shim_path`)

`_resolve_python()` builds a candidate list (`/usr/bin/python3` first, then
`shutil.which` for `python3`/`python3.13`..`python3.9`, deduped by realpath),
probes each for `>= (3, 9)`, prefers `/usr/bin/python3` when it qualifies, and
returns an absolute realpath or `None`. The probe is a tiny module-level helper
`_probe_python_version(path) -> tuple | None` so tests can patch it cleanly.

- [ ] **Step 1: Write the failing test**

Create `tests/test_cli_resolve_python.py`:

```python
from unittest import mock

from sonari import cli


def test_resolve_prefers_usr_bin_python3_when_it_qualifies():
    # /usr/bin/python3 reports 3.9; a newer 3.13 is also present, but stability
    # wins: /usr/bin/python3 must be chosen.
    def fake_which(name):
        return {"python3": "/opt/homebrew/bin/python3",
                "python3.13": "/opt/homebrew/bin/python3.13"}.get(name)

    def fake_realpath(p):
        return p  # identity so dedup is by literal path

    def fake_probe(path):
        return {"/usr/bin/python3": (3, 9),
                "/opt/homebrew/bin/python3": (3, 13),
                "/opt/homebrew/bin/python3.13": (3, 13)}.get(path)

    with mock.patch("shutil.which", side_effect=fake_which), \
         mock.patch("os.path.realpath", side_effect=fake_realpath), \
         mock.patch.object(cli, "_probe_python_version", side_effect=fake_probe):
        chosen = cli._resolve_python()
    assert chosen == "/usr/bin/python3"


def test_resolve_falls_back_to_first_qualifying_path_candidate():
    # /usr/bin/python3 is too old; the first qualifying PATH candidate wins.
    def fake_which(name):
        return {"python3": "/opt/homebrew/bin/python3"}.get(name)

    def fake_probe(path):
        return {"/usr/bin/python3": (3, 8),
                "/opt/homebrew/bin/python3": (3, 12)}.get(path)

    with mock.patch("shutil.which", side_effect=fake_which), \
         mock.patch("os.path.realpath", side_effect=lambda p: p), \
         mock.patch.object(cli, "_probe_python_version", side_effect=fake_probe):
        chosen = cli._resolve_python()
    assert chosen == "/opt/homebrew/bin/python3"


def test_resolve_returns_none_when_all_below_39():
    def fake_which(name):
        return {"python3": "/opt/homebrew/bin/python3"}.get(name)

    def fake_probe(path):
        return (3, 8)  # everything too old

    with mock.patch("shutil.which", side_effect=fake_which), \
         mock.patch("os.path.realpath", side_effect=lambda p: p), \
         mock.patch.object(cli, "_probe_python_version", side_effect=fake_probe):
        assert cli._resolve_python() is None


def test_resolve_dedups_candidates_by_realpath():
    # which('python3') and which('python3.9') both point at the same realpath;
    # the probe must be called once for that realpath, not twice.
    def fake_which(name):
        return {"python3": "/a/python3", "python3.9": "/a/python3.9"}.get(name)

    def fake_realpath(p):
        # both /a/python3 and /a/python3.9 resolve to one canonical path
        return "/canon/python3" if p in ("/a/python3", "/a/python3.9") else p

    probe = mock.Mock(side_effect=lambda p: (3, 9))
    with mock.patch("shutil.which", side_effect=fake_which), \
         mock.patch("os.path.realpath", side_effect=fake_realpath), \
         mock.patch.object(cli, "_probe_python_version", probe):
        chosen = cli._resolve_python()
    assert chosen == "/usr/bin/python3" or chosen == "/canon/python3"
    # /usr/bin/python3 + the single deduped /canon/python3 => at most 2 probes.
    assert probe.call_count <= 2
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_cli_resolve_python.py -v`
Expected: FAIL with `AttributeError: module 'sonari.cli' has no attribute '_resolve_python'`.

- [ ] **Step 3: Add `_probe_python_version` and `_resolve_python` to `cli.py`**

Insert into `src/sonari/cli.py` immediately after the `_daemon_shim_path`
function (after line 339, before `_plist`):

```python
_PYTHON_CANDIDATE_NAMES = (
    "python3", "python3.13", "python3.12", "python3.11", "python3.10",
    "python3.9",
)


def _probe_python_version(path: str):
    """Return (major, minor) reported by *path*, or None if it cannot be run.

    Patched in tests. Runs the interpreter so we read its REAL version, not the
    one running cli.py.
    """
    try:
        out = subprocess.check_output(
            [path, "-c", "import sys; print('%d.%d' % sys.version_info[:2])"],
            stderr=subprocess.DEVNULL, text=True, timeout=5).strip()
        major, minor = out.split(".")
        return (int(major), int(minor))
    except Exception:  # noqa: BLE001 - any failure means "not a usable python"
        return None


def _resolve_python():
    """Return the absolute realpath of the best python3 >= 3.9, or None.

    Preference: /usr/bin/python3 when it qualifies (guaranteed present and stable
    across logins); otherwise the first qualifying candidate in PATH order.
    Candidates are deduped by realpath so a symlink farm is probed once.
    """
    candidates = ["/usr/bin/python3"]
    for name in _PYTHON_CANDIDATE_NAMES:
        found = shutil.which(name)
        if found:
            candidates.append(found)

    seen = set()
    qualifying = []  # list of (realpath, was_usr_bin)
    for cand in candidates:
        real = os.path.realpath(cand)
        if real in seen:
            continue
        seen.add(real)
        ver = _probe_python_version(cand)
        if ver is not None and ver >= (3, 9):
            qualifying.append((real, cand == "/usr/bin/python3"))

    if not qualifying:
        return None
    for real, was_usr_bin in qualifying:
        if was_usr_bin:
            return real
    return qualifying[0][0]
```

Note: `subprocess` is already imported at module scope (line 323) and `shutil`
at line 16; no new imports needed.

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_cli_resolve_python.py -v`
Expected: PASS (all four tests).

- [ ] **Step 5: Commit**

```bash
git add tests/test_cli_resolve_python.py src/sonari/cli.py
git commit -m "feat: add _resolve_python() to pick best absolute python3>=3.9"
```

---

## Task 3: Self-locating `bin/` shims

**Files:**
- Modify: `bin/sonari-hook`, `bin/sonari-daemon`, `bin/sonari`
- Modify (tests): `tests/test_sonari_hook_bin.py` (add src-first ordering test),
  `tests/test_bin_sonari.py` (add scrubbed-env smoke test)

`bin/sonari-hook` already self-locates but only on `ImportError`; change it to
prepend `../src` to `sys.path` **unconditionally and first**, before any
`import sonari`, so a stale global `sonari` never shadows the plugin's source.
`bin/sonari-daemon` and `bin/sonari` become self-locating bash launchers that
put `<root>/src` on `PYTHONPATH` and exec a resolved `python3` (falling back to
`/usr/bin/python3`).

- [ ] **Step 1: Write the failing tests**

Add to `tests/test_sonari_hook_bin.py` (append at end):

```python
def test_hook_src_is_first_on_syspath_and_shadows_stale_global(tmp_path):
    """A stale globally-installed 'sonari' must NOT shadow the plugin's own src.

    We plant a fake 'sonari' package EARLIER on PYTHONPATH than the plugin src
    and assert the hook still resolves the real plugin package (its handle_event
    produces a real prose message). The rewritten shim inserts ../src at
    sys.path[0] before importing, so the real package wins.
    """
    stale = tmp_path / "stale"
    pkg = stale / "sonari"
    pkg.mkdir(parents=True)
    (pkg / "__init__.py").write_text("RAISE = True\n")
    # A stale hooks_entry that would crash if it were the one imported.
    (pkg / "hooks_entry.py").write_text("raise RuntimeError('stale wins')\n")

    sent_log = tmp_path / "sent.jsonl"
    env = dict(os.environ)
    # Put the STALE dir before the plugin src AND the fakeclient on PYTHONPATH.
    env["PYTHONPATH"] = os.pathsep.join([
        str(stale),
        str(REPO / "tests" / "_fakeclient"),
        str(REPO / "src"),
        env.get("PYTHONPATH", ""),
    ])
    env["SONARI_FAKE_SENT_LOG"] = str(sent_log)
    payload = json.dumps({"session_id": "s1", "delta": "Hi.",
                          "index": 0, "final": True}).encode()
    res = subprocess.run([sys.executable, str(HOOK), "MessageDisplay"],
                         input=payload, capture_output=True, env=env)
    assert res.returncode == 0, res.stderr.decode()
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "prose"
    assert lines[0]["delta"] == "Hi."
```

Add to `tests/test_bin_sonari.py` (append at end):

```python
import pytest


def test_cli_runs_under_usr_bin_python3_with_scrubbed_env():
    """PYTHONPATH=<repo>/src /usr/bin/python3 -m sonari.cli --help exits 0 with
    NO installed sonari anywhere - proves the self-contained source path works on
    the macOS system interpreter. Skipped if /usr/bin/python3 is absent.
    """
    sys_py = "/usr/bin/python3"
    if not os.path.exists(sys_py):
        pytest.skip("/usr/bin/python3 not present")
    # Scrub the environment so nothing but our src/ can supply 'sonari'.
    env = {
        "PATH": "/usr/bin:/bin",
        "PYTHONPATH": os.path.join(REPO, "src"),
        "HOME": os.environ.get("HOME", "/tmp"),
    }
    proc = subprocess.run([sys_py, "-m", "sonari.cli", "--help"],
                          capture_output=True, text=True, env=env)
    assert proc.returncode == 0, proc.stderr
    assert "usage" in proc.stdout.lower()
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_sonari_hook_bin.py::test_hook_src_is_first_on_syspath_and_shadows_stale_global tests/test_bin_sonari.py::test_cli_runs_under_usr_bin_python3_with_scrubbed_env -v`
Expected: FAIL - the hook test fails because the current shim imports the stale
`sonari` first (it only falls back to `../src` on ImportError), raising
`RuntimeError('stale wins')` inside the swallowing try/except (no message sent).
(The scrubbed-env smoke test may already pass because the current `bin/sonari`
relies on PYTHONPATH; if it passes that is fine - it will keep passing after the
rewrite.)

- [ ] **Step 3: Rewrite the three shims**

Replace `bin/sonari-hook` lines 33-41 (the `# Resolve the package: ...` block).

From:

```python
    # Resolve the package: prefer an installed 'sonari'; fall back to ../src.
    try:
        import sonari  # noqa: F401
    except Exception:
        here = os.path.dirname(os.path.abspath(__file__))
        src = os.path.join(os.path.dirname(here), "src")
        if src not in sys.path:
            sys.path.insert(0, src)
```

To:

```python
    # Resolve the package from the plugin's own src/ (self-contained; never rely
    # on an installed 'sonari'). Insert first so it shadows any stale global copy.
    here = os.path.dirname(os.path.abspath(__file__))
    src = os.path.join(os.path.dirname(here), "src")
    if src not in sys.path:
        sys.path.insert(0, src)
```

Replace the entire `bin/sonari-daemon` with:

```bash
#!/usr/bin/env bash
# Self-contained: run the plugin's own src with no installed 'sonari'.
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
export PYTHONPATH="$root/src${PYTHONPATH:+:$PYTHONPATH}"
py="$(command -v python3 || true)"
[ -x "$py" ] || py="/usr/bin/python3"
exec "$py" -m sonari.daemon "$@"
```

Replace the entire `bin/sonari` with:

```bash
#!/usr/bin/env bash
# Self-contained: run the plugin's own src with no installed 'sonari'.
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
export PYTHONPATH="$root/src${PYTHONPATH:+:$PYTHONPATH}"
py="$(command -v python3 || true)"
[ -x "$py" ] || py="/usr/bin/python3"
exec "$py" -m sonari.cli "$@"
```

Mark all three executable:

```bash
chmod +x bin/sonari bin/sonari-daemon bin/sonari-hook
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_sonari_hook_bin.py tests/test_bin_sonari.py -v`
Expected: PASS (existing shim tests + the two new tests).

- [ ] **Step 5: Commit**

```bash
git add bin/sonari bin/sonari-daemon bin/sonari-hook \
  tests/test_sonari_hook_bin.py tests/test_bin_sonari.py
git commit -m "feat: self-locating bin shims (src-first, resolved python3)"
```

---

## Task 4: plist helper - `env` injection + XML-escaping

**Files:**
- Modify: `src/sonari/cli.py` (`_plist`, `_launchagent_plist`, add `_xml_escape`)
- Modify (tests): `tests/test_cli_install.py` (replace the two `_launchagent_plist`
  tests that assume the `sys.executable` default + add an escaping test)

`_plist` gains an optional `env: dict | None`; when given it emits an
`EnvironmentVariables` `<dict>`. All interpolated strings are XML-escaped via a
new `_xml_escape`. `_launchagent_plist` changes signature to take a **required**
`python_executable` and the plugin `src_path`, emits
`[<py>, "-m", "sonari.daemon"]` with `env={"PYTHONPATH": src_path}`, and drops
the `sys.executable` default. `_hotkeyd_plist` is unchanged.

- [ ] **Step 1: Write/replace the failing tests**

In `tests/test_cli_install.py`, **replace** the existing
`test_launchagent_plist_is_valid_and_complete` and
`test_launchagent_plist_uses_absolute_python_not_bare_python3` functions
(lines 9-56) with:

```python
def test_launchagent_plist_embeds_resolved_python_and_pythonpath(tmp_path):
    log = "/home/u/.sonari/speechd.log"
    fake_python = "/usr/bin/python3"
    src = "/Users/u/.claude/plugins/sonari/src"
    xml = cli._launchagent_plist(python_executable=fake_python,
                                 src_path=src, log_path=log)
    assert isinstance(xml, str)
    assert xml.startswith("<?xml")
    data = plistlib.loads(xml.encode("utf-8"))
    assert data["Label"] == cli.LAUNCH_AGENT_LABEL
    assert data["ProgramArguments"] == [fake_python, "-m", "sonari.daemon"]
    assert data["EnvironmentVariables"]["PYTHONPATH"] == src
    assert data["RunAtLoad"] is True
    assert data["KeepAlive"] is True
    assert data["StandardErrorPath"] == log
    assert data["StandardOutPath"] == log
    # First arg must be an absolute interpreter path, never a bare name.
    interpreter = data["ProgramArguments"][0]
    assert os.path.isabs(interpreter)
    assert interpreter not in ("python3", "python")


def test_plist_xml_escapes_special_chars_in_paths():
    """A plugin path containing & / space / < must not corrupt the plist; the
    parsed PYTHONPATH must equal the original string intact."""
    log = "/home/u/.sonari/speechd.log"
    fake_python = "/usr/bin/python3"
    src = "/Users/u/My Plugins/A & B/<sonari>/src"
    xml = cli._launchagent_plist(python_executable=fake_python,
                                 src_path=src, log_path=log)
    # Raw XML must not contain a bare unescaped '&' or '<' inside the src value.
    assert "A & B" not in xml  # the bare ampersand was escaped
    assert "&amp;" in xml
    data = plistlib.loads(xml.encode("utf-8"))
    assert data["EnvironmentVariables"]["PYTHONPATH"] == src
```

Note: `import sys` is still used by other tests in this file (the install test),
so leave the imports as-is.

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py -k plist -v`
Expected: FAIL - `_launchagent_plist` does not accept `src_path=` and does not
emit `EnvironmentVariables`; the escaping test fails because `_plist` interpolates
raw strings.

- [ ] **Step 3: Add `_xml_escape`, update `_plist`, rewrite `_launchagent_plist`**

In `src/sonari/cli.py`, insert `_xml_escape` directly above `_plist` (before
line 342):

```python
def _xml_escape(s: str) -> str:
    """Escape the three XML-significant characters for safe plist interpolation."""
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
```

Replace the whole `_plist` function (lines 342-375) with:

```python
def _plist(label: str, program_args: list, log_path: str,
           env: Optional[dict] = None) -> str:
    """Return a full LaunchAgent plist XML for *label*.

    *program_args* is the ProgramArguments array (already absolute paths).
    *env*, when given, is emitted as an EnvironmentVariables <dict> (used to
    inject PYTHONPATH for the self-contained speech daemon). Every interpolated
    string is XML-escaped so a path containing &, <, or > cannot corrupt the
    plist. RunAtLoad + KeepAlive keep the agent alive in the Aqua (GUI) session;
    ProcessType Interactive so it participates in the foreground session that
    Carbon hotkeys require.
    """
    args_xml = "".join(
        f"        <string>{_xml_escape(a)}</string>\n" for a in program_args)
    env_xml = ""
    if env:
        pairs = "".join(
            f"        <key>{_xml_escape(k)}</key>\n"
            f"        <string>{_xml_escape(v)}</string>\n"
            for k, v in env.items())
        env_xml = (
            '    <key>EnvironmentVariables</key>\n'
            '    <dict>\n'
            f'{pairs}'
            '    </dict>\n'
        )
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
        '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
        '<plist version="1.0">\n'
        '<dict>\n'
        '    <key>Label</key>\n'
        f'    <string>{_xml_escape(label)}</string>\n'
        '    <key>ProgramArguments</key>\n'
        '    <array>\n'
        f'{args_xml}'
        '    </array>\n'
        f'{env_xml}'
        '    <key>RunAtLoad</key>\n'
        '    <true/>\n'
        '    <key>KeepAlive</key>\n'
        '    <true/>\n'
        '    <key>StandardErrorPath</key>\n'
        f'    <string>{_xml_escape(log_path)}</string>\n'
        '    <key>StandardOutPath</key>\n'
        f'    <string>{_xml_escape(log_path)}</string>\n'
        '    <key>ProcessType</key>\n'
        '    <string>Interactive</string>\n'
        '</dict>\n'
        '</plist>\n'
    )
```

Replace the whole `_launchagent_plist` function (lines 378-397) with:

```python
def _launchagent_plist(python_executable: str, src_path: str,
                       log_path: str) -> str:
    """Return the LaunchAgent plist XML for the speech daemon.

    *python_executable* is the resolved absolute interpreter (>= 3.9).
    *src_path* is the plugin's <root>/src directory; it is injected as
    PYTHONPATH so the daemon imports the plugin's own source with no installed
    'sonari'. ProgramArguments runs the module directly: [<py>, -m, sonari.daemon].
    """
    return _plist(
        LAUNCH_AGENT_LABEL,
        [python_executable, "-m", "sonari.daemon"],
        log_path,
        env={"PYTHONPATH": src_path},
    )
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py -k plist -v`
Expected: PASS (both new plist tests).

Also confirm the hotkeyd plist test still passes (it does not pass `env`, so its
plist must NOT contain `EnvironmentVariables`):

Run: `.venv/bin/python -m pytest tests/test_cli_hotkeyd.py::test_hotkeyd_plist_is_valid_and_complete -v`
Expected: PASS.

Note: `test_install_writes_plist_and_loads` and the hotkeyd `install` tests will
break at this point because `install()` still calls the OLD `_launchagent_plist`
signature; that call is rewritten in Task 7, which updates those install tests.
Run only the `-k plist` selection here.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_install.py
git commit -m "feat: plist EnvironmentVariables (PYTHONPATH) + XML-escaping; resolved-interp signature"
```

---

## Task 5: `paths.INSTALL_RECORD_PATH` + install.json write/read scaffolding

**Files:**
- Modify: `src/sonari/paths.py` (add `INSTALL_RECORD_PATH`)
- Test: `tests/test_paths.py` (append a constant test)

This task only adds the path constant and a helper `_write_install_record` in
`cli.py` so later tasks (7/9) can call it. The actual `install()` call site is
wired in Task 7; this keeps the constant + writer self-contained and tested.

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_paths.py`:

```python
def test_install_record_path_lives_under_sonari_dir():
    from sonari import paths
    assert paths.INSTALL_RECORD_PATH == paths.SONARI_DIR / "install.json"
```

Append to `tests/test_cli_install.py`:

```python
def test_write_install_record_writes_expected_keys(tmp_path):
    rec = tmp_path / "install.json"
    with mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", rec):
        cli._write_install_record(
            python="/usr/bin/python3",
            python_version="3.9",
            plugin_root="/plug",
            src="/plug/src",
        )
    import json as _json
    data = _json.loads(rec.read_text())
    assert data["python"] == "/usr/bin/python3"
    assert data["python_version"] == "3.9"
    assert data["plugin_root"] == "/plug"
    assert data["src"] == "/plug/src"
    assert "installed_at" in data and isinstance(data["installed_at"], str)
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_paths.py::test_install_record_path_lives_under_sonari_dir tests/test_cli_install.py::test_write_install_record_writes_expected_keys -v`
Expected: FAIL - `paths.INSTALL_RECORD_PATH` and `cli._write_install_record` do not exist.

- [ ] **Step 3: Add the constant and the writer**

In `src/sonari/paths.py`, after the `HOTKEYD_BIN_PATH` line (line 11), add:

```python
INSTALL_RECORD_PATH = SONARI_DIR / "install.json"
```

In `src/sonari/cli.py`, add (near `_resolve_python`, after it):

```python
def _write_install_record(python: str, python_version: str,
                          plugin_root: str, src: str) -> None:
    """Persist the durable install record used by doctor + migration."""
    from datetime import datetime, timezone
    record = {
        "python": python,
        "python_version": python_version,
        "plugin_root": plugin_root,
        "src": src,
        "installed_at": datetime.now(timezone.utc).isoformat(),
    }
    os.makedirs(os.path.dirname(str(paths.INSTALL_RECORD_PATH)), exist_ok=True)
    with open(str(paths.INSTALL_RECORD_PATH), "w", encoding="utf-8") as f:
        json.dump(record, f, indent=2)
        f.write("\n")
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_paths.py::test_install_record_path_lives_under_sonari_dir tests/test_cli_install.py::test_write_install_record_writes_expected_keys -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/paths.py src/sonari/cli.py tests/test_paths.py tests/test_cli_install.py
git commit -m "feat: INSTALL_RECORD_PATH + _write_install_record(install.json)"
```

---

## Task 6: `~/.local/bin/sonari` launcher (placement + removal)

**Files:**
- Modify: `src/sonari/cli.py` (add `_launcher_path`, `_place_launcher`,
  `_remove_launcher`, `_local_bin_on_path`)
- Test: `tests/test_cli_launcher.py` (create)

A small, independently testable launcher module: place a `0o755` wrapper at
`~/.local/bin/sonari` that execs the absolute plugin `bin/sonari`, detect whether
`~/.local/bin` is on PATH, and remove the launcher. Tasks 7 and 8 wire these into
`install()`/`uninstall()`.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_cli_launcher.py`:

```python
import os
import stat
from unittest import mock

from sonari import cli


def test_launcher_path_is_local_bin_sonari(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    assert cli._launcher_path() == str(tmp_path / ".local" / "bin" / "sonari")


def test_place_launcher_writes_executable_wrapper(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    plugin_root = "/Users/u/My Plugins/sonari"
    cli._place_launcher(plugin_root)
    path = tmp_path / ".local" / "bin" / "sonari"
    assert path.exists()
    mode = stat.S_IMODE(os.stat(path).st_mode)
    assert mode == 0o755
    text = path.read_text()
    # Execs the absolute plugin bin/sonari, with the (spaced) path quoted.
    assert 'exec "/Users/u/My Plugins/sonari/bin/sonari" "$@"' in text


def test_place_launcher_overwrites_existing(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    lb = tmp_path / ".local" / "bin"
    lb.mkdir(parents=True)
    (lb / "sonari").write_text("#!/bin/sh\necho stale\n")
    cli._place_launcher("/plug")
    assert 'exec "/plug/bin/sonari" "$@"' in (lb / "sonari").read_text()


def test_remove_launcher_deletes_file(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    lb = tmp_path / ".local" / "bin"
    lb.mkdir(parents=True)
    (lb / "sonari").write_text("x")
    removed = cli._remove_launcher()
    assert removed is True
    assert not (lb / "sonari").exists()


def test_remove_launcher_absent_is_noop(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    assert cli._remove_launcher() is False


def test_local_bin_on_path_true_when_present(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    lb = str(tmp_path / ".local" / "bin")
    monkeypatch.setenv("PATH", lb + ":/usr/bin")
    assert cli._local_bin_on_path() is True


def test_local_bin_on_path_false_when_absent(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("PATH", "/usr/bin:/bin")
    assert cli._local_bin_on_path() is False
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_cli_launcher.py -v`
Expected: FAIL with `AttributeError` for `_launcher_path` (and the others).

- [ ] **Step 3: Add the launcher helpers to `cli.py`**

Add to `src/sonari/cli.py` (after `_write_install_record`):

```python
def _local_bin_dir() -> str:
    return os.path.join(os.path.expanduser("~"), ".local", "bin")


def _launcher_path() -> str:
    return os.path.join(_local_bin_dir(), "sonari")


def _place_launcher(plugin_root: str) -> str:
    """Write an executable ~/.local/bin/sonari that execs the plugin bin/sonari.

    The plugin path is baked in and shell-quoted so a path with spaces is safe.
    Overwrites any prior Sonari-owned launcher. Returns the launcher path.
    """
    target = os.path.join(plugin_root, "bin", "sonari")
    wrapper = (
        "#!/usr/bin/env bash\n"
        f'exec "{target}" "$@"\n'
    )
    path = _launcher_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(wrapper)
    os.chmod(path, 0o755)
    return path


def _remove_launcher() -> bool:
    """Remove ~/.local/bin/sonari if present. Returns True if it was removed."""
    path = _launcher_path()
    if os.path.exists(path):
        try:
            os.remove(path)
            return True
        except OSError:
            return False
    return False


def _local_bin_on_path() -> bool:
    """Return True if ~/.local/bin is on the current PATH."""
    lb = _local_bin_dir()
    entries = os.environ.get("PATH", "").split(os.pathsep)
    return lb in entries
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_cli_launcher.py -v`
Expected: PASS (all seven tests).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_launcher.py
git commit -m "feat: ~/.local/bin/sonari launcher place/remove + PATH detection helpers"
```

---

## Task 7: `install()` rewrite (resolve interp, install.json, new plists, launcher)

**Files:**
- Modify: `src/sonari/cli.py` (`install`)
- Modify (tests): `tests/test_cli_install.py` (`test_install_writes_plist_and_loads`),
  `tests/test_cli_hotkeyd.py` (`test_install_writes_hotkeyd_plist_and_keymap`,
  `test_install_build_failure_is_nonfatal`)

`install()` now: resolves python (fatal if `None`), pre-checks swiftc/CLT
(non-fatal), builds hotkeyd, writes keymap+resolved, writes `install.json`,
writes BOTH plists with the resolved interpreter + `PYTHONPATH` (skipping the
hotkeyd agent if no binary), places the launcher, runs migrations
(`_legacy_migrate` + `_dev_install_migrate` - the latter is added in Task 10 but
referenced here; until Task 10 lands, the install tests patch
`cli._dev_install_migrate`), reports the voice, and prints eyes-free next steps
including PATH advice.

To keep this task self-contained, `install()` calls `_dev_install_migrate` via
`getattr(self_module, ...)`-free direct reference; the function is defined in
Task 10. **Order the execution so Task 10 is done before Task 7, OR** stub it.
This plan orders Task 10 BEFORE Task 7 is impossible given numbering; therefore
add a minimal no-op `_dev_install_migrate` here and flesh it out in Task 10.
Add this stub now (Task 10 replaces the body):

- [ ] **Step 1: Add the `_dev_install_migrate` stub (so install() can call it)**

In `src/sonari/cli.py`, add directly below `_legacy_migrate` (after line 514):

```python
def _dev_install_migrate(home: Optional[str] = None) -> list:
    """Detect a dev editable 'sonari' footprint and PRINT cleanup guidance.

    Safe no-op when there is no dev footprint. Body is implemented in a later
    task; for now it returns no lines so install() can call it unconditionally.
    """
    return []
```

- [ ] **Step 2: Replace the install tests (write the new expectations first)**

In `tests/test_cli_install.py`, **replace** `test_install_writes_plist_and_loads`
(lines 59-83) with:

```python
def test_install_writes_plist_and_loads(tmp_path, capsys):
    la_dir = tmp_path / "LaunchAgents"
    plist = la_dir / (cli.LAUNCH_AGENT_LABEL + ".plist")
    record = tmp_path / "install.json"
    run = mock.Mock(return_value=0)
    monkeypatch_home = tmp_path / "home"
    monkeypatch_home.mkdir()
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_probe_python_version", return_value=(3, 9)), \
         mock.patch.object(cli, "_build_hotkeyd", return_value=(True, "built")), \
         mock.patch.object(cli, "_place_launcher", return_value=str(tmp_path / "launcher")), \
         mock.patch.object(cli, "_dev_install_migrate", return_value=[]), \
         mock.patch.object(cli, "_legacy_migrate", return_value=[]), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(tmp_path / "com.sonari.hotkeyd.plist")), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir", lambda: tmp_path.mkdir(parents=True, exist_ok=True)), \
         mock.patch("sonari.paths.ensure_sonari_dir") as ensure:
        rc = cli.install()
    assert rc == 0
    ensure.assert_called_once()
    assert plist.exists()
    # The speechd plist now embeds the resolved interpreter + PYTHONPATH=<src>.
    data = plistlib.loads(plist.read_text().encode("utf-8"))
    assert data["ProgramArguments"][0] == "/usr/bin/python3"
    assert data["ProgramArguments"][1:] == ["-m", "sonari.daemon"]
    assert data["EnvironmentVariables"]["PYTHONPATH"].endswith(os.path.join("", "src")) \
        or data["EnvironmentVariables"]["PYTHONPATH"].endswith("/src")
    # install.json was written with the resolved interpreter.
    import json as _json
    rec = _json.loads(record.read_text())
    assert rec["python"] == "/usr/bin/python3"
    assert rec["src"].endswith("src")
    # The launcher was placed.
    cli._place_launcher.assert_called_once()
    assert any(c.args[0][0] == "load" for c in run.call_args_list)
    out = capsys.readouterr().out
    assert "doctor" in out.lower()


def test_install_fatal_when_no_python_found(capsys):
    with mock.patch.object(cli, "_resolve_python", return_value=None), \
         mock.patch("sonari.paths.ensure_sonari_dir"):
        rc = cli.install()
    assert rc != 0
    out = capsys.readouterr().out
    assert "python3" in out.lower()
    assert "xcode-select --install" in out.lower()
```

In `tests/test_cli_hotkeyd.py`, **replace** `test_install_writes_hotkeyd_plist_and_keymap`
(lines 53-82) by adding the new mocks (`_resolve_python`, `_place_launcher`,
`_dev_install_migrate`, `_legacy_migrate`, `INSTALL_RECORD_PATH`). Replace the
`with` block's first lines so it reads:

```python
def test_install_writes_hotkeyd_plist_and_keymap(tmp_path, capsys):
    speechd_plist = tmp_path / "com.sonari.speechd.plist"
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    km = tmp_path / "keymap.json"
    resolved = tmp_path / "hotkeyd.resolved.json"
    binp = tmp_path / "sonari-hotkeyd"
    record = tmp_path / "install.json"
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(speechd_plist)), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_place_launcher", return_value=str(tmp_path / "launcher")), \
         mock.patch.object(cli, "_dev_install_migrate", return_value=[]), \
         mock.patch.object(cli, "_legacy_migrate", return_value=[]), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
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
    loads = [c.args[0] for c in run.call_args_list]
    assert any(a[0] == "load" and a[1] == str(hotkeyd_plist) for a in loads)
```

In `tests/test_cli_hotkeyd.py`, **replace** `test_install_build_failure_is_nonfatal`
(lines 85-109) so its `with` block adds the same four new mocks:

```python
def test_install_build_failure_is_nonfatal(tmp_path, capsys):
    speechd_plist = tmp_path / "com.sonari.speechd.plist"
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    km = tmp_path / "keymap.json"
    resolved = tmp_path / "hotkeyd.resolved.json"
    binp = tmp_path / "sonari-hotkeyd"
    record = tmp_path / "install.json"
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(speechd_plist)), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_place_launcher", return_value=str(tmp_path / "launcher")), \
         mock.patch.object(cli, "_dev_install_migrate", return_value=[]), \
         mock.patch.object(cli, "_legacy_migrate", return_value=[]), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
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
    # The hotkeyd LaunchAgent is NOT written when there is no binary.
    assert not hotkeyd_plist.exists()
    out = capsys.readouterr().out
    assert "warning" in out.lower() or "swiftc" in out.lower()
```

- [ ] **Step 3: Run the install tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py::test_install_writes_plist_and_loads tests/test_cli_install.py::test_install_fatal_when_no_python_found tests/test_cli_hotkeyd.py::test_install_writes_hotkeyd_plist_and_keymap tests/test_cli_hotkeyd.py::test_install_build_failure_is_nonfatal -v`
Expected: FAIL - `install()` still uses the old plist signature and writes the
hotkeyd plist even on build failure; `_resolve_python` is not consulted.

- [ ] **Step 4: Rewrite `install()`**

Replace the whole `install()` function (lines 431-478) with:

```python
def install() -> int:
    """Install Sonari as a self-contained plugin: resolve python, build hotkeyd,
    write both LaunchAgents (resolved interp + PYTHONPATH), place the launcher.
    """
    paths.ensure_sonari_dir()

    # 1. Resolve the best python3 >= 3.9 (FATAL if none).
    python = _resolve_python()
    if python is None:
        print("No suitable python3 found (need 3.9+). macOS normally ships "
              "/usr/bin/python3; if missing, install the Command Line Tools "
              "(xcode-select --install).")
        return 1
    ver = _probe_python_version(python)
    py_ver = "{0}.{1}".format(*ver) if ver else "3.9"
    print(f"Using interpreter: {python} (Python {py_ver})")

    plugin_root = os.path.realpath(paths.repo_root())
    src = os.path.join(plugin_root, "src")

    # 2. Pre-check swiftc / Command Line Tools (non-fatal).
    if shutil.which("swiftc") is None:
        print("Xcode Command Line Tools not found; global hotkeys disabled. "
              "Install them with:  xcode-select --install   then re-run: "
              "sonari install")

    # 3-4. Keymap + build hotkeyd.
    keymap.write_default_keymap_if_absent()
    keymap.write_resolved()
    ok, detail = _build_hotkeyd()

    # 5. Durable install record.
    _write_install_record(python=python, python_version=py_ver,
                          plugin_root=plugin_root, src=src)

    # 6. speechd LaunchAgent (resolved interpreter + PYTHONPATH=<src>).
    log = str(paths.LOG_PATH)
    xml = _launchagent_plist(python_executable=python, src_path=src,
                             log_path=log)
    os.makedirs(os.path.dirname(LAUNCH_AGENT_PATH), exist_ok=True)
    with open(LAUNCH_AGENT_PATH, "w", encoding="utf-8") as f:
        f.write(xml)
    print(f"Wrote LaunchAgent: {LAUNCH_AGENT_PATH}")
    _launchctl(["unload", LAUNCH_AGENT_PATH])
    rc = _launchctl(["load", LAUNCH_AGENT_PATH])
    if rc == 0:
        print(f"Loaded LaunchAgent {LAUNCH_AGENT_LABEL}.")
    else:
        print(f"warning: 'launchctl load' returned {rc}; "
              f"the daemon will still autostart on next login.")

    # 7. hotkeyd LaunchAgent (skip entirely if no binary).
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

    # 8. ~/.local/bin/sonari launcher.
    launcher = _place_launcher(plugin_root)
    print(f"Placed launcher: {launcher}")

    # 9. Migrations.
    for line in _legacy_migrate():
        print(f"  - {line}")
    for line in _dev_install_migrate():
        print(f"  - {line}")

    # 10. Voice check.
    try:
        from . import speaker
        voice = speaker.best_enhanced_voice()
        if voice:
            print(f"Voice: {voice}.")
        else:
            print("Voice: no enhanced voice found; will fall back to Samantha. "
                  "Install one via System Settings -> Accessibility -> "
                  "Spoken Content.")
    except Exception:  # noqa: BLE001 - voice check must never break install
        pass

    # 11. Eyes-free next steps.
    print("")
    print("Enable the Sonari plugin in Claude Code, then run 'sonari doctor'.")
    print(f"  - Per session: claude --plugin-dir {plugin_root}")
    print("  - Or enable 'sonari' from the /plugin menu (local marketplace).")
    if not _local_bin_on_path():
        print('Add ~/.local/bin to your PATH so `sonari` works in every shell:')
        print('  export PATH="$HOME/.local/bin:$PATH"')
    return 0
```

- [ ] **Step 5: Run the install tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py tests/test_cli_hotkeyd.py -v`
Expected: PASS (all install/hotkeyd tests).

- [ ] **Step 6: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_install.py tests/test_cli_hotkeyd.py
git commit -m "feat: install() self-contained (resolved interp, install.json, launcher, PATH advice)"
```

---

## Task 8: `uninstall()` change - preserve config.json, remove launcher + install.json

**Files:**
- Modify: `src/sonari/cli.py` (`uninstall`)
- Modify (tests): `tests/test_cli_uninstall.py` (assert config.json preserved,
  launcher + install.json removed)

`uninstall()` stops removing `paths.CONFIG_PATH` (now preserved alongside
`keymap.json`), removes `~/.local/bin/sonari` and `~/.sonari/install.json`, and
updates the "Preserved …" line to mention both config.json and keymap.json.

- [ ] **Step 1: Update the uninstall test (write new expectations)**

In `tests/test_cli_uninstall.py`, **replace**
`test_uninstall_removes_launchagent_but_preserves_keymap` (lines 8-55) with:

```python
def test_uninstall_removes_launchagent_but_preserves_keymap_and_config(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    plist = tmp_path / "com.sonari.speechd.plist"
    plist.write_text("<plist/>")
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    hotkeyd_plist.write_text("<plist/>")
    sonari_dir = tmp_path / ".sonari"
    sonari_dir.mkdir()
    # Runtime artifacts uninstall should remove.
    log = sonari_dir / "speechd.log"
    log.write_text("log")
    resolved = sonari_dir / "hotkeyd.resolved.json"
    resolved.write_text("[]")
    binp = sonari_dir / "sonari-hotkeyd"
    binp.write_text("binary")
    record = sonari_dir / "install.json"
    record.write_text("{}")
    # PRESERVED across uninstall: user keymap AND config.
    keymap = sonari_dir / "keymap.json"
    keymap.write_text('{"custom": true}')
    config = sonari_dir / "config.json"
    config.write_text('{"rate": 180}')
    # The launcher uninstall should remove.
    local_bin = tmp_path / ".local" / "bin"
    local_bin.mkdir(parents=True)
    launcher = local_bin / "sonari"
    launcher.write_text("#!/bin/sh\n")

    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli.paths, "SONARI_DIR", sonari_dir), \
         mock.patch.object(cli.paths, "CONFIG_PATH", config), \
         mock.patch.object(cli.paths, "LOG_PATH", log), \
         mock.patch.object(cli.paths, "SOCKET_PATH", sonari_dir / "speechd.sock"), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", keymap), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp), \
         mock.patch.object(cli, "_legacy_migrate", return_value=[]) as mig:
        rc = cli.uninstall()

    assert rc == 0
    assert not plist.exists()
    assert not hotkeyd_plist.exists()
    assert not binp.exists()
    assert not log.exists()
    assert not resolved.exists()
    assert not record.exists()
    assert not launcher.exists()
    # Preserved.
    assert keymap.exists()
    assert keymap.read_text() == '{"custom": true}'
    assert config.exists()
    assert config.read_text() == '{"rate": 180}'
    assert any(c.args[0][0] == "unload" for c in run.call_args_list)
    mig.assert_called_once()
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_cli_uninstall.py::test_uninstall_removes_launchagent_but_preserves_keymap_and_config -v`
Expected: FAIL - current `uninstall()` deletes `config.json` and never removes the
launcher or `install.json`.

- [ ] **Step 3: Update `uninstall()`**

In `src/sonari/cli.py`, in `uninstall()`, replace the artifacts block
(lines 543-563) with:

```python
    # Spec §5.4: remove Sonari-owned runtime artifacts but PRESERVE the user's
    # keymap.json AND config.json so customizations survive uninstall/reinstall.
    sonari_dir = paths.SONARI_DIR
    hk_log = sonari_dir / "hotkeyd.log"
    artifacts = [
        paths.SOCKET_PATH,
        paths.LOG_PATH,
        paths.HOTKEYD_RESOLVED_PATH,
        paths.INSTALL_RECORD_PATH,
        hk_log,
    ]
    for artifact in artifacts:
        if os.path.exists(str(artifact)):
            try:
                os.remove(str(artifact))
            except OSError:
                pass

    if _remove_launcher():
        print(f"Removed launcher: {_launcher_path()}")

    preserved = []
    if os.path.exists(str(paths.KEYMAP_PATH)):
        preserved.append("keymap.json")
    if os.path.exists(str(paths.CONFIG_PATH)):
        preserved.append("config.json")
    if preserved:
        print(f"Preserved your settings: {', '.join(preserved)}")
    print(f"Removed Sonari runtime files from {sonari_dir} "
          f"(keymap.json and config.json left in place).")
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_cli_uninstall.py -v`
Expected: PASS (all uninstall tests, including the legacy-migrate ones).

Also re-run the hotkeyd uninstall test (it does not set CONFIG_PATH but should
still pass since config removal is gone):

Run: `.venv/bin/python -m pytest tests/test_cli_hotkeyd.py::test_uninstall_removes_hotkeyd_agent_and_binary -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_uninstall.py
git commit -m "feat: uninstall preserves config.json, removes launcher + install.json"
```

---

## Task 9: `doctor()` new checks

**Files:**
- Modify: `src/sonari/cli.py` (`doctor`)
- Modify (tests): `tests/test_cli_doctor.py` (patch new probes in `_ok_patches`;
  assert new keys present)

Add five doctor checks: `python3 >= 3.9` (resolved path), `plugin path resolved`
(install.json exists + its src has `sonari/__init__.py`), `speechd LaunchAgent
loaded`, `hotkeyd LaunchAgent loaded` (both via `_launchctl`), `sonari launcher`
(present + `~/.local/bin` on PATH). Upgrade the `swiftc` detail to name
`xcode-select --install` when missing.

- [ ] **Step 1: Update the doctor "all ok" test (write new expectations)**

In `tests/test_cli_doctor.py`, **replace** `_ok_patches` (lines 6-15) with:

```python
def _ok_patches():
    """Context managers that make every doctor check pass."""
    return [
        mock.patch("shutil.which", side_effect=lambda n: "/usr/bin/" + n),
        mock.patch("sonari.speaker.best_enhanced_voice", return_value="Ava (Premium)"),
        mock.patch("os.access", return_value=True),
        mock.patch("sonari.paths.ensure_sonari_dir"),
        mock.patch("sonari.client.send", return_value={"ok": True}),
        mock.patch("os.path.exists", return_value=True),
        mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"),
        mock.patch.object(cli, "_launchctl", return_value=0),
        mock.patch.object(cli, "_local_bin_on_path", return_value=True),
        mock.patch.object(cli, "_read_install_record",
                          return_value={"src": "/plug/src"}),
    ]
```

**Replace** `test_doctor_all_ok` (lines 43-48) with:

```python
def test_doctor_all_ok():
    d = _as_dict(_run(_ok_patches()))
    for key in ("say", "afplay", "enhanced voice", "SONARI_DIR writable",
                "daemon socket", "plugin hooks.json", "python3",
                "plugin path resolved", "speechd LaunchAgent loaded",
                "hotkeyd LaunchAgent loaded", "sonari launcher"):
        assert key in d, key
        assert d[key][0] is True, (key, d[key])
```

Note: in `test_doctor_say_missing`, `test_doctor_socket_unreachable`,
`test_doctor_hooks_json_missing` the patches are indexed positionally
(`patches[0]`, `patches[4]`, `patches[5]`) - those indices are unchanged (the
new patches were appended at the END), so those tests keep working.

- [ ] **Step 2: Run the doctor test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_cli_doctor.py::test_doctor_all_ok -v`
Expected: FAIL - the new check keys (`python3`, `plugin path resolved`, etc.) and
`cli._read_install_record` do not exist yet.

- [ ] **Step 3: Add `_read_install_record` + the new doctor checks**

In `src/sonari/cli.py`, add a reader near `_write_install_record`:

```python
def _read_install_record():
    """Return the install.json record dict, or None if unreadable/absent."""
    try:
        with open(str(paths.INSTALL_RECORD_PATH), "r", encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else None
    except Exception:  # noqa: BLE001 - doctor must never raise
        return None
```

In `doctor()`, append the new checks just before `return results` (after the
`keymap resolves` block, after line 308):

```python
    # python3 >= 3.9 resolved.
    try:
        py = _resolve_python()
        results.append(("python3", py is not None,
                        py or "no python3 >= 3.9 found; install the Command "
                              "Line Tools (xcode-select --install)"))
    except Exception as exc:  # noqa: BLE001
        results.append(("python3", False, f"error: {exc}"))

    # plugin path resolved (install.json -> src contains sonari/__init__.py).
    try:
        rec = _read_install_record()
        src = rec.get("src") if rec else None
        init = os.path.join(src, "sonari", "__init__.py") if src else None
        ok = bool(init) and os.path.exists(init)
        results.append(("plugin path resolved", ok,
                        src if ok else "install.json missing or src has no "
                                       "sonari package (run 'sonari install')"))
    except Exception as exc:  # noqa: BLE001
        results.append(("plugin path resolved", False, f"error: {exc}"))

    # LaunchAgents loaded.
    speechd_loaded = _launchctl(["list", LAUNCH_AGENT_LABEL]) == 0
    results.append(("speechd LaunchAgent loaded", speechd_loaded,
                    LAUNCH_AGENT_LABEL if speechd_loaded
                    else "not loaded (run 'sonari install')"))
    hotkeyd_loaded = _launchctl(["list", HOTKEYD_LAUNCH_AGENT_LABEL]) == 0
    results.append(("hotkeyd LaunchAgent loaded", hotkeyd_loaded,
                    HOTKEYD_LAUNCH_AGENT_LABEL if hotkeyd_loaded
                    else "not loaded (build CLT then 'sonari install')"))

    # ~/.local/bin/sonari launcher + PATH.
    launcher = _launcher_path()
    launcher_ok = os.path.exists(launcher)
    on_path = _local_bin_on_path()
    if launcher_ok and on_path:
        detail = launcher
    elif launcher_ok:
        detail = (f"{launcher} present, but ~/.local/bin is NOT on PATH; add: "
                  'export PATH="$HOME/.local/bin:$PATH"')
    else:
        detail = "missing (run 'sonari install')"
    results.append(("sonari launcher", launcher_ok and on_path, detail))
```

Also upgrade the `swiftc` check detail (replace lines 285-287) to name the
installer command:

```python
    swiftc = shutil.which("swiftc")
    results.append(("swiftc", swiftc is not None,
                    swiftc or "not found; install Command Line Tools: "
                              "xcode-select --install"))
```

- [ ] **Step 4: Run the doctor tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_cli_doctor.py -v`
Expected: PASS (all doctor tests).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_doctor.py
git commit -m "feat: doctor checks python3>=3.9, plugin path, both LaunchAgents, launcher+PATH"
```

---

## Task 10: `_dev_install_migrate()` - detect editable footprint, print guidance

**Files:**
- Modify: `src/sonari/cli.py` (flesh out the `_dev_install_migrate` stub from Task 7)
- Test: `tests/test_cli_dev_migrate.py` (create)

`_dev_install_migrate(home=None) -> list` detects whether an importable `sonari`
resolves from OUTSIDE the current plugin (an editable install in another
interpreter's site-packages) and returns human-readable guidance lines. It does
**not** auto `pip uninstall`. It is a safe no-op when no dev footprint exists.
Detection is delegated to a small `_detect_editable_sonari() -> str | None`
helper (returns the foreign interpreter/site path, or None) so tests can stub it.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_cli_dev_migrate.py`:

```python
from unittest import mock

from sonari import cli


def test_dev_migrate_noop_when_no_editable_footprint():
    with mock.patch.object(cli, "_detect_editable_sonari", return_value=None):
        lines = cli._dev_install_migrate()
    assert lines == []


def test_dev_migrate_prints_guidance_when_editable_detected():
    with mock.patch.object(cli, "_detect_editable_sonari",
                           return_value="/opt/homebrew/bin/python3"):
        lines = cli._dev_install_migrate()
    assert len(lines) >= 1
    joined = " ".join(lines)
    # Guidance names the interpreter and the manual uninstall command.
    assert "/opt/homebrew/bin/python3" in joined
    assert "pip uninstall sonari" in joined
    # MUST NOT claim to have auto-uninstalled anything.
    assert "Removed" not in joined


def test_dev_migrate_never_auto_uninstalls(monkeypatch):
    """Even when an editable footprint exists, _dev_install_migrate must not
    shell out to pip - it only returns guidance strings."""
    called = []
    monkeypatch.setattr(cli.subprocess, "call",
                        lambda *a, **k: called.append(a) or 0)
    monkeypatch.setattr(cli.subprocess, "check_output",
                        lambda *a, **k: called.append(a) or "")
    with mock.patch.object(cli, "_detect_editable_sonari",
                           return_value="/some/python3"):
        cli._dev_install_migrate()
    assert called == [], "dev migrate must not run any subprocess"
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_cli_dev_migrate.py -v`
Expected: FAIL - `_detect_editable_sonari` does not exist and the stub
`_dev_install_migrate` ignores it (returns `[]` even when a footprint is detected).

- [ ] **Step 3: Implement detection + flesh out the migrate function**

In `src/sonari/cli.py`, add `_detect_editable_sonari` directly above the
`_dev_install_migrate` stub, and replace the stub body. First the detector:

```python
def _detect_editable_sonari() -> Optional[str]:
    """Return a foreign interpreter path if an editable 'sonari' resolves from
    OUTSIDE this plugin's src, else None.

    We import sonari (which, via PYTHONPATH/conftest, is THIS plugin's source)
    and compare its location to the plugin root. If a DIFFERENT interpreter on
    PATH imports a sonari that lives in its own site-packages (the dev editable
    install), report that interpreter. Best-effort; any failure -> None.
    """
    plugin_src = os.path.realpath(os.path.join(paths.repo_root(), "src"))
    other = shutil.which("python3")
    if not other:
        return None
    try:
        loc = subprocess.check_output(
            [other, "-c",
             "import sonari, os; print(os.path.realpath(sonari.__file__))"],
            stderr=subprocess.DEVNULL, text=True, timeout=5).strip()
    except Exception:  # noqa: BLE001
        return None
    if not loc:
        return None
    # If that interpreter's sonari is NOT inside this plugin's src, it is a
    # foreign (editable/site-packages) install worth cleaning up.
    if os.path.realpath(loc).startswith(plugin_src + os.sep):
        return None
    return other
```

Then replace the `_dev_install_migrate` stub (from Task 7) with:

```python
def _dev_install_migrate(home: Optional[str] = None) -> list:
    """Detect a dev editable 'sonari' footprint and return cleanup GUIDANCE.

    Never auto-uninstalls (uninstalling another interpreter's package is risky).
    Safe no-op when there is no foreign footprint.
    """
    interp = _detect_editable_sonari()
    if not interp:
        return []
    return [
        f"Detected an old editable 'sonari' install in {interp}. "
        f"The plugin's own source now shadows it, so this is cleanup, not a "
        f"blocker. Remove it with: {interp} -m pip uninstall sonari "
        f"(optionally also: --break-system-packages).",
    ]
```

Note: the `home` parameter is kept for signature parity with `_legacy_migrate`
(both are called the same way from `install()`); it is currently unused.

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_cli_dev_migrate.py -v`
Expected: PASS (all three tests).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_dev_migrate.py
git commit -m "feat: _dev_install_migrate() prints editable-sonari cleanup guidance (no auto-uninstall)"
```

---

## Task 11: Missing slash-command files (`voice`, `rate`, `skip`)

**Files:**
- Create: `commands/sonari:voice.md`, `commands/sonari:rate.md`, `commands/sonari:skip.md`
- Modify (tests): `tests/test_commands.py` (extend the file list + add coverage)

The `voice`/`rate`/`skip` CLI subcommands already exist (`_cmd_voice` ->
`SET_VOICE`, `_cmd_rate` -> `SET_RATE` absolute, `_cmd_skip` -> `SKIP`; verified
in `cli.py` lines 148-170 and `_build_parser`). This task only adds the three
missing thin command `.md` files mirroring the existing ones: voice/rate echo
output, skip is silent like repeat/stop.

- [ ] **Step 1: Update the command tests (write new expectations)**

In `tests/test_commands.py`, **replace** `test_all_command_files_exist`
(lines 12-15) with:

```python
def test_all_command_files_exist():
    for name in ("sonari:status.md", "sonari:verbosity.md", "sonari:stop.md",
                 "sonari:repeat.md", "sonari:doctor.md", "sonari:keymap.md",
                 "sonari:voice.md", "sonari:rate.md", "sonari:skip.md"):
        assert os.path.exists(os.path.join(CMD, name)), name
```

Append to `tests/test_commands.py`:

```python
def test_voice_command_runs_voice_and_passes_argument():
    txt = _read("sonari:voice.md")
    assert "sonari voice" in txt
    assert "$ARGUMENTS" in txt or "ARGUMENTS" in txt
    assert "Bash" in txt


def test_rate_command_runs_rate_and_passes_argument():
    txt = _read("sonari:rate.md")
    assert "sonari rate" in txt
    assert "$ARGUMENTS" in txt or "ARGUMENTS" in txt
    assert "Bash" in txt


def test_skip_is_silent():
    txt = _read("sonari:skip.md")
    assert "sonari skip" in txt
    assert "nothing" in txt.lower()
```

Also add CLI subcommand smoke tests (mirroring `tests/test_cli_control.py`
patterns) to lock the subcommands in. Append to `tests/test_cli_control.py`:

```python
def test_rate_subcommand_sends_absolute_set_rate():
    with mock.patch("sonari.client.send", return_value=None) as send:
        rc = cli.main(["rate", "300"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_RATE, "rate": 300}
    assert "delta" not in msg  # absolute, not a delta


def test_voice_subcommand_sends_set_voice():
    with mock.patch("sonari.client.send", return_value=None) as send:
        rc = cli.main(["voice", "Zoe (Premium)"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_VOICE,
                   "voice": "Zoe (Premium)"}


def test_skip_subcommand_sends_skip():
    with mock.patch("sonari.client.send", return_value=None) as send:
        rc = cli.main(["skip"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SKIP}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_commands.py -v`
Expected: FAIL - the three new `.md` files do not exist (the CLI subcommand tests
in `test_cli_control.py` already pass since the subcommands exist; that is fine).

- [ ] **Step 3: Create the three command files**

Create `commands/sonari:voice.md`:

```markdown
---
description: Set the Sonari say voice
argument-hint: <voice name>
---

Run the Sonari voice command using the Bash tool, forwarding the requested voice:

```
sonari voice $ARGUMENTS
```

Print the command's output to the user verbatim. If the command errors, briefly
report the error.
```

Create `commands/sonari:rate.md`:

```markdown
---
description: Set Sonari speech rate in words per minute
argument-hint: <wpm>
---

Run the Sonari rate command using the Bash tool, forwarding the requested
words-per-minute value:

```
sonari rate $ARGUMENTS
```

Print the command's output to the user verbatim. If the command errors, briefly
report the error.
```

Create `commands/sonari:skip.md`:

```markdown
---
description: Skip the current Sonari utterance and move to the next
---

Run the Sonari skip command using the Bash tool:

```
sonari skip
```

This is a silent control action. Print nothing to the user - just run the
command.
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_commands.py tests/test_cli_control.py -v`
Expected: PASS (command-file tests + CLI subcommand tests).

- [ ] **Step 5: Commit**

```bash
git add commands/sonari:voice.md commands/sonari:rate.md commands/sonari:skip.md \
  tests/test_commands.py tests/test_cli_control.py
git commit -m "feat: add sonari:voice/rate/skip slash-command files + cli subcommand tests"
```

---

## Task 12: Docs + version bump + manual smoke checklist + final dual-interpreter gate

**Files:**
- Modify: `README.md` (Requirements + Install + controls table)
- Modify: `pyproject.toml` (`version = "0.3.0"`)
- Modify: `.claude-plugin/plugin.json` (`"version": "0.3.0"`)
- Modify: `.claude-plugin/marketplace.json` (add `"version": "0.3.0"` to the plugin entry)
- Create: `docs/superpowers/phase3-manual-smoke-checklist.md`
- Modify (tests): `tests/test_manifests.py` (assert plugin.json version is 0.3.0)

- [ ] **Step 1: Write the failing version test**

Append to `tests/test_manifests.py`:

```python
def test_plugin_json_version_is_0_3_0():
    data = _load(PLUGIN_JSON)
    assert data.get("version") == "0.3.0"


def test_pyproject_version_is_0_3_0():
    text = (REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8")
    assert 'version = "0.3.0"' in text
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_manifests.py -k version -v`
Expected: FAIL - versions are still `0.1.0`.

- [ ] **Step 3: Bump versions**

In `pyproject.toml`, change line 7:

```toml
version = "0.3.0"
```

In `.claude-plugin/plugin.json`, change line 3:

```json
  "version": "0.3.0",
```

In `.claude-plugin/marketplace.json`, add a `"version"` field to the plugin
entry (after `"name": "sonari",` inside `plugins[0]`):

```json
  "plugins": [
    {
      "name": "sonari",
      "version": "0.3.0",
      "description": "Eyes-free text-to-speech layer for Claude Code (macOS): narrates prose, options, plans, and permissions with a single ordered speech queue and per-type earcons.",
      "author": {
        "name": "Nima Hakimi"
      },
      "category": "accessibility",
      "source": "./"
    }
  ]
```

- [ ] **Step 4: Run the version test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_manifests.py -v`
Expected: PASS (all manifest tests, including the two new version tests; JSON
must remain valid).

- [ ] **Step 5: Rewrite the README Requirements + Install sections**

In `README.md`, **replace** the `## Requirements` section (lines 21-26) with:

```markdown
## Requirements

- macOS (Sonari uses the built-in `say` and `afplay` commands).
- Python 3.9 or newer - macOS ships `/usr/bin/python3`, which is enough. Sonari
  picks the best `python3 >= 3.9` it can find automatically.
- Xcode Command Line Tools for global hotkeys - `xcode-select --install`. (Speech
  works without them; only the hotkeys need `swiftc`.)
- Claude Code 2.1.162 or newer.
- No third-party Python packages at runtime, and no `pip` install. `pytest` is
  only needed to run the tests.
```

**Replace** the `## Install` section (lines 28-59) with:

```markdown
## Install

Sonari is a self-contained Claude Code plugin: it ships its own source and runs
on the macOS system Python with no `pip` install.

1. Enable the `sonari` plugin in Claude Code - either per session with
   `claude --plugin-dir /path/to/sonari`, or register this repo as a local
   plugin marketplace and enable `sonari` from the `/plugin` menu.
2. Run the one-time installer:

```bash
sonari install
```

`sonari install` resolves the best `python3 >= 3.9`, builds the hotkey daemon
locally with `swiftc` (no notarization needed), writes both LaunchAgents with
absolute paths, and places a `~/.local/bin/sonari` launcher so the `sonari`
command works in every shell. If `~/.local/bin` is not on your PATH, the
installer prints the exact line to add.

Verify everything is wired up:

```bash
sonari doctor
```

`doctor` reports each check pass/fail: an enhanced voice, `say`/`afplay`,
`python3 >= 3.9`, the resolved plugin path, the speech and hotkey LaunchAgents,
the daemon socket, the `~/.local/bin/sonari` launcher, and the plugin hooks.
Start a Claude Code session and you should hear a **ready** earcon.

### Development

Contributors can run the test suite from a venv:

```bash
python3 -m venv .venv && .venv/bin/pip install -e .[dev]
.venv/bin/python -m pytest -q
```

The public install path above does **not** use `pip` - the venv is for tests only.
```

In the controls table (lines 82-90), confirm the `voice`/`rate` rows are present
(they already are). Leave the table as-is - the three new slash-command files
(Task 11) bring the documented commands into line with shipped files.

- [ ] **Step 6: Create the manual fresh-install smoke checklist**

Create `docs/superpowers/phase3-manual-smoke-checklist.md`:

```markdown
# Sonari Phase 3 - Fresh-Install Smoke Checklist (self-contained, screen-off)

Run these on the real Mac to validate the **public, pip-free** install path. The
deterministic pytest suite (3.9 + 3.13) covers all install/uninstall/doctor logic
and the plist contents; this checklist covers what cannot be unit-tested: a real
fresh install on the system interpreter, both LaunchAgents loading, and live
speech/earcons/hotkeys. Reuses the structure of
`phase2-manual-smoke-checklist.md`. Do `(screen off)` items with the screen off.

---

## Pre-install (clean slate)

- [ ] **No editable sonari shadows the plugin.** Run
  `/usr/bin/python3 -c "import sonari"` - expect `ModuleNotFoundError` (or, if a
  dev install lingers, note it; `sonari install` will print cleanup guidance).
- [ ] **System python is 3.9+.** Run `/usr/bin/python3 --version` - expect 3.9.6
  or newer.

## Install

- [ ] **Run `sonari install`.** Expect: it prints the resolved interpreter
  (`/usr/bin/python3`), builds hotkeyd, writes both LaunchAgents, writes
  `~/.sonari/install.json`, and places `~/.local/bin/sonari`. No fatal error.
- [ ] **PATH advice (if needed).** If `~/.local/bin` is not on PATH, install
  prints the exact `export PATH=...` line. Add it and open a fresh shell.
- [ ] **Doctor all-ok.** Run `sonari doctor`. Expect every line `[ok ]`,
  including `python3`, `plugin path resolved`, `speechd LaunchAgent loaded`,
  `hotkeyd LaunchAgent loaded`, `sonari launcher`, `swiftc`, `hotkeyd binary`.

## Self-contained verification

- [ ] **CLI runs with no installed package.** In a scrubbed shell:
  `PYTHONPATH=<plugin>/src /usr/bin/python3 -m sonari.cli doctor` - expect it
  runs and the daemon lazy-starts via `bin/sonari-daemon`.
- [ ] **Launcher works in a fresh shell.** Open a new terminal and run
  `sonari status` (resolved via `~/.local/bin/sonari`) - expect daemon status.
- [ ] **speechd plist is correct.** `plutil -p
  ~/Library/LaunchAgents/com.sonari.speechd.plist` - expect ProgramArguments
  `[<abs python3>, -m, sonari.daemon]` and EnvironmentVariables PYTHONPATH =
  `<plugin>/src`.

## Live session (screen off)

- [ ] **Ready earcon + ordered narration.** Start a real `claude` session; hear
  the ready earcon, then prose in order, then decision earcons. (screen off)
- [ ] **All nine hotkeys.** Exercise Ctrl+Cmd+S/R/./D/L/]/[/V/O - each fires,
  no character leak, no beep. (screen off)
- [ ] **Native numeric selection.** Trigger AskUserQuestion, permission, and a
  plan; pick options by digit, Esc cancels. (screen off)

## Spaces-in-path (optional, if a spaced plugin dir is available)

- [ ] Install from a plugin root containing a space; confirm the speechd plist
  PYTHONPATH and the launcher resolve correctly (covered hermetically by the
  XML-escape test, re-verify live if convenient).

## Uninstall

- [ ] **Run `sonari uninstall`.** Expect: both LaunchAgents unloaded/removed, the
  hotkeyd binary removed, `~/.local/bin/sonari` removed, `~/.sonari/install.json`
  removed.
- [ ] **Config + keymap preserved.** Confirm `~/.sonari/config.json` and
  `~/.sonari/keymap.json` still exist after uninstall.

## Sign-off

- [ ] Fresh install on system python works end-to-end with NO pip install.
- [ ] doctor all-ok; both LaunchAgents loaded; launcher present + on PATH.
- [ ] Uninstall removes agents/binary/launcher/install.json and preserves
  config.json + keymap.json.
```

- [ ] **Step 7: FINAL GATE - run the full suite under BOTH interpreters**

Run: `.venv/bin/python -m pytest -q`
Expected: PASS, 0 failures, 0 warnings.
Run: `.venv39/bin/python -m pytest -q`
Expected: PASS, 0 failures, 0 warnings.

If either run shows failures or warnings, fix them before committing (use
superpowers:systematic-debugging). Do not commit a red or warning-laden suite.

- [ ] **Step 8: Commit**

```bash
git add README.md pyproject.toml .claude-plugin/plugin.json \
  .claude-plugin/marketplace.json tests/test_manifests.py \
  docs/superpowers/phase3-manual-smoke-checklist.md
git commit -m "docs: pip-free README + 0.3.0 version bump + phase3 smoke checklist; dual-interp gate green"
```

---

## Self-Review

### Spec coverage (every section maps to a task)

- **§3.1 (plugin ships its own source):** realized by Tasks 3 (shims read `../src`)
  + 7 (PYTHONPATH=<plugin>/src in plist + install.json).
- **§3.2 (`_resolve_python`):** Task 2 (algorithm, dedup, preference, None case).
- **§3.3 (hook/daemon/cli wiring without pip):** Task 3 (all three shims);
  speechd embeds resolved interp via Task 4 + Task 7.
- **§3.4 (plugin-path resolution + persistence):** Task 5 (install.json record) +
  Task 7 (`os.path.realpath(repo_root())`, PYTHONPATH).
- **§3.5 (swift build-on-install, non-fatal):** Task 7 (pre-check + skip hotkeyd
  agent on failure); `_build_hotkeyd` itself is unchanged (Phase 2).
- **§4 install order:** Task 7 implements steps 1-11 in order; voice check + PATH
  advice included.
- **§4 uninstall:** Task 8 (preserve config.json/keymap.json, remove launcher +
  install.json).
- **§4 doctor new checks:** Task 9 (python3, plugin path resolved, both
  LaunchAgents loaded, launcher + PATH; swiftc detail upgraded).
- **§5.1 bin shims:** Task 3 (hook src-first; daemon + cli self-locating; chmod +x;
  daemon-shim executable verification is implicit via repo chmod - see note).
- **§5.2 hooks.json:** no change required (verified) - no task needed.
- **§5.3 cli plist/install:** Task 4 (`_plist env=`, `_xml_escape`,
  `_launchagent_plist` signature) + Task 7 (install call site).
- **§5.4 uninstall + slash-command gap:** Task 8 (uninstall) + Task 11 (voice/
  rate/skip command files; CLI subcommands already exist - verified in cli.py).
- **§5.5 paths INSTALL_RECORD_PATH + install.json:** Task 5.
- **§5.6 launcher:** Task 6 (place/remove/PATH) wired by Task 7/8.
- **§5.7 pyproject:** Task 1 (requires-python) + Task 12 (version 0.3.0).
- **§5.8 future-import sweep:** Task 1.
- **§5.9 README:** Task 12.
- **§6 edge cases:** no swiftc (Task 7 non-fatal + Task 9 detail); no python3
  (Task 7 fatal + Task 9 FAIL); spaces/`&` in path (Task 4 escape test); PATH
  advice (Tasks 7/9); stale global sonari (Task 3 src-first test + Task 10
  guidance); re-run idempotency (Task 7 overwrites); launchd non-GUI (unchanged
  warning).
- **§7 testing strategy:** dual-interpreter gate (Task 1 partial, Task 12 final);
  hermetic install/uninstall/doctor (Tasks 7/8/9); resolution unit tests (Task 2);
  shim tests incl. scrubbed-env subprocess (Task 3); install.json (Task 5);
  launcher (Task 6). The one real-compile swiftc test already exists
  (`tests/test_cli_hotkeyd.py::test_build_hotkeyd_compiles_when_swiftc_present`
  is mocked; the real compile lives in `tests/test_hotkeyd_swift.py`) - unchanged.
- **§8 migration:** Task 10 (`_dev_install_migrate` + `_detect_editable_sonari`),
  called from install() in Task 7; LaunchAgent rewrite is automatic via Task 7.
- **§9 verification list:** covered by Task 12's manual smoke checklist + the
  scrubbed-env subprocess test (Task 3) + plist assertions (Tasks 4/7).

### Note on §5.1 "install verifies the daemon shim is executable"

The spec folds in the deferred item that install chmod +x's the daemon shim. The
shims are chmod +x'd in the repo (Task 3, Step 3). The repo shims are already
executable and tracked, and `_place_launcher` chmods the launcher 0o755. If a
strict "install re-verifies + chmods bin/sonari-daemon" is desired, add to
`install()` after step 8: `os.chmod(_daemon_shim_path(), os.stat(_daemon_shim_path()).st_mode | 0o111)`
guarded by `os.path.exists`. This is optional hardening; the executable bits are
committed in Task 3, so the public clone ships them executable.

### Placeholder scan

No `TODO`/`TBD`/"similar to Task N"/"handle edge cases" placeholders remain.
Every code step shows complete code. The only forward reference (Task 7 calling
`_dev_install_migrate`) is resolved by adding a real no-op stub in Task 7 Step 1
and fleshing it out in Task 10 - both bodies are shown in full.

### Type / name consistency check

- `_resolve_python() -> str | None` (Task 2) - consumed by `install()` (Task 7,
  fatal on None) and `doctor()` (Task 9). Consistent.
- `_probe_python_version(path) -> tuple | None` (Task 2) - used by
  `_resolve_python` and `install()` for the version string. Consistent.
- `_plist(label, program_args, log_path, env=None)` (Task 4) - `_launchagent_plist`
  passes `env={"PYTHONPATH": src_path}`; `_hotkeyd_plist` passes no env (so no
  EnvironmentVariables key). Consistent with the hotkeyd plist test.
- `_launchagent_plist(python_executable, src_path, log_path)` (Task 4) - keyword
  call site in `install()` (Task 7) and tests (Task 4) match exactly.
- `_xml_escape(s) -> str` (Task 4) - used only inside `_plist`. Consistent.
- `paths.INSTALL_RECORD_PATH` (Task 5) - written by `_write_install_record`
  (Task 5), read by `_read_install_record` (Task 9), removed by `uninstall`
  (Task 8). Consistent.
- `_write_install_record(python, python_version, plugin_root, src)` (Task 5) -
  call site in `install()` (Task 7) passes all four by keyword. Consistent.
- `_read_install_record() -> dict | None` (Task 9) - patched in doctor tests.
  Consistent.
- `_place_launcher(plugin_root) -> str`, `_remove_launcher() -> bool`,
  `_launcher_path() -> str`, `_local_bin_on_path() -> bool` (Task 6) - install
  calls `_place_launcher(plugin_root)` + `_local_bin_on_path()` (Task 7);
  uninstall calls `_remove_launcher()` + `_launcher_path()` (Task 8); doctor
  calls `_launcher_path()` + `_local_bin_on_path()` (Task 9). Consistent.
- `_detect_editable_sonari() -> str | None` + `_dev_install_migrate(home=None) ->
  list` (Task 10) - install calls `_dev_install_migrate()` (Task 7). Consistent.
- LaunchAgent constants `LAUNCH_AGENT_LABEL`, `HOTKEYD_LAUNCH_AGENT_LABEL`,
  `LAUNCH_AGENT_PATH`, `HOTKEYD_LAUNCH_AGENT_PATH` - reused unchanged across
  Tasks 4/7/8/9. Consistent.
- MsgType values for new command files (Task 11) match existing handlers:
  `SET_VOICE`, `SET_RATE` (absolute), `SKIP` - verified against cli.py + protocol.py.
- `PROTOCOL_VERSION` stays `1` (no task changes it). Consistent with spec §7.

## Execution Handoff

Plan complete and saved to
`docs/superpowers/plans/2026-06-05-sonari-phase3-packaging.md`.

Two execution options:

1. **Subagent-Driven (recommended)** - dispatch a fresh subagent per task with
   two-stage review between tasks (superpowers:subagent-driven-development).
2. **Inline Execution** - execute tasks in this session with checkpoints
   (superpowers:executing-plans).
```
