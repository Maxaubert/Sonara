"""Settings page HTTP server (#34).

A ThreadingHTTPServer on 127.0.0.1 serving the settings page and a tiny JSON
API. Every request must carry the daemon.lock token (?token= or X-Sonara-Token)
or gets 403 -- this blocks other local users and web pages (CSRF/DNS-rebind).
All mutations dispatch through daemon.handle_message / the keymap module, the
exact paths the CLI uses, so the page cannot drift from CLI behavior.
"""
from __future__ import annotations

import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

# config keys the page may read and write (verbosity deliberately absent)
_PAGE_KEYS = (
    "voice", "rate", "minqueue", "summary_mode", "summary_model",
    "summary_timeout", "summary_settle_ms", "audio_control", "duck_level",
    "chatterbox_max_chunk_chars",
)


def _installed_voices() -> dict:
    """Voices grouped by engine. Lazy imports; each group degrades to []."""
    from sonara import kokoro, chatterbox
    out = {"windows": [], "kokoro": [], "chatterbox": []}
    try:
        from sonara.platform import get_platform
        for v in get_platform().tts.list_voices():
            name = getattr(v, "display_name", None) or str(v)
            out["windows"].append(name)
    except Exception:  # noqa: BLE001 - listing must never break the page
        pass
    try:
        if kokoro.is_installed():
            out["kokoro"] = list(kokoro.VOICES)
    except Exception:  # noqa: BLE001
        pass
    try:
        if chatterbox.is_provisioned():
            out["chatterbox"] = list(chatterbox.list_voices())
    except Exception:  # noqa: BLE001
        pass
    return out


def _engine_status() -> dict:
    from sonara import kokoro, chatterbox
    def safe(fn):
        try:
            return bool(fn())
        except Exception:  # noqa: BLE001
            return False
    return {"kokoro": safe(kokoro.is_installed),
            "chatterbox": safe(chatterbox.is_provisioned)}


def _keymap_state() -> list:
    from sonara import keymap
    km = keymap.load_keymap()
    out = []
    for action in keymap.ACTION_MESSAGES:
        b = km.get(action) or {}
        out.append({"action": action, "key": b.get("key"),
                    "mods": list(b.get("mods", []))})
    return out


class SettingsServer:
    def __init__(self, daemon, token: str, port: int):
        self._daemon = daemon
        self._token = token
        self._want_port = port
        self._httpd = None
        self._thread = None
        self._started = time.monotonic()
        self.port = None

    def start(self) -> int:
        handler = _make_handler(self)
        try:
            self._httpd = ThreadingHTTPServer(("127.0.0.1", self._want_port), handler)
        except OSError:
            # pinned port taken: ephemeral fallback keeps the page available
            self._httpd = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self._httpd.daemon_threads = True
        self.port = self._httpd.server_address[1]
        self._thread = threading.Thread(target=self._httpd.serve_forever,
                                        name="sonara-webui", daemon=True)
        self._thread.start()
        return self.port

    def stop(self) -> None:
        if self._httpd is not None:
            try:
                self._httpd.shutdown()
                self._httpd.server_close()
            except Exception:  # noqa: BLE001 - teardown must never raise
                pass
            self._httpd = None

    # ---- state assembly ------------------------------------------------
    def state(self) -> dict:
        cfg = {k: self._daemon.config.get(k) for k in _PAGE_KEYS}
        return {
            "config": cfg,
            "voices": _installed_voices(),
            "engines": _engine_status(),
            "keymap": _keymap_state(),
            "daemon": {
                "pid": os.getpid(),
                "uptime_s": int(time.monotonic() - self._started),
                "foreground": self._daemon.sessions.foreground(),
            },
        }


def _make_handler(server: SettingsServer):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *a):  # silent: stderr is the daemon log
            pass

        def _authed(self) -> bool:
            q = parse_qs(urlparse(self.path).query)
            tok = (self.headers.get("X-Sonara-Token")
                   or (q.get("token") or [None])[0])
            return tok == server._token

        def _json(self, code: int, obj) -> None:
            body = json.dumps(obj).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if not self._authed():
                return self._json(403, {"error": "missing or wrong token; open via: sonara settings"})
            path = urlparse(self.path).path
            if path == "/api/state":
                return self._json(200, server.state())
            return self._json(404, {"error": "unknown path"})

        def do_POST(self):
            if not self._authed():
                return self._json(403, {"error": "missing or wrong token; open via: sonara settings"})
            return self._json(404, {"error": "unknown path"})
    return Handler
