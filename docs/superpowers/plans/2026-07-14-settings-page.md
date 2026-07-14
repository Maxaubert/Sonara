# Sonara Settings Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A token-protected, daemon-served local settings page (macOS-System-Settings look) that live-edits Sonara config, hotkeys, and daemon lifecycle through the existing message handlers.

**Architecture:** The daemon grows a `SettingsServer` (stdlib `ThreadingHTTPServer`, localhost, pinned port 27431) exposing `GET /settings` (one self-contained HTML file), `GET /api/state`, `POST /api/set`, `POST /api/keymap`, `POST /api/preview`. All mutations flow through `daemon.handle_message` or the existing keymap module — no parallel mutation logic. Spec: `docs/superpowers/specs/2026-07-14-settings-page-design.md`. Visual base: `docs/superpowers/mockups/codex-b.html`.

**Tech Stack:** Python 3.14 stdlib only for the daemon (`http.server`, `json`, `threading`). Vanilla HTML/CSS/JS single file for the page (no CDN, no frameworks). Playwright (dev-only, optional skip) for e2e.

## Global Constraints

- Daemon code stays stdlib-only; no new runtime dependencies.
- Bind `127.0.0.1` only. Every request requires the `daemon.lock` token via `?token=` or `X-Sonara-Token`; otherwise respond 403.
- Pinned port: config key `settings_port`, default `27431`; fall back to an ephemeral port if bind fails.
- Page label is "Audio duck"; the config key stays `audio_control`.
- Verbosity is NOT on the page.
- Accent `#4F46E5`; font stack `"Segoe UI Variable","Segoe UI",-apple-system,sans-serif`.
- Hotkey actions on the page are the REAL ones from `keymap.ACTION_MESSAGES` (nav_prev, nav_next, nav_start, flush, pause, mute, next_session, faster, slower), not the mockup's placeholder list.
- Test hygiene: monkeypatch `sonara.paths` / `sonara.keymap.KEYMAP_PATH` to tmp_path in every test that touches files; never touch the live `~/.sonara`.

---

### Task 1: Config key + lockfile field

**Files:**
- Modify: `src/sonara/config.py` (DEFAULTS)
- Modify: `src/sonara/platform/transport.py:16-22` (`write_lockfile`)
- Test: `tests/test_config.py`, `tests/test_transport.py`

**Interfaces:**
- Produces: `DEFAULTS["settings_port"] == 27431`; `transport.write_lockfile(path, host, port, token, pid, http_port=None)` — writes `"http_port"` key only when not None. `read_lockfile` already returns the whole dict, unchanged.

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_config.py`:

```python
def test_settings_port_default():
    # (#34) the settings page binds a PINNED port so bookmarks and the page's
    # reconnect-after-restart polling survive daemon restarts.
    from sonara.config import DEFAULTS
    assert DEFAULTS["settings_port"] == 27431
```

Also add `"settings_port",` to the pinned key-set in
`test_defaults_has_documented_top_level_keys` (it asserts the exact DEFAULTS
key set and fails otherwise).

Append to `tests/test_transport.py`:

```python
def test_write_lockfile_optional_http_port(tmp_path):
    from sonara.platform import transport
    p = tmp_path / "lock"
    transport.write_lockfile(p, "127.0.0.1", 5000, "tok", 42)
    assert "http_port" not in transport.read_lockfile(p)
    transport.write_lockfile(p, "127.0.0.1", 5000, "tok", 42, http_port=27431)
    assert transport.read_lockfile(p)["http_port"] == 27431
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_config.py::test_settings_port_default tests/test_transport.py::test_write_lockfile_optional_http_port -q`
Expected: FAIL (KeyError / TypeError: unexpected keyword `http_port`)

- [ ] **Step 3: Implement**

In `src/sonara/config.py` DEFAULTS, after `chatterbox_max_chunk_chars`:

```python
    "settings_port": 27431,               # settings page port (pinned so bookmarks
                                          # and restart-reconnect work; 0 = ephemeral)
```

In `src/sonara/platform/transport.py` replace `write_lockfile`:

```python
def write_lockfile(path, host, port, token, pid, http_port=None) -> None:
    data = {"host": host, "port": int(port), "token": token, "pid": int(pid)}
    if http_port is not None:
        data["http_port"] = int(http_port)   # settings page (#34)
    tmp = str(path) + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.chmod(tmp, 0o600)
    os.replace(tmp, str(path))
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_config.py tests/test_transport.py -q`
Expected: PASS (the pre-existing `600` perms failure on Windows is known; only it may fail)

- [ ] **Step 5: Commit**

```bash
git add src/sonara/config.py src/sonara/platform/transport.py tests/test_config.py tests/test_transport.py
git commit -m "feat(webui): settings_port default + optional http_port lockfile field (#34)"
```

---

### Task 2: SettingsServer core — auth + /api/state

**Files:**
- Create: `src/sonara/webui.py`
- Test: `tests/test_webui.py` (new)

**Interfaces:**
- Consumes: a daemon-like object with `.config` (dict), `.handle_message(msg)`, `.sessions.foreground()`, and module functions it calls lazily: `sonara.keymap.load_keymap()`, `sonara.keymap.ACTION_MESSAGES`, `sonara.kokoro.VOICES` / `is_installed()`, `sonara.chatterbox.list_voices()` / `is_provisioned()`, platform `tts.list_voices()`.
- Produces: `class SettingsServer(daemon, token, port)` with `.start() -> int` (actual bound port), `.stop()`, `.port`. HTTP: 403 without token; `GET /api/state` returns the JSON documented in Step 3.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_webui.py`:

