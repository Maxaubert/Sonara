# Sonari Phase 3.1 - Packaging Hardening (Design Spec)

**Status:** Approved (user, 2026-06-06) - ready for implementation planning
**Date:** 2026-06-06
**Scope:** Phase 3 **sub-project #1.1** - harden the published packaging so a fresh
marketplace user can set Sonari up eyes-free, and so the long-lived daemon/hotkeyd survive a
plugin auto-update. No new user-facing features; this is durability + on-ramp + hardening only.
**Depends on:** Phase 1 (output, complete), Phase 2 (control + native-numeric selection,
complete), Phase 3 #1 (self-contained packaging,
`2026-06-05-sonari-phase3-packaging-design.md`, complete) and #2 (publish to
`github.com/nimkimi/sonari`, v0.3.0 public, complete). Suite green under `.venv` (3.13) and
`.venv39` (3.9).
**Builds on / amends:** `2026-06-05-sonari-phase3-packaging-design.md`. That spec made the
plugin self-contained off `PYTHONPATH=<plugin>/src`. This spec moves the long-lived runtime
**off** that versioned plugin-cache `src` into a stable copy, adds a slash-command on-ramp,
and adds spoken setup guidance - keeping its § numbering and tone for continuity.

---

## 1. Goal & success criteria

A blind/low-vision developer installs Sonari from the **public GitHub marketplace**, hears
Claude immediately (hooks lazy-start the daemon), is **told by voice** to run one slash
command to finish setup, runs `/sonari:install` entirely eyes-free, and from then on
autostart + global hotkeys + the control CLI all work - and **keep** working across a plugin
auto-update, because the long-lived daemon no longer points at the version-pinned cache.

**Success =** on a Mac with the published plugin enabled:

1. The user can complete one-time setup **without leaving Claude Code and without a working
   `sonari` on PATH** - `/sonari:install` runs `sonari install` via the plugin's
   Bash-tool `bin/` and prints the result.
2. After `/sonari:install`, both LaunchAgents reference a **stable** location
   (`~/.sonari/app` for the daemon, `~/.sonari/sonari-hotkeyd` for hotkeyd) - **not** the
   versioned marketplace cache. A simulated plugin-version bump (cache dir change/prune) does
   **not** break speech, autostart, or hotkeys after the bump; they keep running on the copy.
3. On `SessionStart`, when setup is **degraded** (not fully installed, or a version drift),
   the daemon speaks **exactly one** short guidance cue for that session telling the user to
   run `/sonari:install`. When setup is healthy it stays **silent**.
4. Only one speech daemon is ever live (single-instance guard); a second start exits cleanly.
5. The lazy daemon and the installed daemon use the **same** interpreter
   (`/usr/bin/python3` preferred), so behavior is consistent whether speech started lazily or
   via the LaunchAgent.
6. `sonari install` reliably (re)creates `~/.local/bin/sonari`.
7. Full suite green under **both** `.venv` (3.13) and `.venv39` (3.9), plus a manual
   clean-room verification from the public marketplace (§9).

Out of scope is enumerated in §10.

## 2. Problems & evidence (clean-room findings)

Confirmed empirically via a **real** marketplace install (`github.com/nimkimi/sonari`,
v0.3.0) on a clean profile:

1. **No bootstrap on-ramp.** A fresh marketplace user *can* hear Claude - `hooks/hooks.json`
   runs `${CLAUDE_PLUGIN_ROOT}/bin/sonari-hook`, which calls `client.ensure_daemon()` →
   `daemon.ensure_running()` → `Popen([bin/sonari-daemon])`, so the daemon lazy-starts. But
   there is **no clean way to run the one-time `sonari install`**: the `sonari` command is not
   on PATH (the `~/.local/bin/sonari` launcher is only created *by* `install`), and there is
   **no install slash command** (`commands/` has `doctor`, `keymap`, `rate`, `repeat`, `skip`,
   `status`, `stop`, `verbosity`, `voice` - no `install`/`uninstall`). So autostart, global
   hotkeys, and the control CLI never get set up unless the user already knows to find and run
   the plugin's `bin/sonari`.

2. **Update fragility (the durability bug).** `install()` (cli.py:542-563) computes
   `plugin_root = os.path.realpath(paths.repo_root())` and `src = <plugin_root>/src`, then
   writes the speechd LaunchAgent with `EnvironmentVariables.PYTHONPATH = <src>`
   (`_launchagent_plist`, cli.py:477-491). When run from the **marketplace cache**, that
   `src` is inside the **version-pinned** cache dir (e.g. `…/sonari/<version>/src`). A plugin
   auto-update changes or prunes that dir, so the LaunchAgent's `PYTHONPATH` dangles and the
   daemon fails to import `sonari` on its next (re)launch - broken until the user re-runs
   `install`. The hotkeyd binary is already stable (`paths.HOTKEYD_BIN_PATH = ~/.sonari/
   sonari-hotkeyd`, paths.py:13), so only the **Python daemon** is exposed.

Secondary issues observed in the same clean-room run:

3. **Duplicate daemon (no single-instance guard).** `daemon.run()` unconditionally
   `os.unlink(SOCKET_PATH)` then `bind()`s (daemon.py:362-369). If a second daemon starts
   (e.g. lazy `ensure_running` racing the LaunchAgent at login), it unlinks the live socket
   and rebinds, orphaning the first daemon and producing two speakers.

4. **Interpreter inconsistency in the lazy path.** `bin/sonari-daemon` (and `bin/sonari`)
   resolve the interpreter as `py="$(command -v python3 || true)"; [ -x "$py" ] || py="/usr/
   bin/python3"` - i.e. the **first `python3` on PATH** (often Homebrew), only falling back
   to `/usr/bin/python3` when none is found. The installed daemon instead uses
   `_resolve_python()` (cli.py:313-342), which **prefers** `/usr/bin/python3`. So the lazy
   daemon can run a different interpreter than the installed one.

5. **Launcher silently vanished.** In the clean-room run, `~/.local/bin/sonari` had
   disappeared at some point (root cause unknown). `install()` must reliably (re)create it,
   and degraded-setup detection must treat its absence as "needs install."

## 3. Architecture / changes

