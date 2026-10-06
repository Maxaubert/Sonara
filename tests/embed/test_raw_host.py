"""A host with no SDK (#274): ``raw_client.py``, written only from
docs/protocol-v1.md and docs/bundling.md, against the release runtime. Each
test names the doc section it checks, so a failure points at a doc error or
a runtime that drifted from it. The doc examples themselves (the Python
example and the curl lines of protocol-v1.md, the PowerShell curl recipe of
bundling.md) run as written."""
from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest

from . import raw_client as raw
from . import support

DOCS = support.REPO / "docs"
CORE_CAPS = ("core", "speak", "control", "set", "get", "voices", "subscribe", "events.state", "events.items",
             "events.log", "engine_status", "engines")


def runtime_args(wav_dir: Path, idle: str = "2"):
    return ["--engine", "fake", "--keys", "fake", "--system", "fake", "--output", "wav:" + str(wav_dir),
            "--idle-exit", idle]


@pytest.fixture(scope="module")
def rt(runtime):
    """One runtime for the module; an anchor client keeps it alive."""
    d = Path(tempfile.mkdtemp(prefix="sonara-embed-raw-"))
    # Named Sonara, so bundling.md's "$env:LOCALAPPDATA\\Sonara" finds it.
    home = d / "Sonara"
    proc, info = raw.start_runtime(runtime, home, runtime_args(d / "wav"))
    anchor, _ = raw.hello(info)
    yield {"info": info, "home": home, "proc": proc, "wav": d / "wav", "dir": d, "anchor": anchor}
    anchor.close()
    try:
        proc.wait(15)
    except subprocess.TimeoutExpired:
        proc.kill()
    shutil.rmtree(d, ignore_errors=True)


def wait_item_end(c: raw.Tcp, item: int, timeout: float = 15) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        e = c.event(timeout)
        if e["event"] == "item" and e["item_id"] == item and e["phase"] != "started":
            return e["phase"]
    raise AssertionError("item %d did not end" % item)


# ---------------------------------------------------------------- Discovery


def test_runtime_json_has_the_documented_fields(rt):
    info = rt["info"]
    assert isinstance(info["pid"], int) and info["pid"] == rt["proc"].pid
    assert isinstance(info["port"], int) and isinstance(info["http_port"], int)
    assert re.fullmatch(r"[0-9a-f]{64}", info["token"]), "token: 64 hex characters"
    assert info["version"] == support.REPO.joinpath("bin", "runtime-version").read_text().strip()
    assert info["protocol"]["major"] == 1 and isinstance(info["protocol"]["minor"], int)
    assert set(CORE_CAPS) <= set(info["capabilities"]), info["capabilities"]
    assert info["extensions"] == ["channels", "agent", "system"], "runtime.json lists the extensions offered"
    assert re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", info["started_at"]), info["started_at"]


def test_a_second_runtime_for_the_same_home_exits_with_code_3(rt, runtime):
    r = subprocess.run([str(runtime), "--home", str(rt["home"]), "--engine", "fake"], capture_output=True,
                       text=True, timeout=15, creationflags=raw.CREATE_NO_WINDOW)
    assert r.returncode == 3
    assert "another instance is already running" in r.stderr


# --------------------------------------------------------------- TCP lines


def test_the_first_tcp_message_must_be_hello_with_the_token(rt):
    info = rt["info"]
    for first in ({"type": "speak", "text": "x"}, {"type": "hello", "token": "0" * 64}):
        c = raw.Tcp(info["port"])
        c.send(first)
        reply = c.read()
        assert reply["ok"] is False and reply["error"]["code"] == "E_AUTH", reply
        assert c.read() is None, "the connection closes"
        c.close()
    c = raw.Tcp(info["port"])
    c.send_raw(b"not json\n")
    assert c.read()["error"]["code"] == "E_AUTH"
    c.close()


def test_a_hello_that_fails_otherwise_leaves_the_connection_open(rt):
    c = raw.Tcp(rt["info"]["port"])
    r = c.request("hello", token=rt["info"]["token"], protocol={"major": 2, "minor": 0})
    assert r["error"]["code"] == "E_INCOMPATIBLE"
    r = c.request("hello", token=rt["info"]["token"], require=["no-such-capability"])
    assert r["error"]["code"] == "E_UNSUPPORTED"
    r = c.request("hello", token=rt["info"]["token"])
    assert r["ok"] is True
    c.close()


def test_hello_replies_version_protocol_capabilities_and_extensions(rt):
    c, r = raw.hello(rt["info"], extensions=["channels"])
    assert r["ok"] is True and r["id"] == "r1"
    assert r["version"] == rt["info"]["version"]
    assert r["protocol"]["major"] == 1
    assert set(CORE_CAPS) <= set(r["capabilities"])
    assert "channels" in r["extensions"] and r["unavailable"] == []
    c.close()


