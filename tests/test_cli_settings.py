import json

from sonara import cli, paths


def _lock(tmp_path, monkeypatch, **extra):
    lock = tmp_path / "daemon.lock"
    lock.write_text(json.dumps({"host": "127.0.0.1", "port": 5000,
                                "token": "tok", "pid": 1, **extra}))
    monkeypatch.setattr(paths, "LOCK_PATH", lock)
    return lock


def test_settings_opens_browser_at_tokenized_url(monkeypatch, tmp_path, capsys):
    _lock(tmp_path, monkeypatch, http_port=27431)
    opened = []
    monkeypatch.setattr(cli.webbrowser, "open", lambda u: opened.append(u) or True)
    rc = cli.main(["settings"])
    assert rc == 0
    assert opened == ["http://127.0.0.1:27431/settings?token=tok"]
    assert "27431" in capsys.readouterr().out


def test_settings_daemon_down_hints_start(monkeypatch, tmp_path, capsys):
    monkeypatch.setattr(paths, "LOCK_PATH", tmp_path / "missing.lock")
    rc = cli.main(["settings"])
    assert rc == 1
    assert "sonara start" in capsys.readouterr().out


def test_settings_subcommand_registered():
    parser = cli._build_parser()
    args = parser.parse_args(["settings"])
    assert args.func is cli._cmd_settings


def test_status_prints_settings_url(monkeypatch, tmp_path, capsys):
    _lock(tmp_path, monkeypatch, http_port=27431)
    monkeypatch.setattr(cli, "_send",
                        lambda msg, expect_reply=False: {"voice": "af_heart"})
    rc = cli.main(["status"])
    assert rc == 0
    assert "Settings page: http://127.0.0.1:27431" in capsys.readouterr().out


def test_settings_survives_browser_open_failure(monkeypatch, tmp_path, capsys):
    _lock(tmp_path, monkeypatch, http_port=27431)
    def boom(url):
        raise OSError("no browser")
    monkeypatch.setattr(cli.webbrowser, "open", boom)
    rc = cli.main(["settings"])
    assert rc == 0                                    # URL printed; that's enough
    assert "27431" in capsys.readouterr().out
