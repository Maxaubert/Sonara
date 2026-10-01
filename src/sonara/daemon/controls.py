"""The user's playback controls (#141): PAUSE, MUTE, SKIP, STOP, NEXT_SESSION,
flush to end (FLUSH_SESSION), Up (NAV: restart the turn, or re-read the last
digest in summary mode) and REPEAT, plus the engaged session and the
caught-up / flush-all paths that CHOICE_ANSWERED and STOP share.

Product rule: one message, always the last. Up restarts the latest turn and
nothing here may silently drop it."""
from __future__ import annotations

import sys

from sonara.daemon import core
from sonara.protocol import MsgType
from sonara.queue import SpeechItem


class Controls:
    """Message handlers for the controls. Given the daemon (*daemon*) for the
    core state (router, speaker, cues, summary pipeline, paused event, mute
    level, heard-markers) and looks it up at call time, so a test that
    replaces a daemon attribute is honoured. Every handler runs with the
    daemon lock held, like the old handle_message branches. No state of its
    own."""

    def __init__(self, daemon) -> None:
        self._d = daemon

    def register(self, table: dict) -> None:
        core.add_handlers(table, {
            MsgType.STOP: self.on_stop,
            MsgType.SKIP: self.on_skip,
            MsgType.NAV: self.on_nav,
            MsgType.PAUSE: self.on_pause,
            MsgType.MUTE: self.on_mute,
            MsgType.NEXT_SESSION: self.on_next_session,
            MsgType.REPEAT: self.on_repeat,
            MsgType.FLUSH_SESSION: self.on_flush_session,
        })

    def on_stop(self, msg):
        # Silence everything (M3/F8): flush-to-end first cancels settle
        # windows, in-flight and parked digests and held questions, so
        # nothing cooking speaks after the stop; then clear the queues.
        d = self._d
        self.flush_all()
        for s in list(d.router.channels):
            d._drop_channel_pending(s)
        for ch in d.router.channels.values():
            ch.wipe()
        d.speaker.cancel()
        return None

    def on_skip(self, msg):
        d = self._d
        cur = d._current_item
        if cur is not None:
            entry = d._pending_heard.get(cur.id)
            if entry is not None:
                entry.heard = True
        d.speaker.cancel()
        return None

    def on_nav(self, msg):
        # One message, always the last: the only nav target is 'first' (Up),
        # which restarts the latest turn from the top. Any other target (the
        # removed prev/next/last stepping) is a SILENT no-op, so a stale
        # client cannot pile items onto a channel.
        d = self._d
        if msg.get("to", "first") != "first":
            return None
        fg = self.engaged_session()
        if d.config.get("summary_mode"):
            # Summary mode speaks ONE digest per turn: Up re-reads it. Flush
            # ('go to end', Ctrl+Alt+Down) is a separate handler.
            moved = self.reread_last(fg) if fg is not None else False
            d._earcon("nav" if moved else "nav_edge")
            return None
        # The "nav" earcon when the turn restarts, "nav_edge" when there is
        # nothing to restart.
        if fg is None:
            d._earcon("nav_edge")
            return None
        d._earcon("nav" if self.restart_turn(fg) else "nav_edge")
        return None

    def on_pause(self, msg):
        # Temporary play/pause. Pause stops the current utterance and holds the
        # loop; resume re-speaks the interrupted item so it picks back up. Also
        # auto-cleared by a new prompt (see the FLUSH handler).
        d = self._d
        target = d.router.active or d.sessions.foreground()
        if d._paused.is_set():
            # Resuming: clear flag, wake loop, then insert "Resumed." cue at
            # the active channel's cursor so it plays ahead of the interrupted
            # utterance (which was re-queued there on pause). mute_exempt so
            # it is always heard even if the session is also muted.
            d._paused.clear()
            d._wake.set()
            # target may be None (no session) -> the cue routes to the
            # CONTROL channel so the confirmation is still heard.
            d._cues.speak(target, "Resumed.", exempt_mute=True)
        else:
            d._paused.set()
            # cancel() bumps the speaker's epoch so even an in-progress
            # utterance aborts. The speak loop re-queues the interrupted item
            # (sees completed=False while paused), so we don't capture it here.
            d.speaker.cancel()
            d._audio.restore()
            # "Paused." is pause_exempt so the paused branch of the speak loop
            # scans for and voices it while holding everything else. target may
            # be None -> CONTROL channel (still scanned by take_pause_exempt).
            d._cues.speak(target, "Paused.", pause_exempt=True)
        return None

    def on_mute(self, msg):
        # Global mute CYCLE: Unmuted -> Muted -> Super Muted -> Unmuted.
        #   1 Muted:       prose silenced, beeps (earcons) still fire.
        #   2 Super Muted: prose AND beeps silenced (full mute).
        # The spoken state confirmation is mute_exempt (always heard via TTS, not
        # an earcon) so the user can tell the state and toggle out.
        d = self._d
        d._mute_level = (d._mute_level + 1) % 3
        # Persist (#65): a respawned daemon restores the level, so mute
        # survives the silent hook-lazy-start replacement.
        d.config["mute_level"] = d._mute_level
        d._persist()
        # Observability (#63): mute transitions and drops are logged so a
        # "mute did not stick" report is diagnosable from speechd.log
        # (state resets from a daemon respawn become visible too).
        print("[mute] level -> {0}".format(d._mute_level),
              file=sys.stderr, flush=True)
        if d._mute_level >= 1:
            d.speaker.cancel()           # stop the current utterance now
        cue = {1: "Muted.", 2: "Super muted.", 0: "Unmuted."}[d._mute_level]
        target = d.router.active or d.sessions.foreground()
        # target may be None -> the cue routes to the CONTROL channel so the
        # confirmation is heard even when no session is registered.
        # pause_exempt: a state change made WHILE PAUSED must still be
        # confirmed, or the user cannot tell what they toggled (deep audit #25).
        d._cues.speak(target, cue, exempt_mute=True, pause_exempt=True)
        d._wake.set()
        return None

    def on_next_session(self, msg):
        # Manual session-change: switch the active reader to another session and
        # confirm immediately (cancel the current item, like pause/mute). The
        # router arms the "Session changed" announcement; on no other session we
        # speak a soft cue.
        d = self._d
        target, _replay = d.router.next_session()
        d.speaker.cancel()
        if target is None:
            d._cues.speak(None, "No session.", exempt_mute=True,
                          pause_exempt=True)
        else:
            # Instant press feedback (#111): fire the switch chime NOW, from
            # the handler (the earcon player is a non-blocking subprocess).
            # The deferred #94 alert only sounded at the target content's
            # synthesis-ready callback, leaving a manual press with ZERO
            # audio for the whole first-chunk synthesis - it felt dead.
            try:
                d._earcon("session_change")
            except Exception:  # noqa: BLE001 - feedback must not break the switch
                pass
        d._wake.set()
        return None

    def on_repeat(self, msg):
        d = self._d
        fg = self.engaged_session()
        if fg is None:
            return None
        entries = d.history.last_message(fg)
        if not entries:
            d._cues.speak(fg, "Nothing to repeat.")
            return None
        d._replay(fg, entries)
        return None

    def on_flush_session(self, msg):
        # Flush to end: silence EVERYTHING queued or in flight across ALL
        # sessions and go idle (#107). The old per-engaged-session flush
        # left other sessions' landed or reorder-parked digests holding
        # the floor: the key chimed success, a handoff started reading
        # seconds later anyway, and a re-press in the silent gap found an
        # "empty" queue (the flush soft-lock). Non-destructive: skipped
        # items keep their history entries, so REPEAT / Up can bring them
        # back.
        d = self._d
        d._earcon("nav" if self.flush_all() else "nav_edge")
        return None

    def user_caught_up(self, session: str) -> bool:
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
        d = self._d
        ch = d.router.channel(session)
        skipped = ch.skip_to_end()         # any pending decision is skipped too
        for it in skipped:
            d._pending_heard.pop(it.id, None)
        cur = d._current_item
        cutting = cur is not None and cur.session == session
        if cutting:
            d.speaker.cancel()             # cut the in-progress utterance
            # Clear now so a rapid SECOND press (before the speak loop's
            # note_spoken runs) sees nothing left to cut and gives the edge
            # chime -- "go to end" is top/bottom, it should only move once
            # (issue #11). note_spoken also nulls this later; idempotent.
            d._current_item = None
        # Deferred and held questions, the settle window, in-flight digests
        # and the voiced marker (summary/pipeline).
        dropped = d._summary.caught_up(session)
        d._ingest.await_choice.discard(session)
        d._wake.set()
        return bool(skipped) or cutting or dropped

    def flush_all(self) -> bool:
        """Flush-to-end (#107): silence everything queued or in flight across
        ALL sessions. Per-session state goes through user_caught_up
        (non-destructive skip, current-utterance cut, in-flight digest kill,
        settle cancel); completed digests PARKED in the reorder buffer are
        landed dead in place (they already left the in-flight count, so the
        per-session kill cannot see them); a stale deferred handoff alert and
        an armed-but-unemitted switch announcement are dropped. Caller holds
        the lock. Returns True when anything was skipped, cut, or killed."""
        from sonara.router import CONTROL
        d = self._d
        sessions = set(d.router.channels) | d._summary.busy_sessions()
        cur = d._current_item
        if cur is not None:
            sessions.add(cur.session)      # cut audio even for an untracked session
        sessions.discard(CONTROL)          # control cues are sub-second; let them be
        dropped = False
        for sid in sessions:
            dropped = self.user_caught_up(sid) or dropped
        dropped = d._digests.kill_parked() or dropped
        d._playback.pending_preamble = None
        d.router.clear_pending_announce()
        return dropped

    def engaged_session(self):
        """The session the user is currently engaged with: the one being read
        (router.active), else the one that most recently read (persists across idle
        gaps), else the foreground. After a session-change the active reader differs
        from the foreground, so Up/repeat must operate on what the user
        HEARS, not the last session to submit a prompt."""
        d = self._d
        return (d.router.active or d.router.last_active
                or d.sessions.foreground())

    def restart_turn(self, session: str) -> bool:
        """Up: restart the current turn from its first message and read it to the
        end. Returns True when there was a turn to restart (the NAV handler
        chimes "nav"), False when nothing is recorded yet ("nav_edge").

        The turn is the history since the last prompt (history resets on FLUSH).
        A restart always plays, even when pressed repeatedly (#128): it cuts
        current speech, drops the channel's not-yet-spoken items and replays
        every message of the turn at the channel cursor. Newly streamed prose
        enqueues after these and continues seamlessly."""
        d = self._d
        ids = d.history.message_ids(session)
        if not ids:
            # A control cue (F6): on the session channel it wiped the seed.
            d._cues.speak(session, "Nothing to navigate yet.")
            return False
        d.speaker.cancel()
        # Clear any not-yet-spoken items from the channel so the replay is the
        # sole pending work.
        for it in d.router.channel(session).truncate_pending():
            d._pending_heard.pop(it.id, None)
        entries = []
        for mid in ids:
            entries.extend(d.history.entries_for_message(session, mid))
        d._replay(session, entries)
        return True

    def reread_last(self, session: str) -> bool:
        """Re-read the last digest immediately (summary-mode Up, issue #11). Speaks
        the EXACT text that was spoken (stored verbatim, prefix and all) so the
        rendered audio is a cache hit and replays byte-identically instead of
        regenerating (~2s + drifting intonation each time). Cuts the current read
        and restarts from the top so the press takes effect AT ONCE. Returns True if
        there was a digest to re-read (caller chimes "nav"), else False (edge)."""
        d = self._d
        text = d._last_digest_text.get(session)
        # A DECISION being spoken RIGHT NOW is not in the record yet (it joins
        # on completion) -- cancelling it without re-queueing ANNIHILATED the
        # question: edge chime, gone forever (live report 2026-07-14). Up during
        # a speaking question restarts it instead. A non-decision current item
        # (a digest) is already the record's text, so re-queueing it would
        # double-speak; the record re-read IS its restart.
        cur = d._current_item
        if cur is not None and (cur.session != session or not cur.is_decision):
            cur = None
        if not text and cur is None:
            return False
        d.speaker.cancel()                       # restart now, don't wait out the read
        ch = d.router.channel(session)
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
                d._pending_heard.pop(it.id, None)
        if cur is not None:
            # un-consume the interrupted question so history keeps ONE copy
            ch.unconsume(cur)
        tail = []
        if text:
            tail.append(SpeechItem(
                id=d._alloc_id(), session=session, kind="summary",
                text=text, is_decision=False))
        if cur is not None:
            tail.append(cur)                     # the interrupted question, from the top
        tail.extend(preserved)                   # then any still-queued question(s)
        ch.insert_at(ch.cursor, tail)            # sets has_decision for a question
        ch.turn_done = True                      # ready() -> plays now (minqueue-exempt)
        d._wake.set()
        return True