Nothing about the runtime data flow changes. What changes: a slash-command on-ramp (A); the
long-lived daemon is decoupled from the versioned cache by copying the package into a stable
`~/.sonari/app` (B); the daemon speaks one setup-guidance cue on `SessionStart` only when
degraded (C); and three secondary hardening fixes (D). Each subsection gives the exact
from→to per file.

### 3.A - Slash-command on-ramp (`commands/sonari:install.md`, `commands/sonari:uninstall.md`)

Add two thin command files mirroring `commands/sonari:doctor.md`. They instruct Claude to run
`sonari install` / `sonari uninstall` **via the Bash tool** and print the output verbatim.
While the plugin is enabled, its `bin/` is on the Bash-tool PATH, so bare `sonari` resolves to
the plugin's `bin/sonari` shim even before `~/.local/bin/sonari` exists. This is the one-step,
eyes-free setup from inside Claude Code that §2.1 is missing.

**New file `commands/sonari:install.md`** (exact content):

```markdown
---
description: Install Sonari (autostart, global hotkeys, control CLI) - one-time setup
---

Run the Sonari installer using the Bash tool:

```
sonari install
```

Print the command's output to the user verbatim so they can hear each step and any
remediation (for example, installing Xcode Command Line Tools for the hotkeys). Do not add
commentary beyond the raw output.
```

**New file `commands/sonari:uninstall.md`** (exact content):

```markdown
---
description: Uninstall Sonari (remove LaunchAgents, hotkey helper, launcher, app copy)
---

Run the Sonari uninstaller using the Bash tool:

```
sonari uninstall
```

Print the command's output to the user verbatim. It removes Sonari's LaunchAgents, the hotkey
helper, the `~/.local/bin/sonari` launcher, and the stable app copy, while preserving
`config.json` and `keymap.json`. Do not add commentary beyond the raw output.
```

Both fence the command in a triple-backtick block exactly like `sonari:doctor.md` (which
Claude runs via Bash and whose output it prints verbatim). No code change is required for
these to work; they piggyback on the existing `bin/sonari` shim.

### 3.B - Decouple the long-lived daemon from the versioned cache (the durability fix)

`sonari install` copies the runtime into a **stable** location and points the speechd
LaunchAgent there, so plugin-cache churn cannot break it. The hotkeyd binary is already
stable and is kept.

#### 3.B.1 `paths.py` - new `APP_DIR` constant

Add a stable app directory next to the other `~/.sonari/*` constants.

From (paths.py:7-14):
```python
SONARI_DIR = Path.home() / ".sonari"
CONFIG_PATH = SONARI_DIR / "config.json"
SOCKET_PATH = SONARI_DIR / "speechd.sock"
LOG_PATH = SONARI_DIR / "speechd.log"
KEYMAP_PATH = SONARI_DIR / "keymap.json"
HOTKEYD_RESOLVED_PATH = SONARI_DIR / "hotkeyd.resolved.json"
HOTKEYD_BIN_PATH = SONARI_DIR / "sonari-hotkeyd"
INSTALL_RECORD_PATH = SONARI_DIR / "install.json"
```
To (add one line):
```python
SONARI_DIR = Path.home() / ".sonari"
APP_DIR = SONARI_DIR / "app"          # stable copy of the sonari package (PYTHONPATH target)
CONFIG_PATH = SONARI_DIR / "config.json"
SOCKET_PATH = SONARI_DIR / "speechd.sock"
LOG_PATH = SONARI_DIR / "speechd.log"
KEYMAP_PATH = SONARI_DIR / "keymap.json"
HOTKEYD_RESOLVED_PATH = SONARI_DIR / "hotkeyd.resolved.json"
HOTKEYD_BIN_PATH = SONARI_DIR / "sonari-hotkeyd"
INSTALL_RECORD_PATH = SONARI_DIR / "install.json"
```

Layout contract: `APP_DIR` (= `~/.sonari/app`) is the PYTHONPATH; the package lives at
`<APP_DIR>/sonari/__init__.py`. The installer copies the plugin's `src/sonari` tree to
`APP_DIR/sonari`.

#### 3.B.2 `cli.py` - copy the package in `install()`

Add a helper that refreshes the stable copy on every install, and call it from `install()`.

**New helper `_copy_app(plugin_root: str) -> str`** (place near `_place_launcher`,
cli.py:380): copies `<plugin_root>/src/sonari` → `<APP_DIR>/sonari`, overwriting on every run
so an update re-points cleanly. Use a remove-then-copy so a renamed/removed module in the new
version does not linger:

```python
def _copy_app(plugin_root: str) -> str:
    """Copy the plugin's sonari package into the stable APP_DIR. Returns APP_DIR.

    Overwrites on every install so a plugin update fully refreshes the copy
    (stale modules from a prior version do not linger). The daemon LaunchAgent
    points PYTHONPATH at APP_DIR, decoupling the long-lived daemon from the
    version-pinned marketplace cache.
    """
    app_dir = str(paths.APP_DIR)
    src_pkg = os.path.join(plugin_root, "src", "sonari")
    dst_pkg = os.path.join(app_dir, "sonari")
    os.makedirs(app_dir, exist_ok=True)
    if os.path.isdir(dst_pkg):
        shutil.rmtree(dst_pkg)
    shutil.copytree(src_pkg, dst_pkg)
    return app_dir
```

(`shutil` and `os` are already imported in cli.py:14-16.)

In `install()` (cli.py:542-558), compute `app_dir` from the copy and use it as the PYTHONPATH
target instead of the cache `src`.

From (cli.py:542-558):
```python
    plugin_root = os.path.realpath(paths.repo_root())
    src = os.path.join(plugin_root, "src")

    # 2. Pre-check swiftc / Command Line Tools (non-fatal).
    if shutil.which("swiftc") is None:
        print("Xcode Command Line Tools not found; global hotkeys disabled. "
              "Install them with:  xcode-select --install   then re-run: "
              "sonari install")

    # 3-4. Keymap + build hotkeyd.
    keymap.write_default_keymap_if_absent()
    keymap.write_resolved()
    ok, detail = _build_hotkeyd()

    # 5. Durable install record.
    _write_install_record(python=python, python_version=py_ver,
                          plugin_root=plugin_root, src=src)
```
To:
```python
    plugin_root = os.path.realpath(paths.repo_root())

    # 2. Pre-check swiftc / Command Line Tools (non-fatal).
    if shutil.which("swiftc") is None:
        print("Xcode Command Line Tools not found; global hotkeys disabled. "
              "Install them with:  xcode-select --install   then re-run: "
              "sonari install")

    # 3. Copy the package into the stable APP_DIR (decouples the long-lived
    #    daemon from the version-pinned marketplace cache; see spec §3.B).
    app_dir = _copy_app(plugin_root)
    print(f"Copied runtime to: {app_dir}")

    # 4. Keymap + build hotkeyd.
    keymap.write_default_keymap_if_absent()
    keymap.write_resolved()
    ok, detail = _build_hotkeyd()

    # 5. Durable install record.
    plugin_version = _read_plugin_version(plugin_root)
    _write_install_record(python=python, python_version=py_ver,
                          plugin_root=plugin_root, app_path=app_dir,
                          plugin_version=plugin_version)
```

