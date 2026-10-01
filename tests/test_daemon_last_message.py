"""One message, always the last (#137): nothing may silently drop the latest
turn. Regressions for the seeded-channel wipe, control cues on the session
channel, a background FLUSH un-pausing the voice (upstream #69), a parked
background digest outliving its session, STOP leaving digests alive, the
router's private fields, _replay's has_decision and the preamble race."""
import re
from pathlib import Path

import sonara.daemon as daemon_module
from sonara.protocol import MsgType, PROTOCOL_VERSION
from sonara.queue import SpeechItem
from sonara.router import CONTROL
from tests.daemon_helpers import make_daemon

_PAD = "This filler sentence carries the turn well past the digest threshold. "


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


def _prose(session, text):
    return _msg(MsgType.PROSE, session, delta=text, index=0, final=True)


def _turn_done(daemon, session):
    daemon.handle_message(_msg(MsgType.EARCON, session, kind="turn_done"))


def _summary_daemon(monkeypatch, foreground="fg"):
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground=foreground)
    config["summary_mode"] = True
    # Deterministic settle: record the window instead of starting a timer.
    monkeypatch.setattr(daemon, "_settle_schedule", lambda session, gen: None)
    return daemon, speaker, sessions, config


def _fire_settle(daemon, session):
    gen = daemon._settle_gen.get(session)
    if gen is not None:
        daemon._settle_fire(session, gen)


def _capture_spawn(daemon, monkeypatch):
    """Record digest dispatches WITHOUT landing them: they stay in flight."""
    calls = []

    def fake(session, gen, text, token=0, leadin=False, seq=None):
        calls.append({"session": session, "gen": gen, "text": text,
                      "token": token, "leadin": leadin, "seq": seq})

    monkeypatch.setattr(daemon, "_start_summary_thread", fake)
    return calls


def _run_worker(daemon, call, digest="The digest."):
    daemon._summarize_fn = lambda text, **kw: digest
    daemon._summary_worker(call["session"], call["gen"], call["text"],
                           call["token"], call["leadin"], call["seq"])


def _pending_texts(daemon, session):
    ch = daemon.router.channels.get(session)
    return [] if ch is None else [it.text for it in ch.pending_items()]


def _short_turn_after_reseed(daemon, session="fg"):
    """Summary mode: FLUSH re-seeds from the persisted digest, then a short
    foreground turn is delivered through _replay(append=True)."""
    daemon.digest_store.set(session, "Old digest.")
    daemon.handle_message(_msg(MsgType.FLUSH, session))
    assert daemon.router.channel(session).seeded is True
    daemon.handle_message(_prose(session, "Short answer here."))
    _turn_done(daemon, session)
    _fire_settle(daemon, session)
    assert _pending_texts(daemon, session) == ["Short answer here."]


# --- seeded-channel wipe (architecture review 0.1) ------------------------

def test_rate_change_keeps_the_unread_short_turn(monkeypatch):
    # The scratchpad repro: pending ['Short answer here.'] became ['Rate 210.'].
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch)
    _short_turn_after_reseed(daemon)
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=10))
    assert _pending_texts(daemon, "fg") == ["Short answer here."]


def test_short_turn_delivery_clears_the_seed_flag(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch)
    _short_turn_after_reseed(daemon)
    assert daemon.router.channel("fg").seeded is False


def test_later_session_content_appends_after_the_short_turn(monkeypatch):
    # Any later session-channel enqueue (setup guidance, a digest) must
    # append, not treat the unread short turn as the placeholder seed.
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch)
    _short_turn_after_reseed(daemon)
    daemon._enqueue("fg", "prose", "Later cue.", False)
    assert _pending_texts(daemon, "fg") == ["Short answer here.", "Later cue."]


# --- F6: control cues go through the CONTROL channel ----------------------

def test_rate_cue_goes_to_the_control_channel_and_keeps_the_seed(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch)
    daemon.digest_store.set("fg", "Old digest.")
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=10))
    ch = daemon.router.channel("fg")
    assert ch.seeded is True and [it.text for it in ch.items] == ["Old digest."]
    assert _pending_texts(daemon, CONTROL) == ["Rate {0}.".format(config["rate"])]


