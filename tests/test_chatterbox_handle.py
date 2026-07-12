"""Pipelined chatterbox playback handle. All seams injected: no torch, no GPU,
no winsound. A fake synth_one returns marker bytes; a fake play returns a fake
sub-handle recording wait/terminate."""
import threading
import time

from sonara.platform.windows.tts import _ChatterboxHandle


class FakeSub:
    def __init__(self, wav):
        self.wav = wav
        self.returncode = None
        self.terminated = False
        self._done = threading.Event()

    def wait(self, timeout=None):
        # "plays" instantly in tests
        self.returncode = 0
        self._done.set()
        return 0

    def terminate(self):
        self.terminated = True
        self.returncode = 1
        self._done.set()


def _recording_play():
    played = []

    def play(wav):
        sub = FakeSub(wav)
        played.append(sub)
        return sub
    return play, played


def _split(_text, max_chars=280):
    return ["c1", "c2", "c3"]


def test_all_chunks_play_in_order_and_return_complete():
    play, played = _recording_play()
    synthed = []
    h = _ChatterboxHandle("whatever", synth_one=lambda c: (synthed.append(c) or c.encode()),
                          play=play, split=_split)
    rc = h.wait()
    assert rc == 0 and h.returncode == 0
    assert [s.wav for s in played] == [b"c1", b"c2", b"c3"]
    assert synthed == ["c1", "c2", "c3"]


def test_on_play_fires_once_before_first_playback():
    play, played = _recording_play()
    calls = []
    h = _ChatterboxHandle("x", synth_one=lambda c: c.encode(),
                          on_play=lambda: calls.append(1), play=play, split=_split)
    h.wait()
    assert calls == [1]                       # exactly once


def test_terminate_aborts_within_one_chunk():
    # A slow synth_one lets us terminate mid-stream; remaining chunks must not
    # play. synth blocks until released so the abort lands between chunks.
    play, played = _recording_play()
    gate = threading.Event()
    synth_count = {"n": 0}

    def slow_synth(c):
        synth_count["n"] += 1
        if synth_count["n"] == 1:
            return c.encode()                 # first chunk synths fast
        gate.wait(2.0)                         # later chunks stall until released
        return c.encode()

    h = _ChatterboxHandle("x", synth_one=slow_synth, play=play, split=_split)
    t = threading.Thread(target=h.wait)
    t.start()
    time.sleep(0.2)
    h.terminate()
    gate.set()
    t.join(3.0)
    assert not t.is_alive()
    assert h.returncode == 1
    assert len(played) <= 2                    # did not play all three


def test_none_from_synth_one_skips_that_chunk():
    play, played = _recording_play()
    h = _ChatterboxHandle("x", synth_one=lambda c: None if c == "c2" else c.encode(),
                          play=play, split=_split)
    h.wait()
    assert [s.wav for s in played] == [b"c1", b"c3"]   # c2 produced nothing, skipped


def test_empty_text_is_a_clean_noop():
    play, played = _recording_play()
    h = _ChatterboxHandle("", synth_one=lambda c: c.encode(),
                          play=play, split=lambda t, max_chars=280: [])
    assert h.wait() == 0 and played == []


def test_producer_thread_stops_after_wait():
    play, _ = _recording_play()
    h = _ChatterboxHandle("x", synth_one=lambda c: c.encode(), play=play, split=_split)
    h.wait()
    time.sleep(0.1)
    assert h._producer is None or not h._producer.is_alive()   # no leaked thread


def test_play_exception_does_not_leak_thread_or_hang():
    # If _play raises, wait() must still return (not hang), set a non-None
    # returncode, and stop the producer thread (no leak, no spin-loop).
    def boom_play(wav):
        raise OSError("winsound failed")
    # >=4 chunks (queue maxsize 2) so the producer genuinely saturates; without
    # the fix setting abort on the _play exception it would spin-loop forever.
    six = lambda _t, max_chars=280: ["c1", "c2", "c3", "c4", "c5", "c6"]
    h = _ChatterboxHandle("x", synth_one=lambda c: c.encode(),
                          play=boom_play, split=six)
    t = threading.Thread(target=h.wait)
    t.start()
    t.join(3.0)
    assert not t.is_alive()                       # returned, did not hang
    assert h.returncode is not None               # not left as "still running"
    time.sleep(0.2)
    assert h._producer is None or not h._producer.is_alive()   # no leaked producer


def test_split_text_is_exposed_on_chatterbox_module():
    from sonara import chatterbox
    chunks = chatterbox.split_text("One. Two. Three.", max_chars=8)
    assert len(chunks) >= 2 and all(len(c) <= 8 for c in chunks)
