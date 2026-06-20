# tests/test_macos_supervisor.py
import plistlib

from sonara.platform.macos.supervisor import MacSupervisorBackend


def test_launchagent_plist_body_locks_autostart_contract():
    # Distinctive sentinels so the assertions prove the body, not constants.
    python = "/usr/bin/python3"
    app_dir = "/my/app/src"
    log = "/tmp/speechd.log"
    xml = MacSupervisorBackend().launchagent_plist(
        python_executable=python, src_path=app_dir, log_path=log)
    parsed = plistlib.loads(xml.encode("utf-8"))
    assert parsed["ProgramArguments"] == [python, "-m", "sonara.daemon"]
    assert parsed["EnvironmentVariables"]["PYTHONPATH"] == app_dir
    assert parsed["RunAtLoad"] is True


def test_resolve_python_prefers_usr_bin(monkeypatch):
    sup = MacSupervisorBackend()
    monkeypatch.setattr(sup, "_probe_python_version", lambda c: (3, 11))
    monkeypatch.setattr("sonara.platform.macos.supervisor.shutil.which",
                        lambda n: "/opt/homebrew/bin/python3")
    monkeypatch.setattr("sonara.platform.macos.supervisor.os.path.realpath",
                        lambda p: p)
    assert sup.resolve_python() == "/usr/bin/python3"


def test_launch_spec_uses_start_new_session():
    argv, kwargs = MacSupervisorBackend().launch_spec()
    assert argv[-1].endswith("sonara-daemon")
    assert kwargs.get("start_new_session") is True


def test_doctor_rows_include_say_and_swiftc(monkeypatch):
    monkeypatch.setattr("sonara.platform.macos.supervisor.shutil.which",
                        lambda n: "/usr/bin/" + n)
    names = [r[0] for r in MacSupervisorBackend().doctor_rows()]
    assert "say" in names and "swiftc" in names


def test_hooks_doctor_row_checks_repo_manifest():
    name, ok, detail = MacSupervisorBackend().hooks_doctor_row()
    assert name == "hooks installed"
    assert detail.endswith("hooks.json") or "missing" in detail


def test_post_install_notes_prints_next_steps(capsys):
    MacSupervisorBackend().post_install_notes()
    out = capsys.readouterr().out
    assert "sonara doctor" in out


def test_install_writes_and_loads_launchagent(tmp_path, monkeypatch):
    import sonara.platform.macos.supervisor as ms
    plist = tmp_path / "speechd.plist"
    monkeypatch.setattr(ms, "LAUNCH_AGENT_PATH", str(plist))
    calls = []
    sup = ms.MacSupervisorBackend()
    monkeypatch.setattr(sup, "launchctl", lambda a: calls.append(a) or 0)
    monkeypatch.setattr(sup, "place_launcher", lambda root: str(tmp_path / "sonara"))
    monkeypatch.setattr(ms.shutil, "which", lambda n: "/usr/bin/swiftc")
    sup.install("/usr/bin/python3", str(tmp_path / "app"))
    assert plist.exists()
    assert ["load", str(plist)] in calls and ["unload", str(plist)] in calls


def test_uninstall_removes_launchagent_and_launcher(tmp_path, monkeypatch):
    import sonara.platform.macos.supervisor as ms
    plist = tmp_path / "speechd.plist"
    plist.write_text("<plist/>", encoding="utf-8")
    launcher = tmp_path / "sonara"
    launcher.write_text("#!/bin/sh\n", encoding="utf-8")
    monkeypatch.setattr(ms, "LAUNCH_AGENT_PATH", str(plist))
    monkeypatch.setattr(ms, "_launcher_path", lambda: str(launcher))
    sup = ms.MacSupervisorBackend()
    monkeypatch.setattr(sup, "launchctl", lambda a: 0)
    sup.uninstall()
    assert not plist.exists() and not launcher.exists()


def test_doctor_rows_include_macos_checks(monkeypatch):
    import sonara.platform.macos.supervisor as ms
    sup = ms.MacSupervisorBackend()
    monkeypatch.setattr(ms.shutil, "which", lambda n: "/usr/bin/" + n)
    monkeypatch.setattr(sup, "launchctl", lambda a: 0)
    monkeypatch.setattr("sonara.platform.macos.tts.MacTtsBackend.best_voice",
                        lambda self: "Ava (Premium)")
    names = {row[0] for row in sup.doctor_rows()}
    assert {"say", "afplay", "swiftc", "hotkeyd binary",
            "speechd LaunchAgent loaded"} <= names


def test_place_launcher_writes_wrapper_execing_plugin_bin(tmp_path, monkeypatch):
    import os
    import sonara.platform.macos.supervisor as ms
    launcher = tmp_path / ".local" / "bin" / "sonara"
    monkeypatch.setattr(ms, "_launcher_path", lambda: str(launcher))
    returned = ms.MacSupervisorBackend().place_launcher("/plug")
    assert returned == str(launcher) and launcher.exists()
    body = launcher.read_text()
    assert body.startswith("#!/usr/bin/env bash")
    # os.path.join uses the host separator; compare against the same join.
    assert os.path.join("/plug", "bin", "sonara") in body
