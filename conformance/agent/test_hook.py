"""The Claude Code hook adapter (``sonara-hook.exe``, L5) end to end against
``sonarad``: hook events as Claude Code delivers them (event name in argv,
payload on stdin) become a session's turn in the runtime. Needs
``cargo build -p sonara-hook`` (or ``$SONARA_HOOK``)."""
from __future__ import annotations

import json
import time

import pytest

import harness

SESSION = "11111111-2222-3333-4444-555555555555"


@pytest.fixture(scope="module")
def hook_exe():
    exe = harness.find_hook()
    if exe is None or not exe.is_file():
        pytest.fail("sonara-hook.exe not found: run 'cargo build -p sonara-hook' or set SONARA_HOOK")
    return exe


def hook(exe, rt, event, payload, env=None):
    code = harness.Hook(exe, rt.home, event, env).send(payload)
    assert code == 0


def listener(rt):
    c = rt.tcp(extensions=["agent"])
    # Read prose at once (the product default holds the turn, #222).
    assert c.request({"type": "set", "key": "read_mode", "value": "immediate"})["ok"]
    assert c.request({"type": "subscribe", "events": ["state", "earcons"]})["ok"]
    c.state()
    return c


def started(c, text, timeout=harness.TIMEOUT):
    """Wait until an item starts whose first chunk is ``text``."""
    return c.next_event(
        lambda e: e.get("event") == "state"
        and e["now_playing"] is not None
        and e["now_playing"]["text"] == text,
        timeout,
    )


def test_a_prompt_and_its_answer_are_read_in_the_sessions_channel(hook_exe, rt):
    c = listener(rt)
    hook(hook_exe, rt, "UserPromptSubmit", {"session_id": SESSION, "cwd": r"C:\work\proj"},
         {"SONARA_HOST_TAB": "tab-9"})
    hook(hook_exe, rt, "MessageDisplay",
         {"session_id": SESSION, "delta": "Here is the answer. ", "index": 0, "final": False})
    s = started(c, "Here is the answer.")
    assert s["now_playing"]["channel"] == SESSION
    assert s["now_playing"]["host_tab"] == "tab-9"
    hook(hook_exe, rt, "Stop", {"session_id": SESSION})
    assert c.next_event(lambda e: e.get("event") == "earcon")["kind"] == "turn_done"


def test_the_old_turns_late_text_is_dropped(hook_exe, rt):
    # #174 end to end: the old turn's MessageDisplay hook starts before the
    # new prompt's hook but delivers after it.
    c = listener(rt)
    hook(hook_exe, rt, "UserPromptSubmit", {"session_id": SESSION, "cwd": "/w/proj"})
    late = harness.Hook(hook_exe, rt.home, "MessageDisplay")
    time.sleep(0.05)
    hook(hook_exe, rt, "UserPromptSubmit", {"session_id": SESSION, "cwd": "/w/proj"})
    assert late.send({"session_id": SESSION, "delta": "Old turn tail.", "index": 3, "final": True}) == 0
    hook(hook_exe, rt, "MessageDisplay",
         {"session_id": SESSION, "delta": "New answer.", "index": 0, "final": True})
    s = started(c, "New answer.")
    assert s["now_playing"]["channel"] == SESSION
    for e in c.events:
        if e.get("event") == "state" and e["now_playing"] is not None:
            assert e["now_playing"]["text"] != "Old turn tail."


def test_a_question_chimes_and_its_permission_prompt_is_suppressed(hook_exe, rt):
    c = listener(rt)
    payload = harness.REPO / "tests" / "fixtures" / "PreToolUse-AskUserQuestion.json"
    hook(hook_exe, rt, "PreToolUse", json.loads(payload.read_text(encoding="utf-8")))
    assert c.next_event(lambda e: e.get("event") == "earcon")["kind"] == "choice"
    started(c, "Which color do you prefer?")
    hook(hook_exe, rt, "Notification",
         {"session_id": SESSION, "notification_type": "permission_prompt",
          "message": "Claude needs your permission to use AskUserQuestion"})
    with pytest.raises(AssertionError):
        c.next_event(lambda e: e.get("event") == "earcon", 0.5)
    hook(hook_exe, rt, "PostToolUse", {"session_id": SESSION, "tool_name": "AskUserQuestion"})
    c.state(lambda s: s["now_playing"] is None)


def test_the_hook_asks_for_system_and_keeps_the_runtime(hook_exe, rt):
    hook(hook_exe, rt, "SessionStart", {"session_id": SESSION, "cwd": "/w/proj"})
    assert "system" in rt.tcp().request({"type": "hello", "token": rt.token})["extensions"]


