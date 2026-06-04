import os
import socket
import threading

from echo.protocol import MsgType, encode, decode
from echo.queue import SpeechItem
from echo.assembler import ProseAssembler
from echo.config import save_config, load_config
from echo.paths import SOCKET_PATH, ensure_echo_dir


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

    def handle_message(self, msg):
        t = msg.get("type")
        session = msg.get("session", "")
        verbosity = self.config.get("verbosity", "everything")

        if t == MsgType.PROSE:
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), msg.get("final", False))
            if self.sessions.should_speak(session):
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
                self._enqueue(session, "choice", self._choice_text(msg), True)
            return None

        if t == MsgType.PLAN:
            if self.sessions.should_speak(session):
                self._enqueue(session, "plan", self._plan_text(msg), True)
            return None

        if t == MsgType.PERMISSION:
            if self.sessions.should_speak(session):
                self._enqueue(session, "permission", self._permission_text(msg), True)
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
            return None

        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            self.sessions.set_foreground(session)
            if t == MsgType.SESSION_START:
                self.sessions.register(session)
            return None

        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            return None

        if t == MsgType.STOP:
            self.queue.clear()
            self.speaker.cancel()
            return None

        if t == MsgType.SKIP:
            self.speaker.cancel()
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
            rate = msg.get("rate")
            self.config["rate"] = rate
            self.speaker.set_rate(rate)
            save_config(self.config)
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
        ensure_echo_dir()
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

