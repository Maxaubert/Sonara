"""E2E: the Engines section of the runtime's settings page (#227, spec
2026-10-04-external-engines-spec.md 11.3), driven by Playwright against a
real sonarad (``--engine fake --system fake --keys fake``, temporary home)
and a fake OpenAI-compatible speech server on loopback. No real provider
is ever called.

Skipped unless playwright + chromium are installed and sonarad is built
(``cargo build -p sonarad``)."""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

import pytest

pw = pytest.importorskip("playwright.sync_api")

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "conformance"))
sys.path.insert(0, str(REPO / "conformance" / "engines"))
import harness  # noqa: E402
from fake_openai import FakeOpenAI  # noqa: E402
from fakes import FakeCloud  # noqa: E402

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
    # No voice is preselected (#235): pick one of the server's.
    page.wait_for_selector("#ef-voice option[value=af_heart]", state="attached")
    page.select_option("#ef-voice", "af_heart")
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
    # A key is bound to its address: the hint says a new one needs it again.
    pw.expect(page.locator("#ef-key-hint")).to_contain_text("another address or region needs the key again")
    page.fill("#ef-label", "Renamed")
    page.click("#ef-save")
    assert eventually(lambda: lv.request({"type": "engine_list"})["engines"][0]["label"] == "Renamed")
    assert lv.request({"type": "engine_list"})["engines"][0]["key_present"] is True
    page.close()


def test_a_validation_error_is_shown_next_to_its_field(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.click("#engine-new")
    page.select_option("#ef-kind", "openai-compatible")
    page.select_option("#ef-preset", "kokoro-fastapi")
    page.fill("#ef-id", "Bad Id!")
    page.fill("#ef-url", provider.url)
    page.click("#ef-save")
    pw.expect(page.locator("#ef-id-err")).to_contain_text("Use only a to z")
    pw.expect(page.locator("#ef-id")).to_have_attribute("aria-invalid", "true")
    pw.expect(page.locator("#ef-id")).to_be_focused()
    assert "ef-id-err" in page.get_attribute("#ef-id", "aria-describedby")
    # The voice is required too (Sonara picks none, #235).
    pw.expect(page.locator("#ef-error")).to_contain_text("Not saved. Check the 2 marked fields: ID, Voice.")
    # Typing in the field clears its error.
    page.fill("#ef-id", "good")
    pw.expect(page.locator("#ef-id-err")).to_be_hidden()
    pw.expect(page.locator("#ef-id")).not_to_have_attribute("aria-invalid", "true")
    assert lv.request({"type": "engine_list"})["engines"] == []
    page.close()


def test_a_refusal_of_the_runtime_is_shown_at_its_field(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.click("#engine-new")
    page.select_option("#ef-preset", "kokoro-fastapi")
    page.fill("#ef-url", provider.url)
    page.locator("#ef-url").dispatch_event("change")
    page.wait_for_selector("#ef-voice option[value=af_heart]", state="attached")
    page.select_option("#ef-voice", "af_heart")
    page.click("details.more summary")
    page.fill("#ef-timeout", "5")
    page.click("#ef-save")
    pw.expect(page.locator("#ef-timeout-err")).to_contain_text("timeout_ms")
    pw.expect(page.locator("#ef-error")).to_contain_text("Not saved:")
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
    page.keyboard.press("Control+A")
    page.keyboard.type("kbd")
    page.locator("#ef-url").focus()
    page.keyboard.press("Control+A")   # the preset's usual address is filled in
    page.keyboard.type(provider.url)
    # Leaving the address loads the server's voices; one is picked with the
    # arrow keys (no voice is preselected, #235).
    page.locator("#ef-voice").focus()
    page.wait_for_selector("#ef-voice option[value=af_heart]", state="attached")
    page.keyboard.press("ArrowDown")
    pw.expect(page.locator("#ef-voice")).to_have_value("af_heart")
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
    # No real provider is ever called: the voice list of the unsaved form
    # (which would go to api.elevenlabs.io) is stopped in the page.
    page.route("**/v1/voices", lambda route: route.abort())
    page.route("**/v1/engine_models", lambda route: route.abort())
    page.click("#engine-new")
    # The form opens on OpenAI with its address filled in; another kind drops it.
    pw.expect(page.locator("#ef-url")).to_have_value("https://api.openai.com/v1")
    page.select_option("#ef-kind", "elevenlabs")
    pw.expect(page.locator("#ef-url")).to_have_value("")
    page.fill("#ef-id", "eleven")
    # A voice is required: starred, and the list waits for the key.
    pw.expect(page.locator("#ef-voice-req")).to_be_visible()
    pw.expect(page.locator("#ef-voice-hint")).to_contain_text("Enter the API key")
    page.select_option("#ef-voice", "__other")
    page.fill("#ef-voice-other", "voice-x")
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
        "id": "az", "kind": "azure", "voice": "en-US-VoiceZNeural",
        "options": {"region": "westeurope"}}})
    page, _ = open_engines(browser, lv.url)
    # No real provider is called: the voice list (Azure's host) is stopped.
    page.route("**/v1/voices", lambda route: route.abort())
    page.locator("#profile-rows [data-engine=az] button.engine-edit").click()
    pw.expect(page.locator("#ef-url")).to_have_value("")
    page.fill("#ef-opt-region", "eastus")
    page.click("#ef-save")
    assert eventually(lambda: lv.request({"type": "engine_list"})["engines"][0]["sends_text_to"]
                      == "eastus.tts.speech.microsoft.com")
    assert "url" not in stored(lv, "az")
    page.close()


