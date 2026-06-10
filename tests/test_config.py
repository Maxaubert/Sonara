from sonari import config
from sonari.config import DEFAULTS


def test_defaults_has_documented_top_level_keys():
    assert set(DEFAULTS.keys()) == {
        "voice",
        "rate",
        "verbosity",
        "background_policy",
        "history_cap",
    }


def test_defaults_scalar_values():
    assert DEFAULTS["voice"] is None
    assert DEFAULTS["rate"] == 200
    assert DEFAULTS["verbosity"] == "everything"
    assert DEFAULTS["background_policy"] == "earcon_only"


def test_defaults_no_longer_carries_earcons():
    # Earcon defaults now live in the platform backend (MacEarconBackend);
    # the daemon backfills them at startup, so config DEFAULTS must not own them.
    assert "earcons" not in DEFAULTS


def test_module_exposes_load_and_save():
    assert callable(config.load_config)
    assert callable(config.save_config)


import copy


def _patch_config_paths(monkeypatch, tmp_path):
    cfg_path = tmp_path / "config.json"
    monkeypatch.setattr(config, "SONARI_DIR", tmp_path)
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_path)
    return cfg_path


def test_load_config_returns_defaults_when_file_missing(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    assert not cfg_path.exists()
    loaded = config.load_config()
    assert loaded == DEFAULTS


def test_load_config_missing_returns_independent_copy(monkeypatch, tmp_path):
    _patch_config_paths(monkeypatch, tmp_path)
    pristine = copy.deepcopy(DEFAULTS)
    loaded = config.load_config()
    loaded["rate"] = 999
    assert DEFAULTS == pristine
    assert DEFAULTS["rate"] == 200


import json as _json


def test_load_config_deep_merges_partial_file(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text(
        _json.dumps(
            {
                "rate": 240,
                "voice": "Ava (Premium)",
                "earcons": {"choice": "/custom/choice.aiff"},
            }
        ),
        encoding="utf-8",
    )
    loaded = config.load_config()

    # overridden scalars
    assert loaded["rate"] == 240
    assert loaded["voice"] == "Ava (Premium)"
    # untouched scalars keep their defaults
    assert loaded["verbosity"] == "everything"
    assert loaded["background_policy"] == "earcon_only"
    # earcons are no longer a DEFAULTS key; a persisted block passes through verbatim
    assert loaded["earcons"] == {"choice": "/custom/choice.aiff"}


def test_load_config_deep_merges_nested_dict_key(monkeypatch, tmp_path):
    # _deep_merge recurses into nested dicts: persisted keys override, base keys survive.
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    monkeypatch.setattr(
        config,
        "DEFAULTS",
        {"voice": None, "rate": 200, "nested": {"a": 1, "b": 2}},
    )
    cfg_path.write_text(
        _json.dumps({"nested": {"b": 99, "c": 3}}),
        encoding="utf-8",
    )
    loaded = config.load_config()
    assert loaded["nested"] == {"a": 1, "b": 99, "c": 3}


def test_load_config_merge_does_not_mutate_defaults(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    monkeypatch.setattr(
        config,
        "DEFAULTS",
        {"voice": None, "rate": 200, "nested": {"choice": "/default.aiff"}},
    )
    cfg_path.write_text(
        _json.dumps({"nested": {"choice": "/custom/choice.aiff"}}),
        encoding="utf-8",
    )
    config.load_config()
    assert config.DEFAULTS["nested"]["choice"] == "/default.aiff"


def test_load_config_tolerates_non_json(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text("this is { not json ::: ", encoding="utf-8")
    loaded = config.load_config()
    assert loaded == DEFAULTS


def test_load_config_tolerates_empty_file(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text("", encoding="utf-8")
    loaded = config.load_config()
    assert loaded == DEFAULTS


def test_load_config_tolerates_json_non_object(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text("[1, 2, 3]", encoding="utf-8")
    loaded = config.load_config()
    assert loaded == DEFAULTS


def test_load_config_corrupt_returns_independent_copy(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text("garbage", encoding="utf-8")
    loaded = config.load_config()
    loaded["rate"] = 999
    assert DEFAULTS["rate"] == 200


def _patch_config_paths_nested(monkeypatch, tmp_path):
    sonari_dir = tmp_path / ".sonari"
    cfg_path = sonari_dir / "config.json"
    monkeypatch.setattr(config, "SONARI_DIR", sonari_dir)
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_path)
    monkeypatch.setattr(
        config,
        "ensure_sonari_dir",
        lambda: sonari_dir.mkdir(parents=True, exist_ok=True),
    )
    return sonari_dir, cfg_path


def test_save_config_creates_dir_and_round_trips(monkeypatch, tmp_path):
    sonari_dir, cfg_path = _patch_config_paths_nested(monkeypatch, tmp_path)
    assert not sonari_dir.exists()

    cfg = config.load_config()
    cfg["rate"] = 175
    cfg["voice"] = "Zoe (Premium)"
    cfg["verbosity"] = "medium"
    cfg["earcons"] = {"choice": "/custom/choice.aiff"}
    config.save_config(cfg)

    assert sonari_dir.exists()
    assert cfg_path.exists()
    # no temp artifact left behind after os.replace
    leftovers = list(sonari_dir.glob("*.tmp"))
    assert leftovers == []

    reloaded = config.load_config()
    assert reloaded["rate"] == 175
    assert reloaded["voice"] == "Zoe (Premium)"
    assert reloaded["verbosity"] == "medium"
    # a persisted (non-default) earcons block round-trips verbatim
    assert reloaded["earcons"]["choice"] == "/custom/choice.aiff"
    # untouched defaults survive the round-trip
    assert reloaded["background_policy"] == "earcon_only"


def test_save_config_writes_valid_json_on_disk(monkeypatch, tmp_path):
    sonari_dir, cfg_path = _patch_config_paths_nested(monkeypatch, tmp_path)
    cfg = config.load_config()
    cfg["rate"] = 123
    config.save_config(cfg)
    on_disk = _json.loads(cfg_path.read_text(encoding="utf-8"))
    assert on_disk == cfg


def test_save_config_is_atomic_on_replace_failure(monkeypatch, tmp_path):
    sonari_dir, cfg_path = _patch_config_paths_nested(monkeypatch, tmp_path)
    sonari_dir.mkdir(parents=True, exist_ok=True)
    cfg_path.write_text(_json.dumps({"rate": 200}), encoding="utf-8")

    def _boom(src, dst):
        raise OSError("simulated replace failure")

    monkeypatch.setattr(config.os, "replace", _boom)
    new_cfg = config.load_config()
    new_cfg["rate"] = 999

    try:
        config.save_config(new_cfg)
    except OSError:
        pass

    # original file content is untouched: os.replace never overwrote it
    on_disk = _json.loads(cfg_path.read_text(encoding="utf-8"))
    assert on_disk == {"rate": 200}