```python
"""Settings page HTTP server (#34): auth, state, dispatch. Uses a FakeDaemon --
never a real daemon, never the live ~/.sonara."""
import json
import urllib.request
import urllib.error

import pytest

from sonara import webui


class FakeSessions:
    def foreground(self):
        return "sess-1"


class FakeDaemon:
    def __init__(self):
        self.config = {"voice": "af_heart", "rate": 250, "minqueue": 5,
                       "summary_mode": True, "summary_model": "haiku",
                       "summary_timeout": 60, "summary_settle_ms": 600,
                       "audio_control": False, "duck_level": 20,
                       "chatterbox_max_chunk_chars": 280, "settings_port": 0}
        self.sessions = FakeSessions()
        self.messages = []

    def handle_message(self, msg):
        self.messages.append(msg)
        return {"ok": True}


@pytest.fixture()
def server(monkeypatch):
    monkeypatch.setattr(webui, "_installed_voices", lambda: {
        "windows": ["Microsoft Zira"], "kokoro": ["af_heart"],
        "chatterbox": ["cb_default"]})
    monkeypatch.setattr(webui, "_engine_status", lambda: {
        "kokoro": True, "chatterbox": True})
    monkeypatch.setattr(webui, "_keymap_state", lambda: [
        {"action": "mute", "key": "m", "mods": ["ctrl", "alt"]}])
    d = FakeDaemon()
    s = webui.SettingsServer(d, token="tok123", port=0)  # 0 = ephemeral for tests
    s.start()
    yield d, s
    s.stop()


def _get(s, path, token="tok123"):
    req = urllib.request.Request(f"http://127.0.0.1:{s.port}{path}")
    if token:
        req.add_header("X-Sonara-Token", token)
    return urllib.request.urlopen(req, timeout=5)


def test_missing_token_is_403(server):
    d, s = server
    with pytest.raises(urllib.error.HTTPError) as ei:
        _get(s, "/api/state", token=None)
    assert ei.value.code == 403


def test_wrong_token_is_403(server):
    d, s = server
    with pytest.raises(urllib.error.HTTPError) as ei:
        _get(s, "/api/state", token="nope")
    assert ei.value.code == 403


def test_token_via_query_param_works(server):
    d, s = server
    r = urllib.request.urlopen(
        f"http://127.0.0.1:{s.port}/api/state?token=tok123", timeout=5)
    assert r.status == 200


def test_state_shape(server):
    d, s = server
    state = json.loads(_get(s, "/api/state").read())
    assert state["config"]["voice"] == "af_heart"
    assert "verbosity" not in state["config"]          # NOT on the page
    assert state["voices"]["kokoro"] == ["af_heart"]
    assert state["engines"] == {"kokoro": True, "chatterbox": True}
    assert state["keymap"][0]["action"] == "mute"
    assert state["daemon"]["foreground"] == "sess-1"
    assert isinstance(state["daemon"]["pid"], int)
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_webui.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'sonara.webui'`

- [ ] **Step 3: Implement `src/sonara/webui.py`**

```python
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
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_webui.py -q`
Expected: 4 passed

- [ ] **Step 5: Commit**

```bash
git add src/sonara/webui.py tests/test_webui.py
git commit -m "feat(webui): SettingsServer with token auth + /api/state (#34)"
```

---

### Task 3: POST /api/set — dispatch through the daemon

**Files:**
- Modify: `src/sonara/webui.py`
- Modify: `src/sonara/daemon.py` (one new method `set_config_value`)
- Test: `tests/test_webui.py`, `tests/test_daemon_summary_mode.py` is NOT touched

**Interfaces:**
- Consumes: `MsgType` constants from `sonara.protocol`.
- Produces: `POST /api/set` body `{"key": str, "value": any}` → dispatches and returns the fresh `/api/state` JSON. Message-backed keys map: `voice→SET_VOICE(voice=)`, `rate→SET_RATE(rate=)`, `minqueue→SET_MINQUEUE(minqueue=)`, `summary_mode→SET_SUMMARY_MODE(enabled=)`, `audio_control→SET_AUDIO_CONTROL(enabled=)`, `duck_level→SET_DUCK_LEVEL(level=)`. Config-only keys (`summary_model`, `summary_timeout`, `summary_settle_ms`, `chatterbox_max_chunk_chars`) go through new `daemon.set_config_value(key, value)` (clamps: timeout 15–300 int, settle 0–5000 int, chunk 80–280 int, model non-empty str; sets `self.config[key]` under `self._lock` and `save_config`). Unknown key → 400.

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_webui.py`:

```python
def _post(s, path, obj, token="tok123"):
    body = json.dumps(obj).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{s.port}{path}", data=body,
                                 headers={"X-Sonara-Token": token,
                                          "Content-Type": "application/json"})
    return urllib.request.urlopen(req, timeout=5)


def test_set_message_backed_key_dispatches(server):
    d, s = server
    r = _post(s, "/api/set", {"key": "rate", "value": 220})
    assert r.status == 200
    assert d.messages[-1]["type"] == "set_rate"
    assert d.messages[-1]["rate"] == 220


def test_set_summary_mode_uses_enabled_field(server):
    d, s = server
    _post(s, "/api/set", {"key": "summary_mode", "value": False})
    assert d.messages[-1] == {"v": 1, "type": "set_summary_mode", "enabled": False}


def test_set_config_only_key_uses_daemon_setter(server, monkeypatch):
    d, s = server
    calls = []
    d.set_config_value = lambda k, v: calls.append((k, v)) or True
    _post(s, "/api/set", {"key": "summary_settle_ms", "value": 800})
    assert calls == [("summary_settle_ms", 800)]
    assert d.messages == []                       # no protocol message for these


def test_set_unknown_key_is_400(server):
    d, s = server
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/set", {"key": "verbosity", "value": "quiet"})
    assert ei.value.code == 400
```

Create `tests/test_daemon_set_config_value.py`:

```python
from tests.daemon_helpers import make_daemon