def test_renaming_a_preset_profile_keeps_the_preset_defaults(live, browser):
    # The address stays the preset's own; the model and voice stay what
    # the user picked (Sonara fills in none, #235).
    lv = live()
    lv.request({"type": "engine_add", "engine": {
        "id": "openai", "kind": "openai-compatible", "model": "m-picked", "voice": "v-picked",
        "options": {"preset": "openai"}}})
    page, _ = open_engines(browser, lv.url)
    page.route("**/v1/engine_models", lambda route: route.abort())
    page.locator("#profile-rows [data-engine=openai] button.engine-edit").click()
    # The form starts from what the profile set (listed or typed).
    assert eventually(lambda: page.evaluate("chosenModel()") == "m-picked")
    assert eventually(lambda: page.evaluate("chosenVoice()") == "v-picked")
    page.fill("#ef-label", "My OpenAI")
    page.click("#ef-save")
    assert eventually(lambda: stored(lv, "openai").get("label") == "My OpenAI")
    raw = stored(lv, "openai")
    assert "url" not in raw, raw
    assert raw["model"] == "m-picked" and raw["voice"] == "v-picked", raw
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


def test_the_form_never_offers_a_program_on_this_pc(live, browser):
    # A command engine runs a program: it is set up only locally (engines.json
    # or `sonara engines add`), never from the settings page.
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.click("#engine-new")
    kinds = page.locator("#ef-kind option").evaluate_all("os => os.map(o => o.value)")
    assert "command" not in kinds and "openai-compatible" in kinds, kinds
    assert page.evaluate("Object.keys(KIND_FIELDS)").count("command") == 0
    page.close()


def test_a_command_engine_is_listed_used_and_removed_but_never_edited(live, browser, tmp_path):
    home = tmp_path / "home"
    home.mkdir(parents=True, exist_ok=True)
    (home / "engines.json").write_text(json.dumps({"format": 1, "engines": [
        {"id": "piper", "kind": "command", "label": "Piper",
         "options": {"argv": [sys.executable, "-c", "pass"]}}]}), encoding="utf-8")
    lv = live()
    page, _ = open_engines(browser, lv.url)
    row = page.locator("#profile-rows [data-engine=piper]")
    pw.expect(row).to_be_visible()
    pw.expect(row).to_contain_text("on this PC")
    pw.expect(row.locator("button.engine-edit")).to_have_count(0)
    pw.expect(row.locator("button.engine-test")).to_be_visible()
    pw.expect(row).to_contain_text("sonara.exe engines add")
    row.locator("button.engine-use").click()
    assert eventually(lambda: lv.get("engine") == "piper")
    page.once("dialog", lambda d: d.accept())
    row.locator("button.engine-remove").click()
    pw.expect(row).to_have_count(0)
    assert eventually(lambda: lv.request({"type": "engine_list"})["engines"] == [])
    page.close()


# ---- the Speech page's engine picker and the setup form (#227) -------------


def add_local(lv, provider, label="Test server"):
    lv.request({"type": "engine_add", "engine": {
        "id": "local", "kind": "openai-compatible", "label": label, "url": provider.url,
        "key_ref": "none", "options": {"preset": "kokoro-fastapi"}}})


