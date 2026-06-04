"""Pure mapping from Claude Code hook events to protocol message dicts."""
import os

from echo.protocol import PROTOCOL_VERSION, MsgType


def _msg(**fields):
    """Build a protocol message dict, always stamped with the protocol version."""
    out = {"v": PROTOCOL_VERSION}
    out.update(fields)
    return out


def _tool_summary(tool: str, ti: dict) -> str:
    """Short, speakable, tool-specific description of a pending tool call."""
    if tool == "Bash":
        cmd = (ti.get("command") or "").strip()
        return cmd[:120] if cmd else "Bash"
    if tool in ("Write", "Edit", "MultiEdit", "NotebookEdit"):
        path = ti.get("file_path") or ti.get("notebook_path") or ""
        base = os.path.basename(path.rstrip("/")) if path else ""
        return base if base else (tool or "")
    return tool or ""


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

    if event == "PreToolUse":
        tool = payload.get("tool_name")
        ti = payload.get("tool_input", {})
        if tool == "AskUserQuestion":
            return [
                _msg(type=MsgType.EARCON, kind="choice"),
                _msg(
                    type=MsgType.CHOICE,
                    session=session,
                    questions=ti.get("questions", []),
                ),
            ]
        if tool == "ExitPlanMode":
            return [
                _msg(type=MsgType.EARCON, kind="plan"),
                _msg(type=MsgType.PLAN, session=session, text=ti.get("plan", "")),
            ]
        return [
            _msg(
                type=MsgType.TOOL,
                session=session,
                tool=tool,
                summary=_tool_summary(tool, ti),
            )
        ]

    if event == "Notification":
        nt = payload.get("notification_type") or payload.get("matcher")
        if nt == "permission_prompt":
            return [
                _msg(type=MsgType.EARCON, kind="permission"),
                _msg(
                    type=MsgType.PERMISSION,
                    session=session,
                    action=payload.get("action", ""),
                ),
            ]
        if nt == "idle_prompt":
            return [_msg(type=MsgType.EARCON, kind="ready")]
        return []

    return []
