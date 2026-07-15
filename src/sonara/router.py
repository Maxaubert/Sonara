"""Choose the single active reader among per-session channels and yield the next
item to speak. One speaker -> one reader at a time. See the design spec."""
from __future__ import annotations

from sonara.channel import SessionChannel
from sonara.queue import SpeechItem

# Reserved channel for GLOBAL control confirmations (pause/mute/rate/...). It is
# served ahead of every session and never announces, so a control cue is heard
# even when no real session is registered or foreground.
CONTROL = "\x00sonara-control"


class Router:
    def __init__(self, sessions, minqueue, announce_text) -> None:
        self.sessions = sessions          # exposes foreground()/folder()
        self._minqueue = minqueue          # () -> int
        self._announce_text = announce_text  # (folder, replay=False) -> str
        self.channels: "dict[str, SessionChannel]" = {}
        self.active: "str | None" = None
        self._last_active: "str | None" = None   # last session that actually read (persists across idle gaps)
        self._pending_announce: "str | None" = None
        self._pending_announce_replay = False
        # Sessions explicitly authorized to bypass the background-policy gate
        # (set by catch_up / nav cross-session replay so their replayed items
        # are voiced even when the session is not the current foreground).
        self._replay_authorized: "set[str]" = set()
        # Sessions you FORCE-switched away from (manual next_session): not
        # auto-resumed until they get NEW content. session -> len(items) when
        # suppressed; a different len (new content or a wipe) lifts it -> auto again.
        self._suppressed: "dict[str, int]" = {}

    def channel(self, session: str) -> SessionChannel:
        ch = self.channels.get(session)
        if ch is None:
            ch = SessionChannel(session)
            self.channels[session] = ch
        return ch

    def drop(self, session: str) -> None:
        self.channels.pop(session, None)
        if self.active == session:
            self.active = None
        if self._last_active == session:
            self._last_active = None
        if self._pending_announce == session:
            self._pending_announce = None
            self._pending_announce_replay = False
        self._replay_authorized.discard(session)
        self._suppressed.pop(session, None)

    def _is_suppressed(self, session: str) -> bool:
        """True if *session* was force-switched away from and has not changed since.
        New content or a wipe changes len(items) and lifts the suppression; a
        dropped channel lifts it too."""
        if session not in self._suppressed:
            return False
        ch = self.channels.get(session)
        if ch is None or len(ch.items) != self._suppressed[session]:
            self._suppressed.pop(session, None)
            return False
        return True

    def next_session(self) -> "tuple[str | None, bool]":
        """Manual session-change: a pure round-robin. Advance the active reader to
        the next session after the current one in a FIXED order (channel insertion
        order, excluding CONTROL), wrapping; with one session it lands on itself.
        A read (caught-up) target is reset to 0 and replayed (replay=True); an
        unread target resumes from its cursor (replay=False). Returns (None, False)
        only when there are no channels. Arms the session-change announcement."""
        keys = [s for s in self.channels if s != CONTROL]
        if not keys:
            return (None, False)
        old = self.active
        if self.active in keys:
            i = keys.index(self.active)
            target = keys[(i + 1) % len(keys)]     # next in the fixed ring (wraps)
        else:
            target = keys[0]
        # Force-switching AWAY from a session suppresses its auto-resume until it
        # gets new content; landing on a session (manual return) clears suppression.
        if old is not None and old != target and old in self.channels:
            self._suppressed[old] = len(self.channels[old].items)
        self._suppressed.pop(target, None)
        replay = self.channels[target].caught_up()
        if replay:
            self.channels[target].reset()
        self._arm_switch(target, replay)
        return (target, replay)

    def _arm_switch(self, target: str, replay: bool) -> None:
        self.active = target
        self._last_active = target                 # auto won't re-announce after
        self._pending_announce = target
        self._pending_announce_replay = replay

    def _ready(self, session: str) -> bool:
        ch = self.channels.get(session)
        if ch is None:
            return False
        if ch.muted:
            # A mute-exempt cue (e.g. "Session muted." / "Session unmuted.") at
            # the cursor must still be spoken even though the channel is muted.
            peeked = ch.peek()
            return peeked is not None and peeked.mute_exempt
        return ch.ready(self._minqueue())

    def _pick(self) -> "str | None":
        # Evict drained replay authorizations FIRST. The old eviction lived inside
        # the oldest-waiting loop behind _ready() (which requires pending > 0), so
        # it could never fire: a one-shot digest authorization leaked for the
        # channel's lifetime and permanently bypassed the background policy (#19).
        for s in list(self._replay_authorized):
            ch = self.channels.get(s)
            if ch is None or ch.pending() == 0:
                self._replay_authorized.discard(s)
        # decisions preempt -- even when idle (user-blocking; I1 fix).
        # Still respect the background-policy gate: earcon_only mode suppresses
        # decision TEXT for non-fg sessions (the earcon itself fires separately
        # and is always cross-session). _replay_authorized bypasses the gate.
        should_speak = getattr(self.sessions, "should_speak", None)
        for s, ch in self.channels.items():
            if not (ch.has_decision and self._ready(s)):
                continue
            if s in self._replay_authorized:           # manual replay bypasses both
                return s
            if self._is_suppressed(s):                 # force-switched-away, no update
                continue
            if should_speak is None or should_speak(s):
                return s
        # the current reader keeps the floor until its batch drains -- minqueue
        # only gates the START of reading; once started, every pending item is
        # read (unless the channel is muted with no exempt item). The active reader
        # is never a suppressed session (suppression only targets the session you
        # switched AWAY from), so no suppression check is needed here.
        if self.active is not None:
            ch = self.channels.get(self.active)
            if ch is not None and ch.pending() > 0:
                if not ch.muted or self._ready(self.active):
                    return self.active
        # foreground first, then oldest-waiting (insertion order). Both skip a
        # force-switched-away session until it gets new content (bug: a session you
        # left mid-read would auto-resume once the new reader drained).
        fg = self.sessions.foreground()
        if self._ready(fg) and not self._is_suppressed(fg):
            return fg
        # Waiting sessions are served by digest RELEASE order first (#88), then
        # insertion order. Without the stamp, three digests landing together
        # were heard in channel-creation order - effectively random vs the
        # turn-finish order the reorder buffer just established.
        best = None
        for i, s in enumerate(self.channels):
            if not self._ready(s):
                continue
            # Replay-authorized sessions bypass the policy gate AND suppression
            # (cross-session catch_up / nav replay must be voiced even in
            # earcon_only mode). Drained authorizations were already evicted
            # by the sweep at the top, so authorized here implies pending > 0.
            if s not in self._replay_authorized:
                if self._is_suppressed(s):
                    continue
                if should_speak is not None and not should_speak(s):
                    continue
            stamp = self.channels[s].release_order
            key = (stamp if stamp is not None else float("inf"), i)
            if best is None or key < best[0]:
                best = (key, s)
        return best[1] if best is not None else None

    def next_item(self) -> "SpeechItem | None":
        # emit a queued hand-off announcement before the new reader's first item
        if self._pending_announce is not None:
            folder = self.sessions.folder(self._pending_announce) or "another session"
            text = self._announce_text(folder, self._pending_announce_replay)
            self._pending_announce = None
            self._pending_announce_replay = False
            # kind "session_change" lets the speak loop fire the session-switch
            # earcon (chime) just before voicing the announcement. NOT mute_exempt:
            # global mute silences hand-offs too (both the chime and the spoken
            # announcement) -- mute means silence.
            return SpeechItem(id=0, session=self.active or "", kind="session_change",
                              text=text, is_decision=False, mute_exempt=False)
        # Global control cues (pause/mute/rate confirmations) are served ahead of
        # every session and never announce or change _last_active -- so they are
        # heard even when no session is registered/foreground.
        ctrl = self.channels.get(CONTROL)
        if ctrl is not None and ctrl.pending() > 0:
            return ctrl.next()
        target = self._pick()
        if target is None:
            self.active = None
            return None
        if target != self.active:
            self.active = target
            # Announce a REAL handoff (auto only): switching to a session different
            # from the LAST one that read. The first-ever reader (no prior
            # _last_active) does not announce -> single session never announces;
            # returning to the same session after an idle gap does not announce.
            if (self._last_active is not None
                    and target != self._last_active):
                self._pending_announce = target
                self._pending_announce_replay = False
                self._last_active = target
                return self.next_item()
            self._last_active = target
        return self.channels[target].next()
