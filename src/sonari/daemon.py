from __future__ import annotations

import os
import secrets
import socket
import subprocess
import threading

from sonari.protocol import MsgType, encode, decode
from sonari.queue import SpeechItem
from sonari.assembler import ProseAssembler
from sonari.config import save_config, load_config
from sonari.paths import (
    LOCK_PATH, SINGLETON_PATH, ensure_sonari_dir, socket_connectable,
    INSTALL_RECORD_PATH,
)
from sonari.platform import transport

# Holds the single-instance flock for this process's lifetime (see main()).
_SINGLETON = None


RATE_MIN = 100
RATE_MAX = 400

# Min-queue batching: how many prose items must accumulate before they are read.
# 1 == read each item as it arrives (the default, unchanged behaviour).
MINQUEUE_MIN = 1
MINQUEUE_MAX = 10

# Cap on concurrent connection-handler threads. Legitimate clients are short-lived
# (one request each), so this bound is generous; it just stops a misbehaving or
# hostile peer from leaking unbounded threads by opening many connections.
_MAX_CONN_THREADS = 32


class SpeechDaemon:
    def __init__(self, speaker, sessions, config) -> None:
        self.speaker = speaker
        self.sessions = sessions
        self.config = config
        self._assemblers = {}
        self._next_id = 0
        from sonari.router import Router
        self.router = Router(
            self.sessions,
            minqueue=self._minqueue,
            announce_text=lambda folder: "Session changed: {0}.".format(folder),
        )
        self._running = threading.Event()
        self._wake = threading.Event()
        self._lock = threading.Lock()
        self._server = None
        self._token = None
        self._poll_interval = 0.1
        from sonari.history import SessionHistory
        self.history = SessionHistory(cap=int(config.get("history_cap", 200)))
        self._options: "dict[str, str]" = {}
        self._pending_heard: dict = {}            # SpeechItem.id -> HistoryEntry
        self._nav_cursor: dict = {}               # session -> anchored message id (absent = latest)
        self._paused = threading.Event()          # play/pause: set == speech halted
        self._muted = False                       # global mute: drop all prose/cues
        self._current_item = None                 # item being spoken right now
        self._warned_immediate: set = set()
        self._guided_sessions: set = set()
        self._conn_sem = threading.BoundedSemaphore(_MAX_CONN_THREADS)
        self._reload_lock = threading.Lock()      # serializes off-lock hotkey reloads

    def _alloc_id(self) -> int:
        self._next_id += 1
        return self._next_id

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
        (no launchctl) and never raises.
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

    def note_spoken(self, item, completed: bool) -> None:
        """Speak-loop bookkeeping: confirm (or decline) the heard-marker for a
        finished utterance."""
        with self._lock:
            self._current_item = None
            entry = self._pending_heard.pop(item.id, None)
            if entry is not None and completed:
                entry.heard = True

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
        """Delegating shim — logic lives in the platform supervisor backend."""
        from sonari.platform import get_platform
        return get_platform().supervisor.is_installed()

    def _setup_health(self, plugin_version: str):
        """Return (state, cue) where state is one of:
        "ok"            -> fully installed, no version drift   -> cue None
        "not_installed" -> no install.json or launcher (never ran `sonari install`)
        "version_drift" -> installed but plugin_version differs from this session's

        Cheap: a few file stats + a string compare. No launchctl. Never raises.
        The hotkeyd binary is deliberately NOT part of this check so a deliberate
        speech-only user (no swiftc) is never nagged.
        """
        rec = self._read_install_record()
        installed = (rec is not None and self._launcher_present())
        if not installed:
            return ("not_installed",
                    "Sonari is reading aloud. To enable hotkeys and autostart, "
                    "run, slash sonari install.")
        recorded = (rec.get("plugin_version") or "")
        # Only flag drift when BOTH sides are known and differ.
        if plugin_version and recorded and plugin_version != recorded:
            return ("version_drift",
                    "Sonari was updated. Run, slash sonari install, to apply.")
        return ("ok", None)

    def handle_message(self, msg):
        t = msg.get("type")
        session = msg.get("session", "")
        verbosity = self.config.get("verbosity", "everything")

        if t == MsgType.PROSE:
            final = msg.get("final", False)
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), final)
            from sonari.assembler import PARAGRAPH_BREAK
            ch = self.router.channel(session)
            for chunk in chunks:
                if chunk is PARAGRAPH_BREAK:
                    self.history.end_message(session)
                    continue
                entry = self.history.record(session, "prose", chunk)
                # At "quiet" verbosity prose is recorded in history (for catch_up)
                # but NOT enqueued for speech.
                if verbosity != "quiet":
                    item = SpeechItem(id=self._alloc_id(), session=session, kind="prose",
                                      text=chunk, is_decision=False)
                    self._pending_heard[item.id] = entry
                    ch.append(item)
            if final:
                # NOTE: turn_done is NOT set here — a per-block "final" flag means
                # this text block finished, but the TURN ends only when the
                # turn_done earcon (or FLUSH) arrives. This keeps minqueue batching
                # correct: items accumulate until the threshold OR the turn ends.
                self.history.end_message(session)
                self._options.pop(session, None)
            self._wake.set()
            return None

        # Decision CONTENT is enqueued (and gated by foreground). The ALERT
        # earcon for a decision travels as a SEPARATE EARCON message that
        # hooks_entry emits BEFORE the content message; it is handled by the
        # MsgType.EARCON branch below, so the earcon fires instantly and
        # cross-session WITHOUT being doubled here.
        if t == MsgType.CHOICE:
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
            self.router.channel(session).append(item)
            self._wake.set()
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
            self.router.channel(session).append(item)
            self._wake.set()
            return None

        if t == MsgType.PERMISSION:
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
            self.router.channel(session).append(item)
            self._wake.set()
            return None

        if t == MsgType.TOOL:
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
            self.speaker.earcon(kind)
            if kind == "turn_done":
                # End-of-turn boundary: safety-net flush in case the final PROSE
                # flag never arrived.
                self.router.channel(session).turn_done = True
            return None

        if t == MsgType.FLUSH:
            cur = self._current_item
            if cur is not None and cur.session == session:
                self.speaker.cancel()
            self._drop_channel_pending(session)
            self.router.channel(session).wipe()
            self._assemblers.pop(session, None)
            self.history.reset(session)
            self._nav_cursor.pop(session, None)
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
            self._drop_channel_pending(session)
            self.router.drop(session)
            self.history.reset(session)
            self._options.pop(session, None)
            self._warned_immediate.discard(session)
            self._guided_sessions.discard(session)
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
            fg = self.sessions.foreground()
            if fg is None:
                return None
            self._nav(fg, msg.get("to", "prev"))
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
                if target is not None:
                    self._speak_cue(target, "Resumed.", exempt_mute=True)
            else:
                self._paused.set()
                # cancel() bumps the speaker's epoch so even an in-progress
                # utterance aborts. The speak loop re-queues the interrupted item
                # (sees completed=False while paused), so we don't capture it here.
                self.speaker.cancel()
                # "Paused." is pause_exempt so the paused branch of the speak loop
                # scans for and voices it while holding everything else (bypassing
                # the normal mute gate). mute_exempt is NOT needed here for that
                # reason; we omit it to keep the flag truthful.
                if target is not None:
                    self._speak_cue(target, "Paused.", pause_exempt=True)
            return None

        if t == MsgType.MUTE:
            # Global mute toggle: silence ALL sessions at once. Earcons still fire
            # (alerts), and the "Muted."/"Unmuted." confirmation is always heard
            # (mute_exempt). The speak loop drops every non-exempt item while muted.
            self._muted = not self._muted
            if self._muted:
                self.speaker.cancel()           # stop the current utterance now
            target = self.router.active or self.sessions.foreground()
            if target is not None:
                self._speak_cue(target, "Muted." if self._muted else "Unmuted.",
                                exempt_mute=True)
            self._wake.set()
            return None

        if t == MsgType.PIN_TOGGLE:
            # Pin the voice to the current (last-prompt) session, or unpin it.
            # The pin overrides "foreground", so a later SET_FOREGROUND from another
            # session can't steal the voice. Confirmation is mute_exempt so the user
            # always hears it; the no-session case has nothing to speak through, so
            # it is an error earcon only.
            action, folder = self.sessions.pin_toggle()
            if action == "none":
                self.speaker.earcon("error")
                return None
            fg = self.sessions.foreground()
            if action == "pinned":
                self.router.repin_reset()
                text = "Pinned {0}.".format(folder) if folder else "Pinned."
            else:
                text = "Auto."
            self._enqueue(fg, "prose", text, False, mute_exempt=True)
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
                             name="sonari-keymap-reload", daemon=True).start()
            return None

        if t == MsgType.REPEAT:
            fg = self.sessions.foreground()
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
            fg = self.sessions.foreground()
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
            # Advance the foreground channel cursor to the next decision item.
            fg = self.sessions.foreground()
            if fg is not None:
                ch = self.router.channel(fg)
                while ch.cursor < len(ch.items) and not ch.items[ch.cursor].is_decision:
                    skipped = ch.items[ch.cursor]
                    self._pending_heard.pop(skipped.id, None)
                    ch.cursor += 1
            self.speaker.cancel()
            return None

        if t == MsgType.CATCH_UP:
            fg = self.sessions.foreground()
            if fg is None:
                return None
            target = fg
            entries = self.history.unheard(fg)
            preamble = None
            if not entries:
                other = self.history.other_session_with_unheard(fg)
                if other is not None:
                    target = other
                    entries = self.history.unheard(other)
                    preamble = "Catching up on another session."
            if not entries:
                self._speak_cue(fg, "You're all caught up.")
                return None
            # Replay cleanly: cut the target's current utterance (it stays
            # unheard, so it replays FROM ITS START) and drop its queued
            # duplicates — every unheard entry is re-replayed in order below.
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
                # Validate/clamp the absolute rate just like the delta branch — an
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
            return None

        if t == MsgType.SET_VERBOSITY:
            self.config["verbosity"] = msg.get("verbosity")
            save_config(self.config)
            return None

        if t == MsgType.SET_MINQUEUE:
            # Validate/clamp before persisting — a bad value reaches disk and would
            # wedge prose buffering on every turn (mirrors the SET_RATE guard).
            try:
                n = max(MINQUEUE_MIN, min(MINQUEUE_MAX, int(msg.get("minqueue"))))
            except (TypeError, ValueError):
                return None
            self.config["minqueue"] = n
            save_config(self.config)
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
            }

        if t == MsgType.PING:
            return {"ok": True}

        return None

    def stop(self) -> None:
        self._running.clear()
        self._wake.set()
        self._stop_hotkeys()
        srv = self._server
        if srv is not None:
            try:
                srv.close()
            except OSError:
                pass

    def _start_hotkeys(self) -> None:
        """Start the platform's global-hotkey listener. On Windows this spawns an
        in-process RegisterHotKey thread; on macOS it is a no-op (the hotkeyd is a
        separate process)."""
        # Kill-switch: a ~/.sonari/no_hotkeys file (or SONARI_DISABLE_HOTKEYS=1)
        # runs speech-only (no in-process hotkey thread). A FILE flag is honoured
        # by EVERY daemon however it is spawned (hooks inherit their own env, not
        # ours), so it reliably isolates the hotkey thread when diagnosing crashes.
        flag = os.path.join(os.path.expanduser("~"), ".sonari", "no_hotkeys")
        if os.environ.get("SONARI_DISABLE_HOTKEYS") or os.path.exists(flag):
            return
        from sonari.platform import get_platform
        try:
            get_platform().hotkey.start(self._dispatch_hotkey)
        except Exception:  # noqa: BLE001 - hotkeys are non-essential; speech must run
            pass

    def _stop_hotkeys(self) -> None:
        from sonari.platform import get_platform
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
            flag = os.path.join(os.path.expanduser("~"), ".sonari", "no_hotkeys")
            if os.environ.get("SONARI_DISABLE_HOTKEYS") or os.path.exists(flag):
                self._stop_hotkeys()
                return
            from sonari.platform import get_platform
            try:
                get_platform().hotkey.reload(self._dispatch_hotkey)
            except Exception:  # noqa: BLE001 - hotkeys are non-essential; speech must run
                pass

    def _replay(self, session: str, entries) -> None:
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
        at = ch.cursor
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
        if at > ch.cursor:  # only if we actually inserted items
            # Mark channel ready: replayed items should be spoken without
            # waiting for minqueue threshold.
            ch.turn_done = True
            # Suppress the "Session changed" auto-announce for programmatic
            # replay (catch_up/nav/repeat): the handoff is not user-visible.
            self.router._last_active = session
            # Authorize cross-session reading: replay targets that are not the
            # current fg bypass the background-policy gate so their replayed
            # items are voiced (catch_up / nav cross-session scenarios).
            fg = self.sessions.foreground()
            if session != fg:
                self.router._replay_authorized.add(session)
        self._wake.set()

    def _nav(self, session: str, to: str) -> None:
        """Move the per-session message cursor and play from there to the end.

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
            return
        n = len(ids)
        # Anchor on a STABLE message id, not a position: new paragraphs streaming
        # in append ids without shifting where the cursor points. Unset/stale ->
        # the latest. The cursor only clears on a new prompt (FLUSH).
        cur_id = self._nav_cursor.get(session)
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
            return
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
        # after these and continues seamlessly — no jump from replay into live.
        entries = []
        for mid in ids[new:]:
            entries.extend(self.history.entries_for_message(session, mid))
        self._replay(session, entries)

    def _resume(self) -> None:
        """Clear pause and wake the speak loop. The interrupted utterance was
        already re-queued at the front by the speak loop when its speak() returned
        not-completed during the pause, so resume picks back up where it stopped."""
        self._paused.clear()
        self._wake.set()

    def _dispatch_hotkey(self, message: dict) -> None:
        """A hotkey fire is handled exactly like an inbound socket message.

        MUST hold self._lock around handle_message, identical to the socket path
        (_handle_conn): the hotkey thread mutates shared state (queue, history,
        config) concurrently with the speak loop, so without the lock it races
        -> 'list changed size during iteration' / corruption. handle_message and
        its callees never acquire self._lock (note_spoken/speak run on the speak
        thread), so this is deadlock-free. An enqueue-based action (repeat /
        skip_back / catch_up) is likewise safe from losing its item to that race.
        """
        try:
            with self._lock:
                self.handle_message(message)
        except Exception:  # noqa: BLE001 - one bad hotkey must not kill the pump
            pass

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
        loop — but for an eyes-free user a swallowed exception is a SILENT no-op,
        the worst outcome (#41). Signal it audibly (error earcon) and log the
        traceback. Never raises — error signaling must not itself re-break the
        loop. Call only from within an active `except` block (print_exc reads the
        handled exception)."""
        try:
            self.speaker.earcon("error")
        except Exception:  # noqa: BLE001 - signaling failure must not wedge the loop
            pass
        try:
            import sys
            import traceback
            traceback.print_exc(file=sys.stderr)
        except Exception:  # noqa: BLE001 - logging failure must not wedge the loop
            pass

    def _speak_cue(self, session: str, text: str, exempt_mute: bool = False,
                   pause_exempt: bool = False) -> None:
        """Insert a one-off confirmation cue at the active read position so it is
        always heard (before any normal queued items). mute_exempt/pause_exempt
        flags ensure it plays through holds and mutes."""
        item = SpeechItem(id=self._alloc_id(), session=session, kind="prose",
                          text=text, is_decision=False, mute_exempt=exempt_mute,
                          pause_exempt=pause_exempt)
        ch = self.router.channel(session)
        ch.items.insert(ch.cursor, item)   # speak next, then continue
        self._wake.set()

    def _speak_loop_once(self) -> None:
        """One iteration of the speak loop. May raise; _speak_loop contains it."""
        if self._paused.is_set():
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
                    completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch)
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
        if item is None:
            self._wake.wait(self._poll_interval)
            self._wake.clear()
            return
        if muted:
            return
        if item.kind == "session_change":
            # Play the session-switch chime (it mixes with the spoken announcement
            # on a separate audio path). A missing chime must not wedge the loop.
            try:
                self.speaker.earcon("session_change")
            except Exception:  # noqa: BLE001
                pass
        try:
            completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch)
        except Exception:  # noqa: BLE001
            self._signal_speak_failure()
            completed = False
        requeued = False
        with self._lock:
            if not completed and self._paused.is_set():
                # paused mid-utterance: rewind the cursor so resume re-speaks it
                ch = self.router.channels.get(item.session)
                if ch is not None and ch.cursor > 0:
                    ch.cursor -= 1
                self._current_item = None
                requeued = True
        if not requeued:
            self.note_spoken(item, completed)

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
        srv = self._server
        while self._running.is_set():
            try:
                conn, _ = srv.accept()
            except OSError:
                return
            self._spawn_conn_handler(conn)

    def run(self) -> None:
        ensure_sonari_dir()
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind((transport.HOST, 0))
        srv.listen(16)
        port = srv.getsockname()[1]
        self._token = secrets.token_hex(32)
        transport.write_lockfile(
            LOCK_PATH, transport.HOST, port, self._token, os.getpid())
        self._server = srv
        self._running.set()

        speak_thread = threading.Thread(target=self._speak_loop, daemon=True)
        accept_thread = threading.Thread(target=self._accept_loop, daemon=True)
        speak_thread.start()
        accept_thread.start()
        self._start_hotkeys()

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
    if socket_connectable():
        return
    from sonari.platform import get_platform
    argv, kwargs = get_platform().supervisor.launch_spec()
    subprocess.Popen(argv, **kwargs)


