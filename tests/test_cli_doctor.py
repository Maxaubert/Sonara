import pytest
from unittest import mock

from sonara import cli
from sonara.install import doctor as install_doctor
from sonara import install_record
from sonara import kokoro_provision as kp
from tests._fakeplatform import fake_platform, FakeSupervisor, FakeHotkey


def _patches(rows=None, hooks_row=None, send=None, install_record=None):
    sup = FakeSupervisor(
        rows=rows if rows is not None else [("schtasks", True, r"C:\Windows\System32\schtasks.exe"),
                                            ("Windows voice", True, "Microsoft David")],
        hooks_row=hooks_row or ("hooks installed", True, "/plug/hooks/hooks.json"),
    )
    return sup, [
        mock.patch("sonara.platform.get_platform", lambda: fake_platform(supervisor=sup)),
        mock.patch("os.access", return_value=True),
        mock.patch("sonara.paths.ensure_sonara_dir"),
        mock.patch("sonara.client.send",
                   return_value=(send if send is not None else {"ok": True})),
        mock.patch(
            "sonara.install_record.read",
            return_value=install_record or {"app_path": "/home/u/.sonara/app"}),
        mock.patch("os.path.exists", return_value=True),
    ]


def _run(patches):
    for p in patches:
        p.start()
    try:
        return install_doctor.doctor()
    finally:
        for p in reversed(patches):
            p.stop()


def _as_dict(results):
    return {check: (ok, detail) for check, ok, detail in results}


def test_doctor_returns_tuples():
    _, patches = _patches()
    results = _run(patches)
    assert isinstance(results, list)
    for row in results:
        assert len(row) == 3
        check, ok, detail = row
        assert isinstance(check, str) and isinstance(ok, bool) and isinstance(detail, str)


def test_doctor_includes_os_rows_and_neutral_rows():
    _, patches = _patches()
    d = _as_dict(_run(patches))
    # OS rows came from the platform supervisor.
    assert d["schtasks"][0] is True and d["Windows voice"][0] is True
    # Neutral rows added by cli.
    for key in ("SONARA_DIR writable", "daemon socket", "hooks installed",
                "keymap resolves", "python3", "plugin path resolved"):
        assert key in d, key


def test_doctor_socket_unreachable():
    _, patches = _patches()
    patches[3] = mock.patch("sonara.client.send", side_effect=ConnectionRefusedError())
    d = _as_dict(_run(patches))
    assert d["daemon socket"][0] is False


def test_doctor_hooks_row_comes_from_backend():
    _, patches = _patches(hooks_row=("hooks installed", False, "no Sonara hooks"))
    d = _as_dict(_run(patches))
    assert d["hooks installed"][0] is False


def test_doctor_subcommand_prints_and_returns(capsys):
    with mock.patch("sonara.install.doctor.doctor",
                    return_value=[("schtasks", True, "schtasks.exe"),
                                  ("Windows voice", False, "voice data missing")]):
        rc = cli.main(["doctor"])
    out = capsys.readouterr().out
    assert "schtasks" in out and "Windows voice" in out
    assert rc == 1  # any failing check -> non-zero


def test_doctor_warning_row_prints_warn_and_keeps_exit_zero(capsys):
    """#160: a warning row (the AltGr clash) is shown as [warn] and does not
    turn doctor's exit code into a failure."""
    from sonara.platform.base import DOCTOR_WARN
    with mock.patch("sonara.install.doctor.doctor",
                    return_value=[("schtasks", True, "ok"),
                                  ("AltGr", DOCTOR_WARN, "Ctrl+Alt+M types \u00b5")]):
        rc = cli.main(["doctor"])
    out = capsys.readouterr().out
    assert "[warn] AltGr:" in out
    assert rc == 0


def test_doctor_subcommand_all_ok_returns_zero(capsys):
    with mock.patch("sonara.install.doctor.doctor", return_value=[("schtasks", True, "ok")]):
        rc = cli.main(["doctor"])
    assert rc == 0
    assert "schtasks" in capsys.readouterr().out


