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
