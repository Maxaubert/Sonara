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
    monkeypatch.setattr(keymap, "KEYMAP_PATH", km)
    monkeypatch.setattr(keymap, "ensure_sonara_dir",
                        lambda: tmp_path.mkdir(parents=True, exist_ok=True))
    return km, None


# --- keytables come from the active platform backend ------------------------

def test_windows_keytables_via_backend(win):
    kc, mm = keymap._keytables()
    assert kc["s"] == 0x53 and kc["."] == 0xBE
    assert mm["ctrl"] == 0x0002 and mm["shift"] == 0x0004 and mm["alt"] == 0x0001


def test_action_messages_faster_has_delta_25():
    assert keymap.ACTION_MESSAGES["faster"] == {"type": "set_rate", "delta": 25}
    assert keymap.ACTION_MESSAGES["slower"] == {"type": "set_rate", "delta": -25}


# --- default_keymap: per-OS chord -------------------------------------------

def test_default_keymap_windows_uses_win_alt(win):
    d = keymap.default_keymap()
    assert all(b["mods"] == ["win", "alt"] for b in d.values())
    assert d["mute"]["key"] == "s"


def test_default_bindings_avoid_windows_win_alt_shortcuts(win):
    # Windows 11 owns Win+Alt+Up/Down (snap top/bottom half), Win+Alt+M (Game
    # Bar microphone), Win+Alt+B/D/G/K/R/T/PrtScn and Win+Alt+digits; on the
    # maintainer's PC RegisterHotKey also refused Win+Alt+Left/Right/P/F/W/Y
    # (#160). The defaults must stay clear of all of them.
    taken = set("bdfgkmprtwy0123456789") | {"up", "down", "left", "right"}
    keys = {b["key"] for b in keymap.default_keymap().values()}
    assert not keys & taken


# --- resolve_keymap ---------------------------------------------------------

def test_resolve_windows_vk_codes(win):
    resolved = keymap.resolve_keymap(
        {"pause": {"key": "p", "mods": ["ctrl", "shift", "alt"]}})
    row = resolved[0]
    assert row["keyCode"] == 0x50                            # VK 'P'
    assert row["modifiers"] == (0x0002 | 0x0004 | 0x0001)    # ctrl|shift|alt
    assert row["action"] == "pause"


def test_default_keymap_binds_only_up_flush_mute_next_session():
    # The default keymap binds Up/flush/mute/next_session. pause/faster/slower are
    # valid actions but ship UNBOUND (blank by default); every default binding is
    # a real action.
    km = keymap.default_keymap()
    assert set(km.keys()) == {"nav_start", "flush", "mute", "next_session"}
    assert set(km.keys()) <= set(keymap.ACTION_MESSAGES.keys())
    assert "pause" in keymap.ACTION_MESSAGES and "pause" not in km
    assert "faster" in keymap.ACTION_MESSAGES and "faster" not in km
    assert "slower" in keymap.ACTION_MESSAGES and "slower" not in km


def test_default_keymap_binds_nav_mute():
    """Regression: actions defined in ACTION_MESSAGES but absent from
    _DEFAULT_KEYS never got a hotkey on a default install. (pause is
    intentionally UNBOUND now.)"""
    km = keymap.default_keymap()
    for action in ("nav_start", "flush", "mute", "next_session"):
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
    # 'ctrl' is a valid modifier in the keytables (the modifier is
    # incidental here -- the point is that the keyless 'pause' entry is skipped).
    resolved = keymap.resolve_keymap({"pause": {"key": None, "mods": ["ctrl"]},
                                      "mute": {"key": "m", "mods": ["ctrl"]}})
    actions = {e["action"] for e in resolved}
    assert "pause" not in actions and "mute" in actions


def test_unbind_action_default_writes_unbound_override(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    keymap.unbind_action("nav_start")            # nav_start HAS a default binding
    user = json.loads(km.read_text(encoding="utf-8"))
    assert user["nav_start"]["key"] is None      # explicit unbound override
    resolved = keymap.resolve_keymap(keymap.load_keymap())
    assert "nav_start" not in {e["action"] for e in resolved}


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
    loaded["nav_start"]["key"] = "x"  # independent copy
    assert keymap.default_keymap()["nav_start"]["key"] == "home"


def test_load_keymap_merges_user_override(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({"pause": {"key": "x", "mods": ["cmd"]}}), encoding="utf-8")
    loaded = keymap.load_keymap()
    assert loaded["pause"] == {"key": "x", "mods": ["cmd"]}
    assert loaded["nav_start"] == keymap.default_keymap()["nav_start"]


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


def test_removed_paragraph_nav_actions_are_gone():
    # D1: Ctrl+Alt+Left/Right paragraph stepping was removed (one message,
    # always the last). Restart (nav_start) is the only nav action.
    for action in ("nav_prev", "nav_next"):
        assert action not in keymap.ACTION_MESSAGES
        assert action not in keymap.default_keymap()
    nav_targets = {m.get("to") for m in keymap.ACTION_MESSAGES.values()
                   if m["type"] == "nav"}
    assert nav_targets == {"first"}


def test_stale_nav_prev_next_in_keymap_json_are_ignored(monkeypatch, tmp_path):
    # Existing installs have nav_prev/nav_next in ~/.sonara/keymap.json (written
    # by an older install). Loading must ignore them without error and keep the
    # remaining bindings.
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({
        "nav_prev": {"key": "left", "mods": ["ctrl", "alt"]},
        "nav_next": {"key": "right", "mods": ["ctrl", "alt"]},
        "nav_start": {"key": "up", "mods": ["ctrl", "alt"]},
        "mute": {"key": "j", "mods": ["ctrl", "alt"]},
    }), encoding="utf-8")
    loaded = keymap.load_keymap()
    assert "nav_prev" not in loaded and "nav_next" not in loaded
    assert loaded["mute"] == {"key": "j", "mods": ["ctrl", "alt"]}
    resolved = keymap.resolve_keymap(loaded)       # must not raise
    assert {e["action"] for e in resolved} == {"nav_start", "flush", "mute",
                                               "next_session"}