def test_doctor_includes_hotkey_rows(monkeypatch):
    from tests._fakeplatform import fake_platform, FakeSupervisor

    class HK:
        def doctor_rows(self):
            return [("hotkey chords", True, "no collisions")]

    pb = fake_platform(supervisor=FakeSupervisor())
    pb.hotkey = HK()
    monkeypatch.setattr("sonara.platform.get_platform", lambda: pb)
    monkeypatch.setattr("os.access", lambda *a, **k: True)
    monkeypatch.setattr("sonara.paths.ensure_sonara_dir", lambda: None)
    monkeypatch.setattr("sonara.client.send", lambda *a, **k: {"ok": True})
    monkeypatch.setattr(install_record, "read", lambda: {"app_path": "/a"})
    monkeypatch.setattr("os.path.exists", lambda p: True)
    names = {r[0] for r in install_doctor.doctor()}
    assert "hotkey chords" in names


def _doctor_rows(monkeypatch):
    pb = fake_platform(supervisor=FakeSupervisor(), hotkey=FakeHotkey(ok=True, detail="ok"))
    monkeypatch.setattr("sonara.platform.get_platform", lambda: pb)
    return {name: (ok, detail) for name, ok, detail in install_doctor.doctor()}


def test_doctor_neural_row_ok_and_green_when_absent(monkeypatch):
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    rows = _doctor_rows(monkeypatch)
    assert "neural voices" in rows
    ok, detail = rows["neural voices"]
    assert ok is True and "not installed" in detail


def test_doctor_neural_row_fails_when_venv_unhealthy(monkeypatch):
    monkeypatch.setattr(kp, "neural_enabled", lambda: True)
    monkeypatch.setattr(kp, "neural_healthy", lambda app: False)
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["neural voices"]
    assert ok is False and "voices install" in detail


def test_doctor_neural_row_ready_when_healthy(monkeypatch):
    """Healthy venv -> (True, detail containing "ready") with the venv python path."""
    monkeypatch.setattr(kp, "neural_enabled", lambda: True)
    monkeypatch.setattr(kp, "neural_healthy", lambda app: True)
    monkeypatch.setattr("sonara.paths.kokoro_venv_python", lambda: "/venv/bin/python")
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["neural voices"]
    assert ok is True and "ready" in detail


def test_doctor_summary_row_ok_when_mode_off(monkeypatch, tmp_path):
    from sonara import config
    monkeypatch.setattr(config, "CONFIG_PATH", tmp_path / "config.json")
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["summary command"]
    assert ok is True and "off" in detail


def test_doctor_summary_row_fails_when_command_missing(monkeypatch, tmp_path):
    import json
    from sonara import config
    cfg_path = tmp_path / "config.json"
    cfg_path.write_text(json.dumps({"summary_mode": True,
                                    "summary_command": "definitely-not-a-cmd-xyz"}),
                        encoding="utf-8")
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_path)
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["summary command"]
    assert ok is False


# ---------------------------------------------------------------------------
# Chatterbox leftovers (#134, D4): reported with sizes, removed only on request
# ---------------------------------------------------------------------------

def test_doctor_reports_no_chatterbox_leftovers_on_a_clean_install(monkeypatch):
    from sonara import chatterbox_legacy as cl
    monkeypatch.setattr(cl, "leftovers_estimate", lambda **k: [])
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["chatterbox leftovers"]
    assert ok is True and detail == "none"


def test_doctor_reports_chatterbox_leftovers_with_sizes_and_the_fix(monkeypatch):
    from pathlib import Path
    from sonara import chatterbox_legacy as cl
    monkeypatch.setattr(cl, "leftovers_estimate", lambda **k: [
        (Path("/s/chatterbox-venv"), 5 * 1024 ** 3, True),
        (Path("/s/chatterbox"), 4 * 1024 ** 3, True),
        (Path("/s/cb-client-ok.wav"), 1024 * 1024, True),
    ])
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["chatterbox leftovers"]
    assert ok is True                       # informational, never a failure
    assert "9.0 GB" in detail
    assert "chatterbox-venv (5.0 GB)" in detail
    assert "sonara cleanup" in detail


# ---------------------------------------------------------------------------
# Neural voices: report what the DAEMON's interpreter can use (found live: the
# system Python had Kokoro in its user site while doctor said 'not installed')
# ---------------------------------------------------------------------------

