"""Pure mapping from Claude Code hook events to protocol message dicts."""
from echo.protocol import PROTOCOL_VERSION, MsgType


def _msg(**fields):
    """Build a protocol message dict, always stamped with the protocol version."""
    out = {"v": PROTOCOL_VERSION}
    out.update(fields)
    return out


def handle_event(event: str, payload: dict) -> list[dict]:
    """Map (event name, parsed stdin payload) to a list of protocol messages.

    PURE: no I/O. Returns [] for any event it does not handle.
    """
    session = payload.get("session_id", "")

    if event == "MessageDisplay":
        return [
            _msg(
                type=MsgType.PROSE,
                session=session,
                delta=payload.get("delta", ""),
                index=payload.get("index", 0),
                final=payload.get("final", False),
            )
        ]

    return []
