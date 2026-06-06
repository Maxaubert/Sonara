# Sonari Phase 3.1 — Packaging Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden Sonari's published packaging so a fresh marketplace user can finish setup eyes-free (slash-command on-ramp + spoken cue) and so the long-lived speech daemon survives a plugin auto-update by running from a stable `~/.sonari/app` copy instead of the version-pinned marketplace cache.

**Architecture:** Five change clusters, all on-ramp/durability/hardening (no new user features): (A) `commands/sonari:install.md` + `sonari:uninstall.md` slash commands piggybacking the existing `bin/sonari` shim; (B) `install()` copies `src/sonari` → `~/.sonari/app/sonari` and points the speechd LaunchAgent's `PYTHONPATH` there, recording `app_path`+`plugin_version` in `install.json`; (C) the daemon evaluates cheap setup-health on `SESSION_START` and speaks exactly one throttled guidance cue only when degraded; (D) single-instance daemon guard + interpreter-preference fix in the shims + defensive launcher verification; plus the docs/version bump to 0.4.0.

**Tech Stack:** Python 3.9–3.13 (dual-interpreter gate: `.venv` = 3.13, `.venv39` = 3.9.6), pytest, macOS LaunchAgents (`launchctl`), bash shims, JSON manifests. Source files live under `src/sonari/`; tests under `tests/`. PROTOCOL_VERSION stays `1`.

---

## File map (created / modified)

| File | Responsibility | Tasks |
|---|---|---|
| `src/sonari/paths.py` | Add `APP_DIR` constant (stable PYTHONPATH target) | 1 |
| `src/sonari/cli.py` | `_read_plugin_version`, `_copy_app`, install durability, `_write_install_record` signature, doctor `app_path`, uninstall removes `APP_DIR`, launcher verify | 2,3,4 |
| `src/sonari/hooks_entry.py` | Carry `plugin_version`+`plugin_root` on `SessionStart` (env reads only; pure) | 5 |
| `bin/sonari-hook` | Resolve `CLAUDE_PLUGIN_VERSION` from `plugin.json` if unset (failure-tolerant) | 5 |
| `src/sonari/daemon.py` | `_read_install_record`, `_launcher_present`, `_setup_health`, `_maybe_guide_setup`, wire into `SESSION_START`/`SESSION_END`; single-instance guard in `main()` | 6,7 |
| `bin/sonari-daemon`, `bin/sonari` | Prefer `/usr/bin/python3` first | 8 |
| `commands/sonari:install.md`, `commands/sonari:uninstall.md` | Slash-command on-ramp | 9 |
| `.claude-plugin/plugin.json`, `pyproject.toml`, `.claude-plugin/marketplace.json`, `README.md`, `docs/...verification.md` | Version 0.4.0 + docs | 10 |

**Iterate with:** `.venv/bin/python -m pytest -q` (3.13). **Dual-interpreter gate (Task 10 + recommended after each task):** also run `.venv39/bin/python -m pytest -q` (3.9.6). Both venvs already exist.

**Git rule:** each task ends with `git add <exact files>` + `git commit`. Only `git add`/`git commit` are permitted (no reset/checkout/rebase/amend/stash/clean/rm/push). Branch is `main`.

---

## Task 1: `paths.py` — `APP_DIR` constant

**Files:**
- Modify: `src/sonari/paths.py:7-14`
- Test: `tests/test_paths.py`

- [ ] **Step 1: Write the failing test**

Append to `tests/test_paths.py` (after `test_install_record_path_lives_under_sonari_dir`, line 122):

```python
def test_app_dir_lives_under_sonari_dir(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.APP_DIR == paths.SONARI_DIR / "app"
    assert paths.APP_DIR.name == "app"
    assert paths.APP_DIR.parent == paths.SONARI_DIR
```

- [ ] **Step 2: Run test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_paths.py::test_app_dir_lives_under_sonari_dir -v`
Expected: FAIL with `AttributeError: module 'sonari.paths' has no attribute 'APP_DIR'`

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/paths.py`, add the `APP_DIR` line immediately after `SONARI_DIR` (line 7):

```python
SONARI_DIR = Path.home() / ".sonari"
APP_DIR = SONARI_DIR / "app"          # stable copy of the sonari package (PYTHONPATH target)
CONFIG_PATH = SONARI_DIR / "config.json"
```

- [ ] **Step 4: Run test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_paths.py -v`
Expected: PASS (all paths tests, including the new one)

- [ ] **Step 5: Commit**

```bash
git add src/sonari/paths.py tests/test_paths.py
git commit -m "feat(paths): add APP_DIR stable app-copy constant"
```

---

## Task 2: `cli.py` — `_read_plugin_version()` helper

Read the plugin's declared `version` from `<plugin_root>/.claude-plugin/plugin.json`, falling back to the `CLAUDE_PLUGIN_VERSION` env var, else `""`. Never raises (version is advisory).

**Files:**
- Modify: `src/sonari/cli.py` (add helper near `_read_install_record`, ~line 370)
- Test: `tests/test_cli_install.py`

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_cli_install.py`:

```python
def test_read_plugin_version_reads_version_from_plugin_json(tmp_path, monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    pdir = tmp_path / ".claude-plugin"
    pdir.mkdir()
    (pdir / "plugin.json").write_text('{"name": "sonari", "version": "0.4.0"}')
    assert cli._read_plugin_version(str(tmp_path)) == "0.4.0"


def test_read_plugin_version_missing_file_returns_empty(tmp_path, monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    assert cli._read_plugin_version(str(tmp_path)) == ""


def test_read_plugin_version_corrupt_file_returns_empty(tmp_path, monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    pdir = tmp_path / ".claude-plugin"
    pdir.mkdir()
    (pdir / "plugin.json").write_text("{ not json")
    assert cli._read_plugin_version(str(tmp_path)) == ""


def test_read_plugin_version_falls_back_to_env(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_PLUGIN_VERSION", "9.9.9")
    # No plugin.json on disk -> env fallback wins.
    assert cli._read_plugin_version(str(tmp_path)) == "9.9.9"
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py -k read_plugin_version -v`
Expected: FAIL with `AttributeError: module 'sonari.cli' has no attribute '_read_plugin_version'`

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/cli.py`, add this helper immediately after `_read_install_record` (after line 369):

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

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py -k read_plugin_version -v`
Expected: PASS (4 tests)

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_install.py
git commit -m "feat(cli): add _read_plugin_version helper (plugin.json + env fallback)"
```

---

## Task 3: `cli.py` — durability fix (`_copy_app` + plist → `APP_DIR` + install record fields)

This is the core durability change and the **biggest red-window risk in cli**. It changes `_write_install_record`'s signature, makes `install()` copy the package into `APP_DIR` and point the speechd plist there, and updates `doctor()` to read `app_path`. The three install tests and the install-record test are updated **in this same task** so the suite stays green at the commit.

**Files:**
- Modify: `src/sonari/cli.py` — `_write_install_record` (345-359), `doctor()` (226-234), `install()` (542-563), add `_copy_app`
- Test: `tests/test_cli_install.py` (`test_write_install_record_writes_expected_keys`, `test_install_writes_plist_and_loads`, + new tests), `tests/test_cli_hotkeyd.py` (`test_install_writes_hotkeyd_plist_and_keymap`, `test_install_build_failure_is_nonfatal`)

### Step group A — `_copy_app` helper (TDD)

- [ ] **Step 1: Write the failing test**

Append to `tests/test_cli_install.py`:

```python
def test_copy_app_copies_package_into_app_dir(tmp_path):
    plugin_root = tmp_path / "plugin"
    src_pkg = plugin_root / "src" / "sonari"
    src_pkg.mkdir(parents=True)
    (src_pkg / "__init__.py").write_text("# sonari\n")
    (src_pkg / "daemon.py").write_text("# daemon\n")
    app_dir = tmp_path / "home" / ".sonari" / "app"
    with mock.patch.object(cli.paths, "APP_DIR", app_dir):
        returned = cli._copy_app(str(plugin_root))
    assert returned == str(app_dir)
    assert (app_dir / "sonari" / "__init__.py").exists()
    assert (app_dir / "sonari" / "daemon.py").exists()


