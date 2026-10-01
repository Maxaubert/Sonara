from __future__ import annotations

import os
import socket
import sys
import threading

from sonara.protocol import MsgType
from sonara.queue import SpeechItem
from sonara.assembler import ProseAssembler
from sonara import config_schema
from sonara.daemon import decision_text, setup_health, tokens
from sonara.daemon.audio import AudioControl
from sonara.daemon.core import SessionRegistry, SharedState
from sonara.daemon.cues import Cues
from sonara.daemon.hotkeys import HotkeyController
from sonara.daemon.playback import SpeakLoop
from sonara.daemon.server import ConnectionServer
from sonara.daemon.summary.pipeline import SummaryPipeline, summary_log
from sonara.daemon.summary.reorder import DigestReorderBuffer
from sonara.config import save_config, load_config
from sonara.paths import (
    LOCK_PATH, SINGLETON_PATH, ensure_sonara_dir, socket_connectable,
    SESSIONS_PATH, SESSION_PREFS_PATH, SESSION_SEEN_PATH,
    SESSION_DIGESTS_PATH, package_root,
)
from sonara.platform import transport
# Re-exported: ensure_running moved to sonara.lifecycle (#141) so clients
# start the daemon without importing it.
from sonara.lifecycle import ensure_running  # noqa: F401

# Holds the single-instance flock for this process's lifetime (see main()).
_SINGLETON = None
_MUTEX = None       # process-lifetime handle to the named single-instance mutex


# Setting bounds live in the config schema (#136); re-exported for callers.
RATE_MIN = config_schema.RATE_MIN
RATE_MAX = config_schema.RATE_MAX
MINQUEUE_MIN = config_schema.MINQUEUE_MIN
MINQUEUE_MAX = config_schema.MINQUEUE_MAX

# Startup channel rehydration horizon (#118): sessions seen within this window
# get their persisted last digest re-seeded as a replayable channel, so the
# manual cycle reaches them across daemon restarts. Matches the settings
# page's "recent sessions" threshold.
_REHYDRATE_WINDOW_S = 3 * 3600


