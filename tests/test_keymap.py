import json

import pytest

from sonari import keymap


def _patch_keymap_paths(monkeypatch, tmp_path):
    km = tmp_path / "keymap.json"
    resolved = tmp_path / "hotkeyd.resolved.json"
    monkeypatch.setattr(keymap, "KEYMAP_PATH", km)
    monkeypatch.setattr(keymap, "HOTKEYD_RESOLVED_PATH", resolved)
    monkeypatch.setattr(keymap, "SONARI_DIR", tmp_path)
    monkeypatch.setattr(keymap, "ensure_sonari_dir",
                        lambda: tmp_path.mkdir(parents=True, exist_ok=True))
    return km, resolved


# --- constants ---------------------------------------------------------------

def test_key_codes_cover_default_keys():
    for k in ("s", "r", "d", "l", "v", "o", ".", "]", "["):
        assert k in keymap.KEY_CODES
    assert keymap.KEY_CODES["s"] == 1
    assert keymap.KEY_CODES["."] == 47
    assert keymap.KEY_CODES["]"] == 30
    assert keymap.KEY_CODES["["] == 33


def test_mod_masks_values():
    assert keymap.MOD_MASKS["cmd"] == 256
    assert keymap.MOD_MASKS["shift"] == 512
    assert keymap.MOD_MASKS["opt"] == 2048
    assert keymap.MOD_MASKS["ctrl"] == 4096


def test_action_messages_faster_has_delta_25():
    assert keymap.ACTION_MESSAGES["faster"] == {"type": "set_rate", "delta": 25}
    assert keymap.ACTION_MESSAGES["slower"] == {"type": "set_rate", "delta": -25}


def test_default_keymap_has_nine_actions():
    assert set(keymap.DEFAULT_KEYMAP.keys()) == {
        "stop", "repeat", "skip", "jump_decision", "catch_up",
        "faster", "slower", "cycle_verbosity", "reread_options",
    }
    assert keymap.DEFAULT_KEYMAP["stop"]["key"] == "s"
    assert keymap.DEFAULT_KEYMAP["stop"]["mods"] == ["ctrl", "cmd"]
    assert keymap.DEFAULT_KEYMAP["skip"]["key"] == "."
    assert keymap.DEFAULT_KEYMAP["faster"]["key"] == "]"
    assert keymap.DEFAULT_KEYMAP["slower"]["key"] == "["


# --- resolve_keymap ----------------------------------------------------------

def test_resolve_stop_entry_exact():
    resolved = keymap.resolve_keymap({"stop": {"key": "s", "mods": ["ctrl", "cmd"]}})
    assert resolved == [{
        "action": "stop",
        "keyCode": 1,
        "modifiers": 4352,  # 4096 | 256
        "message": '{"type": "stop"}',
    }]


def test_resolve_faster_message_is_json_with_delta():
    resolved = keymap.resolve_keymap({"faster": {"key": "]", "mods": ["ctrl", "cmd"]}})
    entry = resolved[0]
    assert entry["keyCode"] == 30
    assert entry["modifiers"] == 4352
    assert json.loads(entry["message"]) == {"type": "set_rate", "delta": 25}


def test_resolve_default_keymap_has_nine_entries():
    resolved = keymap.resolve_keymap(keymap.DEFAULT_KEYMAP)
    assert len(resolved) == 9
    actions = {e["action"] for e in resolved}
    assert actions == set(keymap.DEFAULT_KEYMAP.keys())


def test_resolve_unknown_key_raises():
    with pytest.raises(ValueError):
        keymap.resolve_keymap({"stop": {"key": "zzz", "mods": ["ctrl", "cmd"]}})


def test_resolve_unknown_mod_raises():
    with pytest.raises(ValueError):
        keymap.resolve_keymap({"stop": {"key": "s", "mods": ["hyper"]}})


def test_resolve_unknown_action_raises():
    with pytest.raises(ValueError):
        keymap.resolve_keymap({"frobnicate": {"key": "s", "mods": ["ctrl", "cmd"]}})


# --- load_keymap -------------------------------------------------------------

def test_load_keymap_returns_defaults_when_missing(monkeypatch, tmp_path):
    _patch_keymap_paths(monkeypatch, tmp_path)
    loaded = keymap.load_keymap()
    assert loaded == keymap.DEFAULT_KEYMAP
    # must be an independent copy
    loaded["stop"]["key"] = "x"
    assert keymap.DEFAULT_KEYMAP["stop"]["key"] == "s"


def test_load_keymap_merges_user_override(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text(json.dumps({"stop": {"key": "x", "mods": ["cmd"]}}), encoding="utf-8")
    loaded = keymap.load_keymap()
    assert loaded["stop"] == {"key": "x", "mods": ["cmd"]}
    # untouched actions keep defaults
    assert loaded["repeat"] == keymap.DEFAULT_KEYMAP["repeat"]


def test_load_keymap_tolerates_corrupt_file(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    km.write_text("{ not json", encoding="utf-8")
    assert keymap.load_keymap() == keymap.DEFAULT_KEYMAP


# --- write_default_keymap_if_absent -----------------------------------------

def test_write_default_keymap_if_absent_writes_once(monkeypatch, tmp_path):
    km, _ = _patch_keymap_paths(monkeypatch, tmp_path)
    assert not km.exists()
    assert keymap.write_default_keymap_if_absent() is True
    assert km.exists()
    on_disk = json.loads(km.read_text(encoding="utf-8"))
    assert on_disk == keymap.DEFAULT_KEYMAP
    # second call is a no-op
    assert keymap.write_default_keymap_if_absent() is False


# --- write_resolved ----------------------------------------------------------

def test_write_resolved_emits_array_of_nine(monkeypatch, tmp_path):
    km, resolved = _patch_keymap_paths(monkeypatch, tmp_path)
    out_path = keymap.write_resolved()
    assert out_path == str(resolved)
    data = json.loads(resolved.read_text(encoding="utf-8"))
    assert isinstance(data, list)
    assert len(data) == 9
    for entry in data:
        assert isinstance(entry["keyCode"], int)
        assert isinstance(entry["modifiers"], int)
        assert isinstance(entry["message"], str)


def test_write_resolved_no_tmp_leftover(monkeypatch, tmp_path):
    km, resolved = _patch_keymap_paths(monkeypatch, tmp_path)
    keymap.write_resolved()
    assert list(tmp_path.glob("*.tmp")) == []
