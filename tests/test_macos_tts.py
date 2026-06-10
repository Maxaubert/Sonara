from sonari.platform.macos import tts as mod
from sonari.platform.macos.tts import MacTtsBackend, _parse_listing


def test_run_builds_say_command_with_voice_and_rate(monkeypatch):
    calls = {}
    class _P:  # fake Popen
        def __init__(self, cmd): calls["cmd"] = cmd
    monkeypatch.setattr(mod.subprocess, "Popen", _P)
    MacTtsBackend().run("Hi", "Ava", 220)
    assert calls["cmd"] == ["say", "-v", "Ava", "-r", "220", "Hi"]


def test_best_voice_prefers_premium_en(monkeypatch):
    listing = "Ava (Premium)   en_US  # hi\nDaniel          en_GB  # hi\n"
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    assert MacTtsBackend().best_voice() == "Ava"


def test_best_voice_falls_back_when_say_errors(monkeypatch):
    def boom(*a, **k): raise FileNotFoundError()
    monkeypatch.setattr(mod.subprocess, "check_output", boom)
    assert MacTtsBackend().best_voice() == "Samantha"


# ---------------------------------------------------------------------------
# best_voice: no-premium fallback — preference list (Allison > Samantha)
# ---------------------------------------------------------------------------

def test_best_voice_prefers_allison_over_samantha_when_no_premium(monkeypatch):
    """Without any Premium/Enhanced voice, Allison must win over Samantha."""
    listing = (
        "Allison         en_US  # I sure like being inside this fancy computer\n"
        "Samantha        en_US  # I sure like being inside this fancy computer\n"
        "Daniel          en_GB  # I sure like being inside this fancy computer\n"
    )
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    assert MacTtsBackend().best_voice() == "Allison"


def test_best_voice_uses_samantha_fallback_when_allison_absent(monkeypatch):
    """When neither Premium nor Allison is present, Samantha is the fallback."""
    listing = (
        "Samantha        en_US  # I sure like being inside this fancy computer\n"
        "Fred            en_US  # I sure like being inside this fancy computer\n"
    )
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    assert MacTtsBackend().best_voice() == "Samantha"


def test_best_voice_uses_hardcoded_samantha_when_no_en_voice(monkeypatch):
    """Only non-English voices available — must return the hard-coded 'Samantha'."""
    listing = (
        "Amelie          fr_CA  # Je m'appelle Amelie\n"
        "Anna            de_DE  # Hallo, ich bin Anna\n"
    )
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    assert MacTtsBackend().best_voice() == "Samantha"


# ---------------------------------------------------------------------------
# best_voice: Premium voice in non-English locale must be excluded
# ---------------------------------------------------------------------------

def test_best_voice_excludes_premium_non_english(monkeypatch):
    """A (Premium) voice in a non-English locale must NOT be chosen."""
    listing = (
        "Amelie (Premium)   fr_CA  # Je m'appelle Amelie\n"
        "Samantha           en_US  # I sure like being inside this fancy computer\n"
    )
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    # Amelie is Premium but French — must be skipped; Samantha is the best English.
    assert MacTtsBackend().best_voice() == "Samantha"


def test_best_voice_excludes_enhanced_non_english(monkeypatch):
    """An (Enhanced) voice in a non-English locale must NOT be chosen."""
    listing = (
        "Thomas (Enhanced)  fr_FR  # Je m'appelle Thomas\n"
        "Allison            en_US  # I sure like being inside this fancy computer\n"
    )
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    assert MacTtsBackend().best_voice() == "Allison"


# ---------------------------------------------------------------------------
# list_voices tests
# ---------------------------------------------------------------------------

def test_list_voices_returns_all_voice_names(monkeypatch):
    """list_voices must return every voice's bare name regardless of locale."""
    listing = (
        "Ava (Premium)   en_US  # hello\n"
        "Daniel          en_GB  # hello\n"
        "Amelie          fr_CA  # bonjour\n"
    )
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    result = MacTtsBackend().list_voices()
    assert result == ["Ava", "Daniel", "Amelie"]


def test_list_voices_returns_empty_list_when_say_errors(monkeypatch):
    """When `say -v ?` fails, list_voices must return an empty list."""
    def boom(*a, **k): raise FileNotFoundError()
    monkeypatch.setattr(mod.subprocess, "check_output", boom)
    assert MacTtsBackend().list_voices() == []


def test_list_voices_strips_premium_qualifier(monkeypatch):
    """Bare names in list_voices must not contain '(Premium)' or '(Enhanced)'."""
    listing = (
        "Ava (Premium)      en_US  # hi\n"
        "Siri (Enhanced)    en_AU  # hi\n"
        "Fred               en_US  # hi\n"
    )
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    result = MacTtsBackend().list_voices()
    assert result == ["Ava", "Siri", "Fred"]
    for name in result:
        assert "(Premium)" not in name
        assert "(Enhanced)" not in name


def test_list_voices_skips_blank_and_malformed_lines(monkeypatch):
    """Blank lines and lines with only one token must be silently skipped."""
    listing = (
        "\n"
        "BadLine\n"
        "Ava (Premium)   en_US  # hi\n"
    )
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    assert MacTtsBackend().list_voices() == ["Ava"]


# ---------------------------------------------------------------------------
# _parse_listing unit tests (shared helper)
# ---------------------------------------------------------------------------

def test_parse_listing_basic():
    listing = "Ava (Premium)   en_US  # hello\nDaniel   en_GB  # hello\n"
    result = _parse_listing(listing)
    assert result == [("Ava", "en_US", True), ("Daniel", "en_GB", False)]


def test_parse_listing_empty_string():
    assert _parse_listing("") == []


def test_parse_listing_skips_blank_lines():
    listing = "\n\nSamantha   en_US  # hi\n\n"
    result = _parse_listing(listing)
    assert result == [("Samantha", "en_US", False)]
