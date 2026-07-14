# Sonari Kokoro Provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in `sonari voices install` command that provisions a uv-managed Python ≥3.10 venv with the Kokoro extra and repoints the daemon at it, so neural voices work with zero manual `pip` - on this machine and on any new one.

**Architecture:** Neural state is *derived from the existence of `~/.sonari/venv`* (no separate flag to drift). A new `kokoro_provision.py` module owns all provisioning (ensure uv → `uv venv` → install pinned deps → predownload model → health check), with injectable `run`/`which` seams so it is fully unit-testable without touching the network. The daemon's interpreter selector becomes "venv python if it exists and probes ≥3.10, else `resolve_python()` (system 3.9)"; `install()` uses that selector, so the daemon runs `~/.sonari/venv/bin/python -m sonari.daemon` with `PYTHONPATH=APP_DIR` (sonari from APP_DIR, kokoro from the venv).

**Tech Stack:** Python 3.9 (daemon core, stdlib-only) + uv-managed CPython 3.12 (neural venv); argparse CLI; pytest with monkeypatched seams (mirrors `tests/test_cli_install.py` / `tests/_fakeplatform.py`).

## Global Constraints

- **Daemon core stays stdlib-only on system Python 3.9** for base (non-neural) users - no new runtime deps; the venv is opt-in and absent by default.
- **Kokoro pinned set (e2e-verified this session, Python ≥3.10):** `kokoro-onnx==0.5.0`, `onnxruntime==1.27.0`, `numpy==2.4.6` (transitively pulls `espeakng-loader`, `phonemizer-fork`; bundled espeak-ng, no system espeak-ng needed).
- **Never leave a half-wired daemon:** any provisioning failure must abort with an actionable message and leave the previous working interpreter/state intact.
- **Tests must not hit the network or run real uv/venv** - inject `run`/`which`; the real provisioning is verified once by dogfooding on the dev Mac (Task 10), never in CI.
- **macOS is the target now; keep selection/provisioning logic platform-neutral** (venv python path differs on Windows) - Windows wiring is an explicit follow-up.
- Run the suite with `--ignore=tests/test_kokoro.py` when the `[kokoro]` extra is absent (that module imports numpy at collection).

---

### Task 1: venv-python path helper + neural-enabled detection

**Files:**
- Modify: `src/sonari/paths.py` (add `KOKORO_VENV` + `kokoro_venv_python()`)
- Create: `src/sonari/kokoro_provision.py` (start the module with `neural_enabled()`)
- Test: `tests/test_kokoro_provision.py`

**Interfaces:**
- Produces: `paths.KOKORO_VENV: Path` (== `SONARI_DIR / "venv"`); `paths.kokoro_venv_python() -> str` (POSIX `<venv>/bin/python`, Windows `<venv>\Scripts\python.exe`); `kokoro_provision.neural_enabled() -> bool` (the venv python file exists).

- [ ] **Step 1: Write the failing test**

```python
# tests/test_kokoro_provision.py
import os
import sys
from sonari import paths, kokoro_provision as kp


def test_kokoro_venv_python_path_is_platform_correct(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "KOKORO_VENV", tmp_path / "venv")
    p = paths.kokoro_venv_python()
    if sys.platform == "win32":
        assert p.endswith(os.path.join("venv", "Scripts", "python.exe"))
    else:
        assert p.endswith(os.path.join("venv", "bin", "python"))


def test_neural_enabled_reflects_venv_python_existence(monkeypatch, tmp_path):
    venv = tmp_path / "venv"
    monkeypatch.setattr(paths, "KOKORO_VENV", venv)
    assert kp.neural_enabled() is False
    # Create the venv python file.
    pybin = tmp_path / "venv" / ("Scripts" if sys.platform == "win32" else "bin")
    pybin.mkdir(parents=True)
    (pybin / ("python.exe" if sys.platform == "win32" else "python")).write_text("")
    assert kp.neural_enabled() is True
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest tests/test_kokoro_provision.py -v`
Expected: FAIL - `AttributeError: module 'sonari.paths' has no attribute 'KOKORO_VENV'` (and no `kokoro_provision` module).

- [ ] **Step 3: Write minimal implementation**

