"""End to end (#202): a release zip built by packaging/release_zip.py from
the built executables, served as the GitHub release, installed by the first
hook into a temporary LOCALAPPDATA, and then Claude Code hook payloads
through bin/sonara-hook-launch become speech in the runtime it started
(fake engine, fake system)."""
from __future__ import annotations

import json

import harness
import plugin_harness as ph

SESSION = "11111111-2222-3333-4444-555555555555"


def captured(name: str) -> dict:
    """A payload captured from Claude Code (tests/fixtures)."""
    return json.loads((ph.REPO / "tests" / "fixtures" / name).read_text(encoding="utf-8"))


def test_the_first_hook_installs_the_release_and_later_hooks_speak(box, releases, exes, tmp_path):
    data, sums = ph.build_release(exes, tmp_path / "build")
    r = releases({ph.ZIP_NAME: data, "SHA256SUMS": sums})
    box.env["SONARA_RELEASE_BASE_URL"] = r.url

    code, took = box.launch("SessionStart", {"session_id": SESSION, "cwd": r"C:\work\proj"})
    assert code == 0 and took < 1.5
    # The bootstrap installs the runtime and starts it (sonara.exe start).
    box.wait_for(lambda: box.runtime_info() is not None, what="the runtime to start")
    box.wait_bootstrap()
    for f in (*ph.EXES, "LICENSE", "THIRD_PARTY_NOTICES.md"):
        assert (box.dest / f).is_file(), f
    info = box.runtime_info()
    assert info["version"] == ph.VERSION

    rt = harness.Runtime.attach(box.dest / "sonarad.exe", box.home, info)
    # The product defaults (#202) as they ship: prose waits for five chunks
    # or the end of the turn.
    listener = rt.tcp(extensions=["agent"])
    assert listener.request({"type": "subscribe", "events": ["state", "earcons"]})["ok"]
    listener.state()

    assert box.launch("UserPromptSubmit", {"session_id": SESSION, "cwd": r"C:\work\proj",
                                           "prompt": "Hi"})[0] == 0
    assert box.launch("MessageDisplay", captured("MessageDisplay.json"))[0] == 0
    assert box.launch("Stop", {"session_id": SESSION})[0] == 0
    assert listener.next_event(lambda e: e.get("event") == "earcon", 30.0)["kind"] == "turn_done"
    s = listener.next_event(
        lambda e: e.get("event") == "state" and e["now_playing"] is not None
        and e["now_playing"]["text"] == "Here is the first sentence.",
        30.0,
    )
    assert s["now_playing"]["channel"] == SESSION
    assert s["rate"] == 250
    listener.close()
    assert r.hits == {"SHA256SUMS": 1, ph.ZIP_NAME: 1}, "installed once"
