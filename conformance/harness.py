"""Black-box harness for protocol v1 conformance (stdlib only).

Starts a built ``sonarad.exe`` with a temporary home, the fake engine and
the silent timed output, and talks to it over TCP JSON lines and HTTP/SSE
exactly as a client would. The binary is ``$SONARAD`` if set, else the
newest of ``target/release/sonarad.exe`` and ``target/debug/sonarad.exe``.
"""
from __future__ import annotations

import json
import os
import socket
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TIMEOUT = 10.0
CREATE_NO_WINDOW = 0x08000000

# The fake engine speaks 10 ms per character at rate 200, so a chunk of
# LONG_SENTENCE lasts about 3 s and SHORT_SENTENCE about 0.2 s.
LONG_SENTENCE = "This sentence is long so that it keeps playing " + "and playing " * 20 + "for a while."
SHORT_SENTENCE = "Short one."


def long_text(n: int) -> str:
    """``n`` long sentences, so ``n`` chunks."""
    return " ".join(LONG_SENTENCE.replace("This sentence", f"Sentence {i}") for i in range(n))


def find_sonarad() -> Path | None:
    env = os.environ.get("SONARAD")
    if env:
        return Path(env)
    found = [REPO / "target" / p / "sonarad.exe" for p in ("release", "debug")]
    found = [p for p in found if p.is_file()]
    if not found:
        return None
    return max(found, key=lambda p: p.stat().st_mtime)


def find_hook() -> Path | None:
    """``$SONARA_HOOK``, else the newest built ``sonara-hook.exe``."""
    env = os.environ.get("SONARA_HOOK")
    if env:
        return Path(env)
    found = [REPO / "target" / p / "sonara-hook.exe" for p in ("release", "debug")]
    found = [p for p in found if p.is_file()]
    if not found:
        return None
    return max(found, key=lambda p: p.stat().st_mtime)


class Hook:
    """One ``sonara-hook.exe <event>`` process, as Claude Code runs it: the
    payload on stdin. Its start time (``t``) is taken when it starts, so a
    test can hold the payload back to deliver it late."""

    def __init__(self, exe: Path, home: Path, event: str, env: dict | None = None):
        e = dict(os.environ)
        e["SONARA_HOME"] = str(home)
        for k in ("SONARA_SUMMARIZER", "SONARA_CAPTURE", "SONARA_HOST_TAB", "PRISM_TAB_ID"):
            e.pop(k, None)
        e.update(env or {})
        self.proc = subprocess.Popen(
            [str(exe), event],
            env=e,
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=CREATE_NO_WINDOW,
        )

    def send(self, payload: dict) -> int:
        """Deliver the payload and wait; returns the exit code."""
        self.proc.communicate(json.dumps(payload).encode(), timeout=TIMEOUT)
        return self.proc.returncode


def wait_until(pred, timeout: float = TIMEOUT, step: float = 0.02) -> bool:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if pred():
            return True
        time.sleep(step)
    return pred()