def open_speech(browser, url):
    page = browser.new_page()
    page.goto(url)
    pw.expect(page.locator("#rt-version")).not_to_have_text("–")
    page.click("[data-page=speech]")
    return page


def options(page, sel):
    return page.locator(sel + " option").evaluate_all("os => os.map(o => [o.value, o.textContent])")


def test_the_engine_dropdown_lists_the_engines_and_add_new_opens_the_form(live, browser, provider):
    lv = live()
    add_local(lv, provider)
    page = open_speech(browser, lv.url)
    page.wait_for_selector("#engine-select option[value=local]", state="attached")
    opts = options(page, "#engine-select")
    assert ["fake", "fake"] in opts and ["local", "Test server"] in opts, opts
    assert opts[-1] == ["__add", "Add new engine…"], opts
    # One plain list: no "Built in" / "Your engines" headings.
    assert page.locator("#engine-select optgroup").count() == 0
    pw.expect(page.locator("#engine-select")).to_have_value("fake")
    page.select_option("#engine-select", "__add")
    pw.expect(page.locator("#h-engines")).to_be_visible()
    pw.expect(page.locator("#engine-form")).to_be_visible()
    pw.expect(page.locator("#ef-kind")).to_be_focused()
    pw.expect(page.locator("#ef-title")).to_have_text("Add an engine")
    # The current engine did not change.
    assert lv.get("engine") == "fake"
    page.close()


def test_selecting_an_engine_switches_to_it_and_its_voices_follow(live, browser, provider):
    lv = live()
    add_local(lv, provider)
    page = open_speech(browser, lv.url)
    page.wait_for_selector("#engine-select option[value=local]", state="attached")
    page.select_option("#engine-select", "local")
    assert eventually(lambda: lv.get("engine") == "local")
    pw.expect(page.locator("#speech .state")).to_contain_text("Now reading with Test server")
    pw.expect(page.locator("#engine-status")).to_contain_text("Test server")
    page.wait_for_selector("#voice-select option[value=am_echo]", state="attached")
    page.select_option("#engine-select", "fake")
    assert eventually(lambda: lv.get("engine") == "fake")
    page.close()


def test_required_fields_are_starred(live, browser):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.route("**/v1/voices", lambda route: route.abort())
    page.route("**/v1/engine_models", lambda route: route.abort())
    page.click("#engine-new")
    pw.expect(page.locator("#ef-legend")).to_contain_text("Required field")
    for fid in ("ef-kind", "ef-label", "ef-id"):
        assert page.locator(f"label[for={fid}] .req").is_visible(), fid
        pw.expect(page.locator("#" + fid)).to_have_attribute("aria-required", "true")
    # Nothing is preselected (#235): OpenAI needs a key, a model and a voice.
    pw.expect(page.locator("#ef-key-req")).to_be_visible()
    pw.expect(page.locator("#ef-voice-req")).to_be_visible()
    pw.expect(page.locator("#ef-model-req")).to_be_visible()
    pw.expect(page.locator("#ef-model-select")).to_have_value("")
    pw.expect(page.locator("#ef-voice")).to_have_value("")
    # ElevenLabs needs a key and a voice; its model is its own choice.
    page.select_option("#ef-kind", "elevenlabs")
    pw.expect(page.locator("#ef-voice-req")).to_be_visible()
    pw.expect(page.locator("#ef-model-req")).to_be_hidden()
    assert options(page, "#ef-model-select")[0] == ["", "ElevenLabs' own choice"]
    pw.expect(page.locator("#ef-key")).to_have_attribute("aria-required", "true")
    # Save with everything missing: each field says what is wrong.
    page.fill("#ef-label", "")
    page.click("#ef-save")
    pw.expect(page.locator("#ef-label-err")).to_contain_text("Give the engine a label")
    pw.expect(page.locator("#ef-key-err")).to_contain_text("Paste the API key")
    pw.expect(page.locator("#ef-voice-err")).to_contain_text("Pick a voice")
    pw.expect(page.locator("#ef-label")).to_be_focused()
    pw.expect(page.locator("#ef-error")).to_contain_text("3 marked fields")
    assert lv.request({"type": "engine_list"})["engines"] == []
    page.close()


