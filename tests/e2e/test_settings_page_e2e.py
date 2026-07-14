"""E2E: the real settings.html driven by Playwright against a real
SettingsServer + FakeDaemon. Skipped unless playwright + chromium installed:
    pip install playwright && playwright install chromium
"""
import json

import pytest

pw = pytest.importorskip("playwright.sync_api")

from tests.test_webui import FakeDaemon  # reuse the fake
from sonara import webui


@pytest.fixture()
def live(monkeypatch):
    monkeypatch.setattr(webui, "_installed_voices", lambda: {
        "windows": ["Microsoft Zira"], "kokoro": ["af_heart", "af_bella"],
        "chatterbox": []})
    monkeypatch.setattr(webui, "_engine_status", lambda: {"kokoro": True, "chatterbox": False})
    monkeypatch.setattr(webui, "_keymap_state", lambda: [
        {"action": "mute", "key": "m", "mods": ["ctrl", "alt"]}])
    d = FakeDaemon()
    s = webui.SettingsServer(d, token="tok123", port=0)
    s.start()
    yield d, s
    s.stop()


def test_rate_change_dispatches_set_rate(live):
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#rate")
        page.locator("#rate").fill("300")
        page.locator("#rate").dispatch_event("change")
        page.wait_for_timeout(300)
        browser.close()
    assert any(m.get("type") == "set_rate" and m.get("rate") == 300
               for m in d.messages)


def test_offline_banner_appears_when_server_dies(live):
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#rate")
        s.stop()
        page.wait_for_selector("#offline-banner", state="visible", timeout=8000)
        browser.close()


def _tiny_wav() -> bytes:
    import io
    import wave
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(8000)
        w.writeframes(b"\x00\x00" * 400)         # 50ms of silence
    return buf.getvalue()


def test_open_voice_dropdown_survives_the_poll(live):
    # (#38) the 3s state poll used to rebuild the select every cycle, closing
    # the open dropdown. With unchanged state the option NODES must persist.
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        page.evaluate("window.__opt = document.querySelector('#voice-select option');"
                      "window.__opt.__mark = 42")
        page.wait_for_timeout(7000)               # two poll cycles
        survived = page.evaluate(
            "document.querySelector('#voice-select option').__mark === 42")
        browser.close()
    assert survived                                # same DOM node, no rebuild


def test_preview_plays_prerendered_file_without_live_synth(live, tmp_path, monkeypatch):
    # (#38) preview = instant file playback; the live-synth POST fallback must
    # NOT fire when the file exists.
    from sonara import previews
    monkeypatch.setattr(previews, "preview_dir", lambda: tmp_path)
    (tmp_path / "af_heart.wav").write_bytes(_tiny_wav())
    d, s = live
    d.preview_voice = lambda v: (_ for _ in ()).throw(
        AssertionError("live synth fallback must not fire"))
    statuses = []
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.on("response", lambda r: statuses.append((r.url, r.status)))
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        page.select_option("#voice-select", "af_heart")
        page.click("#voice-preview")
        page.wait_for_timeout(1500)
        browser.close()
    audio_hits = [st for (u, st) in statuses if "/api/preview-audio" in u]
    assert audio_hits and audio_hits[0] == 200     # file streamed
