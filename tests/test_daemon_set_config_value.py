from tests.daemon_helpers import make_daemon


def test_set_config_value_clamps_and_persists(monkeypatch):
    import sonara.daemon as daemon_module
    saved = []
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: saved.append(dict(cfg)))
    daemon, *_ = make_daemon()
    assert daemon.set_config_value("summary_settle_ms", 99999) is True
    assert daemon.config["summary_settle_ms"] == 5000          # clamped
    assert daemon.set_config_value("not_a_key", 1) is False
    assert saved                                               # persisted


def test_set_config_value_rejects_removed_chatterbox_settings(monkeypatch):
    # Chatterbox was removed (#134): a stale settings page that still sends
    # one of its settings is refused, never written back into config.
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, *_ = make_daemon()
    for key, value in (("chatterbox_max_chunk_chars", 160),
                       ("chatterbox_exaggeration", 0.5),
                       ("chatterbox_variant", "original")):
        assert daemon.set_config_value(key, value) is False
        assert key not in daemon.config
