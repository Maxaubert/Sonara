"""install() / uninstall() / copy_app failure paths (#139: H3, E6, E8, E19, #127).

An install that fails after stop_sonara() used to leave the STOPPED sentinel
behind, so Sonara stayed off with no cue. From the deployed copy it always
failed that way, because repo_root() resolves to ~/.sonara there.
"""
import json
import os

import pytest

from sonara import cli, paths
from sonara.install import app_copy
from sonara.install import deps
from sonara import install_record
from sonara.install import installer
from sonara.install import service
from sonara import kokoro_provision as kp
from tests._fakeplatform import fake_platform, FakeSupervisor, FakeHotkey, FakeTts


def _make_plugin(root):
    """A minimal plugin tree: what install() needs to find at the plugin root."""
    (root / "src" / "sonara").mkdir(parents=True)
    (root / "src" / "sonara" / "__init__.py").write_text("", encoding="utf-8")
    (root / "bin").mkdir()
    (root / "bin" / "sonara-hook").write_text("#!/usr/bin/env python3\n", encoding="utf-8")
    (root / "hooks").mkdir()
    (root / "hooks" / "hooks.json").write_text('{"hooks": {}}', encoding="utf-8")
    return root


@pytest.fixture
def env(tmp_path, monkeypatch):
    """install() with every OS step faked; records stop/copy calls."""
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    monkeypatch.setattr(deps, "winrt_importable", lambda python: True)
    monkeypatch.delenv("CLAUDE_PLUGIN_ROOT", raising=False)
    calls = []
    sup = FakeSupervisor(python="/PY/pythonw.exe")
    pb = fake_platform(supervisor=sup, hotkey=FakeHotkey(), tts=FakeTts("Aria"))
    monkeypatch.setattr("sonara.platform.get_platform", lambda: pb)

    def fake_stop(s=None):
        calls.append(("stop",))
        paths.ensure_sonara_dir()
        paths.STOPPED_SENTINEL_PATH.write_text("sonara shutdown")
        return True

    monkeypatch.setattr(service, "stop_sonara", fake_stop)
    monkeypatch.setattr(app_copy, "copy_app",
                        lambda root: calls.append(("copy", root)) or str(tmp_path / "app"))
    monkeypatch.setattr(install_record, "write", lambda **k: None)
    monkeypatch.setattr("sonara.keymap.migrate_default_chord", lambda: None)
    monkeypatch.setattr("sonara.keymap.write_default_keymap_if_absent", lambda: None)
    return {"calls": calls, "sup": sup, "tmp": tmp_path}


# --- H3/E6: the deployed copy -------------------------------------------------

def test_install_from_deployed_copy_refuses_before_stopping(env, monkeypatch, capsys):
    deployed = env["tmp"] / "deployed-sonara-dir"      # ~/.sonara: no src/, no bin/
    deployed.mkdir()
    monkeypatch.setattr(paths, "repo_root", lambda: str(deployed))
    rc = installer.install()
    assert rc == 1
    assert ("stop",) not in env["calls"]                # nothing was stopped
    assert not paths.STOPPED_SENTINEL_PATH.exists()
    out = capsys.readouterr().out
    assert "plugin" in out.lower() and "/sonara:install" in out


def test_install_from_deployed_copy_uses_claude_plugin_root(env, monkeypatch):
    deployed = env["tmp"] / "deployed"
    deployed.mkdir()
    plugin = _make_plugin(env["tmp"] / "plugin")
    monkeypatch.setattr(paths, "repo_root", lambda: str(deployed))
    monkeypatch.setenv("CLAUDE_PLUGIN_ROOT", str(plugin))
    assert installer.install() == 0
    assert ("copy", os.path.realpath(str(plugin))) in env["calls"]


def test_install_from_deployed_copy_uses_install_record(env, monkeypatch):
    deployed = env["tmp"] / "deployed"
    deployed.mkdir()
    plugin = _make_plugin(env["tmp"] / "plugin")
    monkeypatch.setattr(paths, "repo_root", lambda: str(deployed))
    paths.ensure_sonara_dir()
    paths.INSTALL_RECORD_PATH.write_text(
        json.dumps({"plugin_root": str(plugin)}), encoding="utf-8")
    assert installer.install() == 0
    assert ("copy", os.path.realpath(str(plugin))) in env["calls"]