def test_set_config_value_clamps_and_persists(monkeypatch):
    import sonara.daemon as daemon_module
    saved = []
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: saved.append(dict(cfg)))
    daemon, *_ = make_daemon()
    assert daemon.set_config_value("summary_settle_ms", 99999) is True
    assert daemon.config["summary_settle_ms"] == 5000          # clamped
    assert daemon.set_config_value("chatterbox_max_chunk_chars", 10) is True
    assert daemon.config["chatterbox_max_chunk_chars"] == 80   # clamped
    assert daemon.set_config_value("not_a_key", 1) is False
    assert saved                                               # persisted
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_webui.py tests/test_daemon_set_config_value.py -q`
Expected: new tests FAIL (404 route / missing method)

- [ ] **Step 3: Implement**

In `src/sonara/webui.py` add at module level:

```python
_MSG_KEYS = {
    "voice":         lambda v: {"type": "set_voice", "voice": str(v)},
    "rate":          lambda v: {"type": "set_rate", "rate": int(v)},
    "minqueue":      lambda v: {"type": "set_minqueue", "minqueue": int(v)},
    "summary_mode":  lambda v: {"type": "set_summary_mode", "enabled": bool(v)},
    "audio_control": lambda v: {"type": "set_audio_control", "enabled": bool(v)},
    "duck_level":    lambda v: {"type": "set_duck_level", "level": int(v)},
}
_CONFIG_KEYS = ("summary_model", "summary_timeout", "summary_settle_ms",
                "chatterbox_max_chunk_chars")
```

In `do_POST` replace the body with routing:

```python
        def do_POST(self):
            if not self._authed():
                return self._json(403, {"error": "missing or wrong token; open via: sonara settings"})
            path = urlparse(self.path).path
            try:
                n = int(self.headers.get("Content-Length") or 0)
                payload = json.loads(self.rfile.read(min(n, 65536)) or b"{}")
            except (ValueError, OSError):
                return self._json(400, {"error": "bad json"})
            if path == "/api/set":
                return self._handle_set(payload)
            return self._json(404, {"error": "unknown path"})

        def _handle_set(self, payload):
            key = payload.get("key")
            value = payload.get("value")
            if key in _MSG_KEYS:
                try:
                    msg = dict(_MSG_KEYS[key](value), v=1)
                except (TypeError, ValueError):
                    return self._json(400, {"error": f"bad value for {key}"})
                server._daemon.handle_message(msg)
                return self._json(200, server.state())
            if key in _CONFIG_KEYS:
                setter = getattr(server._daemon, "set_config_value", None)
                if setter is not None and setter(key, value):
                    return self._json(200, server.state())
                return self._json(400, {"error": f"bad value for {key}"})
            return self._json(400, {"error": f"unknown key {key!r}"})
```

In `src/sonara/daemon.py`, next to `_duck_level`, add:

```python
    def set_config_value(self, key: str, value) -> bool:
        """Set a config-only tuning key (settings page, #34). These have no
        protocol message (the CLI edits config.json directly); clamp, set under
        the lock, persist. Returns False for unknown keys/bad values."""
        clamps = {
            "summary_model":   lambda v: str(v).strip() or None,
            "summary_timeout": lambda v: max(15, min(300, int(v))),
            "summary_settle_ms": lambda v: max(0, min(5000, int(v))),
            "chatterbox_max_chunk_chars": lambda v: max(80, min(280, int(v))),
        }
        fn = clamps.get(key)
        if fn is None:
            return False
        try:
            cleaned = fn(value)
        except (TypeError, ValueError):
            return False
        if cleaned is None:
            return False
        with self._lock:
            self.config[key] = cleaned
            save_config(self.config)
        return True
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_webui.py tests/test_daemon_set_config_value.py -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonara/webui.py src/sonara/daemon.py tests/test_webui.py tests/test_daemon_set_config_value.py
git commit -m "feat(webui): POST /api/set dispatching through handle_message + clamped config setter (#34)"
```

---

### Task 4: POST /api/keymap — full hotkey editing

**Files:**
- Modify: `src/sonara/keymap.py` (new `bind_action`)
- Modify: `src/sonara/webui.py`
- Test: `tests/test_keymap.py`, `tests/test_webui.py`

**Interfaces:**
- Produces: `keymap.bind_action(action: str, key: str, mods: list[str]) -> None`
  (raises ValueError on unknown action/empty key; persists via `_write_user_keymap`,
  same override shape `{action: {"key": ..., "mods": [...]}}` that `load_keymap`
  reads). `POST /api/keymap` bodies: `{"action": a, "key": k, "mods": [..]}` binds;
  `{"action": a, "unbind": true}` unbinds via existing `keymap.unbind_action`.
  Both then dispatch `{"v":1,"type":"reload_keymap"}` and return fresh state.

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_keymap.py`:

```python
def test_bind_action_persists_override(tmp_path, monkeypatch):
    from sonara import keymap
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    monkeypatch.setattr(keymap, "ensure_sonara_dir", lambda: None)
    keymap.bind_action("mute", "k", ["ctrl", "shift"])
    km = keymap.load_keymap()
    assert km["mute"] == {"key": "k", "mods": ["ctrl", "shift"]}


def test_bind_action_rejects_unknown_action(tmp_path, monkeypatch):
    from sonara import keymap
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    import pytest
    with pytest.raises(ValueError):
        keymap.bind_action("warp_drive", "w", ["ctrl"])
```

Append to `tests/test_webui.py`:

```python
def test_keymap_bind_writes_and_reloads(server, monkeypatch):
    d, s = server
    bound = []
    monkeypatch.setattr(webui, "_bind_action", lambda a, k, m: bound.append((a, k, m)))
    r = _post(s, "/api/keymap", {"action": "mute", "key": "k", "mods": ["ctrl", "alt"]})
    assert r.status == 200
    assert bound == [("mute", "k", ["ctrl", "alt"])]
    assert d.messages[-1]["type"] == "reload_keymap"


def test_keymap_unbind(server, monkeypatch):
    d, s = server
    unbound = []
    monkeypatch.setattr(webui, "_unbind_action", lambda a: unbound.append(a))
    _post(s, "/api/keymap", {"action": "mute", "unbind": True})
    assert unbound == ["mute"]
    assert d.messages[-1]["type"] == "reload_keymap"


def test_keymap_bad_action_is_400(server, monkeypatch):
    d, s = server
    def boom(a, k, m):
        raise ValueError("unknown action")
    monkeypatch.setattr(webui, "_bind_action", boom)
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/keymap", {"action": "warp", "key": "w", "mods": []})
    assert ei.value.code == 400
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_keymap.py -k bind_action -q; python -m pytest tests/test_webui.py -k keymap -q`
Expected: FAIL (no `bind_action`, 404 route)

- [ ] **Step 3: Implement**

In `src/sonara/keymap.py`, after `unbind_action`:

```python
def bind_action(action: str, key: str, mods: list) -> None:
    """Bind *action* to key+mods in the user's keymap.json (settings page, #34).
    The override fully replaces the default binding, exactly like a hand-edit."""
    if action not in ACTION_MESSAGES:
        raise ValueError(f"unknown action {action!r}")
    key = (key or "").strip().lower()
    if not key:
        raise ValueError("empty key")
    user = _read_user_keymap()
    user[action] = {"key": key, "mods": [str(m).lower() for m in (mods or [])]}
    _write_user_keymap(user)
```

In `src/sonara/webui.py` add module-level indirection (patchable in tests):

```python
def _bind_action(action, key, mods):
    from sonara import keymap
    keymap.bind_action(action, key, mods)


def _unbind_action(action):
    from sonara import keymap
    keymap.unbind_action(action)
```

In `do_POST` routing add before the 404:

```python
            if path == "/api/keymap":
                return self._handle_keymap(payload)
```

And the handler method on `Handler`:

```python
        def _handle_keymap(self, payload):
            action = payload.get("action")
            try:
                if payload.get("unbind"):
                    _unbind_action(action)
                else:
                    _bind_action(action, payload.get("key"),
                                 payload.get("mods") or [])
            except ValueError as exc:
                return self._json(400, {"error": str(exc)})
            server._daemon.handle_message({"v": 1, "type": "reload_keymap"})
            return self._json(200, server.state())
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_keymap.py tests/test_webui.py -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonara/keymap.py src/sonara/webui.py tests/test_keymap.py tests/test_webui.py
git commit -m "feat(webui): hotkey bind/unbind endpoint through the keymap module (#34)"
```

---

### Task 5: POST /api/preview — voice sample without config change

**Files:**
- Modify: `src/sonara/daemon.py` (new `preview_voice`)
- Modify: `src/sonara/webui.py`
- Test: `tests/test_webui.py`, `tests/test_daemon_preview.py` (new)

**Interfaces:**
- Produces: `daemon.preview_voice(voice: str) -> bool` — spawns a daemon thread
  calling the platform tts `run(sample_text, voice, rate).wait(30)`; coalesced by
  a `_preview_busy` flag (returns False while one is playing). Sample text:
  `"This is {voice} speaking for Sonara."` `POST /api/preview {"voice": name}` →
  202 `{"ok": true}` or 409 when busy.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_daemon_preview.py`:

```python
import threading

from tests.daemon_helpers import make_daemon


