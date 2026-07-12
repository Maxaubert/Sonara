"""End-to-end pipeline test: real hooks_entry -> real SpeechDaemon -> recording FakeSpeaker.

Proves the ordering contract from the design spec section 4:
  - a decision earcon fires IMMEDIATELY when the decision appears,
  - but the decision's spoken text is FIFO and is voiced only AFTER the
    preceding prose,
  - foreground gating works,
  - a turn_done earcon ends the turn.
No real audio is ever produced: the Speaker is replaced by a recorder.
"""
from sonara.hooks_entry import handle_event
from sonara.protocol import PROTOCOL_VERSION
from sonara.sessions import SessionManager
from sonara.daemon import SpeechDaemon
from sonara.config import DEFAULTS


SID = "sess-e2e-1"


class FakeSpeaker:
    """Records spoken text and earcons into one shared ordered list.

    Mirrors the public surface SpeechDaemon uses: speak/cancel/earcon/
    set_voice/set_rate. speak() is synchronous here (no threads) so the
    drain helper produces a deterministic ordering.
    """

    def __init__(self, log):
        self.log = log
        self.voice = None
        self.rate = DEFAULTS["rate"]
        self.cancelled = 0
        self._epoch = 0

    def speak(self, text, cancel_epoch=None, on_play=None):
        self.log.append(("text", text))

    def cancel_epoch(self):
        return self._epoch

    def cancel(self):
        self.cancelled += 1
        self._epoch += 1

    def earcon(self, kind):
        self.log.append(("earcon", kind))

    def set_voice(self, v):
        self.voice = v

    def set_rate(self, r):
        self.rate = r


def drain_queue(daemon, speaker):
    """Run the speak-loop logic to exhaustion via the router.

    Calls router.next_item() in a loop and speaks each item until the router
    has no more ready items.  This mirrors what SpeechDaemon._speak_loop does
    per iteration, minus the blocking wait, so the test is deterministic.
    """
    while True:
        item = daemon.router.next_item()
        if item is None:
            return
        speaker.speak(item.text)


def make_daemon():
    log = []
    speaker = FakeSpeaker(log)
    sessions = SessionManager(background_policy="earcon_only")
    cfg = {k: (dict(v) if isinstance(v, dict) else v) for k, v in DEFAULTS.items()}
    cfg["verbosity"] = "everything"
    daemon = SpeechDaemon(speaker, sessions, cfg)
    daemon._setup_health = lambda v: ("ok", None)  # no setup cue in ordering tests
    return daemon, speaker, log


def feed_event(daemon, event, payload):
    """Run a hook event through the real handle_event and feed every
    resulting protocol message into the real daemon, just like bin/sonara-hook
    -> client -> daemon would in production."""
    for msg in handle_event(event, payload):
        assert msg["v"] == PROTOCOL_VERSION
        daemon.handle_message(msg)


def test_scripted_session_full_ordering():
    daemon, speaker, log = make_daemon()

    # 1. SessionStart: registers + sets foreground (no audio yet).
    feed_event(daemon, "SessionStart", {"session_id": SID})

    # 2. MessageDisplay: two sentences of prose, streamed as one final delta.
    feed_event(daemon, "MessageDisplay", {
        "session_id": SID,
        "delta": "Let me check the files. I will start now.",
        "index": 0,
        "final": True,
    })

    # 3. PreToolUse AskUserQuestion with two options.
    #    -> choice earcon fires IMMEDIATELY (recorded now),
    #       choice TEXT is enqueued AFTER the queued prose (recorded on drain).
    feed_event(daemon, "PreToolUse", {
        "session_id": SID,
        "tool_name": "AskUserQuestion",
        "tool_input": {
            "questions": [{
                "question": "Which approach?",
                "options": [
                    {"label": "Refactor"},
                    {"label": "Rewrite"},
                ],
            }],
        },
    })

    # Drain everything queued so far: prose sentences first, THEN the choice
    # text. The choice EARCON is already in the log from step 3 (before any
    # of this prose was spoken) -> that is the ordering proof.
    drain_queue(daemon, speaker)

    # 4. UserPromptSubmit: user answered + sent a new prompt -> flush + fg.
    feed_event(daemon, "UserPromptSubmit", {"session_id": SID})

    # 5. MessageDisplay: more prose for the new turn.
    feed_event(daemon, "MessageDisplay", {
        "session_id": SID,
        "delta": "Applying the change now.",
        "index": 0,
        "final": True,
    })

    # 6. PreToolUse Bash -> permission via Notification permission_prompt.
    #    permission earcon fires immediately; permission text enqueued after.
    feed_event(daemon, "Notification", {
        "session_id": SID,
        "notification_type": "permission_prompt",
        "action": "Run: pytest -q",
    })

    drain_queue(daemon, speaker)

    # 7. Stop: turn_done earcon ends the turn.
    feed_event(daemon, "Stop", {"session_id": SID})
    drain_queue(daemon, speaker)

    assert log == [
        ("earcon", "choice"),
        ("text", "Let me check the files."),
        ("text", "I will start now."),
        ("text", "Which approach? Option 1: Refactor. Option 2: Rewrite. Press the option's number to choose, or Escape to cancel. Selecting is immediate."),
        ("earcon", "permission"),
        ("text", "Applying the change now."),
        ("text", "Run: pytest -q Press the option's number to choose, or Escape to cancel."),
        ("earcon", "turn_done"),
    ]


def test_background_session_is_earcon_only():
    """A non-foreground session still fires decision earcons but its prose
    and decision TEXT are NOT spoken (foreground gating by the router)."""
    daemon, speaker, log = make_daemon()

    feed_event(daemon, "SessionStart", {"session_id": "fg"})
    # Background session never becomes foreground.
    feed_event(daemon, "MessageDisplay", {
        "session_id": "bg",
        "delta": "Background chatter that must stay silent.",
        "index": 0,
        "final": True,
    })
    feed_event(daemon, "PreToolUse", {
        "session_id": "bg",
        "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{
            "question": "Pick one",
            "options": [{"label": "A"}, {"label": "B"}],
        }]},
    })
    drain_queue(daemon, speaker)

    # Earcon fired (alerts are cross-session), but no text was spoken
    # (router only reads the foreground session "fg", which has no items).
    assert log == [("earcon", "choice")]