Then point the speechd plist at `app_dir`.

From (cli.py:560-563):
```python
    # 6. speechd LaunchAgent (resolved interpreter + PYTHONPATH=<src>).
    log = str(paths.LOG_PATH)
    xml = _launchagent_plist(python_executable=python, src_path=src,
                             log_path=log)
```
To:
```python
    # 6. speechd LaunchAgent (resolved interpreter + PYTHONPATH=<APP_DIR>).
    log = str(paths.LOG_PATH)
    xml = _launchagent_plist(python_executable=python, src_path=app_dir,
                             log_path=log)
```

`_launchagent_plist(python_executable, src_path, log_path)` (cli.py:477-491) is unchanged in
body - it already injects `PYTHONPATH=src_path` and runs `[<py>, -m, sonari.daemon]`. The
only change is the **argument** (`app_dir`, not the cache `src`). The docstring's reference to
"the plugin's `<root>/src` directory" should be updated to "the stable `APP_DIR` copy" for
accuracy; that is a comment-only edit.

#### 3.B.3 `cli.py` - `_read_plugin_version` + updated `_write_install_record`

**New helper `_read_plugin_version(plugin_root: str) -> str`** (place near
`_read_install_record`, cli.py:362): read the plugin's declared version from
`<plugin_root>/.claude-plugin/plugin.json` `version`, falling back to the
`CLAUDE_PLUGIN_VERSION` env var if set, else `""`. Failure-tolerant (never raises):

```python
def _read_plugin_version(plugin_root: str) -> str:
    """Return the plugin's declared version, or "" if unreadable.

    Reads <plugin_root>/.claude-plugin/plugin.json 'version'; falls back to the
    CLAUDE_PLUGIN_VERSION env var. Never raises (version is advisory).
    """
    path = os.path.join(plugin_root, ".claude-plugin", "plugin.json")
    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        v = data.get("version") if isinstance(data, dict) else None
        if isinstance(v, str) and v:
            return v
    except Exception:  # noqa: BLE001 - version is advisory, never fatal
        pass
    return os.environ.get("CLAUDE_PLUGIN_VERSION", "") or ""
```

Update `_write_install_record` to record the new fields. The current record keys are
`python`, `python_version`, `plugin_root`, `src`, `installed_at` (cli.py:345-359). Replace
`src` with `app_path` and add `plugin_version`.

From (cli.py:345-359):
```python
def _write_install_record(python: str, python_version: str,
                          plugin_root: str, src: str) -> None:
    """Persist the durable install record used by doctor + migration."""
    from datetime import datetime, timezone
    record = {
        "python": python,
        "python_version": python_version,
        "plugin_root": plugin_root,
        "src": src,
        "installed_at": datetime.now(timezone.utc).isoformat(),
    }
    os.makedirs(os.path.dirname(str(paths.INSTALL_RECORD_PATH)), exist_ok=True)
    with open(str(paths.INSTALL_RECORD_PATH), "w", encoding="utf-8") as f:
        json.dump(record, f, indent=2)
        f.write("\n")
```
To:
```python
def _write_install_record(python: str, python_version: str,
                          plugin_root: str, app_path: str,
                          plugin_version: str) -> None:
    """Persist the durable install record used by doctor + session-start health."""
    from datetime import datetime, timezone
    record = {
        "python": python,
        "python_version": python_version,
        "app_path": app_path,
        "plugin_root": plugin_root,
        "plugin_version": plugin_version,
        "installed_at": datetime.now(timezone.utc).isoformat(),
    }
    os.makedirs(os.path.dirname(str(paths.INSTALL_RECORD_PATH)), exist_ok=True)
    with open(str(paths.INSTALL_RECORD_PATH), "w", encoding="utf-8") as f:
        json.dump(record, f, indent=2)
        f.write("\n")
```

**`doctor()` reads `src` today** (cli.py:226-234): `rec.get("src")` then checks
`<src>/sonari/__init__.py`. Update that read to `app_path` (the new key), so the "plugin path
resolved" check verifies the stable copy:

From (cli.py:227-228):
```python
        rec = _read_install_record()
        src = rec.get("src") if rec else None
        init = os.path.join(src, "sonari", "__init__.py") if src else None
```
To:
```python
        rec = _read_install_record()
        app = rec.get("app_path") if rec else None
        init = os.path.join(app, "sonari", "__init__.py") if app else None
```
(Rename the local `src` → `app` and the FAIL detail string accordingly; the surrounding
`try/except` and the rest of the check are unchanged.)

#### 3.B.4 `cli.py` - `uninstall()` removes `APP_DIR`

`uninstall()` currently removes a fixed `artifacts` list (cli.py:656-662) plus the agents,
the hotkeyd binary, and the launcher, and preserves `keymap.json` + `config.json`. Add
`APP_DIR` removal. Because `APP_DIR` is a directory, remove it with `shutil.rmtree` (the
`artifacts` loop is file-oriented), placed right after the artifacts loop and before the
launcher removal.

Insert after cli.py:668 (the end of the `for artifact in artifacts:` loop):
```python
    # Remove the stable app copy (spec §3.B). config.json + keymap.json live in
    # SONARI_DIR (not APP_DIR) and are preserved below.
    if os.path.isdir(str(paths.APP_DIR)):
        try:
            shutil.rmtree(str(paths.APP_DIR))
            print(f"Removed app copy: {paths.APP_DIR}")
        except OSError:
            pass
```
`config.json` and `keymap.json` are in `SONARI_DIR`, not `APP_DIR`, so removing `APP_DIR`
cannot touch them; the existing "Preserved your settings" output (cli.py:673-681) is
unchanged.

