"""A Python app that uses Sonara for speech the way a developer would
(#274): only the public API of the installed ``sonara-client`` wheel, and
the ``sonarad.exe`` it ships from the release zip. tests/embed/
test_python_host.py installs the wheel into a fresh venv and runs this file
with that venv's Python.

Environment (from the test; an app sets none of these):
  SONARA_HOME            a temp home
  SONARA_RUNTIME         the sonarad.exe to start (the unpacked release zip)
  SONARA_EMBED_ARGS      JSON list of runtime arguments
  SONARA_EMBED_MODE      "scenario" (the whole API, fake engine) or "voice"
  SONARA_EMBED_PROVIDER  a fake OpenAI-compatible server's /v1 URL

It checks the protocol side itself and prints a JSON report as its last
line; the test checks the WAV files against it. Standard library only.
"""
from __future__ import annotations

import json
import os
import re
import sys
import threading
import time

import sonara_client

ARGS = json.loads(os.environ.get("SONARA_EMBED_ARGS") or "[]")
MODE = os.environ.get("SONARA_EMBED_MODE") or "scenario"
SECRET = "sk-embed-python-0123456789"


def check(cond, what: str) -> None:
    if not cond:
        raise AssertionError("check failed: " + what)


class Tracker:
    """Item phases and state snapshots, read on a thread of their own."""

    def __init__(self, client) -> None:
        self.phases: dict = {}
        self.states: list = []
        self.cond = threading.Condition()
        self.sub = client.subscribe(["state", "items"])
        threading.Thread(target=self._run, daemon=True).start()

    def _run(self) -> None:
        try:
            for e in self.sub:
                with self.cond:
                    if e.get("event") == "item":
                        self.phases.setdefault(e["item_id"], []).append(e["phase"])
                    elif e.get("event") == "state":
                        self.states.append(e)
                    self.cond.notify_all()
        except sonara_client.SonaraError:
            pass

    def until(self, test, what: str, timeout: float = 15.0) -> None:
        deadline = time.monotonic() + timeout
        with self.cond:
            while not test():
                left = deadline - time.monotonic()
                if left <= 0:
                    raise AssertionError("timed out: " + what)
                self.cond.wait(left)

    def last(self) -> dict:
        return self.states[-1]

    def started(self, item: int) -> None:
        self.until(lambda: "started" in self.phases.get(item, []), "item %d started" % item)

    def end(self, item: int, timeout: float = 15.0) -> str:
        def ended():
            return next((p for p in self.phases.get(item, []) if p != "started"), None)
        self.until(lambda: ended() is not None, "item %d ended" % item, timeout)
        return ended()


