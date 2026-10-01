"""Characterisation of every existing setting's accept/clamp/reject rules (#136).

Written before the clamps moved into config_schema.py, against the public
paths (handle_message, set_config_value, the settings page key tables), so the
move provably keeps behaviour: the same inputs give the same stored values.
"""
from __future__ import annotations

import pytest

import sonara.daemon as daemon_module
from sonara import webui
from sonara.protocol import MsgType
from tests.daemon_helpers import make_daemon


@pytest.fixture
def daemon(monkeypatch):
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    d, *_ = make_daemon()
    return d


def _send(d, mtype, **fields):
    d.handle_message({"v": 1, "type": mtype, **fields})


# --- message-backed settings: (type, field, key, input, stored) -------------
# stored None = rejected, the previous value stays.
_MSG_CASES = [
    (MsgType.SET_RATE, "rate", "rate", 250, 250),
    (MsgType.SET_RATE, "rate", "rate", 50, 100),
    (MsgType.SET_RATE, "rate", "rate", 999, 400),
    (MsgType.SET_RATE, "rate", "rate", "310", 310),
    (MsgType.SET_RATE, "rate", "rate", "fast", None),
    (MsgType.SET_RATE, "rate", "rate", None, None),
    (MsgType.SET_MINQUEUE, "minqueue", "minqueue", 3, 3),
    (MsgType.SET_MINQUEUE, "minqueue", "minqueue", -4, 0),
    (MsgType.SET_MINQUEUE, "minqueue", "minqueue", 50, 10),
    (MsgType.SET_MINQUEUE, "minqueue", "minqueue", "x", None),
    (MsgType.SET_DUCK_LEVEL, "level", "duck_level", 45, 45),
    (MsgType.SET_DUCK_LEVEL, "level", "duck_level", -1, 0),
    (MsgType.SET_DUCK_LEVEL, "level", "duck_level", 101, 100),
    (MsgType.SET_DUCK_LEVEL, "level", "duck_level", None, None),
    (MsgType.SET_VOLUME, "volume", "volume", 150, 150),
    (MsgType.SET_VOLUME, "volume", "volume", 5, 25),
    (MsgType.SET_VOLUME, "volume", "volume", 500, 200),
    (MsgType.SET_VOLUME, "volume", "volume", "loud", None),
    (MsgType.SET_AUDIO_MODE, "mode", "audio_mode", "duck", "duck"),
    (MsgType.SET_AUDIO_MODE, "mode", "audio_mode", "pause", "pause"),
    (MsgType.SET_AUDIO_MODE, "mode", "audio_mode", "loud", None),
    (MsgType.SET_AUDIO_MODE, "mode", "audio_mode", None, None),
    (MsgType.SET_SUMMARY_MODE, "enabled", "summary_mode", True, True),
    (MsgType.SET_SUMMARY_MODE, "enabled", "summary_mode", 0, False),
    (MsgType.SET_VOICE, "voice", "voice", "af_bella", "af_bella"),
    (MsgType.SET_VOICE, "voice", "voice", None, None),
    (MsgType.SET_VERBOSITY, "verbosity", "verbosity", "quiet", "quiet"),
]


@pytest.mark.parametrize("mtype,field,key,value,stored", _MSG_CASES)
def test_message_setting_clamps(daemon, mtype, field, key, value, stored):
    before = daemon.config.get(key)
    _send(daemon, mtype, **{field: value})
    if stored is None and key != "voice":
        assert daemon.config.get(key) == before
    else:
        assert daemon.config.get(key) == stored


def test_rate_delta_clamps(daemon):
    _send(daemon, MsgType.SET_RATE, delta=1000)
    assert daemon.config["rate"] == 400
    _send(daemon, MsgType.SET_RATE, delta=-1000)
    assert daemon.config["rate"] == 100
    _send(daemon, MsgType.SET_RATE, delta="x")
    assert daemon.config["rate"] == 100


def test_summary_mode_without_enabled_is_ignored(daemon):
    daemon.config["summary_mode"] = True
    _send(daemon, MsgType.SET_SUMMARY_MODE)
    assert daemon.config["summary_mode"] is True


# --- config-only settings via set_config_value -------------------------------
_CONFIG_CASES = [
    ("summary_model", "  sonnet ", "sonnet"),
    ("summary_model", "   ", None),
    ("summary_timeout", 90, 90),
    ("summary_timeout", 1, 15),
    ("summary_timeout", 9999, 300),
    ("summary_timeout", "x", None),
    ("summary_settle_ms", 800, 800),
    ("summary_settle_ms", -5, 0),
    ("summary_settle_ms", 99999, 5000),
    ("summary_style", "brief", "brief"),
    ("summary_style", "long", None),
    ("summary_command", "codex", "codex"),
    ("summary_command", "bash", None),
    ("fast_cues", 0, False),
    ("fast_cues", "yes", True),
    ("cue_voice", " af_bella ", "af_bella"),
    ("cue_voice", "", None),
]


@pytest.mark.parametrize("key,value,stored", _CONFIG_CASES)
def test_config_only_setting_clamps(daemon, key, value, stored):
    before = daemon.config.get(key)
    ok = daemon.set_config_value(key, value)
    assert ok is (stored is not None)
    assert daemon.config.get(key) == (stored if ok else before)


@pytest.mark.parametrize("key", ["rate", "voice", "verbosity", "duck_level",
                                 "mute_level", "settings_port", "nope"])
def test_set_config_value_refuses_non_config_only_keys(daemon, key):
    assert daemon.set_config_value(key, 1) is False


# --- the settings page key tables --------------------------------------------
def test_page_key_tables():
    assert set(webui._PAGE_KEYS) == {
        "voice", "rate", "minqueue", "summary_mode", "summary_model",
        "summary_style", "summary_command", "summary_timeout",
        "summary_settle_ms", "duck_level", "audio_mode", "volume",
        "fast_cues", "cue_voice"}
    assert set(webui._MSG_KEYS) == {
        "voice", "rate", "minqueue", "summary_mode", "duck_level",
        "audio_mode", "volume"}
    assert set(webui._CONFIG_KEYS) == {
        "summary_model", "summary_style", "summary_command",
        "summary_timeout", "summary_settle_ms", "fast_cues", "cue_voice"}


@pytest.mark.parametrize("key,value,msg", [
    ("voice", 5, {"type": "set_voice", "voice": "5"}),
    ("rate", "220", {"type": "set_rate", "rate": 220}),
    ("minqueue", 2, {"type": "set_minqueue", "minqueue": 2}),
    ("summary_mode", 1, {"type": "set_summary_mode", "enabled": True}),
    ("duck_level", "40", {"type": "set_duck_level", "level": 40}),
    ("audio_mode", "duck", {"type": "set_audio_mode", "mode": "duck"}),
    ("volume", 120, {"type": "set_volume", "volume": 120}),
])
def test_page_message_builders(key, value, msg):
    assert webui._MSG_KEYS[key](value) == msg


def test_page_message_builder_rejects_non_numbers():
    with pytest.raises((TypeError, ValueError)):
        webui._MSG_KEYS["rate"]("fast")