### 3.C - Detect-and-guide spoken setup health, in the daemon, on session start

Keep hooks thin. The hook's `SessionStart` message carries the current plugin version + root;
the daemon evaluates setup health on `SESSION_START` and speaks **one** short cue **only when
degraded** (at most once per session). Decision: **guide only - never auto-run install** (no
silent `launchctl`/`swiftc` from a hook).

#### 3.C.1 `hooks_entry.py` - carry plugin version + root on `SessionStart`

`handle_event` is PURE (no I/O) per its contract (hooks_entry.py:28-32). Reading
`plugin.json` is I/O, so the **read happens in `bin/sonari-hook`** (already does file I/O and
is wrapped in a total try/except, hooks_entry's caller) and is passed in. But to keep the
change minimal and within the existing structure, read `${CLAUDE_PLUGIN_ROOT}` from the env in
`hooks_entry` (env read only, not file I/O - preserves purity) and read the version from the
env too, with a cheap optional file fallback performed by a tiny local helper that is
failure-tolerant. To respect the "PURE: no I/O" docstring, the version-from-file fallback is
done in the **shim** (`bin/sonari-hook`) and injected; `hooks_entry` only reads env vars.

Concretely:

**`bin/sonari-hook`** - before calling `handle_event`, resolve the plugin version cheaply and
export it so `hooks_entry` can read it as an env var (failure-tolerant; never breaks the
hook). Insert after the `from sonari.hooks_entry import handle_event` import (sonari-hook:45):
```python
    # Resolve plugin version (advisory) so the SessionStart message can carry it.
    if not os.environ.get("CLAUDE_PLUGIN_VERSION"):
        try:
            import json as _json
            root = os.environ.get("CLAUDE_PLUGIN_ROOT") or os.path.dirname(here)
            with open(os.path.join(root, ".claude-plugin", "plugin.json"),
                      "r", encoding="utf-8") as _f:
                _v = _json.load(_f).get("version")
            if isinstance(_v, str) and _v:
                os.environ["CLAUDE_PLUGIN_VERSION"] = _v
        except Exception:
            pass
```
(`here` is already defined at sonari-hook:39; the whole shim is wrapped in try/except + always
exits 0, so this cannot break a session.)

**`hooks_entry.py` `SessionStart` branch** - add `plugin_version` + `plugin_root` to the
`SESSION_START` message only (env reads only; purity preserved).

From (hooks_entry.py:96-100):
```python
    if event == "SessionStart":
        return [
            _msg(type=MsgType.SET_FOREGROUND, session=session),
            _msg(type=MsgType.SESSION_START, session=session),
        ]
```
To:
```python
    if event == "SessionStart":
        return [
            _msg(type=MsgType.SET_FOREGROUND, session=session),
            _msg(
                type=MsgType.SESSION_START,
                session=session,
                plugin_version=os.environ.get("CLAUDE_PLUGIN_VERSION", ""),
                plugin_root=os.environ.get("CLAUDE_PLUGIN_ROOT", ""),
            ),
        ]
```
(`os` is already imported, hooks_entry.py:4. Empty strings when the env is absent - the daemon
treats `""` as "unknown" and does not raise a version-drift cue on an empty version; see
§3.C.3.)

#### 3.C.2 `daemon.py` - health helpers

Add cheap, failure-tolerant health helpers to `SpeechDaemon`. They do **only** a few file
stats + a version compare - **no `launchctl`** on the hot path. Add to the imports
(daemon.py:12) the constants the check needs:

From (daemon.py:12):
```python
from sonari.paths import SOCKET_PATH, ensure_sonari_dir, socket_connectable, repo_root
```
To:
```python
from sonari.paths import (
    SOCKET_PATH, ensure_sonari_dir, socket_connectable, repo_root,
    INSTALL_RECORD_PATH, HOTKEYD_BIN_PATH,
)
```

Add a per-session guard set in `__init__` (daemon.py:35, alongside `_warned_immediate`):
```python
        self._guided_sessions: set[str] = set()
```

Add the health helpers as methods (place after `_choice_notes`, daemon.py:119):

```python
    @staticmethod
    def _read_install_record():
        """Return the install.json dict, or None if unreadable/absent. Never raises."""
        import json
        try:
            with open(str(INSTALL_RECORD_PATH), "r", encoding="utf-8") as f:
                data = json.load(f)
            return data if isinstance(data, dict) else None
        except Exception:  # noqa: BLE001 - health check must never raise
            return None

    @staticmethod
    def _launcher_present() -> bool:
        """True if ~/.local/bin/sonari exists (cheap stat)."""
        import os as _os
        return _os.path.exists(
            _os.path.join(_os.path.expanduser("~"), ".local", "bin", "sonari"))

    def _setup_health(self, plugin_version: str):
        """Return (state, cue) where state is one of:
        "ok"        -> fully installed, no version drift   -> cue None
        "not_installed" -> no install.json or launcher (never ran `sonari install`)
        "version_drift" -> installed but plugin_version differs from this session's

        Cheap: a few file stats + a string compare. No launchctl. Never raises.
        """
        import os as _os
        rec = self._read_install_record()
        installed = (rec is not None and self._launcher_present())
        if not installed:
            return ("not_installed",
                    "Sonari is reading aloud. To enable hotkeys and autostart, "
                    "run, slash sonari install.")
        recorded = (rec.get("plugin_version") or "")
        # Only flag drift when BOTH sides are known and differ.
        if plugin_version and recorded and plugin_version != recorded:
            return ("version_drift",
                    "Sonari was updated. Run, slash sonari install, to apply.")
        return ("ok", None)
```

Notes on "installed": `rec is not None` covers the lazy-only / never-ran-install case;
`_launcher_present()` covers the vanished-launcher case (§2.5); both are cheap stats.
**The hotkeyd binary is deliberately NOT part of this check.** A speech-only user on a machine
without `swiftc` runs `sonari install` once - which writes `install.json` + the launcher (the
hotkeyd build is non-fatal) - and is then treated as installed, so they are **never nagged**
about hotkeys. A missing hotkeyd binary is surfaced by `sonari doctor` (with the
`xcode-select --install` guidance), not by the spoken session cue. (This resolves the
speech-only-nagging concern; if `HOTKEYD_BIN_PATH` is now otherwise unused in `daemon.py`,
drop it from the import added in 3.C.2.)

#### 3.C.3 `daemon.py` - emit the cue on `SESSION_START`

The `SESSION_START` branch currently just sets foreground + registers (daemon.py:191-195).
Extend it to evaluate health and enqueue **one** cue, throttled per session, only when
degraded. The cue is enqueued as a normal `prose` item for that session so it goes through the
ordered queue (it is the first thing said in a fresh session, so ordering is natural).

From (daemon.py:191-195):
```python
        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            self.sessions.set_foreground(session)
            if t == MsgType.SESSION_START:
                self.sessions.register(session)
            return None
```
To:
```python
        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            self.sessions.set_foreground(session)
            if t == MsgType.SESSION_START:
                self.sessions.register(session)
                self._maybe_guide_setup(session, msg.get("plugin_version", ""))
            return None
```

Add the throttled emitter (place near the other `_enqueue` helpers, e.g. after `_enqueue`,
daemon.py:57):
```python
    def _maybe_guide_setup(self, session: str, plugin_version: str) -> None:
        """Speak ONE setup-guidance cue for this session, only when degraded.

        Throttle: at most once per session. Silent when healthy. The check is a
        few file stats + a version compare (no launchctl) and never raises.
        """
        if session in self._guided_sessions:
            return
        try:
            state, cue = self._setup_health(plugin_version or "")
        except Exception:  # noqa: BLE001 - guidance must never break a session
            return
        self._guided_sessions.add(session)
        if state != "ok" and cue:
            self._enqueue(session, "prose", cue, False)
```

The throttle records the session **whether or not** a cue fired, so a degraded session that
later becomes healthy mid-session is not re-evaluated, and a healthy session never re-checks.
Clear the per-session guard on `SESSION_END` (daemon.py:197-201) alongside the existing
`_warned_immediate.discard`:

From (daemon.py:197-201):
```python
        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._last_options = None
            self._warned_immediate.discard(session)
            return None
```
To:
```python
        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._last_options = None
            self._warned_immediate.discard(session)
            self._guided_sessions.discard(session)
            return None
```

Performance: `_setup_health` runs once per session at start, under `self._lock`
(daemon.py:341-342 acquires the lock around `handle_message`). It does ≤3 `os.path.exists`
calls + one `open`/`json.load` of a tiny file + a string compare - negligible, and it is not
on the per-utterance speech path.

### 3.D - Secondary hardening

#### 3.D.1 Single-instance daemon guard (`daemon.py` `main()`)

Guard in `main()` (daemon.py:416-430), before building the daemon, so a second daemon exits
gracefully instead of unlinking + rebinding the live socket. Put it in `main()` (not in
`run()`) so the in-process test harness that calls `run()` on an isolated socket is
unaffected, and so the guard is exactly at the process-start boundary that `ensure_running`
spawns.

From (daemon.py:416-430):
```python
def main() -> None:
    from sonari.speaker import Speaker
    from sonari.queue import SpeechQueue
    from sonari.sessions import SessionManager

    cfg = load_config()
    queue = SpeechQueue()
    speaker = Speaker(
        voice=cfg.get("voice"),
        rate=cfg.get("rate", 200),
        earcons=cfg.get("earcons"),
    )
    sessions = SessionManager(background_policy=cfg.get("background_policy", "earcon_only"))
    daemon = SpeechDaemon(queue, speaker, sessions, cfg)
    daemon.run()
```
To:
```python
def main() -> None:
    # Single-instance guard: if a daemon is already accepting connections, exit
    # cleanly instead of unlinking + rebinding the live socket (prevents the
    # duplicate-daemon race between a lazy start and the LaunchAgent at login).
    if socket_connectable():
        return

    from sonari.speaker import Speaker
    from sonari.queue import SpeechQueue
    from sonari.sessions import SessionManager

    cfg = load_config()
    queue = SpeechQueue()
    speaker = Speaker(
        voice=cfg.get("voice"),
        rate=cfg.get("rate", 200),
        earcons=cfg.get("earcons"),
    )
    sessions = SessionManager(background_policy=cfg.get("background_policy", "earcon_only"))
    daemon = SpeechDaemon(queue, speaker, sessions, cfg)
    daemon.run()
```
(`socket_connectable` is already imported, daemon.py:12.) This is a best-effort guard; a
genuine simultaneous-start race (both pass the check before either binds) is acceptably rare
and self-heals on the next start - see §6. `run()`'s `os.unlink(SOCKET_PATH)` is kept (it
still correctly clears a **stale** socket file from a crashed daemon, which the guard does not
cover because a stale socket is not connectable).

