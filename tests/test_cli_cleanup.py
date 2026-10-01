"""`sonara cleanup` (#134, D4): removes the removed Chatterbox engine's venv,
model cache and smoke-test files on request. The daemon is stopped first
(a running worker locks files in the venv), and voices/chatterbox, the
user's own recorded clips, is never touched."""
from sonara import cli, paths


def _leftovers():
    venv = paths.CHATTERBOX_VENV / "Scripts"
    venv.mkdir(parents=True)
    (venv / "python.exe").write_bytes(b"x")
    cache = paths.CHATTERBOX_MODEL_CACHE / "hf-cache"
    cache.mkdir(parents=True)
    (cache / "model.bin").write_bytes(b"x")
    (paths.SONARA_DIR / "chatterbox-smoke-results.txt").write_bytes(b"x")
    clips = paths.CHATTERBOX_VOICES_DIR
    clips.mkdir(parents=True)
    (clips / "cherami.wav").write_bytes(b"RIFF")


def _daemon(monkeypatch, running, order):
    def stop(sup=None):
        order.append("stop")
        paths.STOPPED_SENTINEL_PATH.write_text("sonara shutdown")   # as the real one
        return True
    monkeypatch.setattr(paths, "socket_connectable", lambda: running)
    monkeypatch.setattr(cli, "stop_sonara", stop)
    monkeypatch.setattr(cli, "start_sonara", lambda: order.append("start") or 0)


def test_cleanup_is_registered():
    args = cli._build_parser().parse_args(["cleanup"])
    assert args.func is cli._cmd_cleanup


def test_cleanup_stops_the_daemon_before_removing(monkeypatch):
    from sonara import chatterbox_legacy as cl
    _leftovers()
    order = []
    _daemon(monkeypatch, True, order)
    real = cl.remove_leftovers
    monkeypatch.setattr(cl, "remove_leftovers",
                        lambda: order.append("rm") or real())
    assert cli.main(["cleanup"]) == 0
    assert order == ["stop", "rm", "start"]       # running before: brought back


def test_cleanup_removes_leftovers_and_keeps_user_clips(monkeypatch):
    _leftovers()
    _daemon(monkeypatch, False, [])
    assert cli.main(["cleanup"]) == 0
    assert not paths.CHATTERBOX_VENV.exists()
    assert not paths.CHATTERBOX_MODEL_CACHE.exists()
    assert not (paths.SONARA_DIR / "chatterbox-smoke-results.txt").exists()
    assert (paths.CHATTERBOX_VOICES_DIR / "cherami.wav").exists()


def test_cleanup_does_not_start_a_daemon_that_was_not_running(monkeypatch):
    # Not running but not shut down either (hooks lazy-start it): cleanup must
    # not start it, and must not leave the stop sentinel behind, which would
    # block that lazy start.
    _leftovers()
    order = []
    _daemon(monkeypatch, False, order)
    cli.main(["cleanup"])
    assert "start" not in order
    assert not paths.STOPPED_SENTINEL_PATH.exists()


def test_cleanup_keeps_an_explicit_shutdown_in_place(monkeypatch):
    _leftovers()
    paths.STOPPED_SENTINEL_PATH.write_text("sonara shutdown")
    order = []
    _daemon(monkeypatch, False, order)
    cli.main(["cleanup"])
    assert "start" not in order
    assert paths.STOPPED_SENTINEL_PATH.exists()


def test_cleanup_with_nothing_to_remove_does_not_stop_the_daemon(monkeypatch, capsys):
    paths.ensure_sonara_dir()
    order = []
    _daemon(monkeypatch, True, order)
    assert cli.main(["cleanup"]) == 0
    assert order == []
    assert "Nothing to clean up" in capsys.readouterr().out


def test_cleanup_refuses_when_the_daemon_does_not_stop(monkeypatch):
    from sonara import chatterbox_legacy as cl
    _leftovers()
    monkeypatch.setattr(paths, "socket_connectable", lambda: True)
    monkeypatch.setattr(cli, "stop_sonara", lambda sup=None: False)
    monkeypatch.setattr(cli, "start_sonara", lambda: 0)
    monkeypatch.setattr(cl, "remove_leftovers",
                        lambda: (_ for _ in ()).throw(AssertionError("removed")))
    assert cli.main(["cleanup"]) == 1
    assert paths.CHATTERBOX_VENV.exists()


def test_cleanup_that_refuses_restores_the_daemon_state(monkeypatch):
    # stop_sonara writes the sentinel and ends the task before it gives up.
    # A refused cleanup must not leave Sonara half shut down: a daemon that
    # was running is started again, so the sentinel and the task are restored.
    from sonara import chatterbox_legacy as cl
    _leftovers()
    order = []

    def stop(sup=None):
        order.append("stop")
        paths.STOPPED_SENTINEL_PATH.write_text("sonara shutdown")
        return False

    def start():
        order.append("start")
        paths.STOPPED_SENTINEL_PATH.unlink()
        return 0
    monkeypatch.setattr(paths, "socket_connectable", lambda: True)
    monkeypatch.setattr(cli, "stop_sonara", stop)
    monkeypatch.setattr(cli, "start_sonara", start)
    monkeypatch.setattr(cl, "remove_leftovers",
                        lambda: (_ for _ in ()).throw(AssertionError("removed")))
    assert cli.main(["cleanup"]) == 1
    assert order == ["stop", "start"]
    assert not paths.STOPPED_SENTINEL_PATH.exists()


def test_cleanup_that_refuses_keeps_an_explicit_shutdown(monkeypatch):
    from sonara import chatterbox_legacy as cl
    _leftovers()
    paths.STOPPED_SENTINEL_PATH.write_text("sonara shutdown")
    order = []
    monkeypatch.setattr(paths, "socket_connectable", lambda: False)
    monkeypatch.setattr(cli, "stop_sonara",
                        lambda sup=None: order.append("stop") and False)
    monkeypatch.setattr(cli, "start_sonara", lambda: order.append("start") or 0)
    monkeypatch.setattr(cl, "remove_leftovers",
                        lambda: (_ for _ in ()).throw(AssertionError("removed")))
    assert cli.main(["cleanup"]) == 1
    assert "start" not in order
    assert paths.STOPPED_SENTINEL_PATH.exists()


def test_cleanup_reports_a_path_it_could_not_remove(monkeypatch, capsys):
    from sonara import chatterbox_legacy as cl
    _leftovers()
    _daemon(monkeypatch, False, [])
    monkeypatch.setattr(cl, "remove_leftovers", lambda: (
        [], [(paths.CHATTERBOX_VENV, PermissionError("locked"))]))
    assert cli.main(["cleanup"]) == 1
    assert "chatterbox-venv" in capsys.readouterr().err
