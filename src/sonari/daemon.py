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
    LOCK_PATH, ensure_sonari_dir, socket_connectable, repo_root,
    INSTALL_RECORD_PATH,
)
from sonari.platform import transport


RATE_MIN = 100
RATE_MAX = 400


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
        self._threads = []
        self._poll_interval = 0.1
        from sonari.history import SessionHistory
        self.history = SessionHistory(cap=int(config.get("history_cap", 200)))
        self._options: "dict[str, str]" = {}
        self._voice_owner: "str | None" = None
        self._captured_msg: "set[str]" = set()
        self._pending_heard: dict = {}            # SpeechItem.id -> HistoryEntry
        self._current_item = None                 # item being spoken right now
        self._warned_immediate: set = set()
        self._guided_sessions: set = set()

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
                 entry=None) -> None:
        item = SpeechItem(
            id=self._alloc_id(),
            session=session,
            kind=kind,
            text=text,
            is_decision=is_decision,
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
            if len(self.queue) == 0:
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
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), msg.get("final", False))
            if chunks:
                speak = verbosity != "quiet" and self._may_speak(session)
                for chunk in chunks:
                    entry = self.history.record(session, "prose", chunk)
                    if speak:
                        self._enqueue(session, "prose", chunk, False, entry=entry)
                    else:
                        self._captured_msg.add(session)
            if msg.get("final", False):
                self.history.end_message(session)
                self._captured_msg.discard(session)
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
            if self._may_speak(session):
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
            if self._may_speak(session):
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
            if self._may_speak(session):
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
            self.speaker.earcon(msg.get("kind", ""))
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
            self._captured_msg.discard(session)
            self._options.pop(session, None)
            return None

        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            self.sessions.set_foreground(session)
            if t == MsgType.SESSION_START:
                self.sessions.register(session)
                self._maybe_guide_setup(session, msg.get("plugin_version", ""))
            return None

        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._drop_pending(self.queue.flush_session(session))
            if self._voice_owner == session:
                self._voice_owner = None
            self.history.reset(session)
            self._captured_msg.discard(session)
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

        if t == MsgType.REPEAT:
            fg = self.sessions.foreground()
            if fg is None:
                return None
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
            self.queue.jump_to_decision()
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
                rate = msg.get("rate")
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
        srv = self._server
        if srv is not None:
            try:
                srv.close()
            except OSError:
                pass

    def _speak_loop(self) -> None:
        self._running.set()
        while self._running.is_set():
            item = self.queue.pop_next()
            if item is not None:
                with self._lock:
                    self._current_item = item
                completed = self.speaker.speak(item.text)
                self.note_spoken(item, completed)
                continue
            with self._lock:
                if self._voice_owner is not None and len(self.queue) == 0:
                    self._voice_owner = None
            # nothing to say: wait until woken by an enqueue or until stop()
            self._wake.wait(self._poll_interval)
            self._wake.clear()

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
                        with self._lock:
                            reply = self.handle_message(msg)
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

    def _accept_loop(self) -> None:
        srv = self._server
        while self._running.is_set():
            try:
                conn, _ = srv.accept()
            except OSError:
                return
            th = threading.Thread(target=self._handle_conn, args=(conn,), daemon=True)
            th.start()

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
        self._threads = [speak_thread, accept_thread]
        speak_thread.start()
        accept_thread.start()

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


def _daemon_shim_path() -> str:
    return os.path.join(repo_root(), "bin", "sonari-daemon")


def ensure_running() -> None:
    if socket_connectable():
        return
    from sonari.platform import get_platform
    argv, kwargs = get_platform().supervisor.launch_spec()
    subprocess.Popen(argv, **kwargs)


def main() -> None:
    # Single-instance guard: if a daemon is already accepting connections, exit
    # cleanly instead of unlinking + rebinding the live socket (prevents the
    # duplicate-daemon race between a lazy start and the LaunchAgent at login).
    if socket_connectable():
        return

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

