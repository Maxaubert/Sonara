"""Startup channel rehydration (#118): a restart empties every in-memory
channel, so the manual session cycle could only reach sessions that spoke
SINCE the restart. Recently-seen sessions are re-seeded from the persisted
digest store as caught-up (replay-only) channels."""
import time

from tests.daemon_helpers import make_daemon
from sonara.protocol import MsgType, PROTOCOL_VERSION


def _spoken(daemon, speaker, n=8):
    for _ in range(n):
        daemon._speak_loop_once()
    return speaker.spoken


def test_rehydrated_session_is_cycle_reachable_and_replays():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground=None)
    daemon.digest_store.set("A", "Alpha's last digest.")
    sessions.touch("A")
    daemon._rehydrate_channels()
    assert _spoken(daemon, speaker, 4) == []       # heard item: never auto-spoken
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.NEXT_SESSION})
    out = _spoken(daemon, speaker)
    assert "Alpha's last digest." in out           # landing replays it
    assert any("reading again" in t for t in out)  # announced as a replay


def test_rehydrate_skips_stale_and_unknown_sessions():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground=None)
    daemon.digest_store.set("old", "stale digest")
    daemon.digest_store.set("ghost", "never seen digest")
    sessions._last_seen["old"] = time.time() - 4 * 3600   # beyond the 3h window
    daemon._rehydrate_channels()
    assert "old" not in daemon.router.channels
    assert "ghost" not in daemon.router.channels


def test_rehydrate_never_overwrites_a_live_channel():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE,
                           "session": "A", "delta": "Live text. ", "index": 0,
                           "final": True})
    daemon.digest_store.set("A", "stale persisted digest")
    before = list(daemon.router.channel("A").items)
    daemon._rehydrate_channels()
    assert daemon.router.channel("A").items == before


def test_heard_decision_mirrors_into_the_persisted_store():
    # Every _last_digest_text write site mirrors into the persisted store so
    # the session survives a restart in the cycle; the note_spoken decision
    # append is the site exercised here.
    from sonara.queue import SpeechItem
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon._last_digest_text["A"] = "Lead-in."
    item = SpeechItem(id=1, session="A", kind="choice", text="Pick one?",
                      is_decision=True)
    daemon.note_spoken(item, True)
    assert daemon.digest_store.get("A") == "Lead-in. Pick one?"


def test_flush_reseeds_the_wiped_channel_from_the_store():
    # #118 round 2: a new prompt's FLUSH bare-wiped the channel, dropping the
    # session out of the manual ring (empty channels are skipped) for the
    # WHOLE turn. The persisted digest re-seeds it: cycle-reachable and
    # replayable, but never auto-spoken.
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.digest_store.set("A", "Old digest.")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH,
                           "session": "A"})
    ch = daemon.router.channel("A")
    assert [i.text for i in ch.items] == ["Old digest."]
    assert ch.caught_up() and ch.seeded
    assert _spoken(daemon, speaker, 4) == []       # never auto-spoken
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.NEXT_SESSION})
    assert "Old digest." in _spoken(daemon, speaker)   # but cycle-reachable


def test_real_content_replaces_the_seed():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.digest_store.set("A", "Old digest.")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH,
                           "session": "A"})
    assert daemon.router.channel("A").seeded
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE,
                           "session": "A", "delta": "Fresh turn. ", "index": 0,
                           "final": True})
    ch = daemon.router.channel("A")
    assert all(i.text != "Old digest." for i in ch.items)   # seed replaced
    assert not ch.seeded
    assert any("Fresh turn." in i.text for i in ch.items)


def test_new_prose_lifts_force_switch_suppression_without_a_wipe():
    # Latent #115 gap: _enqueue appends via ch.items.append directly, which
    # bypassed channel.append's gen bump - streamed prose never lifted the
    # force-switch suppression. _enqueue now bumps gen itself.
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE,
                           "session": "A", "delta": "A one. ", "index": 0,
                           "final": True})
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE,
                           "session": "B", "delta": "B one. ", "index": 0,
                           "final": True})
    daemon.router.active = "A"
    daemon.router.next_session()                   # away from A -> suppressed
    assert daemon.router._is_suppressed("A") is True
    daemon._enqueue("A", "prose", "fresh prose", False)
    assert daemon.router._is_suppressed("A") is False


def test_session_end_forgets_the_persisted_digest():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.digest_store.set("A", "to be forgotten")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END,
                           "session": "A"})
    assert daemon.digest_store.get("A") is None