def test_copy_app_is_remove_then_copy_so_stale_modules_vanish(tmp_path):
    app_dir = tmp_path / "home" / ".sonari" / "app"

    def _root_with(modules):
        root = tmp_path / ("plug-" + "-".join(modules))
        pkg = root / "src" / "sonari"
        pkg.mkdir(parents=True)
        (pkg / "__init__.py").write_text("# pkg\n")
        for m in modules:
            (pkg / m).write_text("# " + m + "\n")
        return root

    first = _root_with(["old_only.py", "daemon.py"])
    second = _root_with(["daemon.py"])
    with mock.patch.object(cli.paths, "APP_DIR", app_dir):
        cli._copy_app(str(first))
        assert (app_dir / "sonari" / "old_only.py").exists()
        cli._copy_app(str(second))
    # The module present only in the FIRST root must be gone after re-copy.
    assert not (app_dir / "sonari" / "old_only.py").exists()
    assert (app_dir / "sonari" / "daemon.py").exists()


def test_copy_app_raises_oserror_when_source_missing(tmp_path):
    plugin_root = tmp_path / "plugin"  # no src/sonari beneath it
    app_dir = tmp_path / "home" / ".sonari" / "app"
    with mock.patch.object(cli.paths, "APP_DIR", app_dir):
        try:
            cli._copy_app(str(plugin_root))
            raised = False
        except OSError:
            raised = True
    assert raised is True
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py -k copy_app -v`
Expected: FAIL with `AttributeError: module 'sonari.cli' has no attribute '_copy_app'`

- [ ] **Step 3: Write minimal implementation**

In `src/sonari/cli.py`, add this helper immediately after `_place_launcher` (after line 396; `os` and `shutil` are already imported at lines 14-16):

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

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py -k copy_app -v`
Expected: PASS (3 tests). `copytree` raises `FileNotFoundError` (an `OSError` subclass) when the source is missing, satisfying the third test.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_install.py
git commit -m "feat(cli): add _copy_app (remove-then-copy package into stable APP_DIR)"
```

### Step group B — `_write_install_record` signature + install() wiring + doctor() (TDD, all in one commit)

- [ ] **Step 6: Update the failing install-record test**

Replace `tests/test_cli_install.py::test_write_install_record_writes_expected_keys` (lines 108-123) with:

```python
def test_write_install_record_writes_expected_keys(tmp_path):
    rec = tmp_path / "install.json"
    with mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", rec):
        cli._write_install_record(
            python="/usr/bin/python3",
            python_version="3.9",
            plugin_root="/plug",
            app_path="/home/u/.sonari/app",
            plugin_version="0.4.0",
        )
    import json as _json
    data = _json.loads(rec.read_text())
    assert data["python"] == "/usr/bin/python3"
    assert data["python_version"] == "3.9"
    assert data["plugin_root"] == "/plug"
    assert data["app_path"] == "/home/u/.sonari/app"
    assert data["plugin_version"] == "0.4.0"
    assert "src" not in data  # src key was replaced by app_path
    assert "installed_at" in data and isinstance(data["installed_at"], str)
```

- [ ] **Step 7: Update the failing speechd-install test**

Replace `tests/test_cli_install.py::test_install_writes_plist_and_loads` (lines 46-88) with the version below. It mocks `_copy_app` so no real `copytree` runs, points `paths.APP_DIR` at a tmp dir, and asserts `PYTHONPATH == str(APP_DIR)` and the new record keys:

```python
def test_install_writes_plist_and_loads(tmp_path, capsys):
    la_dir = tmp_path / "LaunchAgents"
    plist = la_dir / (cli.LAUNCH_AGENT_LABEL + ".plist")
    record = tmp_path / "install.json"
    app_dir = tmp_path / ".sonari" / "app"
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_probe_python_version", return_value=(3, 9)), \
         mock.patch.object(cli, "_build_hotkeyd", return_value=(True, "built")), \
         mock.patch.object(cli, "_copy_app", return_value=str(app_dir)) as copy_app, \
         mock.patch.object(cli, "_read_plugin_version", return_value="0.4.0"), \
         mock.patch.object(cli, "_place_launcher", return_value=str(tmp_path / "launcher")) as place_launcher, \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(tmp_path / "com.sonari.hotkeyd.plist")), \
         mock.patch.object(cli.paths, "APP_DIR", app_dir), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir", lambda: tmp_path.mkdir(parents=True, exist_ok=True)), \
         mock.patch("sonari.paths.ensure_sonari_dir") as ensure:
        rc = cli.install()
    assert rc == 0
    ensure.assert_called_once()
    copy_app.assert_called_once()
    assert plist.exists()
    # The speechd plist embeds the resolved interpreter + PYTHONPATH=<APP_DIR>.
    data = plistlib.loads(plist.read_text().encode("utf-8"))
    assert data["ProgramArguments"][0] == "/usr/bin/python3"
    assert data["ProgramArguments"][1:] == ["-m", "sonari.daemon"]
    assert data["EnvironmentVariables"]["PYTHONPATH"] == str(app_dir)
    # install.json was written with the resolved interpreter + new fields.
    import json as _json
    rec = _json.loads(record.read_text())
    assert rec["python"] == "/usr/bin/python3"
    assert rec["app_path"] == str(app_dir)
    assert rec["plugin_version"] == "0.4.0"
    assert "src" not in rec
    place_launcher.assert_called_once()
    assert any(c.args[0][0] == "load" for c in run.call_args_list)
    out = capsys.readouterr().out
    assert "doctor" in out.lower()
```

- [ ] **Step 8: Add the copy-failure-fatal test (spec §5) and the spaces-in-$HOME plist test (spec §7 test 14)**

Append to `tests/test_cli_install.py`:

```python
def test_install_copy_failure_is_fatal_and_writes_no_plist(tmp_path, capsys):
    plist = tmp_path / "com.sonari.speechd.plist"
    record = tmp_path / "install.json"
    app_dir = tmp_path / ".sonari" / "app"
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", mock.Mock(return_value=0)), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_probe_python_version", return_value=(3, 9)), \
         mock.patch.object(cli, "_build_hotkeyd", return_value=(True, "built")), \
         mock.patch.object(cli, "_copy_app", side_effect=OSError("read-only")), \
         mock.patch.object(cli.paths, "APP_DIR", app_dir), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir", lambda: None), \
         mock.patch("sonari.paths.ensure_sonari_dir"):
        rc = cli.install()
    assert rc == 1
    # No speechd plist was written when the copy failed.
    assert not plist.exists()
    out = capsys.readouterr().out.lower()
    assert "~/.sonari" in out or ".sonari is writable" in out