def test_preview_voice_speaks_sample_with_named_voice(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon()
    ran = []
    done = threading.Event()

    class H:
        def wait(self, timeout=None):
            done.set()
            return 0
    def fake_run(text, voice, rate, on_play=None):
        ran.append((text, voice, rate))
        return H()
    monkeypatch.setattr(daemon, "_preview_runner", fake_run)
    assert daemon.preview_voice("af_bella") is True
    assert done.wait(2)
    text, voice, rate = ran[0]
    assert voice == "af_bella"
    assert "af_bella" in text
    assert daemon.config["voice"] != "af_bella"       # config untouched


def test_preview_voice_coalesces(monkeypatch):
    daemon, *_ = make_daemon()
    release = threading.Event()

    class H:
        def wait(self, timeout=None):
            release.wait(5)
            return 0
    monkeypatch.setattr(daemon, "_preview_runner", lambda *a, **k: H())
    assert daemon.preview_voice("af_bella") is True
    assert daemon.preview_voice("af_heart") is False   # busy
    release.set()
```

Append to `tests/test_webui.py`:

```python
def test_preview_endpoint(server):
    d, s = server
    d.preview_voice = lambda v: v == "af_heart"
    r = _post(s, "/api/preview", {"voice": "af_heart"})
    assert r.status == 202
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/preview", {"voice": "busy_voice"})
    assert ei.value.code == 409
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_daemon_preview.py tests/test_webui.py::test_preview_endpoint -q`
Expected: FAIL (no `preview_voice` / 404)

- [ ] **Step 3: Implement**

In `src/sonara/daemon.py` (near `set_config_value`):

```python
    def preview_voice(self, voice: str) -> bool:
        """Speak a short sample in *voice* WITHOUT changing config (settings
        page, #34). Runs on its own thread via the platform tts runner (same
        say_runner contract the Speaker uses); coalesced to one at a time."""
        if getattr(self, "_preview_busy", False):
            return False
        runner = getattr(self, "_preview_runner", None)
        if runner is None:
            from sonara.platform import get_platform
            runner = get_platform().tts.run
        self._preview_busy = True
        text = "This is {0} speaking for Sonara.".format(voice)
        rate = self.config.get("rate", 200)

        def _run():
            try:
                handle = runner(text, voice, rate)
                handle.wait(30)
            except Exception:  # noqa: BLE001 - preview must never crash anything
                pass
            finally:
                self._preview_busy = False
        threading.Thread(target=_run, name="sonara-preview", daemon=True).start()
        return True
```

In `webui.py` `do_POST` routing add:

```python
            if path == "/api/preview":
                fn = getattr(server._daemon, "preview_voice", None)
                if fn is not None and fn(str(payload.get("voice") or "")):
                    return self._json(202, {"ok": True})
                return self._json(409, {"error": "preview busy or unavailable"})
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_daemon_preview.py tests/test_webui.py -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonara/daemon.py src/sonara/webui.py tests/test_daemon_preview.py tests/test_webui.py
git commit -m "feat(webui): voice preview endpoint, no config change (#34)"
```

---

### Task 6: The page — settings.html served at GET /settings

**Files:**
- Create: `src/sonara/settings.html` (start from `docs/superpowers/mockups/codex-b.html`, committed in-repo)
- Modify: `src/sonara/webui.py` (serve it)
- Test: `tests/test_webui.py`

**Interfaces:**
- Consumes: `/api/state`, `/api/set`, `/api/keymap`, `/api/preview` from Tasks 2–5.
- Produces: `GET /settings` (and `GET /`) → 200 text/html, the page.

The page is the mockup REWIRED, not redesigned. Concrete deltas from
`codex-b.html`:

1. Copy `docs/superpowers/mockups/codex-b.html` to `src/sonara/settings.html`.
2. Delete the Verbosity remnants if any; keep "Audio duck" naming (already edited
   in the mockup).
3. Replace the hotkeys section rows with the REAL actions and ids:
   `nav_prev, nav_next, nav_start, flush, pause, mute, next_session, faster,
   slower` (labels: "Previous item", "Next item", "Restart / re-read",
   "Flush to end", "Pause / resume", "Mute cycle", "Next session", "Faster",
   "Slower"). Each row gets `data-action="<action>"`.
4. Give every control an id and wire this JS block (replace the mockup's demo
   handlers at the bottom of the file):

```html
<script>
const TOKEN = new URLSearchParams(location.search).get("token") || "";
const API = (p) => fetch(p, {headers: {"X-Sonara-Token": TOKEN}});
const POST = (p, obj) => fetch(p, {method: "POST",
  headers: {"X-Sonara-Token": TOKEN, "Content-Type": "application/json"},
  body: JSON.stringify(obj)});

let state = null;
async function refresh() {
  try {
    const r = await API("/api/state");
    if (!r.ok) throw new Error(r.status);
    state = await r.json();
    render(state);
    setBanner(false);
  } catch (e) { setBanner(true); }
}
function setBanner(down) {
  document.getElementById("offline-banner").style.display = down ? "flex" : "none";
  document.querySelectorAll("input,select,button.switch").forEach(el => el.disabled = down);
}
async function set(key, value) {
  const r = await POST("/api/set", {key, value});
  if (r.ok) { state = await r.json(); render(state); pulseSaved(); }
}
function render(s) {
  // voices: rebuild the select grouped by engine
  const sel = document.getElementById("voice-select");
  sel.innerHTML = "";
  for (const [engine, names] of Object.entries(s.voices)) {
    if (!names.length) continue;
    const og = document.createElement("optgroup");
    og.label = engine[0].toUpperCase() + engine.slice(1);
    for (const n of names) {
      const o = document.createElement("option");
      o.value = n; o.textContent = n; o.selected = (n === s.config.voice);
      og.appendChild(o);
    }
    sel.appendChild(og);
  }
  document.getElementById("rate").value = s.config.rate;
  document.getElementById("rate-out").textContent = s.config.rate + " wpm";
  document.getElementById("minqueue-out").textContent = s.config.minqueue;
  setSwitch("summary-switch", s.config.summary_mode);
  document.getElementById("model-select").value = s.config.summary_model;
  document.getElementById("timeout").value = s.config.summary_timeout;
  document.getElementById("timeout-out").textContent = s.config.summary_timeout + " s";
  document.getElementById("settle").value = s.config.summary_settle_ms;
  document.getElementById("settle-out").textContent = s.config.summary_settle_ms + " ms";
  setSwitch("duck-switch", s.config.audio_control);
  document.getElementById("duck").value = s.config.duck_level;
  document.getElementById("duck-out").textContent = s.config.duck_level + "%";
  document.getElementById("chunk").value = s.config.chatterbox_max_chunk_chars;
  document.getElementById("chunk-out").textContent = s.config.chatterbox_max_chunk_chars;
  // hotkeys
  for (const b of s.keymap) {
    const row = document.querySelector(`[data-action="${b.action}"] .kbd`);
    if (row) row.innerHTML = b.key
      ? [...b.mods, b.key].map(k => `<i>${k}</i>`).join("")
      : `<i class="unbound">unbound</i>`;
  }
  // system
  document.getElementById("sys-pid").textContent = s.daemon.pid;
  document.getElementById("sys-uptime").textContent = fmtUptime(s.daemon.uptime_s);
  document.getElementById("engine-kokoro").textContent = s.engines.kokoro ? "Installed" : "Not installed";
  document.getElementById("engine-chatterbox").textContent = s.engines.chatterbox ? "Installed" : "Not installed";
}
function setSwitch(id, on) {
  document.getElementById(id).setAttribute("aria-checked", on ? "true" : "false");
}
function fmtUptime(sec) {
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60);
  return h ? `${h}h ${m}m` : `${m}m`;
}
let savedT;
function pulseSaved() {
  document.querySelectorAll(".state").forEach(el => el.classList.add("flash"));
  clearTimeout(savedT);
  savedT = setTimeout(() => document.querySelectorAll(".state")
    .forEach(el => el.classList.remove("flash")), 1200);
}
// control wiring
document.getElementById("voice-select").addEventListener("change",
  e => set("voice", e.target.value));
document.getElementById("voice-preview").addEventListener("click",
  () => POST("/api/preview", {voice: document.getElementById("voice-select").value}));
