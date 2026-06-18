# Sonari M2 — Windows Acceptance Checklist

> **Purpose:** This file is the gate M2 cannot close on macOS. Every item marked ⚠ is mock-blind — it cannot be verified by the macOS test suite. A human on a real Windows 10/11 machine must work through each item and tick it off before M2 is declared production-ready on Windows.
>
> **Scope:** Windows 10 21H2+ and Windows 11. Python 3.9+. Tested architectures: win-amd64. win-arm64 has an open risk — see Risks section.
>
> **Related:** The consolidated on-hardware residual checklist (winrt-absent, interrupt-silence, %TEMP% non-accumulation, hook-staleness, double-fire, autostart-after-logon, NTFS checkout) lives in **GitHub issue #28**. This file covers the TTS / earcon / crash-survival specifics.

---

## Pre-requisites

- A real Windows 10 or 11 machine (not a VM without audio, not Docker/Server Core — see Risk a).
- Python 3.9+ installed from python.org or via the Microsoft Store **real** installer (not a Store stub — see Risk e).
- `claude` CLI installed and at least one session run so the hooks directory exists.
- `git` and the Sonari repo checked out, or the wheel installed via pip.

---

## 1. Install

### 1a. Install the PyWinRT projection set

```powershell
pip install winrt-runtime ^
            winrt-Windows.Media.SpeechSynthesis ^
            winrt-Windows.Storage.Streams
```

Expected: all packages install without error. Confirm no `win-arm64` availability warning is printed (see Risk h).

> Only these three projections are required. Playback is stdlib `winsound` (COM-free) — the `winrt-Windows.Media.Playback` / `winrt-Windows.Media.Core` (MediaPlayer/MediaSource) packages are **no longer needed** and were dropped from this list; see Risk (a).

### 1b. Register the Task Scheduler task (non-admin)

```powershell
sonari install
```

Expected:
- No UAC elevation prompt appears (the install runs at LeastPrivilege for the current user).
- Exit code 0.

### 1c. Confirm the task is visible

```powershell
schtasks /query /tn Sonari.Speechd
```

Expected: the task appears in the output with status "Ready" or "Running". If the command returns exit code 1 ("ERROR: The system cannot find the file specified"), the install failed.

### 1d. Inspect the registered task XML

```powershell
schtasks /query /tn Sonari.Speechd /xml
```

Confirm:
- `<LogonType>InteractiveToken</LogonType>` is present (required for SAPI audio in the GUI session).
- `<RunLevel>LeastPrivilege</RunLevel>` is present (confirms no admin required).
- `<RestartOnFailure><Interval>PT5M</Interval>` is present.
- `<UserId>` matches your own `DOMAIN\username` (run `whoami` to check).

---

## 2. ⚠ Speech

> **Mock-blind risk.** The macOS suite proves the `_TtsHandle` contract holds against a fake `winsound`. It does NOT prove that `winsound.PlaySound(path, SND_FILENAME | SND_ASYNC)` actually routes audio to the speakers from a `DETACHED_PROCESS | CREATE_NO_WINDOW` Task Scheduler process, nor that the daemon survives a long run. See Risk (a).

### 2a. Start a `claude` session and send a short prompt

```cmd
claude "Say hello"
```

Expected:
- Claude's prose response is spoken aloud by a **OneCore** neural voice (not a legacy Desktop SAPI voice).
- The voice is intelligible at the default rate (200 wpm — the value of `rate` in `config.DEFAULTS`).
- There is no noticeable trailing silence longer than ~750 ms after the utterance ends (the `SpeechAppendedSilence.MIN` + `SpeechPunctuationSilence.MIN` options were applied).

If no audio is heard, proceed to Risk (a) diagnostics.

### 2b. Confirm the voice is OneCore (neural), not Desktop legacy

```powershell
python -c "
from winrt.windows.media.speechsynthesis import SpeechSynthesizer
v = SpeechSynthesizer.default_voice
print(v.display_name, v.id)
"
```

Expected: `v.id` contains `Speech_OneCore` (e.g. `HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Speech_OneCore\Voices\Tokens\TTS_MS_EN-US_ARIA_11.0`).

