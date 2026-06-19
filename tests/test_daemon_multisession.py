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

    # Switch back to A — FLUSH clears the paused flag.
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

    When the hand-off goes through handle_message (not via the _speakable
    shortcut), the router emits a 'Session changed: ...' announcement as B
    takes the floor.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    # Register both sessions with folder names via handle_message so the
    # announcement text can include the folder name.
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

    # B sends a prompt (SET_FOREGROUND — this is the real hand-off path that
    # the daemon receives from the hooks extension). The handler adds A to
    # _speakable only if A has pending items; here A still has 1 item.
    daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "B",
    })
    daemon.handle_message(_prose("B", "B one. "))
    daemon.router.channel("B").turn_done = True

    _drain(daemon, n=10)

    assert "A one." in speaker.spoken, f"A's message was not spoken. {speaker.spoken!r}"
    assert "B one." in speaker.spoken, f"B's message was not spoken. {speaker.spoken!r}"

    # When A drains via the _speakable path, the router suppresses the
    # announcement for A and its successor (B). BUT: once _speakable evicts A
    # and the router picks B directly as the new fg reader, B has not been
    # announced yet (if the eviction clears _announced). Let's assert both
    # messages were heard -- the announcement behavior depends on the code
    # path. The core invariant is: NOTHING IS LOST.
    # We also verify ordering: A before B.
    idx_a = speaker.spoken.index("A one.")
    idx_b = speaker.spoken.index("B one.")
    assert idx_a < idx_b, (
        f"B was spoken before A. Spoken: {speaker.spoken!r}"
    )


# ---------------------------------------------------------------------------
# Test 3 — Cooperative hand-off waits: A finishes before B takes the floor
# ---------------------------------------------------------------------------

def test_cooperative_handoff_waits_for_current_session():
    """While A is mid-message, switching to B does NOT cut A off.

    A's full content must be heard before the announcement and B's content.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    # A has two prose items queued (neither turn_done yet).
    daemon.handle_message(_prose("A", "A part one. ", index=0, final=False))
    daemon.handle_message(_prose("A", "A part two. ", index=1, final=True))
    # Mark turn done so the batch is readable.
    daemon.router.channel("A").turn_done = True

    # B switches in while A has items pending.
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "B"})
    daemon.handle_message(_prose("B", "B content. "))
    daemon.router.channel("B").turn_done = True

    _drain(daemon, n=15)

    spoken = speaker.spoken
    assert "A part two." in spoken, f"A's final chunk was cut off. Spoken: {spoken!r}"
    assert "B content." in spoken, f"B was never spoken. Spoken: {spoken!r}"

    # A must fully finish before B content appears.
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

    # Drive one iteration: only A should speak.
    daemon._speak_loop_once()
    assert "Running bash." not in speaker.spoken, (
        "B's tool announcement was spoken while A was the active reader."
    )

    # Now drain A completely.
    _drain(daemon, n=5)
    assert "A talking." in speaker.spoken

    # Switch foreground to B and authorize it.
    sessions.set_foreground("B")
    daemon.router._speakable.add("B")

    # Drive more iterations: B's tool cue must now be heard.
    _drain(daemon, n=5)
    assert "Running bash." in speaker.spoken, (
        "B's tool announcement was not spoken after B became the active reader."
    )


# ---------------------------------------------------------------------------
# Test 5b — Background session's decision preempts current reader
# ---------------------------------------------------------------------------

def test_background_decision_preempts_current_reader():
    """A CHOICE (decision) from a background session in the _speakable set must
    be spoken before a non-decision session's prose. Decisions are user-blocking
    and the router drains speakable sessions before the fg session.

    Scenario: A has queued prose; B (speakable) has a blocking choice. The
    router must serve B's decision first, then A's prose.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    # A has prose queued.
    daemon.handle_message(_prose("A", "Long A message. "))
    daemon.router.channel("A").turn_done = True

    # B (background/speakable) submits a blocking choice.
    daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.CHOICE,
        "session": "B",
        "questions": [{"question": "Pick one.", "options": [{"label": "Yes"}, {"label": "No"}]}],
    })
    # Authorize B as speakable (simulates the cooperative hand-off path).
    daemon.router._speakable.add("B")

    # First loop call: the router drains _speakable first -> B's decision is spoken.
    daemon._speak_loop_once()
    assert any("Pick one." in t for t in speaker.spoken), (
        f"B's blocking decision was not spoken first. Spoken: {speaker.spoken!r}"
    )

    # Subsequent iterations: A's prose is spoken after B drains.
    _drain(daemon, n=5)
    assert "Long A message." in speaker.spoken, (
        f"A's prose was not spoken after B drained. Spoken: {speaker.spoken!r}"
    )

    # Ordering: B's decision before A's prose.
    idx_b_decision = next(i for i, t in enumerate(speaker.spoken) if "Pick one." in t)
    idx_a = speaker.spoken.index("Long A message.")
    assert idx_b_decision < idx_a, (
        f"B's decision did not precede A's prose. Spoken: {speaker.spoken!r}"
    )


# ---------------------------------------------------------------------------
# Test 6 — Muted foreground: no audio from it; daemon does not get stuck
# ---------------------------------------------------------------------------

def test_muted_foreground_produces_no_speech():
    """If the active (foreground) session is muted, none of its prose is spoken.

    The daemon must not get stuck -- it should simply skip muted items and
    produce no output (other than mute-exempt cues) for that session.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")

    # Queue some prose for A.
    daemon.handle_message(_prose("A", "You should not hear this. "))
    daemon.router.channel("A").turn_done = True

    # Mute session A (the MUTE handler toggles the active channel).
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})

    # A's channel is now muted.
    assert daemon.router.channel("A").muted, "Channel A should be muted."

    # Drive the loop.
    _drain(daemon, n=10)

    # The prose must not have been spoken.
    assert "You should not hear this." not in speaker.spoken, (
        f"Muted session A spoke prose it shouldn't have. Spoken: {speaker.spoken!r}"
    )

    # The "Session muted." confirmation cue IS mute_exempt and must be heard.
    assert "Session muted." in speaker.spoken, (
        f"'Session muted.' confirmation was not spoken. Spoken: {speaker.spoken!r}"
    )