#### 3.D.2 Interpreter consistency (`bin/sonari-daemon`, `bin/sonari`)

Make the shims **prefer `/usr/bin/python3`** first, falling back to the first `python3` on
PATH - matching `_resolve_python`'s preference (cli.py:320-342) so the lazy daemon and the
installed daemon use the same interpreter. This is the daemon's lazy-start path
(`ensure_running` → `Popen([bin/sonari-daemon])`, daemon.py:403-413); apply the same to
`bin/sonari` for consistency.

`bin/sonari-daemon` from (lines 6-7):
```bash
py="$(command -v python3 || true)"
[ -x "$py" ] || py="/usr/bin/python3"
```
To:
```bash
# Prefer /usr/bin/python3 (stable across logins, matches `sonari install`'s
# resolved interpreter); fall back to the first python3 on PATH.
if [ -x /usr/bin/python3 ]; then
    py="/usr/bin/python3"
else
    py="$(command -v python3 || true)"
fi
[ -x "$py" ] || py="/usr/bin/python3"
```
Apply the **identical** replacement to `bin/sonari` (lines 6-7 are the same two lines).

Note: this only changes the *lazy* daemon and the CLI shim. The *installed* daemon's
interpreter is whatever `_resolve_python()` recorded in the plist (a 3.9+ interpreter,
preferring `/usr/bin/python3`); since macOS `/usr/bin/python3` is 3.9.6 (verified in the
Phase 3 spec), the shim's preferred interpreter satisfies the package's `requires-python =
">=3.9"`. The `bin/sonari-hook` shim is **not** changed here (it inserts `../src` and uses its
`#!/usr/bin/env python3` shebang; the hook path is unrelated to this consistency fix).

