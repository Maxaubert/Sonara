# Zero-Prerequisite Python Provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When `/sonara:install` finds no usable Python, provision a uv-managed CPython automatically, so a Python-less Windows machine can install Sonara and hear speech.

**Architecture:** A PowerShell bootstrap (needs no Python) finds-or-provisions Python via `uv`, records the interpreter to two plain-text files, then hands off to `sonara install`. The shims (cmd/bash) and the Python-side resolver fall back to the recorded interpreter when none is on PATH.

**Tech Stack:** Python 3 (stdlib), PowerShell 5+, cmd, bash; `uv` (downloaded binary).

## Global Constraints

- Windows-only. PowerShell + cmd + bash shims must all resolve the same interpreter.
- `uv` is downloaded as a binary from its GitHub releases (no remote-script execution).
- Provision **Python 3.12** (satisfies base >= 3.9 and Kokoro >= 3.10).
- Provisioning is a **fallback**: a usable system Python is always preferred; never provision when one exists.
- The hook path always exits 0 (never fail loudly); the install path may exit non-zero with actionable guidance.
- Interpreter record = two plain-text files: `~/.sonara/python.path` (console `python.exe`) and `~/.sonara/pythonw.path` (windowless `pythonw.exe`).
- Test interpreter on this machine: `C:/Program Files/Python314/python.exe`; run tests as `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest <path> -q`.

---

## File Structure

- **Create** `bin/sonara-bootstrap.ps1` — find-or-provision Python, write the record, run `sonara install`. The only non-Python piece.
- **Modify** `src/sonara/paths.py` — record path constants + `recorded_python()`/`recorded_pythonw()` readers.
- **Modify** `src/sonara/platform/windows/supervisor.py` — `resolve_python_windows()` falls back to the recorded `pythonw` (line 250 `return None`).
- **Modify** `bin/sonara.cmd`, `bin/sonara-hook.cmd`, `bin/sonara` — read the recorded interpreter when none is on PATH. (NOTE: `bin/sonara-hook` is a **Python** entrypoint, not a launcher — do **not** touch it.)
- **Modify** `commands/install.md` — route `/sonara:install` through the bootstrap.
- **Tests** in `tests/test_paths.py` (readers), `tests/test_win_supervisor.py` (resolver fallback), `tests/test_bin_shims.py` (shim + `.ps1` content).

---

### Task 1: Interpreter record readers (paths.py)

**Files:**
- Modify: `src/sonara/paths.py` (after line 16, the `KOKORO_VENV` constant)
- Test: `tests/test_paths.py`

**Interfaces:**
- Produces: `paths.PYTHON_RECORD_PATH: Path`, `paths.PYTHONW_RECORD_PATH: Path`,
  `paths.recorded_python() -> str | None`, `paths.recorded_pythonw() -> str | None`
  (the recorded path iff the file exists and the path is a real file, else `None`).

- [ ] **Step 1: Write the failing tests**

Add to `tests/test_paths.py`:

```python
def test_recorded_python_returns_path_when_file_holds_a_real_file(tmp_path, monkeypatch):
    from sonara import paths
    rec = tmp_path / "python.path"
    rec.write_text(str(tmp_path / "py.exe"), encoding="utf-8")
    (tmp_path / "py.exe").write_text("")          # the recorded path must exist
    monkeypatch.setattr(paths, "PYTHON_RECORD_PATH", rec)
    assert paths.recorded_python() == str(tmp_path / "py.exe")


def test_recorded_python_none_when_missing_file(tmp_path, monkeypatch):
    from sonara import paths
    monkeypatch.setattr(paths, "PYTHON_RECORD_PATH", tmp_path / "nope.path")
    assert paths.recorded_python() is None


def test_recorded_python_none_when_recorded_path_does_not_exist(tmp_path, monkeypatch):
    from sonara import paths
    rec = tmp_path / "python.path"
    rec.write_text(r"C:\does\not\exist\python.exe", encoding="utf-8")
    monkeypatch.setattr(paths, "PYTHON_RECORD_PATH", rec)
    assert paths.recorded_python() is None


def test_recorded_pythonw_reads_its_own_file(tmp_path, monkeypatch):
    from sonara import paths
    rec = tmp_path / "pythonw.path"
    rec.write_text(str(tmp_path / "pyw.exe"), encoding="utf-8")
    (tmp_path / "pyw.exe").write_text("")
    monkeypatch.setattr(paths, "PYTHONW_RECORD_PATH", rec)
    assert paths.recorded_pythonw() == str(tmp_path / "pyw.exe")
```