def test_install_plist_pythonpath_handles_spaces_in_app_dir(tmp_path, capsys):
    plist = tmp_path / "com.sonari.speechd.plist"
    record = tmp_path / "install.json"
    # APP_DIR with a space in the path (e.g. a username with a space).
    app_dir = tmp_path / "Spaced Home" / ".sonari" / "app"
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", mock.Mock(return_value=0)), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_probe_python_version", return_value=(3, 9)), \
         mock.patch.object(cli, "_build_hotkeyd", return_value=(False, "swiftc not found")), \
         mock.patch.object(cli, "_copy_app", return_value=str(app_dir)), \
         mock.patch.object(cli, "_read_plugin_version", return_value="0.4.0"), \
         mock.patch.object(cli, "_place_launcher", return_value=str(tmp_path / "launcher")), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(tmp_path / "hk.plist")), \
         mock.patch.object(cli.paths, "APP_DIR", app_dir), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir", lambda: None), \
         mock.patch("sonari.paths.ensure_sonari_dir"):
        rc = cli.install()
    assert rc == 0
    data = plistlib.loads(plist.read_text().encode("utf-8"))
    # The spaced path round-trips through the plist XML intact.
    assert data["EnvironmentVariables"]["PYTHONPATH"] == str(app_dir)
    assert " " in data["EnvironmentVariables"]["PYTHONPATH"]
```

- [ ] **Step 9: Run the updated tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py -v`
Expected: FAIL — `test_write_install_record_writes_expected_keys` fails on the new keyword args (`_write_install_record() got an unexpected keyword argument 'app_path'`); `test_install_writes_plist_and_loads`, `test_install_copy_failure_is_fatal_and_writes_no_plist`, and `test_install_plist_pythonpath_handles_spaces_in_app_dir` fail because `install()` does not yet call `_copy_app`/use `APP_DIR`/handle the OSError.

- [ ] **Step 10: Write the implementation**

(a) In `src/sonari/cli.py`, replace `_write_install_record` (lines 345-359) with:

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

(b) In `doctor()`, replace the "plugin path resolved" read (lines 226-232) with:

```python
        rec = _read_install_record()
        app = rec.get("app_path") if rec else None
        init = os.path.join(app, "sonari", "__init__.py") if app else None
        ok = bool(init) and os.path.exists(init)
        results.append(("plugin path resolved", ok,
                        app if ok else "install.json missing or app copy has no "
                                       "sonari package (run 'sonari install')"))
```

(c) In `install()`, replace lines 542-563 (from `plugin_root = ...` through the speechd plist build) with:

```python
    plugin_root = os.path.realpath(paths.repo_root())

    # 2. Pre-check swiftc / Command Line Tools (non-fatal).
    if shutil.which("swiftc") is None:
        print("Xcode Command Line Tools not found; global hotkeys disabled. "
              "Install them with:  xcode-select --install   then re-run: "
              "sonari install")

    # 3. Copy the package into the stable APP_DIR (decouples the long-lived
    #    daemon from the version-pinned marketplace cache; see spec §3.B).
    #    Fatal-with-guidance: a half-copy must not produce a dangling plist.
    try:
        app_dir = _copy_app(plugin_root)
    except OSError as exc:
        print(f"Could not copy the runtime to ~/.sonari/app: {exc}. "
              f"Check that ~/.sonari is writable.")
        return 1
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

    # 6. speechd LaunchAgent (resolved interpreter + PYTHONPATH=<APP_DIR>).
    log = str(paths.LOG_PATH)
    xml = _launchagent_plist(python_executable=python, src_path=app_dir,
                             log_path=log)
```

(d) Update the `_launchagent_plist` docstring (lines 482-485) comment-only, replacing "the plugin's `<root>/src` directory" with "the stable `APP_DIR` copy":

```python
    *python_executable* is the resolved absolute interpreter (>= 3.9).
    *src_path* is the stable APP_DIR copy (~/.sonari/app); it is injected as
    PYTHONPATH so the daemon imports the stable package copy, surviving plugin
    cache churn. ProgramArguments runs the module directly: [<py>, -m, sonari.daemon].
```

- [ ] **Step 11: Run the install tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py -v`
Expected: PASS (all, including the four new/updated install tests)

- [ ] **Step 12: Update the two hotkeyd install tests (still broken by the signature/APP_DIR change)**

In `tests/test_cli_hotkeyd.py`, both `test_install_writes_hotkeyd_plist_and_keymap` (53-86) and `test_install_build_failure_is_nonfatal` (88-118) call `cli.install()` without mocking `_copy_app`/`_read_plugin_version`/`paths.APP_DIR`, so they now run a real `copytree` against the live repo (or fail). Add three mocks to **each** `with` block. For `test_install_writes_hotkeyd_plist_and_keymap`, insert after the `_place_launcher` mock (line 65):

```python
         mock.patch.object(cli, "_copy_app", return_value=str(tmp_path / "app")), \
         mock.patch.object(cli, "_read_plugin_version", return_value="0.4.0"), \
         mock.patch.object(cli.paths, "APP_DIR", tmp_path / "app"), \
```

For `test_install_build_failure_is_nonfatal`, insert after its `_place_launcher` mock (line 100):

```python
         mock.patch.object(cli, "_copy_app", return_value=str(tmp_path / "app")), \
         mock.patch.object(cli, "_read_plugin_version", return_value="0.4.0"), \
         mock.patch.object(cli.paths, "APP_DIR", tmp_path / "app"), \
```

- [ ] **Step 13: Run the hotkeyd tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_cli_hotkeyd.py -v`
Expected: PASS (all)

- [ ] **Step 14: Run the full cli suite to confirm no regression**

Run: `.venv/bin/python -m pytest tests/test_cli_install.py tests/test_cli_hotkeyd.py tests/test_cli_uninstall.py -v`
Expected: PASS (uninstall still green — it does not yet exercise APP_DIR; Task 4 adds that)

- [ ] **Step 15: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_install.py tests/test_cli_hotkeyd.py
git commit -m "feat(cli): copy package to stable APP_DIR + record app_path/plugin_version (durability fix)"
```

---

## Task 4: `cli.py` — `uninstall()` removes `APP_DIR` (preserving config/keymap)

**Files:**
- Modify: `src/sonari/cli.py` — `uninstall()` (after the artifacts loop, ~line 668)
- Test: `tests/test_cli_uninstall.py`

- [ ] **Step 1: Update `test_uninstall_removes_launchagent_but_preserves_keymap_and_config` to create + assert-remove `APP_DIR`**

In `tests/test_cli_uninstall.py`, after the `record` setup (line 22) add an `APP_DIR` tree:

```python
    record = sonari_dir / "install.json"
    record.write_text("{}")
    # The stable app copy uninstall should remove.
    app_dir = sonari_dir / "app"
    (app_dir / "sonari").mkdir(parents=True)
    (app_dir / "sonari" / "__init__.py").write_text("# pkg\n")
```

Add `APP_DIR` to the patch block (after the `INSTALL_RECORD_PATH` mock, line 43):

```python
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "APP_DIR", app_dir), \
```

And add the removal assertion after `assert not record.exists()` (line 54):

```python
    assert not record.exists()
    assert not app_dir.exists()
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `.venv/bin/python -m pytest tests/test_cli_uninstall.py::test_uninstall_removes_launchagent_but_preserves_keymap_and_config -v`
Expected: FAIL on `assert not app_dir.exists()` (uninstall does not yet remove `APP_DIR`)

- [ ] **Step 3: Write the implementation**

In `src/sonari/cli.py`, in `uninstall()`, insert this block immediately after the `for artifact in artifacts:` loop (after line 668, before the `if _remove_launcher():` block):

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

