"""The speak loop: takes the next item from the router and speaks it, holds
while paused (voicing only pause-exempt cues), drops non-exempt items while
muted, signals a failed utterance audibly (#41), and plays a deferred
session-change alert as a preamble to the content it announces (#94).

The loop runs on its own thread and takes the daemon lock around every read
of shared state; speaking itself runs off-lock.

SpeakLoop keeps the daemon's config, router, speaker, cues, audio control,
shared state and heard-marker map by reference, so the daemon must not
rebind those attributes after construction."""
from __future__ import annotations

import sys

from sonara import config_schema


class SpeakLoop:
    """Owns the speak loop and the deferred handoff alert. Given the daemon's
    shared state explicitly: *running*, *paused* and *wake* are the daemon's
    events, *shared* the core.SharedState (the item being spoken),
    *pending_heard* the item id -> history entry map. *muted* and *earcon*
    are the daemon's mute check and earcon path; *note_spoken* and
    *requeue_or_note* its heard-marker bookkeeping. *on_change* publishes
    the state stream (#143), called with the lock held when an utterance
    starts and after every loop iteration."""

    def __init__(self, config, lock, wake, running, paused, router, speaker,
                 cues, audio, shared, pending_heard, muted, earcon,
                 note_spoken, requeue_or_note, on_change=None) -> None:
        self._config = config
        self._lock = lock
        self._wake = wake
        self._running = running
        self._paused = paused
        self._router = router
        self._speaker = speaker
        self._cues = cues
        self._audio = audio
        self._shared = shared
        self._pending_heard = pending_heard
        self._muted = muted
        self._earcon = earcon
        self._note_spoken = note_spoken
        self._requeue_or_note = requeue_or_note
        self._on_change = on_change if on_change is not None else (lambda: None)
        self.poll_interval = 0.1
        self.pending_preamble = None   # (session, alert_text) deferred to content on_play (#94)

    def run(self) -> None:
        self._running.set()
        while self._running.is_set():
            try:
                self.run_once()
            except Exception:  # noqa: BLE001 - NOTHING may permanently kill the
                # speak thread. A crash in next_item/note_spoken/etc. used to leave
                # the daemon alive (earcons kept firing) but mute forever until a
                # restart. Log the traceback (captured by the daemon log) and keep
                # going; a short wait avoids a tight error-spin.
                import traceback
                traceback.print_exc(file=sys.stderr)
                self._wake.wait(0.1)

    def signal_failure(self) -> None:
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
            import traceback
            traceback.print_exc(file=sys.stderr)
        except Exception:  # noqa: BLE001 - logging failure must not wedge the loop
            pass

    def run_once(self) -> None:
        """One iteration of the speak loop. May raise; run() contains it.
        Ends by publishing the state: an utterance ended, or a digest
        landed while the loop was idle."""
        try:
            self._run_once()
        finally:
            with self._lock:
                self._on_change()

    def _run_once(self) -> None:
        if self._paused.is_set():
            # Idempotently restore other apps' audio while paused -- closes the window
            # where a re-duck slipped in during the pause transition. Safe to call
            # repeatedly: AudioControl.restore() is a no-op when not ducked/paused.
            self._audio.restore()
            # While paused, still drain a single pause_exempt cue (e.g. "Paused.")
            # before holding. Scan ALL channels at/after their cursor: a mid-utterance
            # pause rewinds the cursor past where Cues.speak inserted the cue, so a
            # plain peek() at the cursor would miss it.
            with self._lock:
                item = None
                for ch in self._router.channels.values():
                    item = ch.take_pause_exempt()
                    if item is not None:
                        self._shared.current_item = item
                        self._on_change()       # speech starts (#143)
                        break
                cancel_epoch = self._speaker.cancel_epoch()
            if item is not None:
                try:
                    completed = self._speaker.speak(item.text, cancel_epoch=cancel_epoch,
                                                    **self._cues.cue_voice_override(item))
                except Exception:  # noqa: BLE001
                    self.signal_failure()
                    completed = False
                self._note_spoken(item, completed)
                return
            self._wake.wait(self.poll_interval)
            self._wake.clear()
            return
        with self._lock:
            item = self._router.next_item()
            self._shared.current_item = item
            cancel_epoch = self._speaker.cancel_epoch()
            # Global mute: drop every non-exempt item (the "Muted."/"Unmuted." cue
            # is mute_exempt so it is still heard). The router already advanced the
            # cursor, so a dropped item is consumed, not replayed.
            muted = (item is not None and self._muted() and not item.mute_exempt)
            if muted:
                self._shared.current_item = None
                self._pending_heard.pop(item.id, None)
                print("[mute] dropped: {0!r}".format((item.text or "")[:60]),
                      file=sys.stderr, flush=True)
            elif item is not None and item.kind != "session_change":
                self._on_change()               # speech starts (#143)
            # Engine fallback notices: spoken once per daemon run so an
            # eyes-free user knows WHY the voice changed (the reason is
            # already in the log). Inside this locked block like every cue:
            # Cues.speak reslices CONTROL and allocates an item id (#155).
            self._cues.maybe_announce_kokoro_fallback()
        if item is None:
            self._audio.restore()
            self._wake.wait(self.poll_interval)
            self._wake.clear()
            return
        if muted:
            # A dropped item also drops a pending alert for its session: mute
            # silences handoffs, so the deferred chime + announcement go too.
            self.drop_preamble_for(item.session)
            return
        if item.kind == "session_change":
            if item.manual:
                # Manual switch (#111): the press already chimed from the hotkey
                # handler; speak the announcement NOW in the fast cue voice
                # instead of deferring to the target content's synthesis-ready
                # callback. Deferral (#94) exists so an AUTO handoff's alert
                # doesn't play seconds before slow-engine audio; a manual press
                # needs immediate confirmation, and the cue voice is warm.
                try:
                    completed = self._speaker.speak(item.text,
                                                    cancel_epoch=cancel_epoch,
                                                    **self._cues.cue_voice_override(item))
                except Exception:  # noqa: BLE001
                    self.signal_failure()
                    completed = False
                if not self._requeue_or_note(item, completed):
                    self._note_spoken(item, completed)
                return
            if config_schema.get(self._config, "fast_cues"):
                # Defer the alert (#94): stash it and play the chime + spoken
                # announcement from the CONTENT utterance's on_play, so a slow
                # engine no longer plays the alert seconds before the audio.
                self.pending_preamble = (item.session, item.text)
                self._shared.current_item = None
                return
            # fast_cues off: legacy immediate announcement in the content voice.
            try:
                self._earcon("session_change")
            except Exception:  # noqa: BLE001
                pass
            try:
                completed = self._speaker.speak(item.text, cancel_epoch=cancel_epoch,
                                                on_play=None,
                                                **self._cues.voice_override(item))
            except Exception:  # noqa: BLE001
                self.signal_failure()
                completed = False
            if not self._requeue_or_note(item, completed):
                self._note_spoken(item, completed)
            return
        # Content item. A stashed alert for THIS session plays as a preamble at
        # synthesis-ready (on_play): chime, then the spoken alert via the fast cue
        # voice (non-tracked so it never clobbers this utterance's cancellation),
        # then the normal duck/pause engage. #90's "announcement never ducks" is
        # preserved: the alert cue itself is played WITHOUT on_play, and the duck/
        # pause engage happens for the CONTENT, after the alert.
        # Read the alert ONCE under the lock (L-preamble): on_play and
        # flush_all (daemon/controls) clear it from other threads, and a None landing between
        # a check and the subscript raised TypeError in the speak loop.
        preamble = None
        with self._lock:
            pending = self.pending_preamble
            if pending is not None:
                if pending[0] == item.session:
                    preamble = pending[1]
                else:
                    self.pending_preamble = None   # stale alert for another session: drop it
        if preamble is not None:
            cue_voice = self._cues.cue_voice()
            rate = config_schema.get(self._config, "rate")

            def on_play(_text=preamble, _voice=cue_voice, _rate=rate):
                # Consume the alert only when it actually plays. If content synthesis
                # is interrupted (e.g. paused) BEFORE on_play fires, the preamble stays
                # armed so the replayed content still announces the handoff (#94).
                self.pending_preamble = None
                try:
                    self._earcon("session_change")
                except Exception:  # noqa: BLE001
                    pass
                try:
                    self._speaker.speak_cue_untracked(_text, _voice, _rate)
                except Exception:  # noqa: BLE001
                    pass
                self._audio.engage()
        else:
            on_play = self._audio.engage
        try:
            completed = self._speaker.speak(item.text, cancel_epoch=cancel_epoch,
                                            on_play=on_play,
                                            **self._cues.voice_override(item))
        except Exception:  # noqa: BLE001
            self.signal_failure()
            completed = False
        if not self._requeue_or_note(item, completed):
            self._note_spoken(item, completed)
            # A deferred alert that never played is kept armed only when the content
            # was requeued for replay (a pause, handled by _requeue_or_note above).
            # Here the content was noted, not requeued (completed, or a non-pause
            # cancel dropped it), so drop any still-armed alert for this session -
            # otherwise it would resurface on a later utterance for the same session
            # (#94). If on_play already played the alert, pending_preamble is None
            # and this is a no-op.
            self.drop_preamble_for(item.session)

    def drop_preamble_for(self, session) -> None:
        """Drop a deferred handoff alert armed for *session*. Check-then-act
        under the lock (L-preamble): on_play clears it from the synth thread."""
        with self._lock:
            pending = self.pending_preamble
            if pending is not None and pending[0] == session:
                self.pending_preamble = None