- [ ] **Step 2: Run to verify they fail**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_paths.py -k recorded -q`
Expected: FAIL with `AttributeError: module 'sonara.paths' has no attribute 'PYTHON_RECORD_PATH'`.

- [ ] **Step 3: Implement**

In `src/sonara/paths.py`, after the `KOKORO_VENV = ...` line (line 16), add:

```python
PYTHON_RECORD_PATH = SONARA_DIR / "python.path"     # recorded console python.exe
PYTHONW_RECORD_PATH = SONARA_DIR / "pythonw.path"   # recorded windowless pythonw.exe


def _read_recorded(record: "Path") -> "str | None":
    """The interpreter path written in *record*, iff it still exists as a file."""
    try:
        path = record.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return path if path and Path(path).is_file() else None


def recorded_python() -> "str | None":
    """The console interpreter the bootstrap recorded (python.exe), or None."""
    return _read_recorded(PYTHON_RECORD_PATH)


def recorded_pythonw() -> "str | None":
    """The windowless interpreter the bootstrap recorded (pythonw.exe), or None."""
    return _read_recorded(PYTHONW_RECORD_PATH)
```

(`Path` is already imported at the top of `paths.py`.)

- [ ] **Step 4: Run to verify they pass**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_paths.py -k recorded -q`
Expected: PASS (4 passed).

- [ ] **Step 5: Commit**

```bash
git add src/sonara/paths.py tests/test_paths.py
git commit -m "feat(paths): recorded_python/recorded_pythonw readers for the interpreter record"
```

---

### Task 2: resolve_python_windows() falls back to the recorded interpreter

**Files:**
- Modify: `src/sonara/platform/windows/supervisor.py:250` (the trailing `return None` of `resolve_python_windows`)
- Test: `tests/test_win_supervisor.py`

**Interfaces:**
- Consumes: `paths.recorded_pythonw()` (Task 1).
- Produces: `resolve_python_windows()` returns the recorded `pythonw` when no system Python is found, else `None` (unchanged otherwise).

- [ ] **Step 1: Write the failing test**

Add to `tests/test_win_supervisor.py`:

```python
def test_resolve_python_windows_falls_back_to_recorded(monkeypatch):
    from sonara.platform.windows import supervisor as sup
    from sonara import paths
    # No system Python anywhere.
    monkeypatch.setattr(sup.shutil, "which", lambda name: None)
    monkeypatch.setattr(paths, "recorded_pythonw", lambda: r"C:\uv\pythonw.exe")
    assert sup.resolve_python_windows() == r"C:\uv\pythonw.exe"


def test_resolve_python_windows_none_when_no_system_and_no_record(monkeypatch):
    from sonara.platform.windows import supervisor as sup
    from sonara import paths
    monkeypatch.setattr(sup.shutil, "which", lambda name: None)
    monkeypatch.setattr(paths, "recorded_pythonw", lambda: None)
    assert sup.resolve_python_windows() is None
```

- [ ] **Step 2: Run to verify it fails**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_win_supervisor.py -k recorded_or_no_system -q` (use `-k "falls_back_to_recorded or none_when_no_system"`)
Expected: the first test FAILS (returns `None` instead of the recorded path).

- [ ] **Step 3: Implement**

In `src/sonara/platform/windows/supervisor.py`, confirm `from sonara import paths` is imported at the top (it is used elsewhere in the module; add it if missing). Change the final line of `resolve_python_windows()` (line 250):

```python
    return None
