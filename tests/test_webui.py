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
                       "chatterbox_max_chunk_chars": 280, "chatterbox_exaggeration": 0.0,
                       "chatterbox_variant": "turbo",
                       "settings_port": 0}
        self.sessions = FakeSessions()
        self.messages = []

    def handle_message(self, msg):
        self.messages.append(msg)
        return {"ok": True}

    def set_summary_prompt(self, style, text):
        self.prompt_calls = getattr(self, "prompt_calls", [])
        self.prompt_calls.append((style, text))
        if style not in ("tidy", "natural", "brief"):
            return False
        if text is not None and not str(text).strip():
            return False
        return True


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


def test_set_summary_style_and_command_route_via_config_setter(server, monkeypatch):
    # (#58) chips + engine select go through /api/set -> set_config_value;
    # a _CONFIG_KEYS membership regression must fail here, not in the browser
    d, s = server
    calls = []
    d.set_config_value = lambda k, v: calls.append((k, v)) or True
    for key, value in (("summary_style", "brief"), ("summary_command", "codex")):
        _post(s, "/api/set", {"key": key, "value": value})
        assert (key, value) in calls
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


def test_preview_audio_served_from_file(server, tmp_path, monkeypatch):
    # (#38) previews play INSTANTLY from pre-rendered files
    from sonara import previews
    monkeypatch.setattr(previews, "preview_dir", lambda: tmp_path)
    (tmp_path / "af_heart.wav").write_bytes(b"RIFFfakewav")
    r = _get(s := server[1], "/api/preview-audio?voice=af_heart")
    assert r.status == 200
    assert r.headers["Content-Type"] == "audio/wav"
    assert r.read() == b"RIFFfakewav"


def test_preview_audio_missing_is_404(server, tmp_path, monkeypatch):
    from sonara import previews
    monkeypatch.setattr(previews, "preview_dir", lambda: tmp_path)
    with pytest.raises(urllib.error.HTTPError) as ei:
        _get(server[1], "/api/preview-audio?voice=nope")
    assert ei.value.code == 404


def test_preview_audio_requires_token(server, tmp_path, monkeypatch):
    from sonara import previews
    monkeypatch.setattr(previews, "preview_dir", lambda: tmp_path)
    (tmp_path / "v.wav").write_bytes(b"RIFF")
    with pytest.raises(urllib.error.HTTPError) as ei:
        _get(server[1], "/api/preview-audio?voice=v", token=None)
    assert ei.value.code == 403


def test_state_exposes_valid_keys_and_mods(server, monkeypatch):
    # (#38) the page validates captures client-side against the real keytables
    monkeypatch.setattr(webui, "_key_names", lambda: {"keys": ["m", "z"], "mods": ["ctrl", "alt"]})
    state = json.loads(_get(server[1], "/api/state").read())
    assert state["keys"] == {"keys": ["m", "z"], "mods": ["ctrl", "alt"]}


def test_keymap_junk_types_are_400_not_traceback(server, monkeypatch):
    # (#38) non-string key / non-list mods used to raise TypeError -> dead reply
    def typo(action, key, mods):
        raise TypeError("key must be a string")
    monkeypatch.setattr(webui, "_bind_action", typo)
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(server[1], "/api/keymap", {"action": "mute", "key": 5, "mods": "ctrl"})
    assert ei.value.code == 400


def test_deeply_nested_json_is_400(server):
    body = ("[" * 3000 + "]" * 3000).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{server[1].port}/api/set", data=body,
                                 headers={"X-Sonara-Token": "tok123",
                                          "Content-Type": "application/json"})
    with pytest.raises(urllib.error.HTTPError) as ei:
        urllib.request.urlopen(req, timeout=5)
    assert ei.value.code == 400


def test_windows_group_excludes_neural_duplicates(monkeypatch):
    # (#38) the WinRT voice list includes the neural voices; the page showed
    # every kokoro/chatterbox voice twice
    class V:
        def __init__(self, n): self.display_name = n
    import types
    fake_tts = types.SimpleNamespace(list_voices=lambda: [V("Microsoft Zira"), V("af_heart"), V("poki")])
    fake_platform = types.SimpleNamespace(tts=fake_tts)
    import sonara.platform as plat
    monkeypatch.setattr(plat, "get_platform", lambda: fake_platform)
    import sonara.kokoro as kokoro, sonara.chatterbox as chatterbox
    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(kokoro, "VOICES", ["af_heart"])
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(chatterbox, "list_voices", lambda: ["poki"])
    out = webui._installed_voices()
    assert out["windows"] == ["Microsoft Zira"]
    assert out["kokoro"] == ["af_heart"] and out["chatterbox"] == ["poki"]


def test_variant_is_page_settable(server, monkeypatch):
    # (#42) mode toggle routes through the config setter like the other keys
    d, s = server
    calls = []
    d.set_config_value = lambda k, v: calls.append((k, v)) or True
    _post(s, "/api/set", {"key": "chatterbox_variant", "value": "original"})
    assert calls == [("chatterbox_variant", "original")]
    state = json.loads(_get(s, "/api/state").read())
    assert "chatterbox_variant" in state["config"]


def test_state_exposes_summary_style_engine_and_prompts(server):
    d, s = server
    d.config["summary_style"] = "brief"
    d.config["summary_command"] = "codex"
    d.config["summary_prompts"] = {"brief": "MY RULES"}
    state = s.state()
    assert state["config"]["summary_style"] == "brief"
    assert state["config"]["summary_command"] == "codex"
    assert state["summary_prompts"] == {"brief": "MY RULES"}
    from sonara.summarizer import INSTRUCTIONS
    assert state["summary_prompt_defaults"] == INSTRUCTIONS


def test_api_prompt_sets_and_resets(server):
    d, s = server
    r = _post(s, "/api/prompt", {"style": "natural", "text": "X"})
    assert r.status == 200
    assert d.prompt_calls[-1] == ("natural", "X")
    r = _post(s, "/api/prompt", {"style": "natural", "text": None})
    assert r.status == 200
    assert d.prompt_calls[-1] == ("natural", None)


def test_api_prompt_rejects_bad_input(server):
    d, s = server
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/prompt", {"style": "bogus", "text": "X"})
    assert ei.value.code == 400
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/prompt", {"style": "brief", "text": "   "})
    assert ei.value.code == 400


def test_settings_page_has_fast_cues_switch():
    # (#60) instant control cues get an on/off switch and a voice picker
    from sonara.webui import _page_bytes
    page = _page_bytes().decode("utf-8")
    assert 'id="cues-switch"' in page
    assert '"fast_cues"' in page                    # wired to the config key
    assert 'id="cue-voice-select"' in page
    assert '"cue_voice"' in page


def test_settings_page_has_summary_styles_ui():
    from sonara.webui import _page_bytes
    page = _page_bytes().decode("utf-8")
    assert 'id="summary-seg"' in page          # 4-chip mode segment
    assert 'data-style="off"' in page
    assert 'data-style="tidy"' in page
    assert 'data-style="natural"' in page
    assert 'data-style="brief"' in page
    assert 'id="engine-select"' in page        # summarizer engine picker
    assert 'id="prompt-text"' in page          # editable prompt textarea
    assert 'id="prompt-reset"' in page         # reset to default
    assert 'id="summary-switch"' not in page   # old on/off switch replaced
