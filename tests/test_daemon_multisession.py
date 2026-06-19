"""Multi-session integration tests for SpeechDaemon.

These tests exercise the per-session SessionChannel + Router design end-to-end,
using a FakeSpeaker to observe what gets spoken. The original bug (#59): pausing
one session would permanently silence the daemon even when switching to a different
session.
"""
from __future__ import annotations

from tests.daemon_helpers import make_daemon
from sonari.protocol import MsgType, PROTOCOL_VERSION


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _prose(session, delta, index=0, final=True):
    return {
        "v": PROTOCOL_VERSION,
        "type": MsgType.PROSE,
        "session": session,
        "delta": delta,
        "index": index,
        "final": final,
    }


def _fg(daemon, s):
    """Switch foreground to *s* and flush *s*'s channel (simulates a new prompt)."""
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": s})
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": s})


def _drain(daemon, n=20):
    """Drive the speak loop up to *n* times until the loop yields nothing."""
    for _ in range(n):
        daemon._speak_loop_once()


# ---------------------------------------------------------------------------
# Test 1 — The original bug repro (#59)
# Pausing B then switching to A must NOT silence A.
# ---------------------------------------------------------------------------

def test_pausing_one_session_does_not_lose_anothers_speech():
    """Pause session B (while it is the active reader) then switch to A.

    A's content must be spoken. The old bug was that the pause flag lingered
    across the session switch, silencing the daemon indefinitely.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    # Switch to B, give it a prose item, then pause.
    _fg(daemon, "B")
    daemon.handle_message(_prose("B", "B speaks. "))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})

    # Switch back to A -- FLUSH clears the paused flag.
    _fg(daemon, "A")
    daemon.handle_message(_prose("A", "A speaks. "))

    # Drain the loop: B may drain its remaining items first (cooperative
    # hand-off), but A must eventually be spoken.
    _drain(daemon, n=10)

    assert "A speaks." in speaker.spoken, (
        "A was not heard after pausing B and switching foreground to A. "
        "Bug: pause flag lingered across session switch."
    )


# ---------------------------------------------------------------------------
# Test 2 — Two sessions take turns, nothing is lost
# ---------------------------------------------------------------------------

def test_two_sessions_take_turns_nothing_lost():
    """Stream a message from A then from B; both must appear in speaker.spoken.

    Spec: foreground first, then oldest-waiting. When B becomes fg before any
    reading has started, B takes the floor first, then A is served as the
    oldest-waiting non-fg session. Both messages must be heard.

    C1 verification: after A reads (establishing _last_active=A), B is a
    different session, so a "Session changed: beta." announcement fires before
    B's item. This confirms the idle-gap handoff announcement works.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    # Register both sessions with folder names so the announcement text
    # includes the folder name.
    daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.SESSION_START,
        "session": "A", "cwd": "/home/user/alpha", "plugin_version": "",
    })
    daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.SESSION_START,
        "session": "B", "cwd": "/home/user/beta", "plugin_version": "",
    })
    # Restore A as fg (SESSION_START for B moved fg to B).
    daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "A",
    })

    # A streams a message.
    daemon.handle_message(_prose("A", "A one. "))
    daemon.router.channel("A").turn_done = True

    # B becomes the new foreground (real hand-off path from the hooks extension).
    daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "B",
    })
    daemon.handle_message(_prose("B", "B one. "))
    daemon.router.channel("B").turn_done = True

    _drain(daemon, n=10)

    # Core invariant: NOTHING IS LOST.
    assert "A one." in speaker.spoken, f"A's message was not spoken. {speaker.spoken!r}"
    assert "B one." in speaker.spoken, f"B's message was not spoken. {speaker.spoken!r}"

    # Spec §ordering: B is fg when reading starts (_active=None), so B reads
    # first. A is served next as oldest-waiting, with a "Session changed: alpha."
    # announcement. This is correct per spec and demonstrates C1 (idle-gap
    # announce) in action.
    idx_a = speaker.spoken.index("A one.")
    idx_b = speaker.spoken.index("B one.")
    assert idx_b < idx_a, (
        f"Expected B (fg) to be spoken before A (oldest-waiting). "
        f"Spoken: {speaker.spoken!r}"
    )

    # C1: "Session changed: alpha." must appear between B one. and A one.
    # (A is _last_active=B at the point B drained, then A is a different session)
    announce_texts = [t for t in speaker.spoken if t.startswith("Session changed:")]
    assert any("alpha" in t for t in announce_texts), (
        f"Expected 'Session changed: alpha.' announcement for idle-gap handoff to A. "
        f"Spoken: {speaker.spoken!r}"
    )


