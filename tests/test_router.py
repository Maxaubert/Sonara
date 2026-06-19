# tests/test_router.py
from sonari.router import Router
from sonari.queue import SpeechItem


class FakeSessions:
    def __init__(self): self._pin = None; self._fg = None; self._folders = {}
    def pinned(self): return self._pin
    def foreground(self): return self._fg
    def folder(self, s): return self._folders.get(s)


def _item(session, text, is_decision=False):
    return SpeechItem(id=0, session=session, kind="prose", text=text, is_decision=is_decision)


def _router(mq=1):
    s = FakeSessions()
    r = Router(s, minqueue=lambda: mq, announce_text=lambda f: "Session changed: {0}.".format(f))
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


def test_muted_channel_is_skipped():
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True; a.muted = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    # A is muted -> router cannot read A (fg is muted, no mute-exempt item)
    assert r.next_item() is None
    # User switches to B -> B becomes fg and reads
    s._fg = "B"
    item = r.next_item()
    assert item.text in ("Session changed: beta.", "b1")


def test_decision_preempts_current_reader():
    r, s = _router(mq=5)
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A")
    for i in range(5): a.append(_item("A", "a%d" % i))
    a.turn_done = True
    assert r.next_item().text == "a0"          # A reading
    # B raises a decision — preempts A mid-batch
    b = r.channel("B"); b.append(_item("B", "Pick?", is_decision=True))
    assert r.next_item().text == "Session changed: beta."   # preempts A
    assert r.next_item().text == "Pick?"


def test_pin_locks_and_repin_resets_cursor():
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}
    a = r.channel("A"); a.append(_item("A", "a1")); a.append(_item("A", "a2")); a.turn_done = True
    s._pin = "A"
    assert r.next_item().text == "a1"
    assert r.next_item().text == "a2"
    assert r.next_item() is None               # caught up, pinned -> no handoff
    # re-pin to A replays from the start
    r.repin_reset()
    assert r.next_item().text == "a1"
