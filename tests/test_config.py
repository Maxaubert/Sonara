from echo import config
from echo.config import DEFAULTS


def test_defaults_has_documented_top_level_keys():
    assert set(DEFAULTS.keys()) == {
        "voice",
        "rate",
        "verbosity",
        "background_policy",
        "earcons",
    }


def test_defaults_scalar_values():
    assert DEFAULTS["voice"] is None
    assert DEFAULTS["rate"] == 200
    assert DEFAULTS["verbosity"] == "everything"
    assert DEFAULTS["background_policy"] == "earcon_only"


def test_defaults_earcon_map_exact():
    assert DEFAULTS["earcons"] == {
        "permission": "/System/Library/Sounds/Funk.aiff",
        "choice": "/System/Library/Sounds/Ping.aiff",
        "plan": "/System/Library/Sounds/Submarine.aiff",
        "error": "/System/Library/Sounds/Sosumi.aiff",
        "turn_done": "/System/Library/Sounds/Tink.aiff",
        "ready": "/System/Library/Sounds/Glass.aiff",
    }


def test_defaults_earcon_kinds_match_contract():
    assert set(DEFAULTS["earcons"].keys()) == {
        "permission",
        "choice",
        "plan",
        "error",
        "turn_done",
        "ready",
    }


def test_module_exposes_load_and_save():
    assert callable(config.load_config)
    assert callable(config.save_config)


import copy


def _patch_config_paths(monkeypatch, tmp_path):
    cfg_path = tmp_path / "config.json"
    monkeypatch.setattr(config, "ECHO_DIR", tmp_path)
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
    loaded["earcons"]["choice"] = "/tmp/hacked.aiff"
    assert DEFAULTS == pristine
    assert DEFAULTS["rate"] == 200
    assert DEFAULTS["earcons"]["choice"] == "/System/Library/Sounds/Ping.aiff"


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
    # nested earcons: overridden key replaced, all others preserved
    assert loaded["earcons"]["choice"] == "/custom/choice.aiff"
    assert loaded["earcons"]["permission"] == "/System/Library/Sounds/Funk.aiff"
    assert loaded["earcons"]["plan"] == "/System/Library/Sounds/Submarine.aiff"
    assert loaded["earcons"]["error"] == "/System/Library/Sounds/Sosumi.aiff"
    assert loaded["earcons"]["turn_done"] == "/System/Library/Sounds/Tink.aiff"
    assert loaded["earcons"]["ready"] == "/System/Library/Sounds/Glass.aiff"


def test_load_config_merges_extra_nested_key(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text(
        _json.dumps({"earcons": {"custom_kind": "/custom/extra.aiff"}}),
        encoding="utf-8",
    )
    loaded = config.load_config()
    assert loaded["earcons"]["custom_kind"] == "/custom/extra.aiff"
    # all six defaults still present
    assert loaded["earcons"]["permission"] == "/System/Library/Sounds/Funk.aiff"
    assert len(loaded["earcons"]) == 7


def test_load_config_merge_does_not_mutate_defaults(monkeypatch, tmp_path):
    cfg_path = _patch_config_paths(monkeypatch, tmp_path)
    cfg_path.write_text(
        _json.dumps({"earcons": {"choice": "/custom/choice.aiff"}}),
        encoding="utf-8",
    )
    config.load_config()
    assert DEFAULTS["earcons"]["choice"] == "/System/Library/Sounds/Ping.aiff"


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
    loaded["earcons"]["plan"] = "/tmp/x.aiff"
    assert DEFAULTS["earcons"]["plan"] == "/System/Library/Sounds/Submarine.aiff"


def _patch_config_paths_nested(monkeypatch, tmp_path):
    echo_dir = tmp_path / ".echo"
    cfg_path = echo_dir / "config.json"
    monkeypatch.setattr(config, "ECHO_DIR", echo_dir)
    monkeypatch.setattr(config, "CONFIG_PATH", cfg_path)
    monkeypatch.setattr(
        config,
        "ensure_echo_dir",
        lambda: echo_dir.mkdir(parents=True, exist_ok=True),
    )
    return echo_dir, cfg_path


def test_save_config_creates_dir_and_round_trips(monkeypatch, tmp_path):
    echo_dir, cfg_path = _patch_config_paths_nested(monkeypatch, tmp_path)
    assert not echo_dir.exists()

    cfg = config.load_config()
    cfg["rate"] = 175
    cfg["voice"] = "Zoe (Premium)"
    cfg["verbosity"] = "medium"
    cfg["earcons"]["choice"] = "/custom/choice.aiff"
    config.save_config(cfg)

    assert echo_dir.exists()
    assert cfg_path.exists()
    # no temp artifact left behind after os.replace
    leftovers = list(echo_dir.glob("*.tmp"))
    assert leftovers == []

    reloaded = config.load_config()
    assert reloaded["rate"] == 175
    assert reloaded["voice"] == "Zoe (Premium)"
    assert reloaded["verbosity"] == "medium"
    assert reloaded["earcons"]["choice"] == "/custom/choice.aiff"
    # untouched defaults survive the round-trip
    assert reloaded["earcons"]["permission"] == "/System/Library/Sounds/Funk.aiff"
    assert reloaded["background_policy"] == "earcon_only"


def test_save_config_writes_valid_json_on_disk(monkeypatch, tmp_path):
    echo_dir, cfg_path = _patch_config_paths_nested(monkeypatch, tmp_path)
    cfg = config.load_config()
    cfg["rate"] = 123
    config.save_config(cfg)
    on_disk = _json.loads(cfg_path.read_text(encoding="utf-8"))
    assert on_disk == cfg


def test_save_config_is_atomic_on_replace_failure(monkeypatch, tmp_path):
    echo_dir, cfg_path = _patch_config_paths_nested(monkeypatch, tmp_path)
    echo_dir.mkdir(parents=True, exist_ok=True)
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