#### 3.D.3 Launcher robustness (`cli.py` `install()`)

`install()` already calls `_place_launcher(plugin_root)` (cli.py:595), which
`os.makedirs(..., exist_ok=True)` + writes `0o755` (cli.py:380-396) - already reliable on
paper. The fix is **defensive verification**: after placing it, confirm it exists and is
executable, and warn (non-fatal) if not. The session-start health check (§3.C.2) treats a
missing launcher as degraded, so a later vanish is re-surfaced as a spoken cue. Root cause of
the earlier vanish is unknown → it is a **verification item** (§9), not a code fix beyond this
defensive check.

In `install()`, change the launcher placement block.

From (cli.py:594-596):
```python
    # 8. ~/.local/bin/sonari launcher.
    launcher = _place_launcher(plugin_root)
    print(f"Placed launcher: {launcher}")
```
To:
```python
    # 8. ~/.local/bin/sonari launcher (verify it landed; non-fatal warn if not).
    launcher = _place_launcher(plugin_root)
    if os.path.exists(launcher) and os.access(launcher, os.X_OK):
        print(f"Placed launcher: {launcher}")
    else:
        print(f"warning: could not place launcher at {launcher}; "
              f"run `sonari` from the plugin's bin/ until this is fixed.")
```

## 4. Docs / version

- **README Install section** (README.md:50-85): the current flow already mentions enabling
  the plugin and running `sonari install`. Update it to the **slash-command** flow so a
  marketplace user does not need `sonari` on PATH first: enable the plugin → run
  `/sonari:install` from inside Claude Code → run `/sonari:doctor`. Keep the `sonari install`
  CLI form as the equivalent for users who already have the launcher on PATH. Add a one-line
  note that the installer copies the runtime to `~/.sonari/app` so it survives plugin updates,
  and that after an update Sonari will say "run /sonari:install" to re-point.
- **README Uninstall section** (README.md:204-215): mention `/sonari:uninstall` as the
  in-session equivalent, and that it also removes `~/.sonari/app`.
- **README Slash commands table** (README.md:145-153): add `/sonari:install` and
  `/sonari:uninstall` rows (CLI: `sonari install` / `sonari uninstall`).
- **Version bump to `0.4.0`** in all three manifests:
  - `.claude-plugin/plugin.json` `version` (currently `"0.3.0"`).
  - `pyproject.toml` `version` (currently `version = "0.3.0"`, line 7).
  - `.claude-plugin/marketplace.json` - **two** places: the top-level `plugins[0].version`
    (currently `"0.3.0"`). (There is no separate root `marketplace.json`; only
    `.claude-plugin/marketplace.json` exists.)

Full onboarding docs remain a **separate** later task (§10); this is the minimal README edit
only.

## 5. Error handling & edge cases

- **No `install.json` yet / lazy-only mode.** `_read_install_record()` returns `None` →
  `_setup_health` returns `("not_installed", <run /sonari:install cue>)`. The user hears
  Claude (lazy daemon) **and** is told once per session to finish setup. Exactly the §2.1
  on-ramp gap, now closed by a spoken cue + the slash command.
- **`APP_DIR` copy fails** (e.g. read-only `~/.sonari`, or source missing). `_copy_app` lets
  the exception propagate to `install()`. Per the install contract (line-by-line eyes-free
  output), `install()` should print a clear failure and return non-zero **before** writing a
  LaunchAgent that would point at a half-copied `APP_DIR`. Implementation note for the build:
  wrap the `_copy_app` call in `install()` in a `try/except OSError` that prints
  `Could not copy the runtime to ~/.sonari/app: <err>. Check that ~/.sonari is writable.` and
  returns 1. (The previous behavior pointed the plist at the cache `src`, which always
  existed; now the copy is a hard dependency, so its failure must be fatal-with-guidance, not
  silent.)
- **`plugin_version` unreadable.** `_read_plugin_version` returns `""` (install record stores
  `""`); the hook env may also be empty. `_setup_health` only raises the **version_drift**
  cue when **both** the session's `plugin_version` and the recorded one are non-empty and
  differ. So an unknown version never produces a spurious "Sonari was updated" cue; at worst
  drift goes undetected until the next install with a known version. The `not_installed` cue is
  unaffected (it does not depend on version).
- **`launchctl` from a hook is avoided.** The session-start health check does **no**
  `launchctl` (only file stats), per the §3.C decision and the performance constraint. The
  only `launchctl` calls remain in `install`/`uninstall`/`doctor`, never on the hot path.
- **Version drift but user does not re-install.** The cue is spoken once per session; the
  daemon keeps running on the **old** `APP_DIR` copy (which still works - that is the whole
  point of decoupling). Nothing breaks; the user is simply nudged to re-run install to pick up
  the new version's code.
- **Second daemon races the guard.** If two daemons both pass `socket_connectable()` before
  either binds, the later `bind()` in `run()` raises `OSError` (address in use) - caught
  nowhere today, so it would crash that second process. That is acceptable (the first daemon
  is the live one); to be tidy, the build may let the exception fall through (process exits
  non-zero, no orphaned socket because the first daemon owns it). No additional handling is
  required by this spec beyond the guard in §3.D.1.
- **`SET_FOREGROUND` without `SESSION_START`.** `_maybe_guide_setup` is only called on the
  `SESSION_START` arm (not the bare `SET_FOREGROUND` arm), so a `UserPromptSubmit`-driven
  foreground change never re-triggers guidance - matching "at most once per session."
- **Plugin root with spaces / `&`.** Unchanged from the Phase 3 spec: the plist is
  XML-escaped (`_xml_escape`, cli.py:418-420) and `PYTHONPATH=<APP_DIR>` is a plain
  `~/.sonari/app` path (no spaces under the default home unless the username has one). The
  `_copy_app` source path is taken from `repo_root()` and may contain spaces; `shutil.copytree`
  handles that natively.

## 6. Concurrency note (single-instance)

