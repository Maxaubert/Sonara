import os
from unittest import mock

from tests.daemon_helpers import make_daemon


def _write_install_json(tmp_path, plugin_version="0.4.0"):
    import sonari.daemon as daemon_mod
    rec = tmp_path / "install.json"
    import json
    rec.write_text(json.dumps({"plugin_version": plugin_version}))
    return rec


def test_setup_health_not_installed_when_no_record(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    missing = tmp_path / "install.json"  # never created
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(missing))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "not_installed"
    assert "slash sonari install" in cue.lower()


def test_setup_health_not_installed_when_launcher_missing(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path)
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: False)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "not_installed"
    assert "slash sonari install" in cue.lower()


def test_setup_health_ok_speech_only_no_hotkeyd(tmp_path, monkeypatch):
    # install.json + launcher present, hotkeyd binary ABSENT, versions match.
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.4.0")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr("sonari.daemon.HOTKEYD_BIN_PATH",
                        str(tmp_path / "nope" / "sonari-hotkeyd"), raising=False)
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "ok"
    assert cue is None


def test_setup_health_ok_when_versions_match(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.4.0")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "ok"
    assert cue is None


def test_setup_health_version_drift(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.3.0")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("0.4.0")
    assert state == "version_drift"
    assert "updated" in cue.lower()
    assert "slash sonari install" in cue.lower()


def test_setup_health_no_drift_when_session_version_empty(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = _write_install_json(tmp_path, plugin_version="0.3.0")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    monkeypatch.setattr(daemon, "_launcher_present", lambda: True)
    state, cue = daemon._setup_health("")  # unknown session version
    assert state == "ok"
    assert cue is None


def test_read_install_record_returns_none_on_corrupt(tmp_path, monkeypatch):
    daemon, *_ = make_daemon()
    rec = tmp_path / "install.json"
    rec.write_text("{ not json")
    monkeypatch.setattr("sonari.daemon.INSTALL_RECORD_PATH", str(rec))
    assert daemon._read_install_record() is None
