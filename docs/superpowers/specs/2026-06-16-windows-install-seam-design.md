# Sonari — Windows Install via the Platform Seam — Design Spec

> **projectType:** claude-plugin
> **Status:** approved design (live brainstorm, 2026-06-16). Consumed by writing-plans.
> **Scope:** complete the deferred install/uninstall move behind the platform seam so
> `sonari install` performs the correct **Windows** setup, with **no macOS artifacts**, while
> macOS behavior is preserved. Decisions confirmed with the user: (A) thin-cli / fat-backend
> orchestration; (B) Windows hooks written to **user-scope** `~/.claude/settings.json`.
> **Grounding:** recon of the current `cli.py` (still 100% macOS), the as-built
> `platform/{macos,windows}` backends, the Phase-3 Windows design
> (`2026-06-10-sonari-phase3-windows-design.md`), the M2 plan/acceptance docs, and a
> Claude-Code-docs research pass on Windows hook resolution (v2.1.154).

---

## 1. Problem

`sonari install` runs the **macOS** code path on every platform. On Windows it:

- writes a LaunchAgent plist to `~/Library/LaunchAgents/...` and calls `launchctl` (returns 1);
- pre-checks `swiftc` / Xcode CLT and reports them "not found";
- places a **bash** launcher at `~/.local/bin/sonari` (not runnable on Windows);
- resolves Python with the macOS resolver, recording the **Microsoft Store stub**
  (`...\WindowsApps\python3.exe`) in `install.json` instead of a real `pythonw.exe`;
- prints the raw WinRT `VoiceInformation` object repr for the voice line.

Root cause: `cli.py` imports the macOS backends at module top, builds a module-level
`_mac_sup = MacSupervisorBackend()`, and `install()` / `uninstall()` / `doctor()` call macOS
mechanisms **directly**, never going through `get_platform()`. The platform seam exists and the
Windows backends are built, but `cli.py` bypasses it. `MacSupervisorBackend.install()` is a `pass`
stub commented *"filled in when Task 7 moves install"* — the intended move was deferred and never
completed.

This spec completes that move for **both** platforms.

---

## 2. Goals / Definition of Done

- `sonari install` on Windows: resolves a real `pythonw.exe` (Store-stub-avoiding), copies the
  runtime to `~/.sonari/app`, writes a correct `install.json`, registers the **Task Scheduler**
  autostart task (no UAC), writes **exec-form** Sonari hooks into `~/.claude/settings.json`,
  places a Windows `sonari` launcher, prints a clean voice **name**, and prints
  Windows-appropriate next steps. **No** LaunchAgent / `launchctl` / `swiftc` / bash-launcher
  output appears.
- `sonari uninstall` on Windows removes the Task, removes **only Sonari's** hook entries from
  `~/.claude/settings.json`, and removes the Windows launcher — preserving `config.json` /
  `keymap.json` (parity with the macOS uninstall contract).
- `sonari doctor` reports Windows rows via `WinSupervisorBackend.doctor_rows()`; no macOS rows
  leak; the portable "hooks installed" row is OS-aware.
- **macOS behavior is unchanged.** The full existing suite stays green on macOS; the macOS
  install/uninstall/doctor produce identical artifacts and the same stdout **line set**. (One
  accepted deviation: because launcher placement now lives in `supervisor.install()` while hotkeyd
  install stays a separate `hotkey.install()` step, the relative **order** of the `Placed/Removed
  launcher` line vs the hotkeyd lines differs from the pre-refactor sequence. The lines and
  artifacts are identical; only their ordering during a one-time install/uninstall shifts. A
  stdout-lock test asserts the macOS line set survives.)
- `cli.py` contains **no** `import sonari.platform.macos...` and no `sys.platform` branch — every
  OS-specific operation is dispatched through `get_platform()`.
- The voice contract is honored: `TtsBackend.best_voice() -> str` on both platforms.
- Out of scope (explicit): Windows **hotkeys** (M3 — the stub stays), Piper neural TTS (M4), and
  the separate daemon "silent-mute on a failed utterance" reliability bug (tracked independently).

---

## 3. Architecture — thin cli, fat backend

`cli.py` owns the **shared install lifecycle** and dispatches OS-specifics to the platform
backend. Each backend owns its OS autostart + hooks + launcher.

```
cli.install()  (OS-agnostic orchestration)
  1. paths.ensure_sonari_dir()
  2. python  = platform.supervisor.resolve_python()        # FATAL if None
  3. app_dir = _copy_app(plugin_root)                      # shared (stays in cli)
  4. keymap.write_default_keymap_if_absent(); write_resolved()   # shared
  5. _write_install_record(python, ver, plugin_root, app_dir, plugin_version)  # shared
  6. platform.supervisor.install(python, app_dir)          # OS autostart + hooks + launcher
  7. ok, detail = platform.hotkey.install(...)             # macOS: builds hotkeyd; win: (False,"M3")
  8. voice = platform.tts.best_voice()  ->  print name     # str on both OSes
  9. platform.supervisor.post_install_notes()              # OS-specific next steps (printed)
```