```python
# src/sonari/paths.py  - add near the other path constants
KOKORO_VENV = SONARI_DIR / "venv"   # opt-in uv-managed venv for neural voices


def kokoro_venv_python() -> str:
    """Absolute path to the neural venv's Python interpreter (may not exist)."""
    import sys
    if sys.platform == "win32":
        return str(KOKORO_VENV / "Scripts" / "python.exe")
    return str(KOKORO_VENV / "bin" / "python")
```

```python
# src/sonari/kokoro_provision.py
"""Provision + wire the opt-in Kokoro neural-voice environment.

Kokoro needs Python >=3.10 (kokoro-onnx requires onnxruntime>=1.20.1 + numpy>=2),
but the daemon defaults to system /usr/bin/python3 (3.9). This module provisions a
uv-managed venv at paths.KOKORO_VENV and the daemon is repointed at it. "Neural
enabled" is derived from the venv's existence - no separate flag to drift.

All subprocess work goes through an injected ``run`` callable so the logic is
unit-testable without touching uv, the network, or a real venv.
"""
from __future__ import annotations

import os

from sonari import paths


def neural_enabled() -> bool:
    """True if the neural venv has been provisioned (its Python exists)."""
    return os.path.exists(paths.kokoro_venv_python())
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 -m pytest tests/test_kokoro_provision.py -v`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/paths.py src/sonari/kokoro_provision.py tests/test_kokoro_provision.py
git commit -m "feat(kokoro): venv path helper + neural_enabled detection"
```

---

### Task 2: neural-aware daemon interpreter selector, wired into install()

**Files:**
- Modify: `src/sonari/cli.py` (add `_daemon_python(sup)`; use it in `install()` at the `sup.resolve_python()` site, cli.py:336)
- Test: `tests/test_cli_install.py` (add cases)

**Interfaces:**
- Consumes: `kokoro_provision.neural_enabled()`, `paths.kokoro_venv_python()`, `sup._probe_python_version()`, `sup.resolve_python()`.
- Produces: `cli._daemon_python(sup) -> str | None` - the venv python when neural is enabled AND it probes ≥3.10, else `sup.resolve_python()`. `install()` uses it so re-running `sonari install` keeps the venv interpreter.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_cli_install.py - add
from sonari import cli, paths
from sonari import kokoro_provision as kp


def test_daemon_python_prefers_venv_when_neural_enabled(monkeypatch):
    class _Sup:
        def resolve_python(self): return "/usr/bin/python3"
        def _probe_python_version(self, p): return (3, 12)
    monkeypatch.setattr(kp, "neural_enabled", lambda: True)
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/venv/bin/python")
    assert cli._daemon_python(_Sup()) == "/venv/bin/python"


def test_daemon_python_falls_back_when_venv_too_old(monkeypatch):
    # A venv that somehow probes <3.10 must NOT be used (defensive).
    class _Sup:
        def resolve_python(self): return "/usr/bin/python3"
        def _probe_python_version(self, p): return (3, 9)
    monkeypatch.setattr(kp, "neural_enabled", lambda: True)
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/venv/bin/python")
    assert cli._daemon_python(_Sup()) == "/usr/bin/python3"


def test_daemon_python_uses_system_when_no_neural(monkeypatch):
    class _Sup:
        def resolve_python(self): return "/usr/bin/python3"
        def _probe_python_version(self, p): return (3, 12)
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    assert cli._daemon_python(_Sup()) == "/usr/bin/python3"


def test_install_uses_venv_interpreter_when_neural_enabled(tmp_path, monkeypatch):
    # install() must hand the venv python to sup.install() when neural is on.
    from tests._fakeplatform import FakeSupervisor, FakeHotkey, FakeTts, fake_platform
    sup = FakeSupervisor(python="/usr/bin/python3")
    pb = fake_platform(supervisor=sup, hotkey=FakeHotkey(ok=True, detail="ok"),
                       tts=FakeTts("Samantha"))
    monkeypatch.setattr(cli, "_platform", lambda: pb)
    monkeypatch.setattr(cli, "_copy_app", lambda root: str(tmp_path / "app"))
    monkeypatch.setattr(cli, "_write_install_record", lambda **k: None)
    monkeypatch.setattr(cli, "_read_plugin_version", lambda root: "0.5.0")
    monkeypatch.setattr("sonari.keymap.write_default_keymap_if_absent", lambda: None)
    monkeypatch.setattr("sonari.keymap.write_resolved", lambda: None)
    monkeypatch.setattr("sonari.paths.ensure_sonari_dir", lambda: None)
    monkeypatch.setattr(kp, "neural_enabled", lambda: True)
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/venv/bin/python")
    cli.install()
    assert ("install", "/venv/bin/python", str(tmp_path / "app")) in sup.calls
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest tests/test_cli_install.py -k daemon_python -v`
Expected: FAIL - `AttributeError: module 'sonari.cli' has no attribute '_daemon_python'`.

