"""Golden cases for the Rust hook adapter (crates/sonara-hook, L5): every
case in crates/sonara-hook/tests/golden/ is a Claude Code hook event (the
captured payloads in tests/fixtures/ and a few inline ones) with the
protocol v1 messages it must become. This test proves the golden messages
are the Python mapping (hooks_entry.handle_event) adapted to the new message
names; crates/sonara-hook/tests/golden.rs proves the Rust mapping produces
them. Together: both mappings agree until the Python plugin is removed (M11).

The adaptation (Python message -> protocol v1):
- PROSE -> stream; EARCON turn_done -> turn_end.
- EARCON choice + CHOICE -> one ask "question" per question (the earcon is
  the ask's own), the TUI notes and selection hints on the last one.
- PLAN -> ask "plan"; EARCON permission + PERMISSION -> ask "permission"
  (text: the action, else the message), both with the selection hints.
- TOOL -> tool; CHOICE_ANSWERED -> answered.
- SET_FOREGROUND -> channel_open (label: the cwd's folder; host_tab) + focus;
  FLUSH -> turn_start; SESSION_START adds nothing more (its plugin_version
  and plugin_root fed the Python setup guide, which is not part of L5);
  SESSION_END -> channel_close.
- A missing session id is the channel "default" (protocol v1 needs one).
- Every agent message that names the session (stream, tool, ask, answered,
  turn_end) carries the payload's cwd folder as "label" (#241; the Python
  plugin sent it only with SET_FOREGROUND).

Set SONARA_REGEN_GOLDEN=1 to rewrite the expected messages from the Python
mapping (then review the diff)."""
from __future__ import annotations

import json
import ntpath
import os
from pathlib import Path

import pytest

from sonara.daemon import decision_text
from sonara.hooks_entry import handle_event
from sonara.protocol import MsgType

REPO = Path(__file__).resolve().parent.parent
GOLDEN = REPO / "crates" / "sonara-hook" / "tests" / "golden"
FIXTURES = Path(__file__).resolve().parent / "fixtures"
INGEST = REPO / "src" / "sonara" / "daemon" / "ingest.py"

HINT = "Press the option's number to choose, or Escape to cancel."
ONCE = "Selecting is immediate."


def _payload(case):
    if "fixture" in case:
        return json.loads((FIXTURES / case["fixture"]).read_text(encoding="utf-8"))
    return case["payload"]


def _channel(m):
    return m.get("session") or "default"


def _base(kind, m):
    return {"type": kind, "channel": _channel(m)}


def _hinted(d):
    d["hint"] = HINT
    d["hint_once"] = ONCE
    return d


def _option(o):
    if isinstance(o, dict):
        c = {"label": o.get("label", "")}
        if (o.get("description") or "").strip():
            c["description"] = o["description"]
        return c
    return {"label": str(o)}


def _questions(m):
    asks = []
    for q in m.get("questions") or []:
        d = _base("ask", m)
        d["kind"] = "question"
        if isinstance(q, dict):
            d["text"] = q.get("question", "")
            d["options"] = [_option(o) for o in q.get("options", []) or []]
            if q.get("multiSelect"):
                d["multi_select"] = True
        else:
            d["text"] = str(q)
            d["options"] = []
        asks.append(d)
    if not asks:
        asks.append(dict(_base("ask", m), kind="question", text=""))
    last = _hinted(asks[-1])
    notes = decision_text.choice_notes(m)
    if notes:
        last["notes"] = notes
    return asks


LABELLED = ("stream", "tool", "ask", "answered", "turn_end")


def _folder(cwd):
    return ntpath.basename((cwd or "").rstrip("/"))


def translate(msgs, payload=None):
    """Python hook messages -> protocol v1 messages (module docs)."""
    out = _translate(msgs)
    label = _folder((payload or {}).get("cwd"))
    if label:
        for d in out:
            if d["type"] in LABELLED:
                d["label"] = label
    return out


def _translate(msgs):
    out = []
    for m in msgs:
        t = m["type"]
        if t == MsgType.PROSE:
            out.append(dict(_base("stream", m), delta=m["delta"], index=m["index"],
                            final=m["final"]))
        elif t == MsgType.EARCON:
            if m["kind"] == "turn_done":
                out.append(_base("turn_end", m))
        elif t == MsgType.CHOICE:
            out.extend(_questions(m))
        elif t == MsgType.PLAN:
            out.append(_hinted(dict(_base("ask", m), kind="plan", text=m["text"])))
        elif t == MsgType.PERMISSION:
            text = (m.get("action") or "").strip() or (m.get("message") or "").strip()
            out.append(_hinted(dict(_base("ask", m), kind="permission", text=text)))
        elif t == MsgType.TOOL:
            out.append(dict(_base("tool", m), name=m["tool"] or "", summary=m["summary"]))
        elif t == MsgType.CHOICE_ANSWERED:
            out.append(_base("answered", m))
        elif t == MsgType.SET_FOREGROUND:
            op = _base("channel_open", m)
            folder = _folder(m.get("cwd"))
            if folder:
                op["label"] = folder
            if m.get("host_tab"):
                op["host_tab"] = m["host_tab"]
            out.extend([op, _base("focus", m)])
        elif t == MsgType.FLUSH:
            out.append(_base("turn_start", m))
        elif t == MsgType.SESSION_START:
            pass
        elif t == MsgType.SESSION_END:
            out.append(_base("channel_close", m))
        else:
            raise AssertionError(f"no adaptation for {t}")
    return out


def _cases():
    return sorted(GOLDEN.glob("*.json"))


def test_there_are_golden_cases_for_every_captured_payload():
    used = {json.loads(p.read_text(encoding="utf-8")).get("fixture") for p in _cases()}
    assert {p.name for p in FIXTURES.glob("*.json")} <= used


def test_the_hints_are_the_python_daemons_selection_cue():
    src = INGEST.read_text(encoding="utf-8")
    assert HINT in src and ONCE in src


@pytest.mark.parametrize("path", _cases(), ids=lambda p: p.stem)
def test_golden_messages_are_the_python_mapping(path):
    case = json.loads(path.read_text(encoding="utf-8"))
    payload = _payload(case)
    msgs = handle_event(case["event"], payload, env=case.get("env", {}))
    got = translate(msgs, payload)
    if os.environ.get("SONARA_REGEN_GOLDEN"):
        case["messages"] = got
        path.write_text(json.dumps(case, indent=2) + "\n", encoding="utf-8")
    assert got == case["messages"]