def test_replies_echo_any_id_and_unknown_fields_are_ignored(rt):
    c = rt["anchor"]
    c.send({"type": "get", "key": "volume", "id": {"any": ["json", 1]}, "no_such_field": True})
    r = c.read()
    while "event" in r:
        r = c.read()
    assert r == {"id": {"any": ["json", 1]}, "ok": True, "key": "volume", "value": 100}


def test_invalid_json_after_hello_is_bad_request_and_the_connection_stays_open(rt):
    c, _ = raw.hello(rt["info"])
    c.send_raw(b"{not json\n")
    assert c.read()["error"]["code"] == "E_BAD_REQUEST"
    assert c.request("get", key="rate")["ok"] is True
    assert c.request("no_such_type")["error"]["code"] == "E_UNKNOWN_TYPE"
    c.send({"id": 5})
    assert c.read()["error"]["code"] == "E_BAD_REQUEST", "missing type"
    c.close()


# -------------------------------------------------------------- set / get


def test_defaults_and_set_get_errors_are_as_documented(rt):
    c = rt["anchor"]
    # Saved settings > Defaults: rate 250, volume 100; voice af_sarah only
    # with an engine that has it (the fake engine does not): null.
    assert c.request("get", key="rate")["value"] == 250
    assert c.request("get", key="volume")["value"] == 100
    assert c.request("get", key="voice")["value"] is None
    assert c.request("get", key="engine")["value"] == "fake"
    assert c.request("get", key="debug_log")["value"] is True
    assert c.request("set", key="rate", value=200) == {"id": "r%d" % c.next_id, "ok": True, "key": "rate",
                                                         "value": 200}
    for key, value, code in (("rate", 99, "E_BAD_REQUEST"), ("rate", 401, "E_BAD_REQUEST"),
                             ("volume", "loud", "E_BAD_REQUEST"), ("voice", "no-such-voice", "E_NOT_FOUND"),
                             ("engine", "no-such-engine", "E_NOT_FOUND"), ("no_such_key", 1, "E_BAD_REQUEST"),
                             ("audio_mode", "duck", "E_UNSUPPORTED")):
        r = c.request("set", key=key, value=value)
        assert r["ok"] is False and r["error"]["code"] == code, (key, value, r)
    voices = c.request("voices")["voices"]
    for v in voices:
        assert set(v) >= {"id", "name", "language", "engine", "license_class", "installed"}, v
    assert c.request("voices", engine="no-such-engine")["error"]["code"] == "E_NOT_FOUND"


# -------------------------------------------------- subscribe, speak, events


def test_subscribe_starts_with_the_state_and_speak_is_followed_to_its_end(rt):
    c, _ = raw.hello(rt["info"])
    r = c.request("subscribe", events=["state", "items"])
    assert r["ok"] is True and r["events"] == ["state", "items"]
    first = c.event()
    assert first["event"] == "state" and isinstance(first["seq"], int)
    for k in ("now_playing", "queued", "paused", "muted", "volume", "rate", "voice", "engine_status"):
        assert k in first, k
    item = c.request("speak", text="Hello from a raw client.", label="raw")["item_id"]
    assert isinstance(item, int) and item >= 1
    seqs, playing, phases = [first["seq"]], [], []
    while not phases or phases[-1] == "started":
        e = c.event()
        if e["event"] == "state":
            seqs.append(e["seq"])
            if e["now_playing"]:
                playing.append(e["now_playing"])
        elif e["item_id"] == item:
            phases.append(e["phase"])
    assert phases == ["started", "finished"]
    assert seqs == sorted(set(seqs)), "seq strictly increasing"
    assert playing[0]["item_id"] == item and playing[0]["label"] == "raw"
    assert playing[0]["text"] == "Hello from a raw client." and playing[0]["chunks"] == 1
    # And it was heard: the fake engine's tone, in the WAV output.
    audio = support.joined(support.item_wavs(rt["wav"])[item])
    assert audio.rate == support.FAKE_RATE and audio.peak == support.FAKE_AMPLITUDE
    assert c.request("subscribe", events=["nope"])["error"]["code"] == "E_UNSUPPORTED"
    assert c.request("subscribe", events=[])["events"] == []
    c.close()


def test_every_control_action_and_restart_when_idle(rt):
    c, _ = raw.hello(rt["info"])
    c.request("subscribe", events=["items"])
    item = c.request("speak", text="Restart me.")["item_id"]
    assert wait_item_end(c, item) == "finished"
    for action in ("play", "pause", "toggle", "toggle", "skip", "previous", "next", "mute", "unmute"):
        assert c.request("control", action=action) == {"id": "r%d" % c.next_id, "ok": True}, action
    # restart when idle: the last item that ended, again, as a new item.
    assert c.request("control", action="restart")["ok"] is True
    e = c.event()
    assert e["phase"] == "started" and e["item_id"] > item
    assert wait_item_end(c, e["item_id"]) == "finished"
    assert c.request("control", action="stop")["ok"] is True
    assert c.request("control", action="dance")["error"]["code"] == "E_BAD_REQUEST"
    # After stop, restart replays nothing.
    c.request("control", action="restart")
    with pytest.raises(OSError):
        c.event(timeout=1.0)
    c.close()