- [ ] **Step 3: Write minimal implementation**

```python
# src/sonari/cli.py - add near _resolve_python (cli.py:257)
def _daemon_python(sup):
    """Interpreter the daemon should run on: the neural venv's Python when it is
    provisioned AND probes >=3.10, else the system Python from resolve_python().
    Deriving neural-state from the venv keeps re-runs of `sonari install` on the
    venv interpreter without a separate flag."""
    from sonari import kokoro_provision as kp
    if kp.neural_enabled():
        venv_py = paths.kokoro_venv_python()
        ver = sup._probe_python_version(venv_py)
        if ver is not None and ver >= (3, 10):
            return venv_py
    return sup.resolve_python()
```

Then in `install()` (cli.py:336) replace `python = sup.resolve_python()` with:

```python
    python = _daemon_python(sup)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 -m pytest tests/test_cli_install.py -v`
Expected: PASS (new cases + existing install tests unaffected - base path still uses `resolve_python()` because `neural_enabled()` is False by default).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_install.py
git commit -m "feat(kokoro): neural-aware daemon interpreter selection"
```

---

### Task 3: ensure uv (bootstrap if absent)

**Files:**
- Modify: `src/sonari/kokoro_provision.py`
- Test: `tests/test_kokoro_provision.py`

**Interfaces:**
- Produces: `kokoro_provision.ensure_uv(which=shutil.which, run=subprocess.check_call, base_python=sys.executable) -> str` - returns the absolute path to a `uv` binary, bootstrapping via `pip install --user uv` when none is on PATH; raises `RuntimeError` (actionable) if it still can't be found.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_kokoro_provision.py - add
import pytest
from sonari import kokoro_provision as kp


def test_ensure_uv_returns_path_when_already_present():
    got = kp.ensure_uv(which=lambda name: "/usr/local/bin/uv",
                       run=lambda *a, **k: pytest.fail("must not bootstrap"))
    assert got == "/usr/local/bin/uv"


def test_ensure_uv_bootstraps_via_pip_when_absent(tmp_path):
    calls = []
    userbase = tmp_path
    bindir = userbase / "bin"
    bindir.mkdir(parents=True)
    (bindir / "uv").write_text("")  # pip install lands uv here

    def fake_run(cmd, **k):
        calls.append(cmd)

    got = kp.ensure_uv(
        which=lambda name: None,                     # not on PATH
        run=fake_run,
        base_python="/usr/bin/python3",
        user_base=lambda py: str(userbase),
    )
    assert got == str(bindir / "uv")
    assert any("pip" in c and "uv" in c for c in calls)  # bootstrap ran


def test_ensure_uv_raises_actionable_when_unfindable(tmp_path):
    with pytest.raises(RuntimeError) as ei:
        kp.ensure_uv(which=lambda name: None, run=lambda *a, **k: None,
                     base_python="/usr/bin/python3",
                     user_base=lambda py: str(tmp_path))  # no uv ever appears
    assert "uv" in str(ei.value).lower()
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest tests/test_kokoro_provision.py -k ensure_uv -v`
Expected: FAIL - `AttributeError: ... has no attribute 'ensure_uv'`.

- [ ] **Step 3: Write minimal implementation**

