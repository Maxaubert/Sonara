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
