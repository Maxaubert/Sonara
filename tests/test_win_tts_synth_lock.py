"""M9: the shared WinRT SpeechSynthesizer is used under a lock. The speak
loop, the preview builder and a settings preview all reach the same backend;
without the lock one thread's voice/rate could land on another's synthesis."""
from __future__ import annotations

import threading
import time

from sonara.platform.windows import tts as wtts


def test_set_voice_rate_and_synthesize_never_interleave(monkeypatch):
    active = [0]
    overlap = []

    class _Op:
        def __init__(self, r):
            self._r = r

        def get(self):
            return self._r

    class _Stream:
        size = 4

        def get_input_stream_at(self, pos):
            return object()

    class FakeSynth:
        class options:
            speaking_rate = 1.0

        def __init__(self):
            self._voice = None

        @property
        def voice(self):
            return self._voice

        @voice.setter
        def voice(self, v):
            active[0] += 1
            if active[0] > 1:
                overlap.append(v)
            self._voice = v

        def synthesize_text_to_stream_async(self, text):
            time.sleep(0.02)            # widen the race window
            active[0] -= 1
            return _Op(_Stream())

    class FakeReader:
        def __init__(self, s):
            pass

        def load_async(self, n):
            return _Op(n)

        def read_bytes(self, buf):
            buf[:] = b"RIFF"

    import winrt.windows.storage.streams as streams
    monkeypatch.setattr(streams, "DataReader", FakeReader)
    b = wtts.WinTtsBackend.__new__(wtts.WinTtsBackend)
    b._kokoro = None
    b._synth = FakeSynth()
    b._synth_lock = threading.Lock()
    monkeypatch.setattr(b, "_resolve_voice", lambda v: v)

    threads = [threading.Thread(target=b._synthesize_wav_blocking,
                                args=("hi", "v{0}".format(i), 200))
               for i in range(6)]
    for t in threads:
        t.start()
    for t in threads:
        t.join(5)
    assert overlap == []


def test_backend_has_a_synth_lock():
    b = wtts.WinTtsBackend()
    assert hasattr(b._synth_lock, "acquire")
