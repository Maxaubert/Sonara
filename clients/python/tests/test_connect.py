"""Unit tests against the fake runtime: the connect algorithm, request
shapes, subscriptions and the extension namespaces."""
from __future__ import annotations

import json
import time

import pytest
from conftest import wait_until

import sonara_client
from sonara_client import SonaraError, connect
from sonara_client.discovery import pid_alive, resolve_home


def _open(home, **kw):
    kw.setdefault("autostart", False)
    return connect("unit", home=str(home), **kw)


def _sent(fake, skip=1):
    return [{k: v for k, v in r.items() if k != "id"} for r in fake.requests()[skip:]]


def test_uses_a_running_compatible_instance_and_greets_it(home, fake):
    f = fake()
    with _open(home, require=["core"], extensions=["channels"], client_version="2.1", keep_alive=True) as c:
        assert c.runtime["pid"] == f.pid
        assert c.info["version"] == "9.9.9"
    hello = f.requests()[0]
    assert hello["type"] == "hello"
    assert hello["token"] == "t0k3n"
    assert hello["client"] == {"name": "unit", "version": "2.1"}
    assert hello["protocol"] == {"major": 1, "minor": 0}
    assert hello["require"] == ["core"]
    assert hello["extensions"] == ["channels"]
    assert hello["keep_alive"] is True
    assert "takeover" not in hello


def test_default_client_version_is_the_package_version(home, fake):
    f = fake()
    _open(home).close()
    assert f.requests()[0]["client"]["version"] == sonara_client.__version__


def test_not_running_without_runtime_json_or_a_runtime_to_start(home, monkeypatch):
    monkeypatch.delenv("SONARA_RUNTIME", raising=False)
    with pytest.raises(SonaraError) as e:
        connect("unit", home=str(home))
    assert e.value.code == "E_NOT_RUNNING"


def test_a_stale_runtime_json_is_ignored(home):
    (home / "runtime.json").write_text(json.dumps({"pid": 2 ** 30, "port": 1, "token": "x"}), encoding="utf-8")
    with pytest.raises(SonaraError) as e:
        connect("unit", home=str(home), runtime_path="x.exe", autostart=False)
    assert e.value.code == "E_NOT_RUNNING"


def test_needs_a_client_name(home):
    with pytest.raises(SonaraError) as e:
        connect("", home=str(home))
    assert e.value.code == "E_BAD_REQUEST"


def test_incompatible_without_takeover_when_nothing_could_replace_it(home, fake, monkeypatch):
    monkeypatch.delenv("SONARA_RUNTIME", raising=False)
    f = fake(hello_error="E_UNSUPPORTED")
    with pytest.raises(SonaraError) as e:
        connect("unit", home=str(home))
    assert e.value.code == "E_INCOMPATIBLE"
    assert not any(r.get("takeover") for r in f.requests())
    assert pid_alive(f.pid)


def test_another_protocol_major_in_the_reply_is_incompatible(home, fake):
    fake(protocol_major=2)
    with pytest.raises(SonaraError) as e:
        _open(home)
    assert e.value.code == "E_INCOMPATIBLE"


def test_a_required_capability_missing_from_the_reply_is_incompatible(home, fake):
    fake()
    with pytest.raises(SonaraError) as e:
        _open(home, require=["channels"])
    assert e.value.code == "E_INCOMPATIBLE"


def test_takes_over_an_idle_incompatible_instance_then_starts_the_bundled_one(home, fake):
    f = fake(hello_error="E_INCOMPATIBLE")
    # The bundled runtime cannot start here, which shows the order: the
    # takeover ran first (the fake exited), then the start was attempted.
    with pytest.raises(SonaraError) as e:
        connect("unit", home=str(home), runtime_path=str(home / "no-such-sonarad.exe"))
    assert e.value.code == "E_START_FAILED"
    takeovers = [r for r in f.requests() if r.get("takeover")]
    assert len(takeovers) == 1 and takeovers[0]["token"] == "t0k3n"
    assert wait_until(lambda: not pid_alive(f.pid), 5)


def test_retries_the_takeover_while_busy(home, fake):
    f = fake(hello_error="E_UNSUPPORTED", busy_takeovers=2)
    with pytest.raises(SonaraError) as e:
        connect("unit", home=str(home), runtime_path=str(home / "none.exe"), takeover_retry=0.02)
    assert e.value.code == "E_START_FAILED"
    assert len([r for r in f.requests() if r.get("takeover")]) == 3


def test_gives_up_when_the_instance_stays_busy(home, fake):
    f = fake(hello_error="E_UNSUPPORTED", busy_takeovers=-1)
    t0 = time.monotonic()
    with pytest.raises(SonaraError) as e:
        connect("unit", home=str(home), runtime_path=str(home / "none.exe"),
                takeover_timeout=0.4, takeover_retry=0.05)
    assert e.value.code == "E_INCOMPATIBLE"
    assert time.monotonic() - t0 >= 0.4
    assert pid_alive(f.pid), "a busy instance is never stopped"


def test_a_wrong_token_is_e_auth(home, fake):
    fake()
    info = json.loads((home / "runtime.json").read_text(encoding="utf-8"))
    (home / "runtime.json").write_text(json.dumps({**info, "token": "wrong"}), encoding="utf-8")
    with pytest.raises(SonaraError) as e:
        _open(home)
    assert e.value.code == "E_AUTH"


