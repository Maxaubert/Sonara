"""E2E: the runtime's settings page (crates/sonarad/assets/settings.html,
#201) driven by Playwright against a real sonarad (``--engine fake
--system fake``, temporary home), so every click goes through protocol v1
and lands in ``config.json``.

Skipped unless playwright + chromium are installed and sonarad is built
(``cargo build -p sonarad``)."""
from __future__ import annotations

import json
import sys
import time
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
        """config.json as sonarad last wrote it. sonarad replaces the file by
        a rename, and Windows denies an open that races that rename
        (PermissionError) or can show the file mid-replace (empty or partial
        JSON): retry those for a moment instead of failing the test."""
        p = self.home / "config.json"
        deadline = time.monotonic() + 2.0
        while True:
            try:
                return json.loads(p.read_text(encoding="utf-8")) if p.exists() else {}
            except (PermissionError, json.JSONDecodeError):
                if time.monotonic() > deadline:
                    raise
                time.sleep(0.02)

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


def test_the_audio_page_shows_the_custom_chimes_folder(live, browser):
    # #209: <home>/earcons/<kind>.wav replaces a built-in chime.
    lv = live()
    folder = lv.home / "earcons"
    page = open_page(browser, lv.url)
    page.click("[data-page=audio]")
    pw.expect(page.locator("#earcons-folder")).to_have_text(str(folder))
    pw.expect(page.locator("#earcons-custom")).to_contain_text("None yet")
    page.close()
    pcm = b"".join((8000 if i % 16 < 8 else -8000).to_bytes(2, "little", signed=True) for i in range(800))
    fmt = (1).to_bytes(2, "little") + (1).to_bytes(2, "little") + (8000).to_bytes(4, "little") \
        + (16000).to_bytes(4, "little") + (2).to_bytes(2, "little") + (16).to_bytes(2, "little")
    body = b"WAVEfmt " + len(fmt).to_bytes(4, "little") + fmt + b"data" + len(pcm).to_bytes(4, "little") + pcm
    (folder / "turn_done.wav").write_bytes(b"RIFF" + len(body).to_bytes(4, "little") + body)
    page = open_page(browser, lv.url)
    page.click("[data-page=audio]")
    pw.expect(page.locator("#earcons-custom")).to_contain_text("Your own: turn_done")
    page.close()


# ---- the sound picker (#211) ----------------------------------------------

FIXTURES = Path(__file__).resolve().parent / "fixtures"


def small_wav(rate=8000, frames=800) -> bytes:
    pcm = b"".join((8000 if i % 16 < 8 else -8000).to_bytes(2, "little", signed=True) for i in range(frames))
    fmt = (1).to_bytes(2, "little") + (1).to_bytes(2, "little") + rate.to_bytes(4, "little") \
        + (rate * 2).to_bytes(4, "little") + (2).to_bytes(2, "little") + (16).to_bytes(2, "little")
    body = b"WAVEfmt " + len(fmt).to_bytes(4, "little") + fmt + b"data" + len(pcm).to_bytes(4, "little") + pcm
    return b"RIFF" + len(body).to_bytes(4, "little") + body


def wav_info(path: Path):
    b = path.read_bytes()
    assert b[:4] == b"RIFF" and b[8:12] == b"WAVE"
    channels = int.from_bytes(b[22:24], "little")
    rate = int.from_bytes(b[24:28], "little")
    bits = int.from_bytes(b[34:36], "little")
    return channels, rate, bits


def audio_page(browser, lv):
    page = open_page(browser, lv.url)
    page.click("[data-page=audio]")
    page.wait_for_selector("#sound-list .sound-row")
    return page