If only Desktop voices are listed (id contains `Speech\Voices\Tokens`), install a OneCore language pack: **Settings → Time & language → Speech → Add voices** and select an English (United States) Neural voice (e.g. "Microsoft Aria Online (Natural)").

### 2c. Crash-survival soak (the headline reason for the winsound switch)

> **Mock-blind risk.** This is the single most important on-hardware check and it has **zero automated coverage.** The previous MediaPlayer-based playback crashed the daemon with a native access violation after ~80 utterances (the daemon-death bug); the `winsound` switch (COM-free, in-process) is what fixes it. A passing mock suite cannot prove the fix — only a real long run can.

Speak **≥ 300 consecutive utterances** through the daemon and confirm the daemon process survives the whole run with no native access violation:

```python
from sonari.platform.windows.tts import WinTtsBackend

b = WinTtsBackend()   # reuses one SpeechSynthesizer across the run
for i in range(300):
    h = b.run(f"Utterance number {i}.", None, 200)
    h.wait(timeout=10.0)   # play to completion before the next
print("survived 300 utterances")
```

Expected:
- All 300 utterances synthesize and play; the script prints "survived 300 utterances".
- The process does **not** die with a native access violation (`0xC0000005`) or any crash dialog.
- Confirm the daemon itself survives the same soak in-session: `tasklist | findstr python` shows the same daemon PID before and after a long, chatty `claude` session.

---

## 3. ⚠ Interrupt

