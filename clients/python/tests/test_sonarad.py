"""End to end against a real sonarad.exe (fake engine, silent output)."""
from __future__ import annotations

import time

import pytest
from conftest import kill, read_json, wait_until

from sonara_client import SonaraError, connect
from sonara_client.discovery import pid_alive

# The fake engine reads 10 ms per character: about 3 s per sentence.
LONG = "This sentence is long so that it keeps playing " + "and playing " * 20 + "for a while."
ARGS = ["--engine", "fake", "--idle-exit", "5"]


@pytest.fixture
def runtime(home, sonarad):
    """connect() bound to this home and build; every runtime is ended at teardown."""
    clients = []
    pids = set()

    def _connect(**kw):
        kw.setdefault("runtime_path", str(sonarad))
        kw.setdefault("runtime_args", ARGS)
        c = connect(kw.pop("name", "py-e2e"), home=str(home), **kw)
        clients.append(c)
        pids.add(c.runtime["pid"])
        return c

    _connect.pids = pids
    yield _connect
    for c in clients:
        c.close()
    info = read_json(home / "runtime.json")
    if info:
        pids.add(info["pid"])
    for pid in pids:
        kill(pid)
    wait_until(lambda: not any(pid_alive(p) for p in pids), 5)


def _until(sub, pred, timeout=10.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        e = sub.read(timeout=max(0.01, end - time.monotonic()))
        if e is not None and pred(e):
            return e
    raise AssertionError("no matching event")


def test_autostarts_speaks_and_follows_the_item_to_its_end(runtime, home):
    c = runtime()
    assert read_json(home / "runtime.json")["pid"] == c.runtime["pid"]
    assert "speak" in c.info["capabilities"]
    with c.subscribe(["items"]) as sub:
        item = c.speak("Hello from the Python client.", label="hello")
        phases = []
        for e in sub:
            if e["item_id"] == item:
                phases.append(e["phase"])
            if phases and phases[-1] != "started":
                break
    assert phases == ["started", "finished"]


def test_a_second_client_shares_the_instance(runtime):
    assert runtime().runtime["pid"] == runtime().runtime["pid"]


def test_state_follows_now_playing_pause_and_stop(runtime):
    c = runtime()
    with c.subscribe(["state"]) as sub:
        assert sub.read(timeout=10)["now_playing"] is None
        item = c.speak(LONG, label="long")
        s = _until(sub, lambda e: (e["now_playing"] or {}).get("item_id") == item)
        assert s["now_playing"]["label"] == "long"
        c.control("pause")
        _until(sub, lambda e: e["paused"] is True)
        c.control("stop")
        _until(sub, lambda e: e["now_playing"] is None)


def test_set_get_voices_and_coded_errors(runtime):
    c = runtime()
    assert c.set("volume", 35) == 35
    assert c.get("volume") == 35
    voices = c.voices("fake")
    assert voices and voices[0]["engine"] == "fake"
    with pytest.raises(SonaraError) as e:
        c.set("volume", 101)
    assert e.value.code == "E_BAD_REQUEST"


def test_extension_namespaces_reach_the_runtime(runtime):
    c = runtime(extensions=["channels", "made-up"])
    assert c.info["unavailable"] == ["made-up"]
    c.channels.open("tab")                     # enabled: the runtime accepts it
    with pytest.raises(SonaraError) as e:
        c.system.set_audio_mode("duck")        # not requested: still unsupported
    assert e.value.code == "E_UNSUPPORTED"


def test_takes_over_an_idle_incompatible_instance(runtime, home, sonarad):
    a = runtime()
    old = a.runtime["pid"]
    # The bundled runtime is the same build and lacks `engine.kokoro` too: the old
    # one steps down, a new one starts, and the client then gives up.
    with pytest.raises(SonaraError) as e:
        connect("needs-kokoro", home=str(home), runtime_path=str(sonarad), runtime_args=ARGS,
                require=["engine.kokoro"])
    assert e.value.code == "E_INCOMPATIBLE"
    assert wait_until(lambda: not pid_alive(old), 5)
    fresh = read_json(home / "runtime.json")
    assert fresh and fresh["pid"] != old
    runtime.pids.add(fresh["pid"])


def test_never_takes_over_a_busy_instance(runtime, home, sonarad):
    a = runtime()
    a.speak(LONG)
    time.sleep(0.1)
    with pytest.raises(SonaraError) as e:
        connect("needs-kokoro", home=str(home), runtime_path=str(sonarad), require=["engine.kokoro"],
                takeover_timeout=0.8)
    assert e.value.code == "E_INCOMPATIBLE"
    assert pid_alive(a.runtime["pid"])
    assert read_json(home / "runtime.json")["pid"] == a.runtime["pid"]
    a.control("stop")