def test_core_api_sends_protocol_messages(home, fake):
    f = fake()
    with _open(home) as c:
        assert c.speak("Hello.", mode="replace", interrupt=True, label="build") == 1
        assert c.speak("Again.") == 2
        c.control("pause")
        assert c.set("volume", 40) == 40
        assert c.get("volume") == 40
        assert c.voices("fake")[0]["id"] == "fake-1"
    assert _sent(f) == [
        {"type": "speak", "text": "Hello.", "mode": "replace", "interrupt": True, "label": "build"},
        {"type": "speak", "text": "Again."},
        {"type": "control", "action": "pause"},
        {"type": "set", "key": "volume", "value": 40},
        {"type": "get", "key": "volume"},
        {"type": "voices", "engine": "fake"},
    ]


def test_errors_carry_the_runtime_code(home, fake):
    fake()
    with _open(home) as c, pytest.raises(SonaraError) as e:
        c.request("fail_me", {"code": "E_NOT_FOUND"})
    assert e.value.code == "E_NOT_FOUND"


def test_a_dropped_connection_is_e_closed(home, fake):
    fake()
    c = _open(home)
    with pytest.raises(SonaraError) as e:
        c.request("close_me")
    assert e.value.code == "E_CLOSED"
    assert c.closed
    with pytest.raises(SonaraError) as e:
        c.speak("late")
    assert e.value.code == "E_CLOSED"


def test_subscribe_opens_its_own_connection_and_iterates_events(home, fake):
    f = fake()
    with _open(home) as c:
        with c.subscribe(["state", "items"]) as sub:
            first = next(iter(sub))
            assert first["event"] == "state" and first["seq"] == 1
            assert sub.read(timeout=5) == {"event": "item", "item_id": 1, "phase": "started"}
            assert sub.read(timeout=0.1) is None
    reqs = f.requests()
    assert [r["type"] for r in reqs].count("hello") == 2
    assert next(r for r in reqs if r["type"] == "subscribe")["events"] == ["state", "items"]


def test_extension_namespaces_send_their_message_types(home, fake):
    f = fake()
    with _open(home) as c:
        c.channels.open("tab-1", label="Tab 1", host_tab="t1", policy="latest")
        c.channels.focus("tab-1")
        assert c.channels.speak("tab-1", "Hi.", mode="replace") == 1
        c.channels.control("tab-1", "pause")
        c.channels.next_channel()
        c.channels.close("tab-1")
        c.agent.turn_start("tab-1", 3, t=12)
        c.agent.stream("tab-1", 3, "Hel", 0, False, t=12)
        c.agent.turn_end("tab-1", 3)
        c.agent.ask("tab-1", "permission", "Run it?", ["yes", "no"])
        c.agent.earcon("turn_done")
        c.agent.set_mute_level(2)
        c.agent.set_summaries({"enabled": True})
        c.system.set_audio_mode("duck")
        c.system.set_duck_level(30)
        c.system.set_hotkeys({"play": "Ctrl+Alt+P"})
    assert _sent(f) == [
        {"type": "channel_open", "channel": "tab-1", "label": "Tab 1", "host_tab": "t1", "policy": "latest"},
        {"type": "focus", "channel": "tab-1"},
        {"type": "speak", "channel": "tab-1", "text": "Hi.", "mode": "replace"},
        {"type": "control", "channel": "tab-1", "action": "pause"},
        {"type": "control", "action": "next_channel"},
        {"type": "channel_close", "channel": "tab-1"},
        {"type": "turn_start", "channel": "tab-1", "turn": 3, "t": 12},
        {"type": "stream", "channel": "tab-1", "turn": 3, "delta": "Hel", "index": 0, "final": False, "t": 12},
        {"type": "turn_end", "channel": "tab-1", "turn": 3},
        {"type": "ask", "channel": "tab-1", "kind": "permission", "text": "Run it?", "options": ["yes", "no"]},
        {"type": "earcon", "kind": "turn_done"},
        {"type": "set", "key": "mute_level", "value": 2},
        {"type": "set", "key": "summaries", "value": {"enabled": True}},
        {"type": "set", "key": "audio_mode", "value": "duck"},
        {"type": "set", "key": "duck_level", "value": 30},
        {"type": "set", "key": "hotkeys", "value": {"play": "Ctrl+Alt+P"}},
    ]


def test_resolve_home_order():
    env = {"SONARA_HOME": r"C:\s", "LOCALAPPDATA": r"C:\l"}
    assert resolve_home(r"C:\h", env) == r"C:\h"
    assert resolve_home(None, env) == r"C:\s"
    assert resolve_home(None, {"LOCALAPPDATA": r"C:\l"}).endswith("Sonara")
    with pytest.raises(SonaraError):
        resolve_home(None, {})


def test_pid_alive_never_ends_the_process(fake):
    # os.kill(pid, 0) terminates a process on Windows; the check must not.
    f = fake()
    assert pid_alive(f.pid)
    time.sleep(0.2)
    assert f.proc.poll() is None
    assert not pid_alive(2 ** 30)


def test_the_package_has_no_runtime_dependencies():
    from pathlib import Path
    text = (Path(__file__).resolve().parent.parent / "pyproject.toml").read_text(encoding="utf-8")
    assert "dependencies = []" in text
