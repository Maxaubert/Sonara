"""Settings page HTTP server (#34).

A ThreadingHTTPServer on 127.0.0.1 serving the settings page and a tiny JSON
API. Every request must carry the daemon.lock token (?token= or X-Sonara-Token)
or gets 403 -- this blocks other local users and web pages (CSRF/DNS-rebind).
All mutations dispatch through daemon.handle_message / the keymap module, the
exact paths the CLI uses, so the page cannot drift from CLI behavior.
"""
from __future__ import annotations

import hmac
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
    "chatterbox_max_chunk_chars", "chatterbox_exaggeration",
)

_MSG_KEYS = {
    "voice":         lambda v: {"type": "set_voice", "voice": str(v)},
    "rate":          lambda v: {"type": "set_rate", "rate": int(v)},
    "minqueue":      lambda v: {"type": "set_minqueue", "minqueue": int(v)},
    "summary_mode":  lambda v: {"type": "set_summary_mode", "enabled": bool(v)},
    "audio_control": lambda v: {"type": "set_audio_control", "enabled": bool(v)},
    "duck_level":    lambda v: {"type": "set_duck_level", "level": int(v)},
}
_CONFIG_KEYS = ("summary_model", "summary_timeout", "summary_settle_ms",
                "chatterbox_max_chunk_chars", "chatterbox_exaggeration")


def _dispatch(daemon, msg):
    """handle_message under the daemon lock -- the same guard every other
    entry point (socket, hotkey pump) uses; without it a page mutation racing
    an in-flight hook message corrupts shared state."""
    lock = getattr(daemon, "_lock", None)
    if lock is not None:
        with lock:
            return daemon.handle_message(msg)
    return daemon.handle_message(msg)


def _spawn_respawner() -> None:
    """Page Restart (#34 follow-up): when the daemon was started directly
    (`sonara start`) no supervisor loop is alive, so a plain shutdown would
    stay down until the next lazy hook start -- live-verified. A tiny detached
    child waits out the dying process, then uses the standard lazy-start path
    (which honors the stop sentinel and the singleton mutex, so it is a no-op
    if something else already respawned or the user shut down meanwhile)."""
    import subprocess
    import sys
    code = ("import time; time.sleep(2.0); "
            "from sonara.daemon import ensure_running; ensure_running()")
    kwargs = {}
    if os.name == "nt":
        # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: survives the parent
        kwargs["creationflags"] = 0x00000008 | 0x00000200
    try:
        subprocess.Popen([sys.executable, "-c", code],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, close_fds=True, **kwargs)
    except Exception:  # noqa: BLE001 - a failed respawner must not break the reply
        pass


def _bind_action(action, key, mods):
    from sonara import keymap
    keymap.bind_action(action, key, mods)


def _unbind_action(action):
    from sonara import keymap
    keymap.unbind_action(action)


def _installed_voices() -> dict:
    """Voices grouped by engine. Lazy imports; each group degrades to []."""
    from sonara import kokoro, chatterbox
    out = {"windows": [], "kokoro": [], "chatterbox": []}
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
    neural = set(out["kokoro"]) | set(out["chatterbox"])
    try:
        from sonara.platform import get_platform
        for v in get_platform().tts.list_voices():
            name = getattr(v, "display_name", None) or str(v)
            if name not in neural:      # the WinRT list echoes neural voices (#38)
                out["windows"].append(name)
    except Exception:  # noqa: BLE001 - listing must never break the page
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


def _page_bytes() -> bytes:
    path = os.path.join(os.path.dirname(__file__), "settings.html")
    with open(path, "rb") as fh:
        return fh.read()


