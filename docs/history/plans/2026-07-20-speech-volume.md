# Speech Volume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** User-adjustable speech volume (25-200%, default 100) applied as digital gain at the single WAV playback choke point, with CLI, protocol, and settings-page slider.

**Architecture:** Pure `_scale_wav` gain function + module volume state in `platform/windows/tts.py`, applied inside `_play_wav_bytes` (covers WinRT, Kokoro, Chatterbox chunks, and cues). `SET_VOLUME` protocol op persists config and pushes the gain to the platform; the confirmation cue plays at the new volume. Settings slider mirrors `duck_level`.

**Tech Stack:** stdlib only (`wave` + `array`; `audioop` left the stdlib in 3.13), pytest, vanilla JS.

**Spec:** `docs/superpowers/specs/2026-07-20-speech-volume-design.md`

## Global Constraints

- stdlib only; no new dependencies. No em-dashes in any copy.
- Range exactly 25..200 percent, default exactly 100; clamp junk, ignore non-ints.
- `percent == 100` must return the input bytes untouched (zero-cost default).
- Non-16-bit or malformed WAV data passes through unchanged; `_scale_wav` never raises.
- Earcons and browser voice previews are NOT affected.
- Known baseline test failures (never "fix"): test_bin_sonara x3, test_daemon_ducking duck_level 20 vs 30, test_paths x2, test_transport, test_win_tts x2+1 error.

---

### Task 1: Gain function and volume state in tts.py

**Files:**
- Modify: `src/sonara/platform/windows/tts.py` (module top near `_TMP_PREFIX`, and `_play_wav_bytes` at ~line 164)
- Test: `tests/test_tts_volume.py` (new)

**Interfaces:**
- Produces: `set_volume(percent)`, `get_volume() -> int`, `_scale_wav(data, percent) -> bytes` (module functions on `sonara.platform.windows.tts`). Task 2 calls `set_volume`.

- [ ] **Step 1: Write the failing tests**

```python
"""Speech-volume gain: pure WAV scaling, no winsound needed (cross-platform)."""
import array
import io
import wave

from sonara.platform.windows import tts


def _wav16(samples, framerate=24000):
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(framerate)
        w.writeframes(array.array("h", samples).tobytes())
    return buf.getvalue()


def _samples(data):
    with wave.open(io.BytesIO(data), "rb") as r:
        out = array.array("h")
        out.frombytes(r.readframes(r.getnframes()))
    return list(out)


def test_100_percent_is_identity_bytes():
    data = _wav16([0, 1000, -1000, 32767])
    assert tts._scale_wav(data, 100) is data


def test_50_percent_halves_samples():
    data = _wav16([0, 1000, -1000, 20000])
    assert _samples(tts._scale_wav(data, 50)) == [0, 500, -500, 10000]


def test_200_percent_doubles_and_clamps():
    data = _wav16([0, 1000, -1000, 20000, -20000])
    assert _samples(tts._scale_wav(data, 200)) == [0, 2000, -2000, 32767, -32768]


def test_8bit_wav_passes_through():
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(1)
        w.setframerate(8000)
        w.writeframes(b"\x80\x90\xa0")
    data = buf.getvalue()
    assert tts._scale_wav(data, 50) == data


def test_malformed_bytes_pass_through():
    junk = b"not a wav at all"
    assert tts._scale_wav(junk, 50) == junk


def test_set_volume_clamps_and_get_reports():
    tts.set_volume(150)
    assert tts.get_volume() == 150
    tts.set_volume(999)
    assert tts.get_volume() == 200
    tts.set_volume(1)
    assert tts.get_volume() == 25
    tts.set_volume("junk")
    assert tts.get_volume() == 25          # unchanged on junk
    tts.set_volume(100)                    # restore for other tests
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_tts_volume.py -q`
Expected: FAIL (`no attribute '_scale_wav'`)

- [ ] **Step 3: Implement**

In `tts.py`, below the module docstring/constants (near `_TMP_PREFIX`):