`get_platform()` is called once and cached in a module-level accessor (`_platform()`), replacing
the `_mac_sup` singleton and the top-level macOS imports.

### 3.1 `cli.py` de-macOS-ification

- **Remove** the top-level `from sonari.platform.macos.hotkeys import ...` and
  `from sonari.platform.macos.supervisor import ...` and the `_mac_sup = MacSupervisorBackend()`
  line. Remove the module-level macOS `LAUNCH_AGENT_LABEL/PATH` constants and the macOS-only
  delegating shims (`_plist`, `_launchagent_plist`, `_launchctl`, `_build_hotkeyd`,
  `_place_launcher`, `_launcher_path`, `_local_bin_on_path`, `_remove_launcher`,
  `_daemon_shim_path`) — they move with the orchestration into the macOS backend.
- `doctor()` → `rows = _platform().supervisor.doctor_rows()` (was `_mac_sup.doctor_rows()`).
- `_combo_label()` → `_platform().hotkey.display_combo(modifiers, key_code)` (drop the imported
  macOS `_MOD_DISPLAY` / `_KEYCODE_DISPLAY`).
- `_resolve_python()` / `_probe_python_version()` shims → `_platform().supervisor.resolve_python()`
  / `._probe_python_version()`.
- The portable doctor row that currently hard-codes the repo `hooks/hooks.json` path becomes an
  OS-aware "hooks installed" check sourced from the supervisor (macOS: plugin `hooks/hooks.json`
  exists; Windows: `~/.claude/settings.json` contains Sonari's hook entries).

### 3.2 macOS backend — fill the stub (behavior-preserving)

Move the existing `cli.install()` / `cli.uninstall()` macOS body **verbatim** into
`MacSupervisorBackend.install(python, app_dir)` / `.uninstall()` (LaunchAgent plist write +
`launchctl unload/load`, `~/.local/bin` launcher placement, removal on uninstall). The hotkeyd
build already lives in `MacHotkeyBackend`; `cli` calls `platform.hotkey.install(...)`.
`post_install_notes()` prints the existing macOS next-steps (`claude --plugin-dir`, `export PATH`
when `~/.local/bin` is off PATH). **Net macOS behavior: identical** — verified by the existing
macOS install/doctor/uninstall tests (updated only where they patched `cli._launchctl` etc. to now
patch the backend methods, as anticipated by the Phase-3 spec §6).

### 3.3 Windows backend — `WinSupervisorBackend.install/uninstall`

`install(python, app_dir)` (extends the current task-only implementation):

1. **Task Scheduler** — `task_install(pythonw=python, supervisor_py=<app_dir>/.../supervisor_loop.py)`
   (already implemented; `/f` overwrite makes reinstall idempotent).
2. **Hooks** — build exec-form hook config via `build_hooks_json(python, hook_py)` where `hook_py`
   is the absolute path to the plugin's `bin/sonari-hook` (a pure-Python `#!/usr/bin/env python3`
   script — `pythonw.exe sonari-hook <Event>` runs it directly). **Merge** it into
   `~/.claude/settings.json` (see §3.5).
3. **Launcher** — write a `sonari.cmd` shim into `~/.local/bin` (created if absent) that invokes
   `"<python's console exe>" -m sonari.cli %*` with `PYTHONPATH=<app_dir>`. (`~/.local/bin` is the
   cross-OS launcher home Sonari already owns; a `.cmd` is the Windows-runnable analogue of the
   macOS bash wrapper.) `is_installed()` continues to gate on the Task existing.

`uninstall()`:

1. `task_uninstall()` (delete the Task).
2. **Un-merge** Sonari's hook entries from `~/.claude/settings.json` (§3.5) — never touch
   non-Sonari hooks or other settings keys.
3. Remove `~/.local/bin/sonari.cmd`.
4. Preserve `config.json` and `keymap.json`.

`post_install_notes()` prints Windows next steps (enable the plugin for slash commands;
`sonari doctor`; that hotkeys arrive in M3).

### 3.4 Voice contract fix (`WinTtsBackend`)

`best_voice()` currently returns a `VoiceInformation` **object**, violating the ABC
(`best_voice() -> str`) and causing the install repr. Split:

- `_best_voice_info(lang_prefix="en-US")` → the `VoiceInformation` object (the current body);
  used internally by `run()` / `_resolve_voice()`.
- `best_voice()` → `self._best_voice_info().display_name` (a `str`), honoring the ABC.

