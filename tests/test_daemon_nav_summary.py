"""Summary-mode navigation (issue #11).

In summary mode the daemon speaks one digest per turn, not the raw per-message
prose. So message-cursor nav (Ctrl+Alt+Left/Right = nav prev/next) is meaningless
and must be a SILENT no-op: no chime, and nothing enqueued onto the gated session
channel (which otherwise piles up and bursts at turn end). Ctrl+Alt+Up (nav
'first') re-reads the last digest. Non-summary nav is unchanged.
"""
from tests.daemon_helpers import make_daemon


def _summary_daemon():
    daemon, _queue, speaker, _sessions, config = make_daemon(foreground="fg")
    config["summary_mode"] = True
    return daemon, speaker


def _nav(daemon, to):
    daemon.handle_message({"type": "nav", "to": to, "session": "fg"})


def _drain(daemon, session="fg"):
    ch = daemon.router.channel(session)
    out = []
    while ch.cursor < len(ch.items):
        out.append(ch.items[ch.cursor])
        ch.cursor += 1
    return out


def _seed_turn_with_digest(daemon):
    h = daemon.history
    h.record("fg", "prose", "The raw unspoken prose of the turn.")
    h.end_message("fg")
    h.record("fg", "summary", "The short digest.")   # last message = the digest


def test_summary_nav_prev_is_silent_noop():
    daemon, speaker = _summary_daemon()
    _seed_turn_with_digest(daemon)
    speaker.earcons.clear()
    _nav(daemon, "prev")
    assert speaker.earcons == []                        # no nav / nav_edge chime
    assert daemon.router.channel("fg").pending() == 0   # nothing replayed
    assert speaker.cancels == 0                          # did not cut current speech


def test_summary_nav_next_is_silent_noop():
    daemon, speaker = _summary_daemon()
    _seed_turn_with_digest(daemon)
    speaker.earcons.clear()
    _nav(daemon, "next")
    assert speaker.earcons == []
    assert daemon.router.channel("fg").pending() == 0
    assert speaker.cancels == 0


def test_summary_nav_prev_empty_history_does_not_pile_up():
    # The reported bug: nav at a fresh turn enqueued "Nothing to navigate yet."
    # onto the gated session channel; 5-6 presses burst out later. In summary mode
    # prev/next must not enqueue anything at all.
    daemon, speaker = _summary_daemon()
    for _ in range(6):
        _nav(daemon, "prev")
    ch = daemon.router.channel("fg")
    assert ch.pending() == 0
    assert [it.text for it in ch.items] == []          # no pile-up
    assert speaker.earcons == []


def test_summary_nav_first_rereads_last_digest():
    daemon, speaker = _summary_daemon()
    _seed_turn_with_digest(daemon)
    _nav(daemon, "first")
    played = [it.text for it in _drain(daemon)]
    assert played == ["The short digest."]             # the digest, not the prose


def test_nav_prev_unchanged_when_summary_mode_off():
    # Regression guard: summary OFF -> prev still replays messages and chimes "nav".
    daemon, _queue, speaker, _sessions, _config = make_daemon(foreground="fg")
    h = daemon.history
    h.record("fg", "prose", "m0")
    h.end_message("fg")
    h.record("fg", "prose", "m1")
    speaker.earcons.clear()
    daemon.handle_message({"type": "nav", "to": "prev", "session": "fg"})
    played = [it.text for it in _drain(daemon)]
    assert played == ["m0", "m1"]
    assert speaker.earcons == ["nav"]
