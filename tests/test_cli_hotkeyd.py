import os
import plistlib
from unittest import mock

from sonari import cli
from sonari import keymap as _keymap
from sonari.platform.macos.hotkeys import MacHotkeyBackend, _hotkeyd_plist, LAUNCH_AGENT_LABEL as HOTKEYD_LAUNCH_AGENT_LABEL


def test_hotkeyd_plist_is_valid_and_complete(tmp_path):
    binary = "/Users/u/.sonari/sonari-hotkeyd"
    log = "/Users/u/.sonari/hotkeyd.log"
    xml = _hotkeyd_plist(binary, log)
    assert isinstance(xml, str)
    assert xml.startswith("<?xml")
    data = plistlib.loads(xml.encode("utf-8"))
    assert data["Label"] == HOTKEYD_LAUNCH_AGENT_LABEL
    assert data["ProgramArguments"] == [binary]
    assert data["RunAtLoad"] is True
    assert data["KeepAlive"] is True
    assert data["StandardErrorPath"] == log
    assert data["StandardOutPath"] == log
    assert data["ProcessType"] == "Interactive"


def test_build_hotkeyd_missing_swiftc_returns_false():
    with mock.patch("shutil.which", return_value=None):
        ok, detail = cli._build_hotkeyd()
    assert ok is False
    assert "swiftc" in detail.lower()


def test_build_hotkeyd_compiles_when_swiftc_present(tmp_path):
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=0) as call, \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"):
        ok, detail = cli._build_hotkeyd()
    assert ok is True
    # swiftc was invoked with the repo's swift source and the bin output path
    args = call.call_args.args[0]
    assert args[0] == "swiftc"
    assert args[1].endswith(os.path.join("hotkeyd", "sonari-hotkeyd.swift"))
    assert args[-1] == str(tmp_path / "sonari-hotkeyd")


def test_build_hotkeyd_nonzero_returncode_is_failure(tmp_path):
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=1), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"):
        ok, _ = cli._build_hotkeyd()
    assert ok is False


def test_build_hotkeyd_skips_recompile_when_source_unchanged(tmp_path, monkeypatch):
    # Recompiling re-prompts for any macOS permission grants (new code
    # identity), so an unchanged reinstall must NOT touch the binary.
    binp = tmp_path / "sonari-hotkeyd"
    binp.write_text("pretend-built binary")
    monkeypatch.setattr(cli.paths, "HOTKEYD_BIN_PATH", binp)
    monkeypatch.setattr(cli.paths, "SONARI_DIR", tmp_path)
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=0) as call1:
        ok1, _ = cli._build_hotkeyd()
    assert ok1 is True and call1.call_count == 1          # first build compiles
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=0) as call2:
        ok2, detail2 = cli._build_hotkeyd()
    assert ok2 is True
    assert call2.call_count == 0                          # unchanged -> no recompile
    assert "unchanged" in detail2.lower()


def test_build_hotkeyd_recompiles_when_source_changes(tmp_path, monkeypatch):
    binp = tmp_path / "sonari-hotkeyd"
    binp.write_text("pretend-built binary")
    (tmp_path / ".hotkeyd.srchash").write_text("a-stale-hash-from-old-source")
    monkeypatch.setattr(cli.paths, "HOTKEYD_BIN_PATH", binp)
    monkeypatch.setattr(cli.paths, "SONARI_DIR", tmp_path)
    with mock.patch("shutil.which", return_value="/usr/bin/swiftc"), \
         mock.patch("subprocess.call", return_value=0) as call:
        ok, _ = cli._build_hotkeyd()
    assert ok is True and call.call_count == 1            # hash differs -> rebuild


def test_install_writes_hotkeyd_plist_and_keymap(tmp_path, capsys):
    speechd_plist = tmp_path / "com.sonari.speechd.plist"
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    km = tmp_path / "keymap.json"
    resolved = tmp_path / "hotkeyd.resolved.json"
    binp = tmp_path / "sonari-hotkeyd"
    record = tmp_path / "install.json"
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(speechd_plist)), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_place_launcher", return_value=str(tmp_path / "launcher")), \
         mock.patch.object(cli, "_copy_app", return_value=str(tmp_path / "app")), \
         mock.patch.object(cli, "_read_plugin_version", return_value="0.4.0"), \
         mock.patch.object(cli.paths, "APP_DIR", tmp_path / "app"), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", km), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", km), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir",
                           lambda: tmp_path.mkdir(parents=True, exist_ok=True)), \
         mock.patch("sonari.paths.ensure_sonari_dir"), \
         mock.patch.object(MacHotkeyBackend, "build", return_value=(True, "built")):
        rc = cli.install()
    assert rc == 0
    assert hotkeyd_plist.exists()
    assert km.exists()
    assert resolved.exists()
    data = plistlib.loads(hotkeyd_plist.read_text().encode("utf-8"))
    assert data["ProgramArguments"] == [str(binp)]
    loads = [c.args[0] for c in run.call_args_list]
    assert any(a[0] == "load" and a[1] == str(hotkeyd_plist) for a in loads)