def test_install_passes_the_resolved_plugin_root_to_the_backend(env, monkeypatch):
    # The settings.json hooks point at <plugin>/bin/sonara-hook; from the
    # deployed copy repo_root() would bake ~/.sonara/bin/sonara-hook instead.
    deployed = env["tmp"] / "deployed"
    deployed.mkdir()
    plugin = _make_plugin(env["tmp"] / "plugin")
    monkeypatch.setattr(paths, "repo_root", lambda: str(deployed))
    monkeypatch.setenv("CLAUDE_PLUGIN_ROOT", str(plugin))
    assert installer.install() == 0
    assert env["sup"].plugin_roots == [os.path.realpath(str(plugin))]


# --- E6: no failure path leaves the sentinel ----------------------------------

def test_install_copy_failure_clears_the_sentinel(env, monkeypatch, capsys):
    def boom(root):
        raise OSError("access denied")
    monkeypatch.setattr(app_copy, "copy_app", boom)
    assert installer.install() == 1
    assert not paths.STOPPED_SENTINEL_PATH.exists()
    assert "access denied" in capsys.readouterr().out


def test_install_backend_failure_clears_the_sentinel(env, monkeypatch, capsys):
    # sup.install raises ValueError on an unparseable ~/.claude/settings.json.
    def bad_install(py, app, plugin_root=None):
        raise ValueError("settings.json is not valid JSON")
    monkeypatch.setattr(env["sup"], "install", bad_install)
    assert installer.install() == 1                       # no traceback
    assert not paths.STOPPED_SENTINEL_PATH.exists()
    out = capsys.readouterr().out
    assert "settings.json is not valid JSON" in out


def test_install_keymap_failure_clears_the_sentinel(env, monkeypatch):
    def boom():
        raise ValueError("bad keymap.json")
    monkeypatch.setattr("sonara.keymap.migrate_default_chord", boom)
    assert installer.install() == 1
    assert not paths.STOPPED_SENTINEL_PATH.exists()


# --- #127: the rename swap ------------------------------------------------------

def _app_with_live(tmp_path, monkeypatch, content="OLD"):
    app = tmp_path / "app"
    (app / "sonara").mkdir(parents=True)
    (app / "sonara" / "daemon.py").write_text(content)
    monkeypatch.setattr(paths, "APP_DIR", app)
    plugin = tmp_path / "plugin"
    (plugin / "src" / "sonara").mkdir(parents=True)
    (plugin / "src" / "sonara" / "daemon.py").write_text("NEW")
    monkeypatch.setattr(app_copy.time, "sleep", lambda s: None)
    return app, plugin


def test_copy_app_retries_a_transient_rename_denial(tmp_path, monkeypatch):
    app, plugin = _app_with_live(tmp_path, monkeypatch)
    real_rename = os.rename
    denied = {"n": 0}

    def flaky(src, dst):
        if src.endswith("sonara.new") and denied["n"] < 2:
            denied["n"] += 1
            raise PermissionError(5, "Access is denied")
        return real_rename(src, dst)

    monkeypatch.setattr(app_copy.os, "rename", flaky)
    app_copy.copy_app(str(plugin))
    assert (app / "sonara" / "daemon.py").read_text() == "NEW"
    assert denied["n"] == 2
    assert not (app / "sonara.new").exists() and not (app / "sonara.old").exists()


def test_copy_app_rolls_back_when_the_new_tree_cannot_be_renamed_in(tmp_path, monkeypatch):
    app, plugin = _app_with_live(tmp_path, monkeypatch)
    real_rename = os.rename

    def stuck(src, dst):
        if src.endswith("sonara.new"):
            raise PermissionError(5, "Access is denied")
        return real_rename(src, dst)

    monkeypatch.setattr(app_copy.os, "rename", stuck)
    with pytest.raises(OSError):
        app_copy.copy_app(str(plugin))
    # A live package always exists: the old one was renamed back.
    assert (app / "sonara" / "daemon.py").read_text() == "OLD"


# --- E8 / E19: uninstall --------------------------------------------------------

def _uninstall(monkeypatch, sup=None):
    sup = sup or FakeSupervisor()
    monkeypatch.setattr("sonara.platform.get_platform",
                        lambda: fake_platform(supervisor=sup, hotkey=FakeHotkey()))
    monkeypatch.setattr(service, "stop_sonara", lambda s=None: True)
    return installer.uninstall()


