# Windows TTS No-Voices Actionable Error — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When no OneCore voices are installed, `WinTtsBackend.run()` raises the actionable `RuntimeError("No TTS voices installed…")` instead of a cryptic `FileNotFoundError [WinError -2147024894]`.

**Architecture:** Reorder `run()` so voice resolution (`_resolve_voice` → `best_voice`) happens *before* `SpeechSynthesizer()` construction. The actionable message already exists in `best_voice()`; it is currently unreachable because activation throws first. Also make the test mock faithfully reproduce real OneCore activation (throw on empty voices) so the regression is catchable in CI.

**Tech Stack:** Python 3.9+, pytest, PyWinRT (`winrt-*`), the repo's fake-winrt harness (`tests/_winfakes.py`).

**Spec:** `docs/superpowers/specs/2026-06-16-windows-tts-no-voices-error-design.md` · **Issue:** nimkimi/sonari#2 (Bug 2)

---

## Environment notes (read first)

- The Windows TTS tests are **mock-tested**; `tests/conftest.py` installs fake winrt modules. `_winfakes.install()` **no-ops on real Windows** (`if sys.platform == "win32": return`), so the suite is meant to run on macOS/Linux CI.
- **Local verification on Windows** must force the fake winrt tree. Use this exact command from the repo root (PowerShell):

```powershell
$env:PYTHONPATH = "$PWD;$PWD\src"
python -c "import tests._winfakes as w; w._install_winrt(); import pytest, sys; sys.exit(pytest.main(['tests/test_win_tts.py','-v']))"
```

- On macOS/Linux the normal invocation works: `python -m pytest tests/test_win_tts.py -v`
- **Commits:** this working tree carries the colon-named `commands/*.md` entries in the index (skip-worktree) to keep the tree clean on Windows. Commit with `git -c core.protectNTFS=false commit …` and stage files explicitly (never `git add -A` / `git commit -a`).

---

## File Structure

- `tests/_winfakes.py` — fake winrt harness. Modify the fake `SpeechSynthesizer.__init__` to raise when `all_voices` is empty (mirrors real OneCore activation). One responsibility: faithful platform fakes.
- `tests/test_win_tts.py` — add one regression test for the no-voices path.
- `src/sonari/platform/windows/tts.py` — reorder `run()`; no new functions, no signature changes.

---

## Task 1: Make the winrt mock faithful to real OneCore (throw on empty voices)

**Files:**
- Modify: `tests/_winfakes.py` (fake `SpeechSynthesizer.__init__`, inside `_install_winrt()`)
- Verify against: `tests/test_winfakes.py`, `tests/test_win_tts.py`

- [ ] **Step 1: Update the fake `SpeechSynthesizer.__init__`**

In `tests/_winfakes.py`, inside `_install_winrt()`, the current fake class is:

```python
    class SpeechSynthesizer:
        all_voices = [_Voice()]; default_voice = _Voice()
        def __init__(self): self.voice = None; self.options = _Opts()
        def synthesize_text_to_stream_async(self, t): return _AsyncOp(_Stream())
        def synthesize_ssml_to_stream_async(self, t): return _AsyncOp(_Stream())
```

Replace the `__init__` so it mirrors real OneCore (activation fails when no voices are installed):

```python
    class SpeechSynthesizer:
        all_voices = [_Voice()]; default_voice = _Voice()
        def __init__(self):
            # Mirror real OneCore: activating a synthesizer on a box with no
            # installed voices raises FileNotFoundError (WinError -2147024894).
            # Read via type(self) so a monkeypatched class attr is honored.
            if not type(self).all_voices:
                raise FileNotFoundError(
                    "[WinError -2147024894] The system cannot find the file specified."
                )
            self.voice = None; self.options = _Opts()
        def synthesize_text_to_stream_async(self, t): return _AsyncOp(_Stream())
        def synthesize_ssml_to_stream_async(self, t): return _AsyncOp(_Stream())
```

- [ ] **Step 2: Run the fake-harness + existing TTS tests to confirm no regression**

Run (PowerShell, repo root):

```powershell
$env:PYTHONPATH = "$PWD;$PWD\src"
python -c "import tests._winfakes as w; w._install_winrt(); import pytest, sys; sys.exit(pytest.main(['tests/test_winfakes.py','tests/test_win_tts.py','-v']))"
```

Expected: PASS — `test_winfakes.py` (constructs `SpeechSynthesizer()` with the default non-empty `all_voices`, so the guard does not fire) and all 6 existing `test_win_tts.py` tests stay green (7 passed total).

- [ ] **Step 3: Commit**

```powershell
git add tests/_winfakes.py
git -c core.protectNTFS=false commit -m @'
test(windows): fake SpeechSynthesizer raises on empty voices (#2)

Real OneCore activation throws FileNotFoundError when no voices are
installed; the fake never did, hiding the bug. Mirror it so the
no-voices regression is reproducible under the winrt mock.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
'@
```

---

## Task 2: Regression test (red) + reorder `run()` (green)

**Files:**
- Modify: `tests/test_win_tts.py` (add one test)
- Modify: `src/sonari/platform/windows/tts.py` (`run()`, currently constructs the synth at lines 160–161 before resolving the voice)

