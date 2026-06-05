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


# ---------------------------------------------------------------------------
# Task 4: option caching + reread_options + clearing
# ---------------------------------------------------------------------------

def test_reread_after_choice_reenqueues_same_text():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Pick a color", "options": [{"label": "Red"}, {"label": "Blue"}]},
    ]))
    spoken = queue.pop_next().text  # drain the original CHOICE item
    assert "Option 1: Red." in spoken
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    item = queue.pop_next()
    assert item is not None
    assert item.text == spoken
    assert item.kind == "choice"
    assert item.session == "fg"
    assert item.is_decision is False


def test_reread_with_no_prior_says_nothing_to_repeat():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon._last_options is None
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    item = queue.pop_next()
    assert item is not None
    assert item.text == "No options to repeat."
    assert item.kind == "prose"


def test_reread_no_foreground_is_noop():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon._last_options = "Option 1: Red."
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))
    assert len(queue) == 0


def test_plan_and_permission_also_cache_for_reread():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Do the thing."))
    plan_spoken = queue.pop_next().text
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    assert queue.pop_next().text == plan_spoken

    daemon.handle_message(_msg(MsgType.PERMISSION, "fg", action="run rm -rf"))
    perm_spoken = queue.pop_next().text
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    assert queue.pop_next().text == perm_spoken


def test_flush_clears_option_cache():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Q", "options": [{"label": "A"}]},
    ]))
    queue.pop_next()  # drain
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    assert daemon._last_options is None
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    assert queue.pop_next().text == "No options to repeat."


def test_session_end_clears_option_cache():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Q", "options": [{"label": "A"}]},
    ]))
    queue.pop_next()
    daemon.handle_message(_msg(MsgType.SESSION_END, "fg"))
    assert daemon._last_options is None


# ---------------------------------------------------------------------------
# Task 5: selection cue + immediate warning + multiSelect/>9 notes
# ---------------------------------------------------------------------------

CUE = "Press the option's number to choose, or Escape to cancel."
WARN = "Selecting is immediate."
MULTI = "Select multiple: press each number, or Space on the highlighted item, then Enter to confirm."
OVER9 = "More than nine options; use arrow keys for ten and up."


def _two_option_choice(session="fg"):
    return _msg(MsgType.CHOICE, session, questions=[
        {"question": "Pick a color", "options": [{"label": "Red"}, {"label": "Blue"}]},
    ])


def test_choice_cue_present_at_everything():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_two_option_choice())
    text = queue.pop_next().text
    assert CUE in text


def test_choice_cue_absent_at_medium():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    daemon.handle_message(_two_option_choice())
    text = queue.pop_next().text
    assert CUE not in text
    assert WARN not in text


def test_choice_cue_absent_at_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
    daemon.handle_message(_two_option_choice())
    text = queue.pop_next().text
    assert CUE not in text
    assert WARN not in text


def test_immediate_warning_fires_once_per_session():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_two_option_choice())
    first = queue.pop_next().text
    assert WARN in first
    daemon.handle_message(_two_option_choice())
    second = queue.pop_next().text
    assert CUE in second          # cue still present every time
    assert WARN not in second     # warning only the first time


def test_immediate_warning_independent_per_session():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_two_option_choice("fg"))
    assert WARN in queue.pop_next().text
    # a different foreground session gets its own first-time warning
    sessions.set_foreground("fg2")
    daemon.handle_message(_two_option_choice("fg2"))
    assert WARN in queue.pop_next().text


def test_multiselect_note_present_in_any_mode():
    for verb in ("everything", "medium", "quiet"):
        daemon, queue, speaker, sessions, config = make_daemon(verbosity=verb, foreground="fg")
        daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
            {"question": "Pick some", "multiSelect": True,
             "options": [{"label": "A"}, {"label": "B"}]},
        ]))
        text = queue.pop_next().text
        assert MULTI in text, verb


def test_over_nine_note_present_in_any_mode():
    opts = [{"label": "Opt {0}".format(i)} for i in range(1, 11)]  # 10 options
    for verb in ("everything", "medium", "quiet"):
        daemon, queue, speaker, sessions, config = make_daemon(verbosity=verb, foreground="fg")
        daemon.handle_message(_msg(MsgType.CHOICE, "fg",
                                   questions=[{"question": "Many", "options": opts}]))
        text = queue.pop_next().text
        assert OVER9 in text, verb


def test_permission_gets_cue_but_not_choice_notes_at_everything():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.PERMISSION, "fg", action="run rm -rf"))
    text = queue.pop_next().text
    assert "run rm -rf" in text
    assert CUE in text
    assert MULTI not in text
    assert OVER9 not in text


def test_plan_gets_cue_at_everything_but_not_at_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Do it."))
    assert CUE in queue.pop_next().text

    daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Do it."))
    assert CUE not in queue.pop_next().text


def test_reread_includes_cue_and_notes():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Pick some", "multiSelect": True,
         "options": [{"label": "A"}, {"label": "B"}]},
    ]))
    spoken = queue.pop_next().text
    assert MULTI in spoken and CUE in spoken
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS, "fg"))
    assert queue.pop_next().text == spoken
