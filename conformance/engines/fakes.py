"""Fake cloud speech servers (stdlib only) for the external engine
conformance tests, one per provider shape of PR2 (#225): ElevenLabs, Azure AI
Speech and Google Cloud Text-to-Speech. Each answers its synthesis path with
raw 16-bit PCM (Google: base64 in JSON) when the request carries the expected
key in the provider's own header, and the provider's auth error otherwise; it
answers its voice list path and keeps every request. Nothing here calls a
real provider."""
from __future__ import annotations

import base64
import json
import struct
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def pcm(samples: int = 4800, value: int = 3000) -> bytes:
    """Raw mono 16-bit little-endian samples."""
    return struct.pack("<h", value) * samples


class Shape:
    """What one provider expects and answers."""

    kind = ""
    key_header = ""
    synth_prefix = ""
    voices_path = ""
    voices_body: object = None

    def ok(self) -> tuple[int, str, bytes]:
        return 200, "audio/pcm", pcm()

    def refused(self) -> tuple[int, str, bytes]:
        raise NotImplementedError


class ElevenLabs(Shape):
    kind = "elevenlabs"
    key_header = "xi-api-key"
    synth_prefix = "/v1/text-to-speech/"
    voices_path = "/v2/voices"
    voices_body = {"voices": [{"voice_id": "voice-a", "name": "A", "labels": {"language": "en"}}],
                   "has_more": False}

    def refused(self):
        body = {"detail": {"type": "authentication_error", "code": "invalid_api_key",
                           "message": "Invalid API key", "status": "invalid_api_key"}}
        return 401, "application/json", json.dumps(body).encode()


class Azure(Shape):
    kind = "azure"
    key_header = "ocp-apim-subscription-key"
    synth_prefix = "/cognitiveservices/v1"
    voices_path = "/cognitiveservices/voices/list"
    voices_body = [{"ShortName": "en-US-AvaMultilingualNeural", "LocalName": "Ava", "Locale": "en-US"}]

    def refused(self):
        return 401, "text/plain", b""


class Google(Shape):
    kind = "google"
    key_header = "x-goog-api-key"
    synth_prefix = "/v1/text:synthesize"
    voices_path = "/v1/voices"
    voices_body = {"voices": [{"languageCodes": ["en-US"], "name": "en-US-Chirp3-HD-Kore"}]}

    def ok(self):
        body = {"audioContent": base64.b64encode(pcm()).decode()}
        return 200, "application/json", json.dumps(body).encode()

    def refused(self):
        body = {"error": {"code": 400, "message": "API key not valid. Please pass a valid API key.",
                          "status": "INVALID_ARGUMENT",
                          "details": [{"reason": "API_KEY_INVALID", "domain": "googleapis.com"}]}}
        return 400, "application/json", json.dumps(body).encode()


SHAPES = {s.kind: s for s in (ElevenLabs(), Azure(), Google())}

# A profile per kind, pointed at the fake server (its url is filled in).
PROFILES = {
    "elevenlabs": {"id": "el", "kind": "elevenlabs", "voice": "voice-a"},
    "azure": {"id": "az", "kind": "azure", "voice": "en-US-AvaMultilingualNeural"},
    "google": {"id": "gg", "kind": "google", "voice": "en-US-Chirp3-HD-Kore"},
}


class FakeCloud:
    """One provider's fake on 127.0.0.1 and an ephemeral port."""

    def __init__(self, kind: str, key: str):
        self.shape = SHAPES[kind]
        self.key = key
        self.requests: list[dict] = []
        self.lock = threading.Lock()
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):  # quiet
                pass

            def _keep(self, body: bytes) -> dict:
                r = {"method": self.command, "path": self.path,
                     "headers": {k.lower(): v for k, v in self.headers.items()}, "body": body}
                with outer.lock:
                    outer.requests.append(r)
                return r

            def _send(self, status: int, ctype: str, body: bytes) -> None:
                self.send_response(status)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)

            def _answer(self, r: dict, good) -> None:
                with outer.lock:
                    key = outer.key
                if r["headers"].get(outer.shape.key_header) != key:
                    self._send(*outer.shape.refused())
                else:
                    self._send(*good())

            def do_GET(self):  # noqa: N802 - http.server API
                r = self._keep(b"")
                if self.path.split("?")[0] == outer.shape.voices_path:
                    body = json.dumps(outer.shape.voices_body).encode()
                    self._answer(r, lambda: (200, "application/json", body))
                else:
                    self._send(404, "application/json", b'{"detail": "Not Found"}')

            def do_POST(self):  # noqa: N802 - http.server API
                n = int(self.headers.get("Content-Length") or 0)
                r = self._keep(self.rfile.read(n))
                if self.path.startswith(outer.shape.synth_prefix):
                    self._answer(r, outer.shape.ok)
                else:
                    self._send(404, "application/json", b'{"detail": "Not Found"}')

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def profile(self) -> dict:
        p = dict(PROFILES[self.shape.kind], url=self.url)
        p["options"] = {"timeout_ms": 5000}
        return p

    def refuse_key(self) -> None:
        """From now on the key Sonara holds is wrong."""
        with self.lock:
            self.key = "a-different-key"

    def speech(self) -> list[dict]:
        with self.lock:
            return [r for r in self.requests if r["method"] == "POST"]

    def stop(self) -> None:
        self.server.shutdown()
        self.server.server_close()