def test_the_id_is_made_from_the_label(live, browser, provider):
    lv = live()
    add_local(lv, provider, label="Local")
    page, _ = open_engines(browser, lv.url)
    page.route("**/v1/voices", lambda route: route.abort())
    page.click("#engine-new")
    pw.expect(page.locator("#ef-label")).to_have_value("OpenAI")
    pw.expect(page.locator("#ef-id")).to_have_value("openai")
    page.select_option("#ef-kind", "elevenlabs")
    pw.expect(page.locator("#ef-label")).to_have_value("ElevenLabs")
    pw.expect(page.locator("#ef-id")).to_have_value("elevenlabs")
    page.fill("#ef-label", "My Voice! (Prod) été")
    pw.expect(page.locator("#ef-id")).to_have_value("my-voice-prod-ete")
    page.fill("#ef-label", "Local")
    pw.expect(page.locator("#ef-id")).to_have_value("local-2")   # unique
    page.fill("#ef-label", "Kokoro")
    pw.expect(page.locator("#ef-id")).to_have_value("my-kokoro")   # a built-in id is taken
    # An ID the user typed stays.
    page.fill("#ef-id", "custom")
    page.fill("#ef-label", "Something else")
    pw.expect(page.locator("#ef-id")).to_have_value("custom")
    page.close()


def test_saving_says_it_is_saved_and_changes_are_marked(live, browser, provider):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.click("#engine-new")
    pw.expect(page.locator("#ef-badge")).to_have_text("Not saved yet")
    pw.expect(page.locator("#ef-test")).to_have_attribute("aria-disabled", "true")
    page.click("#ef-cancel")
    add_through_the_form(page, provider)
    save = page.locator("#ef-save")
    pw.expect(save).to_have_text("Saved")
    pw.expect(save).to_have_class(re.compile(r"\bis-saved\b"))
    pw.expect(page.locator("#ef-status")).to_contain_text("Test server is saved.")
    pw.expect(page.locator("#ef-status")).to_have_attribute("role", "status")
    pw.expect(page.locator("#ef-badge")).to_have_text("Saved")
    pw.expect(page.locator("#ef-title")).to_have_text("Edit Test server")
    pw.expect(page.locator("#ef-id")).to_be_disabled()
    # Test is tied to the saved engine: on now, off while there are changes.
    pw.expect(page.locator("#ef-test")).not_to_have_attribute("aria-disabled", "true")
    page.fill("#ef-label", "Renamed")
    pw.expect(save).to_have_text("Save changes")
    pw.expect(page.locator("#ef-badge")).to_have_text("Unsaved changes")
    pw.expect(page.locator("#ef-status")).to_contain_text("Unsaved changes")
    pw.expect(page.locator("#ef-test")).to_have_attribute("aria-disabled", "true")
    page.click("#ef-save")
    pw.expect(save).to_have_text("Saved")
    pw.expect(page.locator("#ef-status")).to_contain_text("Changes to Renamed are saved.")
    before = len(provider.speech())
    page.click("#ef-test")
    pw.expect(page.locator("#ef-status")).to_contain_text("Test passed")
    assert len(provider.speech()) == before + 1
    page.close()


def test_voices_load_before_a_save_and_name_the_engine(live, browser):
    cloud = FakeCloud("elevenlabs", SECRET)
    try:
        lv = live()
        page, _ = open_engines(browser, lv.url)
        page.click("#engine-new")
        page.select_option("#ef-kind", "elevenlabs")
        page.fill("#ef-url", cloud.url)
        page.fill("#ef-key", SECRET)
        # No voice id typed and nothing saved: the voices are there.
        page.wait_for_selector("#ef-voice option[value=voice-a]", state="attached")
        assert ["voice-a", "A (ElevenLabs)"] in options(page, "#ef-voice")
        pw.expect(page.locator("#ef-voice-hint")).to_contain_text("1 voice from ElevenLabs")
        assert lv.request({"type": "engine_list"})["engines"] == []
        assert any(r["path"].startswith("/v2/voices") and r["headers"].get("xi-api-key") == SECRET
                   for r in cloud.requests)
        page.select_option("#ef-voice", "voice-a")
        page.click("#ef-save")
        pw.expect(page.locator("#ef-save")).to_have_text("Saved")
        view = lv.request({"type": "engine_list"})["engines"][0]
        assert view["id"] == "elevenlabs" and view["voice"] == "voice-a" and view["key_present"]
        page.close()
    finally:
        cloud.stop()


