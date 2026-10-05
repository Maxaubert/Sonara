"""The ``command`` kind of external engines (#226, ``docs/protocol-v1-engines.md``
"Kinds", "A program"): a program of the user's own on this PC speaks. The
program here is this Python interpreter (an ``.exe`` with a full path)
running a small script that reads the text on stdin and prints a WAV. No
shell is involved, the text never leaves the PC and is never an argument,
and a failing program makes the runtime read with its built-in engine.

A program is never added or changed over the protocol (security review of
PR3): ``engine_add`` of kind ``command``, or replacing one, is
``E_FORBIDDEN`` over TCP and HTTP. The user writes ``engines.json`` (or
``sonara engines add --kind command`` does, ``conformance/plugin``) and
``engine_reload`` makes the runtime read it again."""
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

FORBIDDEN = ("a command engine runs a program on this PC, so it is never added or changed over "
             "the protocol: add it with `sonara engines add <id> --kind command`, or in "
             "engines.json")


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


def install(rt, client, *profiles: dict) -> dict:
    """What the user (or ``sonara engines add --kind command``) does: write
    ``engines.json``, then ask the runtime to read it again."""
    (rt.home / "engines.json").write_text(
        json.dumps({"format": 1, "engines": list(profiles)}), encoding="utf-8")
    return ok(client, {"type": "engine_reload"})


def records(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]


def test_a_program_speaks_on_this_pc(rt, client, tmp_path):
    profile, record = program(tmp_path)
    reply = install(rt, client, profile)
    assert reply["problems"] == []
    view = reply["engines"][0]
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


def test_a_program_is_never_added_over_tcp_or_http(rt, client, tmp_path):
    profile, record = program(tmp_path)
    missing = {"id": "gone", "kind": "command",
               "options": {"argv": [str(tmp_path / "no-such-tts.exe")]}}
    for p in (profile, missing, {"id": "x", "kind": "command"}):
        for replace in (False, True):
            msg = {"type": "engine_add", "engine": p, "replace": replace}
            r = client.request(msg)
            assert r["ok"] is False
            assert r["error"] == {"code": "E_FORBIDDEN", "message": FORBIDDEN}, r
            status, body = rt.post("engine_add", {k: v for k, v in msg.items() if k != "type"})
            assert status == 403, body
            assert body["error"]["code"] == "E_FORBIDDEN"
    assert not (rt.home / "engines.json").exists()
    assert ok(client, {"type": "engine_list"})["engines"] == []
    assert records(record) == [], "the program never ran"


def test_a_local_program_is_never_replaced_over_the_protocol(rt, client, tmp_path):
    profile, record = program(tmp_path)
    install(rt, client, profile)
    before = (rt.home / "engines.json").read_text(encoding="utf-8")
    other = {"id": "say", "kind": "openai-compatible", "url": "http://127.0.0.1:9/v1",
             "key_ref": "none"}
    swapped = dict(profile, options=dict(profile["options"],
                                         argv=[sys.executable, "-c", "print(1)"]))
    for p in (other, swapped):
        r = client.request({"type": "engine_add", "engine": p, "replace": True})
        assert r["error"]["code"] == "E_FORBIDDEN"
        status, body = rt.post("engine_add", {"engine": p, "replace": True})
        assert (status, body["error"]["code"]) == (403, "E_FORBIDDEN")
    assert (rt.home / "engines.json").read_text(encoding="utf-8") == before
    # engine_reload takes no profile: what it carries is ignored.
    r = ok(client, {"type": "engine_reload", "engine": swapped, "engines": [swapped]})
    assert r["engines"][0]["options"]["argv"] == profile["options"]["argv"]
    status, body = rt.post("engine_reload", {"engine": swapped})
    assert status == 200 and body["engines"][0]["options"]["argv"] == profile["options"]["argv"]
    # Testing, selecting and removing it stay allowed.
    ok(client, {"type": "engine_test", "engine": "say", "play": False})
    ok(client, {"type": "set", "key": "engine", "value": "say"})
    assert ok(client, {"type": "engine_remove", "engine": "say"})["engine"] == "fake"


def test_arguments_with_shell_characters_reach_the_program_literally(rt, client, tmp_path):
    marker = tmp_path / "pwned"
    meta = f'a & echo x > "{marker}" | calc ^ %PATH% $(y) ; <in'
    profile, record = program(tmp_path, meta, "trailing\\", '"')
    install(rt, client, profile)
    text = 'Fish & chips | "quoted" > not a file %PATH%'
    ok(client, {"type": "engine_test", "engine": "say", "text": text, "voice": "amy",
                "play": False})
    seen = records(record)[-1]
    assert seen["args"] == ["--voice", "amy", meta, "trailing\\", '"']
    assert seen["text"] == text, "the text goes on stdin, never into argv"
    assert not marker.exists()


def test_a_bad_entry_is_listed_with_its_error(rt, client, tmp_path):
    r = install(rt, client, {"id": "typo", "kind": "command",
                             "options": {"argv": [sys.executable, "--say", "{text}"]}})
    assert r["engines"][0]["error"].startswith("{text} is not allowed in argv")
    assert len(r["problems"]) == 1
    (rt.home / "engines.json").write_text("{not json", encoding="utf-8")
    r = client.request({"type": "engine_reload"})
    assert r["error"]["code"] == "E_BAD_REQUEST"
    assert ok(client, {"type": "engine_list"})["engines"][0]["id"] == "typo"


def test_a_failing_program_falls_back_and_reports_server(rt, client, tmp_path):
    profile, record = program(tmp_path, "--fail")
    install(rt, client, profile)
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
