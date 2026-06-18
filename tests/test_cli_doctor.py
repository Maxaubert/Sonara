from unittest import mock

from sonari import cli
from tests._fakeplatform import fake_platform, FakeSupervisor


def _patches(rows=None, hooks_row=None, send=None, install_record=None):
    sup = FakeSupervisor(
        rows=rows if rows is not None else [("say", True, "/usr/bin/say"),
                                            ("afplay", True, "/usr/bin/afplay")],
        hooks_row=hooks_row or ("hooks installed", True, "/plug/hooks/hooks.json"),
    )
    return sup, [
        mock.patch.object(cli, "_platform", lambda: fake_platform(supervisor=sup)),
        mock.patch("os.access", return_value=True),
        mock.patch("sonari.paths.ensure_sonari_dir"),
        mock.patch("sonari.client.send",
                   return_value=(send if send is not None else {"ok": True})),
        mock.patch.object(
            cli, "_read_install_record",
            return_value=install_record or {"app_path": "/home/u/.sonari/app"}),
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
    for key in ("SONARI_DIR writable", "daemon socket", "hooks installed",
                "keymap resolves", "python3", "plugin path resolved"):
        assert key in d, key


def test_doctor_socket_unreachable():
    _, patches = _patches()
    patches[3] = mock.patch("sonari.client.send", side_effect=ConnectionRefusedError())
    d = _as_dict(_run(patches))
    assert d["daemon socket"][0] is False


def test_doctor_hooks_row_comes_from_backend():
    _, patches = _patches(hooks_row=("hooks installed", False, "no Sonari hooks"))
    d = _as_dict(_run(patches))
    assert d["hooks installed"][0] is False


def test_doctor_subcommand_prints_and_returns(capsys):
    with mock.patch("sonari.cli.doctor",
                    return_value=[("say", True, "/usr/bin/say"),
                                  ("afplay", False, "not found")]):
        rc = cli.main(["doctor"])
    out = capsys.readouterr().out
    assert "say" in out and "afplay" in out
    assert rc == 1  # any failing check -> non-zero


def test_doctor_subcommand_all_ok_returns_zero(capsys):
    with mock.patch("sonari.cli.doctor", return_value=[("say", True, "ok")]):
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
    monkeypatch.setattr("sonari.paths.ensure_sonari_dir", lambda: None)
    monkeypatch.setattr("sonari.client.send", lambda *a, **k: {"ok": True})
    monkeypatch.setattr(cli, "_read_install_record", lambda: {"app_path": "/a"})
    monkeypatch.setattr("os.path.exists", lambda p: True)
    names = {r[0] for r in cli.doctor()}
    assert "hotkey chords" in names
