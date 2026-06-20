# Sonara — Windows fork of Sonari

Status: approved design. Forks Sonari into an independent Windows line ("sonara")
on the user's GitHub, with a full internal rename. nimkimi/sonari stays upstream
as the macOS line.

## 1. Goal & lineage

Split Sonari into two independently-developed/-released lines so a Windows change
can't break Mac and vice versa:

- **`nimkimi/sonari`** — stays upstream, the **macOS** line (unchanged by this work).
- **`Maxaubert/sonara`** — new **public** repo, the **Windows** line. Seeded from
  our consolidated per-session-channels work (the 35-commit branch, full history),
  then fully renamed sonari → sonara.

The per-session-channels work is already pushed to `nimkimi/sonari` as a branch, so
nimkimi can merge it upstream separately. The two lines diverge from that shared
fork point. sonara is an **independent repo** (not a GitHub fork-relationship), so
it has its own issues/releases.

**macOS code is kept** (renamed, not deleted) — sonara stays cross-platform in
structure but is the Windows-focused product.

## 2. Rename scope

The rename is mechanical but pervasive (~173 tracked files). Three case variants:

- `sonari` → `sonara` (lowercase): package `src/sonari/` → `src/sonara/`, every
  `import sonari` / `from sonari`, `bin/sonari*` → `bin/sonara*`, `commands/` slash
  commands (`/sonara:*`), the `~/.sonari` runtime dir, config/lock/log filenames
  that embed it.
- `Sonari` → `Sonara` (capitalized): display/spoken/printed strings, the
  `Sonari.Speechd` Task Scheduler name, macOS LaunchAgent labels, plugin display.
- `SONARI` → `SONARA` (upper): `SONARI_DIR` constant, `SONARI_DISABLE_HOTKEYS` env
  var, any other SONARI_* identifiers.

**Structural touch points (must all change together):** `src/sonari/` dir; all
imports; `bin/` scripts (`sonari`, `sonari-daemon`, `sonari-hook`,
`sonari-hook.cmd`, `sonari.cmd`); `plugin.json` + `marketplace.json` (`name`,
command refs); `commands/*.md`; `pyproject.toml`; `paths.py` (SONARI_DIR, all
derived paths, env vars, task name); README + PRIVACY; **all `tests/`**.

**Repo-reference gotcha:** a blunt `sonari`→`sonara` sed would corrupt
`nimkimi/sonari` into `nimkimi/sonara` (which does not exist). So repo/marketplace
references that mean "this project's repo" must be retargeted to `Maxaubert/sonara`
FIRST (install instructions, marketplace source, plugin marketplace add), and any
genuine upstream-credit reference to `nimkimi/sonari` kept verbatim. Do the repo-ref
pass before the general rename, and verify no `nimkimi/sonara` string exists after.

**Left un-renamed (out of scope):** the dated historical docs under
`docs/superpowers/` (plans/specs/checklists) — records of past work; renaming them is
churn. They keep "sonari" in text and filenames. (This spec is the one exception
that names the rename.)

## 3. Execution & verification

- Apply the rename in the worktree as an isolated, reviewable change, with the
  package-dir move (`git mv`) and the import rewrite done together so nothing
  imports a missing module mid-step.
- **Verification loop (hard gate):** the full test suite must be green except the
  known pre-existing Windows-environmental failures (test_bin_shims, test_bin_sonari,
  test_daemon_main::test_ensure_running…, test_kokoro_provision, test_paths,
  test_transport, test_win_autostart, test_win_tts). After the rename those same
  tests should still be the only failures — any NEW failure means an incomplete
  rename. Then a real `sonara install` must copy the runtime, register the hooks via
  the plugin, start the daemon, and speak.
- Grep gates after the rename: `git grep -il "sonari"` returns only the historical
  `docs/superpowers/` files (and this spec); `git grep -i "nimkimi/sonara"` returns
  nothing.

## 4. Live migration

sonara installs to a **fresh `~/.sonara`** dir; the existing `~/.sonari` is left
untouched as a fallback. Migration steps:

1. Copy the current `~/.sonari/config.json` (af_heart voice, rate=250,
   verbosity, earcons, minqueue, background_policy) and `~/.sonari/keymap.json`
   (the nav/pause/mute/next_session bindings) into `~/.sonara` so the setup carries
   over. The custom earcon WAVs (session_change, turn_done, user_action, nav,
   nav_edge) live in `~/.sonari/earcons/`; copy them to `~/.sonara/earcons/` and
   rewrite the `earcons` paths in the copied config to the new dir, so sonara is
   self-contained and removing `~/.sonari` later can't break it.
2. Uninstall the old sonari daemon + Task Scheduler task + the `~/.local/bin/sonari`
   launcher (so two daemons don't run), or just stop the old daemon.
3. `sonara install`; restart; `sonara doctor` green; verify speech + a hotkey.

The old `~/.sonari` and its Task remain as a rollback path until sonara is confirmed
working.

## 5. Phasing (one project, sequential phases)

1. **Bootstrap:** create `Maxaubert/sonara` (public); push the consolidated branch
   (full history) as `main`.
2. **Repo-reference pass:** retarget `nimkimi/sonari` → `Maxaubert/sonara` in
   user-facing install/marketplace; keep upstream credit.
3. **Rename:** package move + imports + bin + plugin + commands + paths + strings +
   tests, in one coherent change; suite green; grep gates pass.
4. **Migrate:** install sonara on the machine (fresh `~/.sonara`, config carried
   over), retire the old sonari daemon, verify live.

## 6. Out of scope (for now)

- Deleting the macOS backend (kept; a later "Windows-only slim-down" could remove it).
- Renaming the historical docs.
- A GitHub fork-relationship / upstream-PR automation (independent repo instead).
- Any feature change — this is a fork + rename only.