class SpeechDaemon:
    def __init__(self, speaker, sessions, config, ducker=None, pauser=None,
                 prefs=None, digests=None) -> None:
        self.speaker = speaker
        self.sessions = sessions
        self.config = config
        if ducker is None:
            from sonara.platform.windows.ducking import NullDucker
            ducker = NullDucker()
        self.ducker = ducker
        if pauser is None:
            from sonara.platform.windows.pausing import NullPauser
            pauser = NullPauser()
        self.pauser = pauser
        if prefs is None:
            from sonara.session_prefs import SessionPrefs
            prefs = SessionPrefs()
        self.session_prefs = prefs
        if digests is None:
            from sonara.digest_store import DigestStore
            digests = DigestStore()
        self.digest_store = digests
        self._assemblers = {}
        self._next_id = 0
        from sonara.router import Router
        self.router = Router(
            self.sessions,
            minqueue=self._minqueue,
            announce_text=lambda folder, replay=False: (
                "Session changed: {0}, reading again.".format(folder) if replay
                else "Session changed: {0}.".format(folder)),
            display_name=lambda sid: self.session_prefs.name(sid),
            channel_init=lambda ch: setattr(
                ch, "muted", self.session_prefs.muted(ch.session)),
        )
        self._running = threading.Event()
        self._wake = threading.Event()
        self._lock = threading.Lock()
        # The loopback socket: accept loop, token check, handler threads.
        # handle_message is looked up at call time, as for the hotkeys.
        self._server = ConnectionServer(
            self._running, self._lock, lambda m: self.handle_message(m))
        self._webui = None
        from sonara.history import SessionHistory
        self.history = SessionHistory(cap=int(config_schema.get(config, "history_cap")))
        self._pending_heard: dict = {}            # SpeechItem.id -> HistoryEntry
        # Shared by several features (#141): the item being spoken and the
        # per-session text summary-mode Up re-reads verbatim.
        self._shared = SharedState()
        self._await_choice: set = set()           # sessions with an unanswered AskUserQuestion (suppress the redundant permission prompt it also fires)
        self._paused = threading.Event()          # play/pause: set == speech halted
        # Mute cycle: 0=unmuted, 1=muted (prose off, beeps on), 2=super muted
        # (prose AND beeps off). RESTORED from config (#65): hooks silently
        # respawn a dead daemon between two messages, and a memory-only mute
        # was reset to audible by the swap - the "mute is not persistent" bug.
        self._mute_level = config_schema.current(config, "mute_level")
        # Digest reorder buffer (#88): turn-end digests become AUDIBLE in
        # dispatch (turn-finish) order, not summarizer-completion order.
        self._digests = DigestReorderBuffer(lock=self._lock, log=summary_log)
        # Control cues on the CONTROL channel, their voice, Kokoro notices.
        self._cues = Cues(config, self.router, speaker, self.session_prefs,
                          alloc_id=self._alloc_id,
                          current_item=lambda: self._current_item,
                          wake=self._wake)
        # Ducking / media pause around speech, and the speech volume. persist
        # resolves save_config here at call time (tests patch it on this module).
        self._audio = AudioControl(
            config, self.ducker, self.pauser, speaker,
            persist=lambda: save_config(self.config), cues=self._cues,
            cue_target=lambda: self.router.active or self.sessions.foreground(),
            wake=self._wake)
        self._warned_immediate: set = set()
        self._setup_guide = setup_health.SetupGuide()   # one setup cue per session
        # Global hotkeys: fires are queued by the pump thread and applied by a
        # worker under self._lock, like a socket message. handle_message is
        # looked up at call time so a replaced handler (tests) is honoured.
        self._hotkeys = HotkeyController(
            self._lock, self._running, lambda m: self.handle_message(m),
            self._cues)
        # Summary mode: settle window, lead-in and turn-end digests, the
        # question hold and the summarizer worker (daemon/summary/pipeline).
        self._summary = SummaryPipeline(
            config, self._lock, self._wake, self.router, sessions,
            self.history, self.digest_store, self._digests, self._shared,
            self._pending_heard, enqueue=self._enqueue, replay=self._replay,
            earcon=self._earcon)
        # The speak loop and the deferred handoff alert (daemon/playback).
        # note_spoken and _requeue_or_note are looked up at call time
        # (tests replace them on the daemon).
        self._playback = SpeakLoop(
            config, self._lock, self._wake, self._running, self._paused,
            self.router, speaker, self._cues, self._audio, self._shared,
            self._pending_heard, muted=lambda: self._muted,
            earcon=self._earcon,
            note_spoken=lambda item, completed: self.note_spoken(item, completed),
            requeue_or_note=lambda item, completed: self._requeue_or_note(
                item, completed))
        # Per-session state (#141): ending or forgetting a session clears
        # everything registered here (_teardown_session).
        self._session_state = SessionRegistry()
        reg = self._session_state
        reg.register_hook("pending_heard", self._drop_channel_pending)
        reg.register_hook("history", self.history.reset)
        reg.register("warned_immediate", self._warned_immediate)
        reg.register_hook("setup_guide", self._setup_guide.forget)
        # Ending the session is a user action like FLUSH: cancel its summary
        # work (bumps the cancel epoch and the settle generation, drops held
        # and deferred questions). A late worker must find no held question
        # to append (zombie channel), and nothing here may outlive the
        # session (audit #21).
        reg.register_hook("summary", self._summary.cancel)
        reg.register("last_digest_text", self._last_digest_text)
        # Ended sessions don't rehydrate (#118).
        reg.register_hook("digest_store", self.digest_store.forget)
        reg.register("assemblers", self._assemblers)
        # A stale _await_choice entry from a dead session would suppress
        # permission chimes DAEMON-WIDE forever: the chime carries no session,
        # so the suppression check is global truthiness (audit #19).
        reg.register("await_choice", self._await_choice)

    @property
    def _current_item(self):
        """The item being spoken right now (core.SharedState)."""
        return self._shared.current_item

    @_current_item.setter
    def _current_item(self, item) -> None:
        self._shared.current_item = item

    @property
    def _last_digest_text(self) -> dict:
        """session -> exact spoken digest text (core.SharedState)."""
        return self._shared.last_digest_text

    def _alloc_id(self) -> int:
        self._next_id += 1
        return self._next_id

    @property
    def _muted(self) -> bool:
        """True when speech is muted (level 1 muted OR level 2 super muted). Used by
        the speak loop to drop non-exempt prose in both muted states."""
        return self._mute_level >= 1

    def _earcon(self, kind: str) -> None:
        """Fire an earcon unless super-muted (level 2). At level 0/1 beeps play; at
        level 2 every beep is suppressed (full mute)."""
        if self._mute_level < 2:
            self.speaker.earcon(kind)

    def _assembler(self, session: str) -> ProseAssembler:
        a = self._assemblers.get(session)
        if a is None:
            a = ProseAssembler()
            self._assemblers[session] = a
        return a

    def _enqueue(self, session: str, kind: str, text: str, is_decision: bool,
                 entry=None, mute_exempt: bool = False,
                 pause_exempt: bool = False) -> None:
        item = SpeechItem(
            id=self._alloc_id(),
            session=session,
            kind=kind,
            text=text,
            is_decision=is_decision,
            mute_exempt=mute_exempt,
            pause_exempt=pause_exempt,
        )
        if entry is not None:
            self._pending_heard[item.id] = entry
        # append() replaces a placeholder seed (#118), bumps gen (the #115
        # suppression lift) and ends a replay-in-progress (#118).
        self.router.channel(session).append(item)
        self._wake.set()

    def _minqueue(self) -> int:
        return config_schema.current(self.config, "minqueue")

    def _maybe_guide_setup(self, session: str, plugin_version: str) -> None:
        """Speak ONE setup-guidance cue for this session, only when degraded
        (see setup_health.SetupGuide.cue_for)."""
        cue = self._setup_guide.cue_for(session, plugin_version)
        if cue:
            self._enqueue(session, "prose", cue, False)

    def _drop_channel_pending(self, session: str) -> None:
        """Drop heard-tracking entries for a session's not-yet-spoken channel items
        (called before wiping/dropping the channel, so _pending_heard can't leak)."""
        ch = self.router.channels.get(session)
        if ch is not None:
            for it in ch.items:
                self._pending_heard.pop(it.id, None)

    def _rehydrate_channels(self) -> None:
        """Re-seed each recently-seen session's channel with its persisted last
        digest as an already-heard item (#118). Channels are in-memory, so a
        restart emptied every queue and the manual cycle could only reach
        sessions that spoke SINCE the restart - everyone else's last message
        was lost. Rehydrated channels are caught up (pending 0): never
        auto-spoken, but landing on them replays the digest like any read
        session, and Up re-reads it."""
        import time
        now = time.time()
        with self._lock:
            for sid, text in self.digest_store.items():
                if not text or sid in self.router.channels:
                    continue
                seen = self.sessions.last_seen(sid)
                if seen is None or (now - seen) > _REHYDRATE_WINDOW_S:
                    continue
                # Heard, replay-only (no auto-speak); real content replaces it.
                self.router.channel(sid).seed(SpeechItem(
                    id=self._alloc_id(), session=sid, kind="summary",
                    text=text, is_decision=False))
                self._last_digest_text[sid] = text   # Up re-read parity

    def _teardown_session(self, session: str) -> None:
        """Per-session cleanup shared by SESSION_END and FORGET_SESSION (#101):
        both retire a session's live state; FORGET_SESSION targets exactly the
        sessions that died WITHOUT a SessionEnd, so its cleanup must match.
        Callers run this BEFORE router.drop(session): the pending_heard
        hook (_drop_channel_pending) needs the channel to still exist, or
        _pending_heard leaks. Every feature's per-session state is in the
        registry, so nothing here is hand-listed (#141)."""
        self._session_state.forget_session(session)

    def note_spoken(self, item, completed: bool) -> None:
        """Speak-loop bookkeeping: confirm (or decline) the heard-marker for a
        finished utterance."""
        with self._lock:
            self._current_item = None
            entry = self._pending_heard.pop(item.id, None)
            if entry is not None and completed:
                entry.heard = True
            # A HEARD question joins the session's re-read record: digests SET
            # the record (a new turn unit), decisions APPEND, so summary-mode Up
            # replays "lead-in + question" -- or a bare question on its own --
            # instead of the dead edge chime (live report 2026-07-14). The Up
            # re-read insert is is_decision=False, so re-reads never re-append.
            if completed and item.is_decision and item.text:
                prev = self._last_digest_text.get(item.session)
                self._last_digest_text[item.session] = (
                    prev + " " + item.text) if prev else item.text
                self.digest_store.set(item.session,
                                      self._last_digest_text[item.session])

    def _requeue_or_note(self, item, completed) -> bool:
        """On a pause-interrupted utterance, re-queue it so resume re-speaks it and
        return True (skip note_spoken). Returns False otherwise (caller notes it).
        A session-change announcement owns no channel cursor position (it comes
        from the router's pending-announce, id 0), so re-arm the announcement
        instead of rewinding a real content item (which double-spoke/lost it)."""
        with self._lock:
            if not (not completed and self._paused.is_set()):
                return False
            if item.kind == "session_change":
                # Re-arm all three announce fields (M13): dropping manual
                # sent a paused manual switch down the deferred auto path.
                self.router.rearm_announce(item.session, replay=item.replay,
                                           manual=item.manual)
            else:
                ch = self.router.channels.get(item.session)
                if ch is not None:
                    ch.rewind()
            self._current_item = None
            return True

    def _selection_cue(self, session: str, verbosity: str) -> str:
        if verbosity != "everything":
            return ""
        cue = "Press the option's number to choose, or Escape to cancel."
        if session not in self._warned_immediate:
            self._warned_immediate.add(session)
            cue += " Selecting is immediate."
        return cue

    def handle_message(self, msg):
        t = msg.get("type")
        session = msg.get("session", "")
        verbosity = config_schema.get(self.config, "verbosity")
        # Liveness for the Sessions tab: any session-bearing hook traffic
        # counts as activity. Settings-page mutations are excluded, or naming
        # a stale row would bump it back into the recent list.
        if (isinstance(session, str) and session
                and t not in (MsgType.SET_SESSION_PREF, MsgType.FORGET_SESSION)):
            self.sessions.touch(session)

        if t == MsgType.PROSE:
            final = msg.get("final", False)
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), final)
            from sonara.assembler import PARAGRAPH_BREAK
            ch = self.router.channel(session)
            for chunk in chunks:
                if chunk is PARAGRAPH_BREAK:
                    self.history.end_message(session)
                    continue
                entry = self.history.record(session, "prose", chunk)
                # Quiet verbosity AND summary mode both record prose to history
                # without enqueueing speech (summary mode reads a recap at turn
                # end instead; repeat / Up still work from history).
                if verbosity != "quiet" and not self.config.get("summary_mode"):
                    item = SpeechItem(id=self._alloc_id(), session=session, kind="prose",
                                      text=chunk, is_decision=False)
                    self._pending_heard[item.id] = entry
                    ch.append(item)
            if final:
                # NOTE: turn_done is NOT set here -- a per-block "final" flag means
                # this text block finished, but the TURN ends only when the
                # turn_done earcon (or FLUSH) arrives. This keeps minqueue batching
                # correct: items accumulate until the threshold OR the turn ends.
                self.history.end_message(session)
            # Wake the speak loop ONLY when a batch is actually ready to read
            # (>= minqueue, the turn is done, or a decision is waiting). Waking on
            # every buffered delta made the loop spin on self._lock and starve the
            # hotkey worker -- the root cause of the "thinking" mute-hang. A finished
            # turn wakes via the turn_done earcon / TOOL / FLUSH paths below; the
            # speak loop's poll_interval is the safety net if a wake is ever missed.
            # Late prose after turn_done: reset the settle window so the turn-end
            # digest waits for the full turn to land (#14). Only when armed.
            self._summary.on_prose(session)
            if ch.ready(self._minqueue()):
                self._wake.set()
            return None

        # Decision CONTENT is enqueued (and gated by foreground). The ALERT
        # earcon for a decision travels as a SEPARATE EARCON message that
        # hooks_entry emits BEFORE the content message; it is handled by the
        # MsgType.EARCON branch below, so the earcon fires instantly and
        # cross-session WITHOUT being doubled here.
        if t == MsgType.CHOICE:
            # A question BLOCKS the turn (no turn_done -> no end-of-turn digest), so
            # its lead-in prose must be voiced before the question. But the CHOICE
            # can reach the daemon BEFORE its lead-in prose (separate hook processes
            # race), so gathering the lead-in now would find nothing and speak the
            # question alone. Build the question item now, then DEFER the lead-in
            # gather + hold/enqueue through the settle window (#16).
            text = decision_text.choice_text(msg)
            extras = [e for e in (decision_text.choice_notes(msg),
                                  self._selection_cue(session, verbosity)) if e]
            if extras:
                text = "{0} {1}".format(text, " ".join(extras))
            entry = self.history.record(session, "choice", text)
            self.history.end_message(session)
            item = SpeechItem(id=self._alloc_id(), session=session, kind="choice",
                              text=text, is_decision=True)
            self._pending_heard[item.id] = entry
            # AskUserQuestion ALSO fires a permission-prompt notification ~5-6s
            # later; mark the question unanswered so that redundant permission
            # (earcon + text) is suppressed until the turn moves on (issue #11 f/u).
            # Set this NOW (not at settle fire) so the suppression is armed before
            # the permission can arrive.
            self._await_choice.add(session)
            # Summary mode: defer the lead-in digest + question through the settle
            # window so late lead-in prose is included, heard before the question.
            # Non-summary speaks prose live, so enqueue the question immediately.
            self._summary.on_decision(session, item)
            return None

        if t == MsgType.PLAN:
            text = decision_text.plan_text(msg)
            cue = self._selection_cue(session, verbosity)
            if cue:
                text = "{0} {1}".format(text, cue)
            entry = self.history.record(session, "plan", text)
            self.history.end_message(session)
            item = SpeechItem(id=self._alloc_id(), session=session, kind="plan",
                              text=text, is_decision=True)
            self._pending_heard[item.id] = entry
            # Same hook race as CHOICE (#16): the PLAN can beat its lead-in prose,
            # so defer the lead-in gather + hold through the settle window in
            # summary mode; the context is then heard before the plan (audit #21).
            self._summary.on_decision(session, item)
            return None

        if t == MsgType.PERMISSION:
            # Redundant permission that pairs with an unanswered AskUserQuestion:
            # the question was already announced, so drop this one. CONSUME the
            # guard here (the permission it exists to suppress has now arrived) --
            # do NOT rely on unrelated prose/turn_done to clear it, since the
            # pre-question prose streams in AFTER the choice and would clear it
            # early (confirmed via message-sequence capture, issue #11 f/u).
            if session in self._await_choice or (not session and self._await_choice):
                self._await_choice.discard(session)
                return None
            text = decision_text.permission_text(msg)
            cue = self._selection_cue(session, verbosity)
            if cue:
                text = "{0} {1}".format(text, cue)
            entry = self.history.record(session, "permission", text)
            self.history.end_message(session)
            item = SpeechItem(id=self._alloc_id(), session=session, kind="permission",
                              text=text, is_decision=True)
            self._pending_heard[item.id] = entry
            # Same hook race as CHOICE (#16): defer through the settle window in
            # summary mode so a late lead-in is digested and heard first (audit #21).
            self._summary.on_decision(session, item)
            return None

        if t == MsgType.TOOL:
            self._await_choice.discard(session)  # a tool ran -> the question was answered
            if verbosity == "everything":
                tool = msg.get("tool", "")
                summary = (msg.get("summary") or "").strip()
                text = summary if summary else "Running {0}.".format(tool)
                ch = self.router.channel(session)
                # A tool announcement is immediate: flush any held prose
                # (below the minqueue threshold) so it reads before the cue.
                ch.turn_done = True
                ch.append(SpeechItem(
                    id=self._alloc_id(), session=session, kind="tool_announce",
                    text=text, is_decision=False))
                self._wake.set()
            return None

        if t == MsgType.EARCON:
            # Instant: the Windows earcon backend plays on a separate audio path
            # that mixes with the speech, so it no longer cuts the reading.
            kind = msg.get("kind", "")
            # Suppress the redundant permission chime that pairs with an unanswered
            # AskUserQuestion (its message carries no session, so gate on "any
            # question awaiting"). Real permission chimes still fire (issue #11 f/u).
            if kind == "permission" and self._await_choice:
                return None
            self._earcon(kind)
            if kind == "turn_done":
                # End-of-turn boundary: safety-net flush in case the final PROSE
                # flag never arrived. Wake the loop so a sub-threshold batch that was
                # left buffered (no per-delta wake) is read now, not after a poll.
                self.router.channel(session).turn_done = True
                self._wake.set()
                # Do NOT digest yet: the turn's final prose can arrive after this
                # signal, so summary mode arms a settle window first (#14).
                self._summary.on_turn_done(session)
            return None

        if t == MsgType.FLUSH:
            cur = self._current_item
            if cur is not None and cur.session == session:
                self.speaker.cancel()
            self._drop_channel_pending(session)
            ch = self.router.channel(session)
            ch.wipe()
            seed = self.digest_store.get(session)
            if seed:
                # Keep the session cycle-reachable while its new turn cooks
                # (#118): a bare-wiped channel fell out of the manual ring
                # (empty channels are skipped, #117) for the WHOLE turn. The
                # persisted last digest re-seeds it as an already-heard,
                # replay-only item; real content replaces it.
                ch.seed(SpeechItem(id=self._alloc_id(), session=session,
                                   kind="summary", text=seed,
                                   is_decision=False))
            self._assemblers.pop(session, None)
            self.history.reset(session)
            # A new prompt is the user cancelling this session: advance the cancel
            # epoch so any digest dispatched before now is dropped when it lands,
            # rather than spoken into the new turn (#13); abandon any settling
            # turn (#14) and drop held or deferred questions (#16).
            self._summary.cancel(session)
            self._last_digest_text.pop(session, None)   # no re-reading a stale digest
            self._await_choice.discard(session)          # new prompt: no question pending
            # A new prompt auto-resumes a paused voice only when it comes from
            # the session the user hears (upstream #69): a background
            # session's /loop tick or agent completion also sends FLUSH, and
            # used to un-pause the voice the user deliberately held.
            if self._engaged_session() == session:
                self._paused.clear()
            self._wake.set()
            return None

        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            old_fg = self.sessions.foreground()
            # Deliberately NOT gated on a paused voice (upstream #69 / #65):
            # under the default earcon_only policy a non-foreground session's
            # live prose is never voiced, so refusing a genuine prompt here
            # would silently drop its turn. A held voice stays held through
            # the FLUSH gate, and the cooperative drain keeps the paused
            # session's interrupted message first on resume.
            self.sessions.set_foreground(session, cwd=msg.get("cwd"))
            if t == MsgType.SESSION_START:
                self.sessions.register(session, cwd=msg.get("cwd"))
                self._maybe_guide_setup(session, msg.get("plugin_version", ""))
            # Cooperative hand-off: if the old foreground still has pending items,
            # authorize it to drain before the new fg takes the floor. This is the
            # "session B arrives while A is mid-response" case. Uses _replay_authorized
            # so the policy gate is bypassed for the drain (A finished reading is
            # a natural completion, not a user-visible session switch).
            if old_fg is not None and old_fg != session:
                old_ch = self.router.channels.get(old_fg)
                if old_ch is not None and old_ch.pending() > 0:
                    self.router.authorize_replay(old_fg)
            return None

        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._teardown_session(session)
            self.router.drop(session)
            return None

        if t == MsgType.STOP:
            # Silence everything (M3/F8): flush-to-end first cancels settle
            # windows, in-flight and parked digests and held questions, so
            # nothing cooking speaks after the stop; then clear the queues.
            self._flush_all()
            for s in list(self.router.channels):
                self._drop_channel_pending(s)
            for ch in self.router.channels.values():
                ch.wipe()
            self.speaker.cancel()
            return None

        if t == MsgType.SKIP:
            cur = self._current_item
            if cur is not None:
                entry = self._pending_heard.get(cur.id)
                if entry is not None:
                    entry.heard = True
            self.speaker.cancel()
            return None

        if t == MsgType.NAV:
            # One message, always the last: the only nav target is 'first' (Up),
            # which restarts the latest turn from the top. Any other target (the
            # removed prev/next/last stepping) is a SILENT no-op, so a stale
            # client cannot pile items onto a channel.
            if msg.get("to", "first") != "first":
                return None
            fg = self._engaged_session()
            if self.config.get("summary_mode"):
                # Summary mode speaks ONE digest per turn: Up re-reads it. Flush
                # ('go to end', Ctrl+Alt+Down) is a separate handler.
                moved = self._reread_last(fg) if fg is not None else False
                self._earcon("nav" if moved else "nav_edge")
                return None
            # The "nav" earcon when the turn restarts, "nav_edge" when there is
            # nothing to restart.
            if fg is None:
                self._earcon("nav_edge")
                return None
            self._earcon("nav" if self._restart_turn(fg) else "nav_edge")
            return None

        if t == MsgType.PAUSE:
            # Temporary play/pause. Pause stops the current utterance and holds the
            # loop; resume re-speaks the interrupted item so it picks back up. Also
            # auto-cleared by a new prompt (see the FLUSH handler).
            target = self.router.active or self.sessions.foreground()
            if self._paused.is_set():
                # Resuming: clear flag, wake loop, then insert "Resumed." cue at
                # the active channel's cursor so it plays ahead of the interrupted
                # utterance (which was re-queued there on pause). mute_exempt so
                # it is always heard even if the session is also muted.
                self._paused.clear()
                self._wake.set()
                # target may be None (no session) -> the cue routes to the
                # CONTROL channel so the confirmation is still heard.
                self._cues.speak(target, "Resumed.", exempt_mute=True)
            else:
                self._paused.set()
                # cancel() bumps the speaker's epoch so even an in-progress
                # utterance aborts. The speak loop re-queues the interrupted item
                # (sees completed=False while paused), so we don't capture it here.
                self.speaker.cancel()
                self._audio.restore()
                # "Paused." is pause_exempt so the paused branch of the speak loop
                # scans for and voices it while holding everything else. target may
                # be None -> CONTROL channel (still scanned by take_pause_exempt).
                self._cues.speak(target, "Paused.", pause_exempt=True)
            return None

        if t == MsgType.MUTE:
            # Global mute CYCLE: Unmuted -> Muted -> Super Muted -> Unmuted.
            #   1 Muted:       prose silenced, beeps (earcons) still fire.
            #   2 Super Muted: prose AND beeps silenced (full mute).
            # The spoken state confirmation is mute_exempt (always heard via TTS, not
            # an earcon) so the user can tell the state and toggle out.
            self._mute_level = (self._mute_level + 1) % 3
            # Persist (#65): a respawned daemon restores the level, so mute
            # survives the silent hook-lazy-start replacement.
            self.config["mute_level"] = self._mute_level
            save_config(self.config)
            # Observability (#63): mute transitions and drops are logged so a
            # "mute did not stick" report is diagnosable from speechd.log
            # (state resets from a daemon respawn become visible too).
            print("[mute] level -> {0}".format(self._mute_level),
                  file=sys.stderr, flush=True)
            if self._mute_level >= 1:
                self.speaker.cancel()           # stop the current utterance now
            cue = {1: "Muted.", 2: "Super muted.", 0: "Unmuted."}[self._mute_level]
            target = self.router.active or self.sessions.foreground()
            # target may be None -> the cue routes to the CONTROL channel so the
            # confirmation is heard even when no session is registered.
            # pause_exempt: a state change made WHILE PAUSED must still be
            # confirmed, or the user cannot tell what they toggled (deep audit #25).
            self._cues.speak(target, cue, exempt_mute=True, pause_exempt=True)
            self._wake.set()
            return None

        if t == MsgType.NEXT_SESSION:
            # Manual session-change: switch the active reader to another session and
            # confirm immediately (cancel the current item, like pause/mute). The
            # router arms the "Session changed" announcement; on no other session we
            # speak a soft cue.
            target, _replay = self.router.next_session()
            self.speaker.cancel()
            if target is None:
                self._cues.speak(None, "No session.", exempt_mute=True,
                                 pause_exempt=True)
            else:
                # Instant press feedback (#111): fire the switch chime NOW, from
                # the handler (the earcon player is a non-blocking subprocess).
                # The deferred #94 alert only sounded at the target content's
                # synthesis-ready callback, leaving a manual press with ZERO
                # audio for the whole first-chunk synthesis - it felt dead.
                try:
                    self._earcon("session_change")
                except Exception:  # noqa: BLE001 - feedback must not break the switch
                    pass
            self._wake.set()
            return None

        if t == MsgType.RELOAD_KEYMAP:
            # keymap.json changed: re-register off the daemon lock.
            self._hotkeys.request_reload()
            return None

        if t == MsgType.REPEAT:
            fg = self._engaged_session()
            if fg is None:
                return None
            entries = self.history.last_message(fg)
            if not entries:
                self._cues.speak(fg, "Nothing to repeat.")
                return None
            self._replay(fg, entries)
            return None

        if t == MsgType.FLUSH_SESSION:
            # Flush to end: silence EVERYTHING queued or in flight across ALL
            # sessions and go idle (#107). The old per-engaged-session flush
            # left other sessions' landed or reorder-parked digests holding
            # the floor: the key chimed success, a handoff started reading
            # seconds later anyway, and a re-press in the silent gap found an
            # "empty" queue (the flush soft-lock). Non-destructive: skipped
            # items keep their history entries, so REPEAT / Up can bring them
            # back.
            self._earcon("nav" if self._flush_all() else "nav_edge")
            return None

        if t == MsgType.CHOICE_ANSWERED:
            # The user ANSWERED the blocking question (#83): they have heard (or
            # read) everything they need up to it. Silence the stale backlog and
            # any in-flight lead-in digest; whatever the assistant says AFTER the
            # answer flows normally. No earcon: answering is its own feedback.
            self._user_caught_up(session)
            return None

        if t == MsgType.SET_RATE:
            is_delta = "delta" in msg
            if is_delta:
                try:
                    target = (int(config_schema.get(self.config, "rate"))
                              + int(msg.get("delta", 0)))
                except (ValueError, TypeError):
                    return None
            else:
                target = msg.get("rate")
            # Validate/clamp the rate in both branches -- an unvalidated value
            # here is persisted to disk and breaks synthesis.
            rate = config_schema.clean("rate", target)
            if rate is config_schema.INVALID:
                return None
            self.config["rate"] = rate
            self.speaker.set_rate(rate)
            save_config(self.config)
            if is_delta:
                # A control cue (F6): on the session channel it waited behind
                # minqueue and could wipe the placeholder seed.
                self._cues.speak(self.sessions.foreground(),
                                 "Rate {0}.".format(rate), exempt_mute=True,
                                 pause_exempt=True, cue_key="rate")
                self._wake.set()
            return None

        if t == MsgType.SET_VOICE:
            voice = msg.get("voice")
            self.config["voice"] = voice
            self.speaker.set_voice(voice)
            save_config(self.config)
            return None

        if t == MsgType.SET_SESSION_PREF:
            sid = msg.get("session")
            key = msg.get("key")
            if not isinstance(sid, str) or not self.session_prefs.set(sid, key, msg.get("value")):
                return None
            if key == "muted":
                val = bool(msg.get("value"))
                ch = self.router.channels.get(sid)
                if ch is not None:
                    ch.muted = val
                cur = self._current_item
                if val and cur is not None and getattr(cur, "session", None) == sid:
                    self.speaker.cancel()
                self._wake.set()
            return None

        if t == MsgType.FORGET_SESSION:
            sid = msg.get("session")
            if not isinstance(sid, str) or self.sessions.is_foreground(sid):
                return None
            self.sessions.unregister(sid)
            self.session_prefs.forget(sid)
            # Forget targets exactly the stale sessions that died WITHOUT
            # SessionEnd, so it needs the same per-session teardown (#101).
            self._teardown_session(sid)
            self.router.drop(sid)
            return None

        if t == MsgType.SET_VERBOSITY:
            self.config["verbosity"] = msg.get("verbosity")
            save_config(self.config)
            return None

        if t == MsgType.SET_MINQUEUE:
            # Validate/clamp before persisting -- a bad value reaches disk and would
            # wedge prose buffering on every turn (mirrors the SET_RATE guard).
            n = config_schema.clean("minqueue", msg.get("minqueue"))
            if n is config_schema.INVALID:
                return None
            self.config["minqueue"] = n
            save_config(self.config)
            return None

        if t in AudioControl.MESSAGES:
            return self._audio.handle(msg)

        if t == MsgType.SET_SUMMARY_MODE:
            if "enabled" not in msg:
                return None
            enabled = config_schema.clean("summary_mode", msg.get("enabled"))
            self.config["summary_mode"] = enabled
            save_config(self.config)
            target = self.router.active or self.sessions.foreground()
            self._cues.speak(target,
                             "Summary mode on." if enabled else "Summary mode off.",
                             exempt_mute=True, pause_exempt=True)
            self._wake.set()
            return None

        if t == MsgType.STATUS:
            return {
                "verbosity": self.config.get("verbosity"),
                "rate": self.config.get("rate"),
                "voice": self.config.get("voice"),
                "foreground": self.sessions.foreground(),
                "minqueue": self.config.get("minqueue"),
                "summary_mode": bool(self.config.get("summary_mode")),
            }

        if t == MsgType.SHUTDOWN:
            if msg.get("stay_down"):
                # Page 'Shut down' (#34): gate both respawn paths, exactly like
                # `sonara shutdown` (the CLI writes the sentinel client-side).
                try:
                    from sonara import paths
                    paths.STOPPED_SENTINEL_PATH.write_text("via settings page")
                except OSError:
                    pass
            # Reply FIRST (the socket write happens after this handler returns),
            # then tear down via a short timer: run() unlinks the lockfile and
            # the OS releases the singleton mutex at process death (#23).
            timer = threading.Timer(0.2, self.stop)
            timer.daemon = True
            timer.start()
            return {"ok": True}

        if t == MsgType.PING:
            return {"ok": True}

        return None

    def stop(self) -> None:
        self._running.clear()
        self._wake.set()
        self._hotkeys.stop_worker()     # unblock the hotkey worker's get() to exit
        self._audio.restore()       # never leave other apps' audio ducked or paused
        self._hotkeys.stop()
        if getattr(self, "_webui", None) is not None:
            self._webui.stop()
        self._server.close()

    def _replay(self, session: str, entries, append: bool = False,
                suppress_announce: bool = True) -> None:
        """Insert history entries as replay items at the active channel cursor.

        Items are inserted in order at the current cursor position so they read
        next via the router, ahead of any already-queued items. Each entry's
        heard-marker is registered in _pending_heard so note_spoken can flip it
        True on completion (same as a normal _enqueue with entry=...).

        The router's _pick() uses oldest-waiting fallthrough so a non-fg session
        with replayed items will be reached once the fg channel drains.

        Pre-set _last_active to the replay target so the router does not emit a
        "Session changed" announcement for programmatic replays (Up / repeat):
        the user asked for this session's content, so the auto-announce would be
        a spurious interruption."""
        ch = self.router.channel(session)
        # append=True adds at the END (after any queued decision), like the long
        # digest path -- so a new short turn never overtakes a queued question
        # (#17). append=False keeps cursor-insert for explicit user replay
        # (Up / repeat), which should read next.
        at = len(ch.items) if append else ch.cursor
        items = []
        for e in entries:
            item = SpeechItem(
                id=self._alloc_id(),
                session=session,
                kind=e.kind,
                text=e.text,
                is_decision=e.kind in ("choice", "plan", "permission"),
            )
            self._pending_heard[item.id] = e
            items.append(item)
        # insert_at replaces a placeholder seed, bumps gen and sets
        # has_decision for a replayed decision (#137, L-replay-decision).
        ch.insert_at(at, items)
        if items:  # only if we actually inserted items
            # Mark channel ready: replayed items should be spoken without
            # waiting for minqueue threshold.
            ch.turn_done = True
            # Suppress the "Session changed" auto-announce for programmatic
            # replay (Up/repeat): the handoff is not user-visible.
            # NOT for automatic turn delivery (the short-turn digest path):
            # there the handoff IS user-visible, and suppressing it played
            # content unattributed after another session read (audit #21).
            if suppress_announce:
                self.router.set_last_active(session)
            # Authorize cross-session reading: replay targets that are not the
            # current fg bypass the background-policy gate so their replayed
            # items are voiced (Up / repeat on a non-foreground reader).
            fg = self.sessions.foreground()
            if session != fg:
                self.router.authorize_replay(session)
        self._wake.set()

    def _user_caught_up(self, session: str) -> bool:
        """The user declared everything queued for *session* stale - they
        answered the question, or pressed flush-to-end (#83). Skip the channel
        backlog non-destructively (history entries stay for repeat / Up),
        cut the in-progress utterance if it is this session's, drop a
        settle-deferred or digest-held question, and advance the digest cancel
        epoch so an in-flight lead-in digest lands dead instead of speaking
        into the post-answer flow. The turn CONTINUES (unlike FLUSH/new
        prompt): history and assemblers stay, but the voiced marker advances past
        everything already said - the eventual turn-end digest covers only
        post-answer prose ("I want to hear what comes after", #83).
        Caller holds the lock. Returns True when anything was skipped/cut."""
        ch = self.router.channel(session)
        skipped = ch.skip_to_end()         # any pending decision is skipped too
        for it in skipped:
            self._pending_heard.pop(it.id, None)
        cur = self._current_item
        cutting = cur is not None and cur.session == session
        if cutting:
            self.speaker.cancel()          # cut the in-progress utterance
            # Clear now so a rapid SECOND press (before the speak loop's
            # note_spoken runs) sees nothing left to cut and gives the edge
            # chime -- "go to end" is top/bottom, it should only move once
            # (issue #11). note_spoken also nulls this later; idempotent.
            self._current_item = None
        # Deferred and held questions, the settle window, in-flight digests
        # and the voiced marker (summary/pipeline).
        dropped = self._summary.caught_up(session)
        self._await_choice.discard(session)
        self._wake.set()
        return bool(skipped) or cutting or dropped

    def _flush_all(self) -> bool:
        """Flush-to-end (#107): silence everything queued or in flight across
        ALL sessions. Per-session state goes through _user_caught_up
        (non-destructive skip, current-utterance cut, in-flight digest kill,
        settle cancel); completed digests PARKED in the reorder buffer are
        landed dead in place (they already left the in-flight count, so the
        per-session kill cannot see them); a stale deferred handoff alert and
        an armed-but-unemitted switch announcement are dropped. Caller holds
        the lock. Returns True when anything was skipped, cut, or killed."""
        from sonara.router import CONTROL
        sessions = set(self.router.channels) | self._summary.busy_sessions()
        cur = self._current_item
        if cur is not None:
            sessions.add(cur.session)      # cut audio even for an untracked session
        sessions.discard(CONTROL)          # control cues are sub-second; let them be
        dropped = False
        for sid in sessions:
            dropped = self._user_caught_up(sid) or dropped
        dropped = self._digests.kill_parked() or dropped
        self._playback.pending_preamble = None
        self.router.clear_pending_announce()
        return dropped

    def _engaged_session(self):
        """The session the user is currently engaged with: the one being read
        (router.active), else the one that most recently read (persists across idle
        gaps), else the foreground. After a session-change the active reader differs
        from the foreground, so Up/repeat must operate on what the user
        HEARS, not the last session to submit a prompt."""
        return (self.router.active or self.router.last_active
                or self.sessions.foreground())

    def _log_start_marker(self) -> None:
        """Startup marker (#63): volatile state (mute level, pause) dies with the
        process, so an unexplained "setting reset itself" is diagnosable only if
        restarts are visible in the log.

        Records the package root too (#123). Sonara deliberately lives in two
        places -- the checkout and the deployed ~/.sonara/app -- and which one a
        daemon imported used to depend on who started it, with nothing in the
        log to tell them apart. Two consecutive daemons on the same box ran
        different copies and looked identical here.
        """
        print("[daemon] started pid={0} root={1}".format(
            os.getpid(), package_root()), file=sys.stderr, flush=True)

    def _restart_turn(self, session: str) -> bool:
        """Up: restart the current turn from its first message and read it to the
        end. Returns True when there was a turn to restart (the NAV handler
        chimes "nav"), False when nothing is recorded yet ("nav_edge").

        The turn is the history since the last prompt (history resets on FLUSH).
        A restart always plays, even when pressed repeatedly (#128): it cuts
        current speech, drops the channel's not-yet-spoken items and replays
        every message of the turn at the channel cursor. Newly streamed prose
        enqueues after these and continues seamlessly."""
        ids = self.history.message_ids(session)
        if not ids:
            # A control cue (F6): on the session channel it wiped the seed.
            self._cues.speak(session, "Nothing to navigate yet.")
            return False
        self.speaker.cancel()
        # Clear any not-yet-spoken items from the channel so the replay is the
        # sole pending work.
        for it in self.router.channel(session).truncate_pending():
            self._pending_heard.pop(it.id, None)
        entries = []
        for mid in ids:
            entries.extend(self.history.entries_for_message(session, mid))
        self._replay(session, entries)
        return True

    def _reread_last(self, session: str) -> bool:
        """Re-read the last digest immediately (summary-mode Up, issue #11). Speaks
        the EXACT text that was spoken (stored verbatim, prefix and all) so the
        rendered audio is a cache hit and replays byte-identically instead of
        regenerating (~2s + drifting intonation each time). Cuts the current read
        and restarts from the top so the press takes effect AT ONCE. Returns True if
        there was a digest to re-read (caller chimes "nav"), else False (edge)."""
        text = self._last_digest_text.get(session)
        # A DECISION being spoken RIGHT NOW is not in the record yet (it joins
        # on completion) -- cancelling it without re-queueing ANNIHILATED the
        # question: edge chime, gone forever (live report 2026-07-14). Up during
        # a speaking question restarts it instead. A non-decision current item
        # (a digest) is already the record's text, so re-queueing it would
        # double-speak; the record re-read IS its restart.
        cur = self._current_item
        if cur is not None and (cur.session != session or not cur.is_decision):
            cur = None
        if not text and cur is None:
            return False
        self.speaker.cancel()                    # restart now, don't wait out the read
        ch = self.router.channel(session)
        if ch.seeded:
            ch.wipe()    # the re-read replaces the placeholder seed (#118)
        # Drop pending prose so the re-read is next -- but PRESERVE queued
        # decision items (a blocking question deleted here was gone forever,
        # nothing replayed it; audit #21). They re-queue after the digest,
        # keeping the context-first order.
        preserved = []
        for it in ch.truncate_pending():
            if it.is_decision:
                preserved.append(it)             # keeps its _pending_heard marker
            else:
                self._pending_heard.pop(it.id, None)
        if cur is not None:
            # un-consume the interrupted question so history keeps ONE copy
            ch.unconsume(cur)
        tail = []
        if text:
            tail.append(SpeechItem(
                id=self._alloc_id(), session=session, kind="summary",
                text=text, is_decision=False))
        if cur is not None:
            tail.append(cur)                     # the interrupted question, from the top
        tail.extend(preserved)                   # then any still-queued question(s)
        ch.insert_at(ch.cursor, tail)            # sets has_decision for a question
        ch.turn_done = True                      # ready() -> plays now (minqueue-exempt)
        self._wake.set()
        return True

    def _maybe_prewarm_cue_voice(self) -> None:
        """Live-apply hook for the cue voice keys (config_schema apply):
        warm the Kokoro engine for cues (#60, see Cues.maybe_prewarm)."""
        self._cues.maybe_prewarm()

    def set_config_value(self, key: str, value) -> bool:
        """Set a config-only tuning key (settings page, #34). These have no
        protocol message; config_schema validates them (#136). Clean, set
        under the lock, persist, then run the key's live-apply hook (switching
        TO a Kokoro cue voice warms it, #60). Returns False for unknown
        keys/bad values."""
        if key not in config_schema.config_only_keys():
            return False
        cleaned = config_schema.clean(key, value)
        if cleaned is config_schema.INVALID:
            return False
        with self._lock:
            self.config[key] = cleaned
            save_config(self.config)
        hook = config_schema.SCHEMA[key].apply
        if hook:
            getattr(self, hook)()
        return True

    def set_summary_prompt(self, style, text) -> bool:
        """Store or reset a per-style custom summarizer instruction (#58).
        text=None (or text equal to the built-in default) resets to default;
        empty/whitespace text is rejected (an empty instruction would strip
        the never-addressed-to-you firewall from the call)."""
        if style not in ("tidy", "natural", "brief"):
            return False
        from sonara.summarizer import default_instruction
        if text is not None:
            text = str(text)
            if not text.strip():
                return False
            if text == default_instruction(style):
                text = None                     # storing the default = reset
        with self._lock:
            prompts = dict(self.config.get("summary_prompts") or {})
            if text is None:
                prompts.pop(style, None)
            else:
                prompts[style] = text
            self.config["summary_prompts"] = prompts
            save_config(self.config)
        return True

    def _start_preview_builder(self, delay_s: float = 15.0):
        """Render missing voice-preview files in the background (#38). Delayed
        so daemon startup (prewarm, first speech) is never contended; every
        failure is contained -- previews are a convenience, not a duty.
        Returns the thread (tests join it)."""
        def _run():
            try:
                import time
                time.sleep(delay_s)
                from sonara import previews
                from sonara.webui import _installed_voices
                made = previews.ensure_all(
                    _installed_voices(),
                    log=lambda m: print("[previews] " + m, flush=True))
                if made:
                    print("[previews] rendered {0} preview file(s)".format(made),
                          flush=True)
            except Exception:  # noqa: BLE001 - preview building must never bite
                pass
        t = threading.Thread(target=_run, name="sonara-previews", daemon=True)
        t.start()
        return t

    def preview_voice(self, voice: str) -> bool:
        """Speak a short sample in *voice* WITHOUT changing config (settings
        page, #34). It queues on the CONTROL channel like any cue (M6): it
        plays after the utterance in progress, never over it. Playing it on
        its own thread cut live speech (winsound has one channel) and the cut
        utterance was still marked heard. A newer preview replaces a pending
        or playing one; mute and pause do not swallow it, the user asked."""
        if not voice:
            return False
        text = "This is {0} speaking for Sonara.".format(voice)
        # HTTP requests run on their own threads: Cues.speak reslices CONTROL
        # and allocates an id, which the speak loop does under the lock too.
        with self._lock:
            self._cues.speak(None, text, exempt_mute=True, pause_exempt=True,
                             cue_key="voice_preview", voice=str(voice))
        return True

    def run(self) -> None:
        ensure_sonara_dir()
        try:
            self._rehydrate_channels()      # restarts keep the cycle populated (#118)
        except Exception:  # noqa: BLE001 - rehydration must never block startup
            pass
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind((transport.HOST, 0))
        srv.listen(16)
        port = srv.getsockname()[1]
        # Reuse the previous token across restarts (#34): the settings page's
        # Restart button and bookmarked page URLs keep working because the
        # respawned daemon accepts the same token. Same-user security boundary
        # is unchanged -- the token still lives 0600 in the user's own home.
        self._server.token = tokens.persistent_token()
        from sonara.webui import SettingsServer
        self._webui = SettingsServer(self, self._server.token,
                                     int(config_schema.get(self.config, "settings_port")))
        try:
            http_port = self._webui.start()
        except Exception:  # noqa: BLE001 - the page must never block speech
            self._webui, http_port = None, None
        self._start_preview_builder()   # render missing voice previews (#38)
        transport.write_lockfile(
            LOCK_PATH, transport.HOST, port, self._server.token, os.getpid(),
            http_port=http_port)
        self._server.sock = srv
        self._running.set()

        speak_thread = threading.Thread(target=self._playback.run, daemon=True)
        accept_thread = threading.Thread(target=self._server.accept_loop, daemon=True)
        hotkey_worker = threading.Thread(target=self._hotkeys.worker,
                                         name="sonara-hotkey-worker", daemon=True)
        self._log_start_marker()
        speak_thread.start()
        accept_thread.start()
        hotkey_worker.start()
        self._hotkeys.start()
        self._maybe_prewarm_cue_voice()    # load the Kokoro engine for cues (#60)

        try:
            while self._running.is_set():
                accept_thread.join(timeout=0.25)
                if not accept_thread.is_alive():
                    break
        except KeyboardInterrupt:
            pass
        finally:
            self.stop()
            try:
                srv.close()
            except OSError:
                pass
            try:
                os.unlink(LOCK_PATH)
            except FileNotFoundError:
                pass


