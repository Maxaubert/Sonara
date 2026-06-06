from __future__ import annotations

import os
import socket
import subprocess
import threading

from sonari.protocol import MsgType, encode, decode
from sonari.queue import SpeechItem
from sonari.assembler import ProseAssembler
from sonari.config import save_config, load_config
from sonari.paths import (
    SOCKET_PATH, ensure_sonari_dir, socket_connectable, repo_root,
    INSTALL_RECORD_PATH,
)


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
        self._threads = []
        self._poll_interval = 0.1
        self._last_spoken: str | None = None
        self._last_options: str | None = None
        self._warned_immediate: set[str] = set()
        self._guided_sessions: set[str] = set()

    def _alloc_id(self) -> int:
        self._next_id += 1
        return self._next_id

    def _assembler(self, session: str) -> ProseAssembler:
        a = self._assemblers.get(session)
        if a is None:
            a = ProseAssembler()
            self._assemblers[session] = a
        return a

    def _enqueue(self, session: str, kind: str, text: str, is_decision: bool) -> None:
        item = SpeechItem(
            id=self._alloc_id(),
            session=session,
            kind=kind,
            text=text,
            is_decision=is_decision,
        )
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

    @staticmethod
    def _choice_text(msg) -> str:
        parts = []
        for q in msg.get("questions", []) or []:
            qtext = q.get("question", "") if isinstance(q, dict) else str(q)
            opts = q.get("options", []) if isinstance(q, dict) else []
            labels = []
            for o in opts:
                if isinstance(o, dict):
                    labels.append(o.get("label", ""))
                else:
                    labels.append(str(o))
            labels = [l for l in labels if l]
            # Number the options so the user can pick by number (eyes-free).
            segs = ["Option {0}: {1}.".format(i, label) for i, label in enumerate(labels, 1)]
            if qtext and segs:
                parts.append("{0} {1}".format(qtext, " ".join(segs)))
            elif segs:
                parts.append(" ".join(segs))
            elif qtext:
                parts.append(qtext)
        return " ".join(parts) if parts else "A question needs your answer."

    @staticmethod
    def _plan_text(msg) -> str:
        text = (msg.get("text") or "").strip()
        if text:
            return "Plan ready. {0}".format(text)
        return "A plan is ready for your review."

    @staticmethod
    def _permission_text(msg) -> str:
        # The 'permission' earcon already signals that approval is needed, so the
        # spoken text is just the pending action (e.g. "Run: pytest -q").
        action = (msg.get("action") or "").strip()
        return action if action else "Permission needed."

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
        """True if ~/.local/bin/sonari exists (cheap stat)."""
        import os as _os
        return _os.path.exists(
            _os.path.join(_os.path.expanduser("~"), ".local", "bin", "sonari"))

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
            if verbosity != "quiet" and self.sessions.should_speak(session):
                for chunk in chunks:
                    self._enqueue(session, "prose", chunk, False)
            return None

        # Decision CONTENT is enqueued (and gated by foreground). The ALERT
        # earcon for a decision travels as a SEPARATE EARCON message that
        # hooks_entry emits BEFORE the content message; it is handled by the
        # MsgType.EARCON branch below, so the earcon fires instantly and
        # cross-session WITHOUT being doubled here.
        if t == MsgType.CHOICE:
            if self.sessions.should_speak(session):
                text = self._choice_text(msg)
                extras = [e for e in (
                    self._choice_notes(msg),
                    self._selection_cue(session, verbosity),
                ) if e]
                if extras:
                    text = "{0} {1}".format(text, " ".join(extras))
                self._last_options = text
                self._enqueue(session, "choice", text, True)
            return None

        if t == MsgType.PLAN:
            if self.sessions.should_speak(session):
                text = self._plan_text(msg)
                cue = self._selection_cue(session, verbosity)
                if cue:
                    text = "{0} {1}".format(text, cue)
                self._last_options = text
                self._enqueue(session, "plan", text, True)
            return None

        if t == MsgType.PERMISSION:
            if self.sessions.should_speak(session):
                text = self._permission_text(msg)
                cue = self._selection_cue(session, verbosity)
                if cue:
                    text = "{0} {1}".format(text, cue)
                self._last_options = text
                self._enqueue(session, "permission", text, True)
            return None

        if t == MsgType.TOOL:
            if verbosity == "everything" and self.sessions.should_speak(session):
                tool = msg.get("tool", "")
                summary = (msg.get("summary") or "").strip()
                text = summary if summary else "Running {0}.".format(tool)
                self._enqueue(session, "tool_announce", text, False)
            return None

        if t == MsgType.EARCON:
            self.speaker.earcon(msg.get("kind", ""))
            return None

        if t == MsgType.FLUSH:
            self.queue.flush_session(session)
            self.speaker.cancel()
            self._assemblers.pop(session, None)
            self._last_options = None
            return None

        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            self.sessions.set_foreground(session)
            if t == MsgType.SESSION_START:
                self.sessions.register(session)
                self._maybe_guide_setup(session, msg.get("plugin_version", ""))
            return None

        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._last_options = None
            self._warned_immediate.discard(session)
            self._guided_sessions.discard(session)
            return None

        if t == MsgType.STOP:
            self.queue.clear()
            self.speaker.cancel()
            return None

        if t == MsgType.SKIP:
            self.speaker.cancel()
            return None

        if t == MsgType.REPEAT:
            last = self._last_spoken
            if last is not None:
                fg = self.sessions.foreground()
                if fg is not None:
                    self._enqueue(fg, "prose", last, False)
            return None

        if t == MsgType.REREAD_OPTIONS:
            fg = self.sessions.foreground()
            if self._last_options and fg is not None:
                self._enqueue(fg, "choice", self._last_options, False)
            elif fg is not None:
                self._enqueue(fg, "prose", "No options to repeat.", False)
            return None

        if t == MsgType.JUMP_DECISION:
            self.queue.jump_to_decision()
            self.speaker.cancel()
            return None

        if t == MsgType.CATCH_UP:
            self.queue.clear()
            self.speaker.cancel()
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
                self.speaker.speak(item.text)
                with self._lock:
                    self._last_spoken = item.text
                continue
            # nothing to say: wait until woken by an enqueue or until stop()
            self._wake.wait(self._poll_interval)
            self._wake.clear()

    def _handle_conn(self, conn) -> None:
        try:
            buf = b""
            with conn:
                conn.settimeout(5.0)
                while self._running.is_set():
                    try:
                        data = conn.recv(4096)
                    except (OSError, socket.timeout):
                        return
                    if not data:
                        return
                    buf += data
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
        # unlink a stale socket file before binding
        try:
            os.unlink(SOCKET_PATH)
        except FileNotFoundError:
            pass
        srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        srv.bind(str(SOCKET_PATH))
        srv.listen(16)
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
                os.unlink(SOCKET_PATH)
            except FileNotFoundError:
                pass


def _daemon_shim_path() -> str:
    return os.path.join(repo_root(), "bin", "sonari-daemon")


def ensure_running() -> None:
    if socket_connectable():
        return
    shim = _daemon_shim_path()
    subprocess.Popen(
        [shim],
        start_new_session=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def main() -> None:
    # Single-instance guard: if a daemon is already accepting connections, exit
    # cleanly instead of unlinking + rebinding the live socket (prevents the
    # duplicate-daemon race between a lazy start and the LaunchAgent at login).
    if socket_connectable():
        return

    from sonari.speaker import Speaker
    from sonari.queue import SpeechQueue
    from sonari.sessions import SessionManager

    cfg = load_config()
    queue = SpeechQueue()
    speaker = Speaker(
        voice=cfg.get("voice"),
        rate=cfg.get("rate", 200),
        earcons=cfg.get("earcons"),
    )
    sessions = SessionManager(background_policy=cfg.get("background_policy", "earcon_only"))
    daemon = SpeechDaemon(queue, speaker, sessions, cfg)
    daemon.run()


if __name__ == "__main__":
    main()

