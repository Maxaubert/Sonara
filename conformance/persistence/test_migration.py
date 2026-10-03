"""Migration from the Python plugin (#201): the first runtime on a home
without ``config.json`` imports the plugin's ``config.json``, ``keymap.json``
and ``session_prefs.json`` from its folder (``--migrate-from``; for the
default home ``%USERPROFILE%\\.sonara``), leaves that folder unchanged,
logs what it did and never migrates twice.

The plugin folders are fixture trees under ``tmp_path``; the runtime gets a
temporary ``USERPROFILE`` too, so no test can read this PC's real one."""
from __future__ import annotations

import json

import pytest

PLUGIN_CONFIG = {
    "_format": 2,
    "voice": "af_sarah",
    "rate": 250,
    "volume": 150,
    "audio_mode": "duck",
    "duck_level": 20,
    "mute_level": 1,
    "verbosity": "medium",
    "minqueue": 3,
    "summary_mode": True,
    "summary_command": "codex",
    "summary_model": "gpt-5.4-mini",
    "summary_timeout": 90,
    "summary_style": "brief",
    "summary_prompts": {"brief": "Say it in one line."},
    "cue_voice": "af_heart",
    "fast_cues": True,
    "settings_port": 27431,
}
PLUGIN_KEYMAP = {
    "nav_start": {"key": "home", "mods": ["win", "alt"]},
    "next_session": {"key": "n", "mods": ["ctrl", "alt"]},
    "pause": {"key": "s", "mods": ["ctrl", "alt"]},
    "flush": {"key": None, "mods": []},
}
PLUGIN_PREFS = {"sess-1": {"name": "Build", "muted": True}, "sess-2": {"voice": "cb_default"}}


def plugin_tree(root, config=PLUGIN_CONFIG, keymap=PLUGIN_KEYMAP, prefs=PLUGIN_PREFS):
    d = root / ".sonara"
    d.mkdir(parents=True)
    for name, data in (("config.json", config), ("keymap.json", keymap),
                       ("session_prefs.json", prefs)):
        if data is not None:
            (d / name).write_text(json.dumps(data), encoding="utf-8")
    return d


def snapshot(d):
    return {p.name: p.read_bytes() for p in d.iterdir()}


@pytest.fixture
def profile(tmp_path):
    """A temporary USERPROFILE for the runtime (never the real one)."""
    p = tmp_path / "profile"
    p.mkdir()
    return {"USERPROFILE": str(p), "HOME": str(p)}


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def get(c, key):
    return ok(c, {"type": "get", "key": key})["value"]


def test_the_plugin_settings_are_imported(start, tmp_path, profile):
    legacy = plugin_tree(tmp_path / "user")
    before = snapshot(legacy)
    rt = start("--migrate-from", str(legacy), env=profile)
    c = rt.tcp(extensions=["agent", "system"])
    assert get(c, "rate") == 250
    assert get(c, "volume") == 100, "a gain above 100 % is 100 %"
    assert get(c, "audio_mode") == "duck"
    assert get(c, "duck_level") == 20
    assert get(c, "mute_level") == 1
    assert get(c, "verbosity") == "skip_code", "#214: the plugin's medium is skip_code"
    assert get(c, "minqueue") == 3
    assert get(c, "read_mode") == "queue", "#222: a minqueue above 1 is a queue"
    s = get(c, "summaries")
    assert (s["enabled"], s["command"], s["model"], s["timeout"], s["style"]) == (
        True, "codex", "gpt-5.4-mini", 90, "brief")
    assert s["prompt"] == "Say it in one line."
    # af_sarah is a Kokoro voice: the fake engine lacks it, so the reader
    # keeps its default and the saved choice waits in config.json.
    assert get(c, "voice") is None
    cfg = json.loads((rt.home / "config.json").read_text(encoding="utf-8"))
    assert cfg["voice"] == "af_sarah"
    assert cfg["_migrated"]["from"] == str(legacy)
    assert cfg["volume"] == 100, "a key of a format 2 file is the user's choice"
    assert "settings_port" not in cfg and "cue_voice" not in cfg
    # Hotkeys under the runtime's names.
    by_action = {b["action"]: b for b in get(c, "hotkeys")["bindings"]}
    assert by_action["restart"]["combo"] == "Win+Alt+Home"
    assert by_action["next_channel"]["combo"] == "Ctrl+Alt+N"
    assert by_action["pause"]["combo"] == "Ctrl+Alt+S"
    assert by_action["flush"]["key"] is None
    assert by_action["mute"]["combo"] == "Ctrl+Alt+M", "not in the plugin's file: the default"
    # Session preferences; a Chatterbox voice speaks as Heart.
    rows = {r["channel"]: r for r in get(c, "channel_prefs")}
    assert rows["sess-1"]["label"] == "Build" and rows["sess-1"]["muted"] is True
    assert rows["sess-2"]["voice"] == "af_heart"
    # The plugin's folder is untouched, and the log says what happened.
    assert snapshot(legacy) == before
    log = (rt.home / "logs" / "sonarad.log").read_text(encoding="utf-8")
    assert "migration" in log and str(legacy) in log
    assert "af_sarah" in log, "the voice that could not apply is logged"