```python
# src/sonari/kokoro_provision.py - add
import shutil
import subprocess
import sys


def _default_user_base(py: str) -> str:
    return subprocess.check_output(
        [py, "-c", "import site; print(site.getuserbase())"],
        text=True).strip()


def ensure_uv(which=shutil.which, run=subprocess.check_call,
              base_python=None, user_base=_default_user_base) -> str:
    """Return a path to `uv`, bootstrapping it via `pip install --user uv` when
    it is not already on PATH. Raises RuntimeError (actionable) if uv cannot be
    obtained - never returns a non-existent path."""
    found = which("uv")
    if found:
        return found
    py = base_python or sys.executable
    run([py, "-m", "pip", "install", "--user", "--quiet", "uv"])
    cand = os.path.join(user_base(py), "bin", "uv")
    if os.path.exists(cand):
        return cand
    found = which("uv")
    if found:
        return found
    raise RuntimeError(
        "Could not install or locate `uv`, needed to provision neural voices. "
        "Install uv (https://docs.astral.sh/uv/) and re-run: sonari voices install")
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 -m pytest tests/test_kokoro_provision.py -k ensure_uv -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/kokoro_provision.py tests/test_kokoro_provision.py
git commit -m "feat(kokoro): ensure_uv bootstrap"
```

---

### Task 4: pinned requirements file + provision the venv

**Files:**
- Create: `src/sonari/requirements-kokoro.txt` (ships with the package copy into APP_DIR)
- Modify: `src/sonari/kokoro_provision.py`
- Test: `tests/test_kokoro_provision.py`

**Interfaces:**
- Produces: `kokoro_provision.requirements_path() -> str` (the bundled pin file beside this module); `kokoro_provision.provision(uv, run=subprocess.check_call) -> None` - runs `uv venv <KOKORO_VENV> --python 3.12` then `uv pip install --python <venv-python> -r <requirements>`.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_kokoro_provision.py - add
from sonari import paths
from sonari import kokoro_provision as kp


def test_requirements_file_pins_verified_versions():
    text = open(kp.requirements_path()).read()
    assert "kokoro-onnx==0.5.0" in text
    assert "onnxruntime==1.27.0" in text
    assert "numpy==2.4.6" in text


def test_provision_runs_uv_venv_then_pip_install(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "KOKORO_VENV", tmp_path / "venv")
    monkeypatch.setattr(paths, "kokoro_venv_python",
                        lambda: str(tmp_path / "venv" / "bin" / "python"))
    cmds = []
    kp.provision("/bin/uv", run=lambda cmd, **k: cmds.append(cmd))
    assert cmds[0] == ["/bin/uv", "venv", str(tmp_path / "venv"), "--python", "3.12"]
    assert cmds[1][:4] == ["/bin/uv", "pip", "install", "--python"]
    assert "-r" in cmds[1] and kp.requirements_path() in cmds[1]
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest tests/test_kokoro_provision.py -k "requirements or provision" -v`
Expected: FAIL - missing `requirements_path`/`provision` (and the file).

- [ ] **Step 3: Write minimal implementation**

```
# src/sonari/requirements-kokoro.txt
# Pinned, e2e-verified Kokoro neural-voice stack (Python >=3.10). See
# docs/superpowers/specs/2026-06-18-sonari-kokoro-provisioning-design.md.
kokoro-onnx==0.5.0
onnxruntime==1.27.0
numpy==2.4.6
```

```python
# src/sonari/kokoro_provision.py - add
def requirements_path() -> str:
    """Absolute path to the bundled pinned Kokoro requirements file."""
    return os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "requirements-kokoro.txt")


