"""#65: mute survives the silent daemon respawn; lifecycle hardening.

The audit found _mute_level was memory-only while hooks silently resurrect a
dead daemon within seconds - mute (and any volatile state) reset between two
assistant messages. These tests pin the persistence + the respawn-trigger
fixes (transient accept errors, faulthandler evidence, split-brain notices).
"""
from unittest import mock

from tests.daemon_helpers import make_daemon
from sonara.protocol import MsgType, PROTOCOL_VERSION


# --- mute persistence --------------------------------------------------------

def test_mute_level_restored_from_config_at_startup():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config2 = dict(config)
    config2["mute_level"] = 1
    from sonara.daemon import SpeechDaemon
    d2 = SpeechDaemon(speaker, sessions, config2)
    assert d2._mute_level == 1
    assert d2._muted is True


def test_mute_level_garbage_in_config_restores_unmuted():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config2 = dict(config)
    config2["mute_level"] = "junk"
    from sonara.daemon import SpeechDaemon
    d2 = SpeechDaemon(speaker, sessions, config2)
    assert d2._mute_level == 0


def test_mute_toggle_persists_level_to_config():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("sonara.daemon.save_config") as save:
        daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert config["mute_level"] == 1
    save.assert_called_once_with(config)
    with mock.patch("sonara.daemon.save_config"):
        daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
        daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert config["mute_level"] == 0            # full cycle lands back at unmuted


# --- transient accept() containment ------------------------------------------

def test_accept_loop_survives_transient_oserror():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")

    class FlakyServer:
        def __init__(self):
            self.calls = 0

        def accept(self):
            self.calls += 1
            if self.calls <= 2:
                raise OSError("transient")      # burst of resets
            daemon._running.clear()             # then a clean shutdown
            raise OSError("closed")

    daemon._running.set()
    daemon._server = FlakyServer()
    daemon._accept_loop()                       # returns instead of hanging
    assert daemon._server.calls == 3            # survived the first two errors


def test_accept_loop_exits_on_persistent_failure():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")

    class DeadServer:
        def __init__(self):
            self.calls = 0

        def accept(self):
            self.calls += 1
            raise OSError("dead socket")

    daemon._running.set()
    daemon._server = DeadServer()
    import time as _t
    with mock.patch.object(_t, "sleep"):
        daemon._accept_loop()
    assert daemon._server.calls > 20            # capped, no infinite loop


# --- faulthandler evidence preservation ---------------------------------------

def _arm(tmp_dir, monkeypatch):
    from sonara import paths, daemon as dmod
    monkeypatch.setattr(paths, "SONARA_DIR", tmp_dir, raising=False)
    dmod._arm_faulthandler()


def test_faulthandler_rotates_a_real_dump(tmp_path, monkeypatch):
    dump = tmp_path / "faulthandler.log"
    dump.write_text("=== faulthandler armed: pid 1 ===\n"
                    "Fatal Python error: Segmentation fault\n"
                    "Thread 0x01 (most recent call first):\n", encoding="utf-8")
    _arm(tmp_path, monkeypatch)
    prev = tmp_path / "faulthandler.prev.log"
    assert prev.exists()
    assert "Segmentation fault" in prev.read_text(encoding="utf-8")


def test_faulthandler_header_only_file_is_not_rotated(tmp_path, monkeypatch):
    dump = tmp_path / "faulthandler.log"
    dump.write_text("=== faulthandler armed: pid 1 ===\n", encoding="utf-8")
    _arm(tmp_path, monkeypatch)
    assert not (tmp_path / "faulthandler.prev.log").exists()


# --- split-brain hotkey notice -------------------------------------------------

def test_hotkey_collisions_are_announced_audibly():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._announce_hotkey_collisions([{"action": "mute", "error": 1409}])
    for _ in range(2):
        daemon._speak_loop_once()
    assert any("hotkeys" in t.lower() for t in speaker.spoken)


def test_no_collisions_stays_silent():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._announce_hotkey_collisions([])
    daemon._announce_hotkey_collisions(None)
    for _ in range(2):
        daemon._speak_loop_once()
    assert speaker.spoken == []


# --- stray daemon sweep ---------------------------------------------------------

def test_kill_stray_daemons_counts_killed_pids(monkeypatch):
    from sonara import cli

    class FakeProc:
        stdout = b"1234\n5678\n"

    monkeypatch.setattr(cli.os, "name", "nt", raising=False)
    out = cli._kill_stray_daemons(runner=lambda argv: FakeProc())
    assert out == 2


def test_kill_stray_daemons_swallows_failures(monkeypatch):
    from sonara import cli
    monkeypatch.setattr(cli.os, "name", "nt", raising=False)

    def boom(argv):
        raise RuntimeError("no powershell")

    assert cli._kill_stray_daemons(runner=boom) == 0
