# Sonari Phase 3 — Self-Contained Packaging & Installer (Design Spec)

**Status:** Approved (user, 2026-06-05) — ready for implementation planning
**Date:** 2026-06-05
**Scope:** Phase 3 **sub-project #1 only** — packaging + installer that makes Sonari
publicly installable as a self-contained, zero-dependency marketplace plugin.
**Depends on:** Phase 1 (complete) and Phase 2 (complete) — `speechd`, the hooks, the Unix
socket protocol, `hotkeyd`, native-numeric selection. 314 tests green.
**Supersedes:** §7 (packaging/install/migration) of
`2026-06-04-echo-eyes-free-claude-code-design.md` and §5 (packaging/install/doctor) of
`2026-06-05-sonari-phase2-control-selection-design.md` — both are folded forward here.
**Folds in (from `phase1-review-followups.md`):** the deferred item "install verifies the
daemon shim is executable" and "LaunchAgent plist XML-escaping" (packaging-relevant; see
§5 and §7).

---

## 1. Goal & success criteria

Make Sonari installable by **any** blind/low-vision macOS developer **with no developer
tooling beyond what Apple ships**, no Python packaging knowledge, and no manual file
editing. The plugin is enabled in Claude Code, the user runs one command (`sonari install`),
and everything works — speech, earcons, global hotkeys, selection — on the **macOS system
Python** with **no PyPI, no pip, no `--break-system-packages`, no Homebrew, and no Apple
Developer account / notarization**.

**Success =** on a clean Mac with the plugin enabled and Xcode Command Line Tools present,
`sonari install` followed by `sonari doctor` reports every check green, a real Claude Code
session narrates and earcons correctly, and all nine Phase 2 global hotkeys + native-numeric
selection work — **using `/usr/bin/python3` (or the best available `python3 >= 3.9`),
without any `sonari` package installed into site-packages.**

Concrete, testable success criteria:

1. The full test suite is green under **both** `/usr/bin/python3` (3.9.6) **and** the
   existing 3.13 venv.
2. `bin/sonari-hook`, `bin/sonari-daemon`, and `bin/sonari` run the package straight from the
   plugin's `src/` directory with **no installed `sonari`** anywhere on the machine.
3. `sonari install` writes both LaunchAgents with **absolute** plugin paths and the
   **resolved absolute** interpreter path; both load; both sockets are reachable.
4. `~/.local/bin/sonari` exists and runs the CLI (so slash commands and manual control work),
   regardless of whether `sonari` is on `sys.path`.
5. `hotkeyd` is compiled locally at install via `swiftc` (no Gatekeeper quarantine, no
   notarization); when `swiftc` is absent, install still completes (speech works) and doctor
   guides `xcode-select --install`.
6. On this dev Mac, `sonari install` cleanly transitions the machine off the current
   editable-pip + Homebrew-Python LaunchAgents onto the self-contained system-Python model.

Out of scope is enumerated explicitly in §10.

## 2. Current state & why it must change

The plugin currently **only works because of a developer-only install footprint** that a
public user will not have:

1. **The package is reachable only via an editable pip install.** Verified on this Mac:
   `/opt/homebrew/bin/python3 -c "import sonari"` resolves to
   `…/claude-tts/src/sonari/__init__.py` — i.e. an `pip install --break-system-packages -e .`
   into Homebrew Python. Remove that and nothing imports.
2. **The bin shims assume an importable `sonari`.** `bin/sonari` is
   `exec python3 -m sonari.cli "$@"`, `bin/sonari-daemon` is `exec python3 -m sonari.daemon
   "$@"`. Both fail on a clean Mac because `sonari` is not on `sys.path` and `python3` may be
   the wrong interpreter. (`bin/sonari-hook` is the **only** shim that already falls back to
   `../src` — see its `import sonari` / `sys.path.insert(0, src)` block, lines 33-41.)
3. **`daemon.ensure_running()` re-spawns the daemon through that broken shim.** It runs
   `subprocess.Popen([_daemon_shim_path()])` where the shim is `bin/sonari-daemon` →
   `python3 -m sonari.daemon`. Lazy daemon start therefore inherits the same pip dependency.