def scenario() -> dict:
    sonara = sonara_client.connect("embed-python", runtime_args=ARGS, extensions=["channels"])
    t = Tracker(sonara)
    check(sonara.info["protocol"]["major"] == 1, "protocol 1")
    for cap in ("speak", "control", "set", "get", "voices", "subscribe", "engines"):
        check(cap in sonara.info["capabilities"], "capability " + cap)
    check("channels" in sonara.info["extensions"], "channels enabled")
    # A second app discovers the running instance (runtime.json) instead of
    # starting another.
    with sonara_client.connect("embed-python-other", runtime_args=ARGS) as other:
        check(other.runtime["pid"] == sonara.runtime["pid"], "shared instance")
    t.until(lambda: t.states, "first state")
    texts = {
        "first": "First sentence here.",
        "second": "Second one.",
        "long": "This long item keeps the reader busy. It has three sentences. The third one ends it.",
        "queued": "Queued and then replaced.",
        "replacement": "The replacement.",
        "cut": "This item is cut short by another one. It never gets to the end of its text.",
        "cut_in": "Cut in.",
        "paused": "This item is paused and resumed. Then it goes on to its end.",
        "skipped": "This item is skipped. Its second sentence is never heard.",
        "after_skip": "After the skip.",
        "stopped": "This item is stopped. Nothing after it is read.",
        "never_read": "Never read.",
        "silent": "Silent words.",
        "fast": "Hello world.",
        "provider": "From the provider.",
    }
    items: dict = {}

    check(sonara.set("rate", 200) == 200, "set rate")
    check(sonara.get("rate") == 200, "get rate")
    check(sonara.set("voice", "tone") == "tone", "set voice")
    ids = {v["id"] for v in sonara.voices()}
    check({"tone", "silence"} <= ids, "fake voices listed")

    items["first"] = sonara.speak(texts["first"], label="first")
    items["second"] = sonara.speak(texts["second"])
    check(t.end(items["first"]) == "finished", "first finished")
    check(t.end(items["second"]) == "finished", "second finished")
    check(any(s["now_playing"] and s["now_playing"].get("label") == "first" for s in t.states),
          "now_playing.label")

    items["long"] = sonara.speak(texts["long"])
    t.started(items["long"])
    items["queued"] = sonara.speak(texts["queued"])
    items["replacement"] = sonara.speak(texts["replacement"], mode="replace")
    check(t.end(items["queued"]) == "skipped", "queued skipped by replace")
    check(t.end(items["long"]) == "finished", "the current item survives replace")
    check(t.end(items["replacement"]) == "finished", "replacement finished")

    items["cut"] = sonara.speak(texts["cut"])
    t.started(items["cut"])
    items["cut_in"] = sonara.speak(texts["cut_in"], interrupt=True)
    check(t.end(items["cut"]) == "skipped", "interrupted item skipped")
    check(t.end(items["cut_in"]) == "finished", "cut_in finished")

    items["paused"] = sonara.speak(texts["paused"])
    t.started(items["paused"])
    sonara.control("pause")
    t.until(lambda: t.last()["paused"] is True, "paused state")
    time.sleep(0.8)
    check("finished" not in t.phases[items["paused"]], "a paused item does not finish")
    sonara.control("play")
    t.until(lambda: t.last()["paused"] is False, "resumed state")
    check(t.end(items["paused"]) == "finished", "paused item finished after resume")

    items["skipped"] = sonara.speak(texts["skipped"])
    items["after_skip"] = sonara.speak(texts["after_skip"])
    t.started(items["skipped"])
    sonara.control("skip")
    check(t.end(items["skipped"]) == "skipped", "skip")
    check(t.end(items["after_skip"]) == "finished", "the next item after skip")

    items["stopped"] = sonara.speak(texts["stopped"])
    items["never_read"] = sonara.speak(texts["never_read"])
    t.started(items["stopped"])
    sonara.control("stop")
    check(t.end(items["stopped"]) == "skipped", "stop cuts the current item")
    check(t.end(items["never_read"]) == "skipped", "stop clears the queue")
    t.until(lambda: t.last()["now_playing"] is None and t.last()["queued"] == 0, "idle after stop")

    sonara.set("voice", "silence")
    check(sonara.get("voice") == "silence", "get voice")
    items["silent"] = sonara.speak(texts["silent"])
    check(t.end(items["silent"]) == "finished", "silent finished")
    sonara.set("voice", "tone")
    sonara.set("rate", 400)
    items["fast"] = sonara.speak(texts["fast"])
    check(t.end(items["fast"]) == "finished", "fast finished")
    sonara.set("rate", 200)

    sonara.control("mute")
    t.until(lambda: t.last()["muted"] is True, "muted")
    sonara.control("unmute")
    t.until(lambda: t.last()["muted"] is False, "unmuted")
    check(sonara.set("volume", 80) == 80, "volume")
    t.until(lambda: t.last()["volume"] == 80, "volume in state")

    opened = sonara.channels.open("tab-build", label="Build")
    check(opened["created"] is True, "channel created")
    sonara.channels.open("tab-tests", label="Tests", policy="queue")
    sonara.channels.speak("tab-build", "Build done.")
    sonara.channels.speak("tab-tests", "Tests passed.")
    channel_items: dict = {}

    def seen():
        for s in t.states:
            n = s["now_playing"]
            if n and n["text"] == "Build done." and n.get("channel") == "tab-build":
                channel_items["channel_build"] = n["item_id"]
            if n and n["text"] == "Tests passed." and n.get("channel") == "tab-tests":
                channel_items["channel_tests"] = n["item_id"]
        return len(channel_items) == 2

    t.until(seen, "both channels read")
    t.end(channel_items["channel_tests"])
    check(any(s["now_playing"] and s["now_playing"].get("channel") == "tab-tests"
              and re.match(r"^Tests\.?$", s["now_playing"]["text"]) for s in t.states),
          "the hand-off to another channel is announced with its label")
    sonara.channels.next_channel()
    t.until(lambda: t.last()["now_playing"] is not None, "next_channel reads")
    sonara.control("stop")
    sonara.channels.close("tab-build")
    sonara.channels.close("tab-tests")
    t.until(lambda: t.last()["now_playing"] is None, "idle after channels")

    kinds = sonara.engines.list()["kinds"]
    for k in ("openai-compatible", "elevenlabs", "azure", "google", "gemini", "cartesia", "deepgram", "command"):
        check(k in kinds, "kind " + k)
    profile = {
        "id": "embed-provider",
        "kind": "openai-compatible",
        "url": os.environ["SONARA_EMBED_PROVIDER"],
        "voice": "af_heart",
        "options": {"preset": "kokoro-fastapi", "timeout_ms": 5000},
    }
    added = sonara.engines.add(profile, secret=SECRET)
    check(added["engine"]["key_present"] is True and SECRET not in json.dumps(added), "engine added, key hidden")
    tested = sonara.engines.test("embed-provider", play=False)
    check(tested["sample_rate"] == 24000 and tested["duration_ms"] > 0, "engine_test %r" % tested)
    sonara.engines.test("embed-provider", text="A preview.")
    pv = sonara.voices("embed-provider", refresh=True)
    check(any(v["id"] == "af_heart" and v["license_class"] == "external" for v in pv), "provider voices")
    check(sonara.set("engine", "embed-provider") == "embed-provider", "set engine")
    items["provider"] = sonara.speak(texts["provider"])
    check(t.end(items["provider"]) == "finished", "provider item finished")
    st = t.last()["engine_status"]
    check(st["engine"] == "embed-provider" and st["ready"] is True, "engine_status %r" % st)
    sonara.set("engine", "fake")
    check(sonara.engines.remove("embed-provider")["removed"] == "embed-provider", "engine removed")

    version = sonara.info["version"]
    sonara.close()
    return {"host": "python", "version": version, "items": items, "texts": texts,
            "channel_items": channel_items, "secret": SECRET}


def voice() -> dict:
    sonara = sonara_client.connect("embed-python-voice", runtime_args=ARGS)
    t = Tracker(sonara)
    t.until(lambda: t.states, "first state")

    def settled():
        st = t.last().get("engine_status") or {}
        return st.get("ready") is True or st.get("status") == "unavailable"

    t.until(settled, "the engine is ready", 180)
    status = t.last()["engine_status"]
    if not status["ready"]:
        sonara.close()
        return {"host": "python", "unavailable": True, "engine_status": status}
    item = sonara.speak(os.environ["SONARA_EMBED_SENTENCE"])
    phase = t.end(item, 60)
    # The status once the item finished: the same engine, still ready, no
    # fallback, so the item was this engine's voice.
    after = t.last().get("engine_status")
    sonara.close()
    return {"host": "python", "version": sonara.info["version"], "item": item, "phase": phase,
            "engine_status": status, "engine_status_after": after}


if __name__ == "__main__":
    report = voice() if MODE == "voice" else scenario()
    print(json.dumps(report))
    sys.exit(0)