def test_the_hook_starts_the_runtime_when_none_runs(hook_exe, tmp_path):
    # #197: no runtime yet -> the hook starts sonarad.exe from its own
    # folder (the fake engine and system through SONARA_RUNTIME_ARGS), and
    # the event reaches it.
    exe = hook_exe.parent / "sonarad.exe"
    if not exe.is_file():
        pytest.fail(f"{exe} not found: build sonarad next to sonara-hook")
    home = tmp_path / "home"
    home.mkdir()
    env = {"SONARA_RUNTIME_ARGS": "--engine fake --system fake"}
    rt = None
    try:
        t0 = time.monotonic()
        # With pipes, as Claude Code runs it: the started runtime must not
        # hold the hook's output pipes open (communicate waits for their end).
        code = harness.Hook(hook_exe, home, "SessionStart", env, pipes=True).send(
            {"session_id": SESSION, "cwd": "/w/proj"})
        took = time.monotonic() - t0
        assert harness.wait_until(lambda: (home / "runtime.json").exists())
        info = json.loads((home / "runtime.json").read_text(encoding="utf-8"))
        rt = harness.Runtime.attach(exe, home, info)
        assert code == 0
        assert took < 5, f"the hook took {took:.1f} s"
        c = rt.tcp(extensions=["agent"])
        assert c.request({"type": "focus", "channel": SESSION})["ok"] is True
        r = c.request({"type": "get", "key": "settings_url"})
        assert r["ok"] is True, "system is enabled for the Claude product"
    finally:
        if rt is None and (home / "runtime.json").exists():
            info = json.loads((home / "runtime.json").read_text(encoding="utf-8"))
            rt = harness.Runtime.attach(exe, home, info)
        if rt is not None:
            rt.kill()


def test_a_session_end_closes_the_channel(hook_exe, rt):
    c = listener(rt)
    hook(hook_exe, rt, "SessionStart", {"session_id": SESSION, "cwd": "/w/proj"})
    hook(hook_exe, rt, "SessionEnd", {"session_id": SESSION})
    r = c.request({"type": "focus", "channel": SESSION})
    assert r["error"]["code"] == "E_NOT_FOUND"


def test_a_session_keeps_its_project_name_when_it_moves_between_worktrees(hook_exe, rt, tmp_path):
    r"""#245, the hook logs of 2026-10-05: a Filesmith session's cwd flipped
    between the repository and its .claude\worktrees\statusbar, and between
    subfolders, and the session's name followed. It stays "Filesmith"."""
    repo = tmp_path / "Filesmith"
    (repo / ".git" / "worktrees" / "statusbar").mkdir(parents=True)
    wt = repo / ".claude" / "worktrees" / "statusbar"
    (wt / "src").mkdir(parents=True)
    (wt / ".git").write_text("gitdir: ../../../.git/worktrees/statusbar\n", encoding="utf-8")
    (repo / "app" / "dist").mkdir(parents=True)
    c = listener(rt)
    sid = "245f-0000"
    hook(hook_exe, rt, "SessionStart", {"session_id": sid, "cwd": str(repo)})
    for cwd in (wt, repo, wt / "src", repo / "app" / "dist", wt):
        hook(hook_exe, rt, "UserPromptSubmit", {"session_id": sid, "cwd": str(cwd)})
    hook(hook_exe, rt, "MessageDisplay",
         {"session_id": sid, "cwd": str(wt / "src"), "delta": "Still Filesmith.", "index": 0,
          "final": True})
    s = started(c, "Still Filesmith.")
    assert s["now_playing"]["channel"] == sid
    assert s["now_playing"]["label"] == "Filesmith"
    prefs = c.request({"type": "get", "key": "channel_prefs"})
    row = next(r for r in prefs["value"] if r["channel"] == sid)
    assert row["client_label"] == "Filesmith"


def test_a_session_started_outside_a_repo_keeps_its_first_folder(hook_exe, rt, tmp_path):
    """Outside any repository the folder the session started in names it,
    also after a cd into a subfolder."""
    start = tmp_path / "notes"
    (start / "drafts").mkdir(parents=True)
    c = listener(rt)
    sid = "245n-0000"
    hook(hook_exe, rt, "SessionStart", {"session_id": sid, "cwd": str(start)})
    hook(hook_exe, rt, "UserPromptSubmit", {"session_id": sid, "cwd": str(start / "drafts")})
    hook(hook_exe, rt, "MessageDisplay",
         {"session_id": sid, "cwd": str(start / "drafts"), "delta": "Notes here.", "index": 0,
          "final": True})
    s = started(c, "Notes here.")
    assert s["now_playing"]["label"] == "notes"
