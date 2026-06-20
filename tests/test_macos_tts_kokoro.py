"""MacTtsBackend Kokoro routing + the afplay playback handle (issue #41).

Mirror of tests/test_win_tts_kokoro.py for the macOS backend: route Kokoro voices
to the portable KokoroEngine and play the WAV bytes through afplay, with a
subprocess-like handle (.wait/.terminate/.poll/.returncode) that cleans up the
temp WAV exactly once. Everything is mocked at the seams (kokoro.is_installed /
_get_kokoro / subprocess.Popen / tempfile.tempdir) so no [kokoro] extra, no real
audio device, and no real model download is touched.
"""
import os
import subprocess

import pytest

from sonara.platform.macos import tts as mod
from sonara.platform.macos.tts import MacTtsBackend
from sonara import kokoro
from sonara import kokoro_provision as kp


def _bare_backend():
    # Skip __init__ (which sweeps temp WAVs / touches the FS); we only test routing.
    b = MacTtsBackend.__new__(MacTtsBackend)
    b._kokoro = None
    return b


class FakePopen:
    """Configurable subprocess.Popen stand-in for afplay.

    poll_seq: values poll() returns in order (the last value then repeats).
    wait_exc: exception instance wait() raises (e.g. TimeoutExpired).
    exit_code: returncode wait() sets when it returns normally.
    terminate() flips exit_code to -15 (SIGTERM) so a following reap-wait yields it.
    """

    def __init__(self, argv=None, *, exit_code=0, wait_exc=None, poll_seq=None):
        self.argv = argv
        self.returncode = None
        self.calls = []
        self.exit_code = exit_code
        self.wait_exc = wait_exc
        self.poll_seq = poll_seq
        self._last_poll = None

    def wait(self, timeout=None):
        self.calls.append("wait")
        if self.wait_exc is not None:
            raise self.wait_exc
        self.returncode = self.exit_code
        return self.returncode

    def poll(self):
        self.calls.append("poll")
        if self.poll_seq is None:
            return self.returncode
        if self.poll_seq:
            self._last_poll = self.poll_seq.pop(0)
        val = self._last_poll
        if val is not None:
            self.returncode = val
        return val

    def terminate(self):
        self.calls.append("terminate")
        self.exit_code = -15  # SIGTERM: a subsequent reap-wait reports -15


def _patch_popen(monkeypatch, proc=None, raises=None):
    """Patch mod.subprocess.Popen; return a dict that captures the created proc/argv."""
    captured = {}

    def factory(argv):
        captured["argv"] = argv
        if raises is not None:
            raise raises
        p = proc if proc is not None else FakePopen()
        p.argv = argv
        captured["proc"] = p
        return p

    monkeypatch.setattr(mod.subprocess, "Popen", factory)
    return captured


def _use_tmp_tempdir(monkeypatch, tmp_path):
    """Point tempfile (mkstemp + the sweep glob) at an isolated dir."""
    monkeypatch.setattr(mod.tempfile, "tempdir", str(tmp_path))


# ---------------------------------------------------------------------------
# run() routing
# ---------------------------------------------------------------------------

def test_run_routes_native_voice_to_say_command(monkeypatch):
    b = _bare_backend()
    captured = _patch_popen(monkeypatch)
    monkeypatch.setattr(b, "_get_kokoro",
                        lambda: pytest.fail("Kokoro used for a native voice"))
    b.run("Hi", "Samantha", 200)
    assert captured["argv"] == ["say", "-v", "Samantha", "-r", "200", "Hi"]


def test_run_routes_kokoro_voice_to_engine_and_afplay(monkeypatch):
    b = _bare_backend()
    seen = {}

    class FakeEngine:
        def wav_bytes(self, text, voice, speed):
            seen["synth"] = (text, voice, speed)
            return b"KOKORO_WAV"

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(b, "_get_kokoro", lambda: FakeEngine())
    monkeypatch.setattr(mod, "_play_wav_bytes",
                        lambda data: seen.setdefault("played", data))
    monkeypatch.setattr(mod.subprocess, "Popen",
                        lambda *a, **k: pytest.fail("say must not run for a Kokoro voice"))

    b.run("hello there", "af_heart", 200)
    assert seen["synth"][0] == "hello there"
    assert seen["synth"][1] == "af_heart"
    assert seen["synth"][2] == pytest.approx(1.0)  # rate 200 -> speed 1.0
    assert seen["played"] == b"KOKORO_WAV"


