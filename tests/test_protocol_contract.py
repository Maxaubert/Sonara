"""The daemon must correctly handle every protocol command the system emits --
whether from a hotkey (keymap.ACTION_MESSAGES) or the CLI. Feeding each command
straight into handle_message must produce the intended effect, proving the bytes
the hotkey listener / CLI send are real protocol commands.

Note: stop/skip/repeat are not hotkey actions, but the CLI sends them, so they
are exercised here with literal messages."""

from sonara import keymap
from sonara.protocol import MsgType
from tests.daemon_helpers import make_daemon


def _msg(action_message, session="fg"):
    d = dict(action_message)
    d["session"] = session
    return d


def test_all_action_messages_are_known_msgtypes():
    valid_types = {
        v for k, v in vars(MsgType).items()
        if not k.startswith("_") and isinstance(v, str)
    }
    for action, message in keymap.ACTION_MESSAGES.items():
        assert message["type"] in valid_types, action


def test_stop_message_clears_and_cancels():
    from sonara.queue import SpeechItem
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose",
                             text="x", is_decision=False))
    daemon.handle_message(_msg({"type": "stop"}))
    assert len(queue) == 0
    assert speaker.cancels == 1


def test_skip_message_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg({"type": "skip"}))
    assert speaker.cancels == 1


def test_repeat_message_reenqueues_last_spoken():
    from sonara.protocol import MsgType, PROTOCOL_VERSION
    # Repeat is now history-based: enqueue prose first, drain it, then repeat.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE,
                           "session": "fg", "delta": "Hello. ", "index": 0,
                           "final": True})
    item = queue.pop_next()
    daemon.note_spoken(item, True)
    daemon.handle_message(_msg({"type": "repeat"}))
    assert queue.pop_next().text == "Hello."


def test_faster_message_bumps_rate_by_25():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 200
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["faster"]))
    assert config["rate"] == 225


def test_slower_message_drops_rate_by_25():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 200
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["slower"]))
    assert config["rate"] == 175


def test_removed_dead_feature_messages_are_ignored():
    # DC1/DC5: jump_decision, catch_up, reread_options, cycle_verbosity and the
    # pre-#92 set_audio_control had no producer and were deleted. A stale client
    # sending one gets nothing: no reply, no cut, no config change, nothing queued.
    daemon, queue, speaker, sessions, config = make_daemon(
        verbosity="everything", foreground="fg")
    for t in ("jump_decision", "catch_up", "reread_options", "cycle_verbosity",
              "set_audio_control"):
        assert not hasattr(MsgType, t.upper())
        assert daemon.handle_message(_msg({"type": t, "enabled": True})) is None
    assert speaker.cancels == 0
    assert config["verbosity"] == "everything"
    assert config.get("audio_mode", "off") == "off"
    assert len(queue) == 0
