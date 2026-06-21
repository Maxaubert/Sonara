# Zero-Prerequisite Python Provisioning — Design

**Status:** approved (design), pending spec review
**Date:** 2026-06-21
**Scope:** Windows-only (Sonara is Windows-only)

## Goal

Make Sonara installable with **zero prerequisites**. Today a user must already have a
real Python >= 3.9 on PATH; with no Python the hooks silently no-op and the CLI fails
outright. After this change, `/sonara:install` provisions a Python automatically when none
is found, so a fresh Windows machine (Python-less) can install Sonara and hear speech.

This is a **fallback**: when a usable system Python already exists, behavior is unchanged
(no download). Provisioning happens only when needed.

## Decisions (from brainstorming)

- **Mechanism:** a `uv`-managed standalone CPython. `uv` is obtained without Python (its
  standalone binary), then `uv python install 3.12` fetches a full, pip-capable CPython.
  3.12 satisfies both the base daemon (>= 3.9) and Kokoro (>= 3.10).
- **Trigger:** only when no usable system Python is found.
- **Bootstrap language:** PowerShell (always present on Windows; needs no Python).
- **uv acquisition:** download the pinned `uv` release binary from its GitHub releases
  (preferred over `irm https://astral.sh/uv/install.ps1 | iex` so we never execute a
  remote script, and the version is reproducible).

## The chicken-and-egg, and how the bootstrap breaks it

`bin/sonara.cmd` runs `python -m sonara.cli`; `bin/sonara-hook.cmd` resolves
`pythonw`/`pyw -3`. With **no** Python, no Sonara Python code can run, so provisioning
cannot itself be Python. The fix: a **PowerShell** entry that runs *before* any Python.

```
/sonara:install
   -> powershell -ExecutionPolicy Bypass -File bin/sonara-bootstrap.ps1
        1. resolve a usable Python (real python.exe >= 3.9, reject Store stubs)
        2. if none -> provision:
             a. ensure uv (download pinned uv.exe to ~/.sonara/tools if absent)
             b. uv python install 3.12
             c. resolve the installed interpreter's python.exe + pythonw.exe
        3. record the chosen interpreter (see "Interpreter record")
        4. hand off: <python.exe> -m sonara.cli install
```

`sonara install` is the step we already hardened (copies runtime, pip-installs PyWinRT,
registers autostart + hooks + hotkeys). It runs **under the resolved interpreter**, so its
pip step and the daemon it registers both use that Python.

## Interpreter record (refinement over the design's "python.json")

The shims are cmd/bash and cannot parse JSON cleanly. So the record is **two plain-text
files**, each one line, written by the bootstrap:

- `~/.sonara/python.path`  — the console interpreter (`python.exe`)
- `~/.sonara/pythonw.path` — the windowless interpreter (`pythonw.exe`)

Plain text means trivial consumption: cmd `set /p P=<"%SONARA_DIR%\python.path"`, bash
`py="$(cat ~/.sonara/python.path)"`. The bootstrap writes the record after it resolves an
interpreter (system or provisioned). It is the authoritative fallback for consumers when
no system Python is on PATH; for a user who already has a system Python and runs `sonara
install` directly (bypassing the bootstrap), no record is written and consumers simply use
PATH — that path is unchanged. The richer install metadata (version, plugin version)
already lives in `install.json`; no new JSON record is needed.

## Interpreter resolution order (single, shared model)

Every consumer resolves the interpreter in the same order:

1. **System Python** — a real `python.exe`/`pythonw.exe` >= 3.9 on PATH, rejecting the
   Microsoft Store stub (the existing `resolve_python_windows` logic, unchanged).
2. **Recorded interpreter** — the path in `~/.sonara/{python,pythonw}.path`, if the file
   exists and the path is executable.
3. **None** — caller reports "no Python; run /sonara:install" (which provisions one).

Because the bootstrap writes the record *before* handing off to `sonara install`, by the
time install runs the record exists, so resolution succeeds end to end.

## Components (each isolated and testable)

