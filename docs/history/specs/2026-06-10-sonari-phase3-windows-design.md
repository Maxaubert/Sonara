# Sonari Phase 3 - Windows Native Support - Design Spec

> **projectType:** claude-plugin
> **Status:** approved design from the live brainstorm (Nima, 2026-06-10), hardened by a 3-critic adversarial review and a 2-validator primary-source pass. Consumed by writing-plans → milestone plans.
> **Grounding:** the recon workflow (`sonari-phase3-windows-recon`) mapped every macOS-coupled line; the review (`sonari-phase3-spec-review`) caught two criticals + completeness gaps; the validation (`sonari-phase3-tts-validation`) confirmed the TTS path against Microsoft + the Piper repos/licenses.
> **Scope:** a **major addition** to Sonari (`~/projects/private/claude-tts`, v0.5.0) - a second supported OS behind a clean platform-abstraction seam, with feature parity.

---

## 1. Core

- **Problem:** Sonari is macOS-only. Every behavior assumes `say`, `afplay`, a Swift/Carbon hotkey daemon, `launchctl`/LaunchAgents, AF_UNIX sockets, and macOS TCC. Blind/low-vision developers on Windows - the larger accessibility population, who overwhelmingly run NVDA or JAWS - cannot use it.
- **Target user:** other Windows blind/low-vision Claude Code users, with Nima dogfooding. Voice quality and screen-reader coexistence are first-class.
- **Why this is tractable:** the riskiest macOS components have *no Windows equivalent to fight*. Global hotkeys are ~40 lines of pure-Python `ctypes` (`RegisterHotKey`) - no compiler, no separate binary, no per-rebuild grant. The TCC / Secure-Keyboard-Entry / cdhash-grant walls that killed Phase 2's caret tracking **do not exist on Windows**. The core is already pure Python, and `Speaker` already injects its TTS + earcon backends.
- **MVP scope:**
  - **In v1 (this phase):** full Windows feature parity - TTS (OneCore default + opt-in Piper neural), earcons, all hotkey actions, voice continuity, multi-session history. A `platform/` seam with macOS + Windows backends. IPC unified on localhost TCP for both OSes. Zero-admin, zero-toolchain Windows install.
  - **Out (later / excluded):** automatic screen-reader audio ducking (v1 ships coexistence-by-configuration; see §4); an opt-in "speak through NVDA" route-through mode; Windows Server / headless audio; win-arm64 native Piper (x64-emulation only); the elevated-terminal hotkey case (documented limitation; §8); any new hotkey *bindings*.
- **Definition of Done** (each testable; **⚠ = audible/behavioral → human listen-test on the Windows box, not headless**):
  - `sonari install` on Windows sets up autostart with **no administrator prompt** and no compiler; `sonari` runs from the same package on both OSes.
  - **No OS branch leaks into the portable core** - core modules (`assembler/cleaner/queue/history/sessions/protocol/hooks_entry`, the `Speaker` class, the keymap resolver) contain no `sys.platform` test and import no `platform/{macos,windows}` module; their observable behavior is unchanged on macOS. The **only** `sys.platform` branch is in `platform/__init__.py`. *(This is the testable invariant - not "files literally unchanged," which is false: the IPC rewrite and helper moves edit core files.)*
  - ⚠ On Windows, Claude's prose / options / plans / permissions are spoken by the **OneCore** default voice, offline, at a usable rate (the 750ms default lag eliminated).
  - ⚠ The opt-in `sonari voices install` fetches Piper + the public-domain default voice into `~/.sonari/`, and Windows speech then uses the **neural** voice; `doctor`/install surface that neural is available.
  - ⚠ `stop` and `skip` interrupt Windows speech **mid-utterance** (OneCore: `MediaPlayer.Pause()`; Piper: kill the subprocess); `TtsBackend.speak()` returns `False` on interrupt, mirroring the macOS `say` exit-code path locked by `test_speaker.py::test_speak_returns_false_when_say_terminated`. The heard-marker is sentence-granular. *(See §9 - the Windows interrupt mechanism is designed, not yet proven on hardware.)*
  - ⚠ All five hotkey actions (`reread_options`, `repeat`, `catch_up`, `stop`, `skip`) fire system-wide while a non-elevated Windows terminal has focus, with the default `Ctrl+Shift+Alt+<key>` chord. **Behavior of each action is identical to macOS - only key resolution differs.**
  - ⚠ Earcons play per message type on Windows (bundled `.wav`, non-blocking).
  - `sonari doctor` on Windows reports per-platform rows: hotkey-chord collisions, the running screen reader (if any), supervisor state, the active TTS engine + neural availability - honestly, no false greens.
  - macOS behavior is **identical** after the seam refactor (Milestone 1 is behavior-preserving, verified on Nima's Mac).
  - The default hotkey chord is **user-configurable** via the existing keymap.
  - A **Windows-limitations doc** exists covering: don't-run-elevated (hotkeys), secure-desktop transience, the opt-in neural download, chord-collision remediation, and **third-party license disclosure** (Piper MIT, embedded espeak-ng LGPL, and the active voice model's license).