def test_every_event_has_a_row_with_what_it_means(live, browser):
    lv = live()
    page = audio_page(browser, lv)
    rows = page.locator("#sound-list .sound-row")
    pw.expect(rows).to_have_count(8)
    pw.expect(page.locator("[data-kind=turn_done]")).to_contain_text("Claude finished its reply")
    pw.expect(page.locator("[data-kind=summary_failed]")).to_contain_text("the reply is read as is")
    # Labelled for a screen reader: name plus description.
    sel = page.locator("#snd-turn_done")
    assert sel.get_attribute("aria-label") == "Reply finished sound"
    assert "snd-turn_done-desc" in sel.get_attribute("aria-describedby")
    library = lv.get("earcons")["library"]
    assert sel.locator("option").count() == len(library) + 1  # the library plus None
    # error has no default sound: it is silent and cannot be played.
    pw.expect(page.locator("#snd-error")).to_have_value("none")
    pw.expect(page.locator("[data-kind=error] .play")).to_be_disabled()
    pw.expect(page.locator("[data-kind=turn_done] [data-act=reset]")).to_be_disabled()
    pw.expect(page.locator("[data-kind=turn_done] [data-act=remove]")).to_be_hidden()
    page.close()


def test_the_play_button_previews_through_the_runtime(live, browser):
    lv = live()
    page = audio_page(browser, lv)
    with page.expect_response(lambda r: r.url.endswith("/v1/earcon_preview")) as resp:
        page.click("[data-kind=nav] .play")
    assert resp.value.request.post_data_json == {"kind": "nav"}
    assert resp.value.status == 200 and resp.value.json()["played"] is True
    page.close()


def test_choosing_a_sound_by_keyboard_previews_saves_and_survives_a_reload(live, browser):
    lv = live()
    page = audio_page(browser, lv)
    default = lv.get("earcons")["events"]["turn_done"]["default"]
    sel = page.locator("#snd-turn_done")
    sel.focus()
    with page.expect_request(lambda r: r.url.endswith("/v1/earcon_preview")) as req:
        page.keyboard.press("ArrowDown")
    assert req.value.post_data_json == {"kind": "turn_done"}
    picked = sel.input_value()
    assert picked != default
    assert eventually(lambda: lv.saved().get("earcon_sounds") == {"turn_done": picked})
    pw.expect(page.locator("#live")).to_contain_text("Reply finished:")
    assert lv.get("earcons")["events"]["turn_done"]["effective"] == picked
    page.reload()
    page.click("[data-page=audio]")
    pw.expect(page.locator("#snd-turn_done")).to_have_value(picked)
    pw.expect(page.locator("#snd-turn_done-now")).to_contain_text("Plays:")
    # Tab order in a row: the sound list, play, then the actions.
    page.locator("#snd-turn_done").focus()
    page.keyboard.press("Tab")
    pw.expect(page.locator("[data-kind=turn_done] .play")).to_be_focused()
    page.keyboard.press("Tab")
    pw.expect(page.locator("[data-kind=turn_done] [data-act=upload]")).to_be_focused()
    page.keyboard.press("Tab")
    reset = page.locator("[data-kind=turn_done] [data-act=reset]")
    pw.expect(reset).to_be_focused()
    page.keyboard.press("Enter")
    assert eventually(lambda: lv.get("earcons")["events"]["turn_done"]["effective"] == default)
    assert eventually(lambda: lv.saved().get("earcon_sounds") == {"turn_done": "default"})
    pw.expect(page.locator("#snd-turn_done")).to_have_value(default)
    # Reset is now disabled: focus moves to the row's sound list, not <body>.
    pw.expect(reset).to_be_disabled()
    pw.expect(page.locator("#snd-turn_done")).to_be_focused()
    page.close()


def test_none_silences_an_event(live, browser):
    lv = live()
    page = audio_page(browser, lv)
    page.select_option("#snd-nav", "none")
    assert eventually(lambda: lv.saved().get("earcon_sounds") == {"nav": "none"})
    pw.expect(page.locator("[data-kind=nav] .play")).to_be_disabled()
    pw.expect(page.locator("#snd-nav-now")).to_contain_text("None (silent)")
    page.close()


