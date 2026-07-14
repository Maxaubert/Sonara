"""Live-verification follow-ups (#34): (1) the token must survive CLEAN daemon
restarts -- the lockfile is unlinked on exit, so reuse needs its own file;
(2) page Restart must respawn the daemon even when no supervisor loop is
running (daemon started via `sonara start`), via a detached respawner."""
import json
import re
import subprocess
import urllib.request

import pytest

from sonara import webui
from tests.test_webui import FakeDaemon, server, _post, _get  # noqa: F401 (fixture reuse)


# --- durable token (daemon side) -------------------------------------------

def test_persistent_token_minted_then_reused(tmp_path, monkeypatch):
    from sonara import daemon, paths
    monkeypatch.setattr(paths, "WEBUI_TOKEN_PATH", tmp_path / "webui.token")
    t1 = daemon._persistent_token()
    assert re.fullmatch(r"[0-9a-f]{64}", t1)
    assert (tmp_path / "webui.token").read_text().strip() == t1
    t2 = daemon._persistent_token()                     # clean restart
    assert t2 == t1                                     # page/bookmarks keep working


def test_persistent_token_replaces_malformed_file(tmp_path, monkeypatch):
    from sonara import daemon, paths
    p = tmp_path / "webui.token"
    p.write_text("not-a-token")
    monkeypatch.setattr(paths, "WEBUI_TOKEN_PATH", p)
    t = daemon._persistent_token()
    assert re.fullmatch(r"[0-9a-f]{64}", t)
    assert p.read_text().strip() == t


def test_persistent_token_survives_unwritable_dir(tmp_path, monkeypatch):
    from sonara import daemon, paths
    monkeypatch.setattr(paths, "WEBUI_TOKEN_PATH",
                        tmp_path / "no-such-dir" / "webui.token")
    monkeypatch.setattr(paths, "ensure_sonara_dir", lambda: None)
    t = daemon._persistent_token()                      # write fails, token still valid
    assert re.fullmatch(r"[0-9a-f]{64}", t)


# --- restart respawner (webui side) -----------------------------------------

def test_restart_op_spawns_respawner(server, monkeypatch):
    d, s = server
    spawned = []
    monkeypatch.setattr(webui, "_spawn_respawner", lambda: spawned.append(True))
    _post(s, "/api/daemon", {"op": "restart"})
    assert spawned == [True]
    assert d.messages[-1] == {"v": 1, "type": "shutdown"}


def test_shutdown_op_does_not_spawn_respawner(server, monkeypatch):
    d, s = server
    spawned = []
    monkeypatch.setattr(webui, "_spawn_respawner", lambda: spawned.append(True))
    _post(s, "/api/daemon", {"op": "shutdown"})
    assert spawned == []                                 # stay down means stay down


def test_spawn_respawner_launches_detached_lazy_start(monkeypatch):
    calls = []
    monkeypatch.setattr(subprocess, "Popen",
                        lambda argv, **kw: calls.append((argv, kw)))
    webui._spawn_respawner()
    argv, kw = calls[0]
    assert "ensure_running" in argv[-1]                  # standard lazy-start path
    assert "time.sleep" in argv[-1]                      # waits out the dying daemon
    import os
    if os.name == "nt":
        assert kw.get("creationflags", 0) & 0x00000008   # DETACHED_PROCESS
