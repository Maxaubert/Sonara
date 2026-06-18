"""The speak thread must survive ANY exception in its loop body — a crash in
pop_next/note_spoken/etc. previously killed the thread permanently (daemon alive,
earcons firing, but mute forever until a restart). Regression guard."""
from tests.daemon_helpers import make_daemon


def test_speak_loop_survives_internal_exception(monkeypatch):
    daemon = make_daemon()[0]
    seen = []

    def boom_first_then_stop():
        seen.append(1)
        if len(seen) == 1:
            raise RuntimeError("boom in the loop body")   # iteration 1 crashes
        daemon._running.clear()                            # iteration 2: end loop

    monkeypatch.setattr(daemon, "_speak_loop_once", boom_first_then_stop)
    daemon._running.set()
    daemon._speak_loop()   # must return normally despite the iteration-1 raise
    assert len(seen) >= 2  # the loop kept going after the exception


def test_speak_loop_once_speaks_and_notes(monkeypatch):
    # The extracted body still does the normal work: speak then note_spoken.
    daemon, queue, speaker, sessions, config = make_daemon()
    noted = []
    monkeypatch.setattr(daemon, "note_spoken", lambda item, completed: noted.append(completed))
    daemon._enqueue("fg", "prose", "hello", False)
    daemon._speak_loop_once()
    assert noted == [True]          # FakeSpeaker.speak returns True (completed)


def test_speak_thread_keeps_speaking_after_a_bad_note_spoken(monkeypatch):
    # End-to-end: note_spoken raises on the first item; the second item must
    # still be spoken (thread did not die).
    import threading
    import time
    daemon, queue, speaker, sessions, config = make_daemon()
    n = {"calls": 0}

    def flaky_note(item, completed):
        n["calls"] += 1
        if n["calls"] == 1:
            raise RuntimeError("boom")

    monkeypatch.setattr(daemon, "note_spoken", flaky_note)
    daemon._enqueue("fg", "prose", "first", False)
    daemon._enqueue("fg", "prose", "second", False)
    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    deadline = time.time() + 3.0
    while time.time() < deadline and n["calls"] < 2:
        time.sleep(0.02)
    daemon.stop()
    t.join(timeout=1.0)
    assert n["calls"] >= 2          # the second item reached note_spoken -> survived


# ---------------------------------------------------------------------------
# A swallowed speak() exception used to be a SILENT no-op (the worst outcome for
# an eyes-free user — e.g. a Kokoro voice synced from a box with the [kokoro]
# extra to one without it). The loop must still survive, but now it also signals
# the failure audibly (error earcon) and logs it. (#41)
# ---------------------------------------------------------------------------

def _raise(exc):
    def _boom(*a, **k):
        raise exc
    return _boom


def test_speak_failure_fires_error_earcon_and_notes_not_completed(monkeypatch):
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    noted = []
    monkeypatch.setattr(daemon, "note_spoken", lambda item, completed: noted.append(completed))
    monkeypatch.setattr(speaker, "speak", _raise(RuntimeError("kokoro extra not installed")))
    daemon._enqueue("fg", "prose", "hello", False)

    daemon._speak_loop_once()                    # exception contained, must not raise

    assert speaker.earcons == ["error"]          # eyes-free user hears the failure
    assert noted == [False]                       # still marked not-completed (unchanged)


def test_speak_failure_on_pause_exempt_cue_fires_error_earcon(monkeypatch):
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    monkeypatch.setattr(speaker, "speak", _raise(RuntimeError("synth blew up")))
    daemon._paused.set()
    daemon._enqueue("fg", "prose", "Paused.", False, pause_exempt=True)

    daemon._speak_loop_once()                    # paused-branch failure, contained

    assert speaker.earcons == ["error"]


def test_cancelled_utterance_does_not_fire_error_earcon(monkeypatch):
    # speak() returning False is an INTERRUPT (terminate), not an error — it must
    # NOT fire the error earcon. Only a raised exception is an error.
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    speaker.complete = False                     # next speak() reports not-completed
    daemon._enqueue("fg", "prose", "hello", False)

    daemon._speak_loop_once()

    assert speaker.earcons == []                 # no false-positive error signal


def test_error_earcon_failure_is_contained(monkeypatch):
    # If signaling the error itself raises, the loop must still not die.
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    monkeypatch.setattr(speaker, "speak", _raise(RuntimeError("synth blew up")))
    monkeypatch.setattr(speaker, "earcon", _raise(RuntimeError("earcon backend down")))
    daemon._enqueue("fg", "prose", "hello", False)

    daemon._speak_loop_once()                    # must return normally despite both raising
