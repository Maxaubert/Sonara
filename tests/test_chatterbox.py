import json

from sonara import chatterbox as cb


# --- registry ---------------------------------------------------------------

def _reg(monkeypatch, tmp_path):
    d = tmp_path / "voices"
    d.mkdir()
    monkeypatch.setattr(cb, "CHATTERBOX_VOICES_DIR", d)
    return d


def test_list_voices_always_has_default(monkeypatch, tmp_path):
    _reg(monkeypatch, tmp_path)
    assert cb.list_voices() == ["cb_default"]


def test_list_voices_discovers_clips(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "calm-lady.wav").write_bytes(b"RIFF")
    (d / "deep.wav").write_bytes(b"RIFF")
    assert cb.list_voices() == ["cb_default", "calm-lady", "deep"]


def test_cb_default_clip_does_not_duplicate(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "cb_default.wav").write_bytes(b"RIFF")
    assert cb.list_voices().count("cb_default") == 1


def test_is_chatterbox_voice_forms(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "calm-lady.wav").write_bytes(b"RIFF")
    assert cb.is_chatterbox_voice("cb_default")
    assert cb.is_chatterbox_voice("calm-lady")
    assert cb.is_chatterbox_voice("chatterbox:calm-lady")
    assert not cb.is_chatterbox_voice("af_heart")
    assert not cb.is_chatterbox_voice(None)


def test_voice_spec_reads_sidecar_and_defaults(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "deep.wav").write_bytes(b"RIFF")
    (d / "deep.json").write_text(json.dumps(
        {"variant": "original", "exaggeration": 0.8}), encoding="utf-8")
    cfg = {"chatterbox_variant": "turbo"}
    spec = cb.voice_spec("deep", cfg)
    assert spec["variant"] == "original" and spec["exaggeration"] == 0.8
    assert spec["voice_path"].endswith("deep.wav")
    default = cb.voice_spec("cb_default", cfg)
    assert default == {"voice_path": None, "variant": "turbo", "exaggeration": None}


def test_voice_spec_tolerates_non_dict_sidecar(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "odd.wav").write_bytes(b"RIFF")
    (d / "odd.json").write_text("0.8", encoding="utf-8")
    spec = cb.voice_spec("odd", {"chatterbox_variant": "turbo"})
    assert spec["variant"] == "turbo" and spec["exaggeration"] is None


# --- VRAM gate ---------------------------------------------------------------

def test_gate_reads_nvidia_smi():
    run = lambda argv, **kw: "11342\n"
    assert cb.free_vram_gb(run=run) > 11
    assert cb.gate_ok({"chatterbox_min_free_vram_gb": 5}, run=run) is True
    assert cb.gate_ok({"chatterbox_min_free_vram_gb": 12}, run=run) is False


def test_gate_passes_when_smi_missing():
    def boom(argv, **kw):
        raise FileNotFoundError("nvidia-smi")
    assert cb.free_vram_gb(run=boom) is None
    assert cb.gate_ok({"chatterbox_min_free_vram_gb": 5}, run=boom) is True


def test_gate_threshold_zero_always_true():
    assert cb.gate_ok({"chatterbox_min_free_vram_gb": 0},
                      run=lambda a, **k: "1\n") is True


# --- client against a scripted fake worker -----------------------------------

FAKE_WORKER = r'''
import base64, json, sys
for line in sys.stdin:
    req = json.loads(line)
    if req["type"] == "ping":
        print(json.dumps({"ok": True, "loaded": False})); sys.stdout.flush()
    elif req["text"] == "die":
        sys.exit(1)
    elif req["text"] == "fail":
        print(json.dumps({"ok": False, "error": "synthetic"})); sys.stdout.flush()
    else:
        print(json.dumps({"ok": True,
                          "wav_b64": base64.b64encode(b"RIFFfake").decode()}))
        sys.stdout.flush()
'''