def test_arrow_keys_browse_the_engine_dropdown_and_enter_switches(live, browser, provider):
    # Browsing a closed select with the arrows fires change in Chromium:
    # it must not switch the engine or leave the page (WCAG 3.2.2).
    lv = live()
    add_local(lv, provider)
    page = open_speech(browser, lv.url)
    page.wait_for_selector("#engine-select option[value=local]", state="attached")
    sel = page.locator("#engine-select")
    sel.focus()
    page.keyboard.press("ArrowDown")
    pw.expect(sel).to_have_value("local")
    pw.expect(page.locator("#engine-status")).to_have_text("Press Enter to switch to Test server.")
    page.wait_for_timeout(400)
    assert lv.get("engine") == "fake"
    page.keyboard.press("ArrowDown")
    pw.expect(sel).to_have_value("__add")
    page.wait_for_timeout(400)
    pw.expect(page.locator("#speech")).to_be_visible()
    pw.expect(page.locator("#engine-form")).to_be_hidden()
    assert lv.get("engine") == "fake"
    page.keyboard.press("Escape")
    pw.expect(sel).to_have_value("fake")
    page.keyboard.press("ArrowDown")
    page.keyboard.press("Enter")
    assert eventually(lambda: lv.get("engine") == "local")
    pw.expect(page.locator("#speech")).to_be_visible()
    pw.expect(page.locator("#engine-status")).to_contain_text("Test server")
    page.close()


def test_the_status_line_never_names_the_engine_it_left(live, browser, provider):
    lv = live()
    add_local(lv, provider)
    page = open_speech(browser, lv.url)
    page.wait_for_selector("#engine-select option[value=local]", state="attached")
    pw.expect(page.locator("#engine-status")).to_contain_text("fake")
    page.evaluate("""() => {
      window.__lines = [];
      const n = document.getElementById("engine-status");
      new MutationObserver(() => window.__lines.push(n.textContent))
        .observe(n, {childList: true, characterData: true, subtree: true});
    }""")
    page.select_option("#engine-select", "local")
    pw.expect(page.locator("#engine-status")).to_contain_text("Test server, ")
    page.wait_for_timeout(3500)   # a poll or more
    lines = page.evaluate("window.__lines")
    assert lines and not any(t.startswith("fake") for t in lines), lines
    page.close()


def test_an_open_page_does_not_ask_a_failing_provider_for_voices_each_poll(live, browser):
    cloud = FakeCloud("elevenlabs", SECRET)
    try:
        lv = live()
        lv.request({"type": "engine_add", "secret": "the-wrong-key-0123456789", "engine": {
            "id": "el", "kind": "elevenlabs", "label": "ElevenLabs", "url": cloud.url,
            "voice": "voice-a", "key_ref": "credman"}})
        lv.request({"type": "set", "key": "engine", "value": "el"})
        page = open_speech(browser, lv.url)
        page.wait_for_timeout(10000)   # three polls and more
        asked = [r for r in cloud.requests if r["path"].startswith("/v2/voices")]
        assert len(asked) <= 1, asked
        page.close()
    finally:
        cloud.stop()


def test_a_half_typed_address_never_gets_the_key(live, browser):
    lv = live()
    page, _ = open_engines(browser, lv.url)
    sent = []

    def keep(route):
        body = route.request.post_data_json or {}
        if "profile" in body:   # a draft's voices (the poll asks for none)
            sent.append(body)
        route.abort()

    page.route("**/v1/voices", keep)
    page.click("#engine-new")
    page.select_option("#ef-kind", "elevenlabs")
    page.fill("#ef-key", SECRET)
    page.wait_for_timeout(700)
    before = len(sent)
    page.locator("#ef-url").press_sequentially("https://api.e", delay=20)
    page.wait_for_timeout(900)
    assert len(sent) == before, sent[before:]
    page.locator("#ef-url").press_sequentially("u.example", delay=20)
    page.keyboard.press("Tab")   # change: the address is complete
    for _ in range(50):   # the route handler runs while Playwright waits
        if len(sent) > before:
            break
        page.wait_for_timeout(100)
    assert len(sent) > before
    assert sent[-1]["profile"]["url"] == "https://api.eu.example"
    page.close()


