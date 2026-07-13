from unittest import mock

from sonara import cli, chatterbox, paths
from sonara import kokoro_provision as kp
from tests._fakeplatform import fake_platform, FakeSupervisor, FakeHotkey


def _patches(rows=None, hooks_row=None, send=None, install_record=None):
    sup = FakeSupervisor(
        rows=rows if rows is not None else [("say", True, "/usr/bin/say"),
                                            ("afplay", True, "/usr/bin/afplay")],
        hooks_row=hooks_row or ("hooks installed", True, "/plug/hooks/hooks.json"),
    )
    return sup, [
        mock.patch.object(cli, "_platform", lambda: fake_platform(supervisor=sup)),
        mock.patch("os.access", return_value=True),
        mock.patch("sonara.paths.ensure_sonara_dir"),
        mock.patch("sonara.client.send",
                   return_value=(send if send is not None else {"ok": True})),
        mock.patch.object(
            cli, "_read_install_record",
            return_value=install_record or {"app_path": "/home/u/.sonara/app"}),
        mock.patch("os.path.exists", return_value=True),
    ]


def _run(patches):
    for p in patches:
        p.start()
    try:
        return cli.doctor()
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
    assert d["say"][0] is True and d["afplay"][0] is True
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
    with mock.patch("sonara.cli.doctor",
                    return_value=[("say", True, "/usr/bin/say"),
                                  ("afplay", False, "not found")]):
        rc = cli.main(["doctor"])
    out = capsys.readouterr().out
    assert "say" in out and "afplay" in out
    assert rc == 1  # any failing check -> non-zero


def test_doctor_subcommand_all_ok_returns_zero(capsys):
    with mock.patch("sonara.cli.doctor", return_value=[("say", True, "ok")]):
        rc = cli.main(["doctor"])
    assert rc == 0
    assert "say" in capsys.readouterr().out


def test_doctor_includes_hotkey_rows(monkeypatch):
    from tests._fakeplatform import fake_platform, FakeSupervisor

    class HK:
        def doctor_rows(self):
            return [("hotkey chords", True, "no collisions")]

    pb = fake_platform(supervisor=FakeSupervisor())
    pb.hotkey = HK()
    monkeypatch.setattr(cli, "_platform", lambda: pb)
    monkeypatch.setattr("os.access", lambda *a, **k: True)
    monkeypatch.setattr("sonara.paths.ensure_sonara_dir", lambda: None)
    monkeypatch.setattr("sonara.client.send", lambda *a, **k: {"ok": True})
    monkeypatch.setattr(cli, "_read_install_record", lambda: {"app_path": "/a"})
    monkeypatch.setattr("os.path.exists", lambda p: True)
    names = {r[0] for r in cli.doctor()}
    assert "hotkey chords" in names


def _doctor_rows(monkeypatch):
    pb = fake_platform(supervisor=FakeSupervisor(), hotkey=FakeHotkey(ok=True, detail="ok"))
    monkeypatch.setattr(cli, "_platform", lambda: pb)
    return {name: (ok, detail) for name, ok, detail in cli.doctor()}


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
# Task 5: chatterbox doctor row
# ---------------------------------------------------------------------------

def test_doctor_chatterbox_rows_absent_ok(monkeypatch):
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: False)
    rows = _doctor_rows(monkeypatch)
    assert "chatterbox" in rows
    ok, detail = rows["chatterbox"]
    assert ok is True and "not installed" in detail


def test_doctor_chatterbox_provisioned_checks_python(monkeypatch, tmp_path):
    fake_py = tmp_path / "python.exe"
    fake_py.write_text("")
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(paths, "chatterbox_venv_python", lambda: str(fake_py))
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["chatterbox"]
    assert ok is True
    assert str(fake_py) in detail


def test_doctor_chatterbox_fails_when_venv_python_missing(monkeypatch, tmp_path):
    missing_py = tmp_path / "no-such-venv" / "python.exe"
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(paths, "chatterbox_venv_python", lambda: str(missing_py))
    rows = _doctor_rows(monkeypatch)
    ok, detail = rows["chatterbox"]
    assert ok is False
    assert "voices install chatterbox" in detail