def _client(tmp_path, monkeypatch, timeout=5):
    script = tmp_path / "fake_worker.py"
    script.write_text(FAKE_WORKER, encoding="utf-8")
    import sys
    monkeypatch.setattr(cb, "chatterbox_venv_python", lambda: sys.executable)
    monkeypatch.setattr(cb, "worker_script_path", lambda: str(script))
    monkeypatch.setattr(cb, "CHATTERBOX_VOICES_DIR", tmp_path)  # empty registry
    return cb.ChatterboxClient()


def test_client_synth_returns_wav_bytes(tmp_path, monkeypatch):
    c = _client(tmp_path, monkeypatch)
    out = c.synth_wav("hello", "cb_default", {"chatterbox_timeout": 5})
    assert out == b"RIFFfake"


def test_client_error_raises_chatterbox_error(tmp_path, monkeypatch):
    c = _client(tmp_path, monkeypatch)
    import pytest
    with pytest.raises(cb.ChatterboxError):
        c.synth_wav("fail", "cb_default", {"chatterbox_timeout": 5})


def test_client_respawns_once_after_dead_worker(tmp_path, monkeypatch):
    c = _client(tmp_path, monkeypatch)
    import pytest
    with pytest.raises(cb.ChatterboxError):
        c.synth_wav("die", "cb_default", {"chatterbox_timeout": 5})
    # worker died; next call must respawn and succeed
    assert c.synth_wav("hello", "cb_default", {"chatterbox_timeout": 5}) == b"RIFFfake"


def test_fallback_notice_pops_once():
    cb._set_fallback_notice("no vram")
    assert cb.pop_fallback_notice() == "no vram"
    assert cb.pop_fallback_notice() is None


def test_spawn_isolates_path_and_strips_pythonpath(monkeypatch):
    # Live bug: the worker inherited PYTHONPATH pointing at the sonara package
    # and Python prepended the worker's own dir, so `import platform` grabbed
    # sonara's platform/ subpackage -> platform.machine() AttributeError. Spawn
    # with -P (isolated sys.path) and no PYTHONPATH so only stdlib/venv win.
    captured = {}

    class FakeProc:
        stdin = None
        stdout = None
        def poll(self):
            return None

    def fake_popen(argv, **kw):
        captured["argv"] = argv
        captured["env"] = kw.get("env")
        return FakeProc()

    monkeypatch.setattr(cb.subprocess, "Popen", fake_popen)
    monkeypatch.setattr(cb, "chatterbox_venv_python", lambda: "PY.exe")
    monkeypatch.setattr(cb, "worker_script_path", lambda: "W.py")
    monkeypatch.setenv("PYTHONPATH", "C:/whatever/sonara")
    cb.ChatterboxClient()._spawn({"chatterbox_idle_unload_s": 600})
    assert captured["argv"][0] == "PY.exe"
    assert "-P" in captured["argv"] and captured["argv"].index("-P") == 1
    assert "W.py" in captured["argv"]
    assert "PYTHONPATH" not in captured["env"]
    assert captured["env"]["HF_HOME"]


def test_warm_sends_warm_request_and_reports_ok(tmp_path, monkeypatch):
    c = _client(tmp_path, monkeypatch)
    # extend the fake worker to answer warm; _client's FAKE_WORKER answers ping
    # and synth. Drive warm via a direct request to confirm the method shape.
    import sys
    calls = []
    monkeypatch.setattr(c, "_request",
                        lambda payload, timeout, config: calls.append((payload, timeout)) or {"ok": True, "loaded": True})
    assert c.warm({"chatterbox_variant": "turbo", "chatterbox_warm_timeout": 90}) is True
    assert calls[0][0] == {"type": "warm", "variant": "turbo"}
    assert calls[0][1] == 90


def test_warm_returns_false_on_error(tmp_path, monkeypatch):
    c = _client(tmp_path, monkeypatch)
    def boom(payload, timeout, config):
        raise cb.ChatterboxError("dead")
    monkeypatch.setattr(c, "_request", boom)
    assert c.warm({"chatterbox_variant": "turbo"}) is False
