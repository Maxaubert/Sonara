from __future__ import annotations

import os
import queue
import secrets
import socket
import subprocess
import sys
import threading

from sonara.protocol import MsgType, encode, decode
from sonara.queue import SpeechItem
from sonara.assembler import ProseAssembler
from sonara.config import save_config, load_config
from sonara.paths import (
    LOCK_PATH, SINGLETON_PATH, ensure_sonara_dir, socket_connectable,
    INSTALL_RECORD_PATH, SESSIONS_PATH, SESSION_PREFS_PATH, SESSION_SEEN_PATH,
)
from sonara.platform import transport

# Holds the single-instance flock for this process's lifetime (see main()).
_SINGLETON = None
_MUTEX = None       # process-lifetime handle to the named single-instance mutex


def _wellformed_token(tok) -> bool:
    return (isinstance(tok, str) and len(tok) == 64
            and all(c in "0123456789abcdef" for c in tok))


def _select_token(prior_lock: dict) -> str:
    """Reuse a well-formed prior lockfile token (settings-page restart
    reconnect + durable bookmarks, #34); otherwise mint a fresh one."""
    tok = (prior_lock or {}).get("token")
    if _wellformed_token(tok):
        return tok
    return secrets.token_hex(32)


def _persistent_token() -> str:
    """The daemon token, durable across CLEAN restarts (#34 follow-up): the
    lockfile is unlinked on exit, so lockfile-based reuse only covered crashes
    -- live-verified when the page's Restart button reconnected to a 403 wall.
    Priority: token file, then a stale lockfile (crash case), else mint. The
    chosen token is (re)written to the file so the NEXT start reuses it."""
    from sonara import paths as _paths
    tok = None
    try:
        tok = _paths.WEBUI_TOKEN_PATH.read_text(encoding="utf-8").strip()
    except OSError:
        pass
    if not _wellformed_token(tok):
        tok = _select_token(transport.read_lockfile(LOCK_PATH) or {})
    try:
        _paths.ensure_sonara_dir()
        _paths.WEBUI_TOKEN_PATH.write_text(tok, encoding="utf-8")
        os.chmod(_paths.WEBUI_TOKEN_PATH, 0o600)
    except OSError:
        pass                     # unwritable dir: token still valid this run
    return tok


RATE_MIN = 100
RATE_MAX = 400

# Min-queue batching: how many prose items must accumulate before they are read.
# 1 == read each item as it arrives (the default, unchanged behaviour).
MINQUEUE_MIN = 0     # 0 = start reading immediately, no batching (#60 follow-up)
MINQUEUE_MAX = 10

# Hotkey debounce: ignore a repeat of the SAME toggle within this window so an
# accidental/rapid double-tap doesn't flip pause/mute/session several times (and pile
# up confirmation cues). Directional keys (nav/repeat/skip) are NOT debounced --
# repeated presses there are intentional.
_HOTKEY_DEBOUNCE_S = 0.30
_DEBOUNCED_HOTKEYS = (
    MsgType.PAUSE, MsgType.MUTE, MsgType.NEXT_SESSION, MsgType.CYCLE_VERBOSITY,
)

# Summary mode: a turn whose prose is already shorter than this is spoken
# as-is instead of being digested (a digest of a short message adds nothing,
# costs a model call, and risks spoken meta-text on borderline input).
# EXCEPTION (#83): a lead-in before a pending QUESTION is digested even when
# short - short mid-turn lead-ins are precisely the "let me check the repo"
# process narration the digest exists to cut.
_SUMMARY_MIN_CHARS = 280

# Max seconds a blocking question is HELD behind its in-flight lead-in digest
# (#83). This is the WEDGE guard, not the normal path: the digest worker's
# finally releases the question the instant the digest lands or fails, and the
# question's attention earcon fires immediately regardless. Live logs measured
# digests at median 8.7s / p90 17.7s, so the original 5s cap fired on nearly
# every question and made the "bounded inversion" (question before context)
# the common case (#103). The cap must comfortably exceed real digest latency;
# past it the question speaks and the digest follows (hung summarizer only).
_DECISION_HOLD_MAX_S = 30.0

# Cap on concurrent connection-handler threads. Legitimate clients are short-lived
# (one request each), so this bound is generous; it just stops a misbehaving or
# hostile peer from leaking unbounded threads by opening many connections.
_MAX_CONN_THREADS = 32


