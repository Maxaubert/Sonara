"""``client.engines`` (protocol 1.2): the request shapes against the fake
runtime, and end to end against a real sonarad (``--engine fake --keys
fake``) with a local fake OpenAI-compatible server."""
from __future__ import annotations

import json
import struct
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
from conftest import kill, read_json, wait_until

from sonara_client import Engines, SonaraError, connect
from sonara_client.discovery import pid_alive

SECRET = "sk-py-client-secret-0123456789"


def _sent(fake, skip=1):
    return [{k: v for k, v in r.items() if k != "id"} for r in fake.requests()[skip:]]


def test_engine_messages_have_the_wire_shape(home, fake):
    f = fake()
    profile = {"id": "kgpu", "kind": "openai-compatible", "url": "http://127.0.0.1:8880/v1",
               "options": {"preset": "kokoro-fastapi"}}
    with connect("unit", home=str(home), autostart=False) as c:
        assert isinstance(c.engines, Engines)
        c.engines.list()
        c.engines.add(profile)
        c.engines.add(profile, secret="sk-x", replace=True)
        c.engines.set_key("kgpu", "sk-y")
        c.engines.set_key("kgpu", None)
        c.engines.test("kgpu")
        c.engines.test("kgpu", text="Hi.", voice="af_sky", play=False)
        c.engines.voices("kgpu", refresh=True)
        c.voices("kgpu", refresh=True)
        c.engines.models("kgpu", refresh=True)
        c.engines.models(profile={"kind": "gemini"}, secret="k-1")
        c.engines.remove("kgpu")
        c.engines.remove("kgpu", forget_key=False)
        c.engines.reload()
    assert _sent(f) == [
        {"type": "engine_list"},
        {"type": "engine_add", "engine": profile},
        {"type": "engine_add", "engine": profile, "secret": "sk-x", "replace": True},
        {"type": "engine_key", "engine": "kgpu", "secret": "sk-y"},
        {"type": "engine_key", "engine": "kgpu", "secret": None},
        {"type": "engine_test", "engine": "kgpu"},
        {"type": "engine_test", "engine": "kgpu", "text": "Hi.", "voice": "af_sky", "play": False},
        {"type": "voices", "engine": "kgpu", "refresh": True},
        {"type": "voices", "engine": "kgpu", "refresh": True},
        {"type": "engine_models", "engine": "kgpu", "refresh": True},
        {"type": "engine_models", "profile": {"kind": "gemini"}, "secret": "k-1"},
        {"type": "engine_remove", "engine": "kgpu"},
        {"type": "engine_remove", "engine": "kgpu", "forget_key": False},
        {"type": "engine_reload"},
    ]


def _wav(samples=4800, rate=24000):
    data = struct.pack("<h", 2000) * samples
    return (b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVE" + b"fmt "
            + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16)
            + b"data" + struct.pack("<I", len(data)) + data)


@pytest.fixture
def provider():
    """A fake provider: POST /v1/audio/speech answers ``state``."""
    seen = []
    state = {"status": 200, "type": "audio/wav", "body": _wav()}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def _answer(self, status, ctype, body):
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):  # noqa: N802
            seen.append((self.path, dict(self.headers), b""))
            self._answer(200, "application/json", json.dumps({"voices": ["af_heart"]}).encode())

        def do_POST(self):  # noqa: N802
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            seen.append((self.path, {k.lower(): v for k, v in self.headers.items()}, body))
            self._answer(state["status"], state["type"], state["body"])

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    server.url = f"http://127.0.0.1:{server.server_address[1]}/v1"
    server.seen = seen
    server.state = state
    yield server
    server.shutdown()
    server.server_close()


def test_add_test_and_remove_against_sonarad(home, sonarad, provider):
    c = connect("py-engines", home=str(home), runtime_path=str(sonarad),
                runtime_args=["--engine", "fake", "--keys", "fake", "--idle-exit", "5"])
    try:
        assert "engines" in c.info["capabilities"]
        profile = {"id": "local", "kind": "openai-compatible", "url": provider.url,
                   "key_ref": "credman", "voice": "af_heart",
                   "options": {"preset": "kokoro-fastapi", "timeout_ms": 5000}}
        added = c.engines.add(profile, secret=SECRET)
        assert added["engine"]["key_present"] is True
        assert SECRET not in json.dumps(added)
        t = c.engines.test("local", play=False)
        assert t["sample_rate"] == 24000
        speech = [s for s in provider.seen if s[0].endswith("/audio/speech")]
        assert speech[0][1]["authorization"] == f"Bearer {SECRET}"
        voices = c.voices("local", refresh=True)
        assert [v["id"] for v in voices] == ["af_heart"]
        provider.state.update(status=401, type="application/json",
                              body=json.dumps({"error": {"message": "Incorrect API key"}}).encode())
        with pytest.raises(SonaraError) as e:
            c.engines.test("local", play=False)
        assert e.value.code == "E_ENGINE" and e.value.reason == "auth"
        assert c.engines.remove("local")["removed"] == "local"
        assert c.engines.list()["engines"] == []
        # A program is never added through the SDK (protocol 1.3).
        program = {"id": "prog", "kind": "command",
                   "options": {"argv": [sys.executable, "-c", "pass"]}}
        with pytest.raises(SonaraError) as e:
            c.engines.add(program)
        assert e.value.code == "E_FORBIDDEN"
        assert c.engines.reload()["engines"] == []
    finally:
        pid = c.runtime["pid"]
        c.close()
        info = read_json(home / "runtime.json")
        for p in {pid, (info or {}).get("pid", pid)}:
            kill(p)
        wait_until(lambda: not pid_alive(pid), 5)