def test_the_speech_page_names_the_engine_on_its_voices(live, browser, provider):
    lv = live()
    add_local(lv, provider)
    lv.request({"type": "set", "key": "engine", "value": "local"})
    page = open_speech(browser, lv.url)
    page.wait_for_selector("#voice-select option[value=am_echo]", state="attached")
    labels = [t for v, t in options(page, "#voice-select") if v == "am_echo"]
    assert labels and labels[0].endswith("(Test server)"), labels
    page.close()


def test_gemini_offers_googles_live_models_and_voices_and_presets_none(live, browser):
    # #235: no model and no voice in Sonara. With the key typed, the model
    # list (the speech models of GET /v1beta/models) and the voices
    # (GET /v1beta/voices) load from Google (here the fake); both are
    # required, nothing is preselected, and the saved engine streams.
    cloud = FakeCloud("gemini", SECRET)
    try:
        lv = live()
        page, _ = open_engines(browser, lv.url)
        page.click("#engine-new")
        assert ["gemini", "Gemini (Google AI Studio)"] in options(page, "#ef-kind")
        page.select_option("#ef-kind", "gemini")
        pw.expect(page.locator("#ef-label")).to_have_value("Gemini")
        pw.expect(page.locator("#ef-id")).to_have_value("gemini")
        pw.expect(page.locator("#ef-key-req")).to_be_visible()
        pw.expect(page.locator("#ef-key")).to_have_attribute("aria-required", "true")
        pw.expect(page.locator("#ef-model-req")).to_be_visible()
        pw.expect(page.locator("#ef-voice-req")).to_be_visible()
        pw.expect(page.locator("#ef-key-hint")).to_contain_text("aistudio.google.com")
        pw.expect(page.locator("#ef-key-hint")).to_contain_text("free tier")
        # Nothing preselected, no model or voice name anywhere in the form.
        pw.expect(page.locator("#ef-model-select")).to_have_value("")
        pw.expect(page.locator("#ef-voice")).to_have_value("")
        form_text = page.locator("#engine-form").inner_html()
        for name in ("gemini-3", "flash-lite", "Kore", "Puck"):
            assert name not in form_text, name
        assert page.locator("#ef-opt-style").is_visible()
        # The wait for audio and the split limit are common options now
        # (review of #236), with Gemini's defaults as placeholders.
        assert page.locator("#ef-opt-first_audio_ms").count() == 0
        pw.expect(page.locator("#ef-first-audio")).to_have_attribute("placeholder", "12000")
        pw.expect(page.locator("#ef-chunk")).to_have_attribute("placeholder", "2000")
        # The pre-release Gemini options are folded into "Send to the
        # engine" (#235): a whole message per request by default.
        assert page.locator("#ef-opt-chunk_chars").count() == 0
        assert page.locator("#ef-opt-quick_start").count() == 0
        pw.expect(page.locator("#ef-send [data-value=message]")).to_have_attribute("aria-checked", "true")
        # Before the key: the lists wait for it, nothing is fetched.
        pw.expect(page.locator("#ef-voice-hint")).to_contain_text("Enter the API key")
        pw.expect(page.locator("#ef-model-hint")).to_contain_text("Enter the API key")
        assert cloud.requests == []
        # Save without anything: each required field says so.
        page.click("#ef-save")
        pw.expect(page.locator("#ef-key-err")).to_contain_text("Paste the API key")
        pw.expect(page.locator("#ef-model-select-err")).to_contain_text("Choose a model")
        pw.expect(page.locator("#ef-voice-err")).to_contain_text("Pick a voice")
        assert lv.request({"type": "engine_list"})["engines"] == []
        # With the key (and the fake's address): Google's lists.
        page.fill("#ef-url", cloud.url)
        page.locator("#ef-url").dispatch_event("change")
        page.fill("#ef-key", SECRET)
        page.wait_for_selector("#ef-model-select option[value=tts-g]", state="attached")
        models = options(page, "#ef-model-select")
        assert ["tts-g", "TTS G (tts-g)"] in models
        assert not any(v == "chat-g" for v, _ in models), "speech models only"
        page.wait_for_selector("#ef-voice option[value=voice-g]", state="attached")
        assert ["voice-g", "G (Gemini)"] in options(page, "#ef-voice")
        pw.expect(page.locator("#ef-model-select")).to_have_value("")
        pw.expect(page.locator("#ef-voice")).to_have_value("")
        assert any(r["path"].startswith("/v1beta/models?") and r["headers"].get("x-goog-api-key") == SECRET
                   for r in cloud.requests)
        assert lv.request({"type": "engine_list"})["engines"] == [], "nothing saved yet"
        page.select_option("#ef-model-select", "tts-g")
        page.select_option("#ef-voice", "voice-g")
        page.click("#ef-save")
        pw.expect(page.locator("#ef-save")).to_have_text("Saved")
        view = lv.request({"type": "engine_list"})["engines"][0]
        assert view["kind"] == "gemini" and view["model"] == "tts-g" and view["voice"] == "voice-g"
        assert view["key_present"] and view["missing"] == []
        assert view["send_mode"] == "message" and "send_mode" not in view["explicit"]
        assert "chunk_chars" not in view["options"] and "quick_start" not in view["options"]
        page.click("#ef-test")
        pw.expect(page.locator("#ef-status")).to_contain_text("Test passed")
        sent = cloud.speech()
        assert sent and sent[-1]["headers"]["x-goog-api-key"] == SECRET
        assert sent[-1]["path"] == "/v1beta/models/tts-g:streamGenerateContent?alt=sse"
        assert json.loads(sent[-1]["body"])["generationConfig"]["speechConfig"] == {
            "voiceConfig": {"prebuiltVoiceConfig": {"voiceName": "voice-g"}}}
        assert SECRET not in page.content()
        page.close()
    finally:
        cloud.stop()


