"""config.json keeps only what the user chose (#136, audit M7/M8).

save_config used to dump the whole merged config, so every default was frozen
into the file on the first save and a later default change (duck_level 20 ->
30, summary_timeout 20 -> 60) never reached existing installs. The bundled
earcon paths were frozen the same way, so new earcon kinds never played.
"""
from __future__ import annotations

import json

import pytest

from sonara import config, config_schema


@pytest.fixture
def cfg_path(monkeypatch, tmp_path):
    sonara_dir = tmp_path / ".sonara"
    path = sonara_dir / "config.json"
    monkeypatch.setattr(config, "SONARA_DIR", sonara_dir)
    monkeypatch.setattr(config, "CONFIG_PATH", path)
    monkeypatch.setattr(config, "ensure_sonara_dir",
                        lambda: sonara_dir.mkdir(parents=True, exist_ok=True))
    return path


def _on_disk(path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _user_keys(path) -> dict:
    data = _on_disk(path)
    data.pop(config.FORMAT_KEY, None)
    return data


def test_defaults_come_from_the_schema():
    assert config.DEFAULTS == config_schema.defaults()


def test_summary_settle_ms_has_a_real_default():
    # DC10: a fresh install showed "null ms" on the settings page
    assert config.DEFAULTS["summary_settle_ms"] == 600


def test_save_persists_only_changed_keys(cfg_path):
    cfg = config.load_config()
    cfg["rate"] = 250
    config.save_config(cfg)
    assert _user_keys(cfg_path) == {"rate": 250}


def test_untouched_save_writes_no_settings(cfg_path):
    config.save_config(config.load_config())
    assert _user_keys(cfg_path) == {}


def test_explicit_set_to_the_default_value_is_kept(cfg_path):
    # The user chose this value: it stays pinned even if the default moves.
    cfg = config.load_config()
    cfg["duck_level"] = 30
    config.save_config(cfg)
    assert _user_keys(cfg_path) == {"duck_level": 30}
    reloaded = config.load_config()
    reloaded["rate"] = 220
    config.save_config(reloaded)
    assert _user_keys(cfg_path) == {"duck_level": 30, "rate": 220}


def test_new_default_reaches_a_config_saved_by_this_version(cfg_path, monkeypatch):
    cfg = config.load_config()
    cfg["rate"] = 250
    config.save_config(cfg)
    monkeypatch.setitem(config.DEFAULTS, "minqueue", 4)
    assert config.load_config()["minqueue"] == 4


def test_plain_dict_saves_only_values_that_differ(cfg_path):
    cfg = dict(config.DEFAULTS)
    cfg["volume"] = 150
    config.save_config(cfg)
    assert _user_keys(cfg_path) == {"volume": 150}


def _write_legacy_full_config(path, **overrides):
    """What save_config wrote before #136: every key, old defaults included."""
    old = dict(config.DEFAULTS)
    old.pop("summary_settle_ms")          # was never in DEFAULTS
    old.update({"duck_level": 20, "summary_timeout": 20})
    old.update(overrides)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(old), encoding="utf-8")


def test_legacy_full_config_picks_up_new_defaults(cfg_path):
    # Upgrade path: an old full dump with the OLD defaults the user never
    # changed. Values equal to a current or past default count as unset.
    _write_legacy_full_config(cfg_path)
    cfg = config.load_config()
    assert cfg["duck_level"] == 30
    assert cfg["summary_timeout"] == 60
    assert cfg["summary_settle_ms"] == 600
    config.save_config(cfg)
    assert _user_keys(cfg_path) == {}


def test_legacy_full_config_keeps_real_choices(cfg_path):
    _write_legacy_full_config(cfg_path, rate=260, duck_level=55,
                              voice="af_bella", summary_mode=True)
    cfg = config.load_config()
    assert (cfg["rate"], cfg["duck_level"]) == (260, 55)
    assert cfg["voice"] == "af_bella" and cfg["summary_mode"] is True
    config.save_config(cfg)
    assert _user_keys(cfg_path) == {"rate": 260, "duck_level": 55,
                                    "voice": "af_bella", "summary_mode": True}


def test_new_format_file_keeps_a_value_equal_to_a_legacy_default(cfg_path):
    # Only an unmarked (pre-#136) file gets the legacy treatment: a user who
    # sets duck_level 20 now keeps it.
    cfg = config.load_config()
    cfg["duck_level"] = 20
    config.save_config(cfg)
    assert config.load_config()["duck_level"] == 20


def test_load_prunes_unknown_keys(cfg_path):
    cfg_path.parent.mkdir(parents=True, exist_ok=True)
    cfg_path.write_text(json.dumps({"rate": 210, "retired_thing": 1}),
                        encoding="utf-8")
    cfg = config.load_config()
    assert "retired_thing" not in cfg
    config.save_config(cfg)
    assert _user_keys(cfg_path) == {"rate": 210}


def test_format_marker_is_not_a_setting(cfg_path):
    cfg = config.load_config()
    cfg["rate"] = 250
    config.save_config(cfg)
    assert config.FORMAT_KEY in _on_disk(cfg_path)
    assert config.FORMAT_KEY not in config.load_config()


# --- M8: earcon paths ---------------------------------------------------------

def test_bundled_earcon_paths_are_stripped_on_load(cfg_path, monkeypatch, tmp_path):
    app = tmp_path / ".sonara" / "app"
    monkeypatch.setattr(config, "APP_DIR", app)
    bundled = app / "sonara" / "platform" / "windows" / "earcons"
    custom = tmp_path / "my-sounds" / "ding.wav"
    cfg_path.parent.mkdir(parents=True, exist_ok=True)
    cfg_path.write_text(json.dumps({"earcons": {
        "choice": str(bundled / "choice.wav"),
        "turn_done": str(custom),
    }}), encoding="utf-8")
    cfg = config.load_config()
    assert cfg["earcons"] == {"turn_done": str(custom)}
    config.save_config(cfg)
    assert _user_keys(cfg_path) == {"earcons": {"turn_done": str(custom)}}


def test_bundled_earcon_paths_from_any_copy_are_stripped(cfg_path, monkeypatch, tmp_path):
    # The daemon may have run from the repo or the plugin cache, not the app dir.
    other = tmp_path / "plugins" / "cache" / "sonara" / "src" / "sonara" / \
        "platform" / "windows" / "earcons" / "plan.wav"
    cfg_path.parent.mkdir(parents=True, exist_ok=True)
    cfg_path.write_text(json.dumps({"earcons": {"plan": str(other)}}),
                        encoding="utf-8")
    cfg = config.load_config()
    assert "earcons" not in cfg


def test_earcon_paths_are_never_frozen_by_a_save(cfg_path):
    cfg = config.load_config()
    cfg["rate"] = 250
    config.save_config(cfg)
    assert "earcons" not in _on_disk(cfg_path)
