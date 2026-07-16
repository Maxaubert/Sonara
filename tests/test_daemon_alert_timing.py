"""#94: the session-change alert plays at the content's synthesis-ready moment
(on_play), not seconds earlier. Deferral is daemon-side and only when fast_cues
is on; fast_cues off keeps the legacy immediate announcement."""
from sonara.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _seed_content(daemon, session, text="The digest body."):
    ch = daemon.router.channel(session)
    ch.append(SpeechItem(id=1, session=session, kind="prose", text=text,
                         is_decision=False))
    ch.turn_done = True
    daemon.router._replay_authorized.add(session)


def _handoff(foreground="a", target="b"):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=foreground)
    daemon.router._last_active = foreground          # so switching is a real handoff
    _seed_content(daemon, target)
    return daemon, speaker, config


def test_fast_cues_on_defers_alert_to_content_on_play():
    daemon, speaker, config = _handoff()
    config["fast_cues"] = True

    daemon._speak_loop_once()                        # session_change item -> stashed
    assert daemon._pending_preamble is not None
    assert daemon._pending_preamble[0] == "b"
    assert speaker.spoken == []                      # nothing spoken yet
    assert speaker.earcons == []                     # chime NOT fired yet
    assert speaker.cue_untracked_calls == []

    daemon._speak_loop_once()                        # content -> on_play fires the alert
    assert "The digest body." in speaker.spoken
    assert speaker.earcons == ["session_change"]     # chime at synthesis-ready
    assert len(speaker.cue_untracked_calls) == 1     # alert spoken via cue voice
    assert speaker.cue_untracked_calls[0][1] == daemon._cue_voice()
    assert daemon._pending_preamble is None


def test_fast_cues_off_speaks_alert_immediately():
    daemon, speaker, config = _handoff()
    config["fast_cues"] = False

    daemon._speak_loop_once()                        # session_change spoken now (legacy)
    assert speaker.earcons == ["session_change"]
    assert speaker.spoken                            # announcement spoken immediately
    assert daemon._pending_preamble is None
    assert speaker.cue_untracked_calls == []         # no deferred cue path


def test_muted_content_drops_pending_alert():
    daemon, speaker, config = _handoff()
    config["fast_cues"] = True
    daemon._speak_loop_once()                        # stash preamble for "b"
    assert daemon._pending_preamble is not None
    daemon._mute_level = 1                            # mute drops the content...
    daemon._speak_loop_once()                        # ...and the deferred alert with it
    assert daemon._pending_preamble is None
    assert speaker.cue_untracked_calls == []
    assert speaker.earcons == []                     # no chime for a muted handoff


def test_stale_preamble_for_other_session_is_dropped():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    config["fast_cues"] = True
    daemon._pending_preamble = ("b", "Reading from b.")   # stale, for a different session
    _seed_content(daemon, "a", text="Foreground content.")
    daemon._speak_loop_once()                        # content for "a"
    assert "Foreground content." in speaker.spoken
    assert speaker.cue_untracked_calls == []         # stale alert not applied
    assert daemon._pending_preamble is None


def test_preamble_on_play_still_engages_audio():
    daemon, speaker, config = _handoff()
    config["fast_cues"] = True
    config["audio_mode"] = "pause"
    daemon._speak_loop_once()                        # stash
    daemon._speak_loop_once()                        # content on_play: alert THEN engage
    assert speaker.cue_untracked_calls              # alert played
    assert daemon.pauser.pause_calls == 1            # audio still engaged after the alert
