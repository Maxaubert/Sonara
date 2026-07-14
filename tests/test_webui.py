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