> **Mock-blind risk.** `_TtsHandle.terminate()` stops playback with `winsound.PlaySound(None, 0)` — the documented stop call on modern Windows (`SND_PURGE` is documented as unsupported there, see #17). The mock records the call but cannot prove the audio actually goes silent. The `try/except Exception: pass` guard silences any error, so a no-op stop would pass silently in the suite — the audio must really stop on hardware.

### 3a. Trigger skip mid-utterance

While a long Claude response is being spoken, issue a skip/stop command (exact key or command depends on hotkey configuration — M3 implements real hotkeys; for now trigger via the daemon socket directly or a short `sonari stop` CLI call if wired).

Expected:
- Audio cuts off within ~100 ms of the interrupt command — `terminate()` calls `winsound.PlaySound(None, 0)`, which purges the in-flight async clip.
- The next utterance (if any) starts without delay (a fresh `PlaySound` on a clean channel).
- The daemon remains running (confirm via `tasklist | findstr python` — the daemon process is still present).

### 3b. Confirm returncode after terminate

Instrument a test script to verify:
```python
from sonari.platform.windows.tts import WinTtsBackend
h = WinTtsBackend().run("This is a long utterance that we will interrupt", None, 200)
import time; time.sleep(0.2)
h.terminate()
assert h.returncode == 1
```

Expected: assertion passes and audio stops.

---

## 4. ⚠ Earcons

> **Mock-blind risk.** Each earcon plays in a **separate, windowless helper process** (`subprocess.Popen([sys.executable, "-c", "...winsound.PlaySound(...)"]`, spawned `CREATE_NO_WINDOW | DETACHED_PROCESS`). That helper has its own audio session, so the earcon **mixes** with the daemon's speech (shared-mode audio) instead of cutting it. The mock records the spawn but cannot verify that audio reaches the speakers, that the helper window never flashes, or that the mix is actually simultaneous. (The old single-channel `winsound` truncation model — earcon mid-utterance cuts speech — is obsolete; see Risk g.)

### 4a. Confirm each earcon is distinct and audible

Trigger each of the 6 earcon types in sequence (permission, choice, plan, error, turn_done, ready) and verify each plays its distinct generated `.wav` tone:

```python
from sonari.platform.windows.earcon import WinEarconBackend
import time

b = WinEarconBackend()
for name, path in b.default_earcons().items():
    print(f"Playing: {name}")
    h = b.play(path)          # returns a Popen handle (or None if path missing)
    assert h is not None
    h.wait(timeout=4.0)       # let the helper process finish before the next
    assert h.poll() == 0      # helper exited cleanly (0)
    time.sleep(0.1)
```

Expected: 6 distinct short tones play in sequence, each audibly different. No console window flashes for any earcon (the helper is windowless).

### 4b. Confirm an earcon mid-utterance MIXES with speech (does not cut it)

This is the behavior the separate-process design exists to deliver. Start a long utterance, then fire an earcon while it is still speaking:

```python
from sonari.platform.windows.tts import WinTtsBackend
from sonari.platform.windows.earcon import WinEarconBackend
import time

speech = WinTtsBackend().run(
    "This is a deliberately long sentence so there is plenty of time "
    "to fire an earcon while it is still being spoken aloud.", None, 200)
time.sleep(0.5)               # speech is now playing
ear = WinEarconBackend()
ear.play(next(iter(ear.default_earcons().values())))   # fire mid-utterance
```

Expected:
- **Speech CONTINUES** — the earcon does **not** truncate or silence it.
- **Both are audible** simultaneously (the earcon mixes over the speech in shared-mode).
- The daemon process does not raise or die.

---

## 5. ⚠ Single-instance

> **Mock-blind risk.** The `msvcrt.locking` fake tracks inodes in-process. Real `msvcrt.locking` is a system-wide byte-range lock — two separate `python.exe` processes must not both hold it. This cross-process behavior cannot be verified from a mock.

### 5a. Confirm two daemons cannot start simultaneously

In two separate PowerShell windows, launch the daemon directly:

```powershell
# Terminal 1
python -m sonari.daemon

# Terminal 2 (immediately after)
python -m sonari.daemon
```

Expected:
- Terminal 1: daemon starts and listens.
- Terminal 2: daemon exits immediately (the singleton lock is held by Terminal 1).
- `tasklist | findstr python` shows exactly **one** daemon process.

### 5b. Confirm the lock releases on daemon exit

Kill Terminal 1's daemon (Ctrl+C), then start the daemon in Terminal 2.

Expected: Terminal 2's daemon starts successfully and acquires the lock.

### 5c. If `msvcrt.locking` proves unreliable

If the above test reveals that two daemons start simultaneously (e.g. on a network drive or unusual filesystem), switch to a named mutex:

```python
# Alternative (named mutex — add to transport.py if needed)
import ctypes
mutex = ctypes.windll.kernel32.CreateMutexW(None, True, "Global\\SonariDaemon")
if ctypes.windll.kernel32.GetLastError() == 0xB7:  # ERROR_ALREADY_EXISTS
    sys.exit("Another instance is already running.")
```

This is the fallback documented in `transport.acquire_singleton`.

---

## 6. ⚠ Autostart + Restart

> **Mock-blind risk.** The Task Scheduler task XML was validated by `ET.fromstring()` in the test suite. Whether `schtasks` actually accepts it (UTF-16 BOM, namespace, non-admin LogonTrigger) and whether the daemon starts on logon is unverifiable from macOS.

### 6a. Logoff/logon autostart

1. Log off the Windows session.
2. Log back in.
3. Wait ~10 seconds, then:
   ```powershell
   tasklist | findstr python
   ```
   Expected: the Sonari daemon process is running.

### 6b. Supervisor restart after daemon kill

1. Identify the daemon's PID:
   ```powershell
   tasklist | findstr python
   ```
2. Kill the daemon process:
   ```powershell
   taskkill /PID <daemon_pid> /F
   ```
3. Wait for the backoff interval (base = 2 seconds for the first crash), then check again:
   ```powershell
   tasklist | findstr python
   ```
   Expected: a new daemon process is running within ~5 seconds.

### 6c. Confirm backoff sequence on repeated crashes

Kill the daemon 4 times in rapid succession and measure the restart delays. Expected sequence (seconds): 2, 4, 8, 16 (capped at 120 for subsequent crashes). Backoff resets to 2s after the daemon runs for ≥ 300 seconds without crashing.

---

## 7. ⚠ Hooks Fire

> **Mock-blind risk.** The exec-form hooks.json builder (`build_hooks_json`) was tested against a JSON schema contract. Whether Claude Code on Windows actually resolves the hooks config path and fires the exec-form hook (no bash shim) with the correct `command` + `args` is unverifiable from macOS.

### 7a. Locate the Claude Code Windows hooks directory

Run `claude` and check where it reads hooks from. Typically one of:
- `%APPDATA%\Claude\hooks.json`
- `%USERPROFILE%\.claude\hooks.json`
- The directory printed by `claude config get hooks_dir` (if that CLI command exists)

Confirm the exact path and update this checklist entry once confirmed.

### 7b. Deploy the hooks.json

```powershell
sonari install
```

Inspect the written `hooks.json` file. Confirm:
- `"type": "command"` is present (exec-form, not shell-form).
- `"command"` is the resolved `pythonw.exe` path (absolute, backslashes doubled in JSON).
- `"args"` is `["<path>\\hook.py", "MessageDisplay"]` for the MessageDisplay hook.

### 7c. Confirm a hook fires during a claude session

Start a `claude` session and send a prompt. Monitor the daemon log (or add a brief debug print to `hook.py`):

```powershell
Get-Content "$env:USERPROFILE\.sonari\daemon.log" -Wait
```

Expected: a line appears in the log each time Claude produces output (MessageDisplay hook fired → daemon received the event → TTS was triggered).

### 7d. Confirm Stop hook fires on session end

At the end of a `claude` session, confirm the Stop hook fires and the daemon receives it (no orphaned speech after the session ends).

---

## 8. RISKS — Probe Explicitly (Mock-Blind)

The following risks cannot be verified from macOS and must be probed on the Windows box. Each is a potential show-stopper.

### Risk (a): `winsound` audio routing + daemon survival from a DETACHED_PROCESS | CREATE_NO_WINDOW process

**This is the #1 risk**, and it has two parts:

**(a.1) Audio routing.** Playback is now stdlib `winsound.PlaySound` (COM-free, in-process) — there is no MediaPlayer, no STA, and no `CoInitializeEx` requirement. The residual concern is simpler: does `winsound.PlaySound` actually route audio to the speakers from a daemon launched by Task Scheduler with `DETACHED_PROCESS | CREATE_NO_WINDOW`? A detached process may not inherit an audio endpoint. The **same routing concern applies to the earcon helper**, which is itself spawned `CREATE_NO_WINDOW | DETACHED_PROCESS` (see section 4).

**Diagnostic:** If no audio plays from the Task Scheduler–launched daemon but the direct script in 2c/4a *does* play, the problem is the launch context, not the code. Confirm the task runs with `<LogonType>InteractiveToken</LogonType>` (section 1d) so it shares the interactive desktop session's audio endpoint. If audio still does not route, run the daemon as a normal session process (Startup-folder launcher or `HKCU\...\Run`) instead of a Task Scheduler task.

**(a.2) Daemon survival over a long run (the crash fix).** The MediaPlayer playback path was **removed** because it crashed the process with a native access violation after ~80 utterances (the daemon-death bug). The whole point of the `winsound` switch is survival. This is verified by the **2c crash-survival soak** (≥ 300 consecutive utterances, daemon survives, no `0xC0000005`). Treat a crash anywhere in that soak as a show-stopping regression.

### Risk (b): `IAsyncOperation.get()` blocking behavior in a daemon thread

`synthesize_text_to_stream_async(text).get()` is called synchronously on the daemon's TTS thread. Verify that `.get()` actually blocks and returns the stream (and does not require `await` in an asyncio context or a WinRT message pump).

**Test:** Run the TTS backend directly in a script:
```python
from sonari.platform.windows.tts import WinTtsBackend
h = WinTtsBackend().run("test blocking", None, 200)
rc = h.wait(timeout=5.0)
print("returncode:", rc)  # must be 0
```

If this hangs indefinitely, `.get()` requires a message pump — add a `comtypes`-based STA loop or switch to the `asyncio`-based PyWinRT pattern.

### Risk (c): Single-instance truly excludes across processes

Covered in section 5 above. If `msvcrt.locking` fails cross-process, the fallback is `kernel32.CreateMutexW` + `GetLastError() == ERROR_ALREADY_EXISTS` (documented in `transport.acquire_singleton` docstring).

### Risk (d): `schtasks /xml` UTF-16 acceptance + non-admin LogonTrigger registration (no UAC)

The `TASK_XML_TEMPLATE` is written with `encoding='utf-16'` (Python emits UTF-16 LE with BOM). On Windows builds before 22H2 this is required; UTF-8 causes "The task XML is malformed." Confirm schtasks accepts the file without error.

Also confirm that a **standard (non-admin) user** can register the task. Expected: no UAC prompt. If UAC appears, the `RunLevel` or `LogonType` is wrong — verify `LeastPrivilege` and `InteractiveToken` are both set.

### Risk (e): Store-stub avoidance on a machine where only Store Python exists

On a fresh Windows 11 install, `python` on PATH may point to `%LOCALAPPDATA%\Microsoft\WindowsApps\python.exe` (the Store stub, exit code 9009). `resolve_python_windows()` must skip this and find the real interpreter via `py -3` launcher or a PATH-based probe.

**Test:** On a machine where Store Python is the only `python` on PATH, confirm `sonari install` still resolves a real Python >= 3.9 and completes without error. If it fails, install Python from python.org and ensure `py.exe` launcher is available.

### Risk (f): `importlib.resources.as_file` temp-path lifetime for wheel installs

**RESOLVED IN IMPLEMENTATION — no probe required.**

`earcons/__init__.py` does not use `importlib.resources.as_file()` at all. It resolves earcon paths via `pathlib.Path(__file__).parent / fname`, which is a sibling-file lookup. This is reliable for all supported install modes (editable installs, unpacked wheels, sdist builds) and is explained in the `default_earcons()` docstring. The `as_file()` approach was explicitly evaluated and rejected precisely because it deletes the extracted temporary file when the `with` block exits.

The sign-off table entry for this risk can be ticked as "N/A — resolved in code".

### Risk (g): earcon ↔ speech / earcon ↔ earcon mixing (the obsolete truncation model)

**Largely resolved by the separate-process design — verify the mix, don't fear the truncation.** The old single-channel concern (a new `PlaySound(..., SND_ASYNC)` silently cancels the previous async sound, so an earcon mid-utterance cuts speech and rapid earcons cut each other) **no longer applies**: each earcon plays in its own windowless helper process (section 4), so it has a separate audio session and mixes shared-mode rather than purging the daemon's speech channel.

**What to verify on hardware:**
1. An earcon fired mid-utterance leaves speech audible and continuous (section **4b**) — this is the headline behavior of the redesign.
2. Several earcons fired in rapid succession all play (or overlap) without truncating speech; none crash the daemon (a failed helper spawn is caught and logged, never raised — see `WinEarconBackend.play`).
3. The helper processes are windowless (no console flash) and short-lived — confirm they exit and do not accumulate (`tasklist | findstr python` does not grow unboundedly during a chatty session).

### Risk (h): PyWinRT projection availability for win-arm64

The PyWinRT packages (`winrt-Windows.Media.SpeechSynthesis`, etc.) are confirmed available for `win-amd64`. As of the M2 research date (2026-06-11), `win-arm64` wheels may be unavailable on PyPI for all projection packages.

**Test (on arm64 hardware or via pip dry-run):**
```powershell
pip install --dry-run winrt-Windows.Media.SpeechSynthesis
```

If no arm64 wheel is found, document this as a known gap. Fallback: use the Windows SAPI 5 COM interface directly via `comtypes` or `pywin32`, which ships arm64 wheels.

---

## 9. Residual

- **Nima is low-vision (magnifier user).** A fully-blind + NVDA screen reader pass is a separate pre-GA step. Confirm that: (a) the spoken audio does not conflict with NVDA speech; (b) NVDA can navigate the `sonari install` output; (c) earcon volume is not overpowering relative to NVDA speech.
- **Uninstall path:** `sonari uninstall` on Windows must delete the Task Scheduler task and the hooks.json. Verify both are removed and no orphaned process remains.
- **Upgrade path:** running `sonari install` over an existing installation (task already registered) must not fail — the `/f` flag on `schtasks /create` overwrites silently.

---

## Sign-off

| Item | Tester | Date | Result | Notes |
|------|--------|------|--------|-------|
| 1a. PyWinRT install | | | | |
| 1b. sonari install (no UAC) | | | | |
| 1c. schtasks /query | | | | |
| 1d. schtasks /query /xml | | | | |
| 2a. Speech audible (OneCore) | | | | |
| 2b. Voice is OneCore/neural | | | | |
| 2c. Crash-survival soak (≥300 utterances, daemon survives, no access violation) | | | | |
| 3a. Skip mid-utterance (PlaySound(None,0) stops) | | | | |
| 3b. returncode after terminate | | | | |
| 4a. All 6 earcons distinct (windowless) | | | | |
| 4b. Earcon mid-utterance MIXES (speech continues, both audible) | | | | |
| 5a. Single-instance cross-process | | | | |
| 5b. Lock releases on exit | | | | |
| 6a. Autostart on logon | | | | |
| 6b. Supervisor restarts daemon | | | | |
| 6c. Backoff sequence | | | | |
| 7a. Hooks dir located | | | | |
| 7b. hooks.json exec-form | | | | |
| 7c. MessageDisplay hook fires | | | | |
| 7d. Stop hook fires | | | | |
| Risk (a): winsound routing + daemon survival (DETACHED_PROCESS) | | | | |
| Risk (b): IAsyncOperation.get() | | | | |
| Risk (c): msvcrt cross-process | | | | |
| Risk (d): UTF-16 + non-admin | | | | |
| Risk (e): Store stub avoidance | | | | |
| Risk (f): as_file temp lifetime | N/A | — | N/A | Resolved in implementation; pathlib sibling lookup used instead |
| Risk (g): earcon/speech mixing (no truncation) | | | | |
| Risk (h): arm64 PyWinRT | | | | |
| Residual: NVDA pass | | | | |
| Residual: uninstall path | | | | |
| Residual: upgrade path | | | | |

---

## 10. Install seam (2026-06-16) — `sonari install` writes settings.json hooks + launcher

> Added when `cli.install/uninstall/doctor` were wired through the platform seam
> (`docs/superpowers/specs/2026-06-16-windows-install-seam-design.md`). These verify the
> Windows install path produces NO macOS artifacts and the hooks land in user settings.

### 10a. Install writes exec-form hooks to user settings.json

```powershell
sonari install
```

Confirm:
- Output mentions "Registered Task Scheduler task", "Wrote Sonari hooks to: …\.claude\settings.json", "Placed launcher", and a real voice **name** (not a `<winrt…VoiceInformation object>` repr).
- **No** macOS output (`LaunchAgent`, `launchctl`, `swiftc`, `xcode-select`, `~/.local/bin/sonari` bash wrapper).
- `%USERPROFILE%\.claude\settings.json` now contains a `hooks` block whose entries are exec-form: `"command"` = the resolved `pythonw.exe` (absolute), `"args"` = `["…\bin\sonari-hook", "MessageDisplay"]` (and the other events). Any pre-existing keys/hooks in that file are preserved.
- `install.json` records a **real** `pythonw.exe` (not `…\WindowsApps\python3.exe`, the Store stub).
- `%USERPROFILE%\.local\bin\sonari.cmd` exists and `sonari doctor` runs through it.

### 10b. Double-fire constraint (do NOT also enable the plugin's shell-form hooks)

The plugin's committed `hooks/hooks.json` is **shell-form** (macOS-only); on Windows it cannot spawn the Python hook. Because Claude Code **merges** plugin-manifest hooks with settings.json hooks, enabling both would fire every event twice. On Windows, Sonari's hooks must come from `settings.json` **only** — do not also enable the plugin's manifest hooks.

### 10c. Uninstall reverses only Sonari's changes

```powershell
sonari uninstall
```

Confirm:
- The Task Scheduler task is gone (`schtasks /query /tn Sonari.Speechd` → not found).
- **Only** Sonari's hook entries are removed from `settings.json` (any non-Sonari hooks and other keys survive).
- `%USERPROFILE%\.local\bin\sonari.cmd` is removed.
- `config.json` and `keymap.json` are preserved.

### 10d. `sonari doctor` shows Windows rows

`sonari doctor` reports the Windows supervisor rows (schtasks, Task Scheduler task, pythonw.exe, neural voice, daemon running) and a "hooks installed" row that reflects whether `settings.json` carries Sonari's hooks — no macOS rows (`say`/`afplay`/`swiftc`/`LaunchAgent`).