- [ ] **Step 4: Run the test to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_cli_uninstall.py -v`
Expected: PASS (both uninstall tests; config.json + keymap.json still preserved)

- [ ] **Step 5: Commit**

```bash
git add src/sonari/cli.py tests/test_cli_uninstall.py
git commit -m "feat(cli): uninstall removes ~/.sonari/app (config/keymap preserved)"
```

---

## Task 5: `hooks_entry.py` + `bin/sonari-hook` — `SessionStart` carries `plugin_version` + `plugin_root`

The hook shim resolves `CLAUDE_PLUGIN_VERSION` from `plugin.json` if unset (file I/O lives in the shim, which is already wrapped in a total try/except). `hooks_entry.handle_event` stays PURE (env reads only) and adds the two fields to the `SESSION_START` message.

**Files:**
- Modify: `src/sonari/hooks_entry.py:96-100`
- Modify: `bin/sonari-hook` (after the `handle_event` import, line 45)
- Test: `tests/test_hooks_entry.py`

- [ ] **Step 1: Write the failing tests**

In `tests/test_hooks_entry.py`, **replace** `test_session_start_sets_foreground_then_session_start` (244-248) and **add** the env-unset + env-set cases. The existing test breaks because the message now carries two extra fields, so it is updated here in the same task:

```python
def test_session_start_carries_plugin_version_and_root_from_env(monkeypatch):
    monkeypatch.setenv("CLAUDE_PLUGIN_VERSION", "0.4.0")
    monkeypatch.setenv("CLAUDE_PLUGIN_ROOT", "/plug/root")
    assert handle_event("SessionStart", {"session_id": "sess-9"}) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "sess-9"},
        {"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START, "session": "sess-9",
         "plugin_version": "0.4.0", "plugin_root": "/plug/root"},
    ]