def provision(uv: str, run=subprocess.check_call) -> None:
    """Create the uv-managed venv (downloading CPython 3.12 if absent) and install
    the pinned Kokoro stack into it. Raises subprocess.CalledProcessError on failure
    (the caller aborts without rewiring the daemon)."""
    venv_dir = str(paths.KOKORO_VENV)
    run([uv, "venv", venv_dir, "--python", "3.12"])
    run([uv, "pip", "install", "--python", paths.kokoro_venv_python(),
         "-r", requirements_path()])
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 -m pytest tests/test_kokoro_provision.py -k "requirements or provision" -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/requirements-kokoro.txt src/sonari/kokoro_provision.py tests/test_kokoro_provision.py
git commit -m "feat(kokoro): pinned requirements + uv provision step"
```

---

### Task 5: model pre-download + health check

**Files:**
- Modify: `src/sonari/kokoro_provision.py`
- Test: `tests/test_kokoro_provision.py`

**Interfaces:**
- Produces:
  - `kokoro_provision.predownload_model(app_dir, run=subprocess.check_call) -> None` - runs the venv python with `PYTHONPATH=app_dir` to build `KokoroEngine(SONARI_DIR/"kokoro")` and trigger `_ensure_loaded()` once.
  - `kokoro_provision.neural_healthy(app_dir, run=subprocess.check_output) -> bool` - runs the venv python to confirm `kokoro.is_installed()` is True there.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_kokoro_provision.py - add
from sonari import paths
from sonari import kokoro_provision as kp


def test_predownload_invokes_venv_python_with_pythonpath(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/venv/bin/python")
    seen = {}
    def fake_run(cmd, env=None, **k):
        seen["cmd"], seen["env"] = cmd, env
    kp.predownload_model("/app", run=fake_run)
    assert seen["cmd"][0] == "/venv/bin/python"
    assert seen["env"]["PYTHONPATH"] == "/app"
    assert "KokoroEngine" in seen["cmd"][-1]   # the -c body builds the engine


def test_neural_healthy_true_when_venv_reports_installed(monkeypatch):
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/venv/bin/python")
    assert kp.neural_healthy("/app", run=lambda *a, **k: "True\n") is True
    assert kp.neural_healthy("/app", run=lambda *a, **k: "False\n") is False


def test_neural_healthy_false_on_subprocess_error(monkeypatch):
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/venv/bin/python")
    def boom(*a, **k): raise OSError("no python")
    assert kp.neural_healthy("/app", run=boom) is False
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest tests/test_kokoro_provision.py -k "predownload or healthy" -v`
Expected: FAIL - missing `predownload_model`/`neural_healthy`.

- [ ] **Step 3: Write minimal implementation**

```python
# src/sonari/kokoro_provision.py - add
_PREDOWNLOAD = (
    "from sonari import kokoro, paths as p; "
    "kokoro.KokoroEngine(p.SONARI_DIR / 'kokoro')._ensure_loaded()")

_HEALTH = "from sonari import kokoro; print(kokoro.is_installed())"


def predownload_model(app_dir: str, run=subprocess.check_call) -> None:
    """Trigger the one-time ~316 MB model download via the venv python, so the
    first real utterance does not stall for minutes."""
    env = dict(os.environ, PYTHONPATH=app_dir)
    run([paths.kokoro_venv_python(), "-c", _PREDOWNLOAD], env=env)


def neural_healthy(app_dir: str, run=subprocess.check_output) -> bool:
    """True if the venv python can import the Kokoro extra (kokoro.is_installed())."""
    env = dict(os.environ, PYTHONPATH=app_dir)
    try:
        out = run([paths.kokoro_venv_python(), "-c", _HEALTH], env=env, text=True)
    except Exception:  # noqa: BLE001 - any failure means "not healthy"
        return False
    return out.strip() == "True"
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 -m pytest tests/test_kokoro_provision.py -k "predownload or healthy" -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/kokoro_provision.py tests/test_kokoro_provision.py
git commit -m "feat(kokoro): model pre-download + venv health check"
```

---

### Task 6: install/uninstall orchestrators

**Files:**
- Modify: `src/sonari/kokoro_provision.py`
- Test: `tests/test_kokoro_provision.py`

**Interfaces:**
- Produces:
  - `kokoro_provision.install_kokoro(app_dir, *, ensure_uv=ensure_uv, provision=provision, predownload_model=predownload_model) -> None` - orchestrates ensure_uv → provision → predownload, in order; lets exceptions propagate (the CLI turns them into an actionable message and does NOT rewire the daemon).
  - `kokoro_provision.uninstall_kokoro(rmtree=shutil.rmtree) -> None` - removes `paths.KOKORO_VENV` (idempotent; ignores absence).

- [ ] **Step 1: Write the failing test**

