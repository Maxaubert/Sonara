"""Task 8 -- exec-form hooks.json builder tests.

Verifies that build_hooks_json() produces valid JSON with Windows paths
correctly escaped, in exec-form (command + args) for all Sonara events.
"""
import json
import pytest
from sonara.install.claude_hooks import build_hooks_json

EXPECTED_EVENTS = {
    "MessageDisplay",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "UserPromptSubmit",
    "SessionStart",
    "SessionEnd",
}


def test_hooks_json_is_exec_form_with_escaped_paths():
    s = build_hooks_json(r"C:\u\.sonara\pythonw.exe", r"C:\plug\hook.py")
    data = json.loads(s)  # valid JSON (backslashes doubled)
    md = data["hooks"]["MessageDisplay"][0]["hooks"][0]
    assert md["type"] == "command"
    assert md["command"].endswith("pythonw.exe")
    assert md["args"][0].endswith("hook.py") and md["args"][-1] == "MessageDisplay"


def test_hooks_json_contains_all_expected_events():
    """All eight hook event types must be present; a silent omission would
    break Windows installs without failing the JSON-validity check (H4: the
    hand-kept template had lost PostToolUse)."""
    s = build_hooks_json(r"C:\u\.sonara\pythonw.exe", r"C:\plug\hook.py")
    data = json.loads(s)
    assert set(data["hooks"]) == EXPECTED_EVENTS


@pytest.mark.parametrize("event", sorted(EXPECTED_EVENTS))
def test_hooks_json_event_is_exec_form(event: str):
    """Every event entry must use exec-form (type=command, command, args)."""
    s = build_hooks_json(r"C:\u\.sonara\pythonw.exe", r"C:\plug\hook.py")
    data = json.loads(s)
    for entry in data["hooks"][event]:
        hook = entry["hooks"][0]
        assert hook["type"] == "command"
        assert hook["command"].endswith("pythonw.exe")
        assert hook["args"][0].endswith("hook.py")
        assert hook["args"][-1] == event


def test_no_redundant_registrations():
    # DC9: the '' PreToolUse matcher already routes AskUserQuestion and
    # ExitPlanMode (hooks_entry branches on tool_name), and idle_prompt produced
    # no message, so neither the tool-specific matchers nor idle_prompt ship.
    data = json.loads(build_hooks_json(r"C:\u\.sonara\pythonw.exe", r"C:\plug\hook.py"))
    assert [e["matcher"] for e in data["hooks"]["PreToolUse"]] == [""]
    assert [e["matcher"] for e in data["hooks"]["Notification"]] == ["permission_prompt"]


def test_template_matchers_mirror_the_plugin_hooks_file():
    # The settings.json template mirrors hooks/hooks.json event by event.
    from pathlib import Path
    plugin = json.loads((Path(__file__).resolve().parent.parent / "hooks" / "hooks.json")
                        .read_text(encoding="utf-8"))["hooks"]
    data = json.loads(build_hooks_json(r"C:\u\.sonara\pythonw.exe", r"C:\plug\hook.py"))
    for event, entries in data["hooks"].items():
        assert [e["matcher"] for e in entries] == [e["matcher"] for e in plugin[event]], event
