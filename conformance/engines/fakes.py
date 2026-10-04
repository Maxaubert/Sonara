"""Fake cloud speech servers (stdlib only) for the external engine
conformance tests, one per provider shape: ElevenLabs, Azure AI Speech and
Google Cloud Text-to-Speech (PR2, #225), Cartesia and Deepgram (PR3, #226), Gemini (#235). Each answers its
synthesis path with raw 16-bit PCM (Google and Gemini: base64 in JSON; Gemini's stream as server-sent
events) when the request carries the expected key in the provider's own header, and the provider's auth
error otherwise; it answers its voice (and model) list paths and keeps every request. The model and voice
ids are made up: Sonara names none of its own (#235). Nothing here calls a real provider."""
from __future__ import annotations

import base64
import json
import struct
import threading
import time
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
    models_path = ""
    models_body: object = None

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
    voices_body = [{"ShortName": "en-US-VoiceZNeural", "LocalName": "Z", "Locale": "en-US"}]

    def refused(self):
        return 401, "text/plain", b""


class Google(Shape):
    kind = "google"
    key_header = "x-goog-api-key"
    synth_prefix = "/v1/text:synthesize"
    voices_path = "/v1/voices"
    voices_body = {"voices": [{"languageCodes": ["en-US"], "name": "en-US-Voice-G"}]}

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
    voices_path = "/v1beta/voices"
    voices_body = {"voices": [{"id": "voice-g", "display_name": "G", "language_code": "en-US",
                               "type": "prebuilt"}]}
    models_path = "/v1beta/models"
    models_body = {"models": [
        {"name": "models/chat-g", "supportedGenerationMethods": ["generateContent"]},
        {"name": "models/tts-g", "displayName": "TTS G", "supportedGenerationMethods": ["generateContent"]}]}

    @staticmethod
    def response(samples: bytes) -> dict:
        part = {"inlineData": {"mimeType": "audio/L16;codec=pcm;rate=24000",
                               "data": base64.b64encode(samples).decode()}}
        return {"candidates": [{"content": {"role": "model", "parts": [part]}, "finishReason": "STOP"}]}

    def ok(self):
        return 200, "application/json", json.dumps(self.response(pcm())).encode()

    def stream(self):
        """`streamGenerateContent?alt=sse`: the audio in two events."""
        audio = pcm()
        half = len(audio) // 2 // 2 * 2
        events = [self.response(audio[:half]), self.response(audio[half:])]
        body = b"".join(b"data: " + json.dumps(e).encode() + b"\r\n\r\n" for e in events)
        return 200, "text/event-stream", body

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
    voices_body = {"stt": [], "tts": [{"name": "d", "canonical_name": "voice-d-en",
                                       "architecture": "arch-d", "languages": ["en"]}]}

    def refused(self):
        body = {"err_code": "INVALID_AUTH", "err_msg": "Invalid credentials.", "request_id": "r"}
        return 401, "application/json", json.dumps(body).encode()


SHAPES = {s.kind: s for s in (ElevenLabs(), Azure(), Google(), Gemini(), Cartesia(), Deepgram())}

# A profile per kind, pointed at the fake server (its url is filled in).
# A profile per kind, pointed at the fake server (its url is filled in): the
# model and voice the user picked from the fake's lists.
PROFILES = {
    "elevenlabs": {"id": "el", "kind": "elevenlabs", "voice": "voice-a"},
    "azure": {"id": "az", "kind": "azure", "voice": "en-US-VoiceZNeural"},
    "google": {"id": "gg", "kind": "google", "voice": "en-US-Voice-G"},
    "gemini": {"id": "ge", "kind": "gemini", "model": "tts-g", "voice": "voice-g"},
    "cartesia": {"id": "ca", "kind": "cartesia", "model": "model-c", "voice": "voice-c"},
    "deepgram": {"id": "dg", "kind": "deepgram", "voice": "voice-d-en"},
}


class FakeCloud:
    """One provider's fake on 127.0.0.1 and an ephemeral port."""

    def __init__(self, kind: str, key: str):
        self.shape = SHAPES[kind]
        self.key = key
        self.requests: list[dict] = []
        self.lock = threading.Lock()
        # `slowly` (#235): raw PCM answers sent piece by piece, and how each
        # such answer ended: (pieces written, the client closed it).
        self.slow: tuple[int, float] | None = None
        self.streams: list[tuple[int, bool]] = []
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
                    slow = outer.slow
                if r["headers"].get(outer.shape.key_header) != outer.shape.key_prefix + key:
                    self._send(*outer.shape.refused())
                elif slow and r["method"] == "POST":
                    self._trickle(*slow)
                else:
                    self._send(*good())

            def _trickle(self, n: int, every: float) -> None:
                """A raw PCM answer made as it is sent: `n` pieces of 0.1 s,
                `every` seconds apart, no Content-Length."""
                self.send_response(200)
                self.send_header("Content-Type", "audio/pcm")
                self.send_header("Connection", "close")
                self.end_headers()
                written, cut = 0, False
                for _ in range(n):
                    try:
                        self.wfile.write(pcm(2400))
                        self.wfile.flush()
                    except OSError:
                        cut = True
                        break
                    written += 1
                    time.sleep(every)
                with outer.lock:
                    outer.streams.append((written, cut))

            def do_GET(self):  # noqa: N802 - http.server API
                r = self._keep(b"")
                path = self.path.split("?")[0]
                if path == outer.shape.voices_path:
                    body = json.dumps(outer.shape.voices_body).encode()
                    self._answer(r, lambda: (200, "application/json", body))
                elif outer.shape.models_path and path == outer.shape.models_path:
                    body = json.dumps(outer.shape.models_body).encode()
                    self._answer(r, lambda: (200, "application/json", body))
                else:
                    self._send(404, "application/json", b'{"detail": "Not Found"}')

            def do_POST(self):  # noqa: N802 - http.server API
                n = int(self.headers.get("Content-Length") or 0)
                r = self._keep(self.rfile.read(n))
                if self.path.startswith(outer.shape.synth_prefix):
                    streamed = ":streamGenerateContent" in self.path
                    self._answer(r, outer.shape.stream if streamed else outer.shape.ok)
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

    def slowly(self, pieces: int, every: float) -> None:
        """From now on speech answers trickle in (`_trickle`)."""
        with self.lock:
            self.slow = (pieces, every)

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
