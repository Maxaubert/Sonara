"""Chatterbox synth cache: re-reading a digest (summary-mode Up) speaks the exact
same text, so the rendered WAV is cached per (variant, voice, exaggeration, text)
and replayed byte-identically instead of regenerating (~2s + non-deterministic
intonation) each time."""
import base64

import sonara.chatterbox as cb


def _fake_request_counter(calls):
    def fake_request(payload, timeout, config):
        calls.append(payload["text"])
        return {"ok": True, "wav_b64": base64.b64encode(b"WAV:" + payload["text"].encode()).decode()}
    return fake_request


def test_synth_wav_caches_repeated_text(monkeypatch):
    cb._SYNTH_CACHE.clear()
    c = cb.ChatterboxClient()
    calls = []
    monkeypatch.setattr(c, "_request", _fake_request_counter(calls))
    cfg = {"chatterbox_timeout": 5}
    a = c.synth_wav("hello world", "cb_default", cfg)
    b = c.synth_wav("hello world", "cb_default", cfg)   # identical -> cache hit
    assert a == b == b"WAV:hello world"
    assert calls == ["hello world"]                      # worker invoked once


def test_synth_wav_different_text_is_not_cached(monkeypatch):
    cb._SYNTH_CACHE.clear()
    c = cb.ChatterboxClient()
    calls = []
    monkeypatch.setattr(c, "_request", _fake_request_counter(calls))
    cfg = {"chatterbox_timeout": 5}
    c.synth_wav("one", "cb_default", cfg)
    c.synth_wav("two", "cb_default", cfg)
    assert calls == ["one", "two"]                       # distinct text -> both synth


def test_synth_cache_is_bounded(monkeypatch):
    cb._SYNTH_CACHE.clear()
    c = cb.ChatterboxClient()
    monkeypatch.setattr(c, "_request", _fake_request_counter([]))
    cfg = {"chatterbox_timeout": 5}
    for i in range(cb._SYNTH_CACHE_MAX + 20):
        c.synth_wav("chunk %d" % i, "cb_default", cfg)
    assert len(cb._SYNTH_CACHE) <= cb._SYNTH_CACHE_MAX   # LRU cap holds