document.getElementById("rate").addEventListener("change", e => set("rate", +e.target.value));
document.getElementById("timeout").addEventListener("change", e => set("summary_timeout", +e.target.value));
document.getElementById("settle").addEventListener("change", e => set("summary_settle_ms", +e.target.value));
document.getElementById("duck").addEventListener("change", e => set("duck_level", +e.target.value));
document.getElementById("chunk").addEventListener("change", e => set("chatterbox_max_chunk_chars", +e.target.value));
document.getElementById("model-select").addEventListener("change", e => set("summary_model", e.target.value));
document.getElementById("summary-switch").addEventListener("click", function () {
  set("summary_mode", this.getAttribute("aria-checked") !== "true");
});
document.getElementById("duck-switch").addEventListener("click", function () {
  set("audio_control", this.getAttribute("aria-checked") !== "true");
});
document.getElementById("mq-minus").addEventListener("click",
  () => set("minqueue", Math.max(1, state.config.minqueue - 1)));
document.getElementById("mq-plus").addEventListener("click",
  () => set("minqueue", Math.min(10, state.config.minqueue + 1)));
// hotkey capture
document.querySelectorAll("[data-action]").forEach(row => {
  const action = row.dataset.action;
  row.querySelector(".kbd").addEventListener("click", () => {
    row.classList.add("listen");
    const h = async (e) => {
      e.preventDefault();
      if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return;
      removeEventListener("keydown", h, true);
      row.classList.remove("listen");
      if (e.key === "Escape") return;
      const mods = [];
      if (e.ctrlKey) mods.push("ctrl");
      if (e.altKey) mods.push("alt");
      if (e.shiftKey) mods.push("shift");
      const r = await POST("/api/keymap", {action, key: e.key.toLowerCase(), mods});
      if (r.ok) { state = await r.json(); render(state); pulseSaved(); }
    };
    addEventListener("keydown", h, true);
  });
  const un = row.querySelector(".unbind");
  if (un) un.addEventListener("click", async () => {
    const r = await POST("/api/keymap", {action, unbind: true});
    if (r.ok) { state = await r.json(); render(state); pulseSaved(); }
  });
});
// daemon buttons
document.getElementById("btn-restart").addEventListener("click",
  () => POST("/api/set", {key: "daemon", value: "restart"}));   // replaced in Task 7
document.getElementById("btn-shutdown").addEventListener("click",
  () => POST("/api/set", {key: "daemon", value: "shutdown"}));  // replaced in Task 7
refresh();
setInterval(refresh, 3000);
</script>
```

5. Add near the top of `<body>` the offline banner:

```html
<div id="offline-banner" style="display:none" class="banner">
  Sonara isn't running — start it with <code>sonara start</code>. Reconnecting…
</div>
```

with CSS `.banner{position:fixed;top:0;left:0;right:0;z-index:99;background:#c43d4c;color:#fff;padding:10px 16px;display:flex;gap:8px;font-size:13px;justify-content:center}` and `.state.flash i{box-shadow:0 0 0 5px rgba(49,173,119,.25)}` and `.kbd .unbound{opacity:.5;font-style:italic}`.

6. Keep the decorative window chrome and search field; the search filters nav
   buttons by `textContent.toLowerCase().includes(q)` (mockup already has it —
   verify it still works after edits).
7. Theme toggle persists: on flip, `localStorage.setItem("sonara-theme", t)`;
   on load, apply `localStorage.getItem("sonara-theme")` before first paint.

- [ ] **Step 1: Write the failing test** (append to `tests/test_webui.py`)

```python
def test_settings_page_served_and_self_contained(server):
    d, s = server
    html = _get(s, "/settings").read().decode("utf-8")
    assert "Sonara" in html and "offline-banner" in html
    assert "http://" not in html.replace(f"http://127.0.0.1", "")  # no external refs
    assert "https://" not in html
    # / redirects or serves too
    assert _get(s, "/").status == 200
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_webui.py::test_settings_page_served_and_self_contained -q`
Expected: FAIL (404)

- [ ] **Step 3: Implement**

Create `src/sonara/settings.html` per the deltas above. In `webui.py`:

```python
def _page_bytes() -> bytes:
    path = os.path.join(os.path.dirname(__file__), "settings.html")
    with open(path, "rb") as fh:
        return fh.read()
```

In `do_GET` before the 404:

```python
            if path in ("/", "/settings"):
                body = _page_bytes()
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
```

- [ ] **Step 4: Verify pass + visual check**

Run: `python -m pytest tests/test_webui.py -q` → PASS.
Visual: screenshot with headless Edge (`msedge --headless --screenshot=... "http://127.0.0.1:<port>/settings?token=..."` against a test server) and confirm the five sections render.

- [ ] **Step 5: Commit**

```bash
git add src/sonara/settings.html src/sonara/webui.py tests/test_webui.py
git commit -m "feat(webui): settings.html page served at /settings, wired to the API (#34)"
```

---

### Task 7: Daemon lifecycle integration

**Files:**
- Modify: `src/sonara/daemon.py` (start/stop server; lockfile http_port; SHUTDOWN `stay_down`)
- Modify: `src/sonara/webui.py` (`POST /api/daemon` replaces the Task-6 placeholder buttons)
- Modify: `src/sonara/settings.html` (buttons post to `/api/daemon`)
- Test: `tests/test_webui.py`, `tests/test_daemon_lifecycle_webui.py` (new)

**Interfaces:**
- Produces: daemon starts `SettingsServer(self, self._token, config.get("settings_port", 27431))`
  right after the TCP server binds, passes `http_port=server.port` into the existing
  `transport.write_lockfile` call (daemon.py:1873), stops it in `stop()`.
  SHUTDOWN message gains optional `"stay_down": true` → writes
  `paths.STOPPED_SENTINEL_PATH` before stopping (page Shut down); without it the
  supervisor respawns (page Restart). `POST /api/daemon {"op": "restart"|"shutdown"}`.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_daemon_lifecycle_webui.py`:

```python
from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def test_shutdown_stay_down_writes_sentinel(monkeypatch, tmp_path):
    from sonara import paths
    sentinel = tmp_path / "stopped"
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", sentinel)
    daemon, *_ = make_daemon()
    stopped = []
    monkeypatch.setattr(daemon, "stop", lambda: stopped.append(True))
    import threading
    real_timer = threading.Timer
    monkeypatch.setattr(threading, "Timer",
                        lambda d, fn: real_timer(0.01, fn))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SHUTDOWN,
                           "stay_down": True})
    import time
    time.sleep(0.3)
    assert sentinel.exists()
    assert stopped


