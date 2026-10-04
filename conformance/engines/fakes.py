"""Fake cloud speech servers (stdlib only) for the external engine
conformance tests, one per provider shape: ElevenLabs, Azure AI Speech and
Google Cloud Text-to-Speech (PR2, #225), Cartesia and Deepgram (PR3, #226), Gemini (#235). Each answers its
synthesis path with raw 16-bit PCM (Google and Gemini: base64 in JSON) when the request carries the expected
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
    """Raw mono 16-bit little-endian samples. The first is -1 (FF FF, an MP3
    frame sync), as near-silent neural speech often starts."""
    return struct.pack("<h", -1) + struct.pack("<h", value) * (samples - 1)


class Shape:
    """What one provider expects and answers."""

    kind = ""
    key_header = ""
    # The key's value in that header: the key after this prefix.
    key_prefix = ""
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


class Gemini(Shape):
    kind = "gemini"
    key_header = "x-goog-api-key"
    synth_prefix = "/v1beta/models/"
    # A fixed list of prebuilt voices: Sonara never asks for it.
    voices_path = "/never-asked"

    def ok(self):
        part = {"inlineData": {"mimeType": "audio/L16;codec=pcm;rate=24000",
                               "data": base64.b64encode(pcm()).decode()}}
        body = {"candidates": [{"content": {"role": "model", "parts": [part]}, "finishReason": "STOP"}]}
        return 200, "application/json", json.dumps(body).encode()

    def refused(self):
        body = {"error": {"code": 400, "message": "API key not valid. Please pass a valid API key.",
                          "status": "INVALID_ARGUMENT",
                          "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo",
                                       "reason": "API_KEY_INVALID", "domain": "googleapis.com"}]}}
        return 400, "application/json", json.dumps(body).encode()


class Cartesia(Shape):
    kind = "cartesia"
    key_header = "authorization"
    key_prefix = "Bearer "
    synth_prefix = "/tts/bytes"
    voices_path = "/voices"
    voices_body = {"data": [{"id": "voice-c", "name": "C", "language": "en"}], "has_more": False}

    def refused(self):
        body = {"error_code": "unauthorized", "title": "Unauthorized",
                "message": "Invalid API key", "request_id": "r"}
        return 401, "application/json", json.dumps(body).encode()


class Deepgram(Shape):
    kind = "deepgram"
    key_header = "authorization"
    key_prefix = "Token "
    synth_prefix = "/v1/speak"
    voices_path = "/v1/models"
    voices_body = {"stt": [], "tts": [{"name": "thalia", "canonical_name": "aura-2-thalia-en",
                                       "architecture": "aura-2", "languages": ["en"]}]}

    def refused(self):
        body = {"err_code": "INVALID_AUTH", "err_msg": "Invalid credentials.", "request_id": "r"}
        return 401, "application/json", json.dumps(body).encode()


SHAPES = {s.kind: s for s in (ElevenLabs(), Azure(), Google(), Gemini(), Cartesia(), Deepgram())}

# A profile per kind, pointed at the fake server (its url is filled in).
PROFILES = {
    "elevenlabs": {"id": "el", "kind": "elevenlabs", "voice": "voice-a"},
    "azure": {"id": "az", "kind": "azure", "voice": "en-US-AvaMultilingualNeural"},
    "google": {"id": "gg", "kind": "google", "voice": "en-US-Chirp3-HD-Kore"},
    "gemini": {"id": "ge", "kind": "gemini"},
    "cartesia": {"id": "ca", "kind": "cartesia", "voice": "voice-c"},
    "deepgram": {"id": "dg", "kind": "deepgram", "voice": "aura-2-thalia-en"},
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
                if r["headers"].get(outer.shape.key_header) != outer.shape.key_prefix + key:
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