_FAULT_FILE = None


def _arm_faulthandler() -> None:
    """Dump every thread's Python stack to SONARI_DIR/faulthandler.log on a NATIVE
    crash (access violation / segfault in WinRT, ctypes, or winsound) — the only
    way to see otherwise-silent C-level daemon deaths. Never raises."""
    global _FAULT_FILE
    try:
        import faulthandler
        # Import SONARI_DIR LIVE (not at module top) so the conftest monkeypatch /
        # any SONARI_DIR redirection takes effect; a top-level import would freeze
        # the value before tests patch it and leak into the real ~/.sonari.
        from sonari.paths import SONARI_DIR
        path = str(SONARI_DIR / "faulthandler.log")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        # mode 'w': only the latest run's crash matters; never grow unbounded.
        _FAULT_FILE = open(path, "w", encoding="utf-8")
        _FAULT_FILE.write("=== faulthandler armed: pid {0} ===\n".format(os.getpid()))
        _FAULT_FILE.flush()
        faulthandler.enable(file=_FAULT_FILE, all_threads=True)
    except Exception:  # noqa: BLE001 - diagnostics must never break startup
        pass


def main() -> None:
    _arm_faulthandler()
    # Single-instance guard. The fast path avoids work when a daemon is clearly
    # already serving. The AUTHORITATIVE guard is the exclusive flock below:
    # with an ephemeral TCP port, bind() never collides (unlike the old fixed
    # AF_UNIX path), so socket_connectable() alone is racy and lets concurrent
    # lazy-starts each bind their own port -> a daemon explosion. The flock lets
    # exactly one process win; the rest exit. The lock auto-releases on death.
    global _SINGLETON
    if socket_connectable():
        return
    ensure_sonari_dir()
    _SINGLETON = transport.acquire_singleton(SINGLETON_PATH)
    if _SINGLETON is None:
        return  # another daemon already owns the single-instance lock

    from sonari.speaker import Speaker
    from sonari.sessions import SessionManager
    from sonari.platform import get_platform

    _backend = get_platform()
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
    sessions = SessionManager(background_policy=cfg.get("background_policy", "earcon_only"))
    daemon = SpeechDaemon(speaker, sessions, cfg)
    daemon.run()


if __name__ == "__main__":
    main()