```

to:

```python
    # No usable system Python -> fall back to the interpreter the bootstrap
    # provisioned + recorded (the zero-Python install path).
    return paths.recorded_pythonw()
```

- [ ] **Step 4: Run to verify it passes**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_win_supervisor.py -k "falls_back_to_recorded or none_when_no_system" -q`
Expected: PASS (2 passed).

- [ ] **Step 5: Commit**

```bash
git add src/sonara/platform/windows/supervisor.py tests/test_win_supervisor.py
git commit -m "feat(win): resolve_python_windows falls back to the recorded interpreter"
```

---

### Task 3: PowerShell bootstrap (bin/sonara-bootstrap.ps1)

**Files:**
- Create: `bin/sonara-bootstrap.ps1`
- Test: `tests/test_bin_shims.py` (content/structure check)

**Interfaces:**
- Produces: a PowerShell entry that ensures a Python, writes `~/.sonara/python.path` +
  `~/.sonara/pythonw.path`, and runs `<python> -m sonara.cli install` with `PYTHONPATH`
  set to the plugin `src`.

- [ ] **Step 1: Pin the uv version**

Find the current latest stable `uv` and use it as `$UvVersion` in the script below
(replace `0.8.4` if a newer stable exists):

Run: `curl -s https://api.github.com/repos/astral-sh/uv/releases/latest | grep '"tag_name"'`
Use the returned version (strip any leading `v`).

- [ ] **Step 2: Write the failing content test**

Add to `tests/test_bin_shims.py`:

```python
def test_bootstrap_ps1_provisions_via_uv_and_hands_off():
    txt = _read("sonara-bootstrap.ps1")
    # downloads the uv BINARY from GitHub releases (no remote-script execution)
    assert "astral-sh/uv/releases/download" in txt
    assert "uv python install 3.12" in txt
    # rejects the Microsoft Store stub when probing system Python
    assert "WindowsApps" in txt
    # writes the interpreter record both consumers read
    assert "python.path" in txt and "pythonw.path" in txt
    # hands off to the real installer
    assert "sonara.cli install" in txt
```

- [ ] **Step 3: Run to verify it fails**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_bin_shims.py -k bootstrap_ps1 -q`
Expected: FAIL with `FileNotFoundError` (the `.ps1` does not exist yet).

- [ ] **Step 4: Create `bin/sonara-bootstrap.ps1`**

```powershell
#Requires -Version 5
<#
  Sonara zero-prerequisite setup.
  Ensures a usable Python (provisioning a uv-managed CPython 3.12 if none is
  found), records the interpreter paths, then runs `sonara install`. PowerShell
  needs no Python, so this breaks the no-Python chicken-and-egg.
#>
$ErrorActionPreference = "Stop"

$SonaraDir  = Join-Path $env:USERPROFILE ".sonara"
$ToolsDir   = Join-Path $SonaraDir "tools"
$PluginRoot = Split-Path -Parent $PSScriptRoot          # ...\bin -> plugin root
$PySrc      = Join-Path $PluginRoot "src"
$UvVersion  = "0.8.4"                                    # pinned (Task 3 Step 1)
New-Item -ItemType Directory -Force -Path $SonaraDir | Out-Null

function Test-RealPython([string]$exe) {
  # True if $exe is a real CPython >= 3.9 (not a Microsoft Store stub).
  try { $real = & $exe -c "import sys; print(sys.executable)" 2>$null } catch { return $false }
  if (-not $real) { return $false }
  if ($real -match "WindowsApps") { return $false }      # Store stub
  try { $ok = & $exe -c "import sys; print(1 if sys.version_info[:2] >= (3,9) else 0)" 2>$null } catch { return $false }
  return ($ok -eq "1")
}