def test_uninstall_leaves_sonara_stopped_so_the_next_hook_cannot_restart_it(
        monkeypatch, capsys):
    paths.ensure_sonara_dir()
    assert _uninstall(monkeypatch) == 0
    # ensure_running() honours the sentinel: a hook from the still-enabled
    # plugin must not spawn a daemon right after an uninstall.
    assert paths.STOPPED_SENTINEL_PATH.exists()
    out = capsys.readouterr().out
    assert "/plugin" in out


def test_uninstall_without_an_existing_sonara_dir_still_writes_the_sentinel(monkeypatch):
    assert _uninstall(monkeypatch) == 0
    assert paths.STOPPED_SENTINEL_PATH.exists()


def test_uninstall_mentions_cleanup_for_leftovers(monkeypatch, capsys):
    (paths.CHATTERBOX_VENV / "Lib").mkdir(parents=True)
    (paths.CHATTERBOX_VENV / "Lib" / "big.bin").write_bytes(b"x" * 10)
    assert _uninstall(monkeypatch) == 0
    assert "sonara cleanup" in capsys.readouterr().out


def test_install_clears_the_uninstall_sentinel(env):
    paths.ensure_sonara_dir()
    paths.STOPPED_SENTINEL_PATH.write_text("uninstalled")
    assert installer.install() == 0
    assert not paths.STOPPED_SENTINEL_PATH.exists()


def test_voices_install_gives_predownload_the_running_package(monkeypatch, tmp_path):
    # E6: from the deployed copy repo_root()/src is the nonexistent
    # ~/.sonara/src, so the model predownload could never import sonara.
    monkeypatch.setattr(paths, "repo_root", lambda: str(tmp_path / "deployed"))
    monkeypatch.setattr(app_copy, "resolve_plugin_root", lambda: str(tmp_path / "plugin"))
    seen = {}
    monkeypatch.setattr(kp, "install_kokoro", lambda pp: seen.update(pp=pp))
    monkeypatch.setattr(installer, "install", lambda: 1)
    cli._cmd_voices_install(None)
    assert seen["pp"] == paths.package_root()


def test_voices_uninstall_never_leaves_sonara_stopped_when_install_fails(monkeypatch):
    # E6: voices uninstall stops Sonara itself, then calls install(); an
    # install that returned early (no Python, no plugin tree) left the
    # sentinel from that stop behind.
    def fake_stop(sup=None):
        paths.ensure_sonara_dir()
        paths.STOPPED_SENTINEL_PATH.write_text("sonara shutdown")
        return True
    monkeypatch.setattr(service, "stop_sonara", fake_stop)
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: None)
    monkeypatch.setattr(installer, "install", lambda: 1)
    assert cli._cmd_voices_uninstall(None) == 1
    assert not paths.STOPPED_SENTINEL_PATH.exists()


def test_voices_install_refuses_before_the_download_without_a_plugin_tree(monkeypatch, capsys):
    # H3: install() refuses without a plugin tree; voices install used to
    # download ~316 MB first and only then get refused.
    monkeypatch.setattr(app_copy, "resolve_plugin_root", lambda: None)
    monkeypatch.setattr(kp, "install_kokoro", lambda pp: pytest.fail("must not download"))
    monkeypatch.setattr(installer, "install", lambda: pytest.fail("must not install"))
    assert cli._cmd_voices_install(None) == 1
    assert "Cannot find the Sonara plugin files" in capsys.readouterr().out


def test_voices_uninstall_refuses_before_deleting_the_venv_without_a_plugin_tree(monkeypatch, capsys):
    # Deleting the venv and then having install() refuse left the scheduled
    # task pointing at the deleted venv pythonw, so logon autostart failed.
    monkeypatch.setattr(app_copy, "resolve_plugin_root", lambda: None)
    monkeypatch.setattr(service, "stop_sonara", lambda sup=None: pytest.fail("must not stop"))
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: pytest.fail("must not delete the venv"))
    monkeypatch.setattr(installer, "install", lambda: pytest.fail("must not install"))
    assert cli._cmd_voices_uninstall(None) == 1
    out = capsys.readouterr().out
    assert "Cannot find the Sonara plugin files" in out
    assert "reverted to the system voice" not in out


def test_voices_uninstall_does_not_claim_a_revert_when_install_fails(monkeypatch, capsys):
    monkeypatch.setattr(service, "stop_sonara", lambda sup=None: True)
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: None)
    monkeypatch.setattr(installer, "install", lambda: 1)
    assert cli._cmd_voices_uninstall(None) == 1
    assert "reverted to the system voice" not in capsys.readouterr().out