class Runtime:
    """One sonarad process on one home."""

    def __init__(self, exe: Path, home: Path, *args: str, wait: bool = True):
        self.exe = exe
        self.home = home
        self.home.mkdir(parents=True, exist_ok=True)
        self.stderr_path = home.parent / f"{home.name}-stderr-{time.monotonic_ns()}.log"
        env = dict(os.environ)
        env["SONARA_HOME"] = str(home)
        self._stderr = open(self.stderr_path, "wb")
        self.proc = subprocess.Popen(
            [str(exe), "--engine", "fake", *args],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=self._stderr,
            creationflags=CREATE_NO_WINDOW,
        )
        self.info: dict = {}
        if wait:
            self.wait_ready()

    @property
    def runtime_json(self) -> Path:
        return self.home / "runtime.json"

    def read_runtime(self) -> dict | None:
        try:
            return json.loads(self.runtime_json.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None

    def stderr(self) -> str:
        self._stderr.flush()
        return self.stderr_path.read_text(encoding="utf-8", errors="replace")

    def wait_ready(self) -> None:
        def ready():
            info = self.read_runtime()
            return info is not None and info.get("pid") == self.proc.pid

        end = time.monotonic() + TIMEOUT
        while time.monotonic() < end:
            if ready():
                self.info = self.read_runtime() or {}
                return
            if self.proc.poll() is not None:
                raise RuntimeError(f"sonarad exited with {self.proc.returncode}: {self.stderr()}")
            time.sleep(0.02)
        raise RuntimeError(f"sonarad wrote no runtime.json: {self.stderr()}")

    @property
    def token(self) -> str:
        return self.info["token"]

    def alive(self) -> bool:
        return self.proc.poll() is None

    def wait_exit(self, timeout: float = TIMEOUT) -> int | None:
        try:
            return self.proc.wait(timeout)
        except subprocess.TimeoutExpired:
            return None

    def tcp(self, hello: bool = True, **fields) -> "TcpClient":
        c = TcpClient(self.info["port"])
        if hello:
            reply = c.hello(self.token, **fields)
            assert reply["ok"], reply
        return c

    def post(self, kind: str, body=None, token: str | None = None, raw: bytes | None = None):
        """POST /v1/<kind>; returns (status, json body)."""
        data = raw if raw is not None else json.dumps(body or {}).encode()
        req = urllib.request.Request(
            f"http://127.0.0.1:{self.info['http_port']}/v1/{kind}",
            data=data,
            method="POST",
            headers={"Content-Type": "application/json"},
        )
        tok = self.token if token is None else token
        if tok:
            req.add_header("Authorization", f"Bearer {tok}")
        return _http(req)

    def sse(self, events: str | None = None, token: str | None = None) -> "SseClient":
        return SseClient(self.info["http_port"], self.token if token is None else token, events)

    def close(self) -> None:
        if self.alive():
            self.proc.kill()
            self.proc.wait(TIMEOUT)
        self._stderr.close()


def _http(req):
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        body = e.read()
        return e.code, json.loads(body) if body else None


class TcpClient:
    """A JSON-lines connection. Replies (with ``ok``) answer requests in
    order; events (with ``event``) are queued for ``next_event``."""

    def __init__(self, port: int):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=TIMEOUT)
        self.buf = b""
        self.events: list = []
        self._id = 0

    def close(self) -> None:
        self.sock.close()

    def send_raw(self, data: bytes) -> None:
        self.sock.sendall(data)

    def send(self, msg: dict) -> None:
        self.send_raw(json.dumps(msg).encode() + b"\n")

    def read_line(self, timeout: float = TIMEOUT):
        """The next JSON line, or None at end of stream."""
        self.sock.settimeout(timeout)
        while b"\n" not in self.buf:
            chunk = self.sock.recv(65536)
            if not chunk:
                return None
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line)

    def reply(self, timeout: float = TIMEOUT):
        while True:
            msg = self.read_line(timeout)
            if msg is None or "ok" in msg:
                return msg
            self.events.append(msg)

    def request(self, msg: dict) -> dict:
        self.send(msg)
        r = self.reply()
        assert r is not None, "connection closed before the reply"
        return r

    def hello(self, token: str, **fields) -> dict:
        return self.request({"type": "hello", "token": token, "client": {"name": "conformance", "version": "1"}, **fields})

    def closed(self, timeout: float = 3.0) -> bool:
        """True when the server closed the connection."""
        try:
            while True:
                if self.read_line(timeout) is None:
                    return True
        except ConnectionError:
            return True
        except socket.timeout:
            return False

    def next_event(self, pred=lambda e: True, timeout: float = TIMEOUT):
        end = time.monotonic() + timeout
        while True:
            for i, e in enumerate(self.events):
                if pred(e):
                    return self.events.pop(i)
            left = end - time.monotonic()
            if left <= 0:
                raise AssertionError(f"no matching event; queued: {self.events}")
            try:
                msg = self.read_line(left)
            except socket.timeout:
                raise AssertionError(f"no matching event; queued: {self.events}") from None
            if msg is None:
                raise AssertionError("connection closed while waiting for an event")
            self.events.append(msg)

    def state(self, pred=lambda s: True, timeout: float = TIMEOUT) -> dict:
        return self.next_event(lambda e: e.get("event") == "state" and pred(e), timeout)

    def item(self, item_id: int, phase: str, timeout: float = TIMEOUT) -> dict:
        return self.next_event(
            lambda e: e.get("event") == "item" and e["item_id"] == item_id and e["phase"] == phase,
            timeout,
        )


class SseClient:
    """``GET /v1/events`` read line by line."""

    def __init__(self, http_port: int, token: str, events: str | None):
        url = f"http://127.0.0.1:{http_port}/v1/events"
        if events is not None:
            url += f"?events={events}"
        req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
        self.resp = urllib.request.urlopen(req, timeout=TIMEOUT)
        self.status = self.resp.status
        self.content_type = self.resp.headers.get("Content-Type", "")
        self.pending: list = []

    def _read_event(self):
        name, data = None, None
        while True:
            line = self.resp.readline()
            if not line:
                return None
            line = line.decode().rstrip("\r\n")
            if line == "":
                if data is not None:
                    return name, json.loads(data)
                continue
            if line.startswith(":"):
                continue
            field, _, value = line.partition(":")
            value = value[1:] if value.startswith(" ") else value
            if field == "event":
                name = value
            elif field == "data":
                data = value

    def next_event(self, pred=lambda name, e: True):
        for i, (n, e) in enumerate(self.pending):
            if pred(n, e):
                return self.pending.pop(i)
        while True:
            got = self._read_event()
            if got is None:
                raise AssertionError("event stream ended")
            if pred(*got):
                return got
            self.pending.append(got)

    def close(self) -> None:
        self.resp.close()


def non_loopback_addresses() -> list:
    """This PC's IPv4 addresses other than 127.x (may be empty)."""
    addrs = set()
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET):
            addrs.add(info[4][0])
    except OSError:
        pass
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("192.0.2.1", 9))  # no packet is sent for UDP connect
        addrs.add(s.getsockname()[0])
        s.close()
    except OSError:
        pass
    return sorted(a for a in addrs if not a.startswith("127.") and a != "0.0.0.0")