The guard in §3.D.1 is a check-then-act and is therefore not race-free, but it eliminates the
**common** duplicate (lazy start finds the LaunchAgent already up, or vice versa) without a
lock file. A lock file (`flock` on `~/.sonari/speechd.lock`) was considered and rejected for
v0.4.0 as over-engineering for a single-user desktop tool - the residual race window is the
few milliseconds between the check and `bind()`, and the loser simply crashes with
`OSError: address in use` while the winner keeps the live socket. Documented here so the
implementer does not "fix" the residual race.

## 7. Testing strategy

All hermetic unit tests mock paths / `launchctl` / `swiftc`; **no** writes to the real `~/`
or `~/Library` (monkeypatch `HOME`, `paths.SONARI_DIR`, `paths.APP_DIR`, the LaunchAgent
paths, and `~/.local/bin`; stub `_launchctl`, `_resolve_python`, `_build_hotkeyd`). The full
suite must stay green under **both** `.venv` (3.13) and `.venv39` (3.9).

1. **APP_DIR copy + plist re-point (the durability fix).** `install()` copies the package so
   `<APP_DIR>/sonari/__init__.py` exists, and the speechd plist's
   `EnvironmentVariables.PYTHONPATH` equals `str(paths.APP_DIR)` - **not** the plugin/cache
   `src`. Assert the plist `ProgramArguments` is still `[<py>, -m, sonari.daemon]`.
2. **`install.json` new fields.** After `install()`, the record has `python`,
   `python_version`, `app_path` (= `str(paths.APP_DIR)`), `plugin_root`, `plugin_version`,
   `installed_at`; `app_path` points at `APP_DIR`; `plugin_version` matches the test plugin's
   `plugin.json`. Update the existing
   `tests/test_cli_install.py::test_write_install_record_writes_expected_keys` (currently
   passes `src="/plug/src"` and asserts `data["src"]`): change it to pass `app_path` +
   `plugin_version` and assert the new keys. `_write_install_record`'s only production caller
   is cli.py:557, which §3.B.2 already updates.
3. **Idempotent re-copy on update.** Run `install()` twice with two different fake plugin
   roots (simulating an update); assert `APP_DIR/sonari` reflects the **second** root's
   contents and a module present only in the first root is gone (remove-then-copy works), and
   the plist still points at `APP_DIR`.
4. **`uninstall()` removes `APP_DIR`, preserves config/keymap.** Create `APP_DIR`,
   `config.json`, `keymap.json`; after `uninstall()`, `APP_DIR` is gone and both `config.json`
   + `keymap.json` remain.
5. **Session-start detect-and-guide.** Drive `handle_message` with a `SESSION_START` message
   and assert, against a fake queue:
   - `not_installed` (no install.json) → exactly one `prose` item containing
     "slash sonari install".
   - launcher missing (install.json present, `~/.local/bin/sonari` absent) → the
     `not_installed` cue.
   - **speech-only OK** (install.json + launcher present, `HOTKEYD_BIN_PATH` **absent**,
     versions match) → **no** enqueue (silent) - a deliberate no-hotkeys user is never nagged.
   - healthy (install.json + launcher present, versions match) → **no** enqueue (silent).
6. **Version-drift cue.** install.json `plugin_version="0.3.0"`, session message
   `plugin_version="0.4.0"`, hotkeyd + launcher present → one `prose` item containing "Sonari
   was updated" / "slash sonari install". And: empty session version → **no** drift cue.
7. **Throttle.** Two `SESSION_START` messages for the same session in a degraded state → only
   **one** cue; `SESSION_END` then a new `SESSION_START` for the same session id → guidance
   may fire again (guard cleared).
