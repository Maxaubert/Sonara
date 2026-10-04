"""The ``command`` kind of external engines (#226, ``docs/protocol-v1.md``
"External engines"): a program of the user's own on this PC speaks. The
program here is this Python interpreter (an ``.exe`` with a full path)
running a small script that reads the text on stdin and prints a WAV. No
shell is involved, the text never leaves the PC, and a failing program makes
the runtime read with its built-in engine."""
from __future__ import annotations

import json
import sys
from pathlib import Path

SCRIPT = r'''
import io, json, os, sys, wave
text = sys.stdin.buffer.read().decode("utf-8")
with open(sys.argv[1], "a", encoding="utf-8") as f:
    f.write(json.dumps({"text": text, "args": sys.argv[2:],
                        "key": os.environ.get("SONARA_ENGINE_KEY")}) + "\n")
if "--fail" in sys.argv:
    print("the voice model is missing", file=sys.stderr)
    sys.exit(2)
buf = io.BytesIO()
w = wave.open(buf, "wb")
w.setnchannels(1)
w.setsampwidth(2)
w.setframerate(16000)
w.writeframes(b"\x10\x00" * 3200)
w.close()
sys.stdout.buffer.write(buf.getvalue())
'''


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def program(tmp_path: Path, *extra: str) -> tuple[dict, Path]:
    script = tmp_path / "say.py"
    script.write_text(SCRIPT, encoding="utf-8")
    record = tmp_path / "record.jsonl"
    profile = {"id": "say", "kind": "command", "label": "Say",
               "options": {"argv": [sys.executable, str(script), str(record), "--voice", "{voice}",
                                    *extra],
                           "voices": ["amy", "joe"], "timeout_ms": 20000}}
    return profile, record


def records(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]


def test_a_program_speaks_on_this_pc(client, tmp_path):
    profile, record = program(tmp_path)
    view = ok(client, {"type": "engine_add", "engine": profile})["engine"]
    assert view["kind"] == "command"
    assert view["key_ref"] == "none" and view["key_present"] is False
    assert view["sends_text_to"] == f"program {Path(sys.executable).name}"
    assert view["local"] is True
    assert view["license_class"] == "external"
    voices = ok(client, {"type": "voices", "engine": "say", "refresh": True})["voices"]
    assert [v["id"] for v in voices] == ["amy", "joe"]
    ok(client, {"type": "set", "key": "engine", "value": "say"})
    ok(client, {"type": "set", "key": "voice", "value": "joe"})
    ok(client, {"type": "subscribe", "events": ["items"]})
    item = ok(client, {"type": "speak", "text": "Hello from a program."})["item_id"]
    client.item(item, "finished")
    seen = records(record)
    assert seen, "the program never ran"
    assert seen[0]["text"] == "Hello from a program."
    assert seen[0]["args"] == ["--voice", "joe"]
    assert seen[0]["key"] is None
    # No shell: characters a shell would act on reach the program as text
    # (engine_test sends its text as given, without the reading rules).
    text = 'Fish & chips | "quoted" > not a file %PATH%'
    r = ok(client, {"type": "engine_test", "engine": "say", "text": text, "play": False})
    assert r["sample_rate"] == 16000 and r["duration_ms"] == 200
    assert records(record)[-1]["text"] == text
    assert not (tmp_path / "not").exists()


def test_a_missing_program_is_refused_when_added(client, tmp_path):
    missing = str(tmp_path / "no-such-tts.exe")
    r = client.request({"type": "engine_add", "engine": {
        "id": "gone", "kind": "command", "options": {"argv": [missing]}}})
    assert r["ok"] is False
    assert r["error"]["code"] == "E_BAD_REQUEST"
    assert "does not exist" in r["error"]["message"]
    r = client.request({"type": "engine_add", "engine": {
        "id": "bat", "kind": "command", "options": {"argv": ["C:\\Tools\\say.bat"]}}})
    assert r["error"]["code"] == "E_BAD_REQUEST"
    assert ".exe" in r["error"]["message"]


def test_a_failing_program_falls_back_and_reports_server(client, tmp_path):
    profile, record = program(tmp_path, "--fail")
    ok(client, {"type": "engine_add", "engine": profile})
    r = client.request({"type": "engine_test", "engine": "say", "play": False})
    assert r["error"]["code"] == "E_ENGINE"
    assert r["error"]["reason"] == "server"
    assert "the voice model is missing" in r["error"]["message"]
    assert str(tmp_path) not in r["error"]["message"], "the program's folder stays out"
    ok(client, {"type": "set", "key": "engine", "value": "say"})
    ok(client, {"type": "subscribe", "events": ["items"]})
    item = ok(client, {"type": "speak", "text": "Read by the fallback."})["item_id"]
    # The fallback (the fake engine) speaks it: finished, not failed.
    client.item(item, "finished")
    assert any(r["text"] == "Read by the fallback." for r in records(record))