def test_session_change_fires_chime_earcon():
    """On a hand-off, the speak loop fires the 'session_change' earcon (chime)
    just before voicing the 'Session changed: ...' announcement."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START,
                           "session": "A", "cwd": "/home/user/alpha", "plugin_version": ""})
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START,
                           "session": "B", "cwd": "/home/user/beta", "plugin_version": ""})
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "A"})
    daemon.handle_message(_prose("A", "A one. "))
    daemon.router.channel("A").turn_done = True
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "B"})
    daemon.handle_message(_prose("B", "B one. "))
    daemon.router.channel("B").turn_done = True
    _drain(daemon, n=10)
    assert "session_change" in speaker.earcons, (
        f"session-change chime not fired on hand-off. earcons: {speaker.earcons!r}")


# ---------------------------------------------------------------------------
# Test 3 — Cooperative hand-off: active reader keeps the floor until its
# batch drains, THEN the new foreground takes over.
# ---------------------------------------------------------------------------

def test_cooperative_handoff_waits_for_current_session():
    """While A is the active reader, switching fg to B does NOT cut A off.

    The router's 'current reader keeps the floor' rule applies when active is
    set (reading has already started). A's full content must be heard before
    B's content.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    # A has two prose items queued.
    daemon.handle_message(_prose("A", "A part one. ", index=0, final=False))
    daemon.handle_message(_prose("A", "A part two. ", index=1, final=True))
    # Mark turn done so the batch is readable.
    daemon.router.channel("A").turn_done = True

    # Start reading A's first item (this sets active=A).
    daemon._speak_loop_once()
    assert "A part one." in speaker.spoken, "First read of A must succeed."

    # B switches in while A still has one item pending (active=A).
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "B"})
    daemon.handle_message(_prose("B", "B content. "))
    daemon.router.channel("B").turn_done = True

    # Drain the rest -- A must fully finish before B content appears.
    _drain(daemon, n=15)

    spoken = speaker.spoken
    assert "A part two." in spoken, f"A's final chunk was cut off. Spoken: {spoken!r}"
    assert "B content." in spoken, f"B was never spoken. Spoken: {spoken!r}"

    # A part two must appear before B content (cooperative drain).
    idx_a_last = max(i for i, t in enumerate(spoken) if "A part" in t)
    idx_b = next(i for i, t in enumerate(spoken) if "B content." == t)
    assert idx_a_last < idx_b, (
        f"B content appeared before A finished. A last at {idx_a_last}, B at {idx_b}. "
        f"Spoken: {spoken!r}"
    )


# ---------------------------------------------------------------------------
# Test 4 — Re-pin replays from the start of the pinned channel
# ---------------------------------------------------------------------------

def test_repin_replays_from_start():
    """PIN_TOGGLE after A has spoken resets A's channel cursor to 0.

    The first item from A must be replayed (channel.reset() is called).
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    # Give A two items and drain them so the cursor is at the end.
    daemon.handle_message(_prose("A", "First sentence. "))
    daemon.router.channel("A").turn_done = True
    # Drain A's items (advance the cursor).
    _drain(daemon, n=5)
    assert "First sentence." in speaker.spoken

    # Remember position before pin.
    ch_a = daemon.router.channel("A")
    cursor_before = ch_a.cursor
    assert cursor_before > 0, "Cursor should have advanced after draining A."

    # Toggle pin: this calls router.repin_reset() which calls channel.reset().
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PIN_TOGGLE})

    # After repin, cursor must be back at 0.
    assert ch_a.cursor == 0, (
        f"Cursor was not rewound after PIN_TOGGLE. cursor={ch_a.cursor}"
    )

    # Drain again: the first item must be re-spoken.
    spoken_before = list(speaker.spoken)
    _drain(daemon, n=5)
    new_items = speaker.spoken[len(spoken_before):]
    assert "First sentence." in new_items, (
        f"Pin replay did not re-speak the first item. New items: {new_items!r}"
    )


# ---------------------------------------------------------------------------
# Test 5a — Background session's tool announcement is suppressed, then heard
# ---------------------------------------------------------------------------

def test_background_tool_announcement_deferred_until_session_is_active():
    """A tool announcement from a background session (not fg) stays in its
    channel until that session becomes the active reader.

    It must NOT be spoken while another session holds the floor, and MUST be
    spoken once the background session becomes foreground.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    # A has content that keeps it as active reader.
    daemon.handle_message(_prose("A", "A talking. "))
    daemon.router.channel("A").turn_done = True

    # B (background) announces a tool.
    daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.TOOL,
        "session": "B", "tool": "Bash", "summary": "Running bash.",
    })

    # Drive one iteration: A should speak (it is fg and active).
    daemon._speak_loop_once()
    assert "Running bash." not in speaker.spoken, (
        "B's tool announcement was spoken while A was the active reader."
    )

    # Now drain A completely.
    _drain(daemon, n=5)
    assert "A talking." in speaker.spoken

    # Switch foreground to B -- it now becomes the fg and will be picked.
    daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "B",
    })

    # Drive more iterations: B's tool cue must now be heard.
    _drain(daemon, n=5)
    assert "Running bash." in speaker.spoken, (
        "B's tool announcement was not spoken after B became the active reader."
    )


