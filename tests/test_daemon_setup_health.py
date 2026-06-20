
from tests.daemon_helpers import make_daemon


def _write_install_json(tmp_path, plugin_version="0.4.0"):
    rec = tmp_path / "install.json"
    import json
    rec.write_text(json.dumps({"plugin_version": plugin_version}))
    return rec


def test_setup_health_not_installed_when_no_record(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    missing = tmp_path / "install.json"  # never created
    monkeypatch.setattr("sonara.daemon.INSTALL_RECORD_PATH", str(missing))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "not_installed"
    assert "slash sonara install" in cue.lower()


def test_setup_health_not_installed_when_launcher_missing(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path)
    monkeypatch.setattr("sonara.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: False)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "not_installed"
    assert "slash sonara install" in cue.lower()


def test_setup_health_ok_speech_only_no_hotkeyd(tmp_path, monkeypatch):
    # install.json + launcher present, hotkeys not yet started, versions match.
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.4.0")
    monkeypatch.setattr("sonara.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "ok"
    assert cue is None


def test_setup_health_ok_when_versions_match(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.4.0")
    monkeypatch.setattr("sonara.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "ok"
    assert cue is None


def test_setup_health_version_drift(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.3.0")
    monkeypatch.setattr("sonara.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "version_drift"
    assert "updated" in cue.lower()
    assert "slash sonara install" in cue.lower()


def test_setup_health_no_drift_when_session_version_empty(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.3.0")
    monkeypatch.setattr("sonara.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("")  # unknown session version
    assert state == "ok"
    assert cue is None


def test_read_install_record_returns_none_on_corrupt(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = tmp_path / "install.json"
    rec.write_text("{ not json")
    monkeypatch.setattr("sonara.daemon.INSTALL_RECORD_PATH", str(rec))
    assert daemon._read_install_record() is None


from sonara.protocol import MsgType, PROTOCOL_VERSION


def _ss(session, plugin_version=""):
    return {"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START,
            "session": session, "plugin_version": plugin_version}


def _se(session):
    return {"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END, "session": session}


def test_session_start_enqueues_one_cue_when_not_installed(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    monkeypatch.setattr(daemon, "_setup_health",
                        lambda v: ("not_installed", "RUN slash sonara install"))
    daemon.handle_message(_ss("s1"))
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "prose"
    assert "slash sonara install" in item.text.lower()


def test_session_start_silent_when_ok(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    monkeypatch.setattr(daemon, "_setup_health", lambda v: ("ok", None))
    daemon.handle_message(_ss("s1"))
    assert len(queue) == 0


def test_session_start_cue_throttled_per_session(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    monkeypatch.setattr(daemon, "_setup_health",
                        lambda v: ("not_installed", "RUN slash sonara install"))
    daemon.handle_message(_ss("s1"))
    daemon.handle_message(_ss("s1"))  # same session again
    assert len(queue) == 1  # only ONE cue


def test_session_end_clears_throttle_so_cue_can_fire_again(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    monkeypatch.setattr(daemon, "_setup_health",
                        lambda v: ("not_installed", "RUN slash sonara install"))
    daemon.handle_message(_ss("s1"))
    assert len(queue) == 1
    queue.pop_next()
    daemon.handle_message(_se("s1"))
    daemon.handle_message(_ss("s1"))  # new session lifecycle, same id
    assert len(queue) == 1


def test_setup_health_exception_never_breaks_session(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    def _boom(v):
        raise RuntimeError("health blew up")
    monkeypatch.setattr(daemon, "_setup_health", _boom)
    # Must not raise; just no cue.
    daemon.handle_message(_ss("s1"))
    assert len(queue) == 0
