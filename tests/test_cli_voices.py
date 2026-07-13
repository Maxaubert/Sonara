import os

import pytest
from sonara import cli, paths
from sonara import kokoro_provision as kp
from sonara import chatterbox_provision as cbp


def test_voices_install_provisions_then_rewires_daemon(monkeypatch, tmp_path):
    order = []
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "repo_root", lambda: str(tmp_path))
    monkeypatch.setattr(kp, "install_kokoro", lambda pythonpath: order.append(("provision", pythonpath)))
    monkeypatch.setattr(kp, "neural_healthy", lambda app_dir: True)
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    rc = cli._cmd_voices_install(object())
    assert rc == 0
    assert order == [("provision", str(tmp_path / "src")), "install"]


def test_voices_install_passes_repo_src_not_app_dir_to_install_kokoro(monkeypatch, tmp_path):
    """install_kokoro must receive repo_root()/src so predownload can import
    sonara even before install() populates APP_DIR."""
    received = []
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "repo_root", lambda: str(tmp_path))
    monkeypatch.setattr(kp, "install_kokoro", lambda pythonpath: received.append(pythonpath))
    monkeypatch.setattr(kp, "neural_healthy", lambda app_dir: True)
    monkeypatch.setattr(cli, "install", lambda: 0)
    cli._cmd_voices_install(object())
    assert received == [os.path.join(str(tmp_path), "src")]


def test_voices_install_reports_failure_without_rewiring(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "repo_root", lambda: str(tmp_path))
    uninstall_called = []
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: uninstall_called.append(True))
    def boom(pythonpath): raise RuntimeError("uv missing")
    monkeypatch.setattr(kp, "install_kokoro", boom)
    monkeypatch.setattr(cli, "install", lambda: pytest.fail("must not rewire on failure"))
    rc = cli._cmd_voices_install(object())
    assert rc == 1
    assert uninstall_called, "uninstall_kokoro must be called on failure to revert half-built state"


def test_voices_install_reverts_on_keyboard_interrupt(monkeypatch, tmp_path):
    # Ctrl+C during the download must still revert the half-built venv, which
    # would otherwise read as fully provisioned forever (venv python exists after
    # step 1 of a multi-GB install) -- `except Exception` missed it (audit #21).
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "repo_root", lambda: str(tmp_path))
    uninstalled = []
    def boom(pythonpath):
        raise KeyboardInterrupt()
    monkeypatch.setattr(kp, "install_kokoro", boom)
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: uninstalled.append(True))
    monkeypatch.setattr(cli, "install", lambda: pytest.fail("must not rewire"))
    with pytest.raises(KeyboardInterrupt):               # interrupt still propagates
        cli._cmd_voices_install(object())
    assert uninstalled


def test_voices_install_chatterbox_reverts_on_keyboard_interrupt(monkeypatch):
    monkeypatch.setattr(paths, "ensure_sonara_dir", lambda: None)
    uninstalled = []
    def boom():
        raise KeyboardInterrupt()
    monkeypatch.setattr(cbp, "install_chatterbox", boom)
    monkeypatch.setattr(cbp, "uninstall_chatterbox", lambda: uninstalled.append(True))

    class Args:
        engine = "chatterbox"
    with pytest.raises(KeyboardInterrupt):
        cli._cmd_voices_install(Args())
    assert uninstalled


def test_voices_uninstall_removes_and_reverts(monkeypatch):
    order = []
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: order.append("rm"))
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    rc = cli._cmd_voices_uninstall(object())
    assert rc == 0
    assert order == ["rm", "install"]   # remove venv, then re-wire to system 3.9


def test_voices_subcommand_registered():
    parser = cli._build_parser()
    args = parser.parse_args(["voices", "install"])
    assert args.func is cli._cmd_voices_install
    assert args.engine == "kokoro"


def test_voices_subcommand_accepts_chatterbox_engine():
    parser = cli._build_parser()
    args = parser.parse_args(["voices", "install", "chatterbox"])
    assert args.func is cli._cmd_voices_install
    assert args.engine == "chatterbox"


# ---------------------------------------------------------------------------
# Task 5: engine dispatch (kokoro default, chatterbox opt-in)
# ---------------------------------------------------------------------------

def test_voices_install_default_still_kokoro(monkeypatch, tmp_path):
    """cli.main(["voices", "install"]) with no engine arg keeps calling
    kokoro_provision.install_kokoro (backward compatible)."""
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "repo_root", lambda: str(tmp_path))
    called = []
    monkeypatch.setattr(kp, "install_kokoro", lambda pythonpath: called.append(pythonpath))
    monkeypatch.setattr(kp, "neural_healthy", lambda app_dir: True)
    monkeypatch.setattr(cli, "install", lambda: 0)
    rc = cli.main(["voices", "install"])
    assert rc == 0
    assert called == [os.path.join(str(tmp_path), "src")]


def test_voices_install_chatterbox_dispatches(monkeypatch):
    """cli.main(["voices", "install", "chatterbox"]) calls
    chatterbox_provision.install_chatterbox, not the kokoro path."""
    called = []
    monkeypatch.setattr(cbp, "install_chatterbox", lambda: called.append(True))
    monkeypatch.setattr(kp, "install_kokoro",
                        lambda *a, **k: pytest.fail("must not touch kokoro"))
    monkeypatch.setattr(cli, "install", lambda: pytest.fail("chatterbox must not rewire daemon"))
    rc = cli.main(["voices", "install", "chatterbox"])
    assert rc == 0
    assert called == [True]


def test_voices_install_chatterbox_reverts_on_failure(monkeypatch):
    def boom():
        raise RuntimeError("uv missing")
    uninstalled = []
    monkeypatch.setattr(cbp, "install_chatterbox", boom)
    monkeypatch.setattr(cbp, "uninstall_chatterbox", lambda: uninstalled.append(True))
    rc = cli.main(["voices", "install", "chatterbox"])
    assert rc == 1
    assert uninstalled == [True]


def test_voices_uninstall_chatterbox_dispatches(monkeypatch):
    called = []
    monkeypatch.setattr(cbp, "uninstall_chatterbox", lambda: called.append(True))
    monkeypatch.setattr(kp, "uninstall_kokoro",
                        lambda *a, **k: pytest.fail("must not touch kokoro"))
    monkeypatch.setattr(cli, "install", lambda: pytest.fail("chatterbox must not rewire daemon"))
    rc = cli.main(["voices", "uninstall", "chatterbox"])
    assert rc == 0
    assert called == [True]


def test_voices_uninstall_default_still_kokoro(monkeypatch):
    order = []
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: order.append("rm"))
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    rc = cli.main(["voices", "uninstall"])
    assert rc == 0
    assert order == ["rm", "install"]
