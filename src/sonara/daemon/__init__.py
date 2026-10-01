from __future__ import annotations

import os
import socket
import sys
import threading

from sonara.protocol import MsgType
from sonara.queue import SpeechItem
from sonara import config_schema
from sonara.daemon import setup_health, tokens
from sonara.daemon.audio import AudioControl
from sonara.daemon.controls import Controls
from sonara.daemon import core
from sonara.daemon.core import SessionRegistry, SharedState
from sonara.daemon.cues import Cues
from sonara.daemon.hotkeys import HotkeyController
from sonara.daemon.ingest import Ingest
from sonara.daemon.playback import SpeakLoop
from sonara.daemon.server import ConnectionServer
from sonara.daemon.settings import Settings
from sonara.daemon import state_stream
from sonara.daemon.summary.pipeline import SummaryPipeline, summary_log
from sonara.daemon.summary.reorder import DigestReorderBuffer
from sonara.config import save_config
from sonara.paths import LOCK_PATH, ensure_sonara_dir, package_root
from sonara.platform import transport
# Re-exported: ensure_running moved to sonara.lifecycle (#141) so clients
# start the daemon without importing it.
from sonara.lifecycle import ensure_running  # noqa: F401

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
            from sonara.platform.base import NullDucker
            ducker = NullDucker()
        self.ducker = ducker
        if pauser is None:
            from sonara.platform.base import NullPauser
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
        # The state stream for embedded players (#143): STATUS and SUBSCRIBE
        # read its snapshot; it is published after every handled message and
        # around every utterance.
        self._state = state_stream.StateStream(
            lambda: state_stream.snapshot(self))
        # The loopback socket: accept loop, token check, handler threads.
        # handle_message is looked up at call time, as for the hotkeys.
        self._server = ConnectionServer(
            self._running, self._lock, lambda m: self.handle_message(m),
            stream=self._state)
        self._webui = None
        from sonara.history import SessionHistory
        self.history = SessionHistory(cap=int(config_schema.get(config, "history_cap")))
        self._pending_heard: dict = {}            # SpeechItem.id -> HistoryEntry
        # Shared by several features (#141): the item being spoken and the
        # per-session text summary-mode Up re-reads verbatim.
        self._shared = SharedState()
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
        # Ducking / media pause around speech, and the speech volume
        # (_persist saves through this module's save_config).
        self._audio = AudioControl(
            config, self.ducker, self.pauser, speaker,
            persist=self._persist, cues=self._cues,
            cue_target=lambda: self.router.active or self.sessions.foreground(),
            wake=self._wake)
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
                item, completed),
            on_change=self._publish_state)
        # Per-session state (#141): ending or forgetting a session clears
        # everything registered here (_teardown_session).
        self._session_state = SessionRegistry()
        reg = self._session_state
        reg.register_hook("pending_heard", self._drop_channel_pending)
        reg.register_hook("history", self.history.reset)
        reg.register_hook("setup_guide", self._setup_guide.forget)
        # Ending the session is a user action like FLUSH: cancel its summary
        # work (bumps the cancel epoch and the settle generation, drops held
        # and deferred questions). A late worker must find no held question
        # to append (zombie channel), and nothing here may outlive the
        # session (audit #21).
        reg.register_hook("summary", self._summary.end_session)
        reg.register("last_digest_text", self._last_digest_text)
        # Ended sessions don't rehydrate (#118).
        reg.register_hook("digest_store", self.digest_store.forget)
        # Hook traffic: prose, decisions, earcons, FLUSH, session lifecycle
        # (daemon/ingest). Registers its own per-session state.
        self._ingest = Ingest(self, reg)
        # Message dispatch (#141): each feature registers the message types
        # it owns; handle_message looks the handler up by type.
        self._handlers: dict = {}
        self._ingest.register(self._handlers)
        # Pause, mute, skip, stop, session switch, flush to end, Up, repeat
        # (daemon/controls).
        self._controls = Controls(self)
        self._controls.register(self._handlers)
        # Rate, voice, verbosity, minqueue, summary mode, session prefs,
        # STATUS and the settings page setters (daemon/settings).
        self._settings = Settings(self)
        self._settings.register(self._handlers)
        self._audio.register(self._handlers)
        self._hotkeys.register(self._handlers)
        core.add_handlers(self._handlers, {
            MsgType.SHUTDOWN: self._on_shutdown,
            MsgType.PING: lambda msg: {"ok": True},
            # Served by the socket server (daemon/server); reaching here
            # (hotkey, settings page) there is no connection to stream to.
            MsgType.SUBSCRIBE: lambda msg: None,
        })

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

    def _persist(self) -> None:
        """Save the config. save_config resolves on this module at call time
        (tests patch sonara.daemon.save_config)."""
        save_config(self.config)

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

    def handle_message(self, msg):
        t = msg.get("type")
        session = msg.get("session", "")
        # Liveness for the Sessions tab: any session-bearing hook traffic
        # counts as activity. Settings-page mutations are excluded, or naming
        # a stale row would bump it back into the recent list.
        if (isinstance(session, str) and session
                and t not in (MsgType.SET_SESSION_PREF, MsgType.FORGET_SESSION)):
            self.sessions.touch(session)
        # Table dispatch (#141): unknown types get no reply. A malformed
        # (unhashable) type is unknown, not a crash.
        handler = self._handlers.get(t) if isinstance(t, str) else None
        if handler is None:
            return None
        reply = handler(msg)
        self._publish_state()
        return reply

    def _publish_state(self) -> None:
        """Push the state to subscribers if it changed (#143). Caller holds
        the daemon lock. Contained: the stream must never break a message
        or the speak loop."""
        try:
            self._state.publish()
        except Exception:  # noqa: BLE001 - a stream bug must not drop speech
            import traceback
            traceback.print_exc(file=sys.stderr)

    def _on_shutdown(self, msg):
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

    def stop(self) -> None:
        self._running.clear()
        self._wake.set()
        self._hotkeys.stop_worker()     # unblock the hotkey worker's get() to exit
        self._audio.restore()       # never leave other apps' audio ducked or paused
        self._hotkeys.stop()
        if getattr(self, "_webui", None) is not None:
            self._webui.stop()
        self._state.close_all()         # end the subscriber threads (#143)
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

    def _maybe_prewarm_cue_voice(self) -> None:
        """Live-apply hook for the cue voice keys (config_schema apply):
        warm the Kokoro engine for cues (#60, see Cues.maybe_prewarm)."""
        self._cues.maybe_prewarm()

    def set_config_value(self, key: str, value) -> bool:
        """Settings page setter for config-only keys (daemon/settings)."""
        return self._settings.set_config_value(key, value)

    def set_summary_prompt(self, style, text) -> bool:
        """Settings page setter for a custom summarizer instruction
        (daemon/settings)."""
        return self._settings.set_summary_prompt(style, text)

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


# The process entry point lives in daemon/startup (#141); re-exported so
# `python -m sonara.daemon` and the CLI keep calling sonara.daemon.main.
from sonara.daemon.startup import main, resolve_earcons  # noqa: E402,F401
