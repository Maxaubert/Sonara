# Sonara Phase 0: stabilise, clean up, document

Spec and implementation plan, 2026-10-01. Source: `docs/history/audits/2026-10-01-phase0-audit.md` (71 verified findings + architecture and docs reviews).

## Goal

A stable, well-structured, documented Sonara before any embedding work (PrismTerminal phases 1 and 2 are out of scope). Done means:
- CI green on every PR (ruff + full unit suite), hermetic tests, a lean CLAUDE.md, current README/PRIVACY/architecture docs.
- Chatterbox gone, dead features gone, `daemon.py` split into focused modules.
- Every confirmed high/medium finding fixed or explicitly deferred with an issue.
- Protocol ready for an embedded player (SUBSCRIBE, SPEAK, host tab id), without building the player.

## Status (2026-10-01)

**Phase 0 is complete pending merges.** Every PR below is open on Maxaubert/Sonara; none is merged yet. #129, #131 and #145 target main. From #146 on the PRs are stacked: each targets the previous PR's branch, in table order, and is retargeted as the one below it merges.

| Item | Issue | PR |
|---|---|---|
| Up always restarts the turn | #128 | #129 |
| Never strand ducked apps | #130 | #131 |
| P1 repo hygiene + CLAUDE.md | #132 | #145 |
| P2 CI, release, ruff, hermetic tests | #133 | #146 |
| P3 remove Chatterbox | #134 | #147 |
| P4 remove dead features | #135 | #148 |
| P5 config single source of truth, earcons | #136 | #149 |
| P6 pipeline never loses the last message | #137 | #150 |
| P7 summary robustness | #138 | #151 |
| P8 install, launcher, hooks | #139 | #152 |
| P9 runtime edges | #140 | #153 |
| P10-P13 daemon split 1-4 | #141 | #154, #156, #157, #158 |
| P14 installer package | #142 | #159 |
| D8 hotkey reset and AltGr warning (Win+Alt default reverted) | #160 | #162 |
| P15 protocol for embedded players | #143 | #163 |
| Review follow-ups | #161 | #164 |
| P16 README, PRIVACY, architecture, embedding research (0.8.2) | #144 | #165 |
| Final verification findings (0.8.3) | #166 | branch `fix/166-final-verification`, stacked on #165 |

The fresh-eyes review over the whole diff and the audit re-run (see Verification) ran on 2026-10-02; their findings are fixed in #166. Open after Phase 0: removing the v1 singleton mutex probe in the first release after 0.8.3 is published (`platform/windows/singleton.py`, TODO #161). No release has been published yet, so users still run daemons that hold the v1 mutex. Next: PrismTerminal integration, researched in [embedding-research.md](embedding-research.md).

## Product rules this plan encodes