```python
# tests/test_kokoro_provision.py - add
import pytest
from sonari import paths
from sonari import kokoro_provision as kp


def test_install_kokoro_runs_steps_in_order():
    order = []
    kp.install_kokoro(
        "/app",
        ensure_uv=lambda **k: order.append("uv") or "/bin/uv",
        provision=lambda uv, **k: order.append(("provision", uv)),
        predownload_model=lambda app, **k: order.append(("model", app)),
    )
    assert order == ["uv", ("provision", "/bin/uv"), ("model", "/app")]


def test_install_kokoro_aborts_if_provision_fails():
    def boom(uv, **k): raise RuntimeError("uv venv failed")
    with pytest.raises(RuntimeError):
        kp.install_kokoro(
            "/app",
            ensure_uv=lambda **k: "/bin/uv",
            provision=boom,
            predownload_model=lambda app, **k: pytest.fail("must not predownload"),
        )


def test_uninstall_kokoro_removes_venv_idempotently(monkeypatch, tmp_path):
    venv = tmp_path / "venv"; venv.mkdir()
    monkeypatch.setattr(paths, "KOKORO_VENV", venv)
    kp.uninstall_kokoro()
    assert not venv.exists()
    kp.uninstall_kokoro()  # second call must not raise
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest tests/test_kokoro_provision.py -k "install_kokoro or uninstall_kokoro" -v`
Expected: FAIL - missing orchestrators.

- [ ] **Step 3: Write minimal implementation**

```python
# src/sonari/kokoro_provision.py - add (ensure `import shutil` present)
def install_kokoro(app_dir, *, ensure_uv=ensure_uv, provision=provision,
                   predownload_model=predownload_model) -> None:
    """Provision the neural venv end-to-end. Any step raising aborts the whole
    operation (the caller reports it and leaves the daemon on its current
    interpreter)."""
    uv = ensure_uv()
    provision(uv)
    predownload_model(app_dir)


def uninstall_kokoro(rmtree=shutil.rmtree) -> None:
    """Remove the neural venv (idempotent). The daemon reverts to system Python on
    the next install/wiring because neural_enabled() then returns False."""
    if os.path.isdir(str(paths.KOKORO_VENV)):
        rmtree(str(paths.KOKORO_VENV))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 -m pytest tests/test_kokoro_provision.py -v`
Expected: PASS (whole module).

- [ ] **Step 5: Commit**

```bash
git add src/sonari/kokoro_provision.py tests/test_kokoro_provision.py
git commit -m "feat(kokoro): install/uninstall orchestrators"
```

---

### Task 7: `voices install` / `voices uninstall` CLI commands

**Files:**
- Modify: `src/sonari/cli.py` (`_cmd_voices_install`, `_cmd_voices_uninstall`, register in `_register_local`)
- Test: `tests/test_cli_voices.py`

**Interfaces:**
- Consumes: `kokoro_provision.install_kokoro/uninstall_kokoro/neural_healthy`, `install()`, `_platform().supervisor`, `paths.APP_DIR`.
- Produces: `cli._cmd_voices_install(args) -> int`, `cli._cmd_voices_uninstall(args) -> int`; a `voices` subparser with `install`/`uninstall` sub-subcommands.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_cli_voices.py
import pytest
from sonari import cli, paths
from sonari import kokoro_provision as kp


def test_voices_install_provisions_then_rewires_daemon(monkeypatch, tmp_path):
    order = []
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(kp, "install_kokoro", lambda app_dir: order.append(("provision", app_dir)))
    monkeypatch.setattr(kp, "neural_healthy", lambda app_dir: True)
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    rc = cli._cmd_voices_install(object())
    assert rc == 0
    assert order == [("provision", str(tmp_path / "app")), "install"]


def test_voices_install_reports_failure_without_rewiring(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    def boom(app_dir): raise RuntimeError("uv missing")
    monkeypatch.setattr(kp, "install_kokoro", boom)
    monkeypatch.setattr(cli, "install", lambda: pytest.fail("must not rewire on failure"))
    rc = cli._cmd_voices_install(object())
    assert rc == 1


def test_voices_uninstall_removes_and_reverts(monkeypatch):
    order = []
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: order.append("rm"))
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    rc = cli._cmd_voices_uninstall(object())
    assert rc == 0
    assert order == ["rm", "install"]   # remove venv, then re-wire to system 3.9