function Find-SystemPython {
  # Returns a console python.exe path, or $null. Prefers the py launcher.
  $cands = @()
  if (Get-Command py -ErrorAction SilentlyContinue) {
    $real = & py -3 -c "import sys; print(sys.executable)" 2>$null
    if ($real) { $cands += $real }
  }
  foreach ($n in @("python","python3")) {
    $c = Get-Command $n -ErrorAction SilentlyContinue
    if ($c) { $cands += $c.Source }
  }
  foreach ($c in $cands) { if (Test-RealPython $c) { return $c } }
  return $null
}

function Get-Uv {
  # Returns the path to uv.exe, downloading it to $ToolsDir if needed.
  $onPath = Get-Command uv -ErrorAction SilentlyContinue
  if ($onPath) { return $onPath.Source }
  $local = Join-Path $ToolsDir "uv.exe"
  if (Test-Path $local) { return $local }
  New-Item -ItemType Directory -Force -Path $ToolsDir | Out-Null
  $zip = Join-Path $ToolsDir "uv.zip"
  $url = "https://github.com/astral-sh/uv/releases/download/$UvVersion/uv-x86_64-pc-windows-msvc.zip"
  Write-Host "Downloading uv $UvVersion..."
  Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
  Expand-Archive -Path $zip -DestinationPath $ToolsDir -Force
  Remove-Item $zip -Force
  if (-not (Test-Path $local)) { throw "uv.exe not found after extracting $url" }
  return $local
}

function Install-UvPython {
  # Installs a uv-managed CPython 3.12 and returns its python.exe path.
  $uv = Get-Uv
  Write-Host "Installing Python 3.12 via uv (this can take a minute)..."
  & $uv python install 3.12
  if ($LASTEXITCODE -ne 0) { throw "uv python install 3.12 failed" }
  $pyexe = & $uv python find 3.12 2>$null
  if (-not $pyexe -or -not (Test-Path $pyexe)) { throw "could not locate the uv-managed Python 3.12" }
  return $pyexe
}

# --- main -----------------------------------------------------------------
$python = Find-SystemPython
if (-not $python) {
  Write-Host "No usable Python found. Provisioning one for Sonara..."
  try {
    $python = Install-UvPython
  } catch {
    Write-Host "Could not provision Python automatically: $_"
    Write-Host "Install Python 3.9+ from https://www.python.org/downloads/windows/ and re-run /sonara:install."
    exit 1
  }
}

# Derive the windowless interpreter (pythonw.exe alongside python.exe).
$pythonw = Join-Path (Split-Path -Parent $python) "pythonw.exe"
if (-not (Test-Path $pythonw)) { $pythonw = $python }

# Record both for the shims + the daemon resolver.
Set-Content -Path (Join-Path $SonaraDir "python.path")  -Value $python  -NoNewline -Encoding ASCII
Set-Content -Path (Join-Path $SonaraDir "pythonw.path") -Value $pythonw -NoNewline -Encoding ASCII

# Hand off to the real installer under that interpreter.
$env:PYTHONPATH = $PySrc + ";" + $env:PYTHONPATH
& $python -m sonara.cli install
exit $LASTEXITCODE
```

- [ ] **Step 5: Run to verify the content test passes**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_bin_shims.py -k bootstrap_ps1 -q`
Expected: PASS.

- [ ] **Step 6: Manual smoke (this machine already has Python, so it takes the system-Python branch)**

Run: `powershell -ExecutionPolicy Bypass -File bin/sonara-bootstrap.ps1`
Expected: it finds the system Python, writes `~/.sonara/python.path` + `pythonw.path`, and runs `sonara install` (which is idempotent). Confirm the two `.path` files now exist and point at a real `python.exe`/`pythonw.exe`:
`cat ~/.sonara/python.path; cat ~/.sonara/pythonw.path`