- **Hard constraints:**
  - **Runtime performance (HARD):** hotkeys feel instant; no model/LLM call on any hotkey path; per-session history bounded.
  - **Zero-friction install (HARD):** no admin rights on Windows; no build toolchain (no compiler, no .NET SDK) at install or runtime.
  - **One voice only**; never talk over the user's screen reader by design (§4).
  - **Dependency discipline:** the core stays stdlib-only; macOS stays stdlib-only (`say`/`afplay`). Windows is permitted **exactly one pip dependency family** - Microsoft's **PyWinRT** projections (`winrt-runtime` + the `winrt-Windows.Media.SpeechSynthesis` / `.Media.Playback` / `.Media.Core` / `.Storage.Streams` packages) - for the default OneCore TTS. The opt-in Piper engine is **not** a pip dependency: it's a hash-pinned binary fetched on demand into `~/.sonari/`, never vendored in git.
  - **Offline-capable:** the default TTS (OneCore) and the opt-in Piper both run fully offline after install; never depend on an online endpoint as the primary path.

---

## 2. Architecture - the platform seam

A new `sonari/platform/` package puts every OS-specific behavior behind a narrow interface. The core imports the interface, never `sys.platform`. A single `get_platform()` factory is the **only** OS branch in the codebase.

```
sonari/
  platform/
    __init__.py        # get_platform() -> PlatformBackend   (THE ONLY sys.platform branch)
    base.py            # the four ABCs + PlatformBackend bundle
    transport.py       # SHARED localhost-TCP IPC (both OSes - not branched)
    macos/  { tts.py, earcon.py, hotkeys.py, supervisor.py, keytables.py }
    windows/{ tts.py, earcon.py, hotkeys.py, supervisor.py, keytables.py }
```

### The four interfaces (contracts)