def test_plain_shutdown_leaves_no_sentinel(monkeypatch, tmp_path):
    from sonara import paths
    sentinel = tmp_path / "stopped"
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", sentinel)
    daemon, *_ = make_daemon()
    monkeypatch.setattr(daemon, "stop", lambda: None)
    import threading
    real_timer = threading.Timer
    monkeypatch.setattr(threading, "Timer", lambda d, fn: real_timer(0.01, fn))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SHUTDOWN})
    import time
    time.sleep(0.3)
    assert not sentinel.exists()
```

Append to `tests/test_webui.py`:

```python
def test_daemon_endpoint_restart_and_shutdown(server):
    d, s = server
    _post(s, "/api/daemon", {"op": "restart"})
    assert d.messages[-1] == {"v": 1, "type": "shutdown"}
    _post(s, "/api/daemon", {"op": "shutdown"})
    assert d.messages[-1] == {"v": 1, "type": "shutdown", "stay_down": True}
    with pytest.raises(urllib.error.HTTPError) as ei:
        _post(s, "/api/daemon", {"op": "reboot-the-moon"})
    assert ei.value.code == 400
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_daemon_lifecycle_webui.py tests/test_webui.py::test_daemon_endpoint_restart_and_shutdown -q`
Expected: FAIL

- [ ] **Step 3: Implement**

daemon.py — in the SHUTDOWN handler, before arming the stop timer:

```python
            if msg.get("stay_down"):
                # Page 'Shut down' (#34): gate both respawn paths, exactly like
                # `sonara shutdown` (the CLI writes the sentinel client-side).
                try:
                    from sonara.paths import STOPPED_SENTINEL_PATH
                    from sonara import paths as _paths
                    _paths.STOPPED_SENTINEL_PATH.write_text("via settings page")
                except OSError:
                    pass
```

(Use `paths.STOPPED_SENTINEL_PATH` via the module attribute so tests can
monkeypatch it: `from sonara import paths` then `paths.STOPPED_SENTINEL_PATH.write_text(...)`.)

daemon.py — in `run()` after the TCP server binds and before `write_lockfile`:

```python
        from sonara.webui import SettingsServer
        self._webui = SettingsServer(self, self._token,
                                     int(self.config.get("settings_port", 27431)))
        try:
            http_port = self._webui.start()
        except Exception:  # noqa: BLE001 - the page must never block speech
            self._webui, http_port = None, None
```

and extend the existing call at daemon.py:1873:

```python
        transport.write_lockfile(
            LOCK_PATH, transport.HOST, port, self._token, os.getpid(),
            http_port=http_port)
```

daemon.py — in `stop()` add:

```python
        if getattr(self, "_webui", None) is not None:
            self._webui.stop()
```

webui.py — `do_POST` routing add:

```python
            if path == "/api/daemon":
                op = payload.get("op")
                if op == "restart":
                    server._daemon.handle_message({"v": 1, "type": "shutdown"})
                    return self._json(202, {"ok": True})
                if op == "shutdown":
                    server._daemon.handle_message(
                        {"v": 1, "type": "shutdown", "stay_down": True})
                    return self._json(202, {"ok": True})
                return self._json(400, {"error": "unknown op"})
```

settings.html — replace the two placeholder button handlers from Task 6:

```javascript
document.getElementById("btn-restart").addEventListener("click",
  () => POST("/api/daemon", {op: "restart"}));
document.getElementById("btn-shutdown").addEventListener("click", () => {
  if (confirm("Shut Sonara down? It stays down until 'sonara start'."))
    POST("/api/daemon", {op: "shutdown"});
});
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_daemon_lifecycle_webui.py tests/test_webui.py tests/test_daemon_lifecycle.py tests/test_cli_lifecycle.py -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonara/daemon.py src/sonara/webui.py src/sonara/settings.html tests/
git commit -m "feat(webui): daemon serves the page, lockfile http_port, stay_down shutdown (#34)"
```

---

### Task 8: CLI `sonara settings`, slash command, docs

**Files:**
- Modify: `src/sonara/cli.py`
- Create: `commands/settings.md`
- Modify: `README.md` (command table)
- Test: `tests/test_cli_settings.py` (new)

**Interfaces:**
- Produces: `sonara settings` reads `paths.LOCK_PATH` via `transport.read_lockfile`,
  builds `http://127.0.0.1:{http_port}/settings?token={token}`, opens with
  `webbrowser.open`, prints the URL. Exit 1 with the standard dead-daemon hint
  when the lock or `http_port` is missing.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_cli_settings.py`:

```python
import json

from sonara import cli, paths


def _lock(tmp_path, monkeypatch, **extra):
    lock = tmp_path / "daemon.lock"
    lock.write_text(json.dumps({"host": "127.0.0.1", "port": 5000,
                                "token": "tok", "pid": 1, **extra}))
    monkeypatch.setattr(paths, "LOCK_PATH", lock)
    return lock


def test_settings_opens_browser_at_tokenized_url(monkeypatch, tmp_path, capsys):
    _lock(tmp_path, monkeypatch, http_port=27431)
    opened = []
    monkeypatch.setattr(cli.webbrowser, "open", lambda u: opened.append(u) or True)
    rc = cli.main(["settings"])
    assert rc == 0
    assert opened == ["http://127.0.0.1:27431/settings?token=tok"]
    assert "27431" in capsys.readouterr().out