def _daemon_on(monkeypatch, python=r"C:\Py\python.exe"):
    monkeypatch.setattr(install_record, "read",
                        lambda: {"python": python, "app_path": "/app"})


def test_doctor_neural_row_ready_when_the_daemon_python_imports_kokoro(monkeypatch):
    from sonara import kokoro
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    _daemon_on(monkeypatch)
    monkeypatch.setattr(kp, "kokoro_importable", lambda python: True)
    monkeypatch.setattr(kokoro, "models_present", lambda d: True)
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["neural voices"]
    assert ok is True and "ready" in detail and r"C:\Py\python.exe" in detail
    assert "not installed" not in detail


def test_doctor_neural_row_says_the_model_downloads_on_first_use(monkeypatch):
    from sonara import kokoro
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    _daemon_on(monkeypatch)
    monkeypatch.setattr(kp, "kokoro_importable", lambda python: True)
    monkeypatch.setattr(kokoro, "models_present", lambda d: False)
    monkeypatch.setattr(kokoro, "download_failed_at", lambda d: None)
    ok, detail = _doctor_rows(monkeypatch)["neural voices"]
    assert ok is True and "first use" in detail


def test_doctor_neural_row_fails_after_a_failed_model_download(monkeypatch):
    import time
    from sonara import kokoro
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    _daemon_on(monkeypatch)
    monkeypatch.setattr(kp, "kokoro_importable", lambda python: True)
    monkeypatch.setattr(kokoro, "models_present", lambda d: False)
    monkeypatch.setattr(kokoro, "download_failed_at", lambda d: time.time() - 60)
    ok, detail = _doctor_rows(monkeypatch)["neural voices"]
    assert ok is False and "voices install" in detail


def test_doctor_neural_row_not_installed_when_the_daemon_python_lacks_it(monkeypatch):
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    _daemon_on(monkeypatch)
    monkeypatch.setattr(kp, "kokoro_importable", lambda python: False)
    ok, detail = _doctor_rows(monkeypatch)["neural voices"]
    assert ok is True and "not installed" in detail


def test_kokoro_importable_probes_the_given_interpreter():
    seen = {}

    def run(cmd, **k):
        seen["cmd"] = cmd
        return "True\n"
    assert kp.kokoro_importable(r"C:\Py\pythonw.exe", run=run) is True
    assert seen["cmd"][0] == r"C:\Py\pythonw.exe"
    assert kp.kokoro_importable("x", run=lambda *a, **k: "False\n") is False

    def boom(*a, **k):
        raise OSError("gone")
    assert kp.kokoro_importable("x", run=boom) is False


def test_doctor_chatterbox_row_uses_the_bounded_walk(monkeypatch):
    """An 8 GB venv must not be walked file by file on every doctor run."""
    from pathlib import Path
    from sonara import chatterbox_legacy as cl
    monkeypatch.setattr(cl, "leftovers",
                        lambda: pytest.fail("doctor walked the full tree"))
    monkeypatch.setattr(cl, "leftovers_estimate", lambda **k: [
        (Path("/s/chatterbox-venv"), 1024 ** 3, False),
        (Path("/s/cb-client-ok.wav"), 1024, True),
    ])
    ok, detail = _doctor_rows(monkeypatch)["chatterbox leftovers"]
    assert ok is True
    assert "more than 1.0 GB" in detail
    assert "chatterbox-venv (more than 1.0 GB)" in detail
    assert "sonara cleanup" in detail


def test_doctor_output_survives_a_narrow_console_encoding(monkeypatch):
    """The AltGr row names characters like the micro sign; a cp437 pipe
    must not turn that into a UnicodeEncodeError traceback."""
    import io
    import sys
    raw = io.BytesIO()
    out = io.TextIOWrapper(raw, encoding="ascii", errors="strict")
    monkeypatch.setattr(sys, "stdout", out)
    monkeypatch.setattr(install_doctor, "doctor", lambda: [("AltGr", False, "types '\u00b5'")])
    assert cli._cmd_doctor(None) == 1
    out.flush()
    assert b"AltGr" in raw.getvalue()