def test_rate_cues_coalesce_to_the_latest_value(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch)
    config["rate"] = 200
    for _ in range(3):
        daemon.handle_message(_msg(MsgType.SET_RATE, "fg", delta=10))
    assert _pending_texts(daemon, CONTROL) == ["Rate 230."]


def test_nothing_to_navigate_is_a_control_cue_and_keeps_the_seed(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch)
    config["summary_mode"] = False
    daemon.digest_store.set("fg", "Old digest.")
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    daemon.handle_message(_msg(MsgType.NAV, to="first"))
    assert daemon.router.channel("fg").seeded is True
    assert _pending_texts(daemon, CONTROL) == ["Nothing to navigate yet."]


# --- F1 / upstream #69: a background FLUSH keeps the pause ----------------

def _paused_while_reading_a(daemon):
    ch = daemon.router.channel("A")
    ch.append(SpeechItem(id=99, session="A", kind="prose", text="a1",
                         is_decision=False))
    ch.turn_done = True
    daemon._speak_loop_once()                    # A reads: engaged session
    daemon.handle_message(_msg(MsgType.PAUSE))
    assert daemon._paused.is_set()


def test_background_prompt_keeps_the_foreground_voice_paused():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    _paused_while_reading_a(daemon)
    daemon.handle_message(_msg(MsgType.SET_FOREGROUND, "B"))
    daemon.handle_message(_msg(MsgType.FLUSH, "B"))
    assert daemon._paused.is_set()               # A's voice stays held


def test_own_prompt_still_auto_resumes_the_paused_voice():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    _paused_while_reading_a(daemon)
    daemon.handle_message(_msg(MsgType.SET_FOREGROUND, "A"))
    daemon.handle_message(_msg(MsgType.FLUSH, "A"))
    assert not daemon._paused.is_set()


def test_unpaused_prompt_still_moves_the_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    daemon.handle_message(_msg(MsgType.SET_FOREGROUND, "B"))
    daemon.handle_message(_msg(MsgType.FLUSH, "B"))
    assert sessions.foreground() == "B"


# --- F2: a parked short background digest is cancellable -------------------

def _park_short_background_turn(daemon, monkeypatch):
    """A's long digest is in flight (seq 0), B's short background turn is
    parked behind it (seq 1)."""
    calls = _capture_spawn(daemon, monkeypatch)
    daemon.handle_message(_prose("A", "Long turn. " + _PAD * 6))
    _turn_done(daemon, "A")
    _fire_settle(daemon, "A")
    assert len(calls) == 1 and calls[0]["seq"] is not None
    daemon.handle_message(_prose("B", "short turn text."))
    _turn_done(daemon, "B")
    _fire_settle(daemon, "B")
    assert daemon._digest_parked                  # B waits behind A's slot
    return calls


def test_parked_digest_never_resurrects_an_ended_session(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch, "A")
    calls = _park_short_background_turn(daemon, monkeypatch)
    daemon.handle_message(_msg(MsgType.SESSION_END, "B"))
    _run_worker(daemon, calls[0])                 # A's slot lands -> B's parked fn runs
    assert "B" not in daemon.router.channels
    assert daemon.digest_store.get("B") is None


def test_parked_digest_is_not_spoken_into_a_new_turn(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch, "A")
    calls = _park_short_background_turn(daemon, monkeypatch)
    daemon.handle_message(_msg(MsgType.FLUSH, "B"))  # B's user prompts again
    _run_worker(daemon, calls[0])
    assert "short turn text." not in _pending_texts(daemon, "B")
    assert daemon._last_digest_text.get("B") != "short turn text."


def test_parked_digest_still_lands_when_nothing_cancelled_it(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch, "A")
    calls = _park_short_background_turn(daemon, monkeypatch)
    _run_worker(daemon, calls[0])
    assert _pending_texts(daemon, "B") == ["short turn text."]


# --- M3 / F8: STOP silences everything, including what is still cooking ----