def test_voices_subcommand_registered():
    parser = cli._build_parser()
    args = parser.parse_args(["voices", "install"])
    assert args.func is cli._cmd_voices_install
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest tests/test_cli_voices.py -v`
Expected: FAIL - missing `_cmd_voices_install` etc.

- [ ] **Step 3: Write minimal implementation**

```python
# src/sonari/cli.py - add command handlers
def _cmd_voices_install(_args) -> int:
    """Provision the Kokoro neural-voice venv, then re-wire the daemon onto it."""
    from sonari import kokoro_provision as kp
    paths.ensure_sonari_dir()
    app_dir = str(paths.APP_DIR)
    print("Provisioning neural voices (uv + Kokoro, one-time ~316 MB download)…")
    try:
        kp.install_kokoro(app_dir)
    except Exception as exc:  # noqa: BLE001 - report, do not half-wire
        print(f"Neural-voice setup failed: {exc}", file=sys.stderr)
        return 1
    rc = install()  # re-wires the daemon onto the venv python (neural_enabled() now True)
    if rc == 0 and kp.neural_healthy(app_dir):
        print("Neural voices ready. Pick one with: sonari voice af_heart")
    return rc


def _cmd_voices_uninstall(_args) -> int:
    """Remove the neural venv and revert the daemon to system Python."""
    from sonari import kokoro_provision as kp
    kp.uninstall_kokoro()
    rc = install()  # neural_enabled() now False -> reverts to resolve_python()
    print("Neural voices removed; reverted to the system voice.")
    return rc
```

```python
# src/sonari/cli.py - in _register_local(sub), add:
    vp = sub.add_parser("voices", help="install/remove neural (Kokoro) voices")
    vsub = vp.add_subparsers(dest="voices_command")
    vsub.add_parser("install", help="provision neural voices").set_defaults(
        func=_cmd_voices_install)
    vsub.add_parser("uninstall", help="remove neural voices").set_defaults(
        func=_cmd_voices_uninstall)
    vp.set_defaults(func=lambda _a: (vp.print_help() or 2))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 -m pytest tests/test_cli_voices.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_voices.py
git commit -m "feat(kokoro): sonari voices install/uninstall commands"
```

---

### Task 8: doctor neural-voices row

**Files:**
- Modify: `src/sonari/cli.py` (`doctor()`)
- Test: `tests/test_cli_doctor.py`

**Interfaces:**
- Consumes: `kokoro_provision.neural_enabled()`, `paths.APP_DIR`, `kokoro_provision.neural_healthy()`.
- Produces: an extra `("neural voices", ok, detail)` row - absent-venv → `(… , True, "not installed (optional)")` so a base install stays all-green; venv present + healthy → `(…, True, "ready (<venv python>)")`; venv present + unhealthy → `(…, False, "venv present but Kokoro import failed - re-run: sonari voices install")`.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_cli_doctor.py - add
from sonari import cli, paths
from sonari import kokoro_provision as kp
from tests._fakeplatform import fake_platform, FakeSupervisor, FakeHotkey


def _doctor_rows(monkeypatch):
    pb = fake_platform(supervisor=FakeSupervisor(), hotkey=FakeHotkey(ok=True, detail="ok"))
    monkeypatch.setattr(cli, "_platform", lambda: pb)
    return {name: (ok, detail) for name, ok, detail in cli.doctor()}


def test_doctor_neural_row_ok_and_green_when_absent(monkeypatch):
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    rows = _doctor_rows(monkeypatch)
    assert "neural voices" in rows
    ok, detail = rows["neural voices"]
    assert ok is True and "not installed" in detail


def test_doctor_neural_row_fails_when_venv_unhealthy(monkeypatch):
    monkeypatch.setattr(kp, "neural_enabled", lambda: True)
    monkeypatch.setattr(kp, "neural_healthy", lambda app: False)
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["neural voices"]
    assert ok is False and "voices install" in detail
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m pytest tests/test_cli_doctor.py -k neural -v`
Expected: FAIL - no "neural voices" row.

- [ ] **Step 3: Write minimal implementation**

```python
# src/sonari/cli.py - in doctor(), before `return results`:
    try:
        from sonari import kokoro_provision as kp
        if not kp.neural_enabled():
            results.append(("neural voices", True, "not installed (optional)"))
        elif kp.neural_healthy(str(paths.APP_DIR)):
            results.append(("neural voices", True,
                            f"ready ({paths.kokoro_venv_python()})"))
        else:
            results.append(("neural voices", False,
                            "venv present but Kokoro import failed - "
                            "re-run: sonari voices install"))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("neural voices", False, f"error: {exc}"))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 -m pytest tests/test_cli_doctor.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_doctor.py
git commit -m "feat(kokoro): doctor neural-voices row"
```