1. **One message, always the last.** Sonara reads the latest turn. Up restarts it (#128, PR #129). No reading of older turns. Nothing may silently drop the latest turn (fixes: seeded-channel wipe, parked digest, lost settle seq). A SPEAK session follows the same rule: each SPEAK replaces its turn (#166).
2. **Never strand other apps' audio.** (#130, PR #131.)
3. **Kokoro + Windows native are the only voices.** User voice clips are never auto-deleted.

## Decisions (approved 2026-10-01)

| # | Decision | Default |
|---|---|---|
| D1 | Ctrl+Alt+Left/Right (step paragraph back/forward inside the current turn) | **Remove** (one message, always the last; Up restarts it). |
| D2 | Dead hotkey features with no key bound (jump-to-decision, catch-up, re-read options, cycle verbosity, the old `audio-control` shim) | **Delete** with their tests (#40 upstream list). |
| D3 | Chatterbox chunked player | **Delete.** Kokoro never used it. |
| D4 | Chatterbox leftovers on disk (8.7 GB venv + model cache) | `sonara doctor` reports them and `sonara cleanup` removes them on request. `voices/chatterbox/` clips are never touched. |
| D5 | Git remotes | Rename locally: `origin` (upstream nimkimi/sonari) to `upstream`, `sonara` (your fork) to `origin`; `gh repo set-default Maxaubert/Sonara`. Nothing is pushed upstream. |
| D6 | Versioning | Bump to 0.6.0 in the CI PR (manifests in sync), then patch/minor per PR from then on. `release.yml` publishes `v<version>` on push to main. |
| D7 | Native Windows voices fail to synthesise on this PC (`FileNotFoundError`, all OneCore voices) | Investigate in the runtime-edges PR; if it is machine-only, document it in doctor output. **Diagnosed 2026-10-01 (#140): machine-only.** WinRT lists David, Zira and Mark, but synthesis fails even with no voice set: `%WINDIR%\Speech_OneCore\Engines\TTS\en-US` holds only `MSTTSLocEnUS.dat`, the voice data files are missing, and `HKLM\...\Speech_OneCore\Voices` has no `Tokens`. Doctor's "Windows voice" row now synthesises for real and names the repair (re-add English (United States) under Settings > Speech, or elevated `DISM /Online /Add-Capability /CapabilityName:Language.TextToSpeech~~~en-US~0.0.1.0`). Repro: `python -m pytest -m live_windows tests/test_win_tts_live.py`. A new program lists no OneCore voices at all on this PC (per-program voice list, found in M3 #179; see the runtime plan's interface notes). Also found: this PC's layout is Norwegian, so Ctrl+Alt+M (mute) is AltGr+M and eats µ (doctor's AltGr row, E16). |
| D8 | Default hotkey chord (#160, user decisions 2026-10-01 and 2026-10-02) | **Ctrl+Alt stays** (Up/Down/M/P, unchanged from 0.6.x). **Reverted 2026-10-02:** Win+Alt had been chosen, but a RegisterHotKey probe on the user's PC returned ERROR_HOTKEY_ALREADY_REGISTERED (1409) for Win+Alt+Up, Down, M and P (Windows, Game Bar, PowerToys own them), and the user wants the arrow keys, M and P, so the chord falls back to Ctrl+Alt. The AltGr collision (Norwegian Ctrl+Alt+M eats the micro sign) is accepted as a warning: doctor's AltGr row is a [warn] (exit 0) and the settings page warns next to the binding, both saying the fix is to rebind that key with Win (Win+Alt+Home/End or Ctrl+Win+Up/Down were free on the probe). Kept from the Win+Alt attempt: 'Reset hotkeys to defaults' button and `sonara keymap --reset`, Home/End/PageUp/PageDown bindable, Win modifier with Win-first labels. Version 0.7.0. Original 2026-10-01 record: **Win+Alt** instead of Ctrl+Alt, which is AltGr on European layouts (Norwegian Ctrl+Alt+M ate the micro sign). Windows 11 documents Win+Alt+B (HDR), D (date/time), H (voice typing), K (mic mute), Up/Down (snap top/bottom half), Enter (taskbar settings) (support.microsoft.com 'Keyboard shortcuts in Windows', read 2026-10-01); Game Bar adds Win+Alt+G/R/T/M/PrtScn. A RegisterHotKey probe on the maintainer's PC (2026-10-01, PowerToys running) found Win+Alt+B/D/F/G/K/M/P/R/T/W/Y, 0-9 and all four arrows taken; Home/End/PageUp/PageDown, A/C/E/H/I/J/L/N/O/Q/S/U/V/X/Z free. Defaults: restart = Win+Alt+Home, flush = Win+Alt+End, mute = Win+Alt+S, next session = Win+Alt+N. Existing keymap.json files keep their bindings; `sonara keymap --reset` or the settings page's 'Reset hotkeys to defaults' switches. Doctor's AltGr row is a [warn]. Version 0.7.0. |

## Delivery model

- One issue, branch (`type/issue-slug`) and PR per item below. As shipped, the first three target main and the rest form one stack, each PR on the previous one's branch (see Status); each is retargeted as the one below it merges.
- Each PR: TDD for every bug fix, full suite + ruff green, deployed to `~/.sonara/app` for hands-on testing when user-facing, then "merge?" to you. I keep building the next PRs while earlier ones wait for approval.
- Execution via workflows: implementer per PR in its own worktree, then an independent reviewer pass before the PR is opened.

## PRs

### Wave 1: foundations (unblocks everything)

**P1. chore: repo hygiene + CLAUDE.md** (docs/tooling only)
- Local: remote rename + gh default (D5); prune gone branches; remove stale worktrees (`.claude/worktrees/...` via `git worktree remove` once confirmed merged).
- Delete root clutter (`.py`, mangled task-report file, `.claire/`), untrack `.superpowers/`, extend `.gitignore`, fix `.gitattributes` (LF for `bin/sonara`, `bin/sonara-hook`).
- `git mv docs/superpowers/* docs/phase*.md` to `docs/history/`; `AUDIT-2026-07-31.md` and the new audit to `docs/history/audits/`; update the ~20 code comments that cite spec paths.
- New lean `CLAUDE.md` (remotes, commands, deploy-drift and safe redeploy, architecture map, conventions).
- CONTRIBUTING fixes (Windows paths, branch naming, CI).
- Close fork issues already fixed in code (#14-#17, #19, #21, #115-#118 after a quick verify), #10 as won't-do.

**P2. ci: CI, release, ruff, hermetic tests, 0.6.0**
- `ci.yml`: ruff + pytest on push/PR. As shipped: windows-latest only, unit suite on Python 3.9 and 3.12, ruff and a `SONARA_DEBUG_LOCKS=1` run on 3.12. No ubuntu job: the suite is Windows-only, and `test_py39_compat.py` plus the 3.9 job cover the syntax check.
- `release.yml`: on push to main, tag + GitHub release `v<version>`, refusing an existing version.
- ruff in `dev` extra (py39 target), fix what it finds. As shipped: `E9,F,B`; `UP` (pyupgrade) was left out as ~270 mechanical rewrites with no bug signal (see `pyproject.toml`).
- Make `test_win_tts.py` hermetic; real-OneCore checks behind a `live_windows` marker.
- `pyproject`: version 0.6.0 (plus plugin.json, marketplace.json), maintainers, readme, urls, `requirements-kokoro.txt` in package data; `_copy_app` ignores `__pycache__`.

### Wave 2: remove weight

**P3. refactor: remove Chatterbox** (scope per the removal plan)
- Delete 3 modules, requirements, `tools/clean_voice_clip.py`, 7 test files; strip daemon/tts/cli/config/paths/webui/settings.html/previews.
- Migration: a saved Chatterbox voice (config `voice`, `cue_voice`, session prefs) maps to `af_heart` on load; `chatterbox_*` keys are stripped.
- D4 cleanup via doctor/cleanup; README/PRIVACY sections removed; settings-page e2e tests rewritten for the single-engine picker.

**P4. refactor: remove dead features and macOS leftovers**
- D2 features + `_options` store + their protocol types; dead helpers (`_resume`, `_drop_pending`, `_audio_pause_on`, `nth_last_message`, `HOTKEYD_*`, `keymap.write_resolved`, `test_hotkeyd_contract.py`).
- Redundant hook registrations (`idle_prompt`, duplicate matchers). Stale protocol/daemon docstrings.

### Wave 3: correctness (TDD, each fix with a regression test)

**P5. fix: config single source of truth**
- `config_schema.py` (default, validator, live-apply per key) used by daemon, webui, CLI.
- Persist only user-changed keys (M7) and no absolute earcon paths (M8), so new defaults reach existing installs.
- Add `summary_settle_ms` default; `duck_level` fallback 30.
- Earcons in sync (H2): add nav, nav_edge, session_change, summary_failed wavs; drop unused plan/ready; set-equality test against every `_earcon()` call.

**P6. fix: speech pipeline never loses the last message**
- `SessionChannel` API (`insert_at`, `truncate_pending`, `skip_to_end`) keeping the seeded/gen/has_decision rules; daemon stops editing `ch.items` directly (fixes the confirmed "rate change wipes the unread short turn" bug).
- Rate/verbosity cues go through the control channel (F6).
- #69: a background session's FLUSH no longer un-pauses the foreground voice (F1).
- Parked short background digest can't resurrect an ended session (F2).
- STOP also cancels settle timers, in-flight and parked digests (M3/F8).
- Speaker cancel/abandon race (F4); router private-field writes replaced by methods (M13); `_replay` sets `has_decision` (L); preamble race (L).
- Hook messages from one event sent on one connection, in order (`client.send_many`).

**P7. fix: summary pipeline robustness**
- Watchdog lands a hung digest slot (M1); summarizer timeout enforced for `.cmd` engines (F5); `_settle_fire` catch-all and the `_settle_gen` pop (L); `_pending_decision` overwrite (F7).
- H1: daemon spawned with `cwd=~/.sonara`; `which('claude')` restricted to PATH, no bare-name fallback.

**P8. fix: install, launcher and hooks**
- H3/E6: install never leaves Sonara stopped on failure; guard when run from the deployed copy.
- H4: settings.json hook template generated from `hooks/hooks.json` (+ set-equality test).
- E1-E5: bootstrap/shim interpreter selection (uv Python PEP 668, Store `python` stub, PS 5.1, ASCII path record M14).
- E7 (3 s hook block after shutdown), E8 (uninstall undone by next hook), E9 (console flashes from probes), #127 (`_copy_app` rename rollback), L-xml escaping, log rotation.

**P9. fix: runtime edges**
- Kokoro: download timeout outside the engine lock, status cue, cached failure (M2/E11, upstream #53); no bogus "Kokoro unavailable" on default installs (E12); `voices install` stops the daemon first and never deletes a working venv (E10).
- WinRT synth lock (M9); preview doesn't cut live speech (M6); hotkey start failure logged and spoken (M4); single-instance mutex scoped to the user (M11); settings page rejects modifier-less hotkeys (E13); CLI errors instead of tracebacks (E18); D7.

### Wave 4: structure

**P10-P13. refactor: split `daemon.py`** (pure moves, suite green after each step)
- P10: `daemon/` package; pure modules (`decision_text`, `platform/windows/process`, `install_record`, `lifecycle` so the client stops importing the daemon); `DigestReorderBuffer`; `setup_health`.
- P11: `hotkeys`, `audio`, `cues`, `server`.
- P12: `summary/pipeline`, `playback`.
- P13: `handle_message` becomes table dispatch; per-session state registry replaces the 20 hand-written pops; tests move to public seams.

**P14. refactor: installer out of cli.py**
- `sonara/install/` (install, uninstall, copy-app, deps) and `install/claude_hooks.py` (from supervisor.py); `cli.py` becomes argparse only. Platform seam leaks fixed (ducking/pausing via `get_platform()`, transport Win32 code under `platform/windows/`).

### Wave 5: embedding-ready and documented

**P15. feat: protocol for embedded players**
- `SUBSCRIBE` state stream (now playing, queue, paused, mute, volume) from `daemon/server.py`; STATUS returns the same snapshot.
- `SPEAK` (text, source, tab) with queue-of-one semantics.
- `PRISM_TAB_ID` passed through the hook as `host_tab`.
- Golden test fixtures for cleaner/assembler text rules (for a later TypeScript port).

**P16. docs: README, PRIVACY, architecture**
- README rewrite (readme skill): current model (channels, last message only), settings page, Kokoro, accurate CLI list, one uninstall story.
- PRIVACY: persisted digests and session files, Codex egress, contact.
- `docs/architecture.md` (data flow, threads, persisted state, platform seam, protocol).
- `docs/plans/embedding-research.md`: the PrismTerminal integration research, with its Phase 0 prerequisites marked done.

## Out of scope

PrismTerminal player and native port (phases 1-3), installer bundling of Python/Kokoro, macOS, upstream issues.

## Verification

Per PR: ruff, full suite, e2e when UI changes, deployed build smoke on this PC for user-facing changes. At the end: a fresh-eyes review workflow over the whole diff from 8d90005 and a re-run of the audit dimensions to confirm the finding list is closed.