def test_install_build_failure_is_nonfatal(tmp_path, capsys):
    speechd_plist = tmp_path / "com.sonari.speechd.plist"
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    km = tmp_path / "keymap.json"
    resolved = tmp_path / "hotkeyd.resolved.json"
    binp = tmp_path / "sonari-hotkeyd"
    record = tmp_path / "install.json"
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(speechd_plist)), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_place_launcher", return_value=str(tmp_path / "launcher")), \
         mock.patch.object(cli, "_copy_app", return_value=str(tmp_path / "app")), \
         mock.patch.object(cli, "_read_plugin_version", return_value="0.4.0"), \
         mock.patch.object(cli.paths, "APP_DIR", tmp_path / "app"), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", km), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", km), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir",
                           lambda: tmp_path.mkdir(parents=True, exist_ok=True)), \
         mock.patch("sonari.paths.ensure_sonari_dir"), \
         mock.patch.object(MacHotkeyBackend, "build",
                           return_value=(False, "swiftc not found")):
        rc = cli.install()
    assert rc == 0  # speechd still installed; build failure only warns
    # The hotkeyd LaunchAgent is NOT written when there is no binary.
    assert not hotkeyd_plist.exists()
    out = capsys.readouterr().out
    assert "warning" in out.lower() or "swiftc" in out.lower()


def test_uninstall_removes_hotkeyd_agent_and_binary(tmp_path):
    speechd_plist = tmp_path / "com.sonari.speechd.plist"
    speechd_plist.write_text("<plist/>")
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    hotkeyd_plist.write_text("<plist/>")
    sonari_dir = tmp_path / ".sonari"
    sonari_dir.mkdir()
    binp = sonari_dir / "sonari-hotkeyd"
    binp.write_text("binary")
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(speechd_plist)), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli.paths, "SONARI_DIR", sonari_dir), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp):
        rc = cli.uninstall()
    assert rc == 0
    assert not hotkeyd_plist.exists()
    unloads = [c.args[0] for c in run.call_args_list]
    assert any(a[0] == "unload" and a[1] == str(hotkeyd_plist) for a in unloads)


def _doctor_ok_patches(tmp_path):
    binp = tmp_path / "sonari-hotkeyd"
    binp.write_text("x")
    resolved = tmp_path / "hotkeyd.resolved.json"
    resolved.write_text("[]")
    # Keep the 'keymap resolves' check hermetic: point load_keymap() at a temp
    # path instead of the real ~/.sonari/keymap.json so the row is not
    # machine-state-dependent (e.g. a malformed user keymap on the test box).
    keymap = tmp_path / "keymap.json"
    return [
        mock.patch("shutil.which", side_effect=lambda n: "/usr/bin/" + n),
        mock.patch("sonari.platform.macos.tts.MacTtsBackend.best_voice", return_value="Ava (Premium)"),
        mock.patch("os.access", return_value=True),
        mock.patch("sonari.paths.ensure_sonari_dir"),
        mock.patch("sonari.client.send", return_value={"ok": True}),
        mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp),
        mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved),
        mock.patch.object(cli.keymap, "KEYMAP_PATH", keymap),
    ]


def test_doctor_includes_hotkeyd_checks(tmp_path):
    patches = _doctor_ok_patches(tmp_path)
    for p in patches:
        p.start()
    try:
        rows = cli.doctor()
    finally:
        for p in reversed(patches):
            p.stop()
    checks = {check for check, _, _ in rows}
    assert "swiftc" in checks
    assert "hotkeyd binary" in checks
    assert "hotkeyd resolved keymap" in checks
    assert "keymap resolves" in checks


def test_doctor_hotkeyd_binary_missing_fails(tmp_path):
    missing = tmp_path / "nope" / "sonari-hotkeyd"
    resolved = tmp_path / "hotkeyd.resolved.json"
    resolved.write_text("[]")
    patches = [
        mock.patch("shutil.which", side_effect=lambda n: "/usr/bin/" + n),
        mock.patch("sonari.platform.macos.tts.MacTtsBackend.best_voice", return_value="V"),
        mock.patch("os.access", return_value=True),
        mock.patch("sonari.paths.ensure_sonari_dir"),
        mock.patch("sonari.client.send", return_value={"ok": True}),
        mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", missing),
        mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved),
    ]
    for p in patches:
        p.start()
    try:
        d = {check: ok for check, ok, _ in cli.doctor()}
    finally:
        for p in reversed(patches):
            p.stop()
    assert d["hotkeyd binary"] is False


def test_keymap_subcommand_prints_all_nine_actions(capsys, tmp_path, monkeypatch):
    # Isolate the keymap so the subcommand reads DEFAULT_KEYMAP, not the
    # developer's real ~/.sonari/keymap.json (which may be remapped). An absent
    # file makes load_keymap() fall back to DEFAULT_KEYMAP -> deterministic
    # Ctrl/Cmd assertions.
    monkeypatch.setattr(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    rc = cli.main(["keymap"])
    assert rc == 0
    out = capsys.readouterr().out
    for action in ("stop", "repeat", "skip", "jump_decision", "catch_up",
                   "faster", "slower", "cycle_verbosity", "reread_options"):
        assert action in out
    # human-readable combos appear (Ctrl+Cmd default)
    assert "Ctrl" in out and "Cmd" in out


def test_display_tables_cover_every_keycode_and_modifier():
    # cli._KEYCODE_DISPLAY / _MOD_DISPLAY are a hand-maintained presentation copy
    # of the authoritative keymap.KEY_CODES / MOD_MASKS values. If a key or
    # modifier is added to keymap.py without a friendly label here, _combo_label
    # silently degrades to 'keyN'. Fail loudly instead.
    for key_code in set(_keymap.KEY_CODES.values()):
        assert key_code in cli._KEYCODE_DISPLAY, (
            "keyCode {0} has no friendly label in cli._KEYCODE_DISPLAY".format(
                key_code))
    display_masks = {mask for mask, _ in cli._MOD_DISPLAY}
    for mask in set(_keymap.MOD_MASKS.values()):
        assert mask in display_masks, (
            "modifier mask {0} has no friendly label in cli._MOD_DISPLAY".format(
                mask))
