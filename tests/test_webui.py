"""Settings page HTTP server (#34): auth, state, dispatch. Uses a FakeDaemon --
never a real daemon, never the live ~/.sonara."""
import json
import urllib.request
import urllib.error

import pytest

from sonara import webui


class FakeSessions:
    def foreground(self):
        return "sess-1"


class FakeDaemon:
    def __init__(self):
        self.config = {"voice": "af_heart", "rate": 250, "minqueue": 5,
                       "summary_mode": True, "summary_model": "haiku",
                       "summary_timeout": 60, "summary_settle_ms": 600,
                       "audio_control": False, "duck_level": 20,
                       "chatterbox_max_chunk_chars": 280, "settings_port": 0}
        self.sessions = FakeSessions()
        self.messages = []

    def handle_message(self, msg):
        self.messages.append(msg)
        return {"ok": True}


@pytest.fixture()
def server(monkeypatch):
    monkeypatch.setattr(webui, "_installed_voices", lambda: {
        "windows": ["Microsoft Zira"], "kokoro": ["af_heart"],
        "chatterbox": ["cb_default"]})
    monkeypatch.setattr(webui, "_engine_status", lambda: {
        "kokoro": True, "chatterbox": True})
    monkeypatch.setattr(webui, "_keymap_state", lambda: [
        {"action": "mute", "key": "m", "mods": ["ctrl", "alt"]}])
    d = FakeDaemon()
    s = webui.SettingsServer(d, token="tok123", port=0)  # 0 = ephemeral for tests
    s.start()
    yield d, s
    s.stop()


def _get(s, path, token="tok123"):
    req = urllib.request.Request(f"http://127.0.0.1:{s.port}{path}")
    if token:
        req.add_header("X-Sonara-Token", token)
    return urllib.request.urlopen(req, timeout=5)


def test_missing_token_is_403(server):
    d, s = server
    with pytest.raises(urllib.error.HTTPError) as ei:
        _get(s, "/api/state", token=None)
    assert ei.value.code == 403


def test_wrong_token_is_403(server):
    d, s = server
    with pytest.raises(urllib.error.HTTPError) as ei:
        _get(s, "/api/state", token="nope")
    assert ei.value.code == 403


def test_token_via_query_param_works(server):
    d, s = server
    r = urllib.request.urlopen(
        f"http://127.0.0.1:{s.port}/api/state?token=tok123", timeout=5)
    assert r.status == 200


def test_state_shape(server):
    d, s = server
    state = json.loads(_get(s, "/api/state").read())
    assert state["config"]["voice"] == "af_heart"
    assert "verbosity" not in state["config"]          # NOT on the page
    assert state["voices"]["kokoro"] == ["af_heart"]
    assert state["engines"] == {"kokoro": True, "chatterbox": True}
    assert state["keymap"][0]["action"] == "mute"
    assert state["daemon"]["foreground"] == "sess-1"
    assert isinstance(state["daemon"]["pid"], int)
    assert isinstance(state["daemon"]["port"], int)


def _post(s, path, obj, token="tok123"):
    body = json.dumps(obj).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{s.port}{path}", data=body,
                                 headers={"X-Sonara-Token": token,
                                          "Content-Type": "application/json"})
    return urllib.request.urlopen(req, timeout=5)


def test_set_message_backed_key_dispatches(server):
    d, s = server
    r = _post(s, "/api/set", {"key": "rate", "value": 220})
    assert r.status == 200
    assert d.messages[-1]["type"] == "set_rate"
    assert d.messages[-1]["rate"] == 220


def test_set_summary_mode_uses_enabled_field(server):
    d, s = server
    _post(s, "/api/set", {"key": "summary_mode", "value": False})
    assert d.messages[-1] == {"v": 1, "type": "set_summary_mode", "enabled": False}


def test_set_config_only_key_uses_daemon_setter(server, monkeypatch):
    d, s = server
    calls = []
    d.set_config_value = lambda k, v: calls.append((k, v)) or True
    _post(s, "/api/set", {"key": "summary_settle_ms", "value": 800})
    assert calls == [("summary_settle_ms", 800)]
    assert d.messages == []                       # no protocol message for these


def test_set_unknown_key_is_400(server):
    d, s = server
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/set", {"key": "verbosity", "value": "quiet"})
    assert ei.value.code == 400


def test_set_dispatches_under_daemon_lock(server):
    # the daemon requires handle_message under its lock (same guard as the
    # socket + hotkey entry points); HTTP threads must honor it too
    import threading
    d, s = server
    d._lock = threading.Lock()
    held = []
    orig = d.handle_message
    def guarded(msg):
        held.append(d._lock.locked())
        return orig(msg)
    d.handle_message = guarded
    _post(s, "/api/set", {"key": "rate", "value": 200})
    assert held == [True]


def test_non_object_json_body_is_400(server):
    d, s = server
    body = b"5"
    req = urllib.request.Request(f"http://127.0.0.1:{s.port}/api/set", data=body,
                                 headers={"X-Sonara-Token": "tok123",
                                          "Content-Type": "application/json"})
    with pytest.raises(urllib.error.HTTPError) as ei:
        urllib.request.urlopen(req, timeout=5)
    assert ei.value.code == 400


def test_keymap_bind_writes_and_reloads(server, monkeypatch):
    d, s = server
    bound = []
    monkeypatch.setattr(webui, "_bind_action", lambda a, k, m: bound.append((a, k, m)))
    r = _post(s, "/api/keymap", {"action": "mute", "key": "k", "mods": ["ctrl", "alt"]})
    assert r.status == 200
    assert bound == [("mute", "k", ["ctrl", "alt"])]
    assert d.messages[-1]["type"] == "reload_keymap"


def test_keymap_unbind(server, monkeypatch):
    d, s = server
    unbound = []
    monkeypatch.setattr(webui, "_unbind_action", lambda a: unbound.append(a))
    _post(s, "/api/keymap", {"action": "mute", "unbind": True})
    assert unbound == ["mute"]
    assert d.messages[-1]["type"] == "reload_keymap"


def test_keymap_bad_action_is_400(server, monkeypatch):
    d, s = server
    def boom(a, k, m):
        raise ValueError("unknown action")
    monkeypatch.setattr(webui, "_bind_action", boom)
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/keymap", {"action": "warp", "key": "w", "mods": []})
    assert ei.value.code == 400


def test_preview_endpoint(server):
    d, s = server
    d.preview_voice = lambda v: v == "af_heart"
    r = _post(s, "/api/preview", {"voice": "af_heart"})
    assert r.status == 202
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/preview", {"voice": "busy_voice"})
    assert ei.value.code == 409


def test_daemon_endpoint_restart_and_shutdown(server):
    d, s = server
    _post(s, "/api/daemon", {"op": "restart"})
    assert d.messages[-1] == {"v": 1, "type": "shutdown"}
    _post(s, "/api/daemon", {"op": "shutdown"})
    assert d.messages[-1] == {"v": 1, "type": "shutdown", "stay_down": True}
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/daemon", {"op": "reboot-the-moon"})
    assert ei.value.code == 400


def test_settings_page_served_and_self_contained(server):
    d, s = server
    html = _get(s, "/settings").read().decode("utf-8")
    assert "Sonara" in html and "offline-banner" in html
    assert "http://" not in html.replace(f"http://127.0.0.1", "")  # no external refs
    assert "https://" not in html
    # / redirects or serves too
    assert _get(s, "/").status == 200


def test_settings_page_requires_token(server):
    d, s = server
    with pytest.raises(urllib.error.HTTPError) as ei:
        _get(s, "/settings", token=None)
    assert ei.value.code == 403
