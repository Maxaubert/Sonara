# Sonari Phase 3 — Fresh-Install Smoke Checklist (self-contained, screen-off)

Run these on the real Mac to validate the **public, pip-free** install path. The
deterministic pytest suite (3.9 + 3.13) covers all install/uninstall/doctor logic
and the plist contents; this checklist covers what cannot be unit-tested: a real
fresh install on the system interpreter, both LaunchAgents loading, and live
speech/earcons/hotkeys. Reuses the structure of
`phase2-manual-smoke-checklist.md`. Do `(screen off)` items with the screen off.

---

## Pre-install (clean slate)

- [ ] **No editable sonari shadows the plugin.** Run
  `/usr/bin/python3 -c "import sonari"` — expect `ModuleNotFoundError` (or, if a
  dev install lingers, note it; `sonari install` will print cleanup guidance).
- [ ] **System python is 3.9+.** Run `/usr/bin/python3 --version` — expect 3.9.6
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
  `PYTHONPATH=<plugin>/src /usr/bin/python3 -m sonari.cli doctor` — expect it
  runs and the daemon lazy-starts via `bin/sonari-daemon`.
- [ ] **Launcher works in a fresh shell.** Open a new terminal and run
  `sonari status` (resolved via `~/.local/bin/sonari`) — expect daemon status.
- [ ] **speechd plist is correct.** `plutil -p
  ~/Library/LaunchAgents/com.sonari.speechd.plist` — expect ProgramArguments
  `[<abs python3>, -m, sonari.daemon]` and EnvironmentVariables PYTHONPATH =
  `<plugin>/src`.

## Live session (screen off)

- [ ] **Ready earcon + ordered narration.** Start a real `claude` session; hear
  the ready earcon, then prose in order, then decision earcons. (screen off)
- [ ] **All nine hotkeys.** Exercise Ctrl+Cmd+S/R/./D/L/]/[/V/O — each fires,
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