- [ ] **Step 1: Write the failing regression test**

Append to `tests/test_win_tts.py`:

```python
def test_run_raises_actionable_error_when_no_voices(monkeypatch):
    # On a box with no OneCore voices, run() must surface the actionable
    # "install a voice" RuntimeError — NOT the raw FileNotFoundError that real
    # SpeechSynthesizer activation throws. Regression: nimkimi/sonari#2.
    import winrt.windows.media.speechsynthesis as ss
    monkeypatch.setattr(ss.SpeechSynthesizer, "all_voices", [])
    with pytest.raises(RuntimeError, match="No TTS voices installed"):
        WinTtsBackend().run("hello", None, 200)
```

- [ ] **Step 2: Run the test to verify it FAILS (red)**

Run (PowerShell, repo root):

```powershell
$env:PYTHONPATH = "$PWD;$PWD\src"
python -c "import tests._winfakes as w; w._install_winrt(); import pytest, sys; sys.exit(pytest.main(['tests/test_win_tts.py::test_run_raises_actionable_error_when_no_voices','-v']))"
```

Expected: FAIL. With the current ordering, `run()` constructs `SpeechSynthesizer()` first; the now-faithful fake (Task 1) raises `FileNotFoundError` because `all_voices` is empty. `pytest.raises(RuntimeError)` does not catch `FileNotFoundError`, so the test errors with the uncaught `FileNotFoundError [WinError -2147024894]` — exactly the bug.

- [ ] **Step 3: Reorder `run()` to resolve the voice before constructing the synth**

In `src/sonari/platform/windows/tts.py`, the current body of `run()` begins:

```python
        speaking_rate = wpm_to_speaking_rate(rate)

        synth = SpeechSynthesizer()
        synth.voice = self._resolve_voice(voice)
```

Change it to resolve first:

```python
        speaking_rate = wpm_to_speaking_rate(rate)

        # Resolve the voice BEFORE constructing the synthesizer. On a box with
        # no OneCore voices, SpeechSynthesizer() activation itself throws a
        # cryptic FileNotFoundError (WinError -2147024894); resolving first lets
        # best_voice() raise the actionable "install a voice" message instead.
        resolved_voice = self._resolve_voice(voice)

        synth = SpeechSynthesizer()
        synth.voice = resolved_voice
```

Leave the rest of `run()` (options, SSML fallback, stream synthesis, MediaPlayer, handle) unchanged. Do not move the `from winrt…import …` statements; importing the module is side-effect-free and `_resolve_voice` does its own lazy import.

- [ ] **Step 4: Run the new test to verify it PASSES (green)**

Run:

```powershell
$env:PYTHONPATH = "$PWD;$PWD\src"
python -c "import tests._winfakes as w; w._install_winrt(); import pytest, sys; sys.exit(pytest.main(['tests/test_win_tts.py::test_run_raises_actionable_error_when_no_voices','-v']))"
```

Expected: PASS. `_resolve_voice(None)` → `best_voice()` → `list_voices()` returns `[]` → raises `RuntimeError("No TTS voices installed…")` before any synth is constructed.

- [ ] **Step 5: Run the full Windows TTS suite for regressions**

Run:

```powershell
$env:PYTHONPATH = "$PWD;$PWD\src"
python -c "import tests._winfakes as w; w._install_winrt(); import pytest, sys; sys.exit(pytest.main(['tests/test_win_tts.py','tests/test_winfakes.py','-v']))"
```

Expected: PASS — 7 in `test_win_tts.py` (6 original + 1 new) and `test_winfakes.py`. The voices-present path is unchanged.

- [ ] **Step 6: Commit**

```powershell
git add src/sonari/platform/windows/tts.py tests/test_win_tts.py
git -c core.protectNTFS=false commit -m @'
fix(windows): actionable error when no OneCore voices installed (#2)

run() constructed SpeechSynthesizer() before resolving the voice, so a
box with no voices got a cryptic FileNotFoundError (WinError -2147024894)
instead of best_voice()'s "No TTS voices installed — add a Speech
language pack" message. Resolve the voice first; the friendly RuntimeError
is now reachable. Adds a regression test for the no-voices path.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
'@
```

---

## Done criteria

- New test `test_run_raises_actionable_error_when_no_voices` passes; the 6 original `test_win_tts.py` tests and `test_winfakes.py` still pass.
- `git -c core.protectNTFS=false show --stat HEAD` and `HEAD~1` show only the three intended files changed (`tests/_winfakes.py`, then `src/.../tts.py` + `tests/test_win_tts.py`) — no `commands/` or other phantom changes.
- Out of scope (do NOT touch): Bug 1 colon filenames, macOS backend, real end-to-end audio verification.

## Delivery (after both tasks)

Handled by `superpowers:finishing-a-development-branch`:
- Fork `nimkimi/sonari` to the contributor account (Maxaubert); add as remote.
- Push `fix/windows-tts-no-voices-error`.
- Open PR with **base `phase-3-windows`** (not `main` — the Windows backend exists only on `phase-3-windows`), referencing issue #2, noting the fix is mock-verified and describing the real-box symptom it resolves.
