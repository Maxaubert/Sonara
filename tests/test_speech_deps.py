"""Installing the speech engine into the interpreter the daemon runs on (#139).

E1: the bootstrap's uv-managed Python is PEP 668 "externally managed", so
`pip install --user` is refused and a zero-Python install ended silent.
E2: `sonara voices install` moves the daemon onto a uv venv that has no pip
and no PyWinRT/pycaw.
"""
import os
import sys

from sonara import cli, paths
from sonara import kokoro_provision as kp


def _capture(monkeypatch, env, uv="UV"):
    states = iter([False, True])
    monkeypatch.setattr(cli, "_winrt_importable", lambda python: next(states))
    monkeypatch.setattr(cli, "_python_env", lambda python: env)
    monkeypatch.setattr(cli, "_find_uv", lambda: uv)
    ran = []
    monkeypatch.setattr(cli.subprocess, "run", lambda cmd, **k: ran.append(cmd))
    assert cli._ensure_speech_deps("PY") is True
    assert len(ran) == 1
    return ran[0]


def test_externally_managed_python_installs_through_uv(monkeypatch):
    cmd = _capture(monkeypatch, {"venv": False, "managed": True, "pip": True})
    assert cmd[:5] == ["UV", "pip", "install", "--python", "PY"]
    assert "--break-system-packages" in cmd
    assert "--user" not in cmd
    assert "winrt-runtime" in cmd and "pycaw" in cmd


def test_externally_managed_python_without_uv_overrides_pep668(monkeypatch):
    cmd = _capture(monkeypatch, {"venv": False, "managed": True, "pip": True}, uv=None)
    assert cmd[:4] == ["PY", "-m", "pip", "install"]
    assert "--break-system-packages" in cmd


def test_venv_without_pip_installs_through_uv(monkeypatch):
    cmd = _capture(monkeypatch, {"venv": True, "managed": False, "pip": False})
    assert cmd[:5] == ["UV", "pip", "install", "--python", "PY"]
    assert "--user" not in cmd and "--break-system-packages" not in cmd


def test_venv_with_pip_installs_without_user(monkeypatch):
    cmd = _capture(monkeypatch, {"venv": True, "managed": False, "pip": True})
    assert cmd[:4] == ["PY", "-m", "pip", "install"]
    assert "--user" not in cmd


def test_plain_system_python_keeps_pip_user(monkeypatch):
    cmd = _capture(monkeypatch, {"venv": False, "managed": False, "pip": True})
    assert cmd[:5] == ["PY", "-m", "pip", "install", "--user"]


def test_manual_hint_is_the_command_that_applies(monkeypatch, capsys):
    monkeypatch.setattr(cli, "_winrt_importable", lambda python: False)
    monkeypatch.setattr(cli, "_python_env",
                        lambda python: {"venv": False, "managed": True, "pip": True})
    monkeypatch.setattr(cli, "_find_uv", lambda: "UV")
    monkeypatch.setattr(cli.subprocess, "run", lambda cmd, **k: None)
    assert cli._ensure_speech_deps("PY") is False
    out = capsys.readouterr().out
    assert "UV pip install --python PY --break-system-packages" in out


def test_find_uv_uses_the_bootstrap_copy(monkeypatch):
    monkeypatch.setattr(cli.shutil, "which", lambda name: None)
    tools = paths.SONARA_DIR / "tools"
    tools.mkdir(parents=True)
    (tools / "uv.exe").write_bytes(b"")
    assert cli._find_uv() == str(tools / "uv.exe")


def test_python_env_probes_the_real_interpreter():
    env = cli._python_env(sys.executable)
    assert env["venv"] is (sys.prefix != sys.base_prefix)
    assert env["pip"] is True
    assert isinstance(env["managed"], bool)


def test_kokoro_venv_requirements_include_the_speech_engine():
    # E2: the daemon runs on this venv once neural voices are on, so it needs
    # PyWinRT and pycaw too, not just the Kokoro stack.
    with open(kp.requirements_path(), encoding="utf-8") as fh:
        names = {line.split("==")[0].split(";")[0].strip().lower()
                 for line in fh if line.strip() and not line.startswith("#")}
    for pkg in cli._WINRT_PACKAGES:
        assert pkg.lower() in names, pkg


def test_requirements_file_is_shipped_with_the_package():
    assert os.path.isfile(kp.requirements_path())


def test_installer_runs_on_the_console_interpreter(monkeypatch, tmp_path):
    pyw = tmp_path / "pythonw.exe"
    py = tmp_path / "python.exe"
    pyw.write_bytes(b"")
    py.write_bytes(b"")
    states = iter([False, True])
    monkeypatch.setattr(cli, "_winrt_importable", lambda python: next(states))
    probed = []
    monkeypatch.setattr(cli, "_python_env", lambda python: probed.append(python) or {})
    monkeypatch.setattr(cli, "_find_uv", lambda: None)
    ran = []
    monkeypatch.setattr(cli.subprocess, "run", lambda cmd, **k: ran.append(cmd))
    assert cli._ensure_speech_deps(str(pyw)) is True
    assert probed == [str(py)] and ran[0][0] == str(py)
