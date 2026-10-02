"""E2E: the runtime's settings page (crates/sonarad/assets/settings.html,
#201) driven by Playwright against a real sonarad (``--engine fake
--system fake``, temporary home), so every click goes through protocol v1
and lands in ``config.json``.

Skipped unless playwright + chromium are installed and sonarad is built
(``cargo build -p sonarad``)."""
from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

pw = pytest.importorskip("playwright.sync_api")

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "conformance"))
import harness  # noqa: E402

EXE = harness.find_sonarad()
pytestmark = pytest.mark.skipif(EXE is None, reason="sonarad.exe not built (cargo build -p sonarad)")


class Live:
    """One runtime plus a TCP client that enabled the extensions (as the
    Claude plugin would), and its settings_url."""

    def __init__(self, home: Path, extensions=("agent", "system")):
        self.home = home
        self.rt = harness.Runtime(EXE, home)
        self.client = self.rt.tcp(extensions=list(extensions))
        r = self.client.request({"type": "get", "key": "settings_url"})
        assert r["ok"], r
        self.url = r["value"]

    def get(self, key):
        r = self.client.request({"type": "get", "key": key})
        assert r["ok"], r
        return r["value"]

    def saved(self) -> dict:
        p = self.home / "config.json"
        return json.loads(p.read_text(encoding="utf-8")) if p.exists() else {}

    def close(self):
        self.client.close()
        self.rt.close()


@pytest.fixture()
def live(tmp_path):
    started = []

    def _start(**kw):
        lv = Live(tmp_path / "home", **kw)
        started.append(lv)
        return lv

    yield _start
    for lv in started:
        lv.close()


@pytest.fixture(scope="module")
def browser():
    with pw.sync_playwright() as p:
        b = p.chromium.launch()
        yield b
        b.close()


def open_page(browser, url):
    page = browser.new_page()
    page.goto(url)
    # The page runs under its own CSP (no eval), so waits use locators.
    pw.expect(page.locator("#rt-version")).not_to_have_text("–")
    return page


def eventually(fn, timeout=8.0):
    return harness.wait_until(fn, timeout)


def test_every_section_of_the_old_page_is_there(live, browser):
    lv = live()
    page = open_page(browser, lv.url)
    names = page.locator(".side-nav button").all_text_contents()
    assert [n.strip() for n in names] == ["Speech", "Summary", "Audio", "Sessions", "Hotkeys",
                                          "Advanced", "System"]
    page.click("[data-page=system]")
    assert page.locator("#app-version").text_content().startswith("Version ")
    assert page.locator("#rt-pid").text_content().strip() == str(lv.rt.proc.pid)
    assert "config.json" in page.locator("#rt-config").text_content()
    page.close()


def test_rate_change_is_saved_and_survives_a_restart(live, browser):
    lv = live()
    page = open_page(browser, lv.url)
    page.locator("#rate").fill("275")
    page.locator("#rate").dispatch_event("change")
    assert eventually(lambda: lv.saved().get("rate") == 275)
    assert lv.get("rate") == 275
    page.close()
    lv.close()
    lv = live()
    page = open_page(browser, lv.url)
    pw.expect(page.locator("#rate-out")).to_have_text("275 wpm")
    assert page.locator("#rate").input_value() == "275"
    page.close()


def test_background_sessions_choice_is_saved(live, browser):
    # #195: the background speech policy (Python background_policy).
    lv = live()
    page = open_page(browser, lv.url)
    seg = "#background-seg [data-value=%s]"
    pw.expect(page.locator(seg % "all")).to_have_attribute("aria-checked", "true")  # #202 default
    page.click(seg % "earcon_only")
    pw.expect(page.locator(seg % "earcon_only")).to_have_attribute("aria-checked", "true")
    assert eventually(lambda: lv.saved().get("background_policy") == "earcon_only")
    assert lv.get("background_policy") == "earcon_only"
    page.close()


def test_summary_mode_style_and_prompt(live, browser):
    lv = live()
    page = open_page(browser, lv.url)
    page.click("[data-page=summary]")
    page.click("#summary-seg [data-value=brief]")
    assert eventually(lambda: lv.get("summaries")["enabled"] is True)
    assert lv.get("summaries")["style"] == "brief"
    pw.expect(page.locator("#summary-seg [data-value=brief]")).to_have_attribute("aria-checked", "true")
    # The editor shows the built-in brief instruction until it is changed.
    builtin = lv.get("summaries")["default_prompts"]["brief"]
    pw.expect(page.locator("#prompt-text")).to_have_value(builtin)
    page.locator("#prompt-text").fill("Say it in one line.")
    page.locator("#prompt-text").blur()
    assert eventually(lambda: lv.get("summaries")["prompt"] == "Say it in one line.")
    assert eventually(lambda: lv.saved().get("summaries", {}).get("prompts") == {"brief": "Say it in one line."})
    page.click("#prompt-reset")
    assert eventually(lambda: lv.get("summaries")["prompts"] == {})
    # Model picks its command too.
    page.select_option("#model-select", "codex|gpt-5.4-mini")
    assert eventually(lambda: lv.get("summaries")["command"] == "codex")
    assert lv.get("summaries")["model"] == "gpt-5.4-mini"
    page.click("#summary-seg [data-value=off]")
    assert eventually(lambda: lv.get("summaries")["enabled"] is False)
    page.close()


