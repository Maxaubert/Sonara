"""The keymap subcommand.

Post-seam-refactor, Windows hotkeys run in-process (started by the daemon), so
there is no build step to test here. cli.install/uninstall/doctor dispatch is
covered in test_cli_install/_uninstall/_doctor.
"""
from unittest import mock

from sonara import cli


def test_keymap_subcommand_prints_the_default_bindings(capsys, tmp_path, monkeypatch):
    # Force the REAL platform to Windows so BOTH resolve_keymap (keytables) and
    # display_combo (labels) agree -> deterministic Ctrl output on any host.
    import sonara.platform as platform
    monkeypatch.setattr(platform.sys, "platform", "win32")
    platform._CACHE = None
    cli._PLATFORM = None
    monkeypatch.setattr(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    try:
        rc = cli.main(["keymap"])
        assert rc == 0
        out = capsys.readouterr().out
        for action in ("nav_next", "nav_prev", "pause", "mute"):
            assert action in out
        # faster/slower are listed too, marked unbound (the keymap lists every action)
        assert "faster" in out and "slower" in out
        assert "(unbound)" in out
        assert "Ctrl" in out
    finally:
        platform._CACHE = None
        cli._PLATFORM = None


def test_keymap_clear_unbinds_and_requests_live_reload(monkeypatch, tmp_path):
    import json
    monkeypatch.setattr(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    sent = []
    with mock.patch("sonara.client.send", side_effect=lambda m, **k: sent.append(m)):
        rc = cli.main(["keymap", "nav_next", "clear"])
    assert rc == 0
    user = json.loads((tmp_path / "keymap.json").read_text(encoding="utf-8"))
    assert user["nav_next"]["key"] is None                  # unbound override written
    assert any(m.get("type") == "reload_keymap" for m in sent)  # live reload requested


def test_keymap_clear_rejects_unknown_action(monkeypatch, tmp_path):
    monkeypatch.setattr(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    with mock.patch("sonara.client.send"):
        rc = cli.main(["keymap", "bogus", "clear"])
    assert rc == 1