@pytest.mark.parametrize("voice", ["AF_HEART", "Af_Heart", "kokoro:af_heart"])
def test_run_routes_kokoro_voice_variants_to_engine(monkeypatch, voice):
    # Case variants and the `kokoro:` engine-prefix must still route to the engine
    # (via is_kokoro_voice's normalization), NOT silently fall through to `say`.
    # The raw voice string is forwarded; the engine normalizes it internally.
    b = _bare_backend()
    seen = {}

    class FakeEngine:
        def wav_bytes(self, text, v, speed):
            seen["voice"] = v
            return b"KOKORO_WAV"

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(b, "_get_kokoro", lambda: FakeEngine())
    monkeypatch.setattr(mod, "_play_wav_bytes",
                        lambda data: seen.setdefault("played", data))
    monkeypatch.setattr(mod.subprocess, "Popen",
                        lambda *a, **k: pytest.fail(f"say must not run for {voice!r}"))

    b.run("hi", voice, 200)
    assert seen["voice"] == voice
    assert seen["played"] == b"KOKORO_WAV"


def test_run_kokoro_voice_without_extra_raises_actionable(monkeypatch):
    # A Kokoro voice reaching run() without the extra (e.g. via config-sync from
    # Windows) must raise an actionable error, NOT silently fall back to `say`.
    b = _bare_backend()
    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    monkeypatch.setattr(b, "_get_kokoro",
                        lambda: pytest.fail("must not build the engine without the extra"))
    monkeypatch.setattr(mod, "_play_wav_bytes",
                        lambda data: pytest.fail("must not reach playback"))
    monkeypatch.setattr(mod.subprocess, "Popen",
                        lambda *a, **k: pytest.fail("must not silently fall back to say"))
    with pytest.raises(RuntimeError) as ei:
        b.run("hi", "af_heart", 200)
    assert "kokoro" in str(ei.value).lower()


def test_run_never_returns_none_on_kokoro_failure(monkeypatch):
    # Speaker dereferences proc.terminate()/proc.wait(); a None handle would crash.
    b = _bare_backend()

    class BoomEngine:
        def wav_bytes(self, *a):
            raise RuntimeError("synth blew up")

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(b, "_get_kokoro", lambda: BoomEngine())
    with pytest.raises(RuntimeError):
        b.run("hi", "af_heart", 200)


# ---------------------------------------------------------------------------
# _play_wav_bytes + _AfplayHandle
# ---------------------------------------------------------------------------

def test_play_wav_bytes_writes_temp_wav_and_spawns_afplay(monkeypatch, tmp_path):
    _use_tmp_tempdir(monkeypatch, tmp_path)
    captured = _patch_popen(monkeypatch)

    handle = mod._play_wav_bytes(b"RIFF....WAVE")
    path = captured["argv"][1]
    assert captured["argv"][0] == "afplay"
    assert os.path.basename(path).startswith("sonara-tts-")
    assert path.endswith(".wav")
    assert os.path.exists(path)
    with open(path, "rb") as f:
        assert f.read() == b"RIFF....WAVE"
    for attr in ("wait", "terminate", "poll"):
        assert callable(getattr(handle, attr))
    assert hasattr(handle, "returncode")


def test_handle_wait_completion_unlinks_and_returncode_zero(monkeypatch, tmp_path):
    _use_tmp_tempdir(monkeypatch, tmp_path)
    captured = _patch_popen(monkeypatch, proc=FakePopen(exit_code=0))

    handle = mod._play_wav_bytes(b"data")
    path = captured["argv"][1]
    assert os.path.exists(path)

    rc = handle.wait()
    assert rc == 0
    assert handle.returncode == 0
    assert not os.path.exists(path)


def test_handle_terminate_reaps_unlinks_and_returncode_nonzero(monkeypatch, tmp_path):
    _use_tmp_tempdir(monkeypatch, tmp_path)
    proc = FakePopen()
    captured = _patch_popen(monkeypatch, proc=proc)

    handle = mod._play_wav_bytes(b"data")
    path = captured["argv"][1]

    handle.terminate()
    assert "terminate" in proc.calls            # SIGTERM sent
    assert "wait" in proc.calls                 # child reaped before returning
    assert not os.path.exists(path)             # temp WAV cleaned up
    assert handle.returncode != 0               # interrupted -> Speaker reports not-completed
    assert handle.returncode != 1               # real -15 stands, not coerced


def test_play_wav_bytes_spawn_failure_unlinks_and_reraises(monkeypatch, tmp_path):
    _use_tmp_tempdir(monkeypatch, tmp_path)
    captured = _patch_popen(monkeypatch, raises=FileNotFoundError("no afplay"))

    with pytest.raises(FileNotFoundError):
        mod._play_wav_bytes(b"data")
    # The just-written temp WAV must not leak when spawn fails.
    path = captured["argv"][1]
    assert not os.path.exists(path)


def test_play_wav_bytes_write_failure_unlinks_and_reraises(monkeypatch, tmp_path):
    # A failure writing the WAV (e.g. disk full) must not leak the temp file or
    # spawn afplay against an unwritten one — the docstring promises "never leak".
    _use_tmp_tempdir(monkeypatch, tmp_path)
    monkeypatch.setattr(mod.subprocess, "Popen",
                        lambda *a, **k: pytest.fail("afplay spawned despite a write failure"))

    def boom_write(fd, data):
        raise OSError("disk full")
    monkeypatch.setattr(mod.os, "write", boom_write)

    with pytest.raises(OSError):
        mod._play_wav_bytes(b"data")
    assert list(tmp_path.glob("sonara-tts-*.wav")) == []


