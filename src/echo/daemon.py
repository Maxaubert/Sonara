from echo.protocol import MsgType
from echo.queue import SpeechItem
from echo.assembler import ProseAssembler


class SpeechDaemon:
    def __init__(self, queue, speaker, sessions, config) -> None:
        self.queue = queue
        self.speaker = speaker
        self.sessions = sessions
        self.config = config
        self._assemblers = {}
        self._next_id = 0

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

    def handle_message(self, msg):
        t = msg.get("type")
        if t == MsgType.PROSE:
            session = msg.get("session", "")
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), msg.get("final", False))
            if self.sessions.should_speak(session):
                for chunk in chunks:
                    self._enqueue(session, "prose", chunk, False)
            return None
        return None
