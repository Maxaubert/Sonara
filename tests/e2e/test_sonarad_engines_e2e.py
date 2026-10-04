"""E2E: the Engines section of the runtime's settings page (#227, spec
2026-10-04-external-engines-spec.md 11.3), driven by Playwright against a
real sonarad (``--engine fake --system fake --keys fake``, temporary home)
and a fake OpenAI-compatible speech server on loopback. No real provider
is ever called.

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
sys.path.insert(0, str(REPO / "conformance" / "engines"))
import harness  # noqa: E402
from fake_openai import FakeOpenAI  # noqa: E402

EXE = harness.find_sonarad()
pytestmark = pytest.mark.skipif(EXE is None, reason="sonarad.exe not built (cargo build -p sonarad)")

SECRET = "sk-e2e-engines-secret-0123456789abcdef"


class Live:
    def __init__(self, home: Path, *args: str):
        self.home = home
        self.rt = harness.Runtime(EXE, home, *args)
        self.client = self.rt.tcp(extensions=["agent", "system"])
        r = self.client.request({"type": "get", "key": "settings_url"})
        assert r["ok"], r
        self.url = r["value"]

    def request(self, msg):
        r = self.client.request(msg)
        assert r["ok"], (msg, r)
        return r

    def get(self, key):
        return self.request({"type": "get", "key": key})["value"]

    def close(self):
        self.client.close()
        self.rt.close()


@pytest.fixture()
def live(tmp_path):
    started = []

    def _start(*args):
        lv = Live(tmp_path / "home", *args)
        started.append(lv)
        return lv

    yield _start
    for lv in started:
        lv.close()


@pytest.fixture()
def provider():
    p = FakeOpenAI()
    yield p
    p.stop()


@pytest.fixture(scope="module")
def browser():
    with pw.sync_playwright() as p:
        b = p.chromium.launch()
        yield b
        b.close()


def open_engines(browser, url):
    page = browser.new_page()
    replies = []
    page.on("response", lambda r: replies.append(r) if "/v1/" in r.url else None)
    page.goto(url)
    pw.expect(page.locator("#rt-version")).not_to_have_text("–")
    page.click("[data-page=engines]")
    pw.expect(page.locator("#h-engines")).to_be_visible()
    return page, replies


def eventually(fn, timeout=8.0):
    return harness.wait_until(fn, timeout)


def add_through_the_form(page, provider, key=SECRET):
    page.click("#engine-new")
    form = page.locator("#engine-form")
    pw.expect(form).to_be_visible()
    page.select_option("#ef-kind", "openai-compatible")
    page.select_option("#ef-preset", "kokoro-fastapi")
    page.fill("#ef-id", "local")
    page.fill("#ef-label", "Test server")
    page.fill("#ef-url", provider.url)
    page.select_option("#ef-keyref", "credman")
    page.fill("#ef-key", key)
    page.click("#ef-save")
    pw.expect(page.locator("#profile-rows [data-engine=local]")).to_be_visible()


def test_add_a_profile_with_a_key_the_key_never_comes_back(live, browser, provider):
    lv = live()
    page, replies = open_engines(browser, lv.url)
    pw.expect(page.locator("#engines-empty")).to_be_visible()
    add_through_the_form(page, provider)
    row = page.locator("#profile-rows [data-engine=local]")
    pw.expect(row).to_be_visible()
    pw.expect(row).to_contain_text("Test server")
    pw.expect(row).to_contain_text("Key saved")
    # Loopback: the text stays on this PC.
    pw.expect(row).to_contain_text("Runs on this PC")
    # The key field empties after the save and is never refilled.
    pw.expect(page.locator("#ef-key")).to_have_value("")
    engines = lv.request({"type": "engine_list"})["engines"]
    assert [e["id"] for e in engines] == ["local"]
    assert engines[0]["key_present"] is True
    # The key is nowhere in the page, its replies, or a home file but the fake key store.
    assert SECRET not in page.content()
    for r in replies:
        try:
            assert SECRET not in r.text()
        except pw.Error:
            pass  # a reply without a body
    for f in lv.home.rglob("*"):
        if f.is_file() and f.name != "fake-keys.json":
            assert SECRET.encode() not in f.read_bytes(), f
    page.close()


def test_the_voice_select_fills_from_the_server(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    add_through_the_form(page, provider)
    # After the profile exists, the form edits it and lists the server's voices.
    page.wait_for_selector("#ef-voice option[value=am_echo]", state="attached")
    assert any(r["path"].startswith("/v1/audio/voices") for r in provider.requests)
    page.select_option("#ef-voice", "am_echo")
    page.click("#ef-save")
    assert eventually(lambda: lv.request({"type": "engine_list"})["engines"][0]["voice"] == "am_echo")
    # The saved key stays when the key field is left empty.
    assert lv.request({"type": "engine_list"})["engines"][0]["key_present"] is True
    # An unlisted id goes through "Other voice id".
    page.select_option("#ef-voice", "__other")
    page.fill("#ef-voice-other", "my_cloned_voice")
    page.click("#ef-save")
    assert eventually(lambda: lv.request({"type": "engine_list"})["engines"][0]["voice"] == "my_cloned_voice")
    page.close()


def test_a_cloud_profile_says_where_text_goes(live, browser):
    lv = live()
    lv.request({"type": "engine_add", "engine": {
        "id": "openai", "kind": "openai-compatible", "options": {"preset": "openai"}}})
    page, _ = open_engines(browser, lv.url)
    row = page.locator("#profile-rows [data-engine=openai]")
    pw.expect(row).to_contain_text("Sends text to api.openai.com")
    pw.expect(row).to_contain_text("No key")
    page.close()


def test_test_speaks_and_shows_the_time(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    add_through_the_form(page, provider)
    row = page.locator("#profile-rows [data-engine=local]")
    before = len(provider.speech())
    with page.expect_response(lambda r: r.url.endswith("/v1/engine_test")) as resp:
        row.locator("button.engine-test").click()
    assert resp.value.json()["ok"] is True
    assert len(provider.speech()) == before + 1
    pw.expect(row.locator(".engine-result")).to_contain_text("ms")
    page.close()


def test_an_auth_failure_shows_its_reason(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    add_through_the_form(page, provider)
    provider.fail(401, {"message": "Incorrect API key provided", "type": "invalid_request_error",
                        "code": "invalid_api_key"})
    row = page.locator("#profile-rows [data-engine=local]")
    row.locator("button.engine-test").click()
    result = row.locator(".engine-result")
    pw.expect(result).to_contain_text("refused the key")
    pw.expect(result).to_contain_text("Incorrect API key")
    page.close()


def test_use_switches_the_engine_and_the_speech_page_names_it(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    add_through_the_form(page, provider)
    row = page.locator("#profile-rows [data-engine=local]")
    row.locator("button.engine-use").click()
    assert eventually(lambda: lv.get("engine") == "local")
    pw.expect(row).to_contain_text("In use")
    pw.expect(row.locator("button.engine-use")).to_be_hidden()
    page.click("[data-page=speech]")
    pw.expect(page.locator("#engine-status")).to_contain_text("Test server")
    page.close()


def test_remove_asks_first_and_switches_away_from_the_current_engine(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    add_through_the_form(page, provider)
    lv.request({"type": "set", "key": "engine", "value": "local"})
    row = page.locator("#profile-rows [data-engine=local]")
    # Declining keeps it.
    page.once("dialog", lambda d: d.dismiss())
    row.locator("button.engine-remove").click()
    page.wait_for_timeout(300)
    assert [e["id"] for e in lv.request({"type": "engine_list"})["engines"]] == ["local"]
    page.once("dialog", lambda d: d.accept())
    row.locator("button.engine-remove").click()
    assert eventually(lambda: lv.request({"type": "engine_list"})["engines"] == [])
    assert lv.get("engine") == "fake"
    pw.expect(page.locator("#engines-empty")).to_be_visible()
    keys = json.loads((lv.home / "fake-keys.json").read_text(encoding="utf-8")) \
        if (lv.home / "fake-keys.json").exists() else {}
    assert "local" not in json.dumps(keys)
    page.close()


def test_edit_keeps_the_key_and_never_prefills_it(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    add_through_the_form(page, provider)
    page.click("#ef-cancel")
    pw.expect(page.locator("#engine-form")).to_be_hidden()
    page.locator("#profile-rows [data-engine=local] button.engine-edit").click()
    pw.expect(page.locator("#engine-form")).to_be_visible()
    pw.expect(page.locator("#ef-id")).to_have_value("local")
    pw.expect(page.locator("#ef-id")).to_be_disabled()
    pw.expect(page.locator("#ef-key")).to_have_value("")
    pw.expect(page.locator("#ef-key-hint")).to_contain_text("Leave empty to keep the saved key")
    page.fill("#ef-label", "Renamed")
    page.click("#ef-save")
    assert eventually(lambda: lv.request({"type": "engine_list"})["engines"][0]["label"] == "Renamed")
    assert lv.request({"type": "engine_list"})["engines"][0]["key_present"] is True
    page.close()


def test_a_validation_error_is_shown_on_the_form(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.click("#engine-new")
    page.select_option("#ef-kind", "openai-compatible")
    page.fill("#ef-id", "Bad Id!")
    page.fill("#ef-url", provider.url)
    page.click("#ef-save")
    pw.expect(page.locator("#ef-error")).to_contain_text("invalid engine id")
    assert lv.request({"type": "engine_list"})["engines"] == []
    page.close()


def test_the_whole_path_works_with_the_keyboard(live, browser, provider):
    lv = live()
    page = browser.new_page()
    page.goto(lv.url)
    pw.expect(page.locator("#rt-version")).not_to_have_text("–")
    page.locator("[data-page=engines]").focus()
    page.keyboard.press("Enter")
    page.locator("#engine-new").focus()
    page.keyboard.press("Enter")
    pw.expect(page.locator("#ef-kind")).to_be_focused()
    page.select_option("#ef-kind", "openai-compatible")
    page.select_option("#ef-preset", "kokoro-fastapi")
    page.locator("#ef-id").focus()
    page.keyboard.type("kbd")
    page.locator("#ef-url").focus()
    page.keyboard.type(provider.url)
    page.locator("#ef-url").press("Enter")   # Enter submits the form
    assert eventually(lambda: [e["id"] for e in lv.request({"type": "engine_list"})["engines"]] == ["kbd"])
    row = page.locator("#profile-rows [data-engine=kbd]")
    row.locator("button.engine-use").focus()
    page.keyboard.press("Enter")
    assert eventually(lambda: lv.get("engine") == "kbd")
    page.close()


def test_the_section_is_hidden_when_the_runtime_refuses_external_engines(live, browser):
    lv = live("--no-external-engines")
    page = browser.new_page()
    page.goto(lv.url)
    pw.expect(page.locator("#rt-version")).not_to_have_text("–")
    pw.expect(page.locator("[data-page=engines]")).to_be_hidden()
    page.close()


def stored(lv, engine_id):
    data = json.loads((lv.home / "engines.json").read_text(encoding="utf-8"))
    return next(e for e in data["engines"] if e["id"] == engine_id)


def test_a_new_cloud_profile_does_not_keep_the_openai_address(live, browser):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.click("#engine-new")
    # The form opens on OpenAI with its address filled in; another kind drops it.
    pw.expect(page.locator("#ef-url")).to_have_value("https://api.openai.com/v1")
    page.select_option("#ef-kind", "elevenlabs")
    pw.expect(page.locator("#ef-url")).to_have_value("")
    page.fill("#ef-id", "eleven")
    # A voice id is required now, so the form asks for one directly.
    pw.expect(page.locator("#ef-voice")).to_have_value("__other")
    pw.expect(page.locator("#ef-voice-other")).to_be_visible()
    pw.expect(page.locator("#ef-voice-hint")).to_contain_text("required")
    page.fill("#ef-voice-other", "21m00Tcm4TlvDq8ikWAM")
    page.fill("#ef-key", SECRET)
    page.click("#ef-save")
    pw.expect(page.locator("#profile-rows [data-engine=eleven]")).to_be_visible()
    view = lv.request({"type": "engine_list"})["engines"][0]
    assert "url" not in view["explicit"], view
    assert view["sends_text_to"] == "api.elevenlabs.io"
    assert "url" not in stored(lv, "eleven")
    page.close()


def test_an_azure_region_edit_moves_where_text_goes(live, browser):
    lv = live()
    lv.request({"type": "engine_add", "engine": {
        "id": "az", "kind": "azure", "voice": "en-US-AvaMultilingualNeural",
        "options": {"region": "westeurope"}}})
    page, _ = open_engines(browser, lv.url)
    page.locator("#profile-rows [data-engine=az] button.engine-edit").click()
    pw.expect(page.locator("#ef-url")).to_have_value("")
    page.fill("#ef-opt-region", "eastus")
    page.click("#ef-save")
    assert eventually(lambda: lv.request({"type": "engine_list"})["engines"][0]["sends_text_to"]
                      == "eastus.tts.speech.microsoft.com")
    assert "url" not in stored(lv, "az")
    page.close()


def test_renaming_a_preset_profile_keeps_the_preset_defaults(live, browser):
    lv = live()
    lv.request({"type": "engine_add", "engine": {
        "id": "openai", "kind": "openai-compatible", "options": {"preset": "openai"}}})
    page, _ = open_engines(browser, lv.url)
    page.locator("#profile-rows [data-engine=openai] button.engine-edit").click()
    page.fill("#ef-label", "My OpenAI")
    page.click("#ef-save")
    assert eventually(lambda: stored(lv, "openai").get("label") == "My OpenAI")
    raw = stored(lv, "openai")
    assert not {"url", "model", "voice"} & set(raw), raw
    page.close()


def test_an_unlisted_select_value_is_kept_by_the_form(live, browser):
    # The runtime accepts only the listed ElevenLabs formats today; a value
    # the page does not list (from a newer runtime) must still survive an edit.
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.click("#engine-new")
    page.select_option("#ef-kind", "elevenlabs")
    page.evaluate("buildKindFields('elevenlabs', {output_format: 'pcm_48000'})")
    pw.expect(page.locator("#ef-opt-output_format")).to_have_value("pcm_48000")
    page.fill("#ef-id", "eleven")
    opts = page.evaluate("collectProfile().options")
    assert opts["output_format"] == "pcm_48000"
    page.close()
