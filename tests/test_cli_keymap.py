"""The keymap subcommand.

Post-seam-refactor, Windows hotkeys run in-process (started by the daemon), so
there is no build step to test here. install/uninstall/doctor dispatch is
covered in test_cli_install/_uninstall/_doctor.
"""
from unittest import mock

from sonara import cli
from sonara import keymap


def test_keymap_subcommand_prints_the_default_bindings(capsys, tmp_path, monkeypatch):
    # Force the REAL platform to Windows so BOTH resolve_keymap (keytables) and
    # display_combo (labels) agree -> deterministic Win+Alt output on any host.
    import sonara.platform as platform
    monkeypatch.setattr(platform.sys, "platform", "win32")
    platform._CACHE = None
    cli._PLATFORM = None
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    try:
        rc = cli.main(["keymap"])
        assert rc == 0
        out = capsys.readouterr().out
        for action in ("nav_start", "flush", "pause", "mute"):
            assert action in out
        assert "nav_prev" not in out and "nav_next" not in out   # removed (D1)
        # faster/slower are listed too, marked unbound (the keymap lists every action)
        assert "faster" in out and "slower" in out
        assert "(unbound)" in out
        assert "Win+Alt+Home" in out and "Ctrl" not in out
    finally:
        platform._CACHE = None
        cli._PLATFORM = None


def test_keymap_clear_unbinds_and_requests_live_reload(monkeypatch, tmp_path):
    import json
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    sent = []
    with mock.patch("sonara.client.send", side_effect=lambda m, **k: sent.append(m)):
        rc = cli.main(["keymap", "nav_start", "clear"])
    assert rc == 0
    user = json.loads((tmp_path / "keymap.json").read_text(encoding="utf-8"))
    assert user["nav_start"]["key"] is None                 # unbound override written
    assert any(m.get("type") == "reload_keymap" for m in sent)  # live reload requested


def test_keymap_reset_restores_defaults_and_requests_live_reload(monkeypatch, tmp_path):
    """#160: `sonara keymap --reset` moves an existing install (whose
    keymap.json keeps the old Ctrl+Alt chords) to the Win+Alt defaults."""
    import json
    import sonara.platform as platform
    monkeypatch.setattr(platform.sys, "platform", "win32")
    platform._CACHE = None
    km = tmp_path / "keymap.json"
    km.write_text(json.dumps({"mute": {"key": "m", "mods": ["ctrl", "alt"]}}),
                  encoding="utf-8")
    monkeypatch.setattr(keymap, "KEYMAP_PATH", km)
    sent = []
    try:
        with mock.patch("sonara.client.send", side_effect=lambda m, **k: sent.append(m)):
            rc = cli.main(["keymap", "--reset"])
        assert rc == 0
        assert json.loads(km.read_text(encoding="utf-8")) == keymap.default_keymap()
    finally:
        platform._CACHE = None
    assert any(m.get("type") == "reload_keymap" for m in sent)


def test_keymap_reset_refuses_an_action_argument(monkeypatch, tmp_path):
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    with mock.patch("sonara.client.send"):
        rc = cli.main(["keymap", "--reset", "mute"])
    assert rc == 2
    assert not (tmp_path / "keymap.json").exists()


def test_keymap_clear_rejects_unknown_action(monkeypatch, tmp_path):
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    with mock.patch("sonara.client.send"):
        rc = cli.main(["keymap", "bogus", "clear"])
    assert rc == 1