# --------------------------------------------------------------- HTTP, SSE


def test_http_speak_with_the_bearer_token_and_follow_it_on_sse(rt):
    info = rt["info"]
    sse = raw.Sse(info, "state,items")
    assert sse.status == 200 and sse.content_type.startswith("text/event-stream")
    status, r = raw.post_json(info, "speak", text="Hello over HTTP.")
    assert status == 200 and r["ok"] is True
    item = r["item_id"]
    names, phases = set(), []
    for name, data in sse:
        names.add(name)
        if name == "item" and data["item_id"] == item:
            phases.append(data["phase"])
            if data["phase"] != "started":
                break
    sse.close()
    assert phases == ["started", "finished"] and names == {"state", "item"}


def test_http_status_codes_are_as_documented(rt):
    info = rt["info"]
    assert raw.post(info, "get", b'{"key": "rate"}', token=None)[0] == 401
    status, r = raw.post(info, "get", b'{"key": "rate"}', token="f" * 64)
    assert (status, r["error"]["code"]) == (401, "E_AUTH")
    status, r = raw.post_json(info, "no_such_type")
    assert (status, r["error"]["code"]) == (404, "E_UNKNOWN_TYPE")
    status, r = raw.post_json(info, "set", key="voice", value="no-such-voice")
    assert (status, r["error"]["code"]) == (404, "E_NOT_FOUND")
    status, r = raw.post_json(info, "subscribe", events=["state"])
    assert (status, r["error"]["code"]) == (400, "E_BAD_REQUEST")
    # An empty body is {}: a get without its key is a bad request.
    status, r = raw.post(info, "get", b"")
    assert (status, r["error"]["code"]) == (400, "E_BAD_REQUEST")
    status, r = raw.post(info, "voices", b"")
    assert status == 200 and isinstance(r["voices"], list)
    status, r = raw.post_json(info, "hello")
    assert status == 200 and r["ok"] is True and r["protocol"]["major"] == 1
    status, r = raw.post_json(info, "engine_add", engine={"id": "x", "kind": "command", "command": ["x.exe"]})
    assert (status, r["error"]["code"]) == (403, "E_FORBIDDEN")


# --------------------------------------------------------------- channels


def test_channels_messages_and_replies_are_as_documented(rt):
    c, r = raw.hello(rt["info"], extensions=["channels"])
    assert "channels" in r["extensions"]
    c.request("subscribe", events=["items"])
    r = c.request("channel_open", channel="raw-a", label="Raw A")
    assert (r["channel"], r["created"], r["policy"]) == ("raw-a", True, "latest")
    r = c.request("speak", channel="raw-a", text="Into a channel.")
    assert r["channel"] == "raw-a" and r["dropped"] == 0 and "item_id" in r
    item = r["item_id"]
    if item is None:  # waiting in its channel: the next started item is it
        item = c.event()["item_id"]
    assert wait_item_end(c, item) == "finished"
    assert c.request("focus", channel="no-such-channel")["error"]["code"] == "E_NOT_FOUND"
    assert c.request("channel_open", channel="")["error"]["code"] == "E_BAD_REQUEST"
    r = c.request("control", action="next_channel")
    assert r["ok"] is True and r["channel"] == "raw-a"
    r = c.request("control", action="flush")
    assert r["ok"] is True and set(r) >= {"flushed", "channel", "scope", "others"}
    assert c.request("control", action="flush", channel="raw-a")["error"]["code"] == "E_BAD_REQUEST"
    assert c.request("channel_close", channel="raw-a")["ok"] is True
    c.request("control", action="stop")
    c.close()


# ------------------------------------------------------------ doc examples


def _code_block(doc: Path, after: str, lang: str) -> str:
    text = doc.read_text(encoding="utf-8")
    start = text.index(after)
    m = re.compile(r"```" + lang + r"\n(.*?)```", re.S).search(text, start)
    assert m, f"no ```{lang} block after {after!r} in {doc.name}"
    return m.group(1)


def test_the_python_example_of_protocol_v1_runs_as_written(rt):
    code = _code_block(DOCS / "protocol-v1.md", "Python, standard library only", "python")
    script = rt["dir"] / "doc_example.py"
    script.write_text(code, encoding="utf-8")
    r = subprocess.run([sys.executable, str(script)], capture_output=True, text=True, timeout=30,
                       env=raw.env_with(SONARA_HOME=str(rt["home"])))
    assert r.returncode == 0, r.stderr
    lines = r.stdout.splitlines()
    assert any("'phase': 'finished'" in ln for ln in lines), r.stdout
    assert any("'id': 's1'" in ln and "'ok': True" in ln for ln in lines), r.stdout


