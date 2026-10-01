# Windows TTS: actionable error when no OneCore voices are installed

**Date:** 2026-06-16
**Status:** Design - approved for planning
**Issue:** nimkimi/sonari#2 (Bug 2)
**Scope:** Bug 2 only. Bug 1 (colon-named `commands/*.md` files break Windows clone) is explicitly out of scope.

## Problem

On a Windows machine with **no OneCore voices installed**, speaking fails with a cryptic, unactionable error. Observed on a real box (Windows 11 Pro x64, Python 3.14.3, winrt 3.2.1):

```
File "src/sonari/platform/windows/tts.py", line 160, in run
    synth = SpeechSynthesizer()
FileNotFoundError: [WinError -2147024894] The system cannot find the file specified.
```

`WinError -2147024894` (`0x80070002`, ERROR_FILE_NOT_FOUND) is raised when the WinRT `SpeechSynthesizer` object is **activated** - before any voice selection happens. The user is given no hint that the real fix is "install a speech voice."

### Root cause

In `WinTtsBackend.run()`:

```python
synth = SpeechSynthesizer()                 # line 160 - activation throws on a no-voices box
synth.voice = self._resolve_voice(voice)    # line 161 - never reached
```

Sonari already has the right message - `best_voice()` raises:

```
RuntimeError("No TTS voices installed. Add a Speech language pack in
              Settings -> Time & language -> Speech -> Add voices.")
```

But `best_voice()` is reached only via `_resolve_voice()` on line 161, which never runs because line 160 throws first. The actionable error is unreachable purely due to **ordering**.

### Why the test suite doesn't catch this

The fake WinRT tree in `tests/_winfakes.py` defines a `SpeechSynthesizer` whose `__init__` never raises, even when `all_voices` is empty. So the mock does not reproduce real OneCore behaviour (activation failing when no voices exist), and the bug is invisible in CI. Fixing the bug faithfully therefore requires making the mock match reality first.

## Approach (chosen: reorder, resolve-before-construct)

Resolve the voice **before** constructing the synthesizer. On a voiceless box, `_resolve_voice()` -> `best_voice()` raises the actionable `RuntimeError` first; the cryptic constructor is never reached.

This is safe on real Windows: the static property `SpeechSynthesizer.all_voices` returns `[]` without throwing on a no-voices box (verified on the real machine) - only the **constructor** and `default_voice` throw. So `list_voices()` / `best_voice()` can run before any synth instance exists.

```python
# resolve first - raises the actionable RuntimeError on a no-voices box
resolved = self._resolve_voice(voice)
synth = SpeechSynthesizer()
synth.voice = resolved
```

No new error text is introduced; the existing `best_voice()` message is simply made reachable.

### Approach considered and rejected

**try/except around the constructor** - catch `OSError`/`FileNotFoundError` at activation and re-raise as the friendly `RuntimeError`. Rejected: it would also swallow genuinely unrelated activation failures (broken speech runtime, COM init issues) under a misleading "no voices installed" message, and it duplicates the message string. Reordering is more precise and reuses existing logic.

## Changes (3 files)

1. **`src/sonari/platform/windows/tts.py`** - in `run()`, move the `_resolve_voice(voice)` call above `SpeechSynthesizer()` construction. Add a short comment explaining the ordering matters (activation throws on a voiceless box). Behaviour when voices exist is unchanged.

2. **`tests/_winfakes.py`** - make the fake `SpeechSynthesizer.__init__` raise `FileNotFoundError` when `all_voices` is empty, mirroring real OneCore activation. This is what lets the regression be reproduced under the mock. Existing tests (which rely on a populated `all_voices`) are unaffected.

3. **`tests/test_win_tts.py`** - add a regression test: with `all_voices` monkeypatched to `[]`, `run(...)` raises `RuntimeError` matching "No TTS voices installed", and specifically does **not** raise `FileNotFoundError`.

## Testing

- TDD order: (a) update the mock so it faithfully raises on empty voices; (b) add the failing regression test asserting the friendly `RuntimeError`; (c) reorder `run()` until it passes.
- Run the full existing Windows test suite (`tests/test_win_tts.py`, plus the broader suite) to confirm no regressions - the reorder must not change the voices-present path.
- Verification is at the **mock level**, consistent with the repo's stated convention that mock-green is not a claim of real-Windows behaviour. End-to-end audio verification is not done here because it requires installing a OneCore voice on the test box (deliberately out of scope per the chosen bug-2-only scope).

## Out of scope

- Bug 1 (colon in `commands/*.md` filenames blocking Windows clone) - opinionated, changes `/sonari:*` command names on macOS too; left for upstream discussion on issue #2.
- Installing a OneCore voice / real end-to-end audio verification.
- Any change to the macOS backend.

## Delivery

- Branch: `fix/windows-tts-no-voices-error` off `phase-3-windows`.
- Push to a fork under the contributor's account; open a PR referencing issue #2.
- PR base is `phase-3-windows` (where the Windows backend lives), not `main`.
- PR description will state that the fix is mock-verified and note the real-box symptom it resolves.