---

### Task 9: `/sonari:voices` slash command

**Files:**
- Create: `commands/voices.md`
- Test: `tests/test_manifests.py` (only if it enumerates command files - otherwise none)

**Interfaces:** none (markdown command file mirroring `commands/voice.md`).

- [ ] **Step 1: Write the failing test (or confirm coverage)**

Run: `python3 -m pytest tests/test_manifests.py -v`
If `test_manifests.py` validates every `commands/*.md` (frontmatter/format), this new file is covered - confirm it still passes after Step 3. If it does NOT enumerate commands, skip the test step (a static doc file needs no unit test) and note that here.

- [ ] **Step 2: Create the command file**

```markdown
---
description: Install or remove Sonari neural (Kokoro) voices
argument-hint: install | uninstall
---

Run the Sonari voices command with the Bash tool, forwarding the requested
action (`install` or `uninstall`):

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonari" voices $ARGUMENTS
```

`install` provisions a uv-managed Python environment with the Kokoro neural-voice
engine (a one-time ~316 MB download) and repoints the daemon at it; `uninstall`
removes it and reverts to the system voice. Print the command's output verbatim.
If it succeeded, tell the user to pick a neural voice with /sonari:voice af_heart.
If it errors, report the error briefly.
```

- [ ] **Step 3: Run manifest tests**

Run: `python3 -m pytest tests/test_manifests.py -v`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add commands/voices.md
git commit -m "feat(kokoro): /sonari:voices slash command"
```

---

### Task 10: Dogfood - real provisioning + neural playback on the dev Mac (manual)

**Files:** none (verification only).

This is the real end-to-end verification the unit tests deliberately cannot do (network + heavy + audio). Run it on the dev Mac after Tasks 1–9 merge.

- [ ] **Step 1: Run the full mocked suite (must be green first)**

Run: `python3 -m pytest -q --ignore=tests/test_kokoro.py`
Expected: all pass (no regressions).

- [ ] **Step 2: Provision for real**

Run: `./bin/sonari voices install`
Expected: uv bootstrap/venv, pinned deps install, model downloads once, daemon restarts on the venv python. No errors.

- [ ] **Step 3: Verify the daemon is on the venv interpreter + healthy**

Run: `./bin/sonari doctor`
Expected: `[ok ] neural voices: ready (…/.sonari/venv/bin/python)`; daemon socket reachable.
Confirm the daemon process runs `~/.sonari/venv/bin/python -m sonari.daemon` (e.g. `pgrep -fl sonari.daemon`).

- [ ] **Step 4: Hear a neural voice**

Run: `./bin/sonari voice af_heart` then trigger speech (status line / a short prompt in a Claude session). Confirm the neural voice is audible (distinct from Samantha).

- [ ] **Step 5: Verify revert**

Run: `./bin/sonari voices uninstall` then `./bin/sonari doctor`.
Expected: neural row back to "not installed (optional)"; daemon reverts to system 3.9; base speech still works. Re-run `voices install` to leave neural enabled (Nima asked for it on this machine).

---

## Self-Review

**Spec coverage:** command surface (T7, T9), uv provisioning (T3–T4), in-process via venv interpreter (T2), model pre-download (T5), verify/doctor (T5, T8), uninstall/revert (T6–T7), pinned reproducible deps (T4), error-without-half-wire (T6–T7), dogfood (T10). Windows + claude-everywhere remain explicit follow-ups (not tasks). ✔ all spec sections map to a task.

**Placeholder scan:** no TBD/TODO; every code step has real code; T9's test step is conditional with an explicit "skip if not enumerated" instruction (not a placeholder). ✔

**Type consistency:** `neural_enabled()`, `kokoro_venv_python()`, `ensure_uv()`, `provision(uv, run=)`, `predownload_model(app_dir, run=)`, `neural_healthy(app_dir, run=)`, `install_kokoro(app_dir, *, …)`, `uninstall_kokoro(rmtree=)`, `_daemon_python(sup)`, `_cmd_voices_install/uninstall` - names/signatures used consistently across tasks. ✔