| Interface | Methods | Notes |
|---|---|---|
| `TtsBackend` | `speak(text, voice, rate) -> bool` (blocks; **True iff finished, False if interrupted** - the heard signal); `cancel()`; `list_voices() -> list[str]`; `best_voice() -> str` | macOS: `say` exit code is the bool. Windows-OneCore: **playback completion** (MediaEnded) is the bool; `cancel()` = `MediaPlayer.Pause()`. Windows-Piper: subprocess exit/kill. |
| `EarconBackend` | `play(path) -> handle\|None` (non-blocking); `default_earcons() -> dict[str,str]` | macOS: `afplay` + `.aiff`. Windows: `winsound` `SND_ASYNC` + bundled `.wav`. **Note:** `winsound` is single-channel - rapid successive earcons truncate each other; keep cues short. |
| `HotkeyBackend` | `register(resolved_bindings)`; `run()`; `resolve(action_chord) -> os_codes` | macOS: Swift binary → TCP. Windows: ctypes `RegisterHotKey` + `GetMessage` pump on a daemon thread, **in-process** (no second process). |
| `SupervisorBackend` | `install(python, app_dir)`; `uninstall()`; `is_running() -> bool`; `restart()`; **`launch_spec() -> (argv, spawn_flags)`** (the daemon's lazy-start command + OS spawn flags); `is_installed() -> bool` (consumed by the core session-start health cue) | macOS: launchctl + plist; spawn via `start_new_session`. Windows: `schtasks /xml` task + Python supervisor; spawn via `DETACHED_PROCESS | CREATE_NO_WINDOW` under `pythonw`. `restart()` forwards to the supervisor (schtasks has no restart verb). |

`get_platform()` returns a `PlatformBackend` bundle; the daemon and CLI receive it by injection. The `Speaker` class, the keymap *resolver*, and the `cli` argparse become OS-agnostic consumers.

### Files that move behind the seam (verified against the v0.5.0 source)

- **`speaker.py`** → the `Speaker` *class* stays portable in core. `run_say`, `play_earcon`, `best_enhanced_voice` move into `platform/macos/{tts,earcon}.py` and are **renamed/wrapped to satisfy the ABC method names** (`speak`, `play`, `best_voice`). New `platform/windows/{tts,earcon}.py`. (Resolves the `best_voice` vs `best_enhanced_voice` / `play` vs `play_earcon` naming drift - the ABC vocabulary wins.)
- **IPC** (`daemon.py` bind/accept + **`ensure_running()`/`_daemon_shim_path()` lazy-spawn**, `client.py` connect, `paths.py` socket probe + `SOCKET_PATH`) → one **shared** `platform/transport.py` on **localhost TCP** (127.0.0.1, ephemeral port + a 256-bit token in a `0o600`/NTFS-private lockfile; daemon PID for an `os.kill(pid,0)` liveness check). **Security: the per-connection token is MANDATORY** - loopback TCP has no filesystem ACL like a `0o600` AF_UNIX socket, so the daemon rejects any connection that doesn't present the token before processing. The Swift hotkeyd (macOS) writes to the TCP port instead of the socket path. `SOCKET_PATH` (a `.sock` file) is replaced by the port+token lockfile; the macOS uninstall artifact list changes accordingly. **`ensure_running()`'s spawn is platform-dispatched** via `SupervisorBackend.launch_spec()` - `start_new_session` (POSIX) becomes `DETACHED_PROCESS|CREATE_NO_WINDOW` on Windows, and the shim path comes from the backend, not `repo_root()/bin/`.
- **Launchers** (`bin/sonari`, `bin/sonari-daemon`, `bin/sonari-hook` bash shims; `cli._place_launcher()`'s `~/.local/bin/sonari` bash wrapper) → these are **macOS-only** (`#!/usr/bin/env bash`, `:`-separated PYTHONPATH, hardcoded `/usr/bin/python3`) and become a **launcher responsibility of `SupervisorBackend`**. Windows equivalent: a `python -m sonari.cli|daemon` invocation under the resolved interpreter (no bash), placed where Windows expects it; the `~/.local/bin` concept maps to a Windows entry the supervisor owns.
- **`daemon._setup_health()` / `_launcher_present()`** → currently hardcode `~/.local/bin/sonari` (would nag "not installed" every session on Windows). They must consume `SupervisorBackend.is_installed()` instead of a baked macOS path.
- **`keymap.py`** → `KEY_CODES` / `MOD_MASKS` move to `platform/{macos,windows}/keytables.py`; the resolver, `ACTION_MESSAGES`, and config I/O stay portable. `cli._combo_label` / `_KEYCODE_DISPLAY` / `_MOD_DISPLAY` (which hardcode Carbon mask values 4096/256/2048/512) are **macOS-Carbon presentation tables** and move to the macOS backend (or read OS codes via `HotkeyBackend`).
- **`cli.py`** install/uninstall/`_launchctl`/`_plist`/`_build_hotkeyd` → `platform/macos/supervisor.py` + `hotkeys.py`. **`_resolve_python` is SPLIT, not moved:** macOS keeps the `/usr/bin/python3` preference + the `python3.N` candidate tuple; Windows is a **net-new** implementation (`py -3` launcher → `python` on PATH → **skip the Microsoft Store stub** via the `WindowsApps` path / exit-9009 detection - there is no macOS analogue). The argparse wiring, control-plane send helpers, install-record I/O stay in `cli.py`. **`doctor()`'s check rows are supplied per-platform** (each backend contributes its rows - `say/afplay/swiftc/launchctl` are macOS rows, not a fixed list).
- **`config.py`** → `'earcons'` is **dropped from the static `DEFAULTS`** dict (it currently bakes six `/System/Library/Sounds/*.aiff` paths at import) and filled at `load_config()` time from `EarconBackend.default_earcons()`, so a Windows daemon never inherits `.aiff` paths.

**Net effect:** the core is logic-only and OS-agnostic; the only `sys.platform` lives in `platform/__init__.py`.

---

## 3. Per-layer Windows backends

### TTS - OneCore default + opt-in Piper neural (`platform/windows/tts.py`)

**Default: OneCore via PyWinRT** (zero bundled binaries; true to Sonari's ship-nothing ethos).
- `Windows.Media.SpeechSynthesis.SpeechSynthesizer.AllVoices` enumerates the built-in OneCore voices (David/Zira/Mark on en-US) from the `Speech_OneCore` registry hive - reachable by a plain (non-UWP) Python process, no admin, **fully offline**. The Narrator-exclusive neural voices (Aria/Jenny/Guy) are **not** reachable by any third-party app (verified) - hence the opt-in Piper path below.
- **Heard signal & interrupt** (per the PyWinRT `samples/text_to_speech.py` reference): `SynthesizeTextToStreamAsync` → `MediaPlayer.set_stream_source` → `add_media_ended(cb)` → `play()`. `MediaEnded` fires on a WinRT background thread → set a **`threading.Event`** (the daemon is thread-based, **not** asyncio - do not copy the sample's `call_soon_threadsafe`) to unblock `speak()` with `True`. `cancel()` = `MediaPlayer.Pause()`/dispose → `speak()` returns `False`.
- **Four mandatory hardening steps:** (1) set `options.appended_silence = MIN` and `options.punctuation_silence = MIN` at init (eliminates the default ~750ms post-utterance lag - critical for rapid short utterances); (2) wrap `options.SpeakingRate` in a try/`AttributeError` guard with an SSML `<prosody rate>` fallback for Windows 10 < 1709; (3) robust fallback chain: match `AllVoices` by name → `DefaultVoice` → if `None`, raise a clear install error ("install an en-US language pack"); (4) **hold a live Python reference to the `SpeechSynthesisStream`** for the whole playback (GC hazard).
- **Quality, stated honestly:** OneCore is DNN-based but "functional, not natural" - more robotic than macOS Premium, better than legacy SAPI5. Acceptable as a command-line daily-driver baseline; not a showpiece voice. This is *why* the opt-in neural path exists.

**Opt-in neural: Piper** (`sonari voices install`; fetched, never vendored).
- `sonari voices install` downloads, into `~/.sonari/`: (1) the **MIT** `rhasspy/piper` `2023.11.14-2` self-contained binary (`piper_windows_amd64.zip`, 21.4MB - `piper.exe` + onnxruntime + espeak-ng DLLs + `espeak-ng-data/`; macOS `piper_macos_aarch64.tar.gz`, 18.3MB), **pinned by SHA256**; and (2) the default voice model **`en_US-ljspeech`** (`.onnx` + `.json`) from HuggingFace `rhasspy/piper-voices` - **public domain, no attribution** (the cleanest possible default). Cached for offline use after first download.
- **Invocation:** Piper runs as a subprocess - text on stdin, raw 16-bit PCM via `--output-raw` (or WAV via `-f`) → played; **kill the process to interrupt** (no flush needed). This maps onto the *same* contract as macOS `say` (Popen + exit/kill), so `TtsBackend.speak()`'s bool comes from the subprocess outcome - arguably simpler than the OneCore async path.
- **Licensing (build must honor):** Piper engine MIT (subprocess invocation = "mere aggregation" under GPL §5 - Sonari stays MIT; the maintained GPL fork is rejected because it's Python-wheel-only). Embedded espeak-ng is LGPL (dynamic link - fine to redistribute; disclose). Default voice `ljspeech` is public-domain. **Forbidden defaults** (license-tainted - the build must not select them): `lessac` (Blizzard research-only), `ryan`/`hfc_*` (CC BY-NC-SA), and the lessac-finetuned chain (`joe`/`amy`/`arctic`/`kathleen`/`libritts_r`). A clean *optional* second voice is `libritts-high` (CC BY 4.0) - but it requires an attribution line in the UI/docs.
- **Caveats to document:** the MIT binary is archived/frozen (Nov 2023) - pin + hash it, migrate if a standalone exe re-emerges; **no win-arm64 build** - ARM users run x64 emulation (slower); models come from HuggingFace (cache locally; pin versions + hashes).

### Earcons - `winsound` (`platform/windows/earcon.py`)
`winsound.PlaySound(path, SND_FILENAME | SND_ASYNC)` - stdlib, non-blocking. **WAV-only** → the six `.aiff` earcons re-encoded to bundled `.wav`; `default_earcons()` returns their packaged paths. Single-channel: keep cues short (rapid successive earcons truncate); TTS routes through a separate audio path (MediaPlayer/Piper), so earcon-vs-TTS overlap is fine.

### Hotkeys - ctypes `RegisterHotKey` in-process (`platform/windows/hotkeys.py`)
`RegisterHotKey` (Win32 via ctypes) on a dedicated daemon thread with a `GetMessage` pump - WM_HOTKEY posts to the **registering thread's** queue, so the pump must live on the same thread that registers. The exact OS-level semantic of Carbon `RegisterEventHotKey` (no window, no admin, fires regardless of focus). **In-process** - collapses the macOS two-process model into one Windows daemon.
- **Default chord `Ctrl+Shift+Alt+<key>`** (`MOD_CONTROL|MOD_SHIFT|MOD_ALT|MOD_NOREPEAT`), **user-configurable**. Rationale: Windows has no free `Cmd`-equivalent; every two-modifier combo is claimed (Ctrl+Alt=AltGr; Win=OS-reserved incl. Win+L lock; Ctrl+Shift=terminal shortcuts; Alt+Shift=input-language switch). Three modifiers clears all uniformly; power users rebind.
- **Collision handling:** `RegisterHotKey` returns **FALSE (0)** with `GetLastError() == 1409 (ERROR_HOTKEY_ALREADY_REGISTERED)` when another app owns the chord - a precise, detectable failure (not a silent success). Check it; `doctor` reports collisions and suggests an alternate chord.

### Supervisor - Task Scheduler (XML) + Python supervisor (`platform/windows/supervisor.py`)
- **No-admin autostart requires the XML path, not bare `schtasks` flags.** `install()` writes a hand-authored Task XML with a `<LogonTrigger>` scoped to the **current `UserId`** (a per-user logon task needs no admin; the bare `/sc onlogon` default creates an any-user trigger that *does*) and a `<Settings><RestartOnFailure>` block, then `schtasks /create /xml <file> /tn <name>` (and `/delete`, `/query` for uninstall/status). `schtasks.exe` is used (not PowerShell) to avoid execution-policy traps.
- **The Python supervisor is the sole daemon-restart authority.** Task Scheduler launches a thin `pythonw.exe` supervisor (no console) at logon; the supervisor `Popen`-restarts the *daemon* with exponential backoff. Task Scheduler's `RestartOnFailure` only relaunches the *supervisor* if it dies - it is the outer keep-alive, not the daemon's restarter. `restart()` forwards to the supervisor.
- Windows Service (pywin32, needs admin) is reserved for a future managed/IT-deployment mode only.

### Python resolution & hooks
- Interpreter discovery (Windows backend, net-new): `py -3` → `python` on PATH → **skip the Store stub** (`WindowsApps` path / exit 9009). The **hook command uses this resolved interpreter path** (or `py`), not a bare `python` - otherwise the hook hits the same Store-stub trap; `doctor` verifies the hook's interpreter isn't the stub.
- Claude Code hooks switch to **exec form** in the plugin **`hooks.json`** manifest (`command: "<resolved python>"`, `args: [".../hooks_entry.py", ...]`) - `hooks_entry.py` (the intake script) is **unchanged logic**; only its invocation form changes. Shell-form hooks die on Windows (`CLAUDE_PLUGIN_ROOT` backslash corruption, `python3` not found). A `.gitattributes` forces LF on the `.py` hook so shebangs survive.

---

## 4. Screen-reader coexistence (NVDA / JAWS / Narrator)

**Decision: v1 covers all three readers via coexistence-by-configuration. Sonari keeps its own voice; it never routes through a reader.** (Unchanged from the brainstorm.)
- Coexistence-by-config is the **only** uniform approach: route-through can't cover Narrator (no external speak API), works only for NVDA (`nvdaController_speakText`, no completion callback) + JAWS (`FreedomSci.JAWSAPI`, commercial), and **sacrifices the Phase 2.1 substrate** (sentence-granular heard-marker, voice continuity). Active ducking isn't uniformly feasible (no reliable "a reader is speaking now" signal).
- **v1:** Sonari owns Claude's-output voice; the user configures their reader not to also auto-announce the Claude pane (documented per reader). No automatic ducking.
- **Seam-ready for later:** screen-reader detection (process check `nvda.exe`/`jfw.exe`/`Narrator.exe` + the legacy `SPI_GETSCREENREADER` flag) and an **opt-in** future "speak through NVDA" mode (degrades the heard-signal to an SSML-`<mark>` approximation). JAWS/Narrator stay config-only.

---

## 5. Milestones (this spec decomposes into separate implementation plans)

**M0 - prerequisite (not code):** stand up Win10/11 **with working audio** (physical PC or a VM with sound passthrough - *not* a headless cloud instance). Required before M2's listen-tests.

- **Milestone 1 - Platform-seam refactor on macOS, zero behavior change (+ AF_UNIX → TCP).** Introduce `platform/` + the four interfaces; move macOS code behind `platform/macos/*`; migrate IPC to localhost TCP on macOS. **Proves** the seam + transport in production on the Mac Nima already uses; any regression caught immediately. *De-risks the whole port with no Windows in the loop.*
- **Milestone 2 - Windows MVP: OneCore speech pipeline, no hotkeys.** `platform/windows/{tts(OneCore),earcon,supervisor}.py` + Task-Scheduler-XML autostart, exec-form `hooks.json`, `py`-launcher resolution, bundled `.wav` earcons, `.gitattributes`. **Proves** a blind Windows user hears Claude. High-value 90%, no UIPI risk.
- **Milestone 3 - Windows hotkeys + parity.** ctypes `RegisterHotKey` in-process, VK_/MOD_ keytables, `Ctrl+Shift+Alt` default + configurable, collision detection in `doctor`, the bounded UIPI/elevation documentation. **Proves** the one wall-risk layer, bounded.
- **Milestone 4 - opt-in Piper neural engine.** `sonari voices install` (hash-pinned MIT Piper binary + public-domain ljspeech model fetched to `~/.sonari/`), the Piper `TtsBackend`, `doctor`/install discovery, license disclosure. Purely additive behind the seam - *reorderable* (can precede M3 if neural is prioritized), and the lowest-risk milestone to defer if needed.

---

## 6. Verification strategy

- **Headless / automated (CI, Python 3.9 + 3.13):** core **logic** tests unchanged. **Backend contract tests** exercise each interface with OS calls mocked (`speak()` returns the right bool on completion vs interrupt; the supervisor builds the right Task XML + `schtasks` argv; the keymap resolves to the right VK/MOD codes; the transport enforces the token). A TTS smoke test synthesizes to a temp WAV and asserts plausible duration without a speaker.
- **Tests that MUST be rewritten/moved in M1 (the AF_UNIX→TCP + seam fallout - *not* "unchanged"):**
  - *Transport:* `test_paths::test_socket_connectable_*` and the `SOCKET_PATH.name == 'speechd.sock'` assertion; `test_daemon_loop` UNIX round-trips (`_make_unix_socket_daemon`, `test_handle_conn_ping/status_round_trip`); `test_client_send` UNIX reply server; `conftest` + `test_cli_uninstall` `SOCKET_PATH` artifact handling → all re-assert against TCP (127.0.0.1:port, token auth, PID-liveness lockfile).
  - *macOS-backend tests move with the code:* `test_cli_hotkeyd` (patches `cli._hotkeyd_plist`/`_build_hotkeyd`/`_launchctl`/`install`/`uninstall` → now `platform.macos.supervisor/hotkeys.*`); the `_KEYCODE_DISPLAY`/`_MOD_DISPLAY` ↔ `keymap.KEY_CODES/MOD_MASKS` cross-check (tables relocated); `test_hotkeyd_swift` stays macOS-only.
- **Human listen-test (⚠), on the Windows box (M0):** voice quality + latency (OneCore *and* Piper), mid-utterance `stop`/`skip`, all five hotkeys in a real terminal, earcons, and **no double-talk** with a running screen reader. Not headlessly verifiable - the build **escalates the unverifiable to Nima**, per the Phase 2 mandate.
- **Residual gap (stated honestly):** Nima is low-vision (magnifier-first), so his listen-test validates the low-vision path but only approximates the fully-blind + NVDA experience. Closing that needs a blind NVDA user (Nima's Windows friends) before GA - a pre-GA acceptance step, not a milestone blocker.

---

## 7. Locked decisions

| Decision | Choice | Why |
|---|---|---|
| Install privilege | **Zero-admin** | Matches macOS LaunchAgent; a UAC prompt is an accessibility regression. |
| IPC transport | **localhost TCP, unified both OSes, mandatory token** | One codepath; deletes a platform fork; kills the 104-char socket bug. CPython doesn't expose AF_UNIX on Windows. Loopback has no ACL → token required. |
| Hotkeys | **ctypes `RegisterHotKey`, in-process, no toolchain** | Exact Carbon analogue; eliminates the Swift/swiftc/grant machinery. |
| TTS default | **OneCore via PyWinRT** | Zero bundled binaries, offline, true to ship-nothing ethos. Functional-not-natural. |
| TTS neural | **opt-in Piper, fetched not vendored** (MIT binary, public-domain ljspeech default) | Real offline neural without taxing the default install or the repo; complexity contained behind the seam + one fetcher. Code quality preserved. |
| Default chord | **`Ctrl+Shift+Alt+<key>`, configurable** | Only chord clearing AltGr / Win-reserved / terminal / layout-switch collisions uniformly. |
| Windows floor | **Win10/11 desktop (1803+; rate needs 1709+)** | Required by PyWinRT + modern flags; no Server/headless; no native arm64 Piper. |
| Phasing | **Seam → OneCore speech → hotkeys → opt-in Piper** | Ships value early; isolates the wall-risk layer; neural is additive. |
| Screen readers | **Coexistence-by-config for all three** | Only uniform approach; preserves the Phase 2.1 substrate. |

---

## 8. Landmines / risk flags (ranked)

1. **UIPI elevation gap - hotkeys (only un-shippable-wall candidate, *conditional*).** If Claude Code runs **as Administrator** while the daemon runs at normal integrity, Windows silently blocks `RegisterHotKey`/`WM_HOTKEY` *delivery* into that elevated foreground window - the hotkey never fires. Only when elevated; speech unaffected. **Mitigation:** document "don't run Claude Code as Administrator (for hotkeys)"; `doctor` compares the terminal's integrity level vs the daemon's and warns. Surface before M3 commits; don't let planning downgrade it to "edge case."
2. **OneCore empty-`AllVoices` on minimal/non-en SKUs.** Stripped enterprise images or non-English locales may return zero/one voice. **Mitigation:** the §3 fallback chain (name → DefaultVoice → clear install error), never a crash.
3. **Secure Desktop (transient).** UAC prompts / Ctrl+Alt+Del / Hello get no hotkey or TTS (same family as macOS Secure Keyboard Entry). Transient, unavoidable from user-space - degrades, doesn't block. Document.
4. **`RegisterHotKey` first-registrant-wins.** PowerToys / Terminal `globalSummon` / AutoHotkey may own the chord → `RegisterHotKey` returns 0, `GetLastError()==1409`. Detectable; `doctor` reports; the three-modifier default is low-collision.
5. **Piper engine frozen + no arm64 + model hosting.** MIT binary unmaintained since Nov 2023 (pin+hash; migrate if a standalone exe reappears); no win-arm64 (x64 emulation; document); models from HuggingFace (cache locally, pin+hash).
6. **`winsound` single-channel / WAV-only.** Bundle `.wav`; always `SND_ASYNC`; keep earcons short (rapid cues truncate).

*(Notably absent - and deliberately so: the PowerShell `-Command` quoting/escaping + execution-policy landmines. We use PyWinRT (not PowerShell) for TTS and `schtasks.exe` (not PowerShell) for task registration.)*

---

## 9. Open assumptions (→ resolved by the build/technical agents, verified by listen-test)

- The exact PyWinRT projection package set to pin, and the thread-bridge in the OneCore backend (MediaEnded → `threading.Event` → unblock `speak()`), including holding the stream ref against GC.
- The Piper subprocess lifecycle: long-lived (one process fed many lines) vs per-utterance spawn, and the raw-PCM → playback path on Windows (which stoppable player; reuse the MediaPlayer audio sink or a temp-WAV).
- The exact Task XML (`<LogonTrigger UserId>` + `<RestartOnFailure>`) that a **standard (non-admin)** user can import via `schtasks /create /xml`, plus the supervisor backoff policy. *Verify non-admin import on the box.*
- The Windows `SupervisorBackend.launch_spec()` spawn flags (`DETACHED_PROCESS|CREATE_NO_WINDOW` under `pythonw`) and where the Windows launcher entry lives (the `~/.local/bin` analogue).
- That the existing stable session-id + hook intake (`hooks_entry.py`) carries over unchanged on Windows (the hook contract is OS-agnostic stdin JSON; only the invocation form changes).
```