```python
# Speech gain percent (25..200); 100 = bypass. Digital gain is the only
# both-ways volume mechanism here: winsound has no volume API and Windows
# per-app session volume can only attenuate.
_VOLUME = [100]


def set_volume(percent) -> None:
    try:
        _VOLUME[0] = max(25, min(200, int(percent)))
    except (TypeError, ValueError):
        pass


def get_volume() -> int:
    return _VOLUME[0]


def _scale_wav(data: bytes, percent: int):
    """Gain a 16-bit PCM WAV by percent/100, hard-clamped to int16. Non-16-bit
    or malformed data returns unchanged: playback must never break for want of
    a volume tweak. Stdlib only (audioop left the stdlib in 3.13)."""
    if percent == 100:
        return data
    import array
    import io
    import wave
    try:
        with wave.open(io.BytesIO(data), "rb") as r:
            if r.getsampwidth() != 2:
                return data
            params = r.getparams()
            frames = r.readframes(r.getnframes())
        samples = array.array("h")
        samples.frombytes(frames)
        gain = percent / 100.0
        out = array.array("h", bytes(len(frames)))
        for i, s in enumerate(samples):
            v = int(s * gain)
            if v > 32767:
                v = 32767
            elif v < -32768:
                v = -32768
            out[i] = v
        buf = io.BytesIO()
        with wave.open(buf, "wb") as w:
            w.setparams(params)
            w.writeframes(out.tobytes())
        return buf.getvalue()
    except Exception:  # noqa: BLE001 - never break playback for a volume tweak
        return data
```

In `_play_wav_bytes`, first line of the body:

```python
    data = _scale_wav(data, _VOLUME[0])
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_tts_volume.py tests/test_win_tts.py -q`
Expected: new tests pass; test_win_tts shows only its known baseline failures

- [ ] **Step 5: Commit**

```bash
git add src/sonara/platform/windows/tts.py tests/test_tts_volume.py
git commit -m "feat(volume): digital speech gain at the WAV playback choke point"
```

---

### Task 2: Config, protocol, daemon handler

NOTE (user decision): NO CLI subcommand. The settings page is the only
user-facing surface; the protocol op exists solely for the page.

