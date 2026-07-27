# tests/test_router.py
from sonara.router import Router
from sonara.queue import SpeechItem


class FakeSessions:
    def __init__(self): self._fg = None; self._folders = {}
    def foreground(self): return self._fg
    def folder(self, s): return self._folders.get(s)


def _item(session, text, is_decision=False):
    return SpeechItem(id=0, session=session, kind="prose", text=text, is_decision=is_decision)


def _router(mq=1):
    s = FakeSessions()
    r = Router(s, minqueue=lambda: mq,
               announce_text=lambda f, replay=False: (
                   "Session changed: {0}, reading again.".format(f) if replay
                   else "Session changed: {0}.".format(f)))
    return r, s


def test_single_session_reads_in_order_no_announcement():
    r, s = _router()
    s._fg = "A"
    ch = r.channel("A"); ch.append(_item("A", "one")); ch.append(_item("A", "two")); ch.turn_done = True
    assert r.next_item().text == "one"
    assert r.next_item().text == "two"
    assert r.next_item() is None


def test_auto_handoff_announces_then_reads_foreground_first():
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True
    # A reads its message (A is fg)
    assert r.next_item().text == "a1"
    # B prompts (becomes foreground) with content
    s._fg = "B"
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    # A is caught up -> hand off to B: announcement first, then B's item
    assert r.next_item().text == "Session changed: beta."
    assert r.next_item().text == "b1"
    assert r.next_item() is None


def test_active_reader_finishes_before_handoff():
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A"); a.append(_item("A", "a1")); a.append(_item("A", "a2")); a.turn_done = True
    assert r.next_item().text == "a1"
    # B prompts mid-A-read
    s._fg = "B"; b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    # A keeps the floor until its queue drains (cooperative)
    assert r.next_item().text == "a2"
    assert r.next_item().text == "Session changed: beta."
    assert r.next_item().text == "b1"


def test_replay_authorization_evicted_once_drained():
    # The old in-loop eviction sat behind _ready() (which requires pending > 0),
    # so it could never fire: a one-shot digest authorization leaked for the
    # channel's lifetime and permanently bypassed the background policy (#19).
    r, s = _router()
    s._fg = "A"
    s.should_speak = lambda sess: sess == "A"       # policy: background B is muted
    b = r.channel("B"); b.append(_item("B", "digest")); b.turn_done = True
    r._replay_authorized.add("B")                   # one-shot digest authorization
    assert r.next_item().text == "digest"           # authorized content is voiced
    assert r.next_item() is None                    # drained
    assert "B" not in r._replay_authorized          # authorization evicted
    b.append(_item("B", "later prose")); b.turn_done = True
    assert r.next_item() is None                    # policy applies again: silent


def test_muted_channel_is_skipped():
    # Spec §3: a muted channel is skipped in auto; hand off past it to
    # the next oldest-waiting ready session.
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True; a.muted = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    # A is fg but muted (no mute-exempt item) -> router falls through to B
    # (oldest-waiting). B is served with an announcement since it is a
    # different session from A.
    item = r.next_item()
    # Announcement fires first (A is _last_active after this, target=B is new)
    # BUT _last_active starts as None -> no announce on first real reader.
    # With _last_active=None at start, B is the FIRST reader -> no announce.
    assert item is not None
    assert item.text == "b1"
    assert r.next_item() is None


def test_decision_preempts_current_reader():
    r, s = _router(mq=5)
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A")
    for i in range(5): a.append(_item("A", "a%d" % i))
    a.turn_done = True
    assert r.next_item().text == "a0"          # A reading
    # B raises a decision -- preempts A mid-batch
    b = r.channel("B"); b.append(_item("B", "Pick?", is_decision=True))
    assert r.next_item().text == "Session changed: beta."   # preempts A
    assert r.next_item().text == "Pick?"


def test_next_session_advances_one_slot_in_fixed_order():
    r, s = _router()
    for name in ("A", "B", "C"):
        ch = r.channel(name); ch.append(_item(name, name.lower())); ch.turn_done = True
    r.active = "A"
    assert r.next_session()[0] == "B"              # A -> B (next in insertion order)
    assert r.next_session()[0] == "C"              # B -> C
    assert r.next_session()[0] == "A"              # C -> A (wrap)


def test_force_switched_session_resumes_after_wipe_and_new_digest():
    # #115: digest-mode channels hold exactly ONE item; every new turn is
    # wipe (len 0) then append (len 1), landing back on the recorded length,
    # and the lazy suppression check never runs while the channel is empty.
    # Length-keyed suppression therefore never lifted: a session you switched
    # away from went silent FOREVER. Keyed on channel.gen it lifts on any
    # new content.
    r, s = _router()
    s._fg = "B"
    a = r.channel("A"); a.append(_item("A", "digest one")); a.turn_done = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    r.active = "A"
    r.next_session()                               # force-switch A -> B
    assert r._is_suppressed("A") is True
    a.wipe()                                       # new prompt in A (unchecked while empty)
    a.append(_item("A", "digest two"))             # next turn's digest lands
    a.turn_done = True
    assert r._is_suppressed("A") is False          # new content lifts suppression
    assert r.next_item() is not None               # A speaks again


def test_next_session_skips_channels_with_nothing_to_hear():
    # #117: landing on an EMPTY channel (freshly wiped by a new prompt, digest
    # still in flight) announced the switch and then fell straight through to
    # an auto handoff - two back-to-back announcements for one press. The
    # manual ring skips empty channels.
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True
    r.channel("B")                                 # B exists but has NO items
    c = r.channel("C"); c.append(_item("C", "c1")); c.turn_done = True
    r.active = "A"
    assert r.next_session()[0] == "C"              # skips empty B
    assert r.next_session()[0] == "A"              # wraps, still skipping B


