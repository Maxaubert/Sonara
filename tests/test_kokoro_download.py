"""Kokoro model download (M2, E11, upstream #53): a timeout on the fetch, the
fetch outside the engine lock, a 'downloading' status once, and a failed fetch
remembered for a cool-down instead of retried on every utterance."""
from __future__ import annotations

import threading
import time

import pytest

from sonara import kokoro


class _FakeK:
    def create(self, text, voice, speed, lang):
        import numpy as np
        return np.ones(4, dtype=np.float32), 24000


class _Resp:
    """A urlopen() response: read(n) in chunks, usable as a context manager."""

    def __init__(self, payload: bytes):
        self._buf = payload

    def read(self, n=-1):
        if n is None or n < 0:
            n = len(self._buf)
        out, self._buf = self._buf[:n], self._buf[n:]
        return out

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


def test_download_passes_a_socket_timeout(monkeypatch, tmp_path):
    seen = {}

    def fake_urlopen(url, timeout=None):
        seen["timeout"] = timeout
        return _Resp(b"x" * 10)

    monkeypatch.setattr(kokoro.urllib.request, "urlopen", fake_urlopen)
    dest = tmp_path / "model.onnx"
    kokoro._download("https://example.invalid/m", dest)
    assert seen["timeout"] and seen["timeout"] <= 60   # a stalled read gives up
    assert dest.read_bytes() == b"x" * 10
    assert not (tmp_path / "model.onnx.tmp").exists()


def test_download_failure_leaves_no_partial_file(monkeypatch, tmp_path):
    def fake_urlopen(url, timeout=None):
        raise OSError("timed out")

    monkeypatch.setattr(kokoro.urllib.request, "urlopen", fake_urlopen)
    dest = tmp_path / "model.onnx"
    with pytest.raises(OSError):
        kokoro._download("https://example.invalid/m", dest)
    assert not dest.exists() and not (tmp_path / "model.onnx.tmp").exists()


def test_model_download_runs_outside_the_engine_lock(tmp_path):
    held = []
    eng = kokoro.KokoroEngine(tmp_path, factory=lambda m, v: _FakeK(),
                              ensure=lambda: held.append(eng._lock.locked()))
    eng.synth("hi", "af_heart")
    assert held == [False]


def test_background_download_never_blocks_a_synth_and_reports_once(tmp_path):
    release = threading.Event()
    started = []
    present = [False]
    notices = []

    def ensure():
        started.append(1)
        release.wait(5)
        present[0] = True

    eng = kokoro.KokoroEngine(tmp_path, factory=lambda m, v: _FakeK(),
                              ensure=ensure, background=True,
                              present=lambda: present[0],
                              on_download=lambda: notices.append(1))
    t0 = time.monotonic()
    with pytest.raises(kokoro.KokoroDownloading):
        eng.synth("hi", "af_heart")
    with pytest.raises(kokoro.KokoroDownloading):
        eng.synth("again", "af_heart")         # still downloading: no 2nd fetch
    assert time.monotonic() - t0 < 2.0          # neither call waited for it
    release.set()
    deadline = time.monotonic() + 5
    while not present[0] and time.monotonic() < deadline:
        time.sleep(0.01)
    eng._download_thread.join(5)
    audio, sr = eng.synth("ready now", "af_heart")
    assert sr == 24000
    assert started == [1] and notices == [1]


def test_failed_download_is_remembered_for_a_cooldown(tmp_path):
    calls = []
    clock = [1000.0]

    def ensure():
        calls.append(1)
        raise OSError("network down")

    eng = kokoro.KokoroEngine(tmp_path, factory=lambda m, v: _FakeK(),
                              ensure=ensure, present=lambda: False,
                              clock=lambda: clock[0])
    with pytest.raises(OSError):
        eng.synth("hi", "af_heart")
    with pytest.raises(kokoro.KokoroUnavailable) as ei:
        eng.synth("hi", "af_heart")             # inside the cool-down: no retry
    assert "voices install" in str(ei.value)
    assert calls == [1]
    clock[0] += kokoro.DOWNLOAD_RETRY_S + 1     # cool-down over: try again
    with pytest.raises(OSError):
        eng.synth("hi", "af_heart")
    assert calls == [1, 1]


def test_failure_memo_survives_a_new_engine(tmp_path):
    """The memo lives on disk, so a daemon restart does not retry at once."""
    def ensure():
        raise OSError("network down")

    clock = lambda: 5000.0  # noqa: E731
    eng = kokoro.KokoroEngine(tmp_path, factory=lambda m, v: _FakeK(),
                              ensure=ensure, present=lambda: False, clock=clock)
    with pytest.raises(OSError):
        eng.synth("hi", "af_heart")
    again = kokoro.KokoroEngine(tmp_path, factory=lambda m, v: _FakeK(),
                                ensure=lambda: pytest.fail("retried in cool-down"),
                                present=lambda: False, clock=clock)
    with pytest.raises(kokoro.KokoroUnavailable):
        again.synth("hi", "af_heart")


def test_forced_download_ignores_and_clears_the_memo(tmp_path):
    clock = lambda: 5000.0  # noqa: E731
    fail = [True]

    def ensure():
        if fail[0]:
            raise OSError("network down")

    eng = kokoro.KokoroEngine(tmp_path, factory=lambda m, v: _FakeK(),
                              ensure=ensure, present=lambda: False, clock=clock)
    with pytest.raises(OSError):
        eng.synth("hi", "af_heart")
    fail[0] = False
    eng.download_models(force=True)             # `sonara voices install` path
    assert kokoro.download_failed_at(tmp_path) is None


def test_files_already_present_skip_the_download_and_the_notice(tmp_path):
    notices = []
    eng = kokoro.KokoroEngine(tmp_path, factory=lambda m, v: _FakeK(),
                              ensure=lambda: pytest.fail("downloaded again"),
                              present=lambda: True,
                              on_download=lambda: notices.append(1))
    eng.synth("hi", "af_heart")
    assert notices == []