def _key_names() -> dict:
    """Bindable key/modifier names from the platform keytables (#38): the page
    validates a capture BEFORE posting, so an unsupported key gets instant
    feedback instead of a rejected bind (or, historically, bricked hotkeys)."""
    try:
        from sonara.keymap import _keytables
        key_codes, mod_masks = _keytables()
        return {"keys": sorted(key_codes), "mods": sorted(mod_masks)}
    except Exception:  # noqa: BLE001 - page must render even with no platform
        return {"keys": [], "mods": []}


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
            "keys": _key_names(),
            "daemon": {
                "pid": os.getpid(),
                "uptime_s": int(time.monotonic() - self._started),
                "foreground": self._daemon.sessions.foreground(),
                "port": self.port,
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
            return tok is not None and hmac.compare_digest(str(tok), server._token)

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
            if path == "/api/preview-audio":
                # Pre-rendered preview file (#38): instant playback in the page.
                from sonara import previews
                q = parse_qs(urlparse(self.path).query)
                voice = (q.get("voice") or [""])[0]
                try:
                    body = previews.preview_path(voice).read_bytes()
                except OSError:
                    return self._json(404, {"error": "no preview for this voice yet"})
                self.send_response(200)
                self.send_header("Content-Type", "audio/wav")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "private, max-age=3600")
                self.end_headers()
                self.wfile.write(body)
                return
            if path in ("/", "/settings"):
                body = _page_bytes()
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            return self._json(404, {"error": "unknown path"})

        def do_POST(self):
            if not self._authed():
                return self._json(403, {"error": "missing or wrong token; open via: sonara settings"})
            path = urlparse(self.path).path
            try:
                n = max(0, int(self.headers.get("Content-Length") or 0))
                payload = json.loads(self.rfile.read(min(n, 65536)) or b"{}")
            except Exception:  # noqa: BLE001 - malformed body is a 400, never a traceback
                return self._json(400, {"error": "bad json"})
            if not isinstance(payload, dict):
                return self._json(400, {"error": "bad json"})
            if path == "/api/set":
                return self._handle_set(payload)
            if path == "/api/keymap":
                return self._handle_keymap(payload)
            if path == "/api/preview":
                fn = getattr(server._daemon, "preview_voice", None)
                if fn is not None and fn(str(payload.get("voice") or "")):
                    return self._json(202, {"ok": True})
                return self._json(409, {"error": "preview busy or unavailable"})
            if path == "/api/daemon":
                return self._handle_daemon(payload)
            return self._json(404, {"error": "unknown path"})

        def _handle_set(self, payload):
            key = payload.get("key")
            value = payload.get("value")
            if key in _MSG_KEYS:
                try:
                    msg = dict(_MSG_KEYS[key](value), v=1)
                except (TypeError, ValueError):
                    return self._json(400, {"error": f"bad value for {key}"})
                _dispatch(server._daemon, msg)
                return self._json(200, server.state())
            if key in _CONFIG_KEYS:
                setter = getattr(server._daemon, "set_config_value", None)
                if setter is not None and setter(key, value):
                    return self._json(200, server.state())
                return self._json(400, {"error": f"bad value for {key}"})
            return self._json(400, {"error": f"unknown key {key!r}"})

        def _handle_keymap(self, payload):
            action = payload.get("action")
            try:
                if payload.get("unbind"):
                    _unbind_action(action)
                else:
                    _bind_action(action, payload.get("key"),
                                 payload.get("mods") or [])
            except Exception as exc:  # noqa: BLE001 - junk input is a 400, not a dead reply
                return self._json(400, {"error": str(exc)})
            _dispatch(server._daemon, {"v": 1, "type": "reload_keymap"})
            return self._json(200, server.state())

        def _handle_daemon(self, payload):
            op = payload.get("op")
            if op == "restart":
                _dispatch(server._daemon, {"v": 1, "type": "shutdown"})
                _spawn_respawner()   # no supervisor may be alive (#34 follow-up)
                return self._json(202, {"ok": True})
            if op == "shutdown":
                _dispatch(server._daemon,
                          {"v": 1, "type": "shutdown", "stay_down": True})
                return self._json(202, {"ok": True})
            return self._json(400, {"error": "unknown op"})
    return Handler