- [ ] **Step 7: Commit**

```bash
git add bin/sonara-bootstrap.ps1 tests/test_bin_shims.py
git commit -m "feat(install): PowerShell bootstrap that provisions Python via uv when absent"
```

---

### Task 4: Shim fallbacks to the recorded interpreter

**Files:**
- Modify: `bin/sonara.cmd`, `bin/sonara-hook.cmd`, `bin/sonara`
- Test: `tests/test_bin_shims.py` (content checks)

**Interfaces:**
- Consumes: `~/.sonara/python.path` (console) and `~/.sonara/pythonw.path` (windowless),
  written by Task 3.

- [ ] **Step 1: Write the failing content tests**

Add to `tests/test_bin_shims.py`:

```python
def test_sonara_cmd_falls_back_to_recorded_python():
    assert "python.path" in _read("sonara.cmd")

def test_sonara_hook_cmd_falls_back_to_recorded_pythonw():
    assert "pythonw.path" in _read("sonara-hook.cmd")

def test_bin_sonara_bash_falls_back_to_recorded_python():
    assert "python.path" in _read("sonara")
```

- [ ] **Step 2: Run to verify they fail**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_bin_shims.py -k "falls_back" -q`
Expected: FAIL (the shims do not reference the record yet).

- [ ] **Step 3: Rewrite `bin/sonara.cmd`**

```bat
@echo off
rem Windows launcher for the Sonara CLI. Prefers system python.exe; falls back to
rem the interpreter recorded by /sonara:install when none is on PATH (zero-Python).
setlocal enabledelayedexpansion
set "PYTHONPATH=%~dp0..\src;%PYTHONPATH%"
where python >nul 2>nul && ( python -m sonara.cli %* & exit /b )
set "REC=%USERPROFILE%\.sonara\python.path"
if exist "%REC%" (
  set /p PY=<"%REC%"
  "!PY!" -m sonara.cli %*
) else (
  echo No Python found. Run /sonara:install to set up Sonara.
  exit /b 1
)
```

- [ ] **Step 4: Rewrite `bin/sonara-hook.cmd`**

```bat
@echo off
rem Windows launcher for the Sonara plugin hook. Resolves a windowless interpreter
rem (pythonw, else pyw -3, else the recorded pythonw from /sonara:install) and
rem always exits 0 so a hook can never break the Claude session.
setlocal enabledelayedexpansion
set "SONARA_DIR=%USERPROFILE%\.sonara"
set "SONARA_HOOK_LOG=%SONARA_DIR%\hook.log"
if not exist "%SONARA_DIR%\" mkdir "%SONARA_DIR%" >nul 2>nul

where pythonw >nul 2>nul
if !errorlevel!==0 (
  pythonw "%~dp0sonara-hook" %* 2>>"%SONARA_HOOK_LOG%"
  exit /b 0
)
where pyw >nul 2>nul
if !errorlevel!==0 (
  pyw -3 "%~dp0sonara-hook" %* 2>>"%SONARA_HOOK_LOG%"
  exit /b 0
)
if exist "%SONARA_DIR%\pythonw.path" (
  set /p PW=<"%SONARA_DIR%\pythonw.path"
  "!PW!" "%~dp0sonara-hook" %* 2>>"%SONARA_HOOK_LOG%"
)
exit /b 0
```

- [ ] **Step 5: Update `bin/sonara` (bash)**

Replace the interpreter-resolution lines so the body reads:

```bash
#!/usr/bin/env bash
# Self-contained: run the plugin's own src with no installed 'sonara'.
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
export PYTHONPATH="$root/src${PYTHONPATH:+:$PYTHONPATH}"
# Windows (Git Bash): prefer the real `python`; else the interpreter recorded by
# /sonara:install (the zero-Python case).
py="$(command -v python || command -v python3 || true)"
if [ -z "$py" ] && [ -f "$HOME/.sonara/python.path" ]; then
    py="$(cat "$HOME/.sonara/python.path")"
