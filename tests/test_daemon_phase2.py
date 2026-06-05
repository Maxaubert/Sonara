from sonari.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


# ---------------------------------------------------------------------------
# Task 2: relative rate (SET_RATE delta)
# ---------------------------------------------------------------------------

def test_set_rate_delta_increments_and_announces():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 200
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=25))
    assert config["rate"] == 225
    assert speaker.rates[-1] == 225
    # the confirmation is enqueued for the foreground session
    item = queue.pop_next()
    assert item is not None
    assert item.text == "Rate 225."
    assert item.session == "fg"
    assert item.is_decision is False


def test_set_rate_delta_negative_decrements():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 200
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=-25))
    assert config["rate"] == 175
    assert speaker.rates[-1] == 175


def test_set_rate_delta_clamps_at_max():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 390
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=25))
    assert config["rate"] == 400
    assert speaker.rates[-1] == 400


def test_set_rate_delta_clamps_at_min():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["rate"] = 110
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=-25))
    assert config["rate"] == 100
    assert speaker.rates[-1] == 100


def test_set_rate_absolute_still_works():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", rate=300))
    assert config["rate"] == 300
    assert speaker.rates[-1] == 300
    # absolute path does NOT enqueue a confirmation (unchanged behavior)
    assert len(queue) == 0


def test_set_rate_delta_no_foreground_still_updates_rate():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    config["rate"] = 200
    daemon.handle_message(_msg(MsgType.SET_RATE, delta=25))
    assert config["rate"] == 225
    assert speaker.rates[-1] == 225
    # no foreground => nothing enqueued
    assert len(queue) == 0


# ---------------------------------------------------------------------------
# Task 3: cycle_verbosity
# ---------------------------------------------------------------------------

def test_cycle_verbosity_everything_to_medium():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY, "fg"))
    assert config["verbosity"] == "medium"
    item = queue.pop_next()
    assert item is not None
    assert item.text == "Verbosity medium."
    assert item.session == "fg"


def test_cycle_verbosity_medium_to_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY, "fg"))
    assert config["verbosity"] == "quiet"
    assert queue.pop_next().text == "Verbosity quiet."


def test_cycle_verbosity_quiet_wraps_to_everything():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY, "fg"))
    assert config["verbosity"] == "everything"
    assert queue.pop_next().text == "Verbosity everything."


def test_cycle_verbosity_unknown_current_defaults_to_everything_step():
    # an out-of-range stored value is treated as the start of the cycle
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["verbosity"] = "bogus"
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY, "fg"))
    assert config["verbosity"] == "everything"


def test_cycle_verbosity_no_foreground_still_persists():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground=None)
    daemon.handle_message(_msg(MsgType.CYCLE_VERBOSITY))
    assert config["verbosity"] == "medium"
    assert len(queue) == 0