def test_next_session_degrades_to_full_ring_when_all_empty():
    # Never dead-end: with every channel empty the plain ring still answers.
    r, s = _router()
    r.channel("A"); r.channel("B")
    target, replay = r.next_session()
    assert target in ("A", "B")


def test_next_session_continues_ring_after_idle_gap():
    # _pick() clears `active` whenever the queue drains; the ring must continue
    # from the LAST reader, not reset to the first channel (#111: sessions past
    # the first were unreachable when presses were spaced across idle gaps).
    r, s = _router()
    for name in ("A", "B", "C"):
        ch = r.channel(name); ch.append(_item(name, name.lower())); ch.turn_done = True
    r.active = None; r._last_active = "B"          # idle gap after B read
    assert r.next_session()[0] == "C"              # continues B -> C, not reset to A
    r.active = None; r._last_active = "C"
    assert r.next_session()[0] == "A"              # wraps


def test_next_session_muted_last_active_advances_to_next_audible():
    # The ring position may be a session that is now muted (excluded from the
    # audible ring): advance from ITS slot to the next audible session instead
    # of resetting to the first.
    r, s = _router()
    for name in ("A", "B", "C"):
        ch = r.channel(name); ch.append(_item(name, name.lower())); ch.turn_done = True
    r.channels["B"].muted = True
    r.active = None; r._last_active = "B"
    assert r.next_session()[0] == "C"              # B's slot -> next audible (C)


def test_next_session_arms_manual_announcement():
    # A manual switch emits its session_change item with manual=True so the
    # speak loop voices it immediately; an auto handoff stays manual=False.
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True
    assert r.next_item().text == "a1"              # A is the reader
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    r.next_session()                               # manual: A -> B
    item = r.next_item()
    assert item.kind == "session_change" and item.manual is True
    # drain B, then an AUTO handoff back to a refilled A announces manual=False
    assert r.next_item().text == "b1"
    s._fg = "A"
    a.append(_item("A", "a2")); a.turn_done = True
    item = r.next_item()
    assert item.kind == "session_change" and item.manual is False


def test_next_session_resumes_an_unread_target_no_replay():
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.append(_item("B", "b2")); b.turn_done = True
    b.next()                                       # B partially read (still unread)
    r.active = "A"
    target, replay = r.next_session()
    assert (target, replay) == ("B", False)        # unread -> resume, not replay
    assert r.channels["B"].cursor == 1             # cursor NOT reset
    assert r.active == "B"


def test_next_session_replays_a_read_target():
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    b.next()                                       # B fully read (caught up)
    r.active = "A"
    assert r.channels["B"].caught_up() is True
    target, replay = r.next_session()
    assert (target, replay) == ("B", True)         # read -> replay
    assert r.channels["B"].cursor == 0             # cursor reset for replay


def test_next_session_single_session_lands_on_itself():
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True; a.next()  # read
    r.active = "A"
    assert r.next_session() == ("A", True)         # wraps to itself; read -> replay


def test_next_session_none_when_no_channels():
    r, s = _router()
    assert r.next_session() == (None, False)       # nothing registered


def test_drop_clears_replay_flag_so_later_handoff_is_not_reading_again():
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True; b.next()  # B read
    r.active = "A"
    target, replay = r.next_session()              # lands on B (read) -> replay armed
    assert (target, replay) == ("B", True)
    r.drop("B")                                    # B ends before the announcement plays
    # A was force-switched away (suppressed); new content lifts the suppression so
    # it can auto-resume. The later auto handoff to A must NOT say "reading again".
    a.append(_item("A", "a2"))                     # new content -> A no longer suppressed
    r.active = None; r._last_active = "X"          # force an auto announce on next pick
    item = r.next_item()
    assert item is not None and "reading again" not in item.text


# --- force-switch suppression (bug 2): a session you manually switched AWAY from
# is not auto-resumed until it gets new content; a manual return clears it. -----

def test_force_switched_away_session_is_not_auto_resumed():
    # Reading A, force-switch to B. When B drains, A must NOT auto-resume -- the
    # user left it on purpose. Before the fix, oldest-waiting pulled A right back.
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.append(_item("A", "a2")); a.turn_done = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    r.active = "A"; a.next()                        # A reading; a2 still pending
    assert r.next_session()[0] == "B"              # force-switch A -> B (suppresses A)
    assert r._is_suppressed("A")
    b.next()                                        # B reads its content and drains
    assert r._pick() is None                       # A has pending a2 but stays suppressed


def test_new_content_lifts_suppression_so_session_can_resume():
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.append(_item("A", "a2")); a.turn_done = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    r.active = "A"; a.next()
    r.next_session(); b.next()                      # switch A -> B, drain B
    assert r._pick() is None                        # A suppressed
    a.append(_item("A", "a3")); a.turn_done = True  # NEW content for A
    assert not r._is_suppressed("A")                # len(items) changed -> lifted
    assert r._pick() == "A"                         # A can be auto-picked again


def test_manual_return_clears_suppression_and_resumes_from_cursor():
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.append(_item("A", "a2")); a.turn_done = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    r.active = "A"; a.next()                        # A read a1; a2 pending (cursor=1)
    r.next_session()                               # A -> B, A suppressed
    assert r._is_suppressed("A")
    target, replay = r.next_session()              # B -> A (manual return)
    assert target == "A" and replay is False       # unread tail -> resume, not replay
    assert not r._is_suppressed("A")               # manual return clears suppression
    assert r.channels["A"].cursor == 1             # resumes where it left off
