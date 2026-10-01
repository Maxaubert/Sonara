# Sonari Phase 3.1 - Manual Clean-Room Verification Checklist

Run on a Mac with a clean profile, against the public marketplace
(`github.com/nimkimi/sonari`). Mirrors design spec §9.

- [ ] **Fresh marketplace install + on-ramp cue.**
  `claude plugin marketplace add nimkimi/sonari` → `claude plugin install
  sonari@sonari` → enable → start a session. Confirm: you hear Claude (lazy
  daemon) AND hear the "run /sonari:install" cue exactly once.
- [ ] **Eyes-free install.** Run `/sonari:install`; confirm each step is printed,
  then `/sonari:doctor` reports green (or only the expected `swiftc`/CLT FAIL).
- [ ] **Plist points at the stable copy.** After install, the speechd plist's
  `EnvironmentVariables.PYTHONPATH` is `~/.sonari/app` (NOT the cache), and
  `~/.sonari/app/sonari/__init__.py` exists.
- [ ] **Simulated version drift.** Install at one version, then start a session
  reporting a newer `plugin_version` (e.g. install 0.3.0, start reporting 0.4.0).
  Confirm: hear "Sonari was updated. Run /sonari:install" once; speech/hotkeys
  still work on the old copy before re-install; re-running `/sonari:install`
  re-points the plist to the refreshed `~/.sonari/app` and the cue stops.
- [ ] **Cache prune resilience.** With the daemon installed, delete/rename the
  marketplace cache `…/<version>/src`; `launchctl kickstart -k` the speechd
  agent. Confirm: it still imports and speaks (PYTHONPATH is `~/.sonari/app`).
- [ ] **Single-instance.** With the LaunchAgent daemon live, run
  `bin/sonari-daemon` (or trigger a lazy start). Confirm: the second process
  exits without orphaning the socket; only one speaker is heard.
- [ ] **Interpreter consistency.** With Homebrew `python3` first on PATH, confirm
  the lazily started daemon runs `/usr/bin/python3` (via `ps`/`lsof` on the pid).
- [ ] **Launcher robustness.** After install, `~/.local/bin/sonari status` works
  in a fresh shell. Remove the launcher by hand; a new session speaks the
  `not_installed` cue; `/sonari:install` recreates it. Note any vanish root cause.
- [ ] **Uninstall.** `/sonari:uninstall` removes both LaunchAgents, the hotkeyd
  binary, the launcher, and `~/.sonari/app`, and preserves `config.json` +
  `keymap.json`.
- [ ] **Dual-interpreter gate.** Full suite green under `.venv` (3.13) AND
  `.venv39` (3.9).
