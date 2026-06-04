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
