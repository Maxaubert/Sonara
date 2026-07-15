"""#90: the session-change announcement must NOT engage audio ducking.

A cross-session handoff speaks a short "Now reading from X" announcement (fast
cue voice, ~0.3s) as its own item BEFORE the digest. That announcement used to
duck other apps' audio immediately via on_play - then the digest, the next item,
synthesized on a cold neural voice for 6-7s while the duck engaged for the
announcement just sat there over silence. Only real content should duck, at its
own playback; the announcement rides through un-ducked (from idle) or keeps the
existing duck (mid-listening).
"""
from sonara.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _handoff_daemon():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    config["audio_control"] = True
    # simulate that session "a" has been read before, so switching to "b" is a
    # real handoff that arms the session-change announcement.
    daemon.router._last_active = "a"
    ch = daemon.router.channel("b")
    ch.append(SpeechItem(id=1, session="b", kind="prose",
                         text="The digest body.", is_decision=False))
    ch.turn_done = True
    daemon.router._replay_authorized.add("b")
    return daemon, speaker


def test_announcement_does_not_duck_but_content_does():
    daemon, speaker = _handoff_daemon()

    daemon._speak_loop_once()                      # speaks the announcement
    assert daemon.ducker.duck_calls == []          # announcement did NOT duck
    assert not daemon.ducker.is_ducked()
    # the announcement really was spoken (a session_change item)
    assert speaker.spoken and speaker.earcons == ["session_change"]

    daemon._speak_loop_once()                      # speaks the digest body
    assert daemon.ducker.duck_calls                # content ducked, at playback
    assert daemon.ducker.is_ducked()
    assert "The digest body." in speaker.spoken


def test_existing_duck_survives_the_announcement():
    # mid-listening: audio already ducked from a prior reading -> the handoff
    # announcement must leave the duck engaged (continuous), not lift it.
    daemon, speaker = _handoff_daemon()
    daemon.ducker.duck({0}, 30)                     # pretend we were mid-reading
    assert daemon.ducker.is_ducked()

    daemon._speak_loop_once()                       # announcement
    assert daemon.ducker.is_ducked()               # still ducked across handoff
    assert daemon.ducker.restore_calls == 0