fi
exec "$py" -m sonara.cli "$@"
```

- [ ] **Step 6: Run to verify the content tests pass**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_bin_shims.py -k "falls_back" -q`
Expected: PASS (3 passed).

- [ ] **Step 7: Smoke the existing shims still work**

Run: `bash bin/sonara --help`
Expected: the CLI usage prints (system Python path still works), exit 0.

- [ ] **Step 8: Commit**

```bash
git add bin/sonara.cmd bin/sonara-hook.cmd bin/sonara tests/test_bin_shims.py
git commit -m "feat(shims): fall back to the recorded interpreter when no Python on PATH"
```

---

### Task 5: Route /sonara:install through the bootstrap

**Files:**
- Modify: `commands/install.md`
- Test: `tests/test_bin_shims.py` (content check)

**Interfaces:**
- Consumes: `bin/sonara-bootstrap.ps1` (Task 3).

- [ ] **Step 1: Write the failing content test**

Add to `tests/test_bin_shims.py`:

```python
def test_install_command_routes_through_bootstrap():
    import os
    p = os.path.join(REPO, "commands", "install.md")
    with open(p, encoding="utf-8") as f:
        txt = f.read()
    assert "sonara-bootstrap.ps1" in txt   # provisions Python if absent
    assert "powershell" in txt.lower()
```

- [ ] **Step 2: Run to verify it fails**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_bin_shims.py -k install_command_routes -q`
Expected: FAIL (install.md still calls `bash .../bin/sonara install`).

- [ ] **Step 3: Rewrite the command body in `commands/install.md`**

Replace the fenced command block (the `bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" install` line) with:

````markdown
```
powershell -ExecutionPolicy Bypass -File "${CLAUDE_PLUGIN_ROOT}/bin/sonara-bootstrap.ps1"
```
````

And update the surrounding prose to note it provisions Python if none is present:

```markdown
This is the one-time setup. If you have no Python, it first provisions one (a
uv-managed CPython); then it installs the Windows speech engine (PyWinRT), copies the
runtime to `~/.sonara/app`, registers the background daemon to autostart, wires up the
Claude Code hooks, and sets up the global hotkeys. It can take a couple of minutes the
first time (it may download Python + the speech-engine packages).
```

- [ ] **Step 4: Run to verify it passes**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest tests/test_bin_shims.py -k install_command_routes -q`
Expected: PASS.

- [ ] **Step 5: Full suite sanity**

Run: `PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest -q`
Expected: only the known pre-existing Windows-env failures remain (no NEW failures from this work).

- [ ] **Step 6: Commit**

```bash
git add commands/install.md tests/test_bin_shims.py
git commit -m "feat(install): /sonara:install runs the bootstrap (provisions Python if absent)"
```

---

## Self-Review

**Spec coverage:** mechanism (uv, Task 3) ✓; trigger fallback-only (Find-SystemPython first, Task 3) ✓; bootstrap chain (Task 3) ✓; record as two plain files (Task 1 + Task 3) ✓; resolution order system→recorded→none (Task 2 + Task 4 shims) ✓; commands/install.md routing (Task 5) ✓; error handling loud-not-silent (Task 3 main block) ✓; testing (Tasks 1-5) ✓. YAGNI exclusions respected (Kokoro untouched, no version management).

**Placeholder scan:** the only deferred value is `$UvVersion`, which Task 3 Step 1 sets to a concrete current version before the script is written — not a plan placeholder.

**Type consistency:** `recorded_python`/`recorded_pythonw` names + `PYTHON_RECORD_PATH`/`PYTHONW_RECORD_PATH` constants are used identically in Tasks 1, 2, and the shim `.path` filenames in Tasks 3-4. `resolve_python_windows()` returns a `pythonw` path; the recorded fallback is `recorded_pythonw()` — consistent.
