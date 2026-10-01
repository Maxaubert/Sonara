"""Hook traffic into the daemon (#141): streamed prose, decisions (CHOICE,
PLAN, PERMISSION), tool announcements, earcons, a new prompt (FLUSH), the
session lifecycle (SESSION_START/END, SET_FOREGROUND, FORGET_SESSION) and an
answered question (CHOICE_ANSWERED).

Owns the per-session prose assemblers, the unanswered-question guard
(await_choice) and the once-per-session "selecting is immediate" warning, and
registers all three with the per-session state registry."""
from __future__ import annotations

from sonara import config_schema
from sonara.assembler import PARAGRAPH_BREAK, ProseAssembler
from sonara.cleaner import normalize_for_speech
from sonara.daemon import core, decision_text
from sonara.protocol import MsgType
from sonara.queue import SpeechItem


class Ingest:
    """Message handlers for hook traffic. Given the daemon (*daemon*) for the
    core state every handler touches (lock-held: router, history, speaker,
    summary pipeline, heard-markers, ids) and looks it up at call time, so a
    test that replaces a daemon attribute is honoured. Every handler runs
    with the daemon lock held, like the old handle_message branches."""

    def __init__(self, daemon, registry: core.SessionRegistry) -> None:
        self._d = daemon
        self.assemblers: dict = {}
        # Sessions with an unanswered AskUserQuestion: suppress the redundant
        # permission prompt it also fires.
        self.await_choice: set = set()
        self.warned_immediate: set = set()
        registry.register("warned_immediate", self.warned_immediate)
        registry.register("assemblers", self.assemblers)
        # A stale await_choice entry from a dead session would suppress
        # permission chimes DAEMON-WIDE forever: the chime carries no session,
        # so the suppression check is global truthiness (audit #19).
        registry.register("await_choice", self.await_choice)

    def register(self, table: dict) -> None:
        core.add_handlers(table, {
            MsgType.PROSE: self.on_prose,
            MsgType.CHOICE: self.on_choice,
            MsgType.PLAN: self.on_plan,
            MsgType.PERMISSION: self.on_permission,
            MsgType.TOOL: self.on_tool,
            MsgType.EARCON: self.on_earcon,
            MsgType.FLUSH: self.on_flush,
            MsgType.SET_FOREGROUND: self.on_foreground,
            MsgType.SESSION_START: self.on_foreground,
            MsgType.SESSION_END: self.on_session_end,
            MsgType.CHOICE_ANSWERED: self.on_choice_answered,
            MsgType.FORGET_SESSION: self.on_forget_session,
            MsgType.SPEAK: self.on_speak,
        })

    def _verbosity(self):
        return config_schema.get(self._d.config, "verbosity")

    def assembler(self, session: str) -> ProseAssembler:
        a = self.assemblers.get(session)
        if a is None:
            a = ProseAssembler()
            self.assemblers[session] = a
        return a

    def selection_cue(self, session: str, verbosity: str) -> str:
        if verbosity != "everything":
            return ""
        cue = "Press the option's number to choose, or Escape to cancel."
        if session not in self.warned_immediate:
            self.warned_immediate.add(session)
            cue += " Selecting is immediate."
        return cue

    def guide_setup(self, session: str, plugin_version: str) -> None:
        """Speak ONE setup-guidance cue for this session, only when degraded
        (see setup_health.SetupGuide.cue_for)."""
        d = self._d
        cue = d._setup_guide.cue_for(session, plugin_version)
        if cue:
            # A control cue (F6): on the session channel the prompt that
            # usually follows SESSION_START at once (FLUSH) wiped it unheard.
            d._cues.speak(session, cue)

    def on_prose(self, msg):
        d = self._d
        session = msg.get("session", "")
        verbosity = self._verbosity()
        final = msg.get("final", False)
        a = self.assembler(session)
        chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), final)
        ch = d.router.channel(session)
        for chunk in chunks:
            if chunk is PARAGRAPH_BREAK:
                d.history.end_message(session)
                continue
            entry = d.history.record(session, "prose", chunk)
            # Quiet verbosity AND summary mode both record prose to history
            # without enqueueing speech (summary mode reads a recap at turn
            # end instead; repeat / Up still work from history).
            if verbosity != "quiet" and not d.config.get("summary_mode"):
                item = SpeechItem(id=d._alloc_id(), session=session, kind="prose",
                                  text=chunk, is_decision=False)
                d._pending_heard[item.id] = entry
                ch.append(item)
        if final:
            # NOTE: turn_done is NOT set here -- a per-block "final" flag means
            # this text block finished, but the TURN ends only when the
            # turn_done earcon (or FLUSH) arrives. This keeps minqueue batching
            # correct: items accumulate until the threshold OR the turn ends.
            d.history.end_message(session)
        # Wake the speak loop ONLY when a batch is actually ready to read
        # (>= minqueue, the turn is done, or a decision is waiting). Waking on
        # every buffered delta made the loop spin on the daemon lock and starve
        # the hotkey worker -- the root cause of the "thinking" mute-hang. A
        # finished turn wakes via the turn_done earcon / TOOL / FLUSH paths; the
        # speak loop's poll_interval is the safety net if a wake is ever missed.
        # Late prose after turn_done: reset the settle window so the turn-end
        # digest waits for the full turn to land (#14). Only when armed.
        d._summary.on_prose(session)
        if ch.ready(d._minqueue()):
            d._wake.set()
        return None

    # Decision CONTENT is enqueued (and gated by foreground). The ALERT
    # earcon for a decision travels as a SEPARATE EARCON message that
    # hooks_entry emits BEFORE the content message; it is handled by
    # on_earcon, so the earcon fires instantly and cross-session WITHOUT
    # being doubled here.

    def on_choice(self, msg):
        # A question BLOCKS the turn (no turn_done -> no end-of-turn digest), so
        # its lead-in prose must be voiced before the question. But the CHOICE
        # can reach the daemon BEFORE its lead-in prose (separate hook processes
        # race), so gathering the lead-in now would find nothing and speak the
        # question alone. Build the question item now, then DEFER the lead-in
        # gather + hold/enqueue through the settle window (#16).
        d = self._d
        session = msg.get("session", "")
        text = decision_text.choice_text(msg)
        extras = [e for e in (decision_text.choice_notes(msg),
                              self.selection_cue(session, self._verbosity())) if e]
        if extras:
            text = "{0} {1}".format(text, " ".join(extras))
        entry = d.history.record(session, "choice", text)
        d.history.end_message(session)
        item = SpeechItem(id=d._alloc_id(), session=session, kind="choice",
                          text=text, is_decision=True)
        d._pending_heard[item.id] = entry
        # AskUserQuestion ALSO fires a permission-prompt notification ~5-6s
        # later; mark the question unanswered so that redundant permission
        # (earcon + text) is suppressed until the turn moves on (issue #11 f/u).
        # Set this NOW (not at settle fire) so the suppression is armed before
        # the permission can arrive.
        self.await_choice.add(session)
        # Summary mode: defer the lead-in digest + question through the settle
        # window so late lead-in prose is included, heard before the question.
        # Non-summary speaks prose live, so enqueue the question immediately.
        d._summary.on_decision(session, item)
        return None

    def on_plan(self, msg):
        d = self._d
        session = msg.get("session", "")
        text = decision_text.plan_text(msg)
        cue = self.selection_cue(session, self._verbosity())
        if cue:
            text = "{0} {1}".format(text, cue)
        entry = d.history.record(session, "plan", text)
        d.history.end_message(session)
        item = SpeechItem(id=d._alloc_id(), session=session, kind="plan",
                          text=text, is_decision=True)
        d._pending_heard[item.id] = entry
        # Same hook race as CHOICE (#16): the PLAN can beat its lead-in prose,
        # so defer the lead-in gather + hold through the settle window in
        # summary mode; the context is then heard before the plan (audit #21).
        d._summary.on_decision(session, item)
        return None

    def on_permission(self, msg):
        d = self._d
        session = msg.get("session", "")
        # Redundant permission that pairs with an unanswered AskUserQuestion:
        # the question was already announced, so drop this one. CONSUME the
        # guard here (the permission it exists to suppress has now arrived) --
        # do NOT rely on unrelated prose/turn_done to clear it, since the
        # pre-question prose streams in AFTER the choice and would clear it
        # early (confirmed via message-sequence capture, issue #11 f/u).
        if session in self.await_choice or (not session and self.await_choice):
            self.await_choice.discard(session)
            return None
        text = decision_text.permission_text(msg)
        cue = self.selection_cue(session, self._verbosity())
        if cue:
            text = "{0} {1}".format(text, cue)
        entry = d.history.record(session, "permission", text)
        d.history.end_message(session)
        item = SpeechItem(id=d._alloc_id(), session=session, kind="permission",
                          text=text, is_decision=True)
        d._pending_heard[item.id] = entry
        # Same hook race as CHOICE (#16): defer through the settle window in
        # summary mode so a late lead-in is digested and heard first (audit #21).
        d._summary.on_decision(session, item)
        return None

    def on_tool(self, msg):
        d = self._d
        session = msg.get("session", "")
        self.await_choice.discard(session)  # a tool ran -> the question was answered
        if self._verbosity() == "everything":
            tool = msg.get("tool", "")
            summary = (msg.get("summary") or "").strip()
            text = summary if summary else "Running {0}.".format(tool)
            ch = d.router.channel(session)
            # A tool announcement is immediate: flush any held prose
            # (below the minqueue threshold) so it reads before the cue.
            ch.turn_done = True
            ch.append(SpeechItem(
                id=d._alloc_id(), session=session, kind="tool_announce",
                text=text, is_decision=False))
            d._wake.set()
        return None

    def on_earcon(self, msg):
        # Instant: the Windows earcon backend plays on a separate audio path
        # that mixes with the speech, so it no longer cuts the reading.
        d = self._d
        session = msg.get("session", "")
        kind = msg.get("kind", "")
        # Suppress the redundant permission chime that pairs with an unanswered
        # AskUserQuestion (its message carries no session, so gate on "any
        # question awaiting"). Real permission chimes still fire (issue #11 f/u).
        if kind == "permission" and self.await_choice:
            return None
        d._earcon(kind)
        if kind == "turn_done":
            # End-of-turn boundary: safety-net flush in case the final PROSE
            # flag never arrived. Wake the loop so a sub-threshold batch that was
            # left buffered (no per-delta wake) is read now, not after a poll.
            d.router.channel(session).turn_done = True
            d._wake.set()
            # Do NOT digest yet: the turn's final prose can arrive after this
            # signal, so summary mode arms a settle window first (#14).
            d._summary.on_turn_done(session)
        return None

    def on_flush(self, msg):
        d = self._d
        session = msg.get("session", "")
        cur = d._current_item
        if cur is not None and cur.session == session:
            d.speaker.cancel()
        d._drop_channel_pending(session)
        ch = d.router.channel(session)
        ch.wipe()
        seed = d.digest_store.get(session)
        if seed:
            # Keep the session cycle-reachable while its new turn cooks
            # (#118): a bare-wiped channel fell out of the manual ring
            # (empty channels are skipped, #117) for the WHOLE turn. The
            # persisted last digest re-seeds it as an already-heard,
            # replay-only item; real content replaces it.
            ch.seed(SpeechItem(id=d._alloc_id(), session=session,
                               kind="summary", text=seed,
                               is_decision=False))
        self.assemblers.pop(session, None)
        d.history.reset(session)
        # A new prompt is the user cancelling this session: advance the cancel
        # epoch so any digest dispatched before now is dropped when it lands,
        # rather than spoken into the new turn (#13); abandon any settling
        # turn (#14) and drop held or deferred questions (#16).
        d._summary.cancel(session)
        d._last_digest_text.pop(session, None)   # no re-reading a stale digest
        self.await_choice.discard(session)       # new prompt: no question pending
        # A new prompt auto-resumes a paused voice only when it comes from
        # the session the user hears (upstream #69): a background
        # session's /loop tick or agent completion also sends FLUSH, and
        # used to un-pause the voice the user deliberately held.
        if d._controls.engaged_session() == session:
            d._paused.clear()
        d._wake.set()
        return None

    def on_foreground(self, msg):
        """SET_FOREGROUND and SESSION_START."""
        d = self._d
        t = msg.get("type")
        session = msg.get("session", "")
        old_fg = d.sessions.foreground()
        # Deliberately NOT gated on a paused voice (upstream #69 / #65):
        # under the default earcon_only policy a non-foreground session's
        # live prose is never voiced, so refusing a genuine prompt here
        # would silently drop its turn. A held voice stays held through
        # the FLUSH gate, and the cooperative drain keeps the paused
        # session's interrupted message first on resume.
        d.sessions.set_foreground(session, cwd=msg.get("cwd"))
        # The embedding host's tab (#143); absent outside a host.
        d.sessions.set_host_tab(session, msg.get("host_tab"))
        if t == MsgType.SESSION_START:
            d.sessions.register(session, cwd=msg.get("cwd"))
            self.guide_setup(session, msg.get("plugin_version", ""))
        # Cooperative hand-off: if the old foreground still has pending items,
        # authorize it to drain before the new fg takes the floor. This is the
        # "session B arrives while A is mid-response" case. authorize_replay
        # bypasses the policy gate for the drain (A finished reading is a
        # natural completion, not a user-visible session switch).
        if old_fg is not None and old_fg != session:
            old_ch = d.router.channels.get(old_fg)
            if old_ch is not None and old_ch.pending() > 0:
                d.router.authorize_replay(old_fg)
        return None

    def on_session_end(self, msg):
        d = self._d
        session = msg.get("session", "")
        d.sessions.unregister(session)
        if ":" in session:
            # A SPEAK session ("<source>:<tab>"): host tab ids change every
            # host run, so its label must not pile up on disk. Claude
            # session ids have no colon and keep their prefs for resume.
            d.session_prefs.forget(session)
        d._teardown_session(session)
        d.router.drop(session)
        return None

    def on_choice_answered(self, msg):
        # The user ANSWERED the blocking question (#83): they have heard (or
        # read) everything they need up to it. Silence the stale backlog and
        # any in-flight lead-in digest; whatever the assistant says AFTER the
        # answer flows normally. No earcon: answering is its own feedback.
        self._d._controls.user_caught_up(msg.get("session", ""))
        return None

    def on_speak(self, msg):
        """SPEAK from an embedding host (#143): read *text* in the session
        f"{source}:{tab or 'default'}". Queue of one: the text replaces the
        session's turn (unread items and history), and the text is spoken as
        given (cleaned for speech; no assembly, no summary). interrupt=true
        also cuts the session's current utterance; other sessions are left
        alone, and the global pause holds (a host must not un-pause the
        voice, the #69 lesson)."""
        d = self._d
        text, source = msg.get("text"), msg.get("source")
        tab, label = msg.get("tab"), msg.get("label")
        if not (isinstance(text, str) and isinstance(source, str) and source
                and (tab is None or isinstance(tab, str))):
            return None
        sid = "{0}:{1}".format(source, tab or "default")
        d.sessions.register(sid, cwd=None)
        d.sessions.set_host_tab(sid, tab)
        if isinstance(label, str) and label and label != d.session_prefs.name(sid):
            d.session_prefs.set(sid, "name", label)   # the router announces it
        cur = d._current_item
        if msg.get("interrupt") is True and cur is not None and cur.session == sid:
            d.speaker.cancel()
        # One message, always the last (FE-1): each SPEAK replaces the
        # session's turn, like a new prompt does for a Claude session, so
        # read texts never pile up and Up replays only the latest. A
        # non-interrupt SPEAK still lets the current utterance finish.
        d._drop_channel_pending(sid)
        ch = d.router.channel(sid)
        ch.wipe()
        d.history.reset(sid)
        d._last_digest_text.pop(sid, None)
        spoken = normalize_for_speech(text)
        if spoken:
            entry = d.history.record(sid, "summary", spoken)
            d.history.end_message(sid)
            d._last_digest_text[sid] = spoken   # summary-mode Up re-reads it
            d._enqueue(sid, "summary", spoken, False, entry=entry)
            ch.turn_done = True   # whole text at once: no minqueue wait
            # Not a foreground Claude session: authorize it past the
            # background policy until it drains, like a digest delivery.
            d.router.authorize_replay(sid)
        return None

    def on_forget_session(self, msg):
        d = self._d
        sid = msg.get("session")
        if not isinstance(sid, str) or d.sessions.is_foreground(sid):
            return None
        d.sessions.unregister(sid)
        d.session_prefs.forget(sid)
        # Forget targets exactly the stale sessions that died WITHOUT
        # SessionEnd, so it needs the same per-session teardown (#101).
        d._teardown_session(sid)
        d.router.drop(sid)
        return None