4. **The LaunchAgents embed the wrong interpreter.** `install()` calls
   `_launchagent_plist(daemon, log, python_executable=sys.executable)`, so on this Mac the
   speechd plist's `ProgramArguments` is `[/opt/homebrew/bin/python3, -m, sonari.daemon]` and
   relies on `sonari` being in that interpreter's site-packages. A public user has no such
   interpreter and no such install.
5. **`requires-python = ">=3.10"`** (`pyproject.toml` line 9) and the README "Python 3.10 or
   newer" exclude the macOS system interpreter, which is **3.9.6**. The package has been
   verified to import and run under 3.9.6 (PEP 585 `list[str]` works in 3.9; every `X | None`
   hint is in a quoted/`Optional[...]`/unevaluated position — `cli.py` uses
   `Optional[...]` + already has `from __future__ import annotations`).
6. **Only `cli.py` has `from __future__ import annotations`.** The other 13 modules in
   `src/sonari/` do not (verified). They run on 3.9 today, but future edits adding `X | Y`
   runtime-evaluated annotations would silently break 3.9; the future-import is cheap
   insurance for a public 3.9 target.
7. **The README install instructions are pip-based** (`pip install -e .`) — wrong for the
   public path.

The fix is to make the plugin **self-contained**: ship the stdlib-only `src/sonari` inside
the plugin, run it directly off `PYTHONPATH=<plugin>/src`, resolve and pin an absolute
`python3 >= 3.9`, and build the Swift binary on the user's machine.

## 3. Architecture (self-contained plugin)

Nothing about the runtime data flow (hooks → speechd, hotkeyd → speechd, native numeric
selection) changes. Only **how the code is located and launched** changes.

### 3.1 The plugin ships its own source

The marketplace plugin directory **is** the repo layout: `.claude-plugin/`, `hooks/`,
`commands/`, `bin/`, `hotkeyd/`, and `src/sonari/` all ship together. `${CLAUDE_PLUGIN_ROOT}`
(already used by `hooks/hooks.json`) is the absolute plugin root at hook time;
`paths.repo_root()` (two dirs up from `src/sonari/paths.py`) is the same root at import time.
No `sonari` is ever installed into any interpreter's site-packages.

### 3.2 Interpreter resolution (`python3 >= 3.9`)

Target = the macOS system interpreter `/usr/bin/python3` (3.9+). Because a user may have a
newer/better `python3` first on PATH, the installer **picks the best available
`python3 >= 3.9` and records its absolute path** (preferring `/usr/bin/python3` when it
qualifies, since it is guaranteed present and stable across logins). The chosen absolute path
is what gets written into the LaunchAgent `ProgramArguments` and the `~/.local/bin/sonari`
launcher.

Resolution algorithm `_resolve_python()` (new, in `cli.py`):

1. Build a candidate list, in priority order: `/usr/bin/python3`, then every `python3` /
   `python3.13` / `python3.12` / `python3.11` / `python3.10` / `python3.9` found via
   `shutil.which`, deduped by realpath.
2. For each candidate, run `<cand> -c "import sys; print('%d.%d' % sys.version_info[:2])"`
   and keep the first that reports `>= (3, 9)`.
3. **Preference rule:** if `/usr/bin/python3` qualifies, choose it (stability over newness);
   otherwise choose the first qualifying candidate in PATH order.
4. Return its absolute realpath, or `None` if none qualify (fatal — see §6).

### 3.3 Hook / daemon / CLI wiring without pip

Three entrypoints, all made source-relative and interpreter-correct:

- **`bin/sonari-hook`** — already self-locating (its `../src` fallback works today). Make it
  robust under the system interpreter and a spaces-in-path plugin root. No installed `sonari`
  required. (Behavior change: prepend `../src` to `sys.path` **unconditionally and first**,
  rather than only on `ImportError`, so a stale globally-installed `sonari` never shadows the
  plugin's own source — see §5.1.)
- **`bin/sonari-daemon`** — rewritten to set `PYTHONPATH=<plugin>/src` and exec the resolved
  interpreter on `-m sonari.daemon` (§5.1). This is what
  `daemon.ensure_running()`/`subprocess.Popen([shim])` spawns for lazy start, **and** what
  the speechd LaunchAgent will reference indirectly via its own `ProgramArguments`.
- **`bin/sonari`** — rewritten to the same source-relative pattern for `-m sonari.cli`
  (§5.1). The `~/.local/bin/sonari` launcher is a thin wrapper that execs this shim with the
  plugin root baked in (§5.6).

The **speechd LaunchAgent** does **not** call the bin shim (launchd has a minimal PATH and a
shim that re-resolves `python3` could drift). Instead `install()` writes the resolved
interpreter and the plugin `src` path **directly** into the plist:
`ProgramArguments = [<resolved python3>, -m, sonari.daemon]` with
`EnvironmentVariables.PYTHONPATH = <plugin>/src` (§5.3). The **hotkeyd LaunchAgent**
references the **built binary path** (`~/.sonari/sonari-hotkeyd`) and needs no interpreter.

### 3.4 Plugin-path resolution

`install()` must resolve the **absolute** plugin root and persist it so daemon and hotkeyd
restarts (which launchd performs without `${CLAUDE_PLUGIN_ROOT}`) keep finding the source:

- Primary: `os.path.realpath(paths.repo_root())` (resolves symlinks; correct because cli.py
  runs from inside the plugin's own `src/sonari`).
- Persisted: written into the plist `EnvironmentVariables.PYTHONPATH` (for speechd) and into
  a new `~/.sonari/install.json` record (for doctor + migration; see §5.5).
- `daemon.ensure_running()` continues to compute the shim path from `repo_root()` at runtime,
  which is correct for the lazy-start path because the CLI itself is running from the plugin.

### 3.5 Swift build-on-install

Unchanged from Phase 2 in mechanism (`_build_hotkeyd()` calls
`swiftc hotkeyd/sonari-hotkeyd.swift -o ~/.sonari/sonari-hotkeyd`), now made the **public**
story: building locally means the binary is produced on the user's machine and therefore is
**not quarantined by Gatekeeper** and needs **no notarization**. Requirements: Xcode Command
Line Tools (provides `swiftc`). The build is **non-fatal**: missing `swiftc` warns + guides
`xcode-select --install` and continues (speech still works; hotkeys are disabled until the
user installs CLT and re-runs `sonari install`). Phase 2 mentioned "ad-hoc signed"; for v1 we
rely on locally-built-binary semantics and do **not** add an explicit `codesign` step (build
output already runs locally without notarization). If a future macOS hardens this, an
`codesign -s -` ad-hoc step is the cheap fallback (noted, not implemented).

## 4. Install / uninstall / doctor (ordered, eyes-free)

All three already print line-by-line, eyes-free output. The ordered behavior:

**`install()` order:**
1. Resolve the best `python3 >= 3.9` (`_resolve_python()`); **fatal** if none (§6) — print
   the exact remediation and exit non-zero.
2. `paths.ensure_sonari_dir()`.
3. Check `swiftc` / Command Line Tools. If absent: print the `xcode-select --install`
   guidance, set a "hotkeys deferred" flag, but **continue** (non-fatal).
4. Build `hotkeyd` (`_build_hotkeyd()`) when `swiftc` is present.
5. Write the default keymap if absent (`keymap.write_default_keymap_if_absent()`) and the
   resolved keymap (`keymap.write_resolved()`).
6. Write `~/.sonari/install.json` (resolved interpreter, absolute plugin root, plugin `src`
   path, timestamp) — the durable install record (§5.5).
7. Write + load **both** LaunchAgents with absolute plugin paths and the resolved
   interpreter (speechd: `[<py>, -m, sonari.daemon]` + `PYTHONPATH`; hotkeyd: `[<binary>]`).
   Skip the hotkeyd agent if the build was skipped/failed.
8. Place `~/.local/bin/sonari` launcher (§5.6).
9. **Migration:** run legacy migration (`_legacy_migrate()`) **and** the new
   DEV-INSTALL migration (`_dev_install_migrate()`, §8).
10. Voice check: report the best enhanced voice (or the Samantha fallback) so the user knows
    whether to install one.
11. Print **eyes-free next steps** (enable the plugin, run `sonari doctor`, and — if
    `~/.local/bin` is not on PATH — the exact line to add).

**`uninstall()` reverses install** (already mostly does):
- `bootout`/unload + remove **both** LaunchAgents.
- Remove the hotkeyd binary.
- Remove the `~/.local/bin/sonari` launcher (**new** — install now places it, so uninstall
  must remove it).
- Remove runtime artifacts (socket, logs, `config.json`, resolved keymap, hotkeyd log,
  `install.json`).
- **Preserve** `~/.sonari/keymap.json` **and** `~/.sonari/config.json`. (Note: the current
  code removes `config.json`; per the approved decision config.json is now **preserved** —
  see §5.4.)
- Run `_legacy_migrate()` for any prior `claude-tts`.

**`doctor()` checks** — keep the existing ones (`say`, `afplay`, enhanced voice,
`SONARI_DIR writable`, daemon socket, plugin hooks.json, `swiftc`, hotkeyd binary, hotkeyd
resolved keymap, keymap resolves) and **add**:
- `python3 >= 3.9 found` — report the resolved absolute path (or FAIL with remediation).
- `Command Line Tools / swiftc` — already covered by the `swiftc` check; upgrade its detail
  string to name `xcode-select --install` when missing.
- `plugin path resolved` — `install.json` exists, its plugin `src` path exists and contains
  `sonari/__init__.py`.
- `speechd LaunchAgent loaded` and `hotkeyd LaunchAgent loaded` — via
  `launchctl print gui/<uid>/<label>` (or `launchctl list <label>`), reported separately.
- `~/.local/bin/sonari launcher present` and `~/.local/bin on PATH`.
The existing "daemon socket" and "hotkeyd resolved keymap" checks already cover socket
reachability and keymap resolution; keep them.

## 5. Exact file/behavior changes

### 5.1 `bin/` shims

**`bin/sonari-hook`** (currently lines 33-41): change the package-resolution block so the
plugin's own `src` is **always** first on `sys.path`, before any `import sonari`:

From:
```python
    # Resolve the package: prefer an installed 'sonari'; fall back to ../src.
    try:
        import sonari  # noqa: F401
    except Exception:
        here = os.path.dirname(os.path.abspath(__file__))
        src = os.path.join(os.path.dirname(here), "src")
        if src not in sys.path:
            sys.path.insert(0, src)
```
To:
```python
    # Resolve the package from the plugin's own src/ (self-contained; never rely
    # on an installed 'sonari'). Insert first so it shadows any stale global copy.
    here = os.path.dirname(os.path.abspath(__file__))
    src = os.path.join(os.path.dirname(here), "src")
    if src not in sys.path:
        sys.path.insert(0, src)
```
(The total try/except + always-exit-0 contract is unchanged.)

**`bin/sonari-daemon`** — from `#!/usr/bin/env bash` + `exec python3 -m sonari.daemon "$@"`
to a self-locating launcher that puts the plugin `src` on `PYTHONPATH` and uses the resolved
interpreter, falling back to `/usr/bin/python3`:
```bash
#!/usr/bin/env bash
# Self-contained: run the plugin's own src with no installed 'sonari'.
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
export PYTHONPATH="$root/src${PYTHONPATH:+:$PYTHONPATH}"
py="$(command -v python3 || true)"
[ -x "$py" ] || py="/usr/bin/python3"
exec "$py" -m sonari.daemon "$@"
```
(The launchd-spawned speechd does **not** use this shim — it embeds the resolved interpreter
directly per §5.3 — but `daemon.ensure_running()`'s lazy `Popen([shim])` does, so the shim
must be self-contained and executable.)

**`bin/sonari`** — same pattern, targeting `-m sonari.cli`:
```bash
#!/usr/bin/env bash
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
export PYTHONPATH="$root/src${PYTHONPATH:+:$PYTHONPATH}"
py="$(command -v python3 || true)"
[ -x "$py" ] || py="/usr/bin/python3"
exec "$py" -m sonari.cli "$@"
```

All three shims must be marked executable in the repo (`chmod +x`); `install()` additionally
verifies the daemon shim is executable and `chmod +x`-es it if not (folds in the deferred
"install verifies the daemon shim is executable" follow-up).

### 5.2 `hooks/hooks.json`

**No change.** It already invokes `${CLAUDE_PLUGIN_ROOT}/bin/sonari-hook <Event>`, which is
exactly the self-contained path. All seven events stay as-is. (Verified: MessageDisplay,
PreToolUse ×3 matchers, Notification ×2, Stop, UserPromptSubmit, SessionStart, SessionEnd.)

### 5.3 `cli.py` — `_launchagent_plist`, `_hotkeyd_plist`, `_plist`, `install`

- **`_plist(label, program_args, log_path)`** — add an optional `env: dict | None`
  parameter; when given, emit an `EnvironmentVariables` `<dict>` of `<key>/<string>` pairs
  (used to inject `PYTHONPATH`). **XML-escape** all interpolated strings (label, each
  program arg, log path, env keys/values) via a small `_xml_escape()` (`&`,`<`,`>` →
  entities) so a plugin path containing `&` or a quote cannot corrupt the plist — folds in
  the deferred "LaunchAgent plist XML-escaping" follow-up.
- **`_launchagent_plist(...)`** — change signature to take the resolved
  `python_executable` (required, no `sys.executable` default) and the plugin `src` path, and
  emit `ProgramArguments = [<python>, "-m", "sonari.daemon"]` with
  `env={"PYTHONPATH": <plugin>/src}`. Remove the `python_executable=None →
  sys.executable` default and the now-misleading docstring about `sys.executable`.
- **`_hotkeyd_plist(binary_path, log_path)`** — unchanged (binary, no interpreter).
- **`install()`** — replace `xml = _launchagent_plist(daemon, log,
  python_executable=sys.executable)` with: resolve the interpreter via `_resolve_python()`
  (fatal if `None`), resolve `src = os.path.join(realpath(repo_root()), "src")`, and call
  `_launchagent_plist(python_executable=<resolved>, src_path=src, log_path=log)`. Add the
  `swiftc`/CLT pre-check (non-fatal), the `~/.local/bin/sonari` placement (§5.6), the
  `install.json` write (§5.5), the dev-install migration call (§8), the voice check, and the
  `~/.local/bin` PATH advice in the printed next-steps. Drop the literal
  `python_executable=sys.executable` everywhere.

### 5.4 `cli.py` — `uninstall`, `_register_local`, slash-command coverage

- **`uninstall()`** — stop removing `paths.CONFIG_PATH` (now **preserved** alongside
  `keymap.json`, per the approved decision); remove `~/.local/bin/sonari`; remove
  `~/.sonari/install.json`. Keep the `bootout`/unload + remove for both agents and the
  hotkeyd binary removal. Update the printed "Preserved …" line to mention both keymap.json
  and config.json.
- **`_register_local`** — unchanged set of local subcommands (`doctor`, `install`,
  `uninstall`, `daemon`, `keymap`). (Note: `_resolve_python` is an internal helper, not a
  subcommand.)
- **Slash-command gap (folded from the Phase-1.x deferred item):** the README documents
  `/sonari:voice`, `/sonari:rate`, `/sonari:skip` but only `doctor`, `status`, `keymap`,
  `repeat`, `stop`, `verbosity` exist in `commands/`. For packaging correctness, **add the
  three missing command files** (`sonari:voice.md`, `sonari:rate.md`, `sonari:skip.md`) so a
  public user's slash-command surface matches the documented controls. They mirror the
  existing thin command files (run `sonari voice <name>` / `sonari rate <wpm>` /
  `sonari skip`; voice/rate echo output, skip is silent like repeat/stop).

### 5.5 `paths.py` — new install record

Add `INSTALL_RECORD_PATH = SONARI_DIR / "install.json"`. `install()` writes a JSON object:
`{"python": "<abs path>", "python_version": "3.9", "plugin_root": "<abs>", "src":
"<abs>/src", "installed_at": "<iso8601>"}`. `doctor()` reads it for the "plugin path
resolved" and "python3 found" checks; `uninstall()` removes it. No change to the existing
`SONARI_DIR` / socket / keymap / hotkeyd path constants.

### 5.6 `~/.local/bin/sonari` launcher (new)

`install()` writes an executable launcher at `~/.local/bin/sonari` so slash commands and
manual `sonari …` invocations work in any shell without the plugin's `bin/` on PATH. It is a
generated wrapper that bakes in the absolute plugin `bin/sonari` path:
```bash
#!/usr/bin/env bash
exec "<ABS_PLUGIN_ROOT>/bin/sonari" "$@"
```
`install()` `os.makedirs("~/.local/bin", exist_ok=True)`, writes the file with `0o755`, and
prints whether `~/.local/bin` is on `$PATH`; if not, it prints the exact line to add to the
user's shell rc (it does **not** edit any rc file — the new design uses no shell-rc edits).
A pre-existing `~/.local/bin/sonari` is overwritten (it is Sonari-owned). `uninstall()`
removes it.

### 5.7 `pyproject.toml`

- Line 9: `requires-python = ">=3.10"` → `requires-python = ">=3.9"`.
- Keep `[project.scripts] sonari = "sonari.cli:main"` (still useful for the dev venv / test
  installs), but it is **not** the public install mechanism. No new runtime deps;
  `dev = ["pytest>=7"]` unchanged. Bump `version` to `0.3.0` to mark the Phase 3 packaging
  release (Phase 1 = 0.1.0; Phase 2 work has been on 0.1.0/0.2.x — set 0.3.0 here and mirror
  it in `.claude-plugin/plugin.json` `version` and `marketplace.json` if a version field is
  added).

### 5.8 `from __future__ import annotations` sweep

Add `from __future__ import annotations` as the **first** statement (after the module
docstring, before other imports) to **all 13 modules currently missing it**:
`__init__.py`, `assembler.py`, `cleaner.py`, `client.py`, `config.py`, `daemon.py`,
`hooks_entry.py`, `keymap.py`, `paths.py`, `protocol.py`, `queue.py`, `sessions.py`,
`speaker.py`. (`cli.py` already has it.) Purely defensive for the 3.9 target; no runtime
behavior change.

### 5.9 Docs (`README.md`)

Rewrite the **Requirements** and **Install** sections for the public, pip-free path:
- Requirements: "Python 3.9 or newer (macOS ships `/usr/bin/python3`)" instead of 3.10;
  "Xcode Command Line Tools (for global hotkeys) — `xcode-select --install`".
- Install: enable the `sonari` plugin from the Claude Code marketplace / `--plugin-dir`, then
  run `sonari install` (which builds hotkeyd, writes the LaunchAgents, and places the
  `~/.local/bin/sonari` launcher). **Delete** the `git clone` + `pip install -e .` block from
  the user-facing flow (keep a short "Development" note that contributors can `pip install -e
  .[dev]` into a venv to run tests). Update the controls table only if the three new slash
  commands change it (they bring it into line). (Full onboarding docs are a **separate**
  Phase-3 sub-project — keep this edit minimal and packaging-focused.)

## 6. Error handling & edge cases

- **No `swiftc` / Command Line Tools.** Non-fatal. `install()` prints: `Xcode Command Line
  Tools not found; global hotkeys disabled. Install them with:  xcode-select --install   then
  re-run: sonari install`. speechd is still installed and speech works. `doctor` reports the
  `swiftc` check FAIL with the same remediation. The hotkeyd LaunchAgent is **not** written
  when there is no binary.
- **No `python3 >= 3.9`.** Fatal for `install()`. Print: `No suitable python3 found (need
  3.9+). macOS normally ships /usr/bin/python3; if missing, install the Command Line Tools
  (xcode-select --install).` Exit non-zero. `doctor` reports the corresponding FAIL.
- **Plugin path with spaces (or `&`/quotes).** The bin shims quote `$here`/`$root`; the
  plist uses XML-escaped strings and `PYTHONPATH` as a separate env entry (not shell-split);
  `~/.local/bin/sonari` quotes the baked path. Add a dedicated test installing into a temp
  plugin root containing a space.
- **`~/.local/bin` not on PATH.** Non-fatal. `install()` and `doctor` detect it and print the
  exact rc line to add (e.g. `export PATH="$HOME/.local/bin:$PATH"`); slash commands still
  work because Claude Code resolves `sonari` via the plugin, and the user can always run the
  absolute `~/.local/bin/sonari`. Sonari never edits a shell rc itself.
- **Missing / no enhanced voice.** Non-fatal. Sonari falls back to Samantha (existing
  `speaker.best_enhanced_voice()` path). `install()` and `doctor` report the detected voice
  and the System Settings → Spoken Content steps.
- **Stale globally-installed `sonari` (e.g. this dev Mac's editable install).** The shims now
  put the plugin `src` **first** on `sys.path`/`PYTHONPATH`, so the plugin's source always
  wins; the dev-install migration (§8) additionally guides removing the old package.
- **Re-running `install()`** is idempotent: it unloads/reloads agents, overwrites the
  launcher and `install.json`, and `write_default_keymap_if_absent()` preserves a customized
  keymap.
- **launchd in a non-GUI / SSH context.** `launchctl load` may warn; install already prints a
  non-fatal warning and the agent autostarts at next GUI login. Unchanged.

## 7. Testing strategy

- **Dual-interpreter gate (new):** the full suite must pass under **both**
  `/usr/bin/python3` (3.9.6) and the 3.13 venv. Add a 3.9 invocation to the test runner /
  CI matrix (a `python3.9` / `/usr/bin/python3 -m pytest` job alongside the existing 3.13
  one). This is the primary guard for the lowered `requires-python` and the
  `from __future__` sweep. PROTOCOL_VERSION stays `1` (no bump).
- **Hermetic install/uninstall/doctor tests (updated):** no writes to the real `~/` or
  `~/Library`; monkeypatch `HOME`, `paths.SONARI_DIR`, the LaunchAgent paths, and
  `~/.local/bin`; stub `launchctl` (existing `_launchctl` patch point) and `_resolve_python`
  (return a fake abs path so no real interpreter probe runs). Assert: the speechd plist
  contains the resolved interpreter + `PYTHONPATH=<src>` + escaped paths; the hotkeyd plist
  references the binary; `install.json` is written with the right keys; `~/.local/bin/sonari`
  is created `0o755` and execs the plugin `bin/sonari`; `uninstall()` removes the launcher,
  the agents, the binary, and `install.json`, and **preserves** `keymap.json` and
  `config.json`. **No real `swiftc`** in these tests — patch `_build_hotkeyd` to a
  success/skip stub.
- **One dedicated real-compile test** (the only place `swiftc` actually runs): skip-if
  `swiftc` absent; compile `hotkeyd/sonari-hotkeyd.swift` to a temp path and assert exit 0 +
  an executable output. Keeps the rest of the suite hermetic and fast.
- **Interpreter-resolution unit tests:** `_resolve_python()` prefers `/usr/bin/python3` when
  it qualifies; falls back to the first qualifying PATH candidate; returns `None` when all
  are `< 3.9` (probe stubbed).
- **Shim tests:** assert the rewritten `bin/sonari-hook` puts `src` first (a fake stale
  `sonari` on `sys.path` does not shadow the plugin's); a smoke test that
  `PYTHONPATH=<repo>/src /usr/bin/python3 -m sonari.cli --help` exits cleanly with **no**
  installed package (run in a subprocess with a scrubbed environment).
- **Spaces-in-path test:** install into a temp plugin root containing a space; assert the
  plist `PYTHONPATH` and the launcher both resolve correctly.
- **Manual fresh-install smoke checklist (new doc section / appendix):** on a clean Mac with
  no `sonari` installed: enable the plugin → `sonari install` → `sonari doctor` all green →
  start a real Claude session → hear the ready earcon, prose narration in order, decision
  earcons → exercise all nine hotkeys and native numeric selection → `sonari uninstall` →
  confirm agents/binary/launcher gone and keymap/config preserved. (Reuse and extend the
  existing `phase2-manual-smoke-checklist.md` structure.) Cross-machine QA on a second Mac is
  a **separate** Phase-3 sub-project (§10).

## 8. Migration (dev-Mac + general)

A new `_dev_install_migrate(home=None) -> list` (alongside the existing `_legacy_migrate`),
called from `install()`, returns human-readable lines and is a safe no-op when there is no
dev footprint:

1. **Detect the editable-pip footprint.** Check whether `sonari` imports from outside the
   current plugin (e.g. a `*.egg-link` / `__editable__*.pth` referencing this repo in a
   Homebrew/other interpreter's site-packages). Because uninstalling another interpreter's
   package programmatically is risky, **do not auto-`pip uninstall`**; instead print exact
   guidance: `Detected an old editable 'sonari' install in <interpreter>. Remove it with:
   <interpreter> -m pip uninstall sonari   (optionally also: --break-system-packages).` The
   self-contained shims already shadow it, so this is cleanup, not a blocker.
2. **Rewrite the LaunchAgents to the new ProgramArguments.** This happens automatically:
   `install()` overwrites both plists with the resolved system interpreter + `PYTHONPATH`
   (replacing the old `[/opt/homebrew/bin/python3, -m, sonari.daemon]` form) and reloads
   them, so the dev Mac transitions onto the public path on the next `sonari install`.
3. **General users:** `_dev_install_migrate` is a no-op (no editable footprint); only
   `_legacy_migrate` (the existing `claude-tts` cleanup) may find anything.

This makes the dev Mac validate the exact public install path — the explicit point of
approved decision #5.

## 9. Verification list (confirm empirically during the build)

1. With the editable `sonari` **uninstalled** from Homebrew Python,
   `PYTHONPATH=<repo>/src /usr/bin/python3 -m sonari.cli doctor` runs and the daemon
   lazy-starts via the rewritten `bin/sonari-daemon` shim.
2. The full suite is green under `/usr/bin/python3` (3.9.6) **and** the 3.13 venv.
3. `swiftc hotkeyd/sonari-hotkeyd.swift -o <tmp>` exits 0 and the locally-built binary runs
   **without** a Gatekeeper quarantine prompt (confirming no notarization is needed).
4. The generated speechd plist's `ProgramArguments` is `[<abs python3>, -m, sonari.daemon]`
   with `EnvironmentVariables.PYTHONPATH = <abs>/src`, and after `launchctl load` the socket
   is reachable; the hotkeyd plist references the built binary and all nine hotkeys fire.
5. `~/.local/bin/sonari status` works in a fresh shell (and the PATH advice prints when
   `~/.local/bin` is absent from `$PATH`).
6. Installing into a plugin root containing a space wires speechd, hotkeyd, and the launcher
   correctly.
7. On this dev Mac, `sonari install` migrates the old Homebrew-Python LaunchAgents to the
   system-Python ones and the old editable package no longer shadows the plugin source.
8. `sonari uninstall` removes both agents, the binary, and the launcher, and **preserves**
   `keymap.json` and `config.json`.
9. `/usr/bin/python3 --version` is 3.9.6 on this Mac (already confirmed); confirm the
   chosen interpreter on a clean Mac is `/usr/bin/python3`.

## 10. Out of scope (deferred — separate sub-projects / later)

- **Code signing & notarization** (Developer-ID, `codesign`, `xcrun notarytool`) — **dropped
  for v1.** Local `swiftc` build avoids quarantine; no Apple Developer account needed.
- **PyPI / `pip` distribution** of the `sonari` package — not the public mechanism.
- **Homebrew formula / `brew install`** — not used.
- **Marketplace / GitHub publish** (registering the marketplace, publishing the repo,
  release tags, GitHub Actions for the dual-interpreter CI gate beyond local runs) — Phase 3
  **sub-project #2**.
- **Onboarding docs** (full user guide, accessibility-focused getting-started, screencasts) —
  Phase 3 **sub-project #3**. This spec only makes the minimal README install/requirements
  edits needed for packaging correctness.
- **Cross-machine QA** (clean second Mac, multiple macOS versions, Terminal/iTerm/VS Code
  matrix) — Phase 3 **sub-project #4**. This spec includes only the single-machine fresh
  smoke checklist.
- **Runtime/feature work** (earcon set authoring, picker desync recovery, background-session
  policy, `background_policy` dead-config, STOP vs CATCH_UP, Stop-hook reconciliation) —
  remaining `phase1-review-followups.md` items, unrelated to packaging.
- **Non-macOS platforms** — `say`/`afplay`/Carbon are macOS-only (Phase 1 non-goal).

---

### Spec self-review (done before saving)
- **Placeholder scan:** no `TODO`/`TBD`/`???`/`<placeholder>` remain; every `<…>` is a
  named runtime value (abs paths, resolved interpreter) explained in context.
- **Internal consistency:** interpreter resolution (§3.2) is referenced identically by
  install (§4/§5.3), doctor (§4), tests (§7), migration (§8), and verification (§9);
  config.json-preservation is stated once as a change-from-current and applied consistently
  in §4/§5.4; the speechd-plist-vs-shim distinction is consistent across §3.3/§5.1/§5.3/§8.
- **Scope:** every item is packaging/installer; publish, onboarding docs, and cross-machine
  QA are explicitly deferred (§10).
- **Ambiguity:** the one genuine choice (build-on-install vs. ad-hoc `codesign`) is resolved
  to "no explicit codesign for v1, ad-hoc as noted fallback" (§3.5). Decided (controller,
  within delegated authority): version = **0.3.0** (pre-1.0, informational); uninstall
  **preserves** both `config.json` and `keymap.json`; **no** explicit `codesign` for v1;
  dual-interpreter (3.9 + 3.13) runs **locally** now, GitHub Actions CI matrix deferred to
  the publish sub-project (§10).
