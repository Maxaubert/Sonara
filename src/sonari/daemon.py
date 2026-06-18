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

# Cap on concurrent connection-handler threads. Legitimate clients are short-lived
# (one request each), so this bound is generous; it just stops a misbehaving or
# hostile peer from leaking unbounded threads by opening many connections.
_MAX_CONN_THREADS = 32


class SpeechDaemon:
    def __init__(self, queue, speaker, sessions, config) -> None:
        self.queue = queue
        self.speaker = speaker
        self.sessions = sessions
        self.config = config
        self._assemblers = {}
        self._next_id = 0
        self._running = threading.Event()
        self._wake = threading.Event()
        self._lock = threading.Lock()
        self._server = None
        self._token = None
        self._poll_interval = 0.1
        from sonari.history import SessionHistory
        self.history = SessionHistory(cap=int(config.get("history_cap", 200)))
        self._options: "dict[str, str]" = {}
        self._voice_owner: "str | None" = None
        self._captured_msg: "set[str]" = set()
        # Sessions with an assistant PROSE message currently streaming (between the
        # first non-final delta and the turn boundary). While the owner has an open
        # message we DON'T release the voice on a transient queue drain — between
        # chunks of one reply the deque routinely hits 0, and releasing there let a
        # second session steal the voice and silence the rest of the reply (H1).
        self._open_msg: "set[str]" = set()
        self._pending_heard: dict = {}            # SpeechItem.id -> HistoryEntry
        self._nav_cursor: dict = {}               # session -> anchored message id (absent = latest)
        self._paused = threading.Event()          # play/pause: set == speech halted
        self._muted_sessions: set = set()         # sessions whose speech is muted
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
                 entry=None, mute_exempt: bool = False) -> None:
        item = SpeechItem(
            id=self._alloc_id(),
            session=session,
            kind=kind,
            text=text,
            is_decision=is_decision,
            mute_exempt=mute_exempt,
        )
        if entry is not None:
            self._pending_heard[item.id] = entry
        self.queue.enqueue(item)
        self._wake.set()

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

    def note_spoken(self, item, completed: bool) -> None:
        """Speak-loop bookkeeping: confirm (or decline) the heard-marker for a
        finished utterance, and release the voice when the queue drains."""
        with self._lock:
            self._current_item = None
            entry = self._pending_heard.pop(item.id, None)
            if entry is not None and completed:
                entry.heard = True
            if len(self.queue) == 0 and self._voice_owner not in self._open_msg:
                # Hold the voice while the owner still has an open message (the
                # queue drains to 0 between chunks of one reply); release only at
                # the turn boundary (H1).
                self._voice_owner = None

    def _may_speak(self, session: str) -> bool:
        """Voice continuity: a busy voice stays with its owner to the end; a
        free voice is acquired only by the FOREGROUND session, and only at a
        message boundary (a message that started captured stays captured)."""
        if self._voice_owner == session:
            return True
        if (self._voice_owner is None
                and self.sessions.is_foreground(session)
                and session not in self._captured_msg):
            self._voice_owner = session
            return True
        return False

    def _claim_for_decision(self, session: str) -> bool:
        """Decisions (question/plan/permission) are user-blocking and belong to the
        window the user is looking at. A decision for the FOREGROUND session claims
        a voice that is free OR held by a session whose message has already ENDED
        (a stale lock) — so the options are read even when a background owner still
        holds a finished-message lock (M4). It deliberately does NOT steal the voice
        from a session still STREAMING a reply (owner in _open_msg): interrupting an
        in-progress response is exactly what H1 prevents, so such a decision stays
        captured (its text is stored for reread / catch_up). A decision for the
        current owner is always honored. Superset of _may_speak."""
        if self._voice_owner == session:
            return True
        if (self.sessions.is_foreground(session)
                and self._voice_owner not in self._open_msg):
            self._voice_owner = session
            self._captured_msg.discard(session)
            return True
        return False

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
            if not final:
                # A message is now streaming for this session: hold its voice
                # across inter-chunk drains until the turn boundary (see _open_msg).
                self._open_msg.add(session)
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), final)
            if chunks:
                from sonari.assembler import PARAGRAPH_BREAK
                speak = verbosity != "quiet" and self._may_speak(session)
                for chunk in chunks:
                    if chunk is PARAGRAPH_BREAK:
                        # A blank-line boundary: start a new message group so the
                        # nav cursor treats each paragraph as its own 'item'.
                        self.history.end_message(session)
                        continue
                    entry = self.history.record(session, "prose", chunk)
                    if speak:
                        self._enqueue(session, "prose", chunk, False, entry=entry)
                    else:
                        self._captured_msg.add(session)
            if final:
                self.history.end_message(session)
                self._captured_msg.discard(session)
                self._open_msg.discard(session)   # turn boundary: voice may release
                self._options.pop(session, None)
            return None

        # Decision CONTENT is enqueued (and gated by foreground). The ALERT
        # earcon for a decision travels as a SEPARATE EARCON message that
        # hooks_entry emits BEFORE the content message; it is handled by the
        # MsgType.EARCON branch below, so the earcon fires instantly and
        # cross-session WITHOUT being doubled here.
        if t == MsgType.CHOICE:
            text = self._choice_text(msg)
            extras = [e for e in (
                self._choice_notes(msg),
                self._selection_cue(session, verbosity),
            ) if e]
            if extras:
                text = "{0} {1}".format(text, " ".join(extras))
            self._options[session] = text
            entry = self.history.record(session, "choice", text)
            self.history.end_message(session)
            if self._claim_for_decision(session):
                self._enqueue(session, "choice", text, True, entry=entry)
            return None

        if t == MsgType.PLAN:
            text = self._plan_text(msg)
            cue = self._selection_cue(session, verbosity)
            if cue:
                text = "{0} {1}".format(text, cue)
            self._options[session] = text
            entry = self.history.record(session, "plan", text)
            self.history.end_message(session)
            if self._claim_for_decision(session):
                self._enqueue(session, "plan", text, True, entry=entry)
            return None

        if t == MsgType.PERMISSION:
            text = self._permission_text(msg)
            cue = self._selection_cue(session, verbosity)
            if cue:
                text = "{0} {1}".format(text, cue)
            self._options[session] = text
            entry = self.history.record(session, "permission", text)
            self.history.end_message(session)
            if self._claim_for_decision(session):
                self._enqueue(session, "permission", text, True, entry=entry)
            return None

        if t == MsgType.TOOL:
            if verbosity == "everything" and self._may_speak(session):
                tool = msg.get("tool", "")
                summary = (msg.get("summary") or "").strip()
                text = summary if summary else "Running {0}.".format(tool)
                self._enqueue(session, "tool_announce", text, False)
            return None

        if t == MsgType.EARCON:
            # Instant: the Windows earcon backend plays on a separate audio path
            # that mixes with the speech, so it no longer cuts the reading.
            kind = msg.get("kind", "")
            self.speaker.earcon(kind)
            if kind == "turn_done":
                # End-of-turn boundary: the assistant produced its last delta, so
                # the prose message is no longer open and the voice may release even
                # if the final PROSE flag never arrived (H1 safety net).
                self._open_msg.discard(session)
            return None

        if t == MsgType.FLUSH:
            self._drop_pending(self.queue.flush_session(session))
            cur = self._current_item
            if cur is not None and cur.session == session:
                self.speaker.cancel()
            if self._voice_owner == session:
                self._voice_owner = None
            self._assemblers.pop(session, None)
            self.history.reset(session)
            self._nav_cursor.pop(session, None)   # new prompt -> fresh navigation
            # A new prompt is a user action -> auto-resume from pause (temp pause).
            self._paused.clear()
            self._wake.set()
            self._captured_msg.discard(session)
            self._open_msg.discard(session)
            self._options.pop(session, None)
            return None

        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            self.sessions.set_foreground(session, cwd=msg.get("cwd"))
            if t == MsgType.SESSION_START:
                self.sessions.register(session, cwd=msg.get("cwd"))
                self._maybe_guide_setup(session, msg.get("plugin_version", ""))
            return None

        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._drop_pending(self.queue.flush_session(session))
            if self._voice_owner == session:
                self._voice_owner = None
            self.history.reset(session)
            self._captured_msg.discard(session)
            self._open_msg.discard(session)
            self._options.pop(session, None)
            self._warned_immediate.discard(session)
            self._guided_sessions.discard(session)
            return None

        if t == MsgType.STOP:
            self._drop_pending(self.queue.clear())
            self.speaker.cancel()
            self._voice_owner = None
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
            if self._paused.is_set():
                self._resume()
            else:
                self._paused.set()
                # cancel() bumps the speaker's epoch, so even an utterance still
                # mid-synthesis aborts. The speak loop re-queues the interrupted
                # item (it sees completed=False while paused), so we don't capture
                # it here — which also avoids replaying an already-finished item.
                self.speaker.cancel()
            return None

        if t == MsgType.MUTE:
            # Toggle a sticky per-session mute. Earcons still fire (alerts), and the
            # "muted"/"unmuted" confirmation is spoken (the mute-on case is exempt).
            fg = self.sessions.foreground()
            if fg is None:
                return None
            if fg in self._muted_sessions:
                self._muted_sessions.discard(fg)
                self._enqueue(fg, "prose", "Session unmuted.", False)
            else:
                self._muted_sessions.add(fg)
                self._drop_pending(self.queue.flush_session(fg))
                cur = self._current_item
                if cur is not None and cur.session == fg:
                    self.speaker.cancel()
                self._enqueue(fg, "prose", "Session muted.", False, mute_exempt=True)
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
                self._enqueue(fg, "prose", "Nothing to repeat.", False)
                return None
            for e in entries:
                self._enqueue(fg, e.kind, e.text, False, entry=e)
            return None

        if t == MsgType.REREAD_OPTIONS:
            fg = self.sessions.foreground()
            if fg is None:
                return None
            text = self._options.get(fg)
            if text:
                self._enqueue(fg, "choice", text, False)
            else:
                self._enqueue(fg, "prose", "No options right now.", False)
            return None

        if t == MsgType.JUMP_DECISION:
            # Mark the cancelled current item heard and drop the heard-markers of
            # the skipped prose, so a later CATCH_UP doesn't replay them out of
            # order (mirrors SKIP) (M6).
            cur = self._current_item
            if cur is not None:
                entry = self._pending_heard.get(cur.id)
                if entry is not None:
                    entry.heard = True
            self._drop_pending(self.queue.jump_to_decision())
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
                self._enqueue(fg, "prose", "You're all caught up.", False)
                return None
            # Replay cleanly: cut the target's current utterance (it stays
            # unheard, so it replays FROM ITS START) and drop its queued
            # duplicates — every unheard entry is re-enqueued in order below.
            cur = self._current_item
            if cur is not None and cur.session == target:
                self.speaker.cancel()
            self._drop_pending(self.queue.flush_session(target))
            if preamble:
                self._enqueue(fg, "prose", preamble, False)
            for e in entries:
                self._enqueue(target, e.kind, e.text,
                              e.kind in ("choice", "plan", "permission"),
                              entry=e)
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
                "queue_len": len(self.queue),
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

    def _nav(self, session: str, to: str) -> None:
        """Move the per-session message cursor and play from there to the end.

        The cursor indexes the current turn's messages (history resets each
        prompt), oldest..newest; absent == the latest. 'next'/'prev' step one
        message and CLAMP at the ends (no wrap; at the newest, 'next' just
        re-reads it); 'first'/'last' jump to the start/end of the turn. Every
        move cuts current speech, clears the queue, and reads the target message
        AND every later one (seek-and-play) so playback continues instead of
        stopping after a single item. Newly streamed prose enqueues after these
        and continues seamlessly."""
        ids = self.history.message_ids(session)
        if not ids:
            self._enqueue(session, "prose", "Nothing to navigate yet.", False)
            return
        # Navigating is an active foreground action: claim the voice for this
        # session so prose streaming in after the replay is spoken (L3). Use the
        # SAME conservative rule as _claim_for_decision (M4): take only a free or
        # stale-lock voice, or one we already own — never SEIZE it from a different
        # session still streaming a reply (owner in _open_msg), which would strand
        # that in-progress response (the very thing H1 prevents).
        if self._voice_owner == session or self._voice_owner not in self._open_msg:
            self._voice_owner = session
            self._captured_msg.discard(session)
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
        self._drop_pending(self.queue.flush_session(session))
        # Seek-and-play: enqueue the target item AND every later item, so nav reads
        # from here forward (not just one item). Newly streamed prose enqueues after
        # these and continues seamlessly — no jump from the replay into a live delta.
        for mid in ids[new:]:
            for e in self.history.entries_for_message(session, mid):
                self._enqueue(session, e.kind, e.text, False, entry=e)

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
                # speak thread. A crash in pop_next/note_spoken/etc. used to leave
                # the daemon alive (earcons kept firing) but mute forever until a
                # restart. Log the traceback (captured by the daemon log) and keep
                # going; a short wait avoids a tight error-spin.
                import sys
                import traceback
                traceback.print_exc(file=sys.stderr)
                self._wake.wait(0.1)

    def _speak_loop_once(self) -> None:
        """One iteration of the speak loop. May raise; _speak_loop contains it."""
        if self._paused.is_set():
            # Play/pause: hold the loop without consuming the queue.
            self._wake.wait(self._poll_interval)
            self._wake.clear()
            return
        # Pop and CLAIM the item atomically under the lock. PAUSE/MUTE/FLUSH run
        # under this same lock, so popping + setting _current_item together means
        # they always observe a consistent current item and can't slip into the
        # gap between pop and claim (losing the item or failing to cancel it).
        with self._lock:
            item = self.queue.pop_next()
            self._current_item = item
            # Capture the speaker's cancel baseline HERE, atomically with the claim.
            # A cancel() arriving after we release the lock (but before/while speak()
            # runs) bumps the epoch past this baseline, so the cancelled utterance is
            # detected instead of playing in full (M2 — the pop->speak gap).
            cancel_epoch = self.speaker.cancel_epoch()
            muted = (item is not None
                     and item.session in self._muted_sessions
                     and not item.mute_exempt)
            if muted:
                # Muted session: drop without speaking; release the claim.
                self._current_item = None
                self._pending_heard.pop(item.id, None)
        if item is None:
            with self._lock:
                if (self._voice_owner is not None and len(self.queue) == 0
                        and self._voice_owner not in self._open_msg):
                    self._voice_owner = None
            # nothing to say: wait until woken by an enqueue or until stop()
            self._wake.wait(self._poll_interval)
            self._wake.clear()
            return
        if muted:
            return
        try:
            completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch)
        except Exception:  # noqa: BLE001 - one bad utterance must not abort the item
            completed = False
        requeued = False
        with self._lock:
            # Re-check pause INSIDE the lock (L2). A FLUSH (new prompt) also runs
            # under this lock and clears pause + flushes the queue; checking pause
            # outside the lock let a FLUSH land between the check and the
            # enqueue_front, resurrecting a just-flushed item. Atomic check+enqueue
            # closes that window.
            if not completed and self._paused.is_set():
                # A pause interrupted this utterance: re-queue it at the front so
                # resume picks back up here, and KEEP its _pending_heard entry (don't
                # note_spoken) so the eventual replay can still record it as heard.
                self._current_item = None
                self.queue.enqueue_front(item)
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
    from sonari.queue import SpeechQueue
    from sonari.sessions import SessionManager
    from sonari.platform import get_platform

    _backend = get_platform()
    cfg = load_config()
    if "earcons" not in cfg:
        cfg["earcons"] = _backend.earcon.default_earcons()
    queue = SpeechQueue()
    speaker = Speaker(
        voice=cfg.get("voice"),
        rate=cfg.get("rate", 200),
        say_runner=_backend.tts.run,
        earcon_player=_backend.earcon.play,
        earcons=cfg.get("earcons"),
    )
    sessions = SessionManager(background_policy=cfg.get("background_policy", "earcon_only"))
    daemon = SpeechDaemon(queue, speaker, sessions, cfg)
    daemon.run()


if __name__ == "__main__":
    main()

