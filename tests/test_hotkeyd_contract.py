"""Every keymap.ACTION_MESSAGES dict must be a valid speechd command: feeding
it straight into a daemon's handle_message must produce the intended effect.
This proves the bytes the Swift hotkeyd sends are real protocol commands."""

from sonari import keymap
from sonari.protocol import MsgType
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
    from sonari.queue import SpeechItem
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose",
                             text="x", is_decision=False))
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["stop"]))
    assert len(queue) == 0
    assert speaker.cancels == 1


def test_skip_message_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["skip"]))
    assert speaker.cancels == 1


def test_repeat_message_reenqueues_last_spoken():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._last_spoken = "Hello."
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["repeat"]))
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
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["cycle_verbosity"]))
    assert config["verbosity"] == "medium"


def test_reread_options_message_works():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._last_options = "Option 1: A."
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["reread_options"]))
    assert queue.pop_next().text == "Option 1: A."


def test_jump_decision_message_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["jump_decision"]))
    assert speaker.cancels == 1


def test_catch_up_message_clears_and_cancels():
    from sonari.queue import SpeechItem
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose",
                             text="x", is_decision=False))
    daemon.handle_message(_msg(keymap.ACTION_MESSAGES["catch_up"]))
    assert len(queue) == 0
    assert speaker.cancels == 1