def test_segments_are_a_keyboard_radio_group_with_a_live_status(live, browser):
    lv = live()
    page = open_page(browser, lv.url)
    page.click("[data-page=audio]")
    off = page.locator("#audio-seg [data-value=off]")
    off.focus()
    page.keyboard.press("ArrowRight")
    assert eventually(lambda: lv.get("audio_mode") == "duck")
    pw.expect(page.locator("#audio-seg [data-value=duck]")).to_be_focused()
    pw.expect(page.locator("#live")).to_contain_text("saved")
    assert lv.saved()["audio_mode"] == "duck"
    # Duck level is enabled only in duck mode.
    assert page.locator("#duck").is_enabled()
    page.keyboard.press("ArrowRight")
    assert eventually(lambda: lv.get("audio_mode") == "pause")
    pw.expect(page.locator("#duck")).to_be_disabled()
    page.close()


def test_sessions_name_audio_and_voice(live, browser):
    lv = live()
    lv.client.request({"type": "channel_open", "channel": "sess-1234abcd", "label": "repo"})
    page = open_page(browser, lv.url)
    page.click("[data-page=sessions]")
    row = page.locator("#session-rows .sess-row").first
    row.wait_for()
    name = row.locator("input")
    assert name.get_attribute("placeholder") == "repo"
    name.fill("Build")
    name.press("Enter")
    assert eventually(lambda: any(r["label"] == "Build" for r in lv.get("channel_prefs")))
    row = page.locator("#session-rows .sess-row").first
    row.locator("[role=switch]").click()
    assert eventually(lambda: lv.get("channel_prefs")[0]["muted"] is True)
    page.locator("#session-rows .sess-row select").first.select_option("silence")
    assert eventually(lambda: lv.get("channel_prefs")[0]["voice"] == "silence")
    prefs = json.loads((lv.home / "session_prefs.json").read_text(encoding="utf-8"))
    assert prefs["sess-1234abcd"]["label"] == "Build"
    page.close()


def test_voice_preview_goes_through_the_runtime(live, browser):
    lv = live()
    page = open_page(browser, lv.url)
    page.wait_for_selector("#voice-select option[value=tone]", state="attached")
    page.select_option("#voice-select", "tone")
    assert eventually(lambda: lv.get("voice") == "tone")
    with page.expect_response(lambda r: r.url.endswith("/v1/preview")) as resp:
        page.click("#voice-preview")
    assert resp.value.status == 200
    assert resp.value.json()["voice"] == "tone"
    page.close()


def test_hotkey_capture_with_the_keyboard(live, browser):
    lv = live()
    page = open_page(browser, lv.url)
    page.click("[data-page=hotkeys]")
    kbd = page.locator("[data-action=pause] .kbd")
    kbd.focus()
    page.keyboard.press("Enter")
    page.wait_for_selector("[data-action=pause].listen")
    page.keyboard.press("Control+Alt+KeyS")

    def bound():
        b = {x["action"]: x for x in lv.get("hotkeys")["bindings"]}
        return b["pause"]["combo"] == "Ctrl+Alt+S"
    assert eventually(bound)
    page.close()


def test_agent_sections_explain_themselves_without_an_agent(live, browser):
    lv = live(extensions=("system",))
    page = open_page(browser, lv.url)
    page.click("[data-page=summary]")
    assert page.locator("#summary .notice.agent-off").is_visible()
    assert page.locator("#summary-seg [data-value=brief]").is_disabled()
    page.click("[data-page=sessions]")
    assert page.locator("#sessions-off").is_visible()
    page.close()


def test_offline_banner_when_the_runtime_exits(live, browser):
    lv = live()
    page = open_page(browser, lv.url)
    lv.rt.close()
    page.wait_for_selector("#offline-banner", state="visible", timeout=8000)
    page.close()


def test_a_saved_voice_the_engine_lacks_is_shown(live, browser, tmp_path):
    """A migrated voice that cannot apply yet (a Kokoro voice under another
    engine) is named on the page, so the user sees it was kept."""
    home = tmp_path / "home"
    home.mkdir()
    (home / "config.json").write_text(json.dumps({"voice": "af_sarah"}), encoding="utf-8")
    lv = live()
    page = open_page(browser, lv.url)
    note = page.locator("#voice-saved")
    pw.expect(note).to_be_visible()
    pw.expect(note).to_contain_text("af_sarah")
    page.close()


def test_no_saved_voice_note_when_the_voice_applies(live, browser):
    lv = live()
    page = open_page(browser, lv.url)
    pw.expect(page.locator("#voice-saved")).to_be_hidden()
    page.close()