def test_the_curl_examples_of_protocol_v1_run_as_written(rt):
    bash = shutil.which("bash")
    curl = shutil.which("curl")
    if not (bash and curl):
        pytest.skip("bash and curl are needed")
    lines = [ln for ln in _code_block(DOCS / "protocol-v1.md", "## Examples", "sh").splitlines() if ln.strip()]
    info = rt["info"]
    env = raw.env_with(TOKEN=info["token"], HTTP_PORT=str(info["http_port"]), PORT=str(info["port"]))
    # speak and pause; the event stream line runs for 2 s.
    for line in lines[:2]:
        r = subprocess.run([bash, "-c", line], capture_output=True, text=True, timeout=15, env=env)
        assert r.returncode == 0 and json.loads(r.stdout)["ok"] is True, (line, r.stdout, r.stderr)
    r = subprocess.run([bash, "-c", lines[2].replace("curl -sN", "curl -sN --max-time 2")],
                       capture_output=True, text=True, timeout=15, env=env)
    assert "event: state" in r.stdout, r.stdout
    raw.post_json(info, "control", action="stop")


@pytest.mark.parametrize("shell", ["pwsh", "powershell"])
def test_the_powershell_recipe_of_bundling_md_runs_as_written(rt, shell):
    """In PowerShell 7 (which passes quotes to programs as they are since
    7.3) and Windows PowerShell 5.1 alike."""
    exe = shutil.which(shell)
    if not exe:
        pytest.skip(f"{shell} is not installed")
    lines = [ln for ln in _code_block(DOCS / "bundling.md", "## curl (any language)", "powershell").splitlines()
             if ln.strip()]
    assert lines[0].startswith("$rt = Get-Content"), lines[0]
    # The runtime.json read and the two requests; the event stream for 2 s.
    script = "\n".join(lines[:3] + [lines[3].replace("curl.exe -sN", "curl.exe -sN --max-time 2")])
    r = subprocess.run([exe, "-NoProfile", "-Command", script], capture_output=True, text=True, timeout=30,
                       env=raw.env_with(LOCALAPPDATA=str(rt["home"].parent)))
    out = r.stdout
    assert "E_BAD_REQUEST" not in out, out
    assert re.search(r'"item_id":\s*\d+', out) and '{"ok":true}' in out, (out, r.stderr)
    assert "event: state" in out, out
    raw.post_json(rt["info"], "control", action="stop")


# ------------------------------------------------- extensions and lifetime


def test_every_extension_of_runtime_json_is_offered(runtime):
    """bundling.md "Extensions" vs protocol-v1.md: the runtime offers
    channels, agent and system."""
    d = Path(tempfile.mkdtemp(prefix="sonara-embed-ext-"))
    proc, info = raw.start_runtime(runtime, d / "home", runtime_args(d / "wav", "1"))
    try:
        c, r = raw.hello(info, extensions=["channels", "agent", "system"])
        assert sorted(r["extensions"]) == ["agent", "channels", "system"] and r["unavailable"] == []
        assert c.request("get", key="audio_mode")["value"] == "pause"
        c.close()
        proc.wait(15)
    finally:
        if proc.poll() is None:
            proc.kill()
        shutil.rmtree(d, ignore_errors=True)


def test_an_idle_takeover_ends_the_runtime(runtime):
    """Takeover: idle, it replies ok with takeover true, exits 0 and
    removes runtime.json; busy, E_BUSY."""
    d = Path(tempfile.mkdtemp(prefix="sonara-embed-takeover-"))
    proc, info = raw.start_runtime(runtime, d / "home", runtime_args(d / "wav", "30"))
    try:
        c, _ = raw.hello(info)
        c.request("speak", text="Busy for a moment. " * 20)
        t = raw.Tcp(info["port"])
        assert t.request("hello", token=info["token"], takeover=True)["error"]["code"] == "E_BUSY"
        c.request("control", action="stop")
        r = t.request("hello", token=info["token"], takeover=True)
        assert r["ok"] is True and r["takeover"] is True
        assert proc.wait(10) == 0
        assert not (d / "home" / "runtime.json").exists()
    finally:
        if proc.poll() is None:
            proc.kill()
        shutil.rmtree(d, ignore_errors=True)


def test_the_runtime_exits_when_the_last_client_left(rt):
    """Lifetime: --idle-exit after the last client left, runtime.json removed."""
    rt["anchor"].close()
    assert rt["proc"].wait(15) == 0
    assert not (rt["home"] / "runtime.json").exists()