def test_stop_cancels_an_armed_settle_window(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch)
    calls = _capture_spawn(daemon, monkeypatch)
    daemon.handle_message(_prose("fg", "Long turn. " + _PAD * 6))
    _turn_done(daemon, "fg")
    gen = daemon._settle_gen["fg"]
    daemon.handle_message(_msg(MsgType.STOP))
    daemon._settle_fire("fg", gen)                # the timer fires anyway
    assert calls == []


def test_stop_drops_an_in_flight_digest(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch)
    calls = _capture_spawn(daemon, monkeypatch)
    daemon.handle_message(_prose("fg", "Long turn. " + _PAD * 6))
    _turn_done(daemon, "fg")
    _fire_settle(daemon, "fg")
    daemon.handle_message(_msg(MsgType.STOP))
    _run_worker(daemon, calls[0])
    assert _pending_texts(daemon, "fg") == []


def test_stop_drops_a_parked_digest(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch, "A")
    calls = _park_short_background_turn(daemon, monkeypatch)
    daemon.handle_message(_msg(MsgType.STOP))
    _run_worker(daemon, calls[0])
    assert _pending_texts(daemon, "A") == []
    assert _pending_texts(daemon, "B") == []


def test_stop_drops_a_held_question(monkeypatch):
    daemon, speaker, sessions, config = _summary_daemon(monkeypatch)
    calls = _capture_spawn(daemon, monkeypatch)
    daemon.handle_message(_prose("fg", "Lead-in. " + _PAD * 6))
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Pick one?", "options": ["a", "b"]}]))
    _fire_settle(daemon, "fg")
    assert daemon._held_decision.get("fg") is not None
    daemon.handle_message(_msg(MsgType.STOP))
    assert daemon._held_decision.get("fg") is None
    _run_worker(daemon, calls[0])
    assert _pending_texts(daemon, "fg") == []


# --- M13: router internals only change through router methods -------------

def test_paused_manual_switch_resumes_as_a_manual_replay_announcement():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    daemon._paused.set()
    item = SpeechItem(id=0, session="B", kind="session_change",
                      text="Session changed: b, reading again.",
                      is_decision=False, manual=True, replay=True)
    assert daemon._requeue_or_note(item, False) is True
    ann = daemon.router.next_item()
    assert ann.kind == "session_change"
    assert ann.manual is True and ann.replay is True


def test_daemon_never_edits_channel_items_or_router_privates():
    src = Path(daemon_module.__file__).read_text(encoding="utf-8")
    assert not re.search(r"\.items\.(insert|append|pop|remove)\(", src)
    assert not re.search(r"del\s+\w+\.items\[", src)
    assert not re.search(r"\.items\[[^\]]*\]\s*=", src)
    assert not re.search(r"\bch\.cursor\s*[-+]?=", src)
    assert not re.search(r"self\.router\._\w+", src)


# --- L-replay-decision ------------------------------------------------------

def test_replayed_decision_marks_the_channel(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    entry = daemon.history.record("fg", "choice", "Pick one?")
    daemon._replay("fg", [entry])
    assert daemon.router.channel("fg").has_decision is True


# --- L-preamble: check-then-act on _pending_preamble -----------------------

def test_preamble_cleared_between_check_and_use_does_not_crash():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")

    class _Racy(type(daemon)):
        # Every read hands out the value, then "on_play on the synth thread"
        # clears it - the window between the old double read.
        @property
        def _pending_preamble(self):
            v = self.__dict__.get("_pp")
            self.__dict__["_pp"] = None
            return v

        @_pending_preamble.setter
        def _pending_preamble(self, v):
            self.__dict__["_pp"] = v

    daemon.__dict__.pop("_pending_preamble", None)
    daemon.__class__ = _Racy
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=7, session="fg", kind="prose", text="content",
                         is_decision=False))
    ch.turn_done = True
    daemon._pending_preamble = ("fg", "Session changed: fg.")
    daemon._speak_loop_once()
    assert "content" in speaker.spoken
    assert [t for t, _v in speaker.cue_untracked_calls] == ["Session changed: fg."]