def resolve_earcons(bundled: dict, overrides) -> dict:
    """The earcon map the speaker plays: the bundled set, resolved from the
    running package on every start, with the user's own wavs (config
    "earcons") on top. Never stored back into config, so new bundled kinds
    reach every install (#136, audit M8)."""
    out = dict(bundled)
    if isinstance(overrides, dict):
        out.update({k: v for k, v in overrides.items()
                    if isinstance(v, str) and v})
    return out


def main() -> None:
    from sonara.platform.windows import process as _process
    _process.arm_faulthandler()
    # Single-instance guard. The fast path avoids work when a daemon is clearly
    # already serving. The AUTHORITATIVE guard is the exclusive flock below:
    # with an ephemeral TCP port, bind() never collides (unlike the old fixed
    # AF_UNIX path), so socket_connectable() alone is racy and lets concurrent
    # lazy-starts each bind their own port -> a daemon explosion. The flock lets
    # exactly one process win; the rest exit. The lock auto-releases on death.
    global _SINGLETON, _MUTEX
    if socket_connectable():
        return
    ensure_sonara_dir()
    # AUTHORITATIVE single-instance guard: a named kernel mutex. The byte-lock
    # below is tied to the lock FILE's inode, so a deleted/recreated file or two
    # daemons racing to create it stop excluding -> a daemon explosion (observed
    # live). The mutex is keyed by name, immune to that, and frees on death.
    try:
        _MUTEX = transport.acquire_singleton_mutex()
    except OSError as exc:
        # M11: a mutex that cannot be created is not "another daemon owns
        # it". Log it and let the lock-file byte-lock below decide.
        print("[singleton] {0}; using the lock file instead".format(exc),
              file=sys.stderr, flush=True)
        _MUTEX = False
    if _MUTEX is None:
        print("[singleton] another Sonara daemon is already running for this "
              "user; exiting", file=sys.stderr, flush=True)
        return
    _SINGLETON = transport.acquire_singleton(SINGLETON_PATH)  # pid record (best-effort)
    if _MUTEX is False and _SINGLETON is None:
        print("[singleton] the lock file is held by another daemon; exiting",
              file=sys.stderr, flush=True)
        return

    _process.harden_process()   # win32: opt out of EcoQoS throttling + raise
                                # priority so global hotkeys stay responsive
                                # after long idle
    _process.preload_vc_runtime()   # win32: system VC runtime first, before any engine (#29)

    from sonara.speaker import Speaker
    from sonara.sessions import SessionManager
    from sonara.platform import get_platform

    _backend = get_platform()
    from sonara.platform.windows.ducking import restore_from_state_file
    restore_from_state_file()   # un-duck anything a crashed prior daemon left down
    from sonara.platform.windows.pausing import resume_from_state_file as _resume_paused
    _resume_paused()   # resume anything a crashed prior daemon left paused
    cfg = load_config()
    speaker = Speaker(
        voice=cfg.get("voice"),
        rate=config_schema.get(cfg, "rate"),
        say_runner=_backend.tts.run,
        earcon_player=_backend.earcon.play,
        earcons=resolve_earcons(_backend.earcon.default_earcons(),
                                cfg.get("earcons")),
    )
    sessions = SessionManager(background_policy=config_schema.get(cfg, "background_policy"),
                              store_path=SESSIONS_PATH, seen_path=SESSION_SEEN_PATH)
    from sonara.session_prefs import SessionPrefs
    from sonara.digest_store import DigestStore
    daemon = SpeechDaemon(speaker, sessions, cfg,
                          ducker=_backend.ducker, pauser=_backend.pauser,
                          prefs=SessionPrefs(store_path=SESSION_PREFS_PATH),
                          digests=DigestStore(store_path=SESSION_DIGESTS_PATH))
    daemon._audio.apply_volume(config_schema.get(cfg, "volume"))   # restore persisted speech gain
    daemon.run()
