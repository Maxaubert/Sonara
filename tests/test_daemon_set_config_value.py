from tests.daemon_helpers import make_daemon


def test_set_config_value_clamps_and_persists(monkeypatch):
    import sonara.daemon as daemon_module
    saved = []
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: saved.append(dict(cfg)))
    daemon, *_ = make_daemon()
    assert daemon.set_config_value("summary_settle_ms", 99999) is True
    assert daemon.config["summary_settle_ms"] == 5000          # clamped
    assert daemon.set_config_value("chatterbox_max_chunk_chars", 10) is True
    assert daemon.config["chatterbox_max_chunk_chars"] == 80   # clamped
    assert daemon.set_config_value("not_a_key", 1) is False
    assert saved                                               # persisted


def test_set_config_value_clamps_exaggeration_as_float(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, *_ = make_daemon()
    assert daemon.set_config_value("chatterbox_exaggeration", 0.65) is True
    assert daemon.config["chatterbox_exaggeration"] == 0.65
    daemon.set_config_value("chatterbox_exaggeration", 5)
    assert daemon.config["chatterbox_exaggeration"] == 1.0   # clamped
    daemon.set_config_value("chatterbox_exaggeration", -1)
    assert daemon.config["chatterbox_exaggeration"] == 0.0