8. **Single-instance guard.** `daemon.main()` returns immediately (no `SpeechDaemon` built)
   when `socket_connectable()` is patched `True`; builds + runs when patched `False`. (Patch
   `socket_connectable` and assert `SpeechDaemon.run` is/ isn't called.)
9. **Interpreter preference in shims.** `bin/sonari-daemon` (and `bin/sonari`) select
   `/usr/bin/python3` when present. Test by running the shim under a `PATH` whose first
   `python3` is a stub that writes a marker, with a stub `/usr/bin/python3` shadow not
   feasible - so instead assert via a shell-level test: invoke the shim with `set -x`/a
   `python3` shim earlier on PATH and confirm the shim picks `/usr/bin/python3` (a string/
   behavior assertion on the resolved `$py`). Minimum viable: a unit test that greps the shim
   source for the `[ -x /usr/bin/python3 ]`-first ordering, plus a subprocess smoke test that
   the shim still launches the daemon module.
10. **Command files exist + run the right command.** `commands/sonari:install.md` and
    `commands/sonari:uninstall.md` exist, have valid front-matter (`description:`), and their
    fenced command is exactly `sonari install` / `sonari uninstall`. Extend
    `tests/test_commands.py::test_all_command_files_exist` to include the two new files, and
    add `test_install_*`/`test_uninstall_*` cases mirroring `test_doctor_shows_output`
    (assert `"sonari install"`/`"sonari uninstall"`, `"Bash"`, and `"print"` in the file).
11. **`hooks_entry` SessionStart fields.** With `CLAUDE_PLUGIN_VERSION` / `CLAUDE_PLUGIN_ROOT`
    set in the env, `handle_event("SessionStart", …)` returns a `session_start` message
    carrying `plugin_version` + `plugin_root`; with them unset, both are `""` (and purity is
    preserved - no file I/O in `hooks_entry`).
12. **`_read_plugin_version`.** Reads `version` from a temp `plugin.json`; returns `""` on a
    missing/corrupt file; falls back to `CLAUDE_PLUGIN_VERSION`.
13. **APP_DIR copy failure path.** Patch `_copy_app` (or `shutil.copytree`) to raise `OSError`;
    `install()` prints the writable-`~/.sonari` guidance and returns 1 **without** writing a
    speechd plist that points at `APP_DIR`.
14. **Spaces in `$HOME` / `APP_DIR`.** With a monkeypatched home/`APP_DIR` whose path contains
    a space, `install()` writes a speechd plist whose `EnvironmentVariables.PYTHONPATH`
    round-trips correctly via `plistlib.loads` (value equals the spaced `APP_DIR`), confirming
    the XML-escape + env-entry handling holds for `APP_DIR`.

**Manual clean-room verification (exit criteria):** see §9.

## 8. Spec self-review (done before saving)

- **Placeholder scan:** no `TODO`/`TBD`/`???`/`<placeholder>` remain; every `<…>` is a named
  runtime value (abs paths, resolved interpreter, version strings) defined in context.
- **Internal consistency:** `APP_DIR` is defined once (§3.B.1) and used identically by the
  copy (§3.B.2), the plist PYTHONPATH (§3.B.2), `install.json` `app_path` (§3.B.3), `doctor`
  (§3.B.3), `uninstall` (§3.B.4), the daemon health check (§3.C.2), and the tests (§7). The
  `src`→`app_path` install-record key rename is applied consistently in `_write_install_record`
  (§3.B.3) and the `doctor` reader (§3.B.3). The "guide only, never auto-run" decision is
  stated in §3.C and reflected in §5 (no `launchctl` on the hot path). The single-instance
  guard lives in `main()` (not `run()`) consistently in §3.D.1, §5, §6, §7.
- **Scope:** every change is on-ramp / durability / hardening; onboarding docs, auto-heal,
  Windows are explicitly deferred (§10). Version bump is the only "feature-ish" change and is
  required by the publish convention.
- **Ambiguity resolved:** (a) version source for drift = `.claude-plugin/plugin.json`
  `version` with `CLAUDE_PLUGIN_VERSION` fallback; (b) drift cue fires only when both versions
  are known and differ; (c) the hotkeyd binary is **NOT** part of the cue's "installed" check -
  once `install.json` + the launcher exist, a speech-only user is never nagged; a missing
  hotkeyd is surfaced by `doctor` only (§3.C.2 + §11); (d) APP_DIR copy failure is fatal-with-guidance
  (§5); (e) single-instance guard is best-effort, no lock file (§6).

## 9. Verification list (confirm empirically during the build)

1. Fresh **public-marketplace** install on a clean profile: `claude plugin marketplace add
   nimkimi/sonari` + `claude plugin install sonari@sonari`, enable, start a session → hear
   Claude (lazy daemon) **and** hear the "run /sonari:install" cue **once**.
2. `/sonari:install` runs `sonari install` via the Bash tool eyes-free, prints each step, and
   `/sonari:doctor` then reports green (or only the expected `swiftc`/CLT FAIL on a machine
   without Command Line Tools).
3. After install, the speechd plist `EnvironmentVariables.PYTHONPATH` is `~/.sonari/app` (not
   the cache), and `~/.sonari/app/sonari/__init__.py` exists.
4. **Simulated plugin-version bump:** change the recorded vs. session `plugin_version`
   (e.g. install at 0.3.0, then start a session reporting 0.4.0) → hear the "Sonari was
   updated. Run /sonari:install" cue once; speech/hotkeys still work on the old copy before
   re-install; after re-running `/sonari:install` the plist re-points to the refreshed
   `~/.sonari/app` cleanly and the cue stops.
5. **Cache prune resilience:** with the daemon installed, delete/rename the marketplace cache
   `…/<version>/src`; restart the speechd LaunchAgent (`launchctl kickstart -k`) → it still
   imports and speaks (because PYTHONPATH is `~/.sonari/app`).
6. **Single-instance:** with the LaunchAgent daemon live, trigger a lazy start (or run
   `bin/sonari-daemon`) → the second process exits without orphaning the socket; only one
   speaker is heard.
7. **Interpreter consistency:** with Homebrew `python3` first on PATH, confirm the lazily
   started daemon runs `/usr/bin/python3` (e.g. `ps`/`lsof` on the daemon pid, or a log line),
   matching the installed daemon.
8. **Launcher robustness:** after `sonari install`, `~/.local/bin/sonari status` works in a
   fresh shell; if the launcher is later removed by hand, a new session speaks the
   `not_installed` cue (re-surfacing the vanish) and `/sonari:install` recreates it.
   **Investigate** the original vanish root cause during this step (note findings; no fix
   required by this spec beyond the defensive re-create + health surfacing).
9. **Uninstall:** `/sonari:uninstall` (or `sonari uninstall`) removes both LaunchAgents, the
   hotkeyd binary, the launcher, and `~/.sonari/app`, and **preserves** `config.json` +
   `keymap.json`.
10. Full suite green under **both** `.venv` (3.13) and `.venv39` (3.9).

## 10. Out of scope (deferred - separate sub-projects / later)

- **Full onboarding docs / accessibility getting-started / screencasts** - Phase 3
  **sub-project #3** (the original packaging spec's deferred onboarding work). This spec makes
  only the minimal README edits in §4.
- **Auto-heal on update** (a hook or LaunchAgent that silently re-runs `install` after a
  plugin auto-update, or a `swiftc`/`launchctl` step driven from a hook) - explicitly rejected
  for v0.4.0: the approved decision is **guide only** (spoken cue + `/sonari:install`), never
  silent privileged actions from a hook. Revisit only if the spoken-cue on-ramp proves
  insufficient in practice.
- **Windows / non-macOS support** (the standing platform issue #1) - `say`/`afplay`/Carbon are
  macOS-only; out of scope as in every prior phase.
- **Lock-file-based single-instance** (`flock`) - rejected as over-engineering for a
  single-user desktop tool (§6); the best-effort `socket_connectable()` guard is sufficient.
- **Cross-machine / multi-macOS QA matrix**, **CI on GitHub Actions for the dual-interpreter
  gate**, **PyPI/Homebrew distribution**, **codesign/notarization** - all remain deferred per
  the Phase 3 packaging spec §10.

## 11. Considered alternatives (recorded, not adopted)

- **Nagging when the hotkeyd binary is absent - REJECTED.** The alternative was to treat a
  missing `HOTKEYD_BIN_PATH` as "not installed" so the cue keeps firing until hotkeys are
  built. Rejected because it nags a deliberate speech-only user (no `swiftc`/Command Line
  Tools) once every session. **Adopted instead (§3.C.2):** once `install.json` + the launcher
  exist the user counts as installed; a missing hotkeyd is surfaced by `sonari doctor` (with
  the `xcode-select --install` guidance), not by the spoken session cue. A genuinely fresh
  user is still nagged because they have no `install.json` at all.
- **Auto-run `install` from the hook on detected drift.** Rejected (see §10): silent
  `launchctl`/`swiftc` from a hook is a surprising privileged action and violates the thin-hook
  contract.
- **Lock file for single-instance.** Rejected (see §6).
