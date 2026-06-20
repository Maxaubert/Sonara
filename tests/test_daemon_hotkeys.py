"""The daemon owns the in-process hotkey thread: run() starts it, stop() stops it,
and a fire is routed through the same handle_message() as a socket command."""
import time

from tests.daemon_helpers import make_daemon


def _wait_until(pred, timeout=2.0):
    deadline = time.time() + timeout
    while time.time() < deadline and not pred():
        time.sleep(0.005)
    return pred()


class _FakeHotkey:
    def __init__(self):
        self.started = None
        self.stopped = False
        self.reloaded = None

    def start(self, dispatch):
        self.started = dispatch

    def stop(self):
        self.stopped = True

    def reload(self, dispatch):
        self.reloaded = dispatch


class _FakePlatform:
    def __init__(self):
        self.hotkey = _FakeHotkey()


def test_start_hotkeys_passes_a_dispatch_callback(monkeypatch):
    pb = _FakePlatform()
    monkeypatch.setattr("sonari.platform.get_platform", lambda: pb)
    daemon = make_daemon()[0]
    daemon._start_hotkeys()
    assert callable(pb.hotkey.started)


def test_dispatch_routes_through_handle_message(monkeypatch):
    pb = _FakePlatform()
    monkeypatch.setattr("sonari.platform.get_platform", lambda: pb)
    daemon = make_daemon()[0]
    daemon._start_hotkeys()
    handled = []
    monkeypatch.setattr(daemon, "handle_message", lambda m: handled.append(m))
    pb.hotkey.started({"type": "skip"})       # simulate a hotkey fire
    assert handled == [{"type": "skip"}]


def test_stop_stops_the_hotkey_listener(monkeypatch):
    pb = _FakePlatform()
    monkeypatch.setattr("sonari.platform.get_platform", lambda: pb)
    daemon = make_daemon()[0]
    daemon._stop_hotkeys()
    assert pb.hotkey.stopped is True


def test_dispatch_hotkey_holds_the_lock_like_the_socket_path(monkeypatch):
    """A hotkey fire mutates shared daemon state (queue/history/config) via
    handle_message; it MUST hold self._lock the way the socket path (_handle_conn)
    does, or it races the speak loop -> 'list changed size' crash / corruption.
    Regression for the unlocked-dispatch concurrency bug (#5)."""
    pb = _FakePlatform()
    monkeypatch.setattr("sonari.platform.get_platform", lambda: pb)
    daemon = make_daemon()[0]
    locked_during_call = []
    real = daemon.handle_message

    def spy(msg):
        locked_during_call.append(daemon._lock.locked())
        return real(msg)

    monkeypatch.setattr(daemon, "handle_message", spy)
    daemon._dispatch_hotkey({"type": "skip"})
    assert locked_during_call == [True]


def test_debounce_suppresses_rapid_repeat_of_same_toggle():
    """A second next_session/pause/mute within the debounce window is ignored; one after the
    window passes is honored."""
    from sonari.protocol import MsgType
    from sonari.daemon import _HOTKEY_DEBOUNCE_S
    daemon = make_daemon()[0]
    assert daemon._debounce_suppress(MsgType.NEXT_SESSION, 100.0) is False   # first fires
    assert daemon._debounce_suppress(MsgType.NEXT_SESSION, 100.10) is True   # +100ms: dropped
    assert daemon._debounce_suppress(MsgType.NEXT_SESSION, 100.0 + _HOTKEY_DEBOUNCE_S + 0.01) is False  # window passed


def test_debounce_is_per_action_not_global():
    """Debouncing next_session must not suppress a different toggle pressed right after."""
    from sonari.protocol import MsgType
    daemon = make_daemon()[0]
    assert daemon._debounce_suppress(MsgType.NEXT_SESSION, 50.0) is False
    assert daemon._debounce_suppress(MsgType.MUTE, 50.05) is False   # different action: not debounced


def test_nav_and_repeat_are_not_debounced():
    """Directional/idempotent keys pass through every time (rapid nav is intentional)."""
    from sonari.protocol import MsgType
    daemon = make_daemon()[0]
    assert daemon._debounce_suppress(MsgType.NAV, 10.0) is False
    assert daemon._debounce_suppress(MsgType.NAV, 10.01) is False   # not suppressed
    assert daemon._debounce_suppress(MsgType.REPEAT, 10.0) is False
    assert daemon._debounce_suppress(MsgType.REPEAT, 10.01) is False


def test_one_bad_hotkey_does_not_raise(monkeypatch):
    pb = _FakePlatform()
    monkeypatch.setattr("sonari.platform.get_platform", lambda: pb)
    daemon = make_daemon()[0]
    monkeypatch.setattr(daemon, "handle_message",
                        lambda m: (_ for _ in ()).throw(RuntimeError("boom")))
    daemon._dispatch_hotkey({"type": "stop"})   # swallowed, no raise


def test_reload_keymap_delegates_to_backend_reload(monkeypatch):
    # RELOAD_KEYMAP delegates to the platform backend's reload() seam (Windows:
    # thread-joined stop+start; macOS: rewrite resolved + reload hotkeyd). The
    # daemon passes its dispatch callback through.
    pb = _FakePlatform()
    monkeypatch.setattr("sonari.platform.get_platform", lambda: pb)
    monkeypatch.setattr("os.path.exists", lambda p: False)   # no kill-switch flag
    monkeypatch.delenv("SONARI_DISABLE_HOTKEYS", raising=False)
    daemon = make_daemon(foreground="fg")[0]
    daemon.handle_message({"type": "reload_keymap"})
    # The reload runs on a short-lived thread (off the daemon lock), so wait for it.
    assert _wait_until(lambda: pb.hotkey.reloaded is not None)
    assert callable(pb.hotkey.reloaded)   # backend.reload(dispatch) was invoked


def test_reload_keymap_honors_kill_switch(monkeypatch):
    # With the kill switch set, reload must NOT re-register hotkeys; it just stops.
    pb = _FakePlatform()
    monkeypatch.setattr("sonari.platform.get_platform", lambda: pb)
    monkeypatch.setenv("SONARI_DISABLE_HOTKEYS", "1")
    daemon = make_daemon(foreground="fg")[0]
    daemon.handle_message({"type": "reload_keymap"})
    assert _wait_until(lambda: pb.hotkey.stopped)
    assert pb.hotkey.reloaded is None
    assert pb.hotkey.stopped is True