def send_choice(page):
    return page.locator("#ef-send [aria-checked=true]").get_attribute("data-value")


def test_send_to_the_engine_is_preselected_per_kind_and_saved_when_chosen(live, browser, provider):
    # #235: "Send to the engine" with two choices, each with its one-line
    # hint. The default is preselected per provider (a whole message for the
    # cloud, a sentence at a time for a server on this PC) and is sent only
    # once the user picks one; an edit shows the stored choice.
    cloud = FakeCloud("elevenlabs", SECRET)
    try:
        lv = live()
        page, _ = open_engines(browser, lv.url)
        page.click("#engine-new")
        group = page.locator("#ef-send")
        pw.expect(group).to_have_attribute("role", "radiogroup")
        pw.expect(page.locator("#ef-send-label")).to_have_text("Send to the engine")
        labels = [b.inner_text() for b in group.locator("[role=radio]").all()]
        assert labels == ["Full message in one request", "As it comes in"]
        # OpenAI (the cloud) first: a whole message.
        assert send_choice(page) == "message"
        pw.expect(page.locator("#ef-send-hint")).to_contain_text("One request per reply")
        pw.expect(page.locator("#ef-send-hint")).to_contain_text("default for this provider")
        # A full message's options (review of #236): the wait for audio and
        # the split limit, shown only for it; the timeout speaks of answers.
        page.locator("#engine-form details.more").evaluate("d => { d.open = true; }")
        pw.expect(page.locator("#ef-first-audio-row")).to_be_visible()
        pw.expect(page.locator("#ef-chunk-row")).to_be_visible()
        pw.expect(page.locator("#ef-timeout-hint")).to_contain_text("full message gets more time")
        # A server on this PC: a sentence at a time.
        page.select_option("#ef-preset", "kokoro-fastapi")
        assert send_choice(page) == "sentence"
        pw.expect(page.locator("#ef-send-hint")).to_contain_text("One request per sentence")
        pw.expect(page.locator("#ef-first-audio-row")).to_be_hidden()
        pw.expect(page.locator("#ef-chunk-row")).to_be_hidden()
        # Its address moved off this PC: the cloud default again.
        page.fill("#ef-url", "https://tts.example.com/v1")
        page.locator("#ef-url").dispatch_event("change")
        assert send_choice(page) == "message"
        # A cloud kind: a whole message; the keyboard picks the other.
        page.select_option("#ef-kind", "elevenlabs")
        assert send_choice(page) == "message"
        page.fill("#ef-url", cloud.url)
        page.locator("#ef-url").dispatch_event("change")
        page.fill("#ef-key", SECRET)
        page.wait_for_selector("#ef-voice option[value=voice-a]", state="attached")
        page.select_option("#ef-voice", "voice-a")
        page.locator("#ef-send [data-value=message]").focus()
        page.keyboard.press("ArrowRight")
        assert send_choice(page) == "sentence"
        pw.expect(page.locator("#ef-send [data-value=sentence]")).to_be_focused()
        hint = page.locator("#ef-send-hint")
        pw.expect(hint).to_contain_text("One request per sentence")
        pw.expect(hint).not_to_contain_text("default")
        page.click("#ef-save")
        pw.expect(page.locator("#ef-save")).to_have_text("Saved")
        view = lv.request({"type": "engine_list"})["engines"][0]
        assert view["send_mode"] == "sentence" and view["explicit"]["send_mode"] == "sentence"
        # Reopened for an edit, the stored choice shows; back to a whole
        # message, saved.
        page.click("#ef-cancel")
        page.locator("#profile-rows [data-engine=elevenlabs] button.engine-edit").click()
        pw.expect(page.locator("#engine-form")).to_be_visible()
        assert send_choice(page) == "sentence"
        page.click("#ef-send [data-value=message]")
        page.locator("#engine-form details.more").evaluate("d => { d.open = true; }")
        page.fill("#ef-first-audio", "8000")
        page.fill("#ef-chunk", "1500")
        page.click("#ef-save")
        pw.expect(page.locator("#ef-save")).to_have_text("Saved")
        view = lv.request({"type": "engine_list"})["engines"][0]
        assert view["send_mode"] == "message"
        assert view["options"]["first_audio_ms"] == 8000 and view["options"]["chunk_chars"] == 1500
        page.close()
    finally:
        cloud.stop()