def test_settings_daemon_down_hints_start(monkeypatch, tmp_path, capsys):
    monkeypatch.setattr(paths, "LOCK_PATH", tmp_path / "missing.lock")
    rc = cli.main(["settings"])
    assert rc == 1
    assert "sonara start" in capsys.readouterr().out


def test_settings_subcommand_registered():
    parser = cli._build_parser()
    args = parser.parse_args(["settings"])
    assert args.func is cli._cmd_settings
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_cli_settings.py -q`
Expected: FAIL

- [ ] **Step 3: Implement**

cli.py — add `import webbrowser` to the imports; add handler:

```python
def _cmd_settings(_args) -> int:
    """Open the browser settings page at its tokenized URL (#34)."""
    from sonara.platform import transport
    info = transport.read_lockfile(paths.LOCK_PATH)
    if not info or not info.get("http_port"):
        print(_daemon_not_running_message())
        return 1
    url = "http://127.0.0.1:{0}/settings?token={1}".format(
        info["http_port"], info.get("token", ""))
    print("Settings page: " + url)
    webbrowser.open(url)
    return 0
```

In `_register_local`:

```python
    sub.add_parser(
        "settings", help="open the browser settings page").set_defaults(
        func=_cmd_settings)
```

Create `commands/settings.md`:

```markdown
---
description: Open the Sonara settings page in the browser
---

Run the Sonara settings command with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" settings
```

It opens the local settings page in the user's default browser and prints the
URL. If it reports the daemon is not running, tell the user to run
`sonara start` first. Do not print the token-bearing URL back to the user
beyond what the command already printed.
```

cli.py — in `_cmd_status`, after printing the reply JSON, print the page URL
when the lockfile has an `http_port`:

```python
    from sonara.platform import transport
    info = transport.read_lockfile(paths.LOCK_PATH)
    if info and info.get("http_port"):
        print("Settings page: http://127.0.0.1:{0}/settings?token={1}".format(
            info["http_port"], info.get("token", "")))
```

(and add `assert "Settings page:" in out` to a status test in
`tests/test_cli_settings.py`:

```python
def test_status_prints_settings_url(monkeypatch, tmp_path, capsys):
    _lock(tmp_path, monkeypatch, http_port=27431)
    monkeypatch.setattr(cli, "_send",
                        lambda msg, expect_reply=False: {"voice": "af_heart"})
    rc = cli.main(["status"])
    assert rc == 0
    assert "Settings page: http://127.0.0.1:27431" in capsys.readouterr().out
```
)

README.md — add to the command table after the `sonara start` row:

```markdown
| `/sonara:settings` | `sonara settings` | Open the browser settings page (voice, rate, summary, audio duck, hotkeys, daemon) |
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_cli_settings.py tests/test_cli_control.py -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonara/cli.py commands/settings.md README.md tests/test_cli_settings.py
git commit -m "feat(cli): sonara settings command + /sonara:settings + README row (#34)"
```

---

### Task 9: Playwright e2e (optional-skip)

**Files:**
- Create: `tests/e2e/test_settings_page_e2e.py`
- Create: `tests/e2e/__init__.py` (empty)

**Interfaces:**
- Consumes: everything shipped. Runs a real `SettingsServer` around a FakeDaemon
  (not a full daemon: hermetic, no audio) and drives the real page in Chromium.

- [ ] **Step 1: Write the test (skips when Playwright absent)**

```python
"""E2E: the real settings.html driven by Playwright against a real
SettingsServer + FakeDaemon. Skipped unless playwright + chromium installed:
    pip install playwright && playwright install chromium
"""
import json

import pytest

pw = pytest.importorskip("playwright.sync_api")

from tests.test_webui import FakeDaemon  # reuse the fake
from sonara import webui


@pytest.fixture()
def live(monkeypatch):
    monkeypatch.setattr(webui, "_installed_voices", lambda: {
        "windows": ["Microsoft Zira"], "kokoro": ["af_heart", "af_bella"],
        "chatterbox": []})
    monkeypatch.setattr(webui, "_engine_status", lambda: {"kokoro": True, "chatterbox": False})
    monkeypatch.setattr(webui, "_keymap_state", lambda: [
        {"action": "mute", "key": "m", "mods": ["ctrl", "alt"]}])
    d = FakeDaemon()
    s = webui.SettingsServer(d, token="tok123", port=0)
    s.start()
    yield d, s
    s.stop()


def test_rate_change_dispatches_set_rate(live):
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#rate")
        page.locator("#rate").fill("300")
        page.locator("#rate").dispatch_event("change")
        page.wait_for_timeout(300)
        browser.close()
    assert any(m.get("type") == "set_rate" and m.get("rate") == 300
               for m in d.messages)


def test_offline_banner_appears_when_server_dies(live):
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#rate")
        s.stop()
        page.wait_for_selector("#offline-banner", state="visible", timeout=8000)
        browser.close()
```

- [ ] **Step 2: Run**

Run: `python -m pytest tests/e2e/ -q`
Expected: 2 passed if Playwright installed, otherwise `2 skipped` — both acceptable. If not installed, run once locally with `pip install playwright && playwright install chromium` to see them green before the PR.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/
git commit -m "test(webui): Playwright e2e for the settings page (skip without playwright) (#34)"
```

---

### Task 10: Full verification + finish

- [ ] Run the whole suite: `python -m pytest -q` — expected: everything passes except the 9 known environmental failures (bin shim WinError 193 ×3, paths layout ×2, transport 600 perms, duck_level 30-vs-20, win_tts mocks ×2).
- [ ] Grep check: `grep -rn "verbosity" src/sonara/settings.html` → no hits.
- [ ] Manual smoke: `sonara shutdown`, deploy via robocopy, `sonara start`, then `sonara settings` — page opens, change rate, hear the spoken confirmation path still works, rebind mute, restart from the page and watch it reconnect.
- [ ] Use superpowers:finishing-a-development-branch → PR referencing issue #34, merge, deploy.
