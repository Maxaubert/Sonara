"""Core: discovery, loopback binding, single instance, idle exit and
takeover (spec section 3).

The idle-exit cases are deterministic (#194): the idle countdown starts
when ``runtime.json`` is written (a slow start never uses it up before the
first client), the exit is decided atomically with requests that start
speech (one is never accepted and then lost), and a request that arrives
while the runtime exits is ``E_BUSY``, never an HTTP 500."""
from __future__ import annotations

import socket
import time

import pytest
from harness import long_text, non_loopback_addresses, wait_until


def test_runtime_json_describes_the_instance(rt):
    info = rt.info
    assert info["pid"] == rt.proc.pid
    assert isinstance(info["port"], int) and isinstance(info["http_port"], int)
    assert info["port"] != info["http_port"]
    assert len(info["token"]) >= 32
    assert info["protocol"] == {"major": 1, "minor": 0}
    assert "core" in info["capabilities"]
    assert info["version"]
    assert info["started_at"].endswith("Z")
    assert not [p for p in rt.home.iterdir() if p.name.endswith(".tmp")]


def test_only_loopback_is_bound(rt):
    addrs = non_loopback_addresses()
    if not addrs:
        pytest.skip("this machine has no non-loopback IPv4 address")
    for addr in addrs:
        for port in (rt.info["port"], rt.info["http_port"]):
            with pytest.raises(OSError):
                socket.create_connection((addr, port), timeout=2).close()


def test_a_second_instance_on_the_same_home_exits(start, rt):
    second = start(home=rt.home, wait=False)
    code = second.wait_exit()
    assert code == 3
    assert "already running" in second.stderr()
    assert rt.alive()
    assert rt.read_runtime()["pid"] == rt.proc.pid


def test_instances_on_other_homes_are_independent(start, rt, tmp_path):
    other = start(home=tmp_path / "other")
    assert other.alive() and rt.alive()
    assert other.info["port"] != rt.info["port"]


def test_idle_exit_after_the_last_client_leaves(start):
    rt = start("--idle-exit", "1")
    c = rt.tcp()
    time.sleep(2)
    assert rt.alive(), "a connected client keeps it alive"
    c.close()
    assert rt.wait_exit() == 0
    assert not rt.runtime_json.exists()


def test_idle_exit_with_no_client_at_all(start):
    rt = start("--idle-exit", "0.5")
    assert rt.wait_exit() == 0
    assert not rt.runtime_json.exists()


def test_keep_alive_survives_the_last_client(start):
    rt = start("--idle-exit", "0.5")
    c = rt.tcp(keep_alive=True)
    c.close()
    time.sleep(2)
    assert rt.alive()


def test_playing_keeps_it_alive_without_clients(start):
    rt = start("--idle-exit", "0.5")
    status, _ = rt.post("speak", {"text": long_text(2)})
    assert status == 200
    # The item lasts about 6 s: well past the idle time, and well before
    # its end, it must still be reading.
    time.sleep(2.0)
    assert rt.alive(), "it reads to the end first"
    assert rt.wait_exit(20) == 0


def test_a_paused_item_does_not_keep_it_alive(start):
    rt = start("--idle-exit", "0.5")
    rt.post("speak", {"text": long_text(2)})
    status, _ = rt.post("control", {"action": "pause"})
    assert status == 200
    assert rt.wait_exit(10) == 0


def test_an_open_event_stream_counts_as_a_client(start):
    rt = start("--idle-exit", "0.5")
    sse = rt.sse("state")
    assert sse.status == 200
    assert sse.next_event(lambda name, e: name == "state")[1]["now_playing"] is None
    time.sleep(1.5)
    assert rt.alive()
    sse.close()
    assert rt.wait_exit() == 0


def test_the_idle_time_starts_when_clients_can_find_it(start):
    # #194: the countdown started before runtime.json was written, so a
    # slow start could use it up and the runtime left right after a client
    # arrived. Many quick starts with a short idle time: each first request
    # finds the runtime alive and is served.
    for _ in range(5):
        rt = start("--idle-exit", "0.3")
        status, r = rt.post("speak", {"text": long_text(1)})
        assert status == 200, r
        assert r["item_id"] == 1
        time.sleep(0.5)
        assert rt.alive(), "the accepted item is being read"
        rt.close()


def test_a_request_while_it_exits_is_busy_not_an_error(start):
    # #194: an SSE subscription racing the idle exit got HTTP 500 (the
    # reader was already shut down). Hammer the window: every reply is a
    # success or E_BUSY until the process is gone.
    rt = start("--idle-exit", "0.2")
    statuses = set()
    end = time.monotonic() + 10
    while rt.alive() and time.monotonic() < end:
        try:
            status, body = rt.post("get", {"key": "volume"})
        except OSError:
            break
        statuses.add(status)
        if status != 200:
            assert body["error"]["code"] == "E_BUSY", body
        time.sleep(0.3)
    assert rt.wait_exit() == 0
    assert statuses <= {200, 409}, statuses  # 409: E_BUSY


def test_standalone_never_idles_out(start):
    rt = start("--idle-exit", "0.2", "--standalone")
    time.sleep(1.5)
    assert rt.alive()


def test_takeover_when_idle(rt):
    c = rt.tcp(hello=False)
    r = c.hello(rt.token, takeover=True, protocol={"major": 2, "minor": 0})
    assert r["ok"] is True
    assert r["takeover"] is True
    assert c.closed()
    assert rt.wait_exit() == 0
    assert not rt.runtime_json.exists()


def test_takeover_when_busy_is_e_busy_then_succeeds_once_idle(rt, client):
    client.request({"type": "speak", "text": long_text(2)})
    other = rt.tcp(hello=False)
    r = other.hello(rt.token, takeover=True)
    assert r["error"]["code"] == "E_BUSY"
    assert rt.alive()
    client.request({"type": "control", "action": "pause"})
    assert other.hello(rt.token, takeover=True)["error"]["code"] == "E_BUSY"
    client.request({"type": "control", "action": "stop"})
    r = other.hello(rt.token, takeover=True)
    assert r["ok"] is True
    assert rt.wait_exit() == 0
    assert not rt.runtime_json.exists()


def test_takeover_needs_the_token(rt):
    c = rt.tcp(hello=False)
    r = c.hello("wrong", takeover=True)
    assert r["error"]["code"] == "E_AUTH"
    time.sleep(0.3)
    assert rt.alive()


def test_a_new_instance_starts_after_a_takeover(start, rt):
    c = rt.tcp(hello=False)
    assert c.hello(rt.token, takeover=True)["ok"]
    assert rt.wait_exit() == 0
    new = start(home=rt.home)
    assert new.info["pid"] != rt.info["pid"]
    assert wait_until(lambda: new.read_runtime() is not None)


def test_relaunch_as_soon_as_runtime_json_is_gone_after_a_takeover(start, rt):
    c = rt.tcp(hello=False)
    assert c.hello(rt.token, takeover=True)["ok"]
    assert wait_until(lambda: not rt.runtime_json.exists(), step=0.001)
    # The instance lock is released before runtime.json goes, so a client
    # that does not wait for the pid still starts (not exit code 3).
    new = start(home=rt.home)
    assert new.info["pid"] != rt.info["pid"]
    assert rt.wait_exit() == 0