class SpeechDaemon:
    def __init__(self, speaker, sessions, config, ducker=None, pauser=None,
                 prefs=None) -> None:
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
        self._server = None
        self._token = None
        self._webui = None
        self._poll_interval = 0.1
        from sonara.history import SessionHistory
        self.history = SessionHistory(cap=int(config.get("history_cap", 200)))
        self._options: "dict[str, str]" = {}
        self._pending_heard: dict = {}            # SpeechItem.id -> HistoryEntry
        self._nav_cursor: dict = {}               # session -> anchored message id (absent = latest)
        self._last_digest_text: dict = {}         # session -> exact spoken digest text (summary-mode Up re-reads it verbatim so cached audio replays)
        self._voiced_upto: dict = {}       # session -> last HistoryEntry voiced this turn (summary mode: a blocking question and turn-end must not double-voice; identity survives history-cap eviction, audit #21)
        self._await_choice: set = set()           # sessions with an unanswered AskUserQuestion (suppress the redundant permission prompt it also fires)
        self._held_decision: dict = {}            # session -> decision item held until its lead-in digest lands (context-first ordering)
        self._paused = threading.Event()          # play/pause: set == speech halted
        # Mute cycle: 0=unmuted, 1=muted (prose off, beeps on), 2=super muted
        # (prose AND beeps off). RESTORED from config (#65): hooks silently
        # respawn a dead daemon between two messages, and a memory-only mute
        # was reset to audible by the swap - the "mute is not persistent" bug.
        try:
            self._mute_level = max(0, min(2, int(config.get("mute_level", 0))))
        except (TypeError, ValueError):
            self._mute_level = 0
        self._hotkey_last: dict = {}              # toggle type -> last fire (debounce)
        # Digest reorder buffer (#88): turn-end digests become AUDIBLE in
        # dispatch (turn-finish) order, not summarizer-completion order.
        self._digest_seq_next = 0                 # next sequence number to hand out
        self._digest_seq_serve = 0                # next sequence number to release
        self._digest_parked: dict = {}            # seq -> apply closure (None = dropped)
        self._digest_release_counter = 0          # channel stamp source (#88)
        self._current_item = None                 # item being spoken right now
        self._pending_preamble = None             # (session, alert_text) deferred to content on_play (#94)
        self._warned_immediate: set = set()
        self._guided_sessions: set = set()
        self._conn_sem = threading.BoundedSemaphore(_MAX_CONN_THREADS)
        self._reload_lock = threading.Lock()      # serializes off-lock hotkey reloads
        # Hotkey fires are handed to this queue by the Windows pump thread and
        # applied by a dedicated worker under self._lock -- so the pump NEVER blocks
        # on the lock and presses can't pile up then burst while the daemon is busy
        # streaming prose (the mute-hang). Drained by _hotkey_worker.
        self._hotkey_q: "queue.Queue" = queue.Queue()
        self._preview_busy = False                  # preview_voice coalescing flag
        self._preview_runner = None                 # injected by tests; runtime uses platform tts.run
        # Summary mode: per-session CANCEL epoch. Only a user action (a new prompt
        # -> FLUSH) advances it; a finished digest is dropped iff the epoch moved
        # since it was dispatched. A turn merely ending does NOT advance it, so the
        # system never drops a finished message -- only the user cancels (#13).
        self._summary_gen: dict = {}
        # Summary mode: per-session turn-end SETTLE window (#14). turn_done can
        # reach the daemon before a turn's final prose (separate hook processes
        # race under multi-session load), so digesting immediately summarizes an
        # incomplete/empty turn. Arm a short window on turn_done, reset it on each
        # new prose delta, and digest only once the session is quiet.
        self._settle_timers: dict = {}     # session -> threading.Timer
        self._settle_gen: dict = {}        # session -> int (stale-fire guard)
        self._settle_pending: set = set()  # sessions with a window armed
        self._pending_decision: dict = {}  # session -> question item awaiting its lead-in (#16)
        # Digest dispatch bookkeeping (#21): each dispatch gets a token, and a
        # held decision is OWNED by the dispatch it waits behind -- only that
        # worker may pop and append it. Without ownership, whichever same-gen
        # worker landed first stole the question and played it before its own
        # lead-in context. _inflight_digests lets a decision with no NEW prose
        # of its own still hold behind an earlier digest that is mid-flight.
        self._summary_token = 0            # monotonically increasing dispatch id
        self._last_dispatch_token: dict = {}   # session -> newest dispatch token
        self._inflight_digests: dict = {}      # session -> workers in flight
        self._summarize_fn = None      # test seam; None -> sonara.summarizer.summarize

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
                 pause_exempt: bool = False, at_front: bool = False) -> None:
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
        ch = self.router.channel(session)
        if at_front:
            ch.items.insert(ch.cursor, item)
        else:
            ch.items.append(item)
        self._wake.set()

    def _minqueue(self) -> int:
        try:
            return max(MINQUEUE_MIN, min(MINQUEUE_MAX, int(self.config.get("minqueue", 1))))
        except (TypeError, ValueError):
            return 1

    def _maybe_guide_setup(self, session: str, plugin_version: str) -> None:
        """Speak ONE setup-guidance cue for this session, only when degraded.

        Throttle: at most once per session (recorded whether or not a cue fires).
        Silent when healthy. The check is a few file stats + a version compare
        (no subprocess) and never raises.
        """
        if session in self._guided_sessions:
            return
        try:
            state, cue = self._setup_health(plugin_version or "")
        except Exception:  # noqa: BLE001 - guidance must never break a session
            return
        self._guided_sessions.add(session)
        if state != "ok" and cue:
            self._enqueue(session, "prose", cue, False)

    def _drop_pending(self, items) -> None:
        for it in items:
            self._pending_heard.pop(it.id, None)

    def _drop_channel_pending(self, session: str) -> None:
        """Drop heard-tracking entries for a session's not-yet-spoken channel items
        (called before wiping/dropping the channel, so _pending_heard can't leak)."""
        ch = self.router.channels.get(session)
        if ch is not None:
            for it in ch.items:
                self._pending_heard.pop(it.id, None)

    def _teardown_session(self, session: str) -> None:
        """Per-session cleanup shared by SESSION_END and FORGET_SESSION (#101):
        both retire a session's live state; FORGET_SESSION targets exactly the
        sessions that died WITHOUT a SessionEnd, so its cleanup must match.
        Callers run this BEFORE router.drop(session) -- _drop_channel_pending
        needs the channel to still exist, or _pending_heard leaks."""
        self._drop_channel_pending(session)
        self.history.reset(session)
        self._options.pop(session, None)
        self._warned_immediate.discard(session)
        self._guided_sessions.discard(session)
        # Ending the session is a user action like FLUSH: BUMP the cancel
        # epoch so an in-flight digest is dropped when it lands. Popping it
        # reset never-FLUSHed sessions to a PASSING guard (get()==0 == the
        # dispatched gen 0), letting a dead session's digest resurrect its
        # history and channel (audit #21).
        self._summary_gen[session] = self._summary_gen.get(session, 0) + 1
        self._cancel_settle(session)
        self._settle_gen.pop(session, None)
        self._pending_decision.pop(session, None)
        # Clear the rest of the per-session state; a late _summary_worker
        # must find no held question to append (zombie channel), and nothing
        # here may outlive the session (audit #21).
        self._held_decision.pop(session, None)
        self._last_digest_text.pop(session, None)
        self._voiced_upto.pop(session, None)
        self._nav_cursor.pop(session, None)
        self._assemblers.pop(session, None)
        self._last_dispatch_token.pop(session, None)
        self._inflight_digests.pop(session, None)
        # A stale _await_choice entry from a dead session would suppress
        # permission chimes DAEMON-WIDE forever: the chime carries no session,
        # so the suppression check is global truthiness (audit #19).
        self._await_choice.discard(session)

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
                self.router._pending_announce = item.session
                self.router._pending_announce_replay = False
            else:
                ch = self.router.channels.get(item.session)
                if ch is not None and ch.cursor > 0:
                    ch.cursor -= 1
            self._current_item = None
            return True

    @staticmethod
    def _choice_text(msg) -> str:
        parts = []
        for q in msg.get("questions", []) or []:
            qtext = q.get("question", "") if isinstance(q, dict) else str(q)
            multi = bool(isinstance(q, dict) and q.get("multiSelect"))
            opts = q.get("options", []) if isinstance(q, dict) else []
            segs = []
            for i, o in enumerate(opts, 1):
                if isinstance(o, dict):
                    label = o.get("label", "")
                    desc = (o.get("description") or "").strip()
                else:
                    label, desc = str(o), ""
                if not label:
                    continue   # keep numbering aligned with the TUI's digits
                seg = "Option {0}: {1}.".format(i, label)
                if desc:
                    seg += " {0}{1}".format(
                        desc, "" if desc.endswith((".", "!", "?")) else ".")
                segs.append(seg)
            head = qtext
            if multi:
                head = "{0}{1}".format(
                    (qtext + " ") if qtext else "",
                    "This is a multi-select; you can pick more than one.")
            if head and segs:
                parts.append("{0} {1}".format(head, " ".join(segs)))
            elif segs:
                parts.append(" ".join(segs))
            elif head:
                parts.append(head)
        return " ".join(parts) if parts else "A question needs your answer."

    @staticmethod
    def _plan_text(msg) -> str:
        text = (msg.get("text") or "").strip()
        if text:
            return "Plan ready. {0}".format(text)
        return "A plan is ready for your review."

    @staticmethod
    def _permission_text(msg) -> str:
        # The 'permission' earcon already signals approval is needed; speak the
        # pending action, else the human-readable message, else a generic cue.
        action = (msg.get("action") or "").strip()
        if action:
            return action
        message = (msg.get("message") or "").strip()
        return message if message else "Permission needed."

    def _selection_cue(self, session: str, verbosity: str) -> str:
        if verbosity != "everything":
            return ""
        cue = "Press the option's number to choose, or Escape to cancel."
        if session not in self._warned_immediate:
            self._warned_immediate.add(session)
            cue += " Selecting is immediate."
        return cue

    @staticmethod
    def _choice_notes(msg) -> str:
        notes = []
        questions = msg.get("questions", []) or []
        if any(isinstance(q, dict) and q.get("multiSelect") for q in questions):
            notes.append(
                "Select multiple: press each number, or Space on the "
                "highlighted item, then Enter to confirm."
            )
        if any(
            isinstance(q, dict) and len(q.get("options", []) or []) > 9
            for q in questions
        ):
            notes.append("More than nine options; use arrow keys for ten and up.")
        return " ".join(notes)

    @staticmethod
    def _read_install_record():
        """Return the install.json dict, or None if unreadable/absent. Never raises."""
        import json
        try:
            with open(str(INSTALL_RECORD_PATH), "r", encoding="utf-8") as f:
                data = json.load(f)
            return data if isinstance(data, dict) else None
        except Exception:  # noqa: BLE001 - health check must never raise
            return None

    @staticmethod
    def _launcher_present() -> bool:
        """Delegating shim -- logic lives in the platform supervisor backend."""
        from sonara.platform import get_platform
        return get_platform().supervisor.is_installed()

    def _setup_health(self, plugin_version: str):
        """Return (state, cue) where state is one of:
        "ok"            -> fully installed, no version drift   -> cue None
        "not_installed" -> no install.json or launcher (never ran `sonara install`)
        "version_drift" -> installed but plugin_version differs from this session's

        Cheap: a few file stats + a string compare. No subprocess. Never raises.
        Hotkey availability is deliberately NOT part of this check so a deliberate
        speech-only user is never nagged.
        """
        rec = self._read_install_record()
        installed = (rec is not None and self._launcher_present())
        if not installed:
            return ("not_installed",
                    "Sonara is reading aloud. To enable hotkeys and autostart, "
                    "run, slash sonara install.")
        recorded = (rec.get("plugin_version") or "")
        # Only flag drift when BOTH sides are known and differ.
        if plugin_version and recorded and plugin_version != recorded:
            return ("version_drift",
                    "Sonara was updated. Run, slash sonara install, to apply.")
        return ("ok", None)

    def handle_message(self, msg):
        t = msg.get("type")
        session = msg.get("session", "")
        verbosity = self.config.get("verbosity", "everything")
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
                # end instead; catch_up / re-read still work from history).
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
                self._options.pop(session, None)
            # Wake the speak loop ONLY when a batch is actually ready to read
            # (>= minqueue, the turn is done, or a decision is waiting). Waking on
            # every buffered delta made the loop spin on self._lock and starve the
            # hotkey worker -- the root cause of the "thinking" mute-hang. A finished
            # turn wakes via the turn_done earcon / TOOL / FLUSH paths below; the
            # speak loop's poll_interval is the safety net if a wake is ever missed.
            # Late prose after turn_done: reset the settle window so the turn-end
            # digest waits for the full turn to land (#14). Only when armed.
            if session in self._settle_pending:
                self._arm_settle(session)
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
            text = self._choice_text(msg)
            extras = [e for e in (self._choice_notes(msg),
                                  self._selection_cue(session, verbosity)) if e]
            if extras:
                text = "{0} {1}".format(text, " ".join(extras))
            self._options[session] = text
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
            if self.config.get("summary_mode"):
                self._pending_decision[session] = item
                self._arm_settle(session)
            else:
                self._enqueue_or_hold_decision(session, item, False)
            return None

        if t == MsgType.PLAN:
            text = self._plan_text(msg)
            cue = self._selection_cue(session, verbosity)
            if cue:
                text = "{0} {1}".format(text, cue)
            self._options[session] = text
            entry = self.history.record(session, "plan", text)
            self.history.end_message(session)
            item = SpeechItem(id=self._alloc_id(), session=session, kind="plan",
                              text=text, is_decision=True)
            self._pending_heard[item.id] = entry
            # Same hook race as CHOICE (#16): the PLAN can beat its lead-in prose,
            # so defer the lead-in gather + hold through the settle window in
            # summary mode; the context is then heard before the plan (audit #21).
            if self.config.get("summary_mode"):
                self._pending_decision[session] = item
                self._arm_settle(session)
            else:
                self._enqueue_or_hold_decision(session, item, False)
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
            text = self._permission_text(msg)
            cue = self._selection_cue(session, verbosity)
            if cue:
                text = "{0} {1}".format(text, cue)
            self._options[session] = text
            entry = self.history.record(session, "permission", text)
            self.history.end_message(session)
            item = SpeechItem(id=self._alloc_id(), session=session, kind="permission",
                              text=text, is_decision=True)
            self._pending_heard[item.id] = entry
            # Same hook race as CHOICE (#16): defer through the settle window in
            # summary mode so a late lead-in is digested and heard first (audit #21).
            if self.config.get("summary_mode"):
                self._pending_decision[session] = item
                self._arm_settle(session)
            else:
                self._enqueue_or_hold_decision(session, item, False)
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
                # signal (separate hook processes race). Arm a settle window and
                # digest once the session is quiet (#14). Non-summary mode has no
                # digest, so nothing to defer there.
                if self.config.get("summary_mode"):
                    self._arm_settle(session)
            return None

        if t == MsgType.FLUSH:
            cur = self._current_item
            if cur is not None and cur.session == session:
                self.speaker.cancel()
            self._drop_channel_pending(session)
            self.router.channel(session).wipe()
            self._assemblers.pop(session, None)
            self.history.reset(session)
            # A new prompt is the user cancelling this session: advance the cancel
            # epoch so any digest dispatched before now is dropped when it lands,
            # rather than spoken into the new turn (#13).
            self._summary_gen[session] = self._summary_gen.get(session, 0) + 1
            self._cancel_settle(session)      # new prompt abandons any settling turn (#14)
            self._nav_cursor.pop(session, None)
            self._last_digest_text.pop(session, None)   # no re-reading a stale digest
            self._voiced_upto.pop(session, None)         # new turn: nothing voiced yet
            self._await_choice.discard(session)          # new prompt: no question pending
            self._held_decision.pop(session, None)       # new prompt: drop any held question
            self._pending_decision.pop(session, None)    # drop a question awaiting its lead-in (#16)
            # Cancelled digests no longer count as in flight: a stale count held
            # the NEW turn's question hostage behind a dead worker (silent up to
            # summary_timeout, probe-confirmed; deep audit #25). The worker's
            # finally skips its decrement when its gen is stale (see there).
            self._inflight_digests.pop(session, None)
            self._last_dispatch_token.pop(session, None)
            self._paused.clear()
            self._wake.set()
            self._options.pop(session, None)
            return None

        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            old_fg = self.sessions.foreground()
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
                    self.router._replay_authorized.add(old_fg)
            return None

        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._teardown_session(session)
            self.router.drop(session)
            return None

        if t == MsgType.STOP:
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
            to = msg.get("to", "prev")
            fg = self._engaged_session()
            if self.config.get("summary_mode"):
                # Summary mode speaks ONE digest per turn, not the raw per-message
                # prose. Message-cursor nav (prev/next) is meaningless here, so it
                # is a SILENT no-op: no chime, and nothing enqueued onto the gated
                # session channel (which otherwise piled up and burst at turn end,
                # issue #11). Only Up (nav 'first') acts, re-reading the last
                # digest. Flush ('go to end', Ctrl+Alt+Down) is a separate handler
                # and still cuts the foreground digest.
                if to == "first":
                    moved = self._reread_last(fg) if fg is not None else False
                    self._earcon("nav" if moved else "nav_edge")
                return None
            # Every nav press chimes: the "nav" earcon when the cursor moves to a
            # message, the "nav_edge" earcon at a boundary / nothing to navigate
            # (the wavs are user-supplied; an unconfigured kind is a silent no-op).
            if fg is None:
                self._earcon("nav_edge")
                return None
            result = self._nav(fg, to)
            self._earcon("nav" if result == "moved" else "nav_edge")
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
                # target may be None (no session) -> _speak_cue routes to the
                # CONTROL channel so the confirmation is still heard.
                self._speak_cue(target, "Resumed.", exempt_mute=True)
            else:
                self._paused.set()
                # cancel() bumps the speaker's epoch so even an in-progress
                # utterance aborts. The speak loop re-queues the interrupted item
                # (sees completed=False while paused), so we don't capture it here.
                self.speaker.cancel()
                self._maybe_restore_audio()
                # "Paused." is pause_exempt so the paused branch of the speak loop
                # scans for and voices it while holding everything else. target may
                # be None -> CONTROL channel (still scanned by take_pause_exempt).
                self._speak_cue(target, "Paused.", pause_exempt=True)
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
            # target may be None -> _speak_cue routes to the CONTROL channel so the
            # confirmation is heard even when no session is registered.
            # pause_exempt: a state change made WHILE PAUSED must still be
            # confirmed, or the user cannot tell what they toggled (deep audit #25).
            self._speak_cue(target, cue, exempt_mute=True, pause_exempt=True)
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
                self._speak_cue(None, "No session.", exempt_mute=True,
                                pause_exempt=True)
            self._wake.set()
            return None

        if t == MsgType.RELOAD_KEYMAP:
            # keymap.json changed (e.g. an unbind): re-register hotkeys so it takes
            # effect without a daemon restart. Run it OFF the daemon lock: this
            # handler is invoked while holding self._lock, but _reload_hotkeys joins
            # the Windows hotkey pump thread, which itself needs self._lock to
            # dispatch a fire. Joining under the lock could stall the daemon up to
            # the join timeout and, on timeout, leave an orphaned thread that
            # re-creates the H2 dark-hotkey race. A short-lived thread does the
            # reload lock-free (and _reload_lock serializes concurrent reloads).
            threading.Thread(target=self._reload_hotkeys,
                             name="sonara-keymap-reload", daemon=True).start()
            return None

        if t == MsgType.REPEAT:
            fg = self._engaged_session()
            if fg is None:
                return None
            self._nav_cursor.pop(fg, None)   # repeat returns to the latest message
            entries = self.history.last_message(fg)
            if not entries:
                self._speak_cue(fg, "Nothing to repeat.")
                return None
            self._replay(fg, entries)
            return None

        if t == MsgType.REREAD_OPTIONS:
            fg = self._engaged_session()
            if fg is None:
                return None
            text = self._options.get(fg)
            if text:
                self._speak_cue(fg, text)
            else:
                self._speak_cue(fg, "No options right now.")
            return None

        if t == MsgType.JUMP_DECISION:
            # Mark the cancelled current item heard and advance the active
            # channel cursor past any leading non-decision items, dropping their
            # heard-markers so a later CATCH_UP doesn't replay them out of order
            # (mirrors SKIP, extended to the whole channel) (M6).
            cur = self._current_item
            if cur is not None:
                entry = self._pending_heard.get(cur.id)
                if entry is not None:
                    entry.heard = True
            # Advance the engaged session's channel cursor to the next decision item.
            fg = self._engaged_session()
            if fg is not None:
                ch = self.router.channel(fg)
                while ch.cursor < len(ch.items) and not ch.items[ch.cursor].is_decision:
                    skipped = ch.items[ch.cursor]
                    self._pending_heard.pop(skipped.id, None)
                    ch.cursor += 1
            self.speaker.cancel()
            return None

        if t == MsgType.FLUSH_SESSION:
            # Flush to end: skip ALL pending items for the engaged session and go
            # idle. Non-destructive: skipped items keep their history entries
            # UNHEARD (we pop their _pending_heard markers so note_spoken never
            # flips them True), so CATCH_UP / REPEAT can bring them back. Mirrors
            # JUMP_DECISION but advances the cursor to the very end, not the next
            # decision. Nothing is wiped; this is a cursor move. In summary mode
            # it ALSO drops deferred/held questions and kills in-flight digests
            # (#83) - a digest landing after "go to end" used to speak anyway.
            fg = self._engaged_session()
            if fg is None:
                self._earcon("nav_edge")
                return None
            dropped = self._user_caught_up(fg)
            self._earcon("nav" if dropped else "nav_edge")
            return None

        if t == MsgType.CHOICE_ANSWERED:
            # The user ANSWERED the blocking question (#83): they have heard (or
            # read) everything they need up to it. Silence the stale backlog and
            # any in-flight lead-in digest; whatever the assistant says AFTER the
            # answer flows normally. No earcon: answering is its own feedback.
            self._user_caught_up(session)
            return None

        if t == MsgType.CATCH_UP:
            fg = self.sessions.foreground()
            if fg is None:
                return None
            target = fg
            # A muted foreground has nothing AUDIBLE to catch up on: treat it as
            # empty so the handler falls through to the other-session pick, or
            # "You're all caught up." instead of replaying into dead air.
            entries = [] if self.session_prefs.muted(fg) else self.history.unheard(fg)
            preamble = None
            if not entries:
                other = self.history.other_session_with_unheard(
                    fg, skip=self.session_prefs.muted)
                if other is not None:
                    target = other
                    entries = self.history.unheard(other)
                    preamble = "Catching up on another session."
            if not entries:
                self._speak_cue(fg, "You're all caught up.")
                return None
            # Replay cleanly: cut the target's current utterance (it stays
            # unheard, so it replays FROM ITS START) and drop its queued
            # duplicates -- every unheard entry is re-replayed in order below.
            cur = self._current_item
            if cur is not None and cur.session == target:
                self.speaker.cancel()
            # Drop pending (not-yet-spoken) channel items for the target so we
            # don't double-speak: _replay re-inserts them fresh at the cursor.
            ch = self.router.channel(target)
            for it in ch.items[ch.cursor:]:
                self._pending_heard.pop(it.id, None)
            del ch.items[ch.cursor:]
            if preamble:
                self._speak_cue(fg, preamble)
            self._replay(target, entries)
            return None

        if t == MsgType.SET_RATE:
            is_delta = "delta" in msg
            if is_delta:
                try:
                    cur = int(self.config.get("rate", 200))
                    rate = max(RATE_MIN, min(RATE_MAX, cur + int(msg.get("delta", 0))))
                except (ValueError, TypeError):
                    return None
            else:
                # Validate/clamp the absolute rate just like the delta branch -- an
                # unvalidated value here is persisted to disk and breaks synthesis.
                try:
                    rate = max(RATE_MIN, min(RATE_MAX, int(msg.get("rate"))))
                except (TypeError, ValueError):
                    return None
            self.config["rate"] = rate
            self.speaker.set_rate(rate)
            save_config(self.config)
            if is_delta:
                fg = self.sessions.foreground()
                if fg is not None:
                    self._enqueue(fg, "prose", "Rate {0}.".format(rate), False)
            return None

        if t == MsgType.SET_VOICE:
            voice = msg.get("voice")
            self.config["voice"] = voice
            self.speaker.set_voice(voice)
            save_config(self.config)
            self._maybe_prewarm_chatterbox()   # switching TO a cb voice warms it
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
            try:
                n = max(MINQUEUE_MIN, min(MINQUEUE_MAX, int(msg.get("minqueue"))))
            except (TypeError, ValueError):
                return None
            self.config["minqueue"] = n
            save_config(self.config)
            return None

        if t == MsgType.SET_AUDIO_MODE:
            mode = msg.get("mode")
            if mode not in ("off", "duck", "pause"):
                return None
            self._apply_audio_mode(mode)
            return None

        if t == MsgType.SET_AUDIO_CONTROL:
            # Pre-#92 compat shim: enabled -> duck, disabled -> off.
            if "enabled" not in msg:
                return None
            self._apply_audio_mode("duck" if bool(msg.get("enabled")) else "off")
            return None

        if t == MsgType.SET_DUCK_LEVEL:
            try:
                level = max(0, min(100, int(msg.get("level"))))
            except (TypeError, ValueError):
                return None
            self.config["duck_level"] = level
            save_config(self.config)
            if self._audio_duck_on() and self.ducker.is_ducked():  # re-apply at the new level
                self.ducker.restore()
                self.ducker.duck(self._duck_exclude_pids(), level)
            target = self.router.active or self.sessions.foreground()
            self._speak_cue(target, "Duck level {0} percent.".format(level),
                            exempt_mute=True, pause_exempt=True,
                            cue_key="duck_level")
            self._wake.set()
            return None

        if t == MsgType.SET_VOLUME:
            try:
                vol = max(25, min(200, int(msg.get("volume"))))
            except (TypeError, ValueError):
                return None
            self.config["volume"] = vol
            save_config(self.config)
            self._apply_volume(vol)
            # No spoken confirmation, ever (user decision): the instant
            # session-volume change is its own feedback, and the slider is
            # the only surface, so the number is already on screen.
            self._wake.set()
            return None

        if t == MsgType.SET_SUMMARY_MODE:
            if "enabled" not in msg:
                return None
            enabled = bool(msg.get("enabled"))
            self.config["summary_mode"] = enabled
            save_config(self.config)
            target = self.router.active or self.sessions.foreground()
            self._speak_cue(target,
                            "Summary mode on." if enabled else "Summary mode off.",
                            exempt_mute=True, pause_exempt=True)
            self._wake.set()
            return None

        if t == MsgType.CYCLE_VERBOSITY:
            order = ["everything", "medium", "quiet"]
            cur = self.config.get("verbosity", "everything")
            if cur in order:
                nxt = order[(order.index(cur) + 1) % len(order)]
            else:
                nxt = order[0]
            self.config["verbosity"] = nxt
            save_config(self.config)
            fg = self.sessions.foreground()
            if fg is not None:
                self._enqueue(fg, "prose", "Verbosity {0}.".format(nxt), False)
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
        self._hotkey_q.put(None)        # unblock the hotkey worker's get() to exit
        self._maybe_restore_audio()       # never leave other apps' audio ducked or paused
        self._stop_hotkeys()
        if getattr(self, "_webui", None) is not None:
            self._webui.stop()
        srv = self._server
        if srv is not None:
            try:
                srv.close()
            except OSError:
                pass

    def _warm_chatterbox_async(self) -> None:
        """Run the prewarm warm() OFF-thread (#27): it can spawn/load the GPU
        worker, which must never run under the daemon lock (dispatch calls this
        from handle_message). Coalesced to one warm at a time."""
        if getattr(self, "_warm_inflight", False):
            return
        self._warm_inflight = True

        def _run():
            try:
                self._maybe_prewarm_chatterbox()
            finally:
                self._warm_inflight = False
        threading.Thread(target=_run, name="sonara-cb-warm", daemon=True).start()

    def _maybe_prewarm_chatterbox(self) -> None:
        """If the selected voice is a Chatterbox voice, the engine is provisioned,
        load the model in the worker in the BACKGROUND so the
        first digest does not pay the ~40s cold load. Best-effort: never blocks the
        caller and never crashes it (chatterbox is optional). Called at daemon
        startup and when the user switches TO a chatterbox voice."""
        try:
            from sonara import chatterbox
            voice = self.config.get("voice")
            if not (chatterbox.is_provisioned()
                    and chatterbox.is_chatterbox_voice(voice)):
                return
        except Exception:  # noqa: BLE001 - optional engine; never break startup
            return

        def _warm():
            try:
                from sonara import chatterbox
                chatterbox.CLIENT.warm(self.config)
            except Exception:  # noqa: BLE001 - warming is best-effort
                pass
        threading.Thread(target=_warm, name="sonara-cb-warm", daemon=True).start()

    def _start_hotkeys(self) -> None:
        """Start the platform's global-hotkey listener. On Windows this spawns an
        in-process RegisterHotKey thread; on macOS it is a no-op (the hotkeyd is a
        separate process)."""
        # Kill-switch: a ~/.sonara/no_hotkeys file (or SONARA_DISABLE_HOTKEYS=1)
        # runs speech-only (no in-process hotkey thread). A FILE flag is honoured
        # by EVERY daemon however it is spawned (hooks inherit their own env, not
        # ours), so it reliably isolates the hotkey thread when diagnosing crashes.
        flag = os.path.join(os.path.expanduser("~"), ".sonara", "no_hotkeys")
        if os.environ.get("SONARA_DISABLE_HOTKEYS") or os.path.exists(flag):
            return
        from sonara.platform import get_platform
        try:
            from sonara import keymap
            keymap.migrate_default_chord()   # one-time upgrade of the legacy chord
            backend = get_platform().hotkey
            backend.start(self._dispatch_hotkey)
            self._announce_hotkey_collisions(getattr(backend, "collisions", None))
        except Exception:  # noqa: BLE001 - hotkeys are non-essential; speech must run
            pass

    def _announce_hotkey_collisions(self, collisions) -> None:
        """Surface failed RegisterHotKey chords AUDIBLY (#65). Windows grants a
        chord to ONE process: in a split-brain (a stray older daemon surviving a
        restart) the new daemon owns the socket but not the keys, so hotkey
        presses act on a daemon the user cannot hear about - mute appears
        broken. Collisions were only recorded for `sonara doctor`; an eyes-free
        user needs to HEAR that the keys went elsewhere."""
        if not collisions:
            return
        names = ", ".join(sorted(str(c.get("action", "?")) for c in collisions))
        print("[hotkeys] failed to register: {0}".format(names),
              file=sys.stderr, flush=True)
        self._speak_cue(None,
                        "Some Sonara hotkeys are held by another program. "
                        "Restarting Sonara may fix it.",
                        exempt_mute=True, pause_exempt=True)

    def _stop_hotkeys(self) -> None:
        from sonara.platform import get_platform
        try:
            get_platform().hotkey.stop()
        except Exception:  # noqa: BLE001 - shutdown must not raise
            pass

    def _reload_hotkeys(self) -> None:
        """Apply a keymap.json change to the live hotkeys. Runs OFF the daemon lock
        (see the RELOAD_KEYMAP handler) and is serialized by _reload_lock so two
        rapid reloads can't interleave their stop/start cycles. Honors the
        no_hotkeys kill switch, then delegates to the platform backend's reload()
        seam: Windows does a (thread-joined) stop+start; macOS rewrites the resolved
        keymap and reloads the separate hotkeyd process."""
        with self._reload_lock:
            flag = os.path.join(os.path.expanduser("~"), ".sonara", "no_hotkeys")
            if os.environ.get("SONARA_DISABLE_HOTKEYS") or os.path.exists(flag):
                self._stop_hotkeys()
                return
            from sonara.platform import get_platform
            try:
                get_platform().hotkey.reload(self._dispatch_hotkey)
            except Exception:  # noqa: BLE001 - hotkeys are non-essential; speech must run
                pass

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
        "Session changed" announcement for programmatic replays (catch_up / nav /
        repeat). The caller (CATCH_UP) already speaks a "Catching up..." preamble
        when crossing sessions; the auto-announce would be a spurious duplicate."""
        ch = self.router.channel(session)
        # append=True adds at the END (after any queued decision), like the long
        # digest path -- so a new short turn never overtakes a queued question
        # (#17). append=False keeps cursor-insert for explicit user replay
        # (catch_up / nav / repeat), which should read next.
        at = len(ch.items) if append else ch.cursor
        n = 0
        for e in entries:
            item = SpeechItem(
                id=self._alloc_id(),
                session=session,
                kind=e.kind,
                text=e.text,
                is_decision=e.kind in ("choice", "plan", "permission"),
            )
            self._pending_heard[item.id] = e
            ch.items.insert(at, item)
            at += 1
            n += 1
        if n > 0:  # only if we actually inserted items
            # Mark channel ready: replayed items should be spoken without
            # waiting for minqueue threshold.
            ch.turn_done = True
            # Suppress the "Session changed" auto-announce for programmatic
            # replay (catch_up/nav/repeat): the handoff is not user-visible.
            # NOT for automatic turn delivery (the short-turn digest path):
            # there the handoff IS user-visible, and suppressing it played
            # content unattributed after another session read (audit #21).
            if suppress_announce:
                self.router._last_active = session
            # Authorize cross-session reading: replay targets that are not the
            # current fg bypass the background-policy gate so their replayed
            # items are voiced (catch_up / nav cross-session scenarios).
            fg = self.sessions.foreground()
            if session != fg:
                self.router._replay_authorized.add(session)
        self._wake.set()

    def _user_caught_up(self, session: str) -> bool:
        """The user declared everything queued for *session* stale - they
        answered the question, or pressed flush-to-end (#83). Skip the channel
        backlog non-destructively (history entries stay UNHEARD for catch-up),
        cut the in-progress utterance if it is this session's, drop a
        settle-deferred or digest-held question, and advance the digest cancel
        epoch so an in-flight lead-in digest lands dead instead of speaking
        into the post-answer flow. The turn CONTINUES (unlike FLUSH/new
        prompt): history and assemblers stay, but _voiced_upto advances past
        everything already said - the eventual turn-end digest covers only
        post-answer prose ("I want to hear what comes after", #83).
        Caller holds the lock. Returns True when anything was skipped/cut."""
        ch = self.router.channel(session)
        skipped = 0
        while ch.cursor < len(ch.items):
            self._pending_heard.pop(ch.items[ch.cursor].id, None)
            ch.cursor += 1
            skipped += 1
        ch.has_decision = False            # any pending decision was skipped
        cur = self._current_item
        cutting = cur is not None and cur.session == session
        if cutting:
            self.speaker.cancel()          # cut the in-progress utterance
            # Clear now so a rapid SECOND press (before the speak loop's
            # note_spoken runs) sees nothing left to cut and gives the edge
            # chime -- "go to end" is top/bottom, it should only move once
            # (issue #11). note_spoken also nulls this later; idempotent.
            self._current_item = None
        dropped = bool(self._pending_decision.pop(session, None))
        self._cancel_settle(session)
        dropped = bool(self._held_decision.pop(session, None)) or dropped
        self._await_choice.discard(session)
        if self._inflight_digests.get(session):
            # Kill in-flight digests: the worker's gen guard drops the result
            # ("user answered") exactly like a new prompt's FLUSH does (#13).
            self._summary_gen[session] = self._summary_gen.get(session, 0) + 1
            self._inflight_digests.pop(session, None)
            self._last_dispatch_token.pop(session, None)
            dropped = True
        # Everything said BEFORE the catch-up is dealt with: advance the voiced
        # marker so the post-answer turn-end digest never re-includes the
        # pre-question lead-in (it was skipped, not merely delayed).
        entries = [e for mid in self.history.message_ids(session)
                   for e in self.history.entries_for_message(session, mid)
                   if e.kind == "prose"]
        if entries:
            self._voiced_upto[session] = entries[-1]
        self._wake.set()
        return bool(skipped or cutting or dropped)

    def _reading_msg_id(self, session: str):
        """The message id of the item currently being spoken for *session*, or None
        (idle / nothing in flight). Used to anchor nav on the live read position."""
        cur = self._current_item
        if cur is None or cur.session != session:
            return None
        entry = self._pending_heard.get(cur.id)
        return entry.msg_id if entry is not None else None

    def _engaged_session(self):
        """The session the user is currently engaged with: the one being read
        (router.active), else the one that most recently read (persists across idle
        gaps), else the foreground. After a session-change the active reader differs
        from the foreground, so nav/repeat/reread/jump must operate on what the user
        HEARS, not the last session to submit a prompt."""
        return (self.router.active or self.router._last_active
                or self.sessions.foreground())

    def _maybe_summarize(self, session: str,
                         leadin_for_decision: bool = False) -> bool:
        """Summary mode: recap the session's prose not yet voiced this turn via a
        throwaway claude -p call (see summarizer.py), or speak it raw when short.
        Runs under the daemon lock, so it only gathers text and spawns the worker
        thread; the subprocess itself runs OFF-lock in _summary_worker.

        Called at turn end AND when a blocking decision arrives: a question never
        reaches turn_done, so its lead-in prose would otherwise be silently dropped.
        Only prose recorded SINCE the last call this turn is voiced (tracked by
        _voiced_upto), so the two triggers never double-voice the same text.

        *leadin_for_decision* (#83): the gathered text precedes a pending
        QUESTION. Short lead-ins are then DIGESTED instead of replayed raw
        (mid-turn narration like "let me check the repo" is exactly the noise
        the digest cuts), and a SKIP/failed lead-in digest drops silently
        instead of falling back to the raw text.

        Returns True iff an ASYNC digest was dispatched (long lead-in) -- the
        decision handlers use this to HOLD the question until the digest lands, so
        the context is heard before the question rather than ~6s after it."""
        if not self.config.get("summary_mode"):
            return False
        leadin = bool(leadin_for_decision)
        entries = []
        for mid in self.history.message_ids(session):
            for e in self.history.entries_for_message(session, mid):
                if e.kind == "prose":
                    entries.append(e)
        # Skip everything up to and including the last entry voiced this turn,
        # located by IDENTITY. An absolute count desynced from the CAPPED history
        # deque once eviction shifted it, over-skipping unvoiced prose -- worst
        # case gathering nothing and silently dropping the turn-end digest
        # (audit #21). A marker that was itself evicted means every surviving
        # entry is unvoiced: keep them all.
        marker = self._voiced_upto.get(session)
        if marker is not None:
            for i in range(len(entries) - 1, -1, -1):
                if entries[i] is marker:
                    entries = entries[i + 1:]
                    break
        text = " ".join(e.text for e in entries).strip()
        if not text:
            return False                 # decision-only / empty / already-voiced
        self._voiced_upto[session] = entries[-1]
        if len(text) < _SUMMARY_MIN_CHARS and not leadin:
            # An already-short turn needs no digest: speak the original prose
            # instead. Digesting borderline-trivial input made the model
            # verbalize meta-text ("no content to be spoken") that was then
            # read aloud; speaking the original is faster and free.
            if self.sessions.is_foreground(session):
                # Append (not cursor-insert) so a short turn never overtakes a
                # queued question -- consistent with the long digest path (#17).
                # A turn delivery must ANNOUNCE on a real reader switch (#21).
                self._replay(session, entries, append=True,
                             suppress_announce=False)
                # Up re-reads the joined text -- parity with the background
                # short-turn and digest paths; without this, Up after a short
                # foreground turn gave a dead edge chime (deep audit #25).
                self._last_digest_text[session] = text
            else:
                # Background sessions are not voiced from their own channel;
                # speak the short turn via the session channel. It joins the
                # digest SEQUENCE (#88): a short turn finishing after a long
                # one must not jump ahead of the long turn's cooking digest.
                self._land_digest(self._alloc_digest_seq(),
                                  lambda: self._enqueue_background_digest(session, text))
            return False                 # spoken synchronously; no need to hold
        # Capture the session's CANCEL epoch WITHOUT advancing it. Only a user
        # action (a new prompt -> FLUSH) advances the epoch; a turn merely ending
        # must never invalidate a previously-dispatched digest. So several
        # turn-ends with no user action between them each keep their digest (they
        # queue and play) -- the system never drops a finished message (#13).
        gen = self._summary_gen.get(session, 0)
        # Overlap the GPU warm-up with the digest (#27): the ~40s post-idle cold
        # model reload then hides inside the 10-30s haiku call instead of
        # stalling speech AFTER it (the reported ~1 minute to first audio).
        self._warm_chatterbox_async()
        self._summary_token += 1
        token = self._summary_token
        self._last_dispatch_token[session] = token
        self._inflight_digests[session] = self._inflight_digests.get(session, 0) + 1
        # Turn-end digests get an ordering slot (#88); lead-in digests bypass
        # (latency-critical, #83) and stay seq=None.
        seq = None if leadin else self._alloc_digest_seq()
        self._start_summary_thread(session, gen, text, token, leadin=leadin,
                                   seq=seq)
        return True                      # async digest in flight -> caller holds

    def _arm_settle(self, session: str) -> None:
        """Defer the turn-end digest until the session's prose settles. Restart the
        window on every new prose delta; fire once quiet (#14). Caller holds the
        lock (this runs from handle_message)."""
        gen = self._settle_gen.get(session, 0) + 1
        self._settle_gen[session] = gen
        self._settle_pending.add(session)
        old = self._settle_timers.pop(session, None)
        if old is not None:
            old.cancel()
        self._settle_schedule(session, gen)

    def _settle_schedule(self, session: str, gen: int) -> None:
        """Start the real settle timer. Test seam: tests replace this to drive
        _settle_fire deterministically instead of waiting on the clock."""
        settle_s = self.config.get("summary_settle_ms", 600) / 1000.0
        t = threading.Timer(settle_s, self._settle_fire, args=(session, gen))
        t.daemon = True
        self._settle_timers[session] = t
        t.start()

    def _settle_fire(self, session: str, gen: int) -> None:
        """The settle window elapsed with no new prose: dispatch the turn-end
        digest now that the full turn has landed. Runs on the Timer thread, so it
        takes the lock. A stale fire (re-armed by later prose, or cancelled by
        FLUSH) is a no-op via the generation guard."""
        with self._lock:
            if self._settle_gen.get(session) != gen:
                return
            self._settle_pending.discard(session)
            self._settle_timers.pop(session, None)
            item = self._pending_decision.pop(session, None)
            if item is not None:
                # A question was waiting on its lead-in: gather it now (present
                # after the settle) and hold the question after the context (#16).
                # Lead-in mode (#83): short lead-ins are digested (not read raw)
                # and a SKIP result drops instead of raw-falling-back.
                digesting = self._maybe_summarize(session,
                                                  leadin_for_decision=True)
                self._enqueue_or_hold_decision(session, item, digesting)
            else:
                self._maybe_summarize(session)

    def _cancel_settle(self, session: str) -> None:
        """Drop any pending settle window: a new prompt abandons the turn. Bumps
        the generation so an already-scheduled fire becomes a no-op."""
        self._settle_pending.discard(session)
        self._settle_gen[session] = self._settle_gen.get(session, 0) + 1
        t = self._settle_timers.pop(session, None)
        if t is not None:
            t.cancel()

    def _enqueue_or_hold_decision(self, session: str, item, digesting: bool) -> None:
        """Enqueue a decision item now, OR hold it until the lead-in digest lands
        (context-first ordering). Held items are appended by the OWNING
        _summary_worker after its digest; if the digest fails or is superseded the
        owner still enqueues the held item, so a blocking question is never lost.

        Holds not only when THIS call dispatched a digest, but also when an
        earlier digest of the same turn is still in flight -- otherwise a decision
        whose own lead-in gather found nothing new was enqueued immediately and
        played BEFORE its context (audit #21)."""
        if digesting or self._inflight_digests.get(session, 0) > 0:
            owner = self._last_dispatch_token.get(session, 0)
            self._held_decision[session] = (owner, item)
            # Cap the hold (#83, retuned #103): the wedge guard for a hung
            # summarizer. The normal release is the digest worker's finally,
            # which frees the question the moment its context lands or fails.
            # Past the cap the question speaks and the digest follows
            # (bounded inversion; a caught-up user drops it).
            self._schedule_hold_release(session, owner, item)
        else:
            self.router.channel(session).append(item)
        self._wake.set()

    def _schedule_hold_release(self, session: str, owner: int, item) -> None:
        """Arm the held-question release timer. Test seam: tests call
        _release_held_decision directly instead of waiting on the clock."""
        t = threading.Timer(_DECISION_HOLD_MAX_S, self._release_held_decision,
                            args=(session, owner, item))
        t.daemon = True
        t.start()

    def _release_held_decision(self, session: str, owner: int, item) -> None:
        """The hold cap elapsed: if the digest still has not landed, speak the
        question NOW (#83). Idempotent vs the digest worker: whichever runs
        first pops the hold; the other finds it gone and does nothing."""
        with self._lock:
            held = self._held_decision.get(session)
            if held is None or held[0] != owner or held[1] is not item:
                return                     # already released (digest landed / caught up)
            self._held_decision.pop(session, None)
            self.router.channel(session).append(item)
            self._wake.set()

    def _alloc_digest_seq(self) -> int:
        """Hand out the next digest sequence number (#88). Caller holds the
        lock. Sequence order == dispatch order == turn-finish order."""
        seq = self._digest_seq_next
        self._digest_seq_next += 1
        return seq

    def _land_digest(self, seq, apply) -> None:
        """Reorder buffer release (#88): park *apply* under *seq* and flush every
        consecutive ready slot from the serve pointer. Digests thus become
        audible strictly in dispatch order regardless of summarizer latency;
        a dropped/cancelled digest lands with apply=None and just frees its
        slot. seq=None bypasses (lead-in digests, #83: latency-critical and
        session-ordered by the question hold). Caller holds the lock. Every
        dispatched seq MUST eventually land exactly once - the workers land in
        their finally - or later digests would park forever."""
        if seq is None:
            if apply is not None:
                apply()
            return
        if seq < self._digest_seq_serve:
            return                       # already served (exceptional re-land)
        self._digest_parked[seq] = apply
        while self._digest_seq_serve in self._digest_parked:
            fn = self._digest_parked.pop(self._digest_seq_serve)
            self._digest_seq_serve += 1
            if fn is not None:
                fn()

    def _start_summary_thread(self, session: str, gen: int, text: str,
                              token: int = 0, leadin: bool = False,
                              seq=None) -> None:
        threading.Thread(target=self._summary_worker,
                         args=(session, gen, text, token, leadin, seq),
                         name="sonara-summary", daemon=True).start()

    def _summary_worker(self, session: str, gen: int, text: str,
                        token: int = 0, leadin: bool = False,
                        seq=None) -> None:
        """Run the summarizer subprocess OFF-lock, then apply the result under the
        lock: enqueue the spoken summary, or fire the failure cue. A result whose
        generation was superseded by a newer turn end is dropped silently.
        Turn-end results release through the reorder buffer (#88, *seq*), so
        digests are heard in turn-finish order regardless of model latency."""
        import sys
        from sonara import summarizer

        def _log(reason):
            # stderr reaches speechd.log via the supervisor redirect, so a
            # silent recap failure is diagnosable instead of a mystery.
            print("[summary] {0}".format(reason), file=sys.stderr, flush=True)

        import time as _time
        fn = self._summarize_fn or summarizer.summarize
        t0 = _time.monotonic()
        style = self.config.get("summary_style", "natural")
        prompts = self.config.get("summary_prompts") or {}
        try:
            summary = fn(text,
                         model=self.config.get("summary_model", "haiku"),
                         command=self.config.get("summary_command", "claude"),
                         timeout=self.config.get("summary_timeout", 60),
                         style=style,
                         instruction=prompts.get(style),
                         debug_log=_log)
        except Exception:  # noqa: BLE001 - a summary failure must never crash the daemon
            summary = None
        if summary:
            # Success trail: when a digest sounds wrong (truncated, odd), the
            # log shows exactly what the model returned vs what was spoken; the
            # duration makes latency complaints diagnosable from the log (#27).
            _log("digest ok in {0:.1f}s: {1} chars in, {2} chars out: {3!r}".format(
                _time.monotonic() - t0, len(text), len(summary), summary[:120]))
        with self._lock:
            # A question whose lead-in this digest recaps was HELD for context-first
            # ordering; append it AFTER the digest below. Only the OWNING worker
            # (the dispatch the hold was placed behind) may take it -- an earlier
            # same-gen worker landing first must leave it for its owner, or the
            # question plays before its own context (audit #21). The finally
            # guarantees the owner plays it even on a dropped/failed digest -- a
            # blocking prompt is never lost (FLUSH clears a stale one).
            held = None
            held_entry = self._held_decision.get(session)
            if held_entry is not None and held_entry[0] == token:
                self._held_decision.pop(session, None)
                held = held_entry[1]

            def apply():
                # Runs at RELEASE time (#88): possibly later than completion,
                # after earlier-dispatched digests landed. State checks (gen,
                # foreground) therefore happen HERE, not at completion.
                if self._summary_gen.get(session, 0) != gen:
                    _log("digest dropped: user prompted this session since dispatch")
                    return               # the user moved on -> this reading is cancelled
                out = summary
                if not out:
                    if leadin:
                        # A lead-in digest that came back SKIP/empty/failed is
                        # pure process narration (#83): drop it silently. The
                        # question it contextualized still speaks via the
                        # held-release in the finally below - only the noise
                        # dies, never the blocking prompt.
                        _log("lead-in digest empty/SKIP: dropped")
                        return
                    # SKIP / empty / failed digest. A session's LATEST message must
                    # ALWAYS be read (user spec: never skip the last message --
                    # digested or not). This digest is the latest (it was not
                    # superseded above), so fall back to the RAW text rather than
                    # dropping it. Only a genuinely empty turn stays silent.
                    if not (text or "").strip():
                        self._earcon("summary_failed")
                        return
                    out = text
                # A held question's context goes via the SESSION channel even when
                # the session is not foreground: it is a real handoff, so the router
                # must announce "Session changed" BEFORE the context (not at the
                # question). Route EVERY digest via its own session channel so a
                # reader switch announces the handoff ("Session changed: folder" +
                # chime) BEFORE the digest. A foreground digest does NOT switch the
                # reader (no announcement) and never carries a "Session X:" prefix:
                # the router announcement is the sole session identifier (#15).
                fg = self.sessions.is_foreground(session)
                # TTS-normalize (#27): digests bypass the assembler cleaner, so
                # markdown residue / snake_case reached the voice raw and was
                # mispronounced. Normalize BEFORE recording so Up's cache-hit
                # re-read speaks the identical string.
                from sonara.cleaner import normalize_for_speech
                out = normalize_for_speech(out)
                entry = self.history.record(session, "summary", out)
                self._enqueue(session, "summary", out, False, entry=entry)
                self._last_digest_text[session] = out   # Up re-reads this verbatim
                ch = self.router.channel(session)
                ch.turn_done = True
                # Stamp the channel with the release index (#88): the router
                # serves waiting digest channels lowest-stamp-first, so the
                # heard order matches the turn-finish order just released.
                ch.release_order = self._digest_release_counter
                self._digest_release_counter += 1
                if not fg:
                    # Let it be voiced + announced regardless of background policy
                    # (earcon_only would otherwise mute a non-foreground session).
                    self.router._replay_authorized.add(session)
                self._wake.set()

            landed = False
            try:
                self._land_digest(seq, apply)
                landed = True
            finally:
                if not landed:
                    # apply raised: the ordering slot must still release or every
                    # later digest parks forever (#88).
                    self._land_digest(seq, None)
                # This worker is done: it no longer counts as in flight (a later
                # decision must not hold behind a digest that already landed).
                # ONLY when this worker's gen is still current: FLUSH/SESSION_END
                # already dropped a cancelled worker's count, so a stale worker
                # must not steal a POST-flush dispatch's count (deep audit #25).
                if self._summary_gen.get(session, 0) == gen:
                    n = self._inflight_digests.get(session, 0) - 1
                    if n > 0:
                        self._inflight_digests[session] = n
                    else:
                        self._inflight_digests.pop(session, None)
                if held is not None:
                    self.router.channel(held.session).append(held)  # question after context
                    self._wake.set()

    def _enqueue_background_digest(self, session: str, text: str) -> None:
        """Speak a background session's short-turn content via ITS OWN channel, so
        the router announces the handoff ("Session changed: folder" + chime) before
        it -- matching the digest path. Was on the CONTROL lane, which is silent
        (no chime), plays out of order, and survives a new prompt's FLUSH (so stale
        content lingered and replayed). Unprefixed (the announcement names it) and
        replay-authorized so the background policy does not mute it. Caller holds
        self._lock."""
        entry = self.history.record(session, "summary", text)
        self._enqueue(session, "summary", text, False, entry=entry)
        self._last_digest_text[session] = text   # Up re-reads this verbatim
        ch = self.router.channel(session)
        ch.turn_done = True
        ch.release_order = self._digest_release_counter   # heard in release order (#88)
        self._digest_release_counter += 1
        self.router._replay_authorized.add(session)
        self._wake.set()

    def _nav(self, session: str, to: str) -> str:
        """Move the per-session message cursor and play from there to the end.
        Returns "moved" if the cursor actually moved, else "edge" (already at the
        boundary, or nothing to navigate) -- the NAV handler uses this to pick the
        nav vs nav-edge chime.

        The cursor indexes the current turn's messages (history resets each
        prompt), oldest..newest; absent == the latest. 'next'/'prev' step one
        message and CLAMP at the ends (no wrap; at the newest, 'next' just
        re-reads it); 'first'/'last' jump to the start/end of the turn. Every
        move cuts current speech, resets the channel cursor to the target message,
        and reads the target message AND every later one (seek-and-play) so
        playback continues instead of stopping after a single item. Newly
        streamed prose enqueues after these and continues seamlessly."""
        ids = self.history.message_ids(session)
        if not ids:
            self._enqueue(session, "prose", "Nothing to navigate yet.", False)
            return "edge"
        n = len(ids)
        # Anchor on a STABLE message id, not a position: new paragraphs streaming
        # in append ids without shifting where the cursor points. Unset/stale ->
        # the latest. The cursor only clears on a new prompt (FLUSH).
        cur_id = self._nav_cursor.get(session)
        if cur_id is None:
            # No parked nav cursor (the user hasn't navigated yet this turn):
            # anchor on the message currently being READ, so next/prev move
            # relative to what the user hears -- not the latest message (which made
            # 'next' during a live read jump to the end with an edge chime).
            cur_id = self._reading_msg_id(session)
        cur = ids.index(cur_id) if cur_id in ids else n - 1
        if to == "next":
            new = min(cur + 1, n - 1)
        elif to == "prev":
            new = max(cur - 1, 0)
        elif to == "first":
            new = 0
        elif to == "last":
            new = n - 1
        else:
            return "edge"
        moved = new != cur                       # did the cursor actually move?
        if new >= n - 1:
            # Reached the latest message: clear the cursor so it tracks the live
            # edge again (absent == latest), and so a following 'prev' steps back
            # from the newest rather than a stale anchor.
            self._nav_cursor.pop(session, None)
        else:
            self._nav_cursor[session] = ids[new]   # parked on a past message
        self.speaker.cancel()
        # Clear any not-yet-spoken items from the channel so the replay is the
        # sole pending work (mirrors the old queue-clear semantics of _nav).
        ch = self.router.channel(session)
        for it in ch.items[ch.cursor:]:
            self._pending_heard.pop(it.id, None)
        del ch.items[ch.cursor:]
        # Seek-and-play: insert the target AND every later item at the channel
        # cursor so they read from here forward. Newly streamed prose appends
        # after these and continues seamlessly -- no jump from replay into live.
        entries = []
        for mid in ids[new:]:
            entries.extend(self.history.entries_for_message(session, mid))
        self._replay(session, entries)
        return "moved" if moved else "edge"

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
        self._nav_cursor.pop(session, None)
        self.speaker.cancel()                    # restart now, don't wait out the read
        ch = self.router.channel(session)
        # Drop pending prose so the re-read is next -- but PRESERVE queued
        # decision items (a blocking question deleted here was gone forever,
        # nothing replayed it; audit #21). They re-queue after the digest,
        # keeping the context-first order.
        preserved = []
        for it in ch.items[ch.cursor:]:
            if it.is_decision:
                preserved.append(it)             # keeps its _pending_heard marker
            else:
                self._pending_heard.pop(it.id, None)
        del ch.items[ch.cursor:]
        if cur is not None and ch.cursor > 0 and ch.items[ch.cursor - 1] is cur:
            # un-consume the interrupted question so history keeps ONE copy
            del ch.items[ch.cursor - 1]
            ch.cursor -= 1
        tail = []
        if text:
            tail.append(SpeechItem(
                id=self._alloc_id(), session=session, kind="summary",
                text=text, is_decision=False))
        if cur is not None:
            tail.append(cur)                     # the interrupted question, from the top
        tail.extend(preserved)                   # then any still-queued question(s)
        ch.items[ch.cursor:ch.cursor] = tail
        ch.has_decision = any(it.is_decision for it in ch.items[ch.cursor:])
        ch.turn_done = True                      # ready() -> plays now (minqueue-exempt)
        self._wake.set()
        return True

    def _resume(self) -> None:
        """Clear pause and wake the speak loop. The interrupted utterance was
        already re-queued at the front by the speak loop when its speak() returned
        not-completed during the pause, so resume picks back up where it stopped."""
        self._paused.clear()
        self._wake.set()

    def _dispatch_hotkey(self, message: dict) -> None:
        """Called ON the Windows hotkey PUMP thread for each fire. It MUST NOT block:
        debounce (cheap, pump-thread-only state) then hand the message to the worker
        queue and return to GetMessage immediately. Running handle_message here
        (under self._lock) used to stall the pump whenever the daemon held the lock
        streaming prose, so presses queued at the OS level and burst later -- the
        mute-hang. The worker (_hotkey_worker) applies the message under the lock."""
        import time as _t
        if self._debounce_suppress(message.get("type"), _t.monotonic()):
            return   # a too-fast repeat of the same toggle -> ignore
        self._hotkey_q.put(message)

    def _hotkey_worker(self) -> None:
        """Drain queued hotkey fires and apply each under self._lock -- OFF the pump
        thread, so a busy daemon can never stall hotkey CAPTURE. Serialized (single
        worker) like the old synchronous dispatch, and serialized against the socket
        path via self._lock."""
        while self._running.is_set():
            try:
                message = self._hotkey_q.get(timeout=0.2)
            except queue.Empty:
                continue
            if message is None:        # shutdown sentinel from stop()
                break
            self._process_hotkey(message)

    def _process_hotkey(self, message: dict) -> None:
        """Apply one hotkey message exactly like an inbound socket message.

        MUST hold self._lock around handle_message, identical to the socket path
        (_handle_conn): it mutates shared state (channels, history, config)
        concurrently with the speak loop, so without the lock it races -> 'list
        changed size during iteration' / corruption. handle_message and its callees
        never acquire self._lock (note_spoken/speak run on the speak thread), so this
        is deadlock-free. Contained so one bad hotkey can't kill the worker."""
        try:
            with self._lock:
                self.handle_message(message)
        except Exception:  # noqa: BLE001 - one bad hotkey must not kill the worker
            import sys
            import traceback
            traceback.print_exc(file=sys.stderr)

    def _debounce_suppress(self, mtype, now) -> bool:
        """True if *mtype* is a repeat of the same TOGGLE hotkey within the debounce
        window -- collapses an accidental/rapid double-tap into one action. Only the
        toggles in _DEBOUNCED_HOTKEYS are debounced; nav/repeat/skip pass through so
        repeated directional presses still register. Runs on the single hotkey pump
        thread, so the unlocked _hotkey_last access is race-free."""
        if mtype not in _DEBOUNCED_HOTKEYS:
            return False
        last = self._hotkey_last.get(mtype)
        if last is not None and (now - last) < _HOTKEY_DEBOUNCE_S:
            return True
        self._hotkey_last[mtype] = now
        return False

    def _speak_loop(self) -> None:
        self._running.set()
        while self._running.is_set():
            try:
                self._speak_loop_once()
            except Exception:  # noqa: BLE001 - NOTHING may permanently kill the
                # speak thread. A crash in next_item/note_spoken/etc. used to leave
                # the daemon alive (earcons kept firing) but mute forever until a
                # restart. Log the traceback (captured by the daemon log) and keep
                # going; a short wait avoids a tight error-spin.
                import sys
                import traceback
                traceback.print_exc(file=sys.stderr)
                self._wake.wait(0.1)

    def _signal_speak_failure(self) -> None:
        """An utterance raised (missing TTS extra, synth/playback failure, ...).
        The inner speak-loop handlers swallow it so one bad item can't wedge the
        loop -- but for an eyes-free user a swallowed exception is a SILENT no-op,
        the worst outcome (#41). Signal it audibly (error earcon) and log the
        traceback. Never raises -- error signaling must not itself re-break the
        loop. Call only from within an active `except` block (print_exc reads the
        handled exception)."""
        try:
            self._earcon("error")
        except Exception:  # noqa: BLE001 - signaling failure must not wedge the loop
            pass
        try:
            import sys
            import traceback
            traceback.print_exc(file=sys.stderr)
        except Exception:  # noqa: BLE001 - logging failure must not wedge the loop
            pass

    def _speak_cue(self, session, text: str, exempt_mute: bool = False,
                   pause_exempt: bool = False, cue_key=None) -> None:
        """Speak a one-off confirmation/feedback cue (pause/mute/repeat/reread/...).
        These ALWAYS go to the reserved CONTROL channel, which the router serves
        ahead of every session on `pending() > 0` -- bypassing the minqueue gate. A
        session channel is gated by `ready()` (minqueue items / turn_done), so a cue
        placed there during a live stream would sit unplayed and then burst out when
        the turn flushed; CONTROL makes the cue immediate regardless of stream state.
        The *session* arg is accepted for call-site clarity but no longer routes.

        *cue_key* coalesces slider spam: a keyed cue removes every pending cue
        with the same key and cuts one mid-speech, so dragging a slider speaks
        only the final value instead of the whole stacked sweep."""
        from sonara.router import CONTROL
        ch = self.router.channel(CONTROL)
        if ch.caught_up():
            ch.wipe()                      # control cues don't replay; keep it small
        elif cue_key is not None:
            stale = [i for i in range(ch.cursor, len(ch.items))
                     if ch.items[i].cue_key == cue_key]
            for i in reversed(stale):
                ch.items.pop(i)
        if cue_key is not None:
            cur = self._current_item
            if cur is not None and getattr(cur, "cue_key", None) == cue_key:
                self.speaker.cancel()      # stale value mid-utterance: cut it
        item = SpeechItem(id=self._alloc_id(), session=CONTROL, kind="prose",
                          text=text, is_decision=False, mute_exempt=exempt_mute,
                          pause_exempt=pause_exempt, cue_key=cue_key)
        # APPEND, do not cursor-insert: CONTROL is already served ahead of every
        # session, and inserting at the cursor made STACKED cues play LIFO --
        # the user heard state confirmations newest-first (deep audit #25).
        ch.append(item)
        self._wake.set()

    def _cue_voice(self):
        """The voice cues speak in (#60): config cue_voice (default af_heart,
        the warm-Kokoro pick -- ~0.3s per cue once loaded, far nicer than the
        native David/Zira). A Chatterbox voice is refused here (its cold
        reload is the very unresponsiveness fast cues exist to fix) and maps
        to None = the platform's native voice, as does any lookup failure."""
        v = self.config.get("cue_voice")
        if not v:
            return None
        try:
            from sonara import kokoro, chatterbox
            if (not kokoro.is_kokoro_voice(v)) and chatterbox.is_chatterbox_voice(v):
                return None
        except Exception:  # noqa: BLE001 - a cue must never die on voice lookup
            return None
        return v

    def _cue_voice_override(self, item) -> dict:
        """speaker.speak kwargs for *item* (#60). Control feedback and
        session-change announcements speak through an always-fast voice
        (warm Kokoro by default, native Windows as floor) instead of the
        configured neural voice, so "Muted." never waits out a cold
        Chatterbox model reload. Config fast_cues (default on) disables."""
        from sonara.router import CONTROL
        if (self.config.get("fast_cues", True)
                and (item.session == CONTROL or item.kind == "session_change")):
            return {"voice": self._cue_voice()}
        return {}

    def _voice_override(self, item) -> dict:
        """speaker.speak kwargs for *item*: the fast-cue voice for control
        feedback and session-change announcements (#60), else the session's
        voice pref, else {} (the global default voice)."""
        kw = self._cue_voice_override(item)
        if kw:
            return kw
        v = self.session_prefs.voice(item.session)
        return {"voice": v} if v else {}

    def _maybe_prewarm_cue_voice(self) -> None:
        """Pre-load the Kokoro engine when cues route to a Kokoro voice (#60):
        the first cue after daemon start otherwise pays the ~3s engine load.
        Best-effort, background, never blocks or breaks the caller."""
        try:
            from sonara import kokoro
            if not (self.config.get("fast_cues", True)
                    and kokoro.is_kokoro_voice(self._cue_voice())
                    and kokoro.is_installed()):
                return
        except Exception:  # noqa: BLE001 - optional engine; never break startup
            return

        def _warm():
            try:
                from sonara.platform import get_platform
                get_platform().tts._kokoro_wav("Ready.", self.config.get("rate", 200))
            except Exception:  # noqa: BLE001 - warming is best-effort
                pass
        threading.Thread(target=_warm, name="sonara-kokoro-warm", daemon=True).start()

    def _maybe_announce_chatterbox_fallback(self) -> None:
        """Speak the pending Chatterbox fallback notice, if any, exactly once per
        daemon run. Called outside self._lock (_speak_cue does not take it)."""
        if getattr(self, "_cb_fallback_announced", False):
            return
        try:
            from sonara import chatterbox
            reason = chatterbox.pop_fallback_notice()
        except Exception:  # noqa: BLE001 - never let the notice check wedge the loop
            return
        if reason:
            self._cb_fallback_announced = True
            self._speak_cue(None, "Chatterbox unavailable, using Heart.",
                            exempt_mute=True)

    def _maybe_announce_kokoro_fallback(self) -> None:
        """Speak the pending Kokoro fallback notice, if any, exactly once per
        daemon run (#29). Mirrors the Chatterbox notice: a dead engine is
        announced instead of producing unexplained error noise."""
        if getattr(self, "_kokoro_fallback_announced", False):
            return
        try:
            from sonara import kokoro
            reason = kokoro.pop_fallback_notice()
        except Exception:  # noqa: BLE001 - never let the notice check wedge the loop
            return
        if reason:
            self._kokoro_fallback_announced = True
            self._speak_cue(None, "Kokoro unavailable, using Windows voice.",
                            exempt_mute=True)

    def _audio_mode(self) -> str:
        mode = self.config.get("audio_mode", "off")
        return mode if mode in ("off", "duck", "pause") else "off"

    def _audio_duck_on(self) -> bool:
        return self._audio_mode() == "duck"

    def _audio_pause_on(self) -> bool:
        return self._audio_mode() == "pause"

    def _duck_level(self) -> int:
        try:
            return max(0, min(100, int(self.config.get("duck_level", 20))))
        except (TypeError, ValueError):
            return 20

    def set_config_value(self, key: str, value) -> bool:
        """Set a config-only tuning key (settings page, #34). These have no
        protocol message (the CLI edits config.json directly); clamp, set under
        the lock, persist. Returns False for unknown keys/bad values."""
        clamps = {
            "summary_model":   lambda v: str(v).strip() or None,
            "summary_timeout": lambda v: max(15, min(300, int(v))),
            "summary_settle_ms": lambda v: max(0, min(5000, int(v))),
            "summary_style": lambda v: (str(v)
                if str(v) in ("tidy", "natural", "brief") else None),
            "summary_command": lambda v: (str(v)
                if str(v) in ("claude", "codex") else None),
            "fast_cues": lambda v: bool(v),
            "cue_voice": lambda v: str(v).strip() or None,
            "chatterbox_max_chunk_chars": lambda v: max(80, min(280, int(v))),
            "chatterbox_exaggeration": lambda v: max(0.0, min(1.0, float(v))),
            "chatterbox_variant": lambda v: (str(v)
                if str(v) in ("turbo", "original") else None),
        }
        fn = clamps.get(key)
        if fn is None:
            return False
        try:
            cleaned = fn(value)
        except (TypeError, ValueError):
            return False
        if cleaned is None:
            return False
        with self._lock:
            self.config[key] = cleaned
            save_config(self.config)
        if key in ("cue_voice", "fast_cues"):
            self._maybe_prewarm_cue_voice()   # switching TO a Kokoro cue voice warms it (#60)
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
        page, #34). Runs on its own thread via the platform tts runner (same
        say_runner contract the Speaker uses); coalesced to one at a time.
        The busy check-and-set is under self._lock: HTTP requests run on
        their own threads, and a bare check-then-act let two previews race."""
        with self._lock:
            if getattr(self, "_preview_busy", False):
                return False
            self._preview_busy = True
        try:
            runner = getattr(self, "_preview_runner", None)
            if runner is None:
                from sonara.platform import get_platform
                runner = get_platform().tts.run
            text = "This is {0} speaking for Sonara.".format(voice)
            rate = self.config.get("rate", 200)

            def _run():
                try:
                    handle = runner(text, voice, rate)
                    handle.wait(30)
                except Exception:  # noqa: BLE001 - preview must never crash anything
                    pass
                finally:
                    self._preview_busy = False
            threading.Thread(target=_run, name="sonara-preview", daemon=True).start()
            return True
        except Exception:  # noqa: BLE001 - a failed spawn must not wedge the flag
            self._preview_busy = False
            return False

    def _duck_exclude_pids(self) -> "set[int]":
        pids = {os.getpid()}
        try:
            pids.update(self.speaker.earcon_pids())
        except AttributeError:
            pass
        return pids

    def _apply_volume(self, percent) -> None:
        """Push the speech gain to the platform playback layer. Best-effort:
        tests and non-Windows runs have no platform backend."""
        try:
            from sonara.platform import get_platform
            get_platform().tts.set_volume(percent)
        except Exception:  # noqa: BLE001 - volume must never break the daemon
            pass

    def _maybe_engage_audio(self) -> None:
        mode = self._audio_mode()
        if mode == "duck":
            if not self.ducker.is_ducked():
                self.ducker.duck(self._duck_exclude_pids(), self._duck_level())
        elif mode == "pause":
            if not self.pauser.is_paused():
                self.pauser.pause()

    def _maybe_restore_audio(self) -> None:
        # Disengage BOTH backends defensively: a mid-speech mode switch can leave
        # the other backend engaged, and idle must never leave media ducked OR paused.
        if self.ducker.is_ducked():
            self.ducker.restore()
        if self.pauser.is_paused():
            self.pauser.resume()

    def _apply_audio_mode(self, mode: str) -> None:
        """Persist the audio behavior mode, disengage whatever backend was
        engaged (so a switch never leaves other apps ducked or paused), and
        speak the mode cue. Shared by SET_AUDIO_MODE and the SET_AUDIO_CONTROL
        compat shim."""
        if mode not in ("off", "duck", "pause"):
            return
        self.config["audio_mode"] = mode
        save_config(self.config)
        self._maybe_restore_audio()
        target = self.router.active or self.sessions.foreground()
        cue = {"off": "Audio off.", "duck": "Audio ducking.",
               "pause": "Media pause."}[mode]
        self._speak_cue(target, cue, exempt_mute=True, pause_exempt=True)
        self._wake.set()

    def _speak_loop_once(self) -> None:
        """One iteration of the speak loop. May raise; _speak_loop contains it."""
        if self._paused.is_set():
            # Idempotently restore other apps' audio while paused -- closes the window
            # where a re-duck slipped in during the pause transition. Safe to call
            # repeatedly: _maybe_restore_audio() is a no-op when not ducked/paused.
            self._maybe_restore_audio()
            # While paused, still drain a single pause_exempt cue (e.g. "Paused.")
            # before holding. Scan ALL channels at/after their cursor: a mid-utterance
            # pause rewinds the cursor past where _speak_cue inserted the cue, so a
            # plain peek() at the cursor would miss it.
            with self._lock:
                item = None
                for ch in self.router.channels.values():
                    item = ch.take_pause_exempt()
                    if item is not None:
                        self._current_item = item
                        break
                cancel_epoch = self.speaker.cancel_epoch()
            if item is not None:
                try:
                    completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch,
                                                   **self._cue_voice_override(item))
                except Exception:  # noqa: BLE001
                    self._signal_speak_failure()
                    completed = False
                self.note_spoken(item, completed)
                return
            self._wake.wait(self._poll_interval)
            self._wake.clear()
            return
        with self._lock:
            item = self.router.next_item()
            self._current_item = item
            cancel_epoch = self.speaker.cancel_epoch()
            # Global mute: drop every non-exempt item (the "Muted."/"Unmuted." cue
            # is mute_exempt so it is still heard). The router already advanced the
            # cursor, so a dropped item is consumed, not replayed.
            muted = (item is not None and self._muted and not item.mute_exempt)
            if muted:
                self._current_item = None
                self._pending_heard.pop(item.id, None)
                print("[mute] dropped: {0!r}".format((item.text or "")[:60]),
                      file=sys.stderr, flush=True)
        # Engine fallback notices: spoken once per daemon run so an eyes-free
        # user knows WHY the voice changed (the reason is already in the log).
        self._maybe_announce_chatterbox_fallback()
        self._maybe_announce_kokoro_fallback()
        if item is None:
            self._maybe_restore_audio()
            self._wake.wait(self._poll_interval)
            self._wake.clear()
            return
        if muted:
            # A dropped item also drops a pending alert for its session: mute
            # silences handoffs, so the deferred chime + announcement go too.
            if (self._pending_preamble is not None
                    and self._pending_preamble[0] == item.session):
                self._pending_preamble = None
            return
        if item.kind == "session_change":
            if self.config.get("fast_cues", True):
                # Defer the alert (#94): stash it and play the chime + spoken
                # announcement from the CONTENT utterance's on_play, so a slow
                # engine no longer plays the alert seconds before the audio.
                self._pending_preamble = (item.session, item.text)
                self._current_item = None
                return
            # fast_cues off: legacy immediate announcement in the content voice.
            try:
                self._earcon("session_change")
            except Exception:  # noqa: BLE001
                pass
            try:
                completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch,
                                               on_play=None,
                                               **self._voice_override(item))
            except Exception:  # noqa: BLE001
                self._signal_speak_failure()
                completed = False
            if not self._requeue_or_note(item, completed):
                self.note_spoken(item, completed)
            return
        # Content item. A stashed alert for THIS session plays as a preamble at
        # synthesis-ready (on_play): chime, then the spoken alert via the fast cue
        # voice (non-tracked so it never clobbers this utterance's cancellation),
        # then the normal duck/pause engage. #90's "announcement never ducks" is
        # preserved: the alert cue itself is played WITHOUT on_play, and the duck/
        # pause engage happens for the CONTENT, after the alert.
        preamble = None
        if self._pending_preamble is not None:
            if self._pending_preamble[0] == item.session:
                preamble = self._pending_preamble[1]
            else:
                self._pending_preamble = None   # stale alert for another session: drop it
        if preamble is not None:
            cue_voice = self._cue_voice()
            rate = self.config.get("rate", 200)

            def on_play(_text=preamble, _voice=cue_voice, _rate=rate):
                # Consume the alert only when it actually plays. If content synthesis
                # is interrupted (e.g. paused) BEFORE on_play fires, the preamble stays
                # armed so the replayed content still announces the handoff (#94).
                self._pending_preamble = None
                try:
                    self._earcon("session_change")
                except Exception:  # noqa: BLE001
                    pass
                try:
                    self.speaker.speak_cue_untracked(_text, _voice, _rate)
                except Exception:  # noqa: BLE001
                    pass
                self._maybe_engage_audio()
        else:
            on_play = self._maybe_engage_audio
        try:
            completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch,
                                           on_play=on_play,
                                           **self._voice_override(item))
        except Exception:  # noqa: BLE001
            self._signal_speak_failure()
            completed = False
        if not self._requeue_or_note(item, completed):
            self.note_spoken(item, completed)
            # A deferred alert that never played is kept armed only when the content
            # was requeued for replay (a pause, handled by _requeue_or_note above).
            # Here the content was noted, not requeued (completed, or a non-pause
            # cancel dropped it), so drop any still-armed alert for this session -
            # otherwise it would resurface on a later utterance for the same session
            # (#94). If on_play already played the alert, _pending_preamble is None
            # and this is a no-op.
            if (self._pending_preamble is not None
                    and self._pending_preamble[0] == item.session):
                self._pending_preamble = None

    def _handle_conn(self, conn) -> None:
        try:
            buf = b""
            with conn:
                conn.settimeout(5.0)
                # --- token handshake: the first newline-terminated line must
                # equal the daemon's session token, or the peer is dropped. ---
                while b"\n" not in buf:
                    try:
                        data = conn.recv(4096)
                    except (OSError, socket.timeout):
                        return
                    if not data:
                        return
                    buf += data
                token_line, buf = buf.split(b"\n", 1)
                if token_line.decode("utf-8", "replace") != self._token:
                    return  # reject unauthenticated peer
                while self._running.is_set():
                    # Process any complete messages already buffered (e.g. a
                    # message that arrived in the same packet as the token).
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        if not line.strip():
                            continue
                        try:
                            msg = decode(line)
                        except (ValueError, UnicodeDecodeError):
                            continue
                        reply = self._handle_message_guarded(msg)
                        if reply is not None:
                            try:
                                conn.sendall(encode(reply))
                            except OSError:
                                return
                    try:
                        data = conn.recv(4096)
                    except (OSError, socket.timeout):
                        return
                    if not data:
                        return
                    buf += data
        except OSError:
            return

    def _handle_message_guarded(self, msg):
        """Dispatch one socket message under the lock, contained so a malformed or
        buggy message logs a traceback instead of silently killing the connection
        thread (mirrors the _dispatch_hotkey guard). Returns the reply or None."""
        try:
            with self._lock:
                return self.handle_message(msg)
        except Exception:  # noqa: BLE001 - one bad message must not drop the connection
            import sys
            import traceback
            traceback.print_exc(file=sys.stderr)
            return None

    def _handle_conn_guarded(self, conn) -> None:
        """Run _handle_conn, contain any crash (log it, don't die silently), and
        always release the concurrency permit so capacity recovers."""
        try:
            self._handle_conn(conn)
        except Exception:  # noqa: BLE001 - a handler crash must be logged, not silent
            import sys
            import traceback
            traceback.print_exc(file=sys.stderr)
        finally:
            self._conn_sem.release()

    def _spawn_conn_handler(self, conn) -> bool:
        """Spawn a handler thread for *conn* if under the concurrency cap; else
        drop (close) the connection. Returns True iff a handler was spawned."""
        if not self._conn_sem.acquire(blocking=False):
            try:
                conn.close()
            except OSError:
                pass
            return False
        try:
            th = threading.Thread(target=self._handle_conn_guarded, args=(conn,), daemon=True)
            th.start()
        except Exception:  # noqa: BLE001 - thread creation can fail (resource limits)
            # The handler that would release the permit never ran: release it here
            # and drop the connection, else this slot leaks forever (M8).
            self._conn_sem.release()
            try:
                conn.close()
            except OSError:
                pass
            return False
        return True

    def _accept_loop(self) -> None:
        import time
        srv = self._server
        failures = 0
        while self._running.is_set():
            try:
                conn, _ = srv.accept()
            except OSError:
                if not self._running.is_set():
                    return                    # shutdown closed the socket
                # A transient accept failure (WSAECONNRESET burst etc.) used to
                # kill the WHOLE daemon, which the hooks then silently respawned
                # with fresh state - one of the mute-reset triggers (#65). Retry;
                # a genuinely dead socket exhausts the cap and exits as before.
                failures += 1
                if failures > 20:
                    print("[daemon] accept failing persistently; exiting",
                          file=sys.stderr, flush=True)
                    return
                print("[daemon] transient accept error; retrying",
                      file=sys.stderr, flush=True)
                time.sleep(0.2)
                continue
            failures = 0
            self._spawn_conn_handler(conn)

    def run(self) -> None:
        ensure_sonara_dir()
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind((transport.HOST, 0))
        srv.listen(16)
        port = srv.getsockname()[1]
        # Reuse the previous token across restarts (#34): the settings page's
        # Restart button and bookmarked page URLs keep working because the
        # respawned daemon accepts the same token. Same-user security boundary
        # is unchanged -- the token still lives 0600 in the user's own home.
        self._token = _persistent_token()
        from sonara.webui import SettingsServer
        self._webui = SettingsServer(self, self._token,
                                     int(self.config.get("settings_port", 27431)))
        try:
            http_port = self._webui.start()
        except Exception:  # noqa: BLE001 - the page must never block speech
            self._webui, http_port = None, None
        self._start_preview_builder()   # render missing voice previews (#38)
        transport.write_lockfile(
            LOCK_PATH, transport.HOST, port, self._token, os.getpid(),
            http_port=http_port)
        self._server = srv
        self._running.set()

        speak_thread = threading.Thread(target=self._speak_loop, daemon=True)
        accept_thread = threading.Thread(target=self._accept_loop, daemon=True)
        hotkey_worker = threading.Thread(target=self._hotkey_worker,
                                         name="sonara-hotkey-worker", daemon=True)
        # Startup marker (#63): volatile state (mute level, pause) dies with the
        # process, so an unexplained "setting reset itself" is diagnosable only
        # if restarts are visible in the log.
        print("[daemon] started pid={0}".format(os.getpid()),
              file=sys.stderr, flush=True)
        speak_thread.start()
        accept_thread.start()
        hotkey_worker.start()
        self._start_hotkeys()
        self._maybe_prewarm_chatterbox()   # warm the model at startup if cb voice
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


def ensure_running() -> None:
    from sonara import paths as _paths
    if os.path.exists(str(_paths.STOPPED_SENTINEL_PATH)):
        return   # explicitly shut down: hook events must not resurrect it (#23)
    if socket_connectable():
        return
    from sonara.platform import get_platform
    argv, kwargs = get_platform().supervisor.launch_spec()
    subprocess.Popen(argv, **kwargs)


_FAULT_FILE = None


def _arm_faulthandler() -> None:
    """Dump every thread's Python stack to SONARA_DIR/faulthandler.log on a NATIVE
    crash (access violation / segfault in WinRT, ctypes, or winsound) -- the only
    way to see otherwise-silent C-level daemon deaths. Never raises."""
    global _FAULT_FILE
    try:
        import faulthandler
        # Import SONARA_DIR LIVE (not at module top) so the conftest monkeypatch /
        # any SONARA_DIR redirection takes effect; a top-level import would freeze
        # the value before tests patch it and leak into the real ~/.sonara.
        from sonara.paths import SONARA_DIR
        path = str(SONARA_DIR / "faulthandler.log")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        # Preserve a REAL crash dump before truncating (#65): every spawn
        # attempt (including instantly-exiting singleton losers) re-arms and
        # rewrote the file, so the silent-respawn flow destroyed the evidence
        # of the very crash it was healing seconds earlier. A file with more
        # than the one-line armed header is a dump: rotate it aside. A
        # header-only file is safe to truncate, so raced losers cannot rotate
        # the preserved dump away either.
        try:
            with open(path, encoding="utf-8") as fh:
                prior = fh.read(65536)
            if prior.count("\n") > 1:
                os.replace(path, str(SONARA_DIR / "faulthandler.prev.log"))
        except OSError:
            pass
        # mode 'w': only the latest run's crash matters; never grow unbounded.
        _FAULT_FILE = open(path, "w", encoding="utf-8")
        _FAULT_FILE.write("=== faulthandler armed: pid {0} ===\n".format(os.getpid()))
        _FAULT_FILE.flush()
        faulthandler.enable(file=_FAULT_FILE, all_threads=True)
    except Exception:  # noqa: BLE001 - diagnostics must never break startup
        pass


def _preload_vc_runtime() -> None:
    """win32: preload the SYSTEM VC++ runtime before any speech engine import
    (#29). PyWinRT bundles an old MSVCP140.dll inside its package; whichever
    engine imports first binds its copy process-wide, and onnxruntime (Kokoro)
    crashes inside the old one ('DLL initialization routine failed') whenever a
    WinRT/Chatterbox voice spoke first in this daemon's lifetime. The System32
    runtime is newer and serves BOTH engines, so loading it first makes engine
    import order irrelevant. Missing DLLs are tolerated: engines then fall back
    to their bundled copies exactly as before."""
    import sys
    if sys.platform != "win32":
        return
    import ctypes
    root = os.environ.get("SystemRoot", r"C:\Windows")
    for dll in ("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"):
        try:
            ctypes.WinDLL(os.path.join(root, "System32", dll))
        except OSError:
            pass


def _harden_process(k32=None) -> None:
    """Keep the daemon responsive to global hotkeys even after long idle.

    Windows 11 puts idle, window-less background processes into EcoQoS / power
    throttling, and the Task Scheduler launches us at BelowNormal priority. A
    throttled hotkey-pump thread drops/delays the first WM_HOTKEY presses after a
    long idle (the "press 3-4 times before it registers" bug), and the timing skew
    occasionally double-fires a toggle. So at startup we (1) opt the process out of
    power throttling (ControlMask=EXECUTION_SPEED, StateMask=0 => "never throttle
    me") and (2) raise the priority class to Normal. Best-effort; never raises.
    *k32* is injectable for tests."""
    import sys
    if sys.platform != "win32":
        return
    try:
        import ctypes
        from ctypes import wintypes
        if k32 is None:
            # Fresh WinDLL so the argtypes/restype we set here never mutate the
            # shared ctypes.windll.kernel32 used elsewhere. Proper HANDLE typing is
            # REQUIRED: GetCurrentProcess()'s pseudo-handle is -1, and without a
            # 64-bit HANDLE restype/argtype ctypes truncates it to a 32-bit value,
            # so both calls fail with ERROR_INVALID_HANDLE (6) and silently no-op.
            k32 = ctypes.WinDLL("kernel32", use_last_error=True)
            k32.GetCurrentProcess.restype = wintypes.HANDLE
            k32.SetProcessInformation.argtypes = [
                wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
            k32.SetProcessInformation.restype = wintypes.BOOL
            k32.SetPriorityClass.argtypes = [wintypes.HANDLE, wintypes.DWORD]
            k32.SetPriorityClass.restype = wintypes.BOOL

        class _PPTS(ctypes.Structure):
            _fields_ = [("Version", wintypes.DWORD),
                        ("ControlMask", wintypes.DWORD),
                        ("StateMask", wintypes.DWORD)]

        _PROCESS_POWER_THROTTLING = 4            # ProcessPowerThrottling info class
        _EXECUTION_SPEED = 0x1                   # PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        _NORMAL_PRIORITY_CLASS = 0x00000020
        h = k32.GetCurrentProcess()
        st = _PPTS(1, _EXECUTION_SPEED, 0)       # Version=1, control speed, state OFF
        k32.SetProcessInformation(h, _PROCESS_POWER_THROTTLING,
                                  ctypes.byref(st), ctypes.sizeof(st))
        k32.SetPriorityClass(h, _NORMAL_PRIORITY_CLASS)
    except Exception:  # noqa: BLE001 - hardening must never break startup
        pass


def main() -> None:
    _arm_faulthandler()
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
    _MUTEX = transport.acquire_singleton_mutex()
    if _MUTEX is None:
        return  # another daemon already owns the single-instance mutex
    _SINGLETON = transport.acquire_singleton(SINGLETON_PATH)  # pid record (best-effort)

    _harden_process()   # win32: opt out of EcoQoS throttling + raise priority so
                        # global hotkeys stay responsive after long idle
    _preload_vc_runtime()   # win32: system VC runtime first, before any engine (#29)

    from sonara.speaker import Speaker
    from sonara.sessions import SessionManager
    from sonara.platform import get_platform

    _backend = get_platform()
    from sonara.platform.windows.ducking import restore_from_state_file
    restore_from_state_file()   # un-duck anything a crashed prior daemon left down
    from sonara.platform.windows.pausing import resume_from_state_file as _resume_paused
    _resume_paused()   # resume anything a crashed prior daemon left paused
    cfg = load_config()
    if "earcons" not in cfg:
        cfg["earcons"] = _backend.earcon.default_earcons()
    speaker = Speaker(
        voice=cfg.get("voice"),
        rate=cfg.get("rate", 200),
        say_runner=_backend.tts.run,
        earcon_player=_backend.earcon.play,
        earcons=cfg.get("earcons"),
    )
    sessions = SessionManager(background_policy=cfg.get("background_policy", "earcon_only"),
                              store_path=SESSIONS_PATH, seen_path=SESSION_SEEN_PATH)
    from sonara.session_prefs import SessionPrefs
    daemon = SpeechDaemon(speaker, sessions, cfg,
                          ducker=_backend.ducker, pauser=_backend.pauser,
                          prefs=SessionPrefs(store_path=SESSION_PREFS_PATH))
    daemon._apply_volume(cfg.get("volume", 100))   # restore persisted speech gain
    daemon.run()


if __name__ == "__main__":
    main()

