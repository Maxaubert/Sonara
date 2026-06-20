"""hotkeyd build + the keymap subcommand.

Post-seam-refactor, the hotkeyd LaunchAgent/build mechanics live in
MacHotkeyBackend (tested here against the backend directly). cli.install/
uninstall/doctor dispatch is covered in test_cli_install/_uninstall/_doctor.
"""
import os
import plistlib
from unittest import mock

from sonara import cli
from sonara.platform.macos.hotkeys import (
    MacHotkeyBackend, _hotkeyd_plist, LAUNCH_AGENT_LABEL as HOTKEYD_LAUNCH_AGENT_LABEL,
)


def test_hotkeyd_plist_is_valid_and_complete():
    binary = "/Users/u/.sonara/sonara-hotkeyd"
    log = "/Users/u/.sonara/hotkeyd.log"
    xml = _hotkeyd_plist(binary, log)
    assert isinstance(xml, str) and xml.startswith("<?xml")
    data = plistlib.loads(xml.encode("utf-8"))
    assert data["Label"] == HOTKEYD_LAUNCH_AGENT_LABEL
    assert data["ProgramArguments"] == [binary]
    assert data["RunAtLoad"] is True and data["KeepAlive"] is True
    assert data["StandardErrorPath"] == log and data["StandardOutPath"] == log
    assert data["ProcessType"] == "Interactive"


def test_build_hotkeyd_missing_swiftc_returns_false():
    with mock.patch("shutil.which", return_value=None):
        ok, detail = MacHotkeyBackend().build()
    assert ok is False and "swiftc" in detail.lower()


def test_build_hotkeyd_compiles_when_swiftc_present(tmp_path):
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=0) as call, \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonara-hotkeyd"):
        ok, detail = MacHotkeyBackend().build()
    assert ok is True
    args = call.call_args.args[0]
    assert args[0] == "swiftc"
    assert args[1].endswith(os.path.join("hotkeyd", "sonara-hotkeyd.swift"))
    assert args[-1] == str(tmp_path / "sonara-hotkeyd")


def test_build_hotkeyd_nonzero_returncode_is_failure(tmp_path):
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=1), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonara-hotkeyd"):
        ok, _ = MacHotkeyBackend().build()
    assert ok is False


def test_build_hotkeyd_skips_recompile_when_source_unchanged(tmp_path, monkeypatch):
    binp = tmp_path / "sonara-hotkeyd"
    binp.write_text("pretend-built binary")
    monkeypatch.setattr(cli.paths, "HOTKEYD_BIN_PATH", binp)
    monkeypatch.setattr(cli.paths, "SONARA_DIR", tmp_path)
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=0) as call1:
        ok1, _ = MacHotkeyBackend().build()
    assert ok1 is True and call1.call_count == 1
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=0) as call2:
        ok2, detail2 = MacHotkeyBackend().build()
    assert ok2 is True and call2.call_count == 0
    assert "unchanged" in detail2.lower()


def test_build_hotkeyd_recompiles_when_source_changes(tmp_path, monkeypatch):
    binp = tmp_path / "sonara-hotkeyd"
    binp.write_text("pretend-built binary")
    (tmp_path / ".hotkeyd.srchash").write_text("a-stale-hash-from-old-source")
    monkeypatch.setattr(cli.paths, "HOTKEYD_BIN_PATH", binp)
    monkeypatch.setattr(cli.paths, "SONARA_DIR", tmp_path)
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=0) as call:
        ok, _ = MacHotkeyBackend().build()
    assert ok is True and call.call_count == 1


def test_keymap_subcommand_prints_the_default_bindings(capsys, tmp_path, monkeypatch):
    # Force the REAL platform to macOS so BOTH resolve_keymap (keytables) and
    # display_combo (labels) agree -> deterministic Ctrl+Cmd output on any host.
    import sonara.platform as platform
    monkeypatch.setattr(platform.sys, "platform", "darwin")
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
        assert "Ctrl" in out and "Cmd" in out
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
