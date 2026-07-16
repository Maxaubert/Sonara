from tests.daemon_helpers import make_daemon
from sonara.platform.windows.ducking import NullDucker


def test_daemon_defaults_to_null_ducker_when_none_passed():
    from sonara.daemon import SpeechDaemon
    from tests.daemon_helpers import FakeSpeaker
    from sonara.sessions import SessionManager
    from sonara.config import DEFAULTS
    d = SpeechDaemon(FakeSpeaker(), SessionManager(), dict(DEFAULTS))
    assert isinstance(d.ducker, NullDucker)


def test_make_daemon_injects_a_fake_ducker():
    daemon, *_ = make_daemon(foreground="fg")
    assert hasattr(daemon.ducker, "duck_calls")
    assert daemon.ducker.duck_calls == [] and daemon.ducker.restore_calls == 0


from sonara.protocol import MsgType, PROTOCOL_VERSION
from sonara.config import DEFAULTS


def test_config_defaults_have_audio_control_off_and_duck_level_20():
    assert DEFAULTS["audio_control"] is False
    assert DEFAULTS["duck_level"] == 20


def test_set_audio_control_on_persists_and_cues(monkeypatch):
    saved = {}
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: saved.update(c))
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL,
                           "enabled": True})
    assert daemon.config["audio_control"] is True
    assert saved.get("audio_control") is True
    # a spoken confirmation cue was queued on the CONTROL channel
    from sonara.router import CONTROL
    texts = [it.text for it in daemon.router.channel(CONTROL).items]
    assert any("Audio control on" in t for t in texts)


def test_set_audio_control_off_while_ducked_restores_now(monkeypatch):
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: None)
    daemon, *_ = make_daemon(foreground="fg")
    daemon.config["audio_control"] = True
    daemon.ducker._ducked = True               # pretend currently ducked
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL,
                           "enabled": False})
    assert daemon.config["audio_control"] is False
    assert daemon.ducker.restore_calls == 1


def test_set_duck_level_clamps_and_persists(monkeypatch):
    saved = {}
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: saved.update(c))
    daemon, *_ = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL,
                           "level": 150})
    assert daemon.config["duck_level"] == 100   # clamped
    assert saved.get("duck_level") == 100
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL,
                           "level": -5})
    assert daemon.config["duck_level"] == 0
    assert saved.get("duck_level") == 0


def test_set_audio_control_missing_enabled_is_noop(monkeypatch):
    saved = {}
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: saved.update(c))
    daemon, *_ = make_daemon(foreground="fg")
    # malformed message with no "enabled" key must not persist or flip the default
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL})
    assert daemon.config["audio_control"] is False   # unchanged from default
    assert saved == {}                               # save_config never called


def test_set_duck_level_reapplies_when_ducked(monkeypatch):
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: None)
    daemon, *_ = make_daemon(foreground="fg")
    daemon.config["audio_control"] = True   # must be on for re-duck to fire
    daemon.ducker._ducked = True
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL,
                           "level": 35})
    assert daemon.ducker.restore_calls == 1                 # restored then re-ducked
    assert daemon.ducker.duck_calls[-1][1] == 35


# ---------------------------------------------------------------------------
# Task 4: speak-loop duck/restore hooks
# ---------------------------------------------------------------------------
from sonara.queue import SpeechItem


def _prose_item(session, text):
    return SpeechItem(id=0, session=session, kind="prose", text=text, is_decision=False)


def test_no_duck_when_audio_control_off():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.config["audio_control"] = False
    queue.enqueue(_prose_item("fg", "Hello."))
    daemon._speak_loop_once()                       # speaks the item
    assert daemon.ducker.duck_calls == []


def test_duck_once_then_restore_only_at_global_idle():
    # The hold/no-flap behavior: many queued items => exactly ONE duck and ONE
    # restore (at global idle), not one per item. (Restore fires only when
    # next_item() returns None, which is also true across multiple sessions, since
    # the idle condition is global -- so one session proves the mechanism without
    # the session-change announcements that would make the count non-deterministic.)
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.config["audio_mode"] = "duck"
    queue.enqueue(_prose_item("fg", "One."))
    queue.enqueue(_prose_item("fg", "Two."))
    queue.enqueue(_prose_item("fg", "Three."))
    for _ in range(3):                               # drain every queued item
        daemon._speak_loop_once()
    assert len(daemon.ducker.duck_calls) == 1        # ducked once at first speak
    assert daemon.ducker.restore_calls == 0          # still speaking -> held
    daemon._speak_loop_once()                        # next_item() now None -> idle
    assert daemon.ducker.restore_calls == 1          # restored only at global idle


def test_duck_excludes_daemon_and_earcon_pids():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.config["audio_mode"] = "duck"
    speaker._earcon_pids = [4242]                    # see Speaker.earcon_pids() below
    queue.enqueue(_prose_item("fg", "Hi."))
    daemon._speak_loop_once()
    import os
    exclude = daemon.ducker.duck_calls[0][0]
    assert os.getpid() in exclude and 4242 in exclude


def test_stop_restores_if_ducked():
    daemon, *_ = make_daemon(foreground="fg")
    daemon.ducker._ducked = True
    daemon.stop()
    assert daemon.ducker.restore_calls == 1


# ---------------------------------------------------------------------------
# New tests: pause-branch restore + SET_DUCK_LEVEL guard
# ---------------------------------------------------------------------------

def test_paused_branch_restores_if_ducked():
    """The paused branch of _speak_loop_once must call restore() when ducked."""
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.ducker._ducked = True
    daemon._paused.set()
    daemon._speak_loop_once()
    assert daemon.ducker.restore_calls >= 1


def test_set_duck_level_does_not_reduck_when_audio_control_off(monkeypatch):
    """SET_DUCK_LEVEL must not re-duck when audio_control is off."""
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: None)
    daemon, *_ = make_daemon(foreground="fg")
    # audio_control defaults to False; leave it that way
    daemon.ducker._ducked = True
    daemon.handle_message({"v": 1, "type": MsgType.SET_DUCK_LEVEL, "level": 30})
    # The restore may happen (ducker was ducked), but no NEW duck call
    assert daemon.ducker.duck_calls == []