def test_a_stored_profile_without_a_model_or_voice_says_choose(live, browser, tmp_path):
    # #235 migration: the user's Gemini profile stored neither (Sonara used
    # to fill them in). It is listed, says what to pick, and its edit form
    # starts with nothing chosen.
    home = tmp_path / "home"
    home.mkdir(parents=True, exist_ok=True)
    (home / "engines.json").write_text(json.dumps({"format": 2, "engines": [
        {"id": "gemini", "kind": "gemini", "label": "Gemini", "key_ref": "credman", "options": {}},
        {"id": "el", "kind": "elevenlabs", "label": "ElevenLabs", "voice": "stored-voice",
         "key_ref": "credman", "options": {}}]}), encoding="utf-8")
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.route("**/v1/voices", lambda route: route.abort())
    page.route("**/v1/engine_models", lambda route: route.abort())
    row = page.locator("#profile-rows [data-engine=gemini]")
    pw.expect(row).to_contain_text("Choose a model and a voice (Edit).")
    pw.expect(page.locator("#profile-rows [data-engine=el]")).not_to_contain_text("Choose a")
    row.locator("button.engine-edit").click()
    pw.expect(page.locator("#ef-model-select")).to_have_value("")
    pw.expect(page.locator("#ef-voice")).to_have_value("")
    pw.expect(page.locator("#ef-model-req")).to_be_visible()
    page.close()


def test_openai_lists_its_models_and_types_its_voice(live, browser, provider):
    # OpenAI's API has a model list but no voice list (#235): the models
    # come from GET /v1/models, the voice is typed, with a link to OpenAI's
    # voice page. The fake server stands in for OpenAI's address.
    lv = live()
    page, _ = open_engines(browser, lv.url)
    page.click("#engine-new")
    page.select_option("#ef-preset", "openai")
    page.fill("#ef-url", provider.url)
    page.locator("#ef-url").dispatch_event("change")
    page.fill("#ef-key", SECRET)
    pw.expect(page.locator("#ef-model-req")).to_be_visible()
    page.wait_for_selector("#ef-model-select option[value=tts-a]", state="attached")
    models = [v for v, _ in options(page, "#ef-model-select")]
    assert models == ["", "tts-a", "tts-b", "__other"], "the speech models, nothing preselected"
    pw.expect(page.locator("#ef-model-select")).to_have_value("")
    pw.expect(page.locator("#ef-voice-hint")).to_contain_text("has no voice list")
    pw.expect(page.locator("#ef-voice-hint")).to_contain_text("platform.openai.com")
    page.select_option("#ef-model-select", "tts-b")
    page.select_option("#ef-voice", "__other")
    page.fill("#ef-voice-other", "typed-voice")
    page.fill("#ef-id", "oa")
    page.click("#ef-save")
    pw.expect(page.locator("#ef-save")).to_have_text("Saved")
    view = lv.request({"type": "engine_list"})["engines"][0]
    assert view["model"] == "tts-b" and view["voice"] == "typed-voice", view
    page.close()