def test_the_migration_runs_once(start, tmp_path, profile):
    legacy = plugin_tree(tmp_path / "user")
    rt = start("--migrate-from", str(legacy), env=profile)
    c = rt.tcp()
    ok(c, {"type": "set", "key": "rate", "value": 310})
    c.close()
    rt.close()
    (legacy / "config.json").write_text(json.dumps({**PLUGIN_CONFIG, "rate": 120}), encoding="utf-8")
    rt = start("--migrate-from", str(legacy), home=rt.home, env=profile)
    c = rt.tcp()
    assert get(c, "rate") == 310, "the user's later change wins; no second import"


def test_an_old_plugin_file_and_missing_files(start, tmp_path, profile):
    legacy = plugin_tree(tmp_path / "user",
                         config={"audio_control": True, "duck_level": 20, "summary_timeout": 20,
                                 "voice": "chatterbox:narrator", "rate": 200},
                         keymap=None, prefs=None)
    rt = start("--migrate-from", str(legacy), env=profile)
    c = rt.tcp(extensions=["agent", "system"])
    assert get(c, "audio_mode") == "duck", "pre-#92 audio_control"
    assert get(c, "duck_level") == 30, "20 was the old default, not a choice"
    assert get(c, "summaries")["timeout"] == 60
    cfg = json.loads((rt.home / "config.json").read_text(encoding="utf-8"))
    assert cfg["voice"] == "af_heart"
    assert not (rt.home / "keymap.json").exists()
    assert not (rt.home / "session_prefs.json").exists()


def test_a_home_with_a_keymap_keeps_it(start, tmp_path, profile):
    legacy = plugin_tree(tmp_path / "user")
    home = tmp_path / "home"
    home.mkdir()
    own = {"mute": {"key": "k", "mods": ["ctrl", "alt"]}}
    (home / "keymap.json").write_text(json.dumps(own), encoding="utf-8")
    rt = start("--migrate-from", str(legacy), home=home, env=profile)
    c = rt.tcp(extensions=["system"])
    by_action = {b["action"]: b for b in get(c, "hotkeys")["bindings"]}
    assert by_action["mute"]["combo"] == "Ctrl+Alt+K"
    assert by_action["restart"]["combo"] == "Ctrl+Alt+Up"
    assert get(c, "rate") == 250, "the settings are still imported"


def test_no_flag_and_a_non_default_home_never_migrate(start, tmp_path, profile):
    """Only the default home reads %USERPROFILE%\\.sonara on its own: a
    bundling app's or a test's home does not pick up the plugin's settings."""
    plugin_tree(tmp_path / "profile")
    rt = start(env=profile)
    c = rt.tcp()
    assert get(c, "rate") == 250, "the default"
    assert not (rt.home / "config.json").exists()


def test_nothing_to_migrate_writes_nothing(start, tmp_path, profile):
    rt = start("--migrate-from", str(tmp_path / "missing"), env=profile)
    c = rt.tcp()
    assert get(c, "rate") == 250, "the default"
    assert not (rt.home / "config.json").exists()


def test_the_default_home_imports_from_the_profile(start, tmp_path, profile):
    """What an end user gets: no flag, the default %LOCALAPPDATA%\\Sonara home
    (passed with --home as the SDK clients do) and the plugin's folder in
    %USERPROFILE%. Both are temporary folders here. The default home's
    instance lock is per user only, so a real runtime on this PC makes the
    test skip rather than collide."""
    lad = tmp_path / "lad"
    home = lad / "Sonara"
    plugin_tree(tmp_path / "profile")
    env = {**profile, "LOCALAPPDATA": str(lad), "SONARA_HOME": ""}
    try:
        rt = start("--home", str(home), home=home, env=env)
    except RuntimeError as e:
        if "already running" in str(e):
            pytest.skip("a runtime on the real default home holds the per-user lock")
        raise
    c = rt.tcp()
    assert get(c, "rate") == 250
    cfg = json.loads((home / "config.json").read_text(encoding="utf-8"))
    assert cfg["_migrated"]["from"] == str(tmp_path / "profile" / ".sonara")