# ---------------------------------------------------------------------------
# Test 5b — Background decision preempts in non-earcon_only config (I1)
# ---------------------------------------------------------------------------

def test_background_decision_preempts_current_reader():
    """A CHOICE (decision) from a background session preempts the current
    active reader when background_policy is not 'earcon_only' (I1 fix).

    With earcon_only (default), decision TEXT for non-fg sessions is suppressed.
    With a permissive policy, background decisions preempt even when the router
    was otherwise idle. This tests the I1 fix using a non-earcon_only setup.

    Scenario: A has prose; while A is active, B (background, policy-allowed)
    raises a blocking choice. Router preempts A mid-batch to serve B's decision.
    """
    from sonari.sessions import SessionManager as SM
    from sonari.daemon import SpeechDaemon
    from sonari.config import DEFAULTS

    speaker_obj = speaker_class = None
    # Inline FakeSpeaker to avoid import cycle
    class FS:
        def __init__(self): self.spoken = []; self._epoch = 0
        def speak(self, t, cancel_epoch=None): self.spoken.append(t); return True
        def cancel_epoch(self): return self._epoch
        def cancel(self): self._epoch += 1
        def earcon(self, k): pass
        def set_rate(self, r): pass
        def set_voice(self, v): pass

    sp = FS()
    sess = SM(background_policy="all")  # not earcon_only: bg sessions get voice
    sess.set_foreground("A")
    cfg = {k: (dict(v) if isinstance(v, dict) else v) for k, v in DEFAULTS.items()}
    cfg["verbosity"] = "everything"
    daemon = SpeechDaemon(sp, sess, cfg)

    # A has prose queued and has started reading (active=A, _last_active=A).
    daemon.handle_message(_prose("A", "Long A message one. "))
    daemon.handle_message(_prose("A", "Long A message two. "))
    daemon.router.channel("A").turn_done = True
    daemon._speak_loop_once()  # reads A's first item, sets active=A
    assert "Long A message one." in sp.spoken

    # B (background, policy allows it) submits a blocking choice.
    daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.CHOICE,
        "session": "B",
        "questions": [{"question": "Pick one.", "options": [{"label": "Yes"}, {"label": "No"}]}],
    })

    # Drain. B's decision preempts (with announcement), then A finishes.
    for _ in range(10):
        daemon._speak_loop_once()

    # Both B's decision and A's remaining item must be heard.
    assert any("Pick one." in t for t in sp.spoken), (
        f"B's blocking decision was not spoken. Spoken: {sp.spoken!r}"
    )
    assert "Long A message two." in sp.spoken, (
        f"A's remaining prose was not spoken after B drained. Spoken: {sp.spoken!r}"
    )

    # Ordering: B's decision appears between A's two items.
    idx_decision = next(i for i, t in enumerate(sp.spoken) if "Pick one." in t)
    idx_a1 = sp.spoken.index("Long A message one.")
    idx_a2 = sp.spoken.index("Long A message two.")
    assert idx_a1 < idx_decision, (
        f"B's decision should appear after A's first item. Spoken: {sp.spoken!r}"
    )
    assert idx_decision < idx_a2, (
        f"A's second item should appear after B's decision. Spoken: {sp.spoken!r}"
    )


# ---------------------------------------------------------------------------
# Test 6 — Muted foreground: no audio from it; daemon does not get stuck
# ---------------------------------------------------------------------------