# --- resolve the default keymap ---------------------------------------------

def test_resolve_default_keymap_emits_one_entry_per_default_binding():
    data = keymap.resolve_keymap(keymap.load_keymap())
    assert isinstance(data, list) and len(data) == len(keymap._DEFAULT_KEYS)
    for entry in data:
        assert isinstance(entry["keyCode"], int)
        assert isinstance(entry["modifiers"], int)
        assert isinstance(entry["message"], str)


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


def test_resolve_nav_action_message(win):
    resolved = keymap.resolve_keymap({"nav_start": {"key": "up", "mods": ["alt"]}})
    assert resolved[0]["action"] == "nav_start"
    assert json.loads(resolved[0]["message"]) == {"type": "nav", "to": "first"}


def test_no_two_default_actions_share_a_key():
    # Default bindings share one chord, so each must use a distinct key -- else
    # resolve_keymap emits two entries for the same keyCode and one silently loses.
    from sonara.keymap import default_keymap
    keys = [b["key"] for b in default_keymap().values()]
    assert len(keys) == len(set(keys))


def test_next_session_action_message():
    from sonara.keymap import ACTION_MESSAGES
    assert ACTION_MESSAGES["next_session"] == {"type": "next_session"}


def test_next_session_default_binding_is_n():
    from sonara.keymap import default_keymap
    km = default_keymap()
    assert km["next_session"]["key"] == "n"


def test_nav_start_action_message_is_nav_first():
    assert keymap.ACTION_MESSAGES["nav_start"] == {"type": "nav", "to": "first"}


def test_flush_action_message():
    assert keymap.ACTION_MESSAGES["flush"] == {"type": "flush_session"}


def test_nav_start_and_flush_default_to_home_and_end():
    # Win+Alt+Up/Down snap windows in Windows 11, so restart/flush moved to
    # the closest free pair: Home (back to the start) and End (to the end).
    km = keymap.default_keymap()
    assert km["nav_start"]["key"] == "home"
    assert km["flush"]["key"] == "end"


def test_existing_keymap_file_keeps_its_ctrl_alt_bindings(win, monkeypatch, tmp_path):
    # #160: changing the default must not rebind an existing install. A
    # keymap.json materialized by an earlier install keeps Ctrl+Alt.
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    old = {"nav_start": {"key": "up", "mods": ["ctrl", "alt"]},
           "flush": {"key": "down", "mods": ["ctrl", "alt"]},
           "mute": {"key": "m", "mods": ["ctrl", "alt"]},
           "next_session": {"key": "p", "mods": ["ctrl", "alt"]}}
    km.write_text(json.dumps(old), encoding="utf-8")
    keymap.migrate_default_chord()                     # daemon start runs it
    loaded = keymap.load_keymap()
    for action, binding in old.items():
        assert loaded[action] == binding


def test_reset_keymap_restores_the_defaults(win, monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({
        "mute": {"key": "m", "mods": ["ctrl", "alt"]},
        "faster": {"key": "f", "mods": ["ctrl", "alt"]},
        "nav_start": {"key": None, "mods": []}}), encoding="utf-8")
    keymap.reset_keymap()
    assert keymap.load_keymap() == keymap.default_keymap()
    assert json.loads(km.read_text(encoding="utf-8")) == keymap.default_keymap()


def test_migrate_legacy_chord_never_lands_on_a_windows_shortcut(win, monkeypatch, tmp_path):
    # A pre-Ctrl+Alt keymap (Ctrl+Shift+Alt+Up) upgrades to the chord that
    # install used (Ctrl+Alt+Up), never to Win+Alt+Up, which Windows owns.
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({"nav_start": {"key": "up", "mods": ["ctrl", "shift", "alt"]},
                              "next_session": {"key": "p", "mods": ["ctrl", "shift", "alt"]}}),
                  encoding="utf-8")
    assert keymap.migrate_default_chord() is True
    user = json.loads(km.read_text(encoding="utf-8"))
    assert user["nav_start"] == {"key": "up", "mods": ["ctrl", "alt"]}
    assert user["next_session"] == {"key": "p", "mods": ["ctrl", "alt"]}