def test_dropping_a_wav_on_a_row_makes_it_that_events_own_sound(live, browser):
    lv = live()
    page = audio_page(browser, lv)
    data = list(small_wav())
    dt = page.evaluate_handle(
        """(bytes) => {
            const dt = new DataTransfer();
            dt.items.add(new File([new Uint8Array(bytes)], "chime.wav", {type: "audio/wav"}));
            return dt;
        }""",
        data,
    )
    row = page.locator("[data-kind=choice]")
    row.dispatch_event("dragenter", {"dataTransfer": dt})
    pw.expect(row).to_have_class("sound-row drag")
    with page.expect_response(lambda r: r.url.endswith("/v1/earcon_upload")) as resp:
        row.dispatch_event("drop", {"dataTransfer": dt})
    assert resp.value.status == 200, resp.value.text()
    pw.expect(row).to_have_class("sound-row")
    saved = lv.home / "earcons" / "choice.wav"
    assert eventually(saved.is_file)
    assert wav_info(saved) == (1, 44100, 16)
    pw.expect(page.locator("#snd-choice")).to_have_value("custom")
    pw.expect(page.locator("#live")).to_contain_text("Choice: your own sound saved")
    assert eventually(lambda: lv.saved().get("earcon_sounds") == {"choice": "custom"})
    # Remove it: the default comes back.
    remove = page.locator("[data-kind=choice] [data-act=remove]")
    pw.expect(remove).to_be_visible()
    remove.focus()
    page.keyboard.press("Enter")
    assert eventually(lambda: not saved.exists())
    pw.expect(remove).to_be_hidden()
    # The button is gone: focus moves to the row's sound list, not <body>.
    pw.expect(page.locator("#snd-choice")).to_be_focused()
    ev = lv.get("earcons")["events"]["choice"]
    assert ev["effective"] == ev["default"]
    page.close()


def test_an_mp3_from_the_file_picker_is_decoded_in_the_browser(live, browser):
    lv = live()
    page = audio_page(browser, lv)
    with page.expect_response(lambda r: r.url.endswith("/v1/earcon_upload")) as resp:
        page.locator("[data-kind=session_change] input[type=file]").set_input_files(str(FIXTURES / "tone.mp3"))
    assert resp.value.status == 200, resp.value.text()
    saved = lv.home / "earcons" / "session_change.wav"
    assert eventually(saved.is_file)
    assert wav_info(saved) == (1, 44100, 16), "mono 16-bit WAV at 44.1 kHz"
    frames = (saved.stat().st_size - 44) // 2
    assert 0.3 * 44100 < frames < 0.6 * 44100
    pw.expect(page.locator("#snd-session_change")).to_have_value("custom")
    page.close()


def test_a_file_that_is_not_audio_is_refused_with_a_message(live, browser):
    lv = live()
    page = audio_page(browser, lv)
    page.locator("[data-kind=nav_edge] input[type=file]").set_input_files(
        {"name": "notes.mp3", "mimeType": "audio/mpeg", "buffer": b"this is not audio at all"})
    pw.expect(page.locator("#live")).to_contain_text("Could not read notes.mp3")
    assert not (lv.home / "earcons" / "nav_edge.wav").exists()
    page.close()


def test_a_file_dropped_outside_a_row_does_not_leave_the_page(live, browser):
    lv = live()
    page = audio_page(browser, lv)
    # A missed drop (a heading, the gap between rows) must not let the
    # browser open the file in place of the settings page.
    prevented = page.evaluate(
        """() => {
            const dt = new DataTransfer();
            dt.items.add(new File([new Uint8Array([1, 2, 3])], "chime.mp3", {type: "audio/mpeg"}));
            const out = {};
            for (const [name, el] of [["heading", document.querySelector("#audio h1")],
                                      ["list", document.querySelector("#sound-list")]]) {
                for (const type of ["dragover", "drop"]) {
                    const ev = new DragEvent(type, {bubbles: true, cancelable: true, dataTransfer: dt});
                    el.dispatchEvent(ev);
                    out[name + "-" + type] = ev.defaultPrevented;
                }
            }
            return out;
        }"""
    )
    assert all(prevented.values()), prevented
    assert not (lv.home / "earcons").exists() or not any((lv.home / "earcons").iterdir())
    page.close()


def test_the_restart_sound_does_not_name_a_fixed_hotkey(live, browser):
    lv = live()
    page = audio_page(browser, lv)
    # The restart hotkey can be rebound, so the row must not hardcode it.
    pw.expect(page.locator("#snd-nav-desc")).not_to_contain_text("Ctrl+Alt")
    page.close()