def test_wait_timeout_propagates_and_does_not_unlink(monkeypatch, tmp_path):
    _use_tmp_tempdir(monkeypatch, tmp_path)
    proc = FakePopen(wait_exc=subprocess.TimeoutExpired(cmd="afplay", timeout=0.1))
    captured = _patch_popen(monkeypatch, proc=proc)

    handle = mod._play_wav_bytes(b"data")
    path = captured["argv"][1]

    with pytest.raises(subprocess.TimeoutExpired):
        handle.wait(timeout=0.1)
    # afplay is still reading the file — cleanup must NOT fire on a timeout.
    assert os.path.exists(path)


def test_poll_observes_exit_runs_cleanup_idempotently(monkeypatch, tmp_path):
    _use_tmp_tempdir(monkeypatch, tmp_path)
    proc = FakePopen(poll_seq=[None, 0])
    captured = _patch_popen(monkeypatch, proc=proc)

    handle = mod._play_wav_bytes(b"data")
    path = captured["argv"][1]

    assert handle.poll() is None        # still playing
    assert os.path.exists(path)
    assert handle.poll() == 0           # exited -> cleanup
    assert not os.path.exists(path)
    # Idempotent: a repeat poll and a trailing wait must not double-unlink/raise.
    assert handle.poll() == 0
    handle.wait()


# ---------------------------------------------------------------------------
# list_voices() Kokoro gating
# ---------------------------------------------------------------------------

def test_list_voices_appends_kokoro_only_when_installed(monkeypatch):
    listing = "Samantha   en_US  # hi\n"
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)  # isolate the is_installed gate

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    voices = MacTtsBackend.__new__(MacTtsBackend).list_voices()
    assert "Samantha" in voices
    assert set(kokoro.VOICES) <= set(voices)

    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    voices = MacTtsBackend.__new__(MacTtsBackend).list_voices()
    assert voices == ["Samantha"]
    assert not any(v in voices for v in kokoro.VOICES)


def test_list_voices_survives_say_failure(monkeypatch):
    def boom(*a, **k):
        raise FileNotFoundError()
    monkeypatch.setattr(mod.subprocess, "check_output", boom)
    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    voices = MacTtsBackend.__new__(MacTtsBackend).list_voices()  # must not raise
    assert "af_heart" in voices


def test_list_voices_lists_kokoro_when_venv_provisioned_without_extra(monkeypatch):
    # The CLI runs on system python where the [kokoro] extra is NOT importable
    # (is_installed False), but the daemon synthesizes via the provisioned venv.
    # list_voices must still advertise the neural voices so they are discoverable
    # (gate on neural_enabled, not just this interpreter's is_installed).
    listing = "Samantha   en_US  # hi\n"
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    monkeypatch.setattr(kp, "neural_enabled", lambda: True)
    voices = MacTtsBackend.__new__(MacTtsBackend).list_voices()
    assert "af_heart" in voices
    assert set(kokoro.VOICES) <= set(voices)


# ---------------------------------------------------------------------------
# stale-WAV sweep (crash recovery, #26 parity)
# ---------------------------------------------------------------------------

def test_init_sweeps_stale_wavs_on_startup(monkeypatch):
    called = {"n": 0}
    monkeypatch.setattr(mod, "_sweep_stale_wavs", lambda *a, **k: called.__setitem__("n", called["n"] + 1))
    MacTtsBackend()
    assert called["n"] == 1


def test_sweep_stale_wavs_deletes_old_keeps_fresh_and_foreign(monkeypatch, tmp_path):
    _use_tmp_tempdir(monkeypatch, tmp_path)
    old = tmp_path / "sonara-tts-old.wav"
    fresh = tmp_path / "sonara-tts-fresh.wav"
    foreign = tmp_path / "other.wav"
    for p in (old, fresh, foreign):
        p.write_bytes(b"x")
    old_mtime = os.path.getmtime(old) - 10_000  # well beyond 300s
    os.utime(old, (old_mtime, old_mtime))

    mod._sweep_stale_wavs(max_age_s=300.0)

    assert not old.exists()       # stale ours -> removed
    assert fresh.exists()         # recent ours -> kept (may still be playing)
    assert foreign.exists()       # not our prefix -> never touched


def test_tmp_prefix_matches_windows_for_cross_sweep():
    # Byte-identical to the Windows prefix so either backend's startup sweep
    # reclaims the other's crash-leaked WAVs (#26). Pin against the Windows value
    # itself (not just a literal) so drift in EITHER backend fails this test.
    from sonara.platform.windows import tts as wtts  # imports cleanly on macOS (lazy winrt)
    assert mod._TMP_PREFIX == wtts._TMP_PREFIX == "sonara-tts-"