**Files:**
- Modify: `src/sonara/config.py` (DEFAULTS: `"volume": 100,` next to `duck_level`)
- Modify: `src/sonara/protocol.py` (`SET_VOLUME = "set_volume"` after `SET_DUCK_LEVEL`)
- Modify: `src/sonara/daemon.py` (handler after SET_DUCK_LEVEL's at ~line 1025; `_apply_volume` helper; startup apply in `main()`)
- Test: `tests/test_daemon_volume.py` (new)

**Interfaces:**
- Consumes: `tts.set_volume` (Task 1).
- Produces: message `set_volume` `{volume: int}`; `SpeechDaemon._apply_volume(percent)`.

- [ ] **Step 1: Write the failing tests**

```python
"""SET_VOLUME daemon handler: clamp, persist, platform apply, spoken cue."""
from sonara.daemon import SpeechDaemon
from sonara.sessions import SessionManager
from tests.daemon_helpers import FakeSpeaker


def make_daemon(monkeypatch, config=None):
    import sonara.daemon as daemon_mod
    monkeypatch.setattr(daemon_mod, "save_config", lambda cfg: None)
    return SpeechDaemon(FakeSpeaker(), SessionManager(), config or {"minqueue": 1})


def test_set_volume_clamps_persists_and_applies(monkeypatch):
    d = make_daemon(monkeypatch)
    applied = []
    monkeypatch.setattr(d, "_apply_volume", lambda v: applied.append(v))
    d.handle_message({"v": 1, "type": "set_volume", "volume": 150})
    assert d.config["volume"] == 150
    assert applied == [150]
    d.handle_message({"v": 1, "type": "set_volume", "volume": 999})
    assert d.config["volume"] == 200
    d.handle_message({"v": 1, "type": "set_volume", "volume": "junk"})
    assert d.config["volume"] == 200                 # unchanged


def test_set_volume_speaks_confirmation(monkeypatch):
    d = make_daemon(monkeypatch)
    monkeypatch.setattr(d, "_apply_volume", lambda v: None)
    d.handle_message({"v": 1, "type": "set_volume", "volume": 150})
    from sonara.router import CONTROL
    ch = d.router.channel(CONTROL)
    assert any("150 percent" in i.text for i in ch.items)
```

Adapt `make_daemon`/`save_config` handling to how the sibling daemon tests neutralize persistence (check `tests/test_daemon_audio_mode.py` first and copy its idiom; if they don't patch `save_config`, construct the same way they do).

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_daemon_volume.py -q`
Expected: FAIL (volume unchanged / no handler)

- [ ] **Step 3: Implement**

`config.py` DEFAULTS: add `"volume": 100,` next to `"duck_level"`.

`protocol.py`: `SET_VOLUME = "set_volume"         # speech gain percent (25-200)` after SET_DUCK_LEVEL.

`daemon.py`, directly after the SET_DUCK_LEVEL handler:

```python
        if t == MsgType.SET_VOLUME:
            try:
                vol = max(25, min(200, int(msg.get("volume"))))
            except (TypeError, ValueError):
                return None
            self.config["volume"] = vol
            save_config(self.config)
            self._apply_volume(vol)
            target = self.router.active or self.sessions.foreground()
            self._speak_cue(target, "Volume {0} percent.".format(vol),
                            exempt_mute=True, pause_exempt=True)
            self._wake.set()
            return None
```

`daemon.py`, helper next to `_maybe_engage_audio`:

```python
    def _apply_volume(self, percent) -> None:
        """Push the speech gain to the platform playback layer. Best-effort:
        tests and non-Windows runs have no platform backend."""
        try:
            from sonara.platform import get_platform
            get_platform().tts.set_volume(percent)
        except Exception:  # noqa: BLE001 - volume must never break the daemon
            pass
```

`daemon.py` `main()`: after the daemon is constructed, before `daemon.run()`:

```python
    daemon._apply_volume(cfg.get("volume", 100))   # restore persisted speech gain
```

Register the new MsgType constant in `tests/test_protocol.py`'s BOTH expected dicts (`"SET_VOLUME": "set_volume",`) - the exact-equality guard fails otherwise. Do NOT touch cli.py (user decision: no CLI surface).

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_daemon_volume.py tests/test_protocol.py tests/test_config.py -q`
Expected: all pass (adapt if test_config has a defaults-pinning test: add the new key there)

- [ ] **Step 5: Commit**

```bash
git add src/sonara/config.py src/sonara/protocol.py src/sonara/daemon.py tests/test_daemon_volume.py tests/test_protocol.py
git commit -m "feat(volume): set_volume op, config persistence, startup apply"
```

---

### Task 3: Settings page slider + webui key

**Files:**
- Modify: `src/sonara/webui.py` (`_PAGE_KEYS` + `_MSG_KEYS`)
- Modify: `src/sonara/settings.html` (Audio page, above the audio-mode row)
- Test: extend `tests/test_webui.py`

**Interfaces:**
- Consumes: `set_volume` message (Task 2).

- [ ] **Step 1: Write the failing test** (mirror the existing `/api/set` duck_level test in `tests/test_webui.py` one-for-one)

```python
def test_api_set_volume_dispatches(server):
    d, s = server
    r = _post(s, "/api/set", {"key": "volume", "value": 150})
    assert r.status == 200
    assert any(m.get("type") == "set_volume" and m.get("volume") == 150
               for m in d.messages)
```

(adapt `_post` helper name to the file's real one; add `"volume": 100` to the FakeDaemon config so state echoes it.)

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_webui.py -q -k volume`
Expected: FAIL (400 unknown key)

- [ ] **Step 3: Implement**

`webui.py`: add `"volume",` to `_PAGE_KEYS`; add to `_MSG_KEYS`:

```python
    "volume":        lambda v: {"type": "set_volume", "volume": int(v)},
```

`settings.html` Audio page, above the audio-mode `.pref` row, following the chunk-slider idiom exactly (range-head + range + ends + hint):

```html
          <div class="pref" id="volume-row"><div class="pref-copy"><strong>Speech volume</strong><div class="hint">How loud Sonara speaks.</div></div><div class="control"><div class="range-head"><span>Volume</span><output id="volume-out">100 %</output></div><input class="range" id="volume" type="range" min="25" max="200" step="5" value="100"><div class="ends"><span>25 quiet</span><span>200 loud</span></div><div class="hint">Applies to speech from every engine. Beeps are unaffected.</div></div></div>
```

JS: wire exactly like the duck-level slider (find its `input`/`change` listeners and `setVal` render call; replicate for `volume` with the `" %"` suffix in `#volume-out`, posting `set("volume", parseInt(el.value))` on change).

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_webui.py -q` and the tag-balance check
`python -c "import pathlib; t=pathlib.Path('src/sonara/settings.html').read_text(encoding='utf-8'); assert t.count('<section')==t.count('</section>')"`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add src/sonara/webui.py src/sonara/settings.html tests/test_webui.py
git commit -m "feat(volume): speech volume slider on the Audio page"
```

---

### Task 4: README note

**Files:**
- Modify: `README.md`

- [ ] **Step 1:** Add "speech volume" to the feature bullet mentioning hotkeys/CLI controls (one clause, e.g. extend the Lightweight/hotkeys bullet or the Audio-related line, matching existing style).

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: speech volume note"
```

---

## Final

Full suite (baseline failures only), whole-branch review, deploy via runbook, live ear test at 50 / 100 / 200, PR referencing the issue.
