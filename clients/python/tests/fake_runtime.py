"""A stand-in for sonarad.exe in unit tests (stdlib only): a JSON-lines
server that writes runtime.json, records every request in
<home>/requests.jsonl and answers from a small script. Run as a child
process so its pid can end (an accepted takeover).

    python fake_runtime.py '<options as JSON>'

Options: home (required), hello_error (a code every plain hello fails
with), busy_takeovers (takeover hellos answered E_BUSY first; -1: always),
protocol_major (in the hello reply).
"""
import json
import os
import socket
import sys
import threading
from datetime import datetime, timezone

OPTS = json.loads(sys.argv[1])
HOME = OPTS["home"]
TOKEN = "t0k3n"
CAPS = ["core", "speak", "control", "set", "get", "voices", "subscribe", "events.state", "events.items", "events.log"]
state = {"busy": OPTS.get("busy_takeovers", 0), "item": 1, "settings": {"volume": 100}}
lock = threading.Lock()


def hello_fields():
    return {"version": "9.9.9", "protocol": {"major": OPTS.get("protocol_major", 1), "minor": 0},
            "capabilities": CAPS, "extensions": [], "unavailable": []}


def serve(conn):
    f = conn.makefile("rwb")

    def send(obj):
        f.write(json.dumps(obj).encode() + b"\n")
        f.flush()

    for line in f:
        msg = json.loads(line)
        with lock:
            with open(os.path.join(HOME, "requests.jsonl"), "a", encoding="utf-8") as log:
                log.write(json.dumps(msg) + "\n")
        mid = msg.get("id")

        def ok(mid=mid, **fields):
            send({"id": mid, "ok": True, **fields})

        def fail(code, message="scripted", mid=mid):
            send({"id": mid, "ok": False, "error": {"code": code, "message": message}})

        t = msg.get("type")
        if t == "hello":
            if msg.get("token") != TOKEN:
                fail("E_AUTH")
            elif msg.get("takeover"):
                if state["busy"] != 0:
                    if state["busy"] > 0:
                        state["busy"] -= 1
                    fail("E_BUSY", "something is playing")
                else:
                    ok(takeover=True, **hello_fields())
                    conn.close()
                    os.remove(os.path.join(HOME, "runtime.json"))
                    os._exit(0)
            elif OPTS.get("hello_error"):
                fail(OPTS["hello_error"])
            else:
                ok(**hello_fields())
        elif t == "speak":
            ok(item_id=state["item"])
            state["item"] += 1
        elif t == "set":
            state["settings"][msg["key"]] = msg["value"]
            ok(key=msg["key"], value=msg["value"])
        elif t == "get":
            ok(key=msg["key"], value=state["settings"].get(msg["key"]))
        elif t == "voices":
            ok(voices=[{"id": "fake-1", "name": "Fake", "language": "en-US", "engine": "fake",
                        "license_class": "permissive", "installed": True}])
        elif t == "subscribe":
            ok(events=msg.get("events"))
            send({"event": "state", "seq": 1, "now_playing": None, "queued": 0, "paused": False,
                  "muted": False, "volume": 100, "rate": 200, "voice": None, "engine_status": {"engine": "fake"}})
            send({"event": "item", "item_id": 1, "phase": "started"})
        elif t == "fail_me":
            fail(msg["code"])
        elif t == "close_me":
            conn.close()
            return
        else:
            ok(echoed=t)


def main():
    srv = socket.socket()
    srv.bind(("127.0.0.1", 0))
    srv.listen()
    info = {"pid": os.getpid(), "port": srv.getsockname()[1], "http_port": 0, "token": TOKEN,
            "version": "9.9.9", "protocol": {"major": OPTS.get("protocol_major", 1), "minor": 0},
            "capabilities": CAPS, "started_at": datetime.now(timezone.utc).isoformat()}
    tmp = os.path.join(HOME, "runtime.json.tmp")
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(info, f)
    os.replace(tmp, os.path.join(HOME, "runtime.json"))
    while True:
        conn, _ = srv.accept()
        threading.Thread(target=serve, args=(conn,), daemon=True).start()


if __name__ == "__main__":
    main()
