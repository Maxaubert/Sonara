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
    assert cb.list_voices() == []          # cb_default no longer advertised (#42)


def test_list_voices_discovers_clips(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "calm-lady.wav").write_bytes(b"RIFF")
    (d / "deep.wav").write_bytes(b"RIFF")
    assert cb.list_voices() == ["calm-lady", "deep"]


def test_cb_default_still_resolves_as_a_chatterbox_voice():
    # removed from the LIST, but a config that still says cb_default must keep
    # working (no-clip default synthesis)
    assert cb.is_chatterbox_voice("cb_default")
    spec = cb.voice_spec("cb_default", {"chatterbox_variant": "turbo"})
    assert spec["voice_path"] is None


def test_cb_default_clip_does_not_duplicate(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "cb_default.wav").write_bytes(b"RIFF")
    assert cb.list_voices().count("cb_default") == 1   # clip named cb_default lists once


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
    elif req["text"] == "cudafail":
        print(json.dumps({"ok": False,
                          "error": "AcceleratorError: CUDA error: unknown error"}))
        sys.stdout.flush()
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


def test_synth_timeout_fallback_single_sourced(tmp_path, monkeypatch):
    # The fallback literal in synth_wav drifted from config DEFAULTS (120 vs 30,
    # audit #19). The fallback must come FROM DEFAULTS so it cannot drift again.
    import base64
    from sonara.config import DEFAULTS
    c = _client(tmp_path, monkeypatch)
    seen = {}

    def fake_request(payload, timeout, config):
        seen["timeout"] = timeout
        return {"ok": True, "wav_b64": base64.b64encode(b"RIFFfake").decode()}
    monkeypatch.setattr(c, "_request", fake_request)
    c.synth_wav("timeout probe", "cb_default", {})   # config missing the key
    assert seen["timeout"] == DEFAULTS["chatterbox_timeout"]


def test_spawn_failure_raises_chatterbox_error(monkeypatch):
    # A raw OSError from Popen bypassed the per-chunk Kokoro fallback (which
    # catches only ChatterboxError), silently dropping the utterance while
    # reporting success (audit #19). Every spawn failure must funnel into
    # ChatterboxError.
    import pytest

    def boom(*a, **k):
        raise OSError("venv python missing")
    monkeypatch.setattr(cb.subprocess, "Popen", boom)
    with pytest.raises(cb.ChatterboxError):
        cb.ChatterboxClient()._spawn({"chatterbox_idle_unload_s": 600})


def test_cooldown_after_worker_failure_fails_fast(monkeypatch):
    # After a worker failure, further requests must fail FAST for a while
    # (cooldown) so the per-chunk Kokoro fallback fires immediately, instead of
    # re-paying spawn+timeout for EVERY chunk of every utterance (audit #21).
    import pytest
    monkeypatch.setattr(cb, "chatterbox_venv_python", lambda: "missing-python.exe")
    monkeypatch.setattr(cb, "worker_script_path", lambda: "worker.py")
    c = cb.ChatterboxClient()
    with pytest.raises(cb.ChatterboxError):
        c._request({"type": "ping"}, 1, {})          # spawn fails -> cooldown armed
    spawns = []
    monkeypatch.setattr(c, "_spawn", lambda config: spawns.append(1))
    with pytest.raises(cb.ChatterboxError):
        c._request({"type": "ping"}, 1, {})          # cooldown: no new spawn attempt
    assert spawns == []


def test_cooldown_expires_and_success_resets_it(tmp_path, monkeypatch):
    import time
    c = _client(tmp_path, monkeypatch)
    c._cooldown_until = time.monotonic() - 1         # expired cooldown
    # unique text: the module-level synth cache must not satisfy this (a cache
    # hit would bypass _request and never clear the memo)
    out = c.synth_wav("cooldown reset probe", "cb_default", {"chatterbox_timeout": 5})
    assert out == b"RIFFfake"                        # request went through
    assert c._cooldown_until == 0.0                  # success cleared the memo


def test_fatal_cuda_error_kills_worker_and_arms_cooldown(tmp_path, monkeypatch):
    # A worker whose CUDA context died (driver swap/reset) stays alive on the
    # pipe and answers EVERY synth with "CUDA error: unknown error" forever.
    # The dead-pipe respawn never fires, so without this the daemon is stuck on
    # the Kokoro fallback until restart (verified live, 2026-07-20).
    import time
    import pytest
    c = _client(tmp_path, monkeypatch)
    with pytest.raises(cb.ChatterboxError):
        c.synth_wav("cudafail", "cb_default", {"chatterbox_timeout": 5})
    assert c._proc is None                           # poisoned worker was killed
    assert c._cooldown_until > time.monotonic()      # rest of utterance fails fast


def test_fatal_cuda_error_recovers_with_fresh_worker(tmp_path, monkeypatch):
    import time
    import pytest
    c = _client(tmp_path, monkeypatch)
    with pytest.raises(cb.ChatterboxError):
        c.synth_wav("cudafail", "cb_default", {"chatterbox_timeout": 5})
    c._cooldown_until = time.monotonic() - 1         # cooldown elapsed
    out = c.synth_wav("post-cuda recovery probe", "cb_default",
                      {"chatterbox_timeout": 5})
    assert out == b"RIFFfake"                        # fresh spawn, healthy again


def test_ordinary_synth_error_keeps_worker_alive(tmp_path, monkeypatch):
    # A per-text failure (bad input, transient OOM) must NOT kill the warm
    # worker: only poisoned-process errors get the kill treatment.
    import pytest
    c = _client(tmp_path, monkeypatch)
    with pytest.raises(cb.ChatterboxError):
        c.synth_wav("fail", "cb_default", {"chatterbox_timeout": 5})
    assert c._proc is not None and c._proc.poll() is None
    assert c._cooldown_until == 0.0


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


def test_voice_spec_exaggeration_falls_back_to_config():
    # (#38) the settings-page slider sets a global chatterbox_exaggeration;
    # a voice sidecar still overrides it
    from sonara import chatterbox
    spec = chatterbox.voice_spec("cb_default", {"chatterbox_variant": "turbo",
                                                "chatterbox_exaggeration": 0.6})
    assert spec["exaggeration"] == 0.6


def test_voice_spec_sidecar_overrides_config_exaggeration(tmp_path, monkeypatch):
    from sonara import chatterbox
    monkeypatch.setattr(chatterbox, "CHATTERBOX_VOICES_DIR", str(tmp_path))
    (tmp_path / "poki.wav").write_bytes(b"RIFF")
    (tmp_path / "poki.json").write_text('{"exaggeration": 0.9}')
    spec = chatterbox.voice_spec("poki", {"chatterbox_exaggeration": 0.2})
    assert spec["exaggeration"] == 0.9                    # sidecar wins
