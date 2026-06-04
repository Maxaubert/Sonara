import threading
import time

from echo.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def test_speak_loop_speaks_queued_item_then_stops():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose", text="hello world", is_decision=False))

    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    try:
        deadline = time.time() + 2.0
        while time.time() < deadline and not speaker.spoken:
            time.sleep(0.01)
        assert speaker.spoken == ["hello world"]
    finally:
        daemon.stop()
        t.join(timeout=2.0)
    assert not t.is_alive()


def test_speak_loop_idles_when_queue_empty_then_stops():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    time.sleep(0.05)
    assert speaker.spoken == []
    daemon.stop()
    t.join(timeout=2.0)
    assert not t.is_alive()
