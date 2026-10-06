"""A protocol v1 client written only from docs/protocol-v1.md and
docs/bundling.md (#274), the way a developer without an SDK would: nothing
here imports or copies the SDKs or the runtime's code, so a test that
fails here and passes with the SDKs points at the docs.

Discovery (protocol-v1.md "Discovery"): start ``sonarad.exe --home <home>``
with no console window and wait up to 5 s for a ``runtime.json`` whose
``pid`` is the new process. TCP JSON lines (one object per line, ``hello``
with the token first, replies with ``ok`` and events with ``event`` on the
same connection). HTTP: ``POST /v1/<type>`` with ``Authorization: Bearer
<token>``, the body without ``type``; ``GET /v1/events`` is Server-Sent
Events.
"""
from __future__ import annotations

import http.client
import json
import os
import socket
import subprocess
import time
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Tuple

CREATE_NO_WINDOW = 0x08000000


def start_runtime(exe: Path, home: Path, args: List[str]) -> Tuple[subprocess.Popen, dict]:
    proc = subprocess.Popen([str(exe), "--home", str(home), *args], stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                            creationflags=CREATE_NO_WINDOW)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        info = read_runtime(home)
        if info and info.get("pid") == proc.pid:
            return proc, info
        time.sleep(0.05)
    proc.kill()
    raise AssertionError("no runtime.json with the new pid within 5 s")


def read_runtime(home: Path) -> Optional[dict]:
    try:
        return json.loads((Path(home) / "runtime.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


class Tcp:
    """One TCP JSON-lines connection. Replies and events share it: replies
    come in request order, events in between are kept."""

    def __init__(self, port: int, timeout: float = 10.0):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=timeout)
        self.file = self.sock.makefile("rwb")
        self.events: List[dict] = []
        self.next_id = 0

    def send_raw(self, line: bytes) -> None:
        self.file.write(line)
        self.file.flush()

    def send(self, msg: dict) -> None:
        self.send_raw(json.dumps(msg).encode("utf-8") + b"\n")

    def read(self) -> Optional[dict]:
        """The next line, or None when the runtime closed the connection."""
        line = self.file.readline()
        if not line:
            return None
        return json.loads(line)

    def request(self, type_: str, **fields: Any) -> dict:
        """Send with a fresh ``id``; the reply that echoes it."""
        self.next_id += 1
        rid = "r%d" % self.next_id
        self.send({"type": type_, "id": rid, **fields})
        while True:
            msg = self.read()
            if msg is None:
                raise ConnectionError("closed before the reply to %s" % type_)
            if "event" in msg:
                self.events.append(msg)
                continue
            assert msg.get("id") == rid, "replies come in request order: %r" % msg
            return msg

    def event(self, timeout: float = 10.0) -> dict:
        if self.events:
            return self.events.pop(0)
        self.sock.settimeout(timeout)
        msg = self.read()
        if msg is None:
            raise ConnectionError("closed")
        assert "event" in msg, msg
        return msg

    def close(self) -> None:
        try:
            self.file.close()
        finally:
            self.sock.close()


def hello(rt: dict, port: Optional[int] = None, **fields: Any) -> Tuple[Tcp, dict]:
    c = Tcp(port or rt["port"])
    reply = c.request("hello", token=rt["token"], client={"name": "raw-host", "version": "1"},
                      protocol={"major": 1, "minor": 0}, **fields)
    return c, reply


def post(rt: dict, type_: str, body: Optional[bytes] = None, token: Optional[str] = "__rt__",
         path: Optional[str] = None) -> Tuple[int, dict]:
    """``POST /v1/<type>``; (status, reply JSON)."""
    conn = http.client.HTTPConnection("127.0.0.1", rt["http_port"], timeout=10)
    headers = {"Content-Type": "application/json"}
    if token is not None:
        headers["Authorization"] = "Bearer " + (rt["token"] if token == "__rt__" else token)
    conn.request("POST", path or "/v1/" + type_, body=body if body is not None else b"", headers=headers)
    r = conn.getresponse()
    data = r.read()
    conn.close()
    return r.status, json.loads(data)


def post_json(rt: dict, type_: str, **fields: Any) -> Tuple[int, dict]:
    return post(rt, type_, json.dumps(fields).encode("utf-8"))


class Sse:
    """``GET /v1/events?events=...``: ``event: <name>`` then ``data: <json>``,
    a blank line ends an event, ``:`` lines are comments (pings)."""

    def __init__(self, rt: dict, events: Optional[str] = None, timeout: float = 10.0):
        self.conn = http.client.HTTPConnection("127.0.0.1", rt["http_port"], timeout=timeout)
        path = "/v1/events" + ("?events=" + events if events else "")
        self.conn.request("GET", path, headers={"Authorization": "Bearer " + rt["token"]})
        self.resp = self.conn.getresponse()
        self.status = self.resp.status
        self.content_type = self.resp.getheader("Content-Type") or ""

    def __iter__(self) -> Iterator[Tuple[str, dict]]:
        name, data = None, []
        while True:
            line = self.resp.fp.readline()
            if not line:
                return
            line = line.decode("utf-8").rstrip("\r\n")
            if line == "":
                if name is not None:
                    yield name, json.loads("\n".join(data))
                name, data = None, []
            elif line.startswith(":"):
                continue
            elif line.startswith("event:"):
                name = line[len("event:"):].strip()
            elif line.startswith("data:"):
                data.append(line[len("data:"):].strip())

    def close(self) -> None:
        self.conn.close()


def env_with(**extra: str) -> Dict[str, str]:
    env = dict(os.environ)
    env.update(extra)
    return env
