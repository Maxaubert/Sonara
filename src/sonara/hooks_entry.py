"""Pure mapping from Claude Code hook events to protocol message dicts."""
from __future__ import annotations

import os

from sonara.protocol import PROTOCOL_VERSION, MsgType


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


def _host_tab(env) -> dict:
    """The embedding host's tab id as message fields (#143): {"host_tab": id}
    when the hook runs inside a host tab, else {} so a plain terminal sends
    the same messages as before. SONARA_HOST_TAB is the generic name;
    PRISM_TAB_ID (PrismTerminal) is accepted too."""
    tab = env.get("SONARA_HOST_TAB") or env.get("PRISM_TAB_ID")
    return {"host_tab": tab} if tab else {}


def handle_event(event: str, payload: dict, env=None) -> list[dict]:
    """Map (event name, parsed stdin payload) to a list of protocol messages.

    PURE: no I/O. *env* is the hook's environment (default os.environ, read
    at call time); every environment read goes through it. Returns [] for
    any event it does not handle.
    """
    if env is None:
        env = os.environ
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
            # No 'plan' chime (removed per user request); the plan is still spoken.
            return [
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

    if event == "PostToolUse":
        # The user ANSWERED a blocking question (#83): everything queued
        # before it is stale - the daemon flushes the backlog and kills
        # in-flight lead-in digests. Every other tool's completion is noise.
        if payload.get("tool_name") == "AskUserQuestion":
            return [_msg(type=MsgType.CHOICE_ANSWERED, session=session)]
        return []

    if event == "Notification":
        nt = payload.get("notification_type") or payload.get("matcher")
        if nt == "permission_prompt":
            return [
                _msg(type=MsgType.EARCON, kind="permission"),
                _msg(
                    type=MsgType.PERMISSION,
                    session=session,
                    action=payload.get("action", ""),
                    message=payload.get("message", ""),
                ),
            ]
        # No 'ready' chime on idle_prompt (removed per user request): Claude Code
        # fires this whenever it goes idle, so the beep felt random and untriggered.
        return []

    if event == "Stop":
        # session is carried so the daemon can close this session's open prose
        # message at the turn boundary (releases the held voice -- H1).
        return [_msg(type=MsgType.EARCON, kind="turn_done", session=session)]

    if event == "UserPromptSubmit":
        return [
            _msg(type=MsgType.SET_FOREGROUND, session=session,
                 cwd=payload.get("cwd", ""), **_host_tab(env)),
            _msg(type=MsgType.FLUSH, session=session),
        ]

    if event == "SessionStart":
        return [
            _msg(type=MsgType.SET_FOREGROUND, session=session,
                 cwd=payload.get("cwd", ""), **_host_tab(env)),
            _msg(
                type=MsgType.SESSION_START,
                session=session,
                cwd=payload.get("cwd", ""),
                plugin_version=env.get("CLAUDE_PLUGIN_VERSION", ""),
                plugin_root=env.get("CLAUDE_PLUGIN_ROOT", ""),
                **_host_tab(env),
            ),
        ]

    if event == "SessionEnd":
        return [_msg(type=MsgType.SESSION_END, session=session)]

    return []
