# tests/test_win_earcon.py
import math
import struct
import wave
from unittest import mock

from sonara.platform.windows.earcon import WinEarconBackend


def _wav(tmp_path):
    p = tmp_path / "e.wav"
    with wave.open(str(p), "w") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(44100)
        w.writeframes(b"".join(
            struct.pack("<h", int(math.sin(i / 10) * 1000)) for i in range(4410)))
    return p


def test_play_existing_spawns_windowless_helper(tmp_path):
    p = _wav(tmp_path)
    with mock.patch("subprocess.Popen") as popen:
        h = WinEarconBackend().play(str(p))
    assert h is popen.return_value          # returns the Popen handle
    argv = popen.call_args.args[0]
    assert argv[-1] == str(p)               # the wav path is the last arg
    assert "winsound" in argv[2]            # the inline player script
    assert popen.call_args.kwargs["creationflags"] == WinEarconBackend._SPAWN_FLAGS


def test_play_missing_returns_none_without_spawning(tmp_path):
    with mock.patch("subprocess.Popen") as popen:
        result = WinEarconBackend().play(str(tmp_path / "nope.wav"))
    assert result is None and popen.call_count == 0


def test_play_spawn_failure_is_logged_not_swallowed(tmp_path, capsys):
    # A real WAV exists, so play() reaches the spawn; force the spawn to blow up.
    p = _wav(tmp_path)
    with mock.patch("subprocess.Popen", side_effect=OSError("spawn boom")):
        result = WinEarconBackend().play(str(p))
    # Contract preserved: a spawn failure still returns None (caller handles it).
    assert result is None
    # ...but it must be DIAGNOSABLE: a traceback is emitted to stderr (which the
    # daemon routes to LOG_PATH), not silently swallowed.
    err = capsys.readouterr().err
    assert "Traceback" in err
    assert "spawn boom" in err


def test_default_earcons_six():
    assert len(WinEarconBackend().default_earcons()) == 6
