import json

import pytest

from sonara import keymap
import sonara.platform as platform


def _force(monkeypatch, plat):
    monkeypatch.setattr(platform.sys, "platform", plat)
    platform._CACHE = None


@pytest.fixture
def win(monkeypatch):
    _force(monkeypatch, "win32")
    yield
    platform._CACHE = None


def _patch_keymap_paths(monkeypatch, tmp_path):
    km = tmp_path / "keymap.json"
    resolved = tmp_path / "hotkeyd.resolved.json"
    monkeypatch.setattr(keymap, "KEYMAP_PATH", km)
    monkeypatch.setattr(keymap, "HOTKEYD_RESOLVED_PATH", resolved)
    monkeypatch.setattr(keymap, "SONARA_DIR", tmp_path)
    monkeypatch.setattr(keymap, "ensure_sonara_dir",
                        lambda: tmp_path.mkdir(parents=True, exist_ok=True))
    return km, resolved


# --- keytables come from the active platform backend ------------------------

def test_windows_keytables_via_backend(win):
    kc, mm = keymap._keytables()
    assert kc["s"] == 0x53 and kc["."] == 0xBE
    assert mm["ctrl"] == 0x0002 and mm["shift"] == 0x0004 and mm["alt"] == 0x0001


def test_action_messages_faster_has_delta_25():
    assert keymap.ACTION_MESSAGES["faster"] == {"type": "set_rate", "delta": 25}
    assert keymap.ACTION_MESSAGES["slower"] == {"type": "set_rate", "delta": -25}


# --- default_keymap: per-OS chord -------------------------------------------

def test_default_keymap_windows_uses_ctrl_shift_alt(win):
    d = keymap.default_keymap()
    assert d["nav_next"]["mods"] == ["ctrl", "shift", "alt"]
    assert d["mute"]["key"] == "m"


# --- resolve_keymap ---------------------------------------------------------

def test_resolve_windows_vk_codes(win):
    resolved = keymap.resolve_keymap(
        {"pause": {"key": "p", "mods": ["ctrl", "shift", "alt"]}})
    row = resolved[0]
    assert row["keyCode"] == 0x50                            # VK 'P'
    assert row["modifiers"] == (0x0002 | 0x0004 | 0x0001)    # ctrl|shift|alt
    assert row["action"] == "pause"


def test_default_keymap_binds_only_nav_pause_mute():
    # The default keymap binds nav/mute/next_session. pause/faster/slower are valid
    # actions but ship UNBOUND (blank by default); every default binding is a real action.
    km = keymap.default_keymap()
    assert set(km.keys()) == {"nav_prev", "nav_next", "mute", "next_session"}
    assert set(km.keys()) <= set(keymap.ACTION_MESSAGES.keys())
    assert "pause" in keymap.ACTION_MESSAGES and "pause" not in km
    assert "faster" in keymap.ACTION_MESSAGES and "faster" not in km
    assert "slower" in keymap.ACTION_MESSAGES and "slower" not in km


def test_default_keymap_binds_nav_mute():
    """Regression: nav_next/prev, mute were defined in ACTION_MESSAGES but absent
    from _DEFAULT_KEYS, so no hotkey was ever registered for them on a default
    install. (pause is intentionally UNBOUND now.)"""
    km = keymap.default_keymap()
    for action in ("nav_next", "nav_prev", "mute", "next_session"):
        assert action in km, f"{action} has no default binding"
        assert km[action]["key"], f"{action} default binding has no key"


def test_resolve_unknown_key_raises():
    with pytest.raises(ValueError):
        keymap.resolve_keymap({"pause": {"key": "zzz", "mods": ["ctrl"]}})


def test_resolve_unknown_mod_raises():
    with pytest.raises(ValueError):
        keymap.resolve_keymap({"pause": {"key": "p", "mods": ["hyper"]}})


def test_resolve_unknown_action_raises():
    with pytest.raises(ValueError):
        keymap.resolve_keymap({"frobnicate": {"key": "s", "mods": ["ctrl"]}})


def test_resolve_skips_unbound_entries():
    # An entry with no key is UNBOUND -> skipped (not an error), so an action with
    # a default binding can be explicitly cleared in keymap.json.
    # 'ctrl' is valid on both macOS and Windows keytables (the modifier is
    # incidental here — the point is that the keyless 'pause' entry is skipped).
    resolved = keymap.resolve_keymap({"pause": {"key": None, "mods": ["ctrl"]},
                                      "mute": {"key": "m", "mods": ["ctrl"]}})
    actions = {e["action"] for e in resolved}
    assert "pause" not in actions and "mute" in actions


