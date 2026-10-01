"""Host tab id through the hook (#143): an embedding host (PrismTerminal
first) runs Claude Code inside one of its tabs and sets SONARA_HOST_TAB
(PRISM_TAB_ID is accepted too) in that tab's environment. The hook passes it
as host_tab on SESSION_START and SET_FOREGROUND, the daemon remembers it per
session and exposes it to the settings page and the state stream."""
from __future__ import annotations

from sonara.hooks_entry import handle_event
from sonara.protocol import MsgType
from sonara.sessions import SessionManager
from tests.daemon_helpers import make_daemon


def _by_type(msgs, t):
    return [m for m in msgs if m["type"] == t]


def test_session_start_carries_host_tab_from_injected_env():
    msgs = handle_event("SessionStart", {"session_id": "s1", "cwd": "/w"},
                        env={"SONARA_HOST_TAB": "tab-7"})
    assert _by_type(msgs, MsgType.SET_FOREGROUND)[0]["host_tab"] == "tab-7"
    assert _by_type(msgs, MsgType.SESSION_START)[0]["host_tab"] == "tab-7"


def test_prompt_submit_foreground_carries_host_tab():
    msgs = handle_event("UserPromptSubmit", {"session_id": "s1"},
                        env={"SONARA_HOST_TAB": "tab-7"})
    assert _by_type(msgs, MsgType.SET_FOREGROUND)[0]["host_tab"] == "tab-7"
    assert "host_tab" not in _by_type(msgs, MsgType.FLUSH)[0]


def test_prism_tab_id_is_accepted_and_generic_name_wins():
    only_prism = handle_event("UserPromptSubmit", {"session_id": "s1"},
                              env={"PRISM_TAB_ID": "p-1"})
    assert only_prism[0]["host_tab"] == "p-1"
    both = handle_event("UserPromptSubmit", {"session_id": "s1"},
                        env={"PRISM_TAB_ID": "p-1", "SONARA_HOST_TAB": "g-2"})
    assert both[0]["host_tab"] == "g-2"


def test_no_host_tab_key_outside_a_host():
    # A plain terminal sends the same messages as before (no new key).
    msgs = handle_event("SessionStart", {"session_id": "s1"}, env={})
    assert all("host_tab" not in m for m in msgs)
    msgs = handle_event("SessionStart", {"session_id": "s1"},
                        env={"SONARA_HOST_TAB": ""})
    assert all("host_tab" not in m for m in msgs)


def test_plugin_version_comes_from_injected_env_not_process(monkeypatch):
    monkeypatch.setenv("CLAUDE_PLUGIN_VERSION", "9.9.9")
    msgs = handle_event("SessionStart", {"session_id": "s1"},
                        env={"CLAUDE_PLUGIN_VERSION": "1.2.3"})
    assert _by_type(msgs, MsgType.SESSION_START)[0]["plugin_version"] == "1.2.3"


def test_handle_event_defaults_to_process_env(monkeypatch):
    monkeypatch.setenv("SONARA_HOST_TAB", "from-os")
    msgs = handle_event("UserPromptSubmit", {"session_id": "s1"})
    assert msgs[0]["host_tab"] == "from-os"


def test_session_manager_keeps_host_tab_until_unregister():
    sm = SessionManager()
    sm.set_foreground("s1")
    assert sm.host_tab("s1") is None
    sm.set_host_tab("s1", "tab-1")
    assert sm.host_tab("s1") == "tab-1"
    sm.set_host_tab("s1", None)          # absent: keep what we know
    assert sm.host_tab("s1") == "tab-1"
    sm.unregister("s1")
    assert sm.host_tab("s1") is None


def test_daemon_stores_host_tab_from_session_start_and_foreground():
    daemon, _q, _sp, sessions, _cfg = make_daemon(foreground=None)
    daemon.handle_message({"v": 1, "type": MsgType.SESSION_START,
                           "session": "s1", "cwd": "", "host_tab": "tab-1"})
    assert sessions.host_tab("s1") == "tab-1"
    daemon.handle_message({"v": 1, "type": MsgType.SET_FOREGROUND,
                           "session": "s1", "host_tab": "tab-2"})
    assert sessions.host_tab("s1") == "tab-2"
    # An older hook (no host_tab) does not erase it.
    daemon.handle_message({"v": 1, "type": MsgType.SET_FOREGROUND,
                           "session": "s1"})
    assert sessions.host_tab("s1") == "tab-2"


def test_daemon_ignores_a_non_string_host_tab():
    daemon, _q, _sp, sessions, _cfg = make_daemon(foreground=None)
    daemon.handle_message({"v": 1, "type": MsgType.SET_FOREGROUND,
                           "session": "s1", "host_tab": ["x"]})
    assert sessions.host_tab("s1") is None


def test_session_end_forgets_host_tab():
    daemon, _q, _sp, sessions, _cfg = make_daemon(foreground=None)
    daemon.handle_message({"v": 1, "type": MsgType.SESSION_START,
                           "session": "s1", "host_tab": "tab-1"})
    daemon.handle_message({"v": 1, "type": MsgType.SESSION_END, "session": "s1"})
    assert sessions.host_tab("s1") is None


def test_webui_sessions_expose_host_tab():
    from sonara import webui
    daemon, _q, _sp, sessions, _cfg = make_daemon(foreground=None)
    daemon.handle_message({"v": 1, "type": MsgType.SESSION_START,
                           "session": "s1", "host_tab": "tab-1"})
    daemon.handle_message({"v": 1, "type": MsgType.SESSION_START,
                           "session": "s2"})
    server = webui.SettingsServer(daemon, token="t", port=0)
    rows = {r["id"]: r for r in server._sessions()}
    assert rows["s1"]["host_tab"] == "tab-1"
    assert rows["s2"]["host_tab"] is None
