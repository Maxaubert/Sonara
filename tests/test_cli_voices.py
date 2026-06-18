import pytest
from sonari import cli, paths
from sonari import kokoro_provision as kp


def test_voices_install_provisions_then_rewires_daemon(monkeypatch, tmp_path):
    order = []
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(kp, "install_kokoro", lambda app_dir: order.append(("provision", app_dir)))
    monkeypatch.setattr(kp, "neural_healthy", lambda app_dir: True)
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    rc = cli._cmd_voices_install(object())
    assert rc == 0
    assert order == [("provision", str(tmp_path / "app")), "install"]


def test_voices_install_reports_failure_without_rewiring(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    def boom(app_dir): raise RuntimeError("uv missing")
    monkeypatch.setattr(kp, "install_kokoro", boom)
    monkeypatch.setattr(cli, "install", lambda: pytest.fail("must not rewire on failure"))
    rc = cli._cmd_voices_install(object())
    assert rc == 1


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