def test_unbind_action_default_writes_unbound_override(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    keymap.unbind_action("nav_next")             # nav_next HAS a default binding
    user = json.loads(km.read_text(encoding="utf-8"))
    assert user["nav_next"]["key"] is None       # explicit unbound override
    resolved = keymap.resolve_keymap(keymap.load_keymap())
    assert "nav_next" not in {e["action"] for e in resolved}


def test_unbind_action_non_default_just_drops(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({"faster": {"key": "]", "mods": ["alt"]}}), encoding="utf-8")
    keymap.unbind_action("faster")               # no default -> remove the binding
    assert "faster" not in json.loads(km.read_text(encoding="utf-8"))


def test_unbind_unknown_action_raises():
    with pytest.raises(ValueError):
        keymap.unbind_action("bogus")


# --- load_keymap ------------------------------------------------------------

def test_load_keymap_returns_defaults_when_missing(monkeypatch, tmp_path):
    _patch_keymap_paths(monkeypatch, tmp_path)
    loaded = keymap.load_keymap()
    assert loaded == keymap.default_keymap()
    loaded["nav_prev"]["key"] = "x"  # independent copy
    assert keymap.default_keymap()["nav_prev"]["key"] == "left"


def test_load_keymap_merges_user_override(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({"pause": {"key": "x", "mods": ["cmd"]}}), encoding="utf-8")
    loaded = keymap.load_keymap()
    assert loaded["pause"] == {"key": "x", "mods": ["cmd"]}
    assert loaded["nav_next"] == keymap.default_keymap()["nav_next"]


def test_load_keymap_drops_unknown_actions(monkeypatch, tmp_path):
    # A stale keymap.json binding a since-removed action must be ignored, not break
    # the whole keymap (resolve_keymap would otherwise raise on the unknown action).
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({"stop": {"key": "s", "mods": ["ctrl"]},
                              "pause": {"key": "p", "mods": ["ctrl"]}}), encoding="utf-8")
    loaded = keymap.load_keymap()
    assert "stop" not in loaded
    assert loaded["pause"] == {"key": "p", "mods": ["ctrl"]}
    keymap.resolve_keymap(loaded)   # must not raise


def test_load_keymap_tolerates_corrupt_file(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text("{ not json", encoding="utf-8")
    assert keymap.load_keymap() == keymap.default_keymap()


def test_write_default_keymap_if_absent_writes_once(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    assert not km.exists()
    assert keymap.write_default_keymap_if_absent() is True
    assert km.exists()
    assert json.loads(km.read_text(encoding="utf-8")) == keymap.default_keymap()
    assert keymap.write_default_keymap_if_absent() is False


# --- write_resolved ---------------------------------------------------------

def test_write_resolved_emits_array_of_bindings(monkeypatch, tmp_path):
    _patch_keymap_paths(monkeypatch, tmp_path)
    keymap.write_resolved()
    data = json.loads((tmp_path / "hotkeyd.resolved.json").read_text(encoding="utf-8"))
    assert isinstance(data, list) and len(data) == len(keymap._DEFAULT_KEYS)
    for entry in data:
        assert isinstance(entry["keyCode"], int)
        assert isinstance(entry["modifiers"], int)
        assert isinstance(entry["message"], str)


def test_write_resolved_no_tmp_leftover(monkeypatch, tmp_path):
    _patch_keymap_paths(monkeypatch, tmp_path)
    keymap.write_resolved()
    assert list(tmp_path.glob("*.tmp")) == []


def test_resolve_nav_action_message(win):
    resolved = keymap.resolve_keymap({"nav_next": {"key": "right", "mods": ["alt"]}})
    assert resolved[0]["action"] == "nav_next"
    assert json.loads(resolved[0]["message"]) == {"type": "nav", "to": "next"}


def test_no_two_default_actions_share_a_key():
    # Default bindings share one chord, so each must use a distinct key — else
    # resolve_keymap emits two entries for the same keyCode and one silently loses.
    from sonara.keymap import default_keymap
    keys = [b["key"] for b in default_keymap().values()]
    assert len(keys) == len(set(keys))


def test_next_session_action_message():
    from sonara.keymap import ACTION_MESSAGES
    assert ACTION_MESSAGES["next_session"] == {"type": "next_session"}


def test_next_session_default_binding_is_p():
    from sonara.keymap import default_keymap
    km = default_keymap()
    assert km["next_session"]["key"] == "p"