- **`bin/sonara-bootstrap.ps1`** (new) — PowerShell. Find-or-provision Python, write the
  record, hand off to `sonara install`. The only non-Python piece. ~80 lines.
- **`recorded_python()` / `recorded_pythonw()`** (new, in `paths.py` — they are
  `SONARA_DIR` file accessors) — read the `.path` files, return the path iff it exists and
  is executable, else `None`. `resolve_python_windows()` (supervisor.py) calls them.
- **`resolve_python_windows()` / `_daemon_python()`** (modified) — append step 2 (the
  recorded interpreter) as a fallback after the system-Python search.
- **Shim fallbacks** (modified) — `bin/sonara.cmd`, `bin/sonara-hook.cmd`, `bin/sonara`,
  `bin/sonara-hook`: prefer system `python`/`pythonw`; else read the recorded `.path`.
  ~3 lines each.
- **`commands/install.md`** (modified) — invoke the PowerShell bootstrap instead of
  `bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" install`. The direct CLI `sonara install`
  remains for users who already have Python.

## Data flow — fresh, Python-less install

1. User: `/plugin marketplace add ...`, `/plugin install sonara@sonara`.
2. The plugin's hooks fire on the next session. With no Python they no-op (silent, exit 0)
   — unchanged, and acceptable because step 3 is the required setup.
3. User: `/sonara:install` -> `bin/sonara-bootstrap.ps1`.
4. No system Python found -> download `uv.exe` (pinned) -> `uv python install 3.12` ->
   write `~/.sonara/python.path` + `pythonw.path`.
5. `<python.exe> -m sonara.cli install` -> copies runtime, `pip install` PyWinRT into the
   provisioned Python, registers the Task Scheduler task (pointing at `pythonw.path`),
   wires hooks, installs hotkeys.
6. Next session: the daemon autostarts under the provisioned `pythonw`; hooks resolve it
   via the record; speech works.

## Error handling

- **Idempotent.** Re-running the bootstrap with a usable Python present (system or
  recorded) is a no-op for provisioning.
- **No network / uv download fails / `uv python install` fails:** the bootstrap prints the
  exact manual remedy (install Python from python.org, or install uv) and exits non-zero.
  It never reports success silently.
- **Bootstrap must not corrupt a working install:** it only writes the record and hands
  off; it never deletes an existing system Python or a prior record unless it has a valid
  replacement.

## Testing

- **Python unit tests** (pytest, run on this Windows box):
  - `recorded_python()`/`recorded_pythonw()`: returns the path when the `.path` file holds
    an executable path; `None` when missing or the path doesn't exist.
  - `resolve_python_windows()` fallback: with no system Python (mocked) and a valid record,
    returns the recorded interpreter; with neither, returns `None`.
- **Content/structure tests** (same pattern as `test_bin_shims.py`):
  - `bin/sonara-bootstrap.ps1` references the pinned uv URL, `uv python install`, writes
    both `.path` files, and ends by invoking `sonara.cli install`.
  - The shims read `~/.sonara/{python,pythonw}.path` as a fallback after the system probe.
- **Manual live verification** (this machine): dry-run `uv python install 3.12` and confirm
  the resolved interpreter path; the full no-Python path is verified on a clean VM later
  (out of scope for this change's automated tests).

## Out of scope (YAGNI)

- Not switching Kokoro off its existing pip-based uv bootstrap (separate concern; the
  provisioned 3.12 could later serve Kokoro directly, but not now).
- Not managing multiple Python versions or upgrades.
- Not provisioning when a usable system Python already exists.
- Not making the hooks themselves provision Python (too slow/heavy for a hook; provisioning
  is the install step's job).

## Global constraints

- Windows-only. PowerShell + cmd + bash shims must all resolve the same interpreter.
- `uv` is pinned to a specific release version (reproducible, no remote-script execution).
- Provision Python **3.12**.
- Provisioning is a fallback; a usable system Python is always preferred.
- The bootstrap and shims never fail loudly in the hook path (hooks always exit 0); the
  install path may exit non-zero with actionable guidance.