def test_session_start_empty_strings_when_env_unset(monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    monkeypatch.delenv("CLAUDE_PLUGIN_ROOT", raising=False)
    msgs = handle_event("SessionStart", {"session_id": "sess-9"})
    assert msgs[1]["plugin_version"] == ""
    assert msgs[1]["plugin_root"] == ""
```

Also update `test_missing_session_id_defaults_to_empty_string` (261-263) to clear the env so its `session == ""` assertions are unaffected by an ambient `CLAUDE_PLUGIN_*`:

```python
def test_missing_session_id_defaults_to_empty_string(monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    monkeypatch.delenv("CLAUDE_PLUGIN_ROOT", raising=False)
    msgs = handle_event("SessionStart", {})
    assert msgs[0]["session"] == ""
    assert msgs[1]["session"] == ""
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_hooks_entry.py -k session_start -v`
Expected: FAIL — `test_session_start_carries_plugin_version_and_root_from_env` and `test_session_start_empty_strings_when_env_unset` fail because the `SESSION_START` message does not yet include `plugin_version`/`plugin_root`.

- [ ] **Step 3: Write the `hooks_entry.py` implementation**

In `src/sonari/hooks_entry.py`, replace the `SessionStart` branch (lines 96-100) with (`os` is already imported, line 4):

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

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_hooks_entry.py -v`
Expected: PASS (all hooks_entry tests)

- [ ] **Step 5: Add the shim env-resolution to `bin/sonari-hook`**

In `bin/sonari-hook`, immediately after `from sonari.hooks_entry import handle_event` (line 45), insert (`here` is defined at line 39; `os`/`sys` are imported at top):

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

- [ ] **Step 6: Smoke-test the shim resolves and exports the version**

Run (the shim sets `CLAUDE_PLUGIN_VERSION` from this repo's plugin.json, which Task 10 will be `0.4.0`; until then it is `0.3.0` — assert it is non-empty so the test is version-agnostic):

```bash
.venv/bin/python - <<'PY'
import os, subprocess, sys
repo = os.path.abspath(".")
# Run the shim for an event that needs no daemon work but exercises the import +
# env-resolution path; UserPromptSubmit emits messages but ensure_daemon/send are
# wrapped in try/except so no daemon is required.
env = dict(os.environ)
env.pop("CLAUDE_PLUGIN_VERSION", None)
env["CLAUDE_PLUGIN_ROOT"] = repo
# Print the resolved version from inside the shim's import context.
code = (
    "import os,sys; here=os.path.join(%r,'bin');"
    "root=os.environ.get('CLAUDE_PLUGIN_ROOT');"
    "import json;"
    "print(json.load(open(os.path.join(root,'.claude-plugin','plugin.json')))['version'])"
) % repo
out = subprocess.check_output([sys.executable, "-c", code], env=env, text=True).strip()
assert out, "plugin.json version must be non-empty"
print("plugin.json version:", out)
PY
```

Expected: prints the current plugin.json version (non-empty); exit 0. This confirms the resolution logic the shim relies on.

- [ ] **Step 7: Commit**

```bash
git add src/sonari/hooks_entry.py bin/sonari-hook tests/test_hooks_entry.py
git commit -m "feat(hooks): SessionStart carries plugin_version + plugin_root (shim resolves version)"
```

---

## Task 6: `daemon.py` — detect-and-guide spoken setup health on `SESSION_START`

**The biggest red-window risk.** The daemon now evaluates `_setup_health` on `SESSION_START` and may enqueue a spoken cue. Any existing test feeding `SESSION_START` and asserting queue/spoken state breaks unless health is arranged to `"ok"` or the expected output is updated. We add a monkeypatch-friendly path and fix the two affected tests in this same task.

**Existing tests touched (and why):**
- `tests/test_daemon_control.py::test_session_start_sets_foreground_and_registers` — feeds `SESSION_START` via `handle_message`; would now enqueue a `not_installed` cue (hermetic tmp `~/.sonari` has no install.json). Fix: monkeypatch `SpeechDaemon._setup_health` → `("ok", None)` so it stays focused on foreground/register.
- `tests/test_e2e_pipeline.py::test_scripted_session_full_ordering` and `test_background_session_is_earcon_only` — drive `SessionStart` with an EXACT expected log; hermetic state is `not_installed`, so a cue would fire and break ordering. Fix: patch `_setup_health` → `("ok", None)` on the daemon so ordering assertions stay focused.
- `tests/test_daemon_phase2.py` and `tests/test_daemon_decisions.py` — verified: they do **not** feed `SESSION_START`/`session_start` through `handle_message` (they use `sessions.set_foreground(...)` directly and CHOICE/PLAN/PERMISSION/TOOL messages), so they need **no** change.

**Files:**
- Modify: `src/sonari/daemon.py` — imports (line 12), `__init__` (line 35), add `_maybe_guide_setup` (after `_enqueue`, ~line 57), add health helpers (after `_choice_notes`, ~line 119), wire `SESSION_START` (191-195) + `SESSION_END` (197-201)
- Test: `tests/test_daemon_setup_health.py` (new), `tests/test_daemon_control.py`, `tests/test_e2e_pipeline.py`

### Step group A — health helpers (`_read_install_record`, `_launcher_present`, `_setup_health`)

- [ ] **Step 1: Write the failing tests (new file)**

Create `tests/test_daemon_setup_health.py`:

```python
import os
from unittest import mock

from tests.daemon_helpers import make_daemon


def _write_install_json(tmp_path, plugin_version="0.4.0"):
    import sonari.daemon as daemon_mod
    rec = tmp_path / "install.json"
    import json
    rec.write_text(json.dumps({"plugin_version": plugin_version}))
    return rec


def test_setup_health_not_installed_when_no_record(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    missing = tmp_path / "install.json"  # never created
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(missing))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "not_installed"
    assert "slash sonari install" in cue.lower()


def test_setup_health_not_installed_when_launcher_missing(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path)
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: False)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "not_installed"
    assert "slash sonari install" in cue.lower()


def test_setup_health_ok_speech_only_no_hotkeyd(tmp_path, monkeypatch):
    # install.json + launcher present, hotkeyd binary ABSENT, versions match.
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.4.0")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr("sonari.daemon.HOTKEYD_BIN_PATH",
                        str(tmp_path / "nope" / "sonari-hotkeyd"))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "ok"
    assert cue is None


def test_setup_health_ok_when_versions_match(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.4.0")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "ok"
    assert cue is None


def test_setup_health_version_drift(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.3.0")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "version_drift"
    assert "updated" in cue.lower()
    assert "slash sonari install" in cue.lower()


def test_setup_health_no_drift_when_session_version_empty(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.3.0")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("")  # unknown session version
    assert state == "ok"
    assert cue is None


def test_read_install_record_returns_none_on_corrupt(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = tmp_path / "install.json"
    rec.write_text("{ not json")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    assert daemon._read_install_record() is None
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_daemon_setup_health.py -v`
Expected: FAIL with `AttributeError: 'SpeechDaemon' object has no attribute '_setup_health'` (and `_launcher_present`, `_read_install_record`)

- [ ] **Step 3: Write the implementation — imports + health helpers**

(a) In `src/sonari/daemon.py`, replace the paths import (line 12) with:

```python
from sonari.paths import (
    SOCKET_PATH, ensure_sonari_dir, socket_connectable, repo_root,
    INSTALL_RECORD_PATH,
)
```

(Note: `HOTKEYD_BIN_PATH` is deliberately **not** imported — the cue's "installed" check excludes hotkeyd so speech-only users are never nagged. The test patches `sonari.daemon.HOTKEYD_BIN_PATH` only to prove its absence is irrelevant; that monkeypatch target tolerates a missing attribute is **not** guaranteed, so the speech-only test patches it with `raising=False`-equivalent behavior via setattr on a fresh name — to keep it simple, the test above sets it and the absence of the import means the cue logic never reads it. If `monkeypatch.setattr` on a non-existent module attribute errors in your pytest, change that line in the test to `monkeypatch.setattr("sonari.daemon.HOTKEYD_BIN_PATH", ..., raising=False)`.)

(b) Add the per-session guard set in `__init__`, immediately after `self._warned_immediate` (line 35):

```python
        self._warned_immediate: set[str] = set()
        self._guided_sessions: set[str] = set()
```

(c) Add the health helpers as methods immediately after `_choice_notes` (after line 119):

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
        "ok"            -> fully installed, no version drift   -> cue None
        "not_installed" -> no install.json or launcher (never ran `sonari install`)
        "version_drift" -> installed but plugin_version differs from this session's

        Cheap: a few file stats + a string compare. No launchctl. Never raises.
        The hotkeyd binary is deliberately NOT part of this check so a deliberate
        speech-only user (no swiftc) is never nagged.
        """
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

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_daemon_setup_health.py -v`
Expected: PASS (7 tests). The speech-only test passes because `_setup_health` never reads `HOTKEYD_BIN_PATH`.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_setup_health.py
git commit -m "feat(daemon): setup-health helpers (install record + launcher + version drift)"
```

### Step group B — emit the cue on `SESSION_START`, throttled, cleared on `SESSION_END`

- [ ] **Step 6: Write the failing tests (throttle + wiring)**

Append to `tests/test_daemon_setup_health.py`:

```python
from sonari.protocol import MsgType, PROTOCOL_VERSION


def _ss(session, plugin_version=""):
    return {"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START,
            "session": session, "plugin_version": plugin_version}


def _se(session):
    return {"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END, "session": session}


def test_session_start_enqueues_one_cue_when_not_installed(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    monkeypatch.setattr(daemon, "_setup_health",
                        lambda v: ("not_installed", "RUN slash sonari install"))
    daemon.handle_message(_ss("s1"))
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "prose"
    assert "slash sonari install" in item.text.lower()


def test_session_start_silent_when_ok(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    monkeypatch.setattr(daemon, "_setup_health", lambda v: ("ok", None))
    daemon.handle_message(_ss("s1"))
    assert len(queue) == 0


def test_session_start_cue_throttled_per_session(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    monkeypatch.setattr(daemon, "_setup_health",
                        lambda v: ("not_installed", "RUN slash sonari install"))
    daemon.handle_message(_ss("s1"))
    daemon.handle_message(_ss("s1"))  # same session again
    assert len(queue) == 1  # only ONE cue


def test_session_end_clears_throttle_so_cue_can_fire_again(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    monkeypatch.setattr(daemon, "_setup_health",
                        lambda v: ("not_installed", "RUN slash sonari install"))
    daemon.handle_message(_ss("s1"))
    assert len(queue) == 1
    queue.pop_next()
    daemon.handle_message(_se("s1"))
    daemon.handle_message(_ss("s1"))  # new session lifecycle, same id
    assert len(queue) == 1


def test_setup_health_exception_never_breaks_session(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    def _boom(v):
        raise RuntimeError("health blew up")
    monkeypatch.setattr(daemon, "_setup_health", _boom)
    # Must not raise; just no cue.
    daemon.handle_message(_ss("s1"))
    assert len(queue) == 0
```

- [ ] **Step 7: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_daemon_setup_health.py -k "session_start or session_end or never_breaks" -v`
Expected: FAIL — `SESSION_START` does not yet call any guidance emitter, so `len(queue) == 1` assertions fail (queue stays empty).

- [ ] **Step 8: Write the implementation — emitter + wiring**

(a) Add the throttled emitter immediately after `_enqueue` (after line 57):

```python
    def _maybe_guide_setup(self, session: str, plugin_version: str) -> None:
        """Speak ONE setup-guidance cue for this session, only when degraded.

        Throttle: at most once per session (recorded whether or not a cue fires).
        Silent when healthy. The check is a few file stats + a version compare
        (no launchctl) and never raises.
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

(b) Replace the `SET_FOREGROUND`/`SESSION_START` branch (lines 191-195) with:

```python
        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            self.sessions.set_foreground(session)
            if t == MsgType.SESSION_START:
                self.sessions.register(session)
                self._maybe_guide_setup(session, msg.get("plugin_version", ""))
            return None
```

(c) Replace the `SESSION_END` branch (lines 197-201) with:

```python
        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._last_options = None
            self._warned_immediate.discard(session)
            self._guided_sessions.discard(session)
            return None
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_daemon_setup_health.py -v`
Expected: PASS (all 12 tests)

- [ ] **Step 10: Fix the existing `SESSION_START`-feeding tests (no red window)**

(i) In `tests/test_daemon_control.py`, update `test_session_start_sets_foreground_and_registers` (79-84) to arrange health "ok":

```python
def test_session_start_sets_foreground_and_registers():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon._setup_health = lambda v: ("ok", None)  # keep focus on fg/register
    daemon.handle_message(_msg(MsgType.SESSION_START, "s9"))
    assert sessions.foreground() == "s9"
    assert sessions.is_foreground("s9") is True
```

(ii) In `tests/test_e2e_pipeline.py`, in `make_daemon()` (66-74), force health "ok" on the built daemon so the hermetic `not_installed` cue never fires and the EXACT log ordering assertions stay valid. Add one line before `return`:

```python
    daemon = SpeechDaemon(queue, speaker, sessions, cfg)
    daemon._setup_health = lambda v: ("ok", None)  # no setup cue in ordering tests
    return daemon, speaker, log
```

- [ ] **Step 11: Run the affected suites to verify green**

Run: `.venv/bin/python -m pytest tests/test_daemon_control.py tests/test_e2e_pipeline.py tests/test_daemon_phase2.py tests/test_daemon_decisions.py -v`
Expected: PASS (all). The phase2/decisions suites are unchanged and still pass (they never feed `SESSION_START`).

- [ ] **Step 12: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_setup_health.py tests/test_daemon_control.py tests/test_e2e_pipeline.py
git commit -m "feat(daemon): speak one throttled setup-guidance cue on SESSION_START when degraded"
```

---

## Task 7: `daemon.py` — single-instance guard in `main()`

If a daemon is already accepting connections, `main()` exits cleanly instead of unlinking + rebinding the live socket. The guard lives in `main()` (not `run()`) so the in-process test harness that calls `run()` on an isolated socket is unaffected.

**Files:**
- Modify: `src/sonari/daemon.py` — `main()` (416-430)
- Test: `tests/test_daemon_single_instance.py` (new)

- [ ] **Step 1: Write the failing tests (new file)**

Create `tests/test_daemon_single_instance.py`:

```python
from unittest import mock

import sonari.daemon as daemon_mod


def test_main_exits_without_building_when_socket_connectable():
    with mock.patch("sonari.daemon.socket_connectable", return_value=True), \
         mock.patch.object(daemon_mod.SpeechDaemon, "run") as run, \
         mock.patch("sonari.daemon.load_config", return_value={}):
        daemon_mod.main()
    run.assert_not_called()


def test_main_builds_and_runs_when_socket_not_connectable():
    with mock.patch("sonari.daemon.socket_connectable", return_value=False), \
         mock.patch.object(daemon_mod.SpeechDaemon, "run") as run, \
         mock.patch("sonari.daemon.load_config", return_value={}), \
         mock.patch("sonari.speaker.Speaker"):
        daemon_mod.main()
    run.assert_called_once()
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_daemon_single_instance.py -v`
Expected: FAIL — `test_main_exits_without_building_when_socket_connectable` fails because `main()` builds + runs unconditionally (`run.assert_not_called()` fails).

- [ ] **Step 3: Write the implementation**

In `src/sonari/daemon.py`, replace the top of `main()` (line 416-419) so the guard runs first (`socket_connectable` is already imported at line 12):

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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_daemon_single_instance.py -v`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_single_instance.py
git commit -m "feat(daemon): single-instance guard in main() (exit if socket connectable)"
```

---

## Task 8: `bin/sonari-daemon` + `bin/sonari` — prefer `/usr/bin/python3` first

Make the shims prefer `/usr/bin/python3` (stable, matches `_resolve_python`'s preference) and fall back to the first `python3` on PATH. The `bin/sonari-hook` shim is NOT changed.

**Files:**
- Modify: `bin/sonari-daemon` (lines 6-7), `bin/sonari` (lines 6-7)
- Test: `tests/test_bin_shims.py` (new)

- [ ] **Step 1: Write the failing tests (new file)**

Create `tests/test_bin_shims.py`:

```python
import os
import subprocess

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(REPO, "bin")


def _read(name):
    with open(os.path.join(BIN, name), encoding="utf-8") as f:
        return f.read()


def test_sonari_daemon_prefers_usr_bin_python3_first():
    txt = _read("sonari-daemon")
    # The /usr/bin/python3 preference must appear BEFORE any `command -v python3`.
    pref = txt.index("[ -x /usr/bin/python3 ]")
    cmdv = txt.index("command -v python3")
    assert pref < cmdv, "shim must prefer /usr/bin/python3 before PATH lookup"


def test_sonari_prefers_usr_bin_python3_first():
    txt = _read("sonari")
    pref = txt.index("[ -x /usr/bin/python3 ]")
    cmdv = txt.index("command -v python3")
    assert pref < cmdv, "shim must prefer /usr/bin/python3 before PATH lookup"


def test_sonari_daemon_picks_usr_bin_python3_even_when_stub_python3_on_path(tmp_path):
    # A fake `python3` earlier on PATH writes a marker; the shim must NOT pick it
    # because /usr/bin/python3 exists and is preferred. We verify by capturing
    # which interpreter the shim selects via a one-shot `--version`-style probe.
    stub_dir = tmp_path / "stub"
    stub_dir.mkdir()
    marker = tmp_path / "stub-was-used"
    stub = stub_dir / "python3"
    stub.write_text(
        "#!/bin/sh\n"
        f'echo used > "{marker}"\n'
        'exit 0\n'
    )
    stub.chmod(0o755)
    env = dict(os.environ)
    env["PATH"] = f"{stub_dir}:{env.get('PATH','')}"
    # Run the shim with a no-op subcommand path: `-m sonari.cli --help`-style.
    # We pass `status` which exits quickly; the daemon shim runs sonari.daemon,
    # so use the CLI shim for a deterministic quick exit.
    subprocess.run([os.path.join(BIN, "sonari"), "--help"], env=env,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    # If /usr/bin/python3 was preferred, the stub marker is NEVER written.
    assert not marker.exists(), "shim used the PATH stub instead of /usr/bin/python3"
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_bin_shims.py -v`
Expected: FAIL — the source-grep tests fail with `ValueError: substring not found` (`[ -x /usr/bin/python3 ]` is absent); the stub test may fail because the current shim picks the PATH `python3` first and writes the marker.

- [ ] **Step 3: Write the implementation**

In `bin/sonari-daemon`, replace lines 6-7:

```bash
py="$(command -v python3 || true)"
[ -x "$py" ] || py="/usr/bin/python3"
```

with:

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

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_bin_shims.py -v`
Expected: PASS (3 tests). On a Mac `/usr/bin/python3` exists, so the stub is never used and `sonari --help` exits 0.

- [ ] **Step 5: Commit**

```bash
git add bin/sonari-daemon bin/sonari tests/test_bin_shims.py
git commit -m "fix(shims): prefer /usr/bin/python3 first (match installed daemon interpreter)"
```

---

## Task 9: `commands/sonari:install.md` + `sonari:uninstall.md` slash commands

Two thin command files mirroring `commands/sonari:doctor.md`. They instruct Claude to run `sonari install` / `sonari uninstall` via the Bash tool and print the output verbatim.

**Files:**
- Create: `commands/sonari:install.md`, `commands/sonari:uninstall.md`
- Test: `tests/test_commands.py`

- [ ] **Step 1: Write the failing tests**

In `tests/test_commands.py`, extend `test_all_command_files_exist` (12-16) to include the two new files:

```python
def test_all_command_files_exist():
    for name in ("sonari:status.md", "sonari:verbosity.md", "sonari:stop.md",
                 "sonari:repeat.md", "sonari:doctor.md", "sonari:keymap.md",
                 "sonari:voice.md", "sonari:rate.md", "sonari:skip.md",
                 "sonari:install.md", "sonari:uninstall.md"):
        assert os.path.exists(os.path.join(CMD, name)), name
```

And append two new test cases:

```python
def test_install_command_runs_sonari_install():
    txt = _read("sonari:install.md")
    assert "sonari install" in txt
    assert "Bash" in txt
    assert "print" in txt.lower()
    assert txt.lstrip().startswith("---")  # has front-matter
    assert "description:" in txt


def test_uninstall_command_runs_sonari_uninstall():
    txt = _read("sonari:uninstall.md")
    assert "sonari uninstall" in txt
    assert "Bash" in txt
    assert "print" in txt.lower()
    assert txt.lstrip().startswith("---")
    assert "description:" in txt
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_commands.py -v`
Expected: FAIL — `test_all_command_files_exist` fails (`sonari:install.md` missing) and the two new tests fail with `FileNotFoundError`.

- [ ] **Step 3: Create the command files**

Create `commands/sonari:install.md` with exactly this content:

````markdown
---
description: Install Sonari (autostart, global hotkeys, control CLI) — one-time setup
---

Run the Sonari installer using the Bash tool:

```
sonari install
```

Print the command's output to the user verbatim so they can hear each step and any
remediation (for example, installing Xcode Command Line Tools for the hotkeys). Do not add
commentary beyond the raw output.
````

Create `commands/sonari:uninstall.md` with exactly this content:

````markdown
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
````

(When creating these files, write the body WITHOUT the outer four-backtick fence shown above — that fence is only this plan's way of displaying a markdown file that itself contains triple-backtick blocks. The file's first line must be `---`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_commands.py -v`
Expected: PASS (all command tests including the two new ones)

- [ ] **Step 5: Commit**

```bash
git add commands/sonari:install.md commands/sonari:uninstall.md tests/test_commands.py
git commit -m "feat(commands): add /sonari:install and /sonari:uninstall slash commands"
```

---

## Task 10: Docs + version bump 0.4.0 + final dual-interpreter gate

Bump the version in all three manifests, update `test_manifests.py` assertions (and add a marketplace.json version assertion), update the README install/uninstall/slash-command sections, add a manual clean-room verification checklist doc, then run the full suite green under BOTH interpreters.

**Files:**
- Modify: `.claude-plugin/plugin.json:3`, `pyproject.toml:7`, `.claude-plugin/marketplace.json:12`
- Modify: `tests/test_manifests.py` (87-94, + new marketplace assertion)
- Modify: `README.md` (Install §49-70, Slash commands table §143-153, Uninstall §204-215)
- Create: `docs/superpowers/verification/2026-06-06-sonari-phase3.1-cleanroom-checklist.md`
- Test: `tests/test_manifests.py`

### Step group A — version bump (TDD via manifest tests)

- [ ] **Step 1: Update the failing manifest tests**

In `tests/test_manifests.py`, replace `test_plugin_json_version_is_0_3_0` and `test_pyproject_version_is_0_3_0` (87-94) with:

```python
def test_plugin_json_version_is_0_4_0():
    data = _load(PLUGIN_JSON)
    assert data.get("version") == "0.4.0"


def test_pyproject_version_is_0_4_0():
    text = (REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8")
    assert 'version = "0.4.0"' in text


def test_marketplace_plugin_version_is_0_4_0():
    mp = REPO_ROOT / ".claude-plugin" / "marketplace.json"
    data = _load(mp)
    plugins = data.get("plugins") or []
    assert plugins, "marketplace.json declares no plugins"
    assert plugins[0].get("version") == "0.4.0"
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `.venv/bin/python -m pytest tests/test_manifests.py -k "0_4_0 or marketplace" -v`
Expected: FAIL — all three assert `0.4.0` but the manifests still say `0.3.0`.

- [ ] **Step 3: Bump the versions**

In `.claude-plugin/plugin.json`, change `"version": "0.3.0"` → `"version": "0.4.0"` (line 3).

In `pyproject.toml`, change `version = "0.3.0"` → `version = "0.4.0"` (line 7).

In `.claude-plugin/marketplace.json`, change the plugin entry `"version": "0.3.0"` → `"version": "0.4.0"` (line 12, inside `plugins[0]`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `.venv/bin/python -m pytest tests/test_manifests.py -v`
Expected: PASS (all manifest tests)

- [ ] **Step 5: Commit**

```bash
git add .claude-plugin/plugin.json pyproject.toml .claude-plugin/marketplace.json tests/test_manifests.py
git commit -m "chore: bump version to 0.4.0 across all three manifests"
```

### Step group B — README + verification checklist doc

- [ ] **Step 6: Update the README Install section**

In `README.md`, rewrite the Install section (§49-70) to lead with the slash-command flow (no `sonari` on PATH required), keep the CLI form as the equivalent, and add the durability note. Replace the Install section body with:

```markdown
## Install

Enable the plugin, then finish setup entirely from inside Claude Code — no
`sonari` on your PATH required:

1. Enable the **Sonari** plugin (via `/plugin`, or per session
   `claude --plugin-dir <plugin-root>`). You will start hearing Claude
   immediately; the daemon lazy-starts on the first hook.
2. Run `/sonari:install` from inside Claude Code. It runs `sonari install` via
   the plugin's `bin/`, so it works before the `~/.local/bin/sonari` launcher
   exists. Each step is printed (and spoken) so you can follow along eyes-free.
3. Run `/sonari:doctor` to confirm everything is green (or to hear the only
   expected failure — `swiftc`/Command Line Tools — on a machine without them).

If you already have `sonari` on your PATH, the CLI equivalent is:

```
sonari install
```

`sonari install` resolves the best `python3 >= 3.9`, **copies the runtime to
`~/.sonari/app`** (so it survives plugin auto-updates), builds the hotkey
daemon, writes both LaunchAgents, and places the `~/.local/bin/sonari` launcher.
After a plugin update, Sonari will say **"run /sonari:install"** once so you can
re-point the daemon at the refreshed copy.
```

(Keep any surrounding headings/anchors the existing README uses; if the existing
section already contains the `sonari install` fenced block at §67, replace the
whole section between the `## Install` heading and the next `##` heading with the
text above.)

- [ ] **Step 7: Update the README Slash-commands table + Uninstall section**

In `README.md`, in the slash-commands table (§145-153), add two rows directly after the header/first rows:

```markdown
| `/sonari:install` | `sonari install` | One-time setup: autostart, global hotkeys, control CLI (copies runtime to `~/.sonari/app`) |
| `/sonari:uninstall` | `sonari uninstall` | Remove LaunchAgents, hotkey helper, launcher, and `~/.sonari/app` (keeps your settings) |
```

In the Uninstall section (§204-215), add the in-session equivalent + the app-copy note. After the existing `sonari uninstall` description, append:

```markdown
The in-session equivalent is `/sonari:uninstall`. Uninstall also removes the
stable app copy at `~/.sonari/app`, and **preserves** your `config.json` and
`keymap.json`.
```

- [ ] **Step 8: Create the clean-room verification checklist doc**

Create `docs/superpowers/verification/2026-06-06-sonari-phase3.1-cleanroom-checklist.md`:

```markdown
# Sonari Phase 3.1 — Manual Clean-Room Verification Checklist

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
```

- [ ] **Step 9: Run the manifest + commands tests to confirm docs/version are consistent**

Run: `.venv/bin/python -m pytest tests/test_manifests.py tests/test_commands.py -v`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add README.md docs/superpowers/verification/2026-06-06-sonari-phase3.1-cleanroom-checklist.md
git commit -m "docs: slash-command install flow, durability note, clean-room checklist"
```

### Step group C — final dual-interpreter gate

- [ ] **Step 11: Full suite under Python 3.13 (zero warnings)**

Run: `.venv/bin/python -m pytest -W error -q`
Expected: PASS, 0 failures, 0 warnings (the `-W error` flag turns any warning into a failure so the "0 warnings" requirement is enforced).

- [ ] **Step 12: Full suite under Python 3.9.6 (zero warnings)**

Run: `.venv39/bin/python -m pytest -W error -q`
Expected: PASS, 0 failures, 0 warnings.

- [ ] **Step 13: If both pass, the work is verified-complete. Final commit (if any uncommitted touch-ups remain)**

```bash
git add -A
git commit -m "test: full suite green under .venv (3.13) and .venv39 (3.9), 0 warnings"
```

(If there is nothing to commit, skip this step — every prior task already committed its own changes.)

---

## Self-Review

### Spec coverage map (every spec section → task)

| Spec section | Requirement | Task(s) |
|---|---|---|
| §3.A | `commands/sonari:install.md` + `sonari:uninstall.md` on-ramp | 9 |
| §3.B.1 | `paths.APP_DIR` constant | 1 |
| §3.B.2 | `_copy_app` + `install()` copies + plist PYTHONPATH → `APP_DIR` | 3 |
| §3.B.3 | `_read_plugin_version`; `_write_install_record` (`app_path`+`plugin_version`); `doctor()` reads `app_path` | 2 (`_read_plugin_version`), 3 (record + doctor) |
| §3.B.4 | `uninstall()` removes `APP_DIR`, preserves config/keymap | 4 |
| §3.C.1 | `hooks_entry` carries `plugin_version`+`plugin_root`; `bin/sonari-hook` resolves version | 5 |
| §3.C.2 | `_read_install_record`, `_launcher_present`, `_setup_health` (hotkeyd excluded) | 6 (group A) |
| §3.C.3 | `_maybe_guide_setup`, wire `SESSION_START`, clear on `SESSION_END`, throttle | 6 (group B) |
| §3.D.1 | single-instance guard in `main()` | 7 |
| §3.D.2 | shims prefer `/usr/bin/python3` (sonari-hook unchanged) | 8 |
| §3.D.3 | launcher defensive verification in `install()` | **3 (covered below)** — see note |
| §4 | README slash-flow + durability note + uninstall note + table; version 0.4.0 (3 manifests) | 10 |
| §5 | copy-failure fatal-with-guidance; empty-version no drift cue; no launchctl on hot path; SET_FOREGROUND-only no guidance | 3 (copy-fail test), 6 (empty-version + SET_FOREGROUND-only behavior) |
| §6 | single-instance is best-effort, no lock file | 7 (guard only; no lock file added) |
| §7 test 1 | plist re-point + ProgramArguments | 3 (Step 7) |
| §7 test 2 | install.json new fields | 3 (Steps 6,7) |
| §7 test 3 | idempotent re-copy (stale module gone) | 3 (`test_copy_app_is_remove_then_copy...`) |
| §7 test 4 | uninstall removes APP_DIR, preserves config/keymap | 4 |
| §7 test 5 | session-start detect-and-guide (not_installed / launcher-missing / speech-only-OK / healthy) | 6 (group A) |
| §7 test 6 | version-drift cue + empty-version no cue | 6 (group A) |
| §7 test 7 | throttle + session_end clears | 6 (group B) |
| §7 test 8 | single-instance guard build/no-build | 7 |
| §7 test 9 | interpreter preference (grep ordering + subprocess smoke) | 8 |
| §7 test 10 | command files exist + run right command | 9 |
| §7 test 11 | hooks_entry SessionStart fields (set/unset) | 5 |
| §7 test 12 | `_read_plugin_version` (read / missing / corrupt / env fallback) | 2 |
| §7 test 13 | APP_DIR copy failure path | 3 (Step 8) |
| §7 test 14 | spaces in `$HOME`/`APP_DIR` plist round-trip | 3 (Step 8) |
| §9 | manual clean-room verification checklist doc | 10 (Step 8) |

**Note on §3.D.3 (launcher defensive verification):** The spec adds a defensive
`os.path.exists(launcher) and os.access(launcher, os.X_OK)` check after
`_place_launcher`. This was NOT broken out as its own task above to avoid a red
window in `test_install_writes_plist_and_loads` (which mocks `_place_launcher` to
return a path that does not exist on disk, so an `os.access` check would print the
warning branch). **Add it as Step 10(e) of Task 3**, and in the same step update
`test_install_writes_plist_and_loads` to create the launcher file so the success
branch is exercised:

```python
def install(...):  # in Task 3 Step 10, replace the launcher block (cli.py:594-596):
    # 8. ~/.local/bin/sonari launcher (verify it landed; non-fatal warn if not).
    launcher = _place_launcher(plugin_root)
    if os.path.exists(launcher) and os.access(launcher, os.X_OK):
        print(f"Placed launcher: {launcher}")
    else:
        print(f"warning: could not place launcher at {launcher}; "
              f"run `sonari` from the plugin's bin/ until this is fixed.")
```

And in Task 3 Step 7's `test_install_writes_plist_and_loads`, change the
`_place_launcher` mock so the returned path actually exists + is executable:

```python
    launcher_path = tmp_path / "launcher"
    launcher_path.write_text("#!/bin/sh\n")
    launcher_path.chmod(0o755)
    # ... then in the with-block:
    mock.patch.object(cli, "_place_launcher", return_value=str(launcher_path)) as place_launcher, \
```

(The other install tests — `test_install_writes_hotkeyd_plist_and_keymap`,
`test_install_build_failure_is_nonfatal`, and the spaces test — only assert `rc`
and the plist, not launcher output, so they tolerate the warning branch. The
copy-failure test returns before reaching the launcher, so it is unaffected.)

### Placeholder scan

No `TODO`/`TBD`/`???`/"implement later"/"add error handling" remain. Every code
step shows complete, runnable code. Every `<…>` is a named runtime value
(plugin root, resolved interpreter, version string) defined in its context.

### Type / name consistency check

- `paths.APP_DIR` — defined in Task 1; used identically in Task 3 (`_copy_app`,
  plist `src_path=app_dir`), Task 3 (`install.json` `app_path`), Task 3 (doctor
  `app`), Task 4 (uninstall `paths.APP_DIR`), Task 6 (tests). ✓
- `_copy_app(plugin_root: str) -> str` — defined Task 3; called Task 3 `install()`. ✓
- `_read_plugin_version(plugin_root: str) -> str` — defined Task 2; called Task 3
  `install()`. ✓
- `_write_install_record(python, python_version, plugin_root, app_path,
  plugin_version)` — signature defined Task 3; caller updated Task 3; test updated
  Task 3 Step 6 (`src` key removed, `app_path`+`plugin_version` added). ✓ No
  caller still passes `src=`. 
- install record key `app_path` (NOT `src`) — written Task 3, read by `doctor()`
  Task 3, asserted Task 3 tests + Task 6 daemon (`rec.get("plugin_version")`). ✓
- `SpeechDaemon._read_install_record` / `_launcher_present` / `_setup_health` /
  `_maybe_guide_setup` / `_guided_sessions` — all defined Task 6, names used
  identically in wiring + tests + the Task 6 fixes to `test_daemon_control.py` /
  `test_e2e_pipeline.py`. ✓
- `SESSION_START` message field `plugin_version` — produced by `hooks_entry`
  (Task 5), read by `handle_message` via `msg.get("plugin_version", "")` (Task 6),
  passed to `_setup_health` (Task 6). Field name matches end-to-end. ✓
- daemon imports `INSTALL_RECORD_PATH` (Task 6); does NOT import `HOTKEYD_BIN_PATH`
  (hotkeyd excluded from the cue check) — consistent with §3.C.2 and the
  speech-only-OK test. ✓
- Shim ordering token `[ -x /usr/bin/python3 ]` — written in Task 8 to both
  `bin/sonari-daemon` and `bin/sonari`; asserted by the Task 8 grep tests. ✓
- Version string `0.4.0` — three manifests (Task 10) + asserted in
  `test_manifests.py` (Task 10). ✓
- `PROTOCOL_VERSION` stays `1` (no task changes it; protocol tests unmodified). ✓

### Conftest interaction note (for the implementer)

`tests/conftest.py`'s autouse `_isolate_sonari_dir` repoints `paths.*` (including,
after Task 1, you should NOT need to add `APP_DIR` there for cli tests because
they patch `cli.paths.APP_DIR` explicitly). The daemon health tests in Task 6
patch `sonari.daemon.INSTALL_RECORD_PATH` directly (the module bound it by value
at import via the `from ... import` in Task 6's import edit), which is why those
tests patch the daemon module attribute, not `paths.INSTALL_RECORD_PATH`. If you
prefer, you may also add `monkeypatch.setattr(paths, "APP_DIR", sonari_dir /
"app", raising=False)` to conftest for belt-and-suspenders isolation; it is not
required by any task here.
