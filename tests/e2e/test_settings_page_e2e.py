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
