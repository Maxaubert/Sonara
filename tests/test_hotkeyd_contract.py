"""The daemon must correctly handle every protocol command the system emits --
whether from a hotkey (keymap.ACTION_MESSAGES) or the CLI. Feeding each command
straight into handle_message must produce the intended effect, proving the bytes
the hotkeyd / CLI send are real protocol commands.

Note: stop/skip/repeat/cycle_verbosity/reread_options/jump_decision/catch_up are no
longer hotkey actions (removed from ACTION_MESSAGES), but the daemon still handles
those protocol commands (stop/skip/repeat ship via the CLI), so they are exercised
here with literal messages."""

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


def test_cycle_verbosity_message_advances():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg({"type": "cycle_verbosity"}))
    assert config["verbosity"] == "medium"


def test_reread_options_message_works():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._options["fg"] = "Option 1: A."
    daemon.handle_message(_msg({"type": "reread_options"}))
    assert queue.pop_next().text == "Option 1: A."


def test_jump_decision_message_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg({"type": "jump_decision"}))
    assert speaker.cancels == 1


def test_catch_up_message_replays_unheard_backlog():
    # catch_up now replays unheard history rather than discarding the queue.
    from sonara.protocol import MsgType, PROTOCOL_VERSION
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE,
                           "session": "fg", "delta": "Unheard item. ",
                           "index": 0, "final": True})
    daemon.handle_message(_msg({"type": "catch_up"}))
    texts = [queue.pop_next().text for _ in range(len(queue))]
    assert "Unheard item." in texts
