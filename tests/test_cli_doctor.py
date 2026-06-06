from unittest import mock

from sonari import cli


def _ok_patches():
    """Context managers that make every doctor check pass."""
    return [
        mock.patch("shutil.which", side_effect=lambda n: "/usr/bin/" + n),
        mock.patch("sonari.speaker.best_enhanced_voice", return_value="Ava (Premium)"),
        mock.patch("os.access", return_value=True),
        mock.patch("sonari.paths.ensure_sonari_dir"),
        mock.patch("sonari.client.send", return_value={"ok": True}),
        mock.patch("os.path.exists", return_value=True),
        mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"),
        mock.patch.object(cli, "_launchctl", return_value=0),
        mock.patch.object(cli, "_local_bin_on_path", return_value=True),
        mock.patch.object(cli, "_read_install_record",
                          return_value={"app_path": "/home/u/.sonari/app"}),
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
    results = _run(_ok_patches())
    assert isinstance(results, list)
    for row in results:
        assert len(row) == 3
        check, ok, detail = row
        assert isinstance(check, str)
        assert isinstance(ok, bool)
        assert isinstance(detail, str)


def test_doctor_all_ok():
    d = _as_dict(_run(_ok_patches()))
    for key in ("say", "afplay", "enhanced voice", "SONARI_DIR writable",
                "daemon socket", "plugin hooks.json", "python3",
                "plugin path resolved", "speechd LaunchAgent loaded",
                "hotkeyd LaunchAgent loaded", "sonari launcher"):
        assert key in d, key
        assert d[key][0] is True, (key, d[key])


def test_doctor_say_missing():
    patches = _ok_patches()
    patches[0] = mock.patch(
        "shutil.which",
        side_effect=lambda n: None if n == "say" else "/usr/bin/" + n)
    d = _as_dict(_run(patches))
    assert d["say"][0] is False
    assert d["afplay"][0] is True


def test_doctor_socket_unreachable():
    patches = _ok_patches()
    patches[4] = mock.patch("sonari.client.send",
                            side_effect=ConnectionRefusedError())
    d = _as_dict(_run(patches))
    assert d["daemon socket"][0] is False


def test_doctor_hooks_json_missing():
    patches = _ok_patches()
    patches[5] = mock.patch("os.path.exists", return_value=False)
    d = _as_dict(_run(patches))
    assert d["plugin hooks.json"][0] is False


def test_doctor_subcommand_prints_and_returns(capsys):
    with mock.patch("sonari.cli.doctor",
                    return_value=[("say", True, "/usr/bin/say"),
                                  ("afplay", False, "not found")]):
        rc = cli.main(["doctor"])
    out = capsys.readouterr().out
    assert "say" in out and "afplay" in out
    # Any failing check makes the command exit non-zero.
    assert rc == 1


def test_doctor_subcommand_all_ok_returns_zero(capsys):
    with mock.patch("sonari.cli.doctor",
                    return_value=[("say", True, "ok")]):
        rc = cli.main(["doctor"])
    assert rc == 0
    assert "say" in capsys.readouterr().out