def test_left_and_right_are_free_by_default():
    # Ctrl+Alt+Left/Right no longer belong to Sonara (D1).
    keys = {b["key"] for b in keymap.default_keymap().values()}
    assert "left" not in keys and "right" not in keys


# --- migrate_default_chord ---------------------------------------------------

def test_migrate_rewrites_legacy_chord_entries(win, monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({
        "nav_start": {"key": "up", "mods": ["ctrl", "shift", "alt"]},
        "mute": {"key": "m", "mods": ["ctrl", "shift", "alt"]},
    }), encoding="utf-8")
    assert keymap.migrate_default_chord() is True
    user = json.loads(km.read_text(encoding="utf-8"))
    assert user["nav_start"]["mods"] == ["ctrl", "alt"]
    assert user["mute"]["mods"] == ["ctrl", "alt"]
    assert user["nav_start"]["key"] == "up"       # key preserved


def test_migrate_preserves_customized_bindings(win, monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({
        "nav_start": {"key": "up", "mods": ["ctrl", "shift"]},         # custom mods
        "mute": {"key": "j", "mods": ["ctrl", "shift", "alt"]},         # custom key
    }), encoding="utf-8")
    assert keymap.migrate_default_chord() is False
    user = json.loads(km.read_text(encoding="utf-8"))
    assert user["nav_start"]["mods"] == ["ctrl", "shift"]
    assert user["mute"]["key"] == "j" and user["mute"]["mods"] == ["ctrl", "shift", "alt"]


def test_migrate_is_idempotent(win, monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({"nav_start": {"key": "up", "mods": ["ctrl", "shift", "alt"]}}),
                  encoding="utf-8")
    assert keymap.migrate_default_chord() is True     # first run migrates
    assert keymap.migrate_default_chord() is False    # second run is a no-op


def test_migrate_missing_file_is_noop(win, monkeypatch, tmp_path):
    _patch_keymap_paths(monkeypatch, tmp_path)         # no file written
    assert keymap.migrate_default_chord() is False


def test_migrate_then_resolve_uses_ctrl_alt(win, monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({"flush": {"key": "down", "mods": ["ctrl", "shift", "alt"]}}),
                  encoding="utf-8")
    keymap.migrate_default_chord()
    resolved = keymap.resolve_keymap(keymap.load_keymap())
    row = next(e for e in resolved if e["action"] == "flush")
    assert row["modifiers"] == (0x0002 | 0x0001)       # ctrl|alt, no shift (0x0004)


def test_bind_action_persists_override(tmp_path, monkeypatch):
    from sonara import keymap
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    monkeypatch.setattr(keymap, "ensure_sonara_dir", lambda: None)
    keymap.bind_action("mute", "k", ["ctrl", "shift"])
    km = keymap.load_keymap()
    assert km["mute"] == {"key": "k", "mods": ["ctrl", "shift"]}


def test_bind_action_rejects_unknown_action(tmp_path, monkeypatch):
    from sonara import keymap
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    import pytest
    with pytest.raises(ValueError):
        keymap.bind_action("warp_drive", "w", ["ctrl"])


def test_rewriting_keymap_json_prunes_unknown_actions(monkeypatch, tmp_path):
    # A stale keymap.json still carries nav_prev/nav_next (removed in #135).
    # Loading ignores them; the next rewrite must drop them from the file too.
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({
        "nav_prev": {"key": "left", "mods": ["ctrl", "alt"]},
        "nav_next": {"key": "right", "mods": ["ctrl", "alt"]},
        "mute": {"key": "j", "mods": ["ctrl", "alt"]},
    }), encoding="utf-8")
    keymap.unbind_action("next_session")
    on_disk = json.loads(km.read_text(encoding="utf-8"))
    assert set(on_disk) == {"mute", "next_session"}
    assert on_disk["mute"] == {"key": "j", "mods": ["ctrl", "alt"]}


def test_bind_action_refuses_a_modifier_less_hotkey(tmp_path, monkeypatch):
    """E13: a bare 'm' registers system-wide and m stops typing everywhere."""
    from sonara import keymap
    import pytest
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    monkeypatch.setattr(keymap, "ensure_sonara_dir", lambda: None)
    for mods in ([], ["shift"]):
        with pytest.raises(ValueError) as ei:
            keymap.bind_action("mute", "m", mods)
        assert "ctrl" in str(ei.value).lower()
    assert not (tmp_path / "keymap.json").exists()      # nothing persisted


def test_bind_action_accepts_win_as_the_modifier(tmp_path, monkeypatch):
    from sonara import keymap
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    monkeypatch.setattr(keymap, "ensure_sonara_dir", lambda: None)
    keymap.bind_action("mute", "m", ["win", "shift"])
    assert keymap.load_keymap()["mute"]["mods"] == ["win", "shift"]
