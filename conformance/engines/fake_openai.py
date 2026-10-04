"""A fake OpenAI-compatible speech server (stdlib only) for the external
engine conformance tests: ``POST /v1/audio/speech`` answers a WAV (or a
scripted error), ``GET /v1/audio/voices`` a Kokoro-FastAPI voice list, and
every request is kept with its headers and body. It never calls a real
provider."""
from __future__ import annotations

import json
import struct
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def wav(samples: int = 4800, rate: int = 24000, value: int = 3000) -> bytes:
    """A mono 16-bit WAV of ``samples`` samples."""
    data = struct.pack("<h", value) * samples
    return (
        b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVE"
        + b"fmt " + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16)
        + b"data" + struct.pack("<I", len(data)) + data
    )


class FakeOpenAI:
    """The server on 127.0.0.1 and an ephemeral port; ``url`` ends in /v1."""

    def __init__(self):
        self.requests: list[dict] = []
        self.status = 200
        self.content_type = "audio/wav"
        self.body = wav()
        self.lock = threading.Lock()
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):  # quiet
                pass

            def _keep(self, body: bytes) -> None:
                with outer.lock:
                    outer.requests.append({
                        "method": self.command,
                        "path": self.path,
                        "headers": {k.lower(): v for k, v in self.headers.items()},
                        "body": body,
                    })

            def _send(self, status: int, ctype: str, body: bytes) -> None:
                self.send_response(status)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):  # noqa: N802 - http.server API
                self._keep(b"")
                if self.path.startswith("/v1/audio/voices"):
                    self._send(200, "application/json", json.dumps({"voices": ["af_heart", "am_echo"]}).encode())
                else:
                    self._send(404, "application/json", b'{"detail": "Not Found"}')

            def do_POST(self):  # noqa: N802 - http.server API
                n = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(n)
                self._keep(body)
                with outer.lock:
                    status, ctype, answer = outer.status, outer.content_type, outer.body
                self._send(status, ctype, answer)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}/v1"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def fail(self, status: int, error: dict) -> None:
        """Answer every speech request with ``status`` and an OpenAI error."""
        with self.lock:
            self.status = status
            self.content_type = "application/json"
            self.body = json.dumps({"error": error}).encode()

    def speech(self) -> list[dict]:
        with self.lock:
            return [r for r in self.requests if r["path"].endswith("/audio/speech")]

    def stop(self) -> None:
        self.server.shutdown()
        self.server.server_close()
