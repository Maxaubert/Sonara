"""E2E: the real settings.html driven by Playwright against a real
SettingsServer + FakeDaemon. Skipped unless playwright + chromium installed:
    pip install playwright && playwright install chromium
"""

import pytest

pw = pytest.importorskip("playwright.sync_api")

from tests.test_webui import FakeDaemon  # reuse the fake
from sonara import webui


@pytest.fixture()
def live(monkeypatch):
    monkeypatch.setattr(webui, "_installed_voices", lambda: {
        "windows": ["Microsoft Zira"], "kokoro": ["af_heart", "af_bella"]})
    monkeypatch.setattr(webui, "_engine_status", lambda: {"kokoro": True})
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


ALL_ACTIONS = ("nav_start", "flush", "pause",
               "mute", "next_session", "faster", "slower")


def _bind_recorder(monkeypatch):
    binds = []
    monkeypatch.setattr(webui, "_bind_action",
                        lambda a, k, m: binds.append((a, k, tuple(m))))
    # all rows need rendered chips (the live fixture only reports 'mute';
    # an empty .kbd is zero-width and unclickable for Playwright)
    monkeypatch.setattr(webui, "_keymap_state", lambda: [
        {"action": a, "key": None, "mods": []} for a in ALL_ACTIONS])
    return binds


def test_second_capture_cancels_first_and_binds_once(live, monkeypatch):
    # (#38 audit) two armed rows used to bind ONE keypress to BOTH actions
    binds = _bind_recorder(monkeypatch)
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        page.click("button[data-page='hotkeys']")
        page.click("[data-action='nav_start'] .kbd")
        page.click("[data-action='flush'] .kbd")      # must cancel the first
        page.keyboard.press("Control+Alt+z")
        page.wait_for_timeout(400)
        browser.close()
    assert binds == [("flush", "z", ("ctrl", "alt"))]  # exactly ONE bind


def test_hotkey_editor_lists_no_prev_next_rows(live, monkeypatch):
    # D1: paragraph stepping (Ctrl+Alt+Left/Right) was removed; the editor
    # offers exactly the live actions, so a stale nav_prev/nav_next is never shown.
    _bind_recorder(monkeypatch)
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        page.click("button[data-page='hotkeys']")
        rows = page.eval_on_selector_all(
            ".key-row[data-action]", "els => els.map(e => e.dataset.action)")
        browser.close()
    assert sorted(rows) == sorted(ALL_ACTIONS)
    assert "nav_prev" not in rows and "nav_next" not in rows


def test_unsupported_key_shows_error_and_never_persists(live, monkeypatch):
    # (#38 audit critical) junk key names used to reach keymap.json and kill
    # ALL hotkeys; now the page validates against the daemon's keytable
    binds = _bind_recorder(monkeypatch)
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        page.click("button[data-page='hotkeys']")
        page.click("[data-action='mute'] .kbd")
        page.keyboard.press("Control+Alt+F7")          # not in the keytable
        page.wait_for_timeout(400)
        error_shown = page.evaluate(
            "!!document.querySelector('.state.error')")
        browser.close()
    assert binds == []                                 # nothing persisted
    assert error_shown                                 # user was told why


def test_hotkey_capture_works_keyboard_only(live, monkeypatch):
    # (#38 audit) .kbd spans were mouse-only -- unacceptable for an
    # accessibility product; Enter must arm the capture
    binds = _bind_recorder(monkeypatch)
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        page.click("button[data-page='hotkeys']")
        page.evaluate("document.querySelector(\"[data-action='mute'] .kbd\").focus()")
        page.keyboard.press("Enter")                   # arm via keyboard
        page.wait_for_timeout(150)
        page.keyboard.press("Control+Alt+m")
        page.wait_for_timeout(400)
        browser.close()
    assert binds == [("mute", "m", ("ctrl", "alt"))]   # Enter itself not captured


def _voice_options(page):
    return page.eval_on_selector_all(
        "#voice-select option", "els => els.map(e => [e.value, e.selected])")


def test_voice_picker_lists_kokoro_voices_with_the_current_one_selected(live):
    # Kokoro is the only neural engine (#134): the picker lists its voices with
    # no engine toggle, and the configured voice is the selected option.
    d, s = live
    d.config["voice"] = "af_bella"
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        opts = _voice_options(page)
        has_toggle = page.query_selector("#engine-seg") is not None
        rate_visible = page.is_visible("#rate-row")
        browser.close()
    assert opts == [["af_heart", False], ["af_bella", True]]
    assert not has_toggle
    assert rate_visible                       # rate applies to every voice now


def test_voice_picker_change_dispatches_set_voice(live):
    d, s = live
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        page.select_option("#voice-select", "af_bella")
        page.wait_for_timeout(300)
        browser.close()
    assert any(m.get("type") == "set_voice" and m.get("voice") == "af_bella"
               for m in d.messages)


def test_voice_picker_keeps_an_off_list_voice_selected(live):
    # A native Windows voice set from the CLI is not in the Kokoro list: it
    # still shows as the selection, and loading the page never changes it.
    d, s = live
    d.config["voice"] = "Microsoft Zira"
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        page.wait_for_timeout(300)
        opts = _voice_options(page)
        browser.close()
    assert opts == [["af_heart", False], ["af_bella", False],
                    ["Microsoft Zira", True]]
    assert not any(m.get("type") == "set_voice" for m in d.messages)


def _capture(s, combo):
    with pw.sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        page.goto(f"http://127.0.0.1:{s.port}/settings?token=tok123")
        page.wait_for_selector("#voice-select option", state="attached")
        page.click("button[data-page='hotkeys']")
        page.click("[data-action='mute'] .kbd")
        page.keyboard.press(combo)
        page.wait_for_timeout(400)
        error_shown = page.evaluate("!!document.querySelector('.state.error')")
        browser.close()
    return error_shown


def test_modifier_less_hotkey_is_refused_with_a_reason(live, monkeypatch):
    """E13: a bare letter would be swallowed system-wide."""
    binds = _bind_recorder(monkeypatch)
    d, s = live
    assert _capture(s, "m") is True
    assert _capture(s, "Shift+m") is True
    assert binds == []


def test_win_key_is_captured_as_the_win_modifier(live, monkeypatch):
    """E13: metaKey was ignored, so Win+X captured as a bare 'x'."""
    binds = _bind_recorder(monkeypatch)
    d, s = live
    _capture(s, "Meta+Alt+x")
    assert binds == [("mute", "x", ("alt", "win"))]