Update the one test that asserts `best_voice().id` to use `_best_voice_info()`. `cli`'s voice line
then prints a clean name on both platforms with no platform knowledge in `cli`.

### 3.5 `~/.claude/settings.json` hook merge (the one net-new helper)

A small, well-bounded helper (in the Windows supervisor module, since it's Windows-install glue):

- **Read** existing `~/.claude/settings.json` (tolerant: missing/empty/invalid → `{}` base;
  never clobber a valid file you can't parse — abort with a clear error instead).
- **Merge** Sonari's exec-form hook entries under `settings["hooks"][<Event>]`, **appending**
  (Claude Code merges plugin + settings hooks, so we add alongside, not replace). Idempotent:
  re-running install first removes any prior Sonari entries (identified by an entry whose
  `command`/`args` reference the absolute `bin/sonari-hook` path) then re-adds the current ones,
  so a reinstall after a path change self-heals.
- **Write** back with `indent=2`, preserving all other keys and hooks.
- **Removal** (uninstall) drops exactly the Sonari-identified entries and prunes now-empty event
  arrays / an empty `hooks` object.

**Ownership marker:** Sonari entries are recognized by the `sonari-hook` path in
`command` or `args[0]`. This is reliable (no other tool invokes that script) and needs no extra
metadata in the file.

**Scope:** user-level `~/.claude/settings.json` (confirmed) so Sonari speaks in every session on
the machine — the parity match for the macOS LaunchAgent. A future `--project` flag (write to
`.claude/settings.json`) is out of scope.

### 3.6 Hook double-fire interaction (documented constraint)

The plugin's committed `hooks/hooks.json` is **shell-form** and macOS-only; on Windows it cannot
spawn the Python hook. Because Claude Code **merges** plugin-manifest hooks with settings.json
hooks, if both were active on Windows each event would fire twice (the shell-form attempt failing).
Therefore, on Windows, Sonari's hooks come from `~/.claude/settings.json` **only**; the plugin
manifest hooks must not be active. The installer does not enable the plugin's hooks; this is called
out in the Windows acceptance notes (the exact "don't also enable manifest hooks on Windows"
guidance lives with M2-WINDOWS-ACCEPTANCE §7).

---

## 4. Testing strategy

- **macOS preservation (primary gate):** the full existing suite stays green. Install/uninstall/
  doctor tests that patched `cli._launchctl` / `cli.install` internals are repointed to the macOS
  backend methods (the move is behavior-preserving; artifacts/output identical).
- **cli dispatch (new, OS-mocked):** `doctor()` pulls rows from the platform supervisor under a
  win32 monkeypatch; `_combo_label` uses `hotkey.display_combo`; `install()` calls
  `supervisor.install` / `hotkey.install` / `tts.best_voice` in order (assert via fakes/spies).
- **Windows install (new, mock-based via the `_winfakes` harness):**
  - `WinSupervisorBackend.install` calls `task_install`, writes the launcher, and merges hooks.
  - settings.json merge helper: empty file, existing unrelated hooks preserved, idempotent
    re-install, uninstall removes only Sonari entries, invalid-JSON aborts safely.
  - `WinTtsBackend.best_voice()` returns a `str`; `_best_voice_info()` returns the object.
- **Deferred human-on-Windows acceptance** (extends `M2-WINDOWS-ACCEPTANCE.md`): the real Task
  registration with no UAC, the exec-form hook actually firing from `~/.claude/settings.json`, and
  the no-double-talk constraint of §3.6. Mock-green is **not** a Windows guarantee.

---

## 5. Risks / landmines

1. **Editing the user's global `~/.claude/settings.json`.** Mitigation: tolerant read, abort on
   unparseable file (never clobber), idempotent Sonari-only merge/removal, `indent=2` round-trip,
   and tests covering preservation of unrelated keys/hooks.
2. **Test churn from the backend move.** The Phase-3 spec already flagged that macOS install/
   hotkeyd tests move with the code; keep the macOS suite green at every step.
3. **`${CLAUDE_PLUGIN_ROOT}` vs baked paths.** The interpreter path is machine-specific, so install
   bakes the resolved `pythonw` + absolute `sonari-hook` path into the written hooks (not a
   `${...}` token). This is intentional and why install writes settings rather than shipping a
   cross-platform manifest.
4. **Store-stub regression.** Guarded by routing `resolve_python` through the Windows backend; a
   doctor row surfaces the resolved interpreter so a stub is visible.

---

## 6. Out of scope (tracked elsewhere)

- Windows global hotkeys (M3) — the `WinHotkeyBackend` stub stays; install prints "deferred".
- Piper neural TTS (M4).
- The daemon **silent-mute** reliability bug (speak loop dies on a failed utterance, leaving only
  earcons, no recovery until restart) — observed 2026-06-16; investigate separately.
