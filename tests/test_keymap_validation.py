"""Page-audit critical (#38): hotkey capture could persist key names the
resolver rejects, and one bad entry disables ALL hotkeys persistently.
bind_action now validates against the platform keytables, and the keytable
covers every letter and digit so ordinary captures are actually bindable."""
import pytest

from sonara import keymap


@pytest.fixture(autouse=True)
def _tmp_keymap(tmp_path, monkeypatch):
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    monkeypatch.setattr(keymap, "ensure_sonara_dir", lambda: None)


def test_bind_action_rejects_unknown_key():
    with pytest.raises(ValueError, match="key"):
        keymap.bind_action("mute", "escape", ["ctrl", "alt"])
    assert not keymap.KEYMAP_PATH.exists()          # nothing persisted


def test_bind_action_rejects_unknown_modifier():
    with pytest.raises(ValueError, match="modifier"):
        keymap.bind_action("mute", "m", ["hyper"])


def test_bind_action_accepts_any_letter_and_digit():
    # the keytable used to cover only 9 letters: capturing 'z' bricked hotkeys
    keymap.bind_action("mute", "z", ["ctrl", "alt"])
    keymap.bind_action("pause", "7", ["ctrl", "alt"])
    km = keymap.load_keymap()
    assert km["mute"]["key"] == "z"
    assert km["pause"]["key"] == "7"
    # and the resolver actually accepts what bind_action persisted
    resolved = keymap.resolve_keymap(km)
    assert any(e["action"] == "mute" for e in resolved)


def test_full_letter_digit_coverage_in_keytable():
    from sonara.platform.windows import keytables
    import string
    for c in string.ascii_lowercase + string.digits:
        assert c in keytables.KEY_CODES, f"missing key {c!r}"