def test_global_mute_silences_every_session():
    """MUTE is global: while muted, NO session's prose is spoken (only the
    mute-exempt "Muted." cue). The daemon must not get stuck."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    daemon.handle_message(_prose("A", "You should not hear A. "))
    daemon.router.channel("A").turn_done = True
    daemon.handle_message(_prose("B", "You should not hear B. "))
    daemon.router.channel("B").turn_done = True

    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})   # global mute
    assert daemon._muted is True

    _drain(daemon, n=10)

    assert "You should not hear A." not in speaker.spoken
    assert "You should not hear B." not in speaker.spoken          # ALL sessions silenced
    assert "Muted." in speaker.spoken, (
        f"'Muted.' confirmation was not spoken. Spoken: {speaker.spoken!r}"
    )


# ---------------------------------------------------------------------------
# Test 7 — C1: Idle-gap handoff announces
# ---------------------------------------------------------------------------

def test_idle_gap_handoff_announces():
    """A reads and fully drains (next_item returns None via _last_active=A),
    then B becomes ready/foreground -> next_item emits 'Session changed: beta.'
    before B's item. This is the C1 bug fix verification.
    """
    r_sessions_map = {}

    class FakeSessions:
        def __init__(self): self._pin = None; self._fg = "A"; self._folders = {"A": "alpha", "B": "beta"}
        def pinned(self): return self._pin
        def foreground(self): return self._fg
        def folder(self, s): return self._folders.get(s)

    from sonari.router import Router
    from sonari.queue import SpeechItem

    def _item(session, text):
        return SpeechItem(id=0, session=session, kind="prose", text=text,
                          is_decision=False)

    fs = FakeSessions()
    router = Router(fs, minqueue=lambda: 1,
                    announce_text=lambda f: "Session changed: {0}.".format(f))

    # A reads and fully drains.
    ch_a = router.channel("A")
    ch_a.append(_item("A", "A item"))
    ch_a.turn_done = True
    assert router.next_item().text == "A item"     # reads, _last_active=A
    assert router.next_item() is None              # A drained, active=None

    # Now B becomes ready/foreground.
    fs._fg = "B"
    ch_b = router.channel("B")
    ch_b.append(_item("B", "B item"))
    ch_b.turn_done = True

    # C1: next_item must emit the announcement BEFORE B's item.
    first = router.next_item()
    assert first is not None, "Expected announce or B item"
    assert first.text == "Session changed: beta.", (
        f"Expected 'Session changed: beta.' announcement for idle-gap handoff. "
        f"Got: {first.text!r}"
    )
    second = router.next_item()
    assert second is not None
    assert second.text == "B item"


# ---------------------------------------------------------------------------
# Test 8 — I1: Background decision while idle is read
# ---------------------------------------------------------------------------

def test_background_decision_while_idle_is_read():
    """When the router is idle (active=None / fg has nothing), a background
    session's decision must be served immediately (I1 fix).
    """
    from sonari.router import Router
    from sonari.queue import SpeechItem

    class FakeSessions:
        def __init__(self): self._pin = None; self._fg = "A"
        def pinned(self): return self._pin
        def foreground(self): return self._fg
        def folder(self, s): return None

    def _item(session, text, is_decision=False):
        return SpeechItem(id=0, session=session, kind="prose", text=text,
                          is_decision=is_decision)

    fs = FakeSessions()
    router = Router(fs, minqueue=lambda: 1,
                    announce_text=lambda f: "Session changed: {0}.".format(f or "another session"))

    # fg (A) has nothing. B has a blocking decision.
    ch_b = router.channel("B")
    ch_b.append(_item("B", "Approve action?", is_decision=True))
    ch_b.turn_done = True

    # I1: decision preempts even when idle (active=None).
    item = router.next_item()
    assert item is not None, "Expected B's decision to be served when router is idle."
    # First-ever reader: no announce (_last_active=None)
    assert item.text == "Approve action?", (
        f"Expected B's decision. Got: {item.text!r}"
    )


# ---------------------------------------------------------------------------
# Test 9 — I3: Muted foreground falls through to a ready background
# ---------------------------------------------------------------------------

def test_muted_foreground_falls_through_to_ready_background():
    """Spec §3: a muted channel is skipped in auto; hand off to
    the next oldest-waiting ready session (I3 fix).

    fg is muted + a ready background session -> next_item serves the
    background session (with announce if applicable), NOT None.
    """
    from sonari.router import Router
    from sonari.queue import SpeechItem

    class FakeSessions:
        def __init__(self): self._pin = None; self._fg = "A"
        def pinned(self): return self._pin
        def foreground(self): return self._fg
        def folder(self, s): return {"A": "alpha", "B": "beta"}.get(s)

    def _item(session, text):
        return SpeechItem(id=0, session=session, kind="prose", text=text,
                          is_decision=False)

    fs = FakeSessions()
    router = Router(fs, minqueue=lambda: 1,
                    announce_text=lambda f: "Session changed: {0}.".format(f))

    # A is fg but muted with normal (non-exempt) prose.
    ch_a = router.channel("A")
    ch_a.append(_item("A", "muted prose"))
    ch_a.turn_done = True
    ch_a.muted = True

    # B is ready (background / oldest-waiting).
    ch_b = router.channel("B")
    ch_b.append(_item("B", "b item"))
    ch_b.turn_done = True

    # I3: must NOT return None; must serve B.
    # B is first reader (_last_active=None), so no announce.
    item = router.next_item()
    assert item is not None, (
        "Expected B to be served when fg is muted. Got None (I3 regression)."
    )
    assert item.text == "b item", (
        f"Expected B's item. Got: {item.text!r}"
    )
