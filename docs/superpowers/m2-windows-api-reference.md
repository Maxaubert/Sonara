# M2 — Verified Windows API Reference (implementation source for the M2 plan)
> Web-grounded, citable code from the `m2-windows-api-intel` workflow (2026-06-11). The M2 plan tasks point here for the backend bodies. **Adapt paths to our layout** (`src/sonari/platform/windows/...`); the plan gives the exact target files. Windows-only imports stay lazy/guarded.

---

## Windows OneCore TTS backend via PyWinRT — production-grade Python module with subprocess-like handle
**pip/import packages:** winrt-runtime>=3.2.1, winrt-Windows.Media.SpeechSynthesis>=3.2.1, winrt-Windows.Media.Playback>=3.2.1, winrt-Windows.Media.Core>=3.2.1, winrt-Windows.Storage.Streams>=3.2.1, winrt.windows.media.speechsynthesis (import name), winrt.windows.media.playback (import name), winrt.system (import name)

### pip packages and Python import statements
_Source: https://pypi.org/pypi/winrt-Windows.Media.SpeechSynthesis/json ; https://pypi.org/pypi/winrt-Windows.Media.Playback/json ; https://github.com/pywinrt/pywinrt/blob/master/samples/text_to_speech/text_to_speech.py_

```python
# Install (Windows only):
# pip install winrt-runtime winrt-Windows.Media.SpeechSynthesis \
#             winrt-Windows.Media.Playback winrt-Windows.Media.Core \
#             winrt-Windows.Storage.Streams

# Python imports:
from winrt.windows.media.speechsynthesis import (
    SpeechSynthesizer,
    SpeechAppendedSilence,
    SpeechPunctuationSilence,
)
from winrt.windows.media.playback import MediaPlayer, MediaPlayerAudioCategory
from winrt.system import Object
```

**Gotchas:** PyPI package names use hyphens and Title-Case (winrt-Windows.Media.SpeechSynthesis); Python import paths use all-lowercase snake_case (winrt.windows.media.speechsynthesis). winrt-runtime must be installed alongside each projection package. winrt-Windows.Media.Core and winrt-Windows.Storage.Streams are required at runtime even if not imported explicitly, because SpeechSynthesisStream wraps them internally.

### SpeechSynthesizer construction; AllVoices enumeration; best_voice() with OneCore priority
_Source: https://learn.microsoft.com/en-us/uwp/api/windows.media.speechsynthesis.speechsynthesizer.allvoices ; https://learn.microsoft.com/en-us/uwp/api/windows.media.speechsynthesis.voiceinformation_

```python
def list_voices() -> list:
    """Return all installed VoiceInformation objects."""
    return list(SpeechSynthesizer.all_voices)

def best_voice(lang_prefix: str = "en-US"):
    """
    Select a voice in priority order:
      1. en-US OneCore (Neural/HQ) — VoiceInformation.Id contains 'Speech_OneCore'
      2. Any en-US voice
      3. System DefaultVoice
    Raises RuntimeError if no voices installed at all.
    """
    voices = list_voices()
    if not voices:
        raise RuntimeError(
            "No TTS voices installed. Add a Speech language pack in "
            "Settings -> Time & language -> Speech -> Add voices."
        )
    ll = lang_prefix.lower()
    def _is_onecore(v) -> bool:
        return "speech_onecore" in (v.id or "").lower()
    for v in voices:
        if v.language.lower().startswith(ll) and _is_onecore(v):
            return v
    for v in voices:
        if v.language.lower().startswith(ll):
            return v
    return SpeechSynthesizer.default_voice
```

**Gotchas:** SpeechSynthesizer.all_voices is a static IVectorView<VoiceInformation>; wrap in list() for Python iteration. VoiceType enum (Natural vs Standard) is unavailable in older SDK projections — use the registry Id string 'Speech_OneCore' as a heuristic instead. default_voice may be None on Server SKUs with no audio subsystem.

### Full synth->stream->play->completion pattern with _TtsHandle
_Source: https://github.com/pywinrt/pywinrt/blob/master/samples/text_to_speech/text_to_speech.py ; https://learn.microsoft.com/en-us/uwp/api/windows.media.playback.mediaplayer ; https://learn.microsoft.com/en-us/uwp/api/windows.media.speechsynthesis.speechsynthesizer.synthesizetexttostreamasync_

```python
import subprocess
import threading
from typing import Optional

class _TtsHandle:
    """
    Subprocess-like handle for in-flight TTS playback.
    returncode: None while playing, 0 = completed normally, 1 = interrupted.
    """
    def __init__(self, player: MediaPlayer, stream, synth: SpeechSynthesizer):
        self._player = player
        self._stream = stream    # GC-ref: must outlive playback
        self._synth  = synth     # GC-ref: must outlive stream
        self._done   = threading.Event()
        self.returncode: Optional[int] = None

        def _on_media_ended(sender: MediaPlayer, args: object) -> None:
            if self.returncode is None:
                self.returncode = 0
            self._done.set()

        self._cb    = _on_media_ended           # prevent GC of callback
        self._token = player.add_media_ended(_on_media_ended)

    def wait(self, timeout: Optional[float] = None) -> int:
        completed = self._done.wait(timeout=timeout)
        if not completed:
            raise subprocess.TimeoutExpired(cmd="onecore-tts", timeout=timeout)
        return self.returncode

    def terminate(self) -> None:
        if self.returncode is None:
            self.returncode = 1
        try:
            self._player.pause()
            self._player.close()
        except Exception:
            pass
        self._done.set()

def run(text: str, voice=None, rate: float = 1.0) -> _TtsHandle:
    synth        = SpeechSynthesizer()
    synth.voice  = voice if voice is not None else best_voice()

    opts = synth.options
    opts.appended_silence    = SpeechAppendedSilence.MIN
    opts.punctuation_silence = SpeechPunctuationSilence.MIN

    use_ssml = False
    try:
        opts.speaking_rate = float(rate)
    except AttributeError:
        pct      = int(rate * 100)
        safe_txt = text.replace("&","&amp;").replace("<","&lt;").replace(">","&gt;")
        text = (
            '<speak version="1.0" '
            'xmlns="http://www.w3.org/2001/10/synthesis" xml:lang="en-US">'
            f'<prosody rate="{pct}%">{safe_txt}</prosody>'
            '</speak>'
        )
        use_ssml = True

    if use_ssml:
        stream = synth.synthesize_ssml_to_stream_async(text).get()
    else:
        stream = synth.synthesize_text_to_stream_async(text).get()

    player                  = MediaPlayer()
    player.audio_category   = MediaPlayerAudioCategory.SPEECH
    player.set_stream_source(stream)

    handle = _TtsHandle(player=player, stream=stream, synth=synth)
    player.play()
    return handle
```

**Gotchas:** Use IAsyncOperation.get() (synchronous blocking) rather than asyncio await — a console daemon thread has no running event loop. threading.Event is required because the media_ended callback fires on a WinRT thread-pool thread, not the Python main thread; asyncio.Event is not thread-safe across threads. Keep Python-side references to stream and synth alive; COM ref-counting alone does not prevent CPython GC from collecting them mid-playback.

### Four hardening steps: silence options, speaking_rate with SSML fallback, GC-ref pattern
_Source: https://learn.microsoft.com/en-us/uwp/api/windows.media.speechsynthesis.speechsynthesizeroptions.appendedsilence ; https://learn.microsoft.com/en-us/uwp/api/windows.media.speechsynthesis.speechsynthesizeroptions.speakingrate ; https://learn.microsoft.com/en-us/uwp/api/windows.media.speechsynthesis.speechsynthesizeroptions.punctuationsilence_

```python
# 1. Minimize inter-utterance silence (Win10 1803 / SDK 17134+)
opts.appended_silence    = SpeechAppendedSilence.MIN      # enum value 1
opts.punctuation_silence = SpeechPunctuationSilence.MIN   # enum value 1

# 2. speaking_rate with AttributeError guard (pre-Win10 1709 fallback to SSML)
try:
    opts.speaking_rate = float(rate)   # range [0.5, 6.0]; default 1.0
except AttributeError:
    # speaking_rate absent on Win10 < 1709 (SDK < 16299)
    pct      = int(rate * 100)
    safe_txt = text.replace("&","&amp;").replace("<","&lt;").replace(">","&gt;")
    text = (
        '<speak version="1.0" '
        'xmlns="http://www.w3.org/2001/10/synthesis" xml:lang="en-US">'
        f'<prosody rate="{pct}%">{safe_txt}</prosody>'
        '</speak>'
    )
    use_ssml = True

# 3. GC-ref pattern: hold stream + synth on the handle object
#    (see _TtsHandle.__init__: self._stream = stream; self._synth = synth)

# 4. Keep callback reference alive
#    (see _TtsHandle.__init__: self._cb = _on_media_ended)
```

**Gotchas:** SpeechAppendedSilence and SpeechPunctuationSilence were introduced in SDK 17134 (Win10 1803); on older systems they raise AttributeError — add a guard if targeting Win10 1709. speaking_rate requires SDK 16299 (Win10 1709). SSML prosody rate is a percentage string: '150%' = 1.5x. The SSML xml:lang must match the voice language or synthesis may fail silently.

### Rate mapping: Sonari wpm 100-400 to SpeakingRate multiplier 0.5-6.0
_Source: https://learn.microsoft.com/en-us/uwp/api/windows.media.speechsynthesis.speechsynthesizeroptions.speakingrate_

```python
_BASELINE_WPM: float = 150.0  # approximate default wpm for OneCore en-US voices

def wpm_to_speaking_rate(wpm: float) -> float:
    """Map Sonari [100-400] wpm to SpeakingRate multiplier [0.5-6.0]."""
    return max(0.5, min(6.0, wpm / _BASELINE_WPM))

# Usage in run():
#   rate = wpm_to_speaking_rate(wpm)   # e.g. 300 wpm -> 2.0x
#   handle = run(text, voice=best_voice(), rate=rate)

# Spot checks:
# wpm=100  -> 0.667 (clamped to 0.667, within [0.5,6.0])
# wpm=150  -> 1.0   (default)
# wpm=300  -> 2.0
# wpm=400  -> 2.667
# wpm=75   -> 0.5   (clamped to minimum)
```

**Gotchas:** The 150 wpm baseline is an empirical approximation for OneCore en-US Neural voices; different voices (Desktop legacy, non-English) have different natural rates. SpeakingRate is a multiplier, not an absolute wpm. Values outside [0.5, 6.0] raise an exception — always clamp. For the SSML prosody fallback, convert the multiplier: pct = int(rate * 100), e.g. rate=2.0 -> '<prosody rate="200%">'.

### Complete onecore_tts.py production module
_Source: https://github.com/pywinrt/pywinrt/blob/master/samples/text_to_speech/text_to_speech.py ; https://learn.microsoft.com/en-us/uwp/api/windows.media.speechsynthesis ; https://learn.microsoft.com/en-us/uwp/api/windows.media.playback.mediaplayer_

```python
"""
onecore_tts.py — Windows OneCore TTS backend via PyWinRT.

Exposes run(text, voice, rate) -> _TtsHandle, where the handle is
subprocess-like: .wait(timeout), .terminate(), .returncode.

Requirements (Windows only):
    pip install winrt-runtime winrt-Windows.Media.SpeechSynthesis \\
                winrt-Windows.Media.Playback winrt-Windows.Media.Core \\
                winrt-Windows.Storage.Streams
"""
from __future__ import annotations

import subprocess
import threading
from typing import Optional

from winrt.windows.media.speechsynthesis import (
    SpeechSynthesizer,
    SpeechAppendedSilence,
    SpeechPunctuationSilence,
)
from winrt.windows.media.playback import MediaPlayer, MediaPlayerAudioCategory
from winrt.system import Object


def list_voices() -> list:
    """Return all installed VoiceInformation objects."""
    return list(SpeechSynthesizer.all_voices)


def best_voice(lang_prefix: str = "en-US"):
    """
    Select a voice in priority order:
      1. en-US OneCore (Neural/HQ) — Id path contains 'Speech_OneCore'
      2. Any en-US voice
      3. System DefaultVoice
    Raises RuntimeError if no voices are installed at all.
    """
    voices = list_voices()
    if not voices:
        raise RuntimeError(
            "No TTS voices installed. Add a Speech language pack in "
            "Settings -> Time & language -> Speech -> Add voices."
        )
    ll = lang_prefix.lower()

    def _is_onecore(v) -> bool:
        return "speech_onecore" in (v.id or "").lower()

    for v in voices:
        if v.language.lower().startswith(ll) and _is_onecore(v):
            return v
    for v in voices:
        if v.language.lower().startswith(ll):
            return v
    return SpeechSynthesizer.default_voice


_BASELINE_WPM: float = 150.0


def wpm_to_speaking_rate(wpm: float) -> float:
    """Map Sonari [100-400] wpm to SpeakingRate multiplier [0.5-6.0]."""
    return max(0.5, min(6.0, wpm / _BASELINE_WPM))


class _TtsHandle:
    """
    Subprocess-like handle for in-flight TTS playback.
    returncode: None while playing, 0 = completed normally, 1 = interrupted.
    """

    def __init__(self, player: MediaPlayer, stream, synth: SpeechSynthesizer):
        self._player = player
        self._stream = stream    # GC-ref: must outlive playback
        self._synth  = synth     # GC-ref: must outlive stream
        self._done   = threading.Event()
        self.returncode: Optional[int] = None

        def _on_media_ended(sender: MediaPlayer, args: object) -> None:
            if self.returncode is None:
                self.returncode = 0
            self._done.set()

        self._cb    = _on_media_ended   # prevent GC of callback closure
        self._token = player.add_media_ended(_on_media_ended)

    def wait(self, timeout: Optional[float] = None) -> int:
        completed = self._done.wait(timeout=timeout)
        if not completed:
            raise subprocess.TimeoutExpired(cmd="onecore-tts", timeout=timeout)
        return self.returncode

    def terminate(self) -> None:
        if self.returncode is None:
            self.returncode = 1
        try:
            self._player.pause()
            self._player.close()
        except Exception:
            pass
        self._done.set()


def run(text: str, voice=None, rate: float = 1.0) -> _TtsHandle:
    """
    Synthesize *text* and begin playback immediately.
    Returns a _TtsHandle the caller can .wait() or .terminate().

    Args:
        text:  Utterance to speak.
        voice: VoiceInformation object; defaults to best_voice().
        rate:  SpeakingRate multiplier [0.5, 6.0]; use wpm_to_speaking_rate().
    """
    synth       = SpeechSynthesizer()
    synth.voice = voice if voice is not None else best_voice()

    opts = synth.options
    opts.appended_silence    = SpeechAppendedSilence.MIN
    opts.punctuation_silence = SpeechPunctuationSilence.MIN

    use_ssml = False
    try:
        opts.speaking_rate = float(rate)
    except AttributeError:
        # Win10 < 1709: speaking_rate not available; fall back to SSML
        pct      = int(rate * 100)
        safe_txt = (
            text.replace("&", "&amp;")
                .replace("<", "&lt;")
                .replace(">", "&gt;")
        )
        text = (
            '<speak version="1.0" '
            'xmlns="http://www.w3.org/2001/10/synthesis" xml:lang="en-US">'
            f'<prosody rate="{pct}%">{safe_txt}</prosody>'
            '</speak>'
        )
        use_ssml = True

    if use_ssml:
        stream = synth.synthesize_ssml_to_stream_async(text).get()
    else:
        stream = synth.synthesize_text_to_stream_async(text).get()

    player                = MediaPlayer()
    player.audio_category = MediaPlayerAudioCategory.SPEECH
    player.set_stream_source(stream)

    handle = _TtsHandle(player=player, stream=stream, synth=synth)
    player.play()
    return handle
```

**Gotchas:** This module imports at the top level, so on macOS/Linux it will ImportError immediately. Guard with a platform check or use the sys.modules injection shim in conftest.py before importing. player.close() is idempotent on real WinRT but may raise on the fake; wrap in try/except. synthesize_text_to_stream_async().get() blocks the calling thread — call run() from a background thread if the main thread must stay responsive.

**Mock strategy:**

Inject fake modules into sys.modules before any import of onecore_tts.py. Place the following in tests/conftest.py so pytest loads it automatically on every platform:

```python
# tests/conftest.py
import sys
import types
import threading

def _install_winrt_fakes():
    root        = types.ModuleType(\"winrt\")
    sys_mod     = types.ModuleType(\"winrt.system\")
    win         = types.ModuleType(\"winrt.windows\")
    win_med     = types.ModuleType(\"winrt.windows.media\")
    win_med_play  = types.ModuleType(\"winrt.windows.media.playback\")
    win_med_synth = types.ModuleType(\"winrt.windows.media.speechsynthesis\")

    class Object: pass
    sys_mod.Object = Object

    class SpeechAppendedSilence:
        DEFAULT = 0; MIN = 1

    class SpeechPunctuationSilence:
        DEFAULT = 0; MIN = 1

    class _FakeOptions:
        appended_silence    = SpeechAppendedSilence.DEFAULT
        punctuation_silence = SpeechPunctuationSilence.DEFAULT
        speaking_rate       = 1.0

    class _FakeStream: pass

    class _FakeAsyncOp:
        def __init__(self, result): self._result = result
        def get(self): return self._result

    class _FakeVoice:
        def __init__(self,
                     id=\"HKEY_LOCAL_MACHINE\\\\SOFTWARE\\\\Microsoft\\\\Speech_OneCore\\\\en-US\",
                     language=\"en-US\", display_name=\"FakeVoice\"):
            self.id = id; self.language = language; self.display_name = display_name

    class SpeechSynthesizer:
        all_voices    = [_FakeVoice()]
        default_voice = _FakeVoice()
        def __init__(self):
            self.voice   = None
            self.options = _FakeOptions()
        def synthesize_text_to_stream_async(self, text):
            return _FakeAsyncOp(_FakeStream())
        def synthesize_ssml_to_stream_async(self, ssml):
            return _FakeAsyncOp(_FakeStream())

    win_med_synth.SpeechSynthesizer      = SpeechSynthesizer
    win_med_synth.SpeechAppendedSilence  = SpeechAppendedSilence
    win_med_synth.SpeechPunctuationSilence = SpeechPunctuationSilence

    class MediaPlayerAudioCategory:
        SPEECH = 3

    class MediaPlayer:
        audio_category = None
        def __init__(self): self._cb = None
        def set_stream_source(self, stream): pass
        def add_media_ended(self, cb):
            self._cb = cb; return 0
        def play(self):
            # Simulate WinRT thread-pool callback after brief delay
            t = threading.Timer(0.01, self._fire)
            t.daemon = True; t.start()
        def _fire(self):
            if self._cb: self._cb(self, None)
        def pause(self): pass
        def close(self): pass

    win_med_play.MediaPlayer             = MediaPlayer
    win_med_play.MediaPlayerAudioCategory = MediaPlayerAudioCategory

    for name, mod in [
        (\"winrt\",                              root),
        (\"winrt.system\",                       sys_mod),
        (\"winrt.windows\",                      win),
        (\"winrt.windows.media\",                win_med),
        (\"winrt.windows.media.playback\",       win_med_play),
        (\"winrt.windows.media.speechsynthesis\", win_med_synth),
    ]:
        sys.modules.setdefault(name, mod)

_install_winrt_fakes()
```

Example pytest tests that run on any platform:

```python
# tests/test_onecore_tts.py
import subprocess
import pytest
from onecore_tts import run, wpm_to_speaking_rate, best_voice, list_voices

def test_list_voices_returns_list():
    voices = list_voices()
    assert isinstance(voices, list) and len(voices) >= 1

def test_best_voice_returns_onecore():
    v = best_voice(\"en-US\")
    assert \"speech_onecore\" in v.id.lower()

def test_run_completes_normally():
    h = run(\"hello\", rate=1.0)
    rc = h.wait(timeout=2.0)
    assert rc == 0

def test_terminate_sets_returncode_1():
    h = run(\"hello\")
    h.terminate()
    assert h.returncode == 1

def test_wait_timeout_raises():
    # Patch MediaPlayer.play to never fire callback
    from winrt.windows.media.playback import MediaPlayer
    orig_play = MediaPlayer.play
    MediaPlayer.play = lambda self: None   # silent no-op
    try:
        h = run(\"stuck\")
        with pytest.raises(subprocess.TimeoutExpired):
            h.wait(timeout=0.05)
    finally:
        MediaPlayer.play = orig_play

def test_wpm_mapping():
    assert wpm_to_speaking_rate(150) == pytest.approx(1.0)
    assert wpm_to_speaking_rate(300) == pytest.approx(2.0)
    assert wpm_to_speaking_rate(50)  == pytest.approx(0.5)   # clamped
    assert wpm_to_speaking_rate(999) == pytest.approx(6.0)   # clamped
```

The key principle: sys.modules.setdefault() must run before Python processes `from winrt... import ...` at module load time. conftest.py is loaded by pytest before any test file is imported, satisfying this ordering automatically. For non-pytest usage (e.g., a standalone test script), call _install_winrt_fakes() at the top of the script before importing onecore_tts.

**Open risks (mock-blind — for the acceptance checklist):**
- wpm baseline (150 wpm) is empirical for OneCore en-US Neural voices — other voices (Desktop legacy, non-English, male vs female Neural) have different natural rates, so the mapped speaking_rate will produce inaccurate wpm for those voices.
- IAsyncOperation.get() blocks the calling thread for the entire synthesis duration (typically 0.1-2s for short utterances). If run() is called on the main thread, the UI/event loop will stall. Always call from a background thread.
- GC risk: if the caller discards the _TtsHandle before playback ends, _stream and _synth are released by CPython GC even though COM still holds a reference to the underlying WinRT object. The handle must be kept alive (e.g., stored in Speaker state) until .wait() returns.
- player.close() after terminate() may raise if the MediaPlayer was already in a terminal state or if the WinRT object was GC'd. The bare except in terminate() silences this, but underlying COM errors are swallowed silently.
- WinRT thread-pool callback safety: _on_media_ended fires on an arbitrary WinRT thread. threading.Event.set() is thread-safe, but any other Python state touched inside the callback must be thread-safe too. Do not call asyncio APIs from the callback.
- Windows Server / headless audio: MediaPlayer requires an audio device or a virtual audio session. On Windows Server Core, Azure VMs, or Docker containers without audio, set_stream_source or play() may raise COM exception AUDCLNT_E_NO_AUDIO_ENDPOINT (0x88890008). Use Windows Audio Session API virtual device workarounds or test on Desktop SKU.
- OneCore voice Id heuristic: identifying a OneCore voice by 'Speech_OneCore' in the registry path Id string is undocumented and could break if Microsoft changes the registry layout in a future Windows release. VoiceType enum (VoiceType.Natural) is the official API but was not available in PyWinRT projections at research time.
- SpeechAppendedSilence.MIN and SpeechPunctuationSilence.MIN require SDK 17134 (Win10 1803). On Win10 1709 these attributes exist but the enum values may differ. On Win10 < 1709 setting them raises AttributeError — add a guard if supporting older Windows.
- The SSML prosody rate fallback uses percentage strings (e.g. '150%'), which is the SSML 1.0 syntax. Some OneCore voices clamp the rate more aggressively via SSML than via speaking_rate, so behavior may differ from the direct speaking_rate path.
- add_media_ended returns a token (EventRegistrationToken) that should ideally be passed to remove_media_ended when the handle is torn down to prevent a callback holding a reference to a closed player. Current implementation omits remove_media_ended — low risk in practice since the player is closed via terminate() or naturally ends, but technically a resource leak on long-lived players.

---

## Sonari Windows earcon backend: winsound play(), stdlib WAV generator, default_earcons(), and pytest mock strategy
**pip/import packages:** wave (stdlib), struct (stdlib), math (stdlib), winsound (Windows stdlib only), pathlib (stdlib), importlib.resources (stdlib, 3.7+), pytest (pip: pytest>=7.0)

### winsound earcon play() — non-blocking, returns a poll()-able stub
_Source: https://docs.python.org/3/library/winsound.html — winsound.PlaySound(sound, flags); flag values confirmed from CPython Modules/winmmmodule.c: SND_FILENAME=0x20000, SND_ASYNC=0x0004_

```python
# sonari/backends/windows.py
from __future__ import annotations
import pathlib

# winsound is Windows-only; imported lazily so the module is importable
# on macOS/Linux (for tests / dev).
try:
    import winsound as _winsound
    _SND_FLAGS = _winsound.SND_FILENAME | _winsound.SND_ASYNC  # 0x20004 = 131076
except ModuleNotFoundError:  # non-Windows; only reached at runtime, not import-time
    _winsound = None  # type: ignore[assignment]
    _SND_FLAGS = 0


class _DoneHandle:
    """Returned on a successful play() call.

    winsound.PlaySound(..., SND_ASYNC) hands the audio buffer to the Win32
    multimedia scheduler and returns immediately — there is no OS-level
    process or thread handle exposed to Python.  poll() therefore returns 0
    (POSIX convention: exited normally) immediately, which satisfies the
    EarconBackend contract (caller may call .poll() to check completion).

    CAVEAT — single-channel truncation:
    If you supply a stereo (2-channel) WAV, Windows mixes it down silently;
    you will NOT get a RuntimeError.  However, non-standard PCM variants
    (float32, 24-bit int, ADPCM) cause PlaySound to return False or raise
    RuntimeError on some Windows builds.  Always generate 16-bit integer
    PCM at 44100 Hz (what generate_earcon() produces).

    CAVEAT — concurrent calls:
    Each new SND_ASYNC call stops the previous one.  Do NOT delete the .wav
    file immediately after play(); the Win32 scheduler still holds a handle
    to it for the duration of playback.
    """
    def poll(self) -> int:
        return 0  # immediately "done" from Python's perspective


class _MissingHandle:
    """Returned when the .wav path does not exist."""
    def poll(self) -> None:
        return None


def play(path: str) -> _DoneHandle | _MissingHandle:
    """Play *path* asynchronously via winsound.

    Returns a handle whose .poll() mimics subprocess.Popen.poll():
      0    -> sound was dispatched successfully
      None -> file was missing (nothing played)

    Raises RuntimeError (from winsound itself) only if Windows cannot
    open the audio device, which is distinct from a missing file.
    """
    if not pathlib.Path(path).exists():
        return _MissingHandle()
    _winsound.PlaySound(path, _SND_FLAGS)  # SND_FILENAME | SND_ASYNC
    return _DoneHandle()
```

**Gotchas:** SND_ASYNC returns as soon as the sound is posted to the Win32 mixer, so poll()=0 does NOT mean playback has finished — it means the dispatch succeeded. A new play() call will cut off any still-playing async sound (there is no SND_NOSTOP-safe concurrent path in Python). The .wav file MUST remain on disk for the full playback duration. winsound.PlaySound() accepts stereo WAV but mixes to mono internally; non-PCM formats (float, 24-bit, ADPCM) cause silent failure or RuntimeError. Do not call this from a non-main thread on Python < 3.11 — the Win32 multimedia APIs are apartment-threaded and winsound does not marshal the call.

### generate_earcon() — write a 16-bit PCM mono WAV using only wave + struct + math
_Source: https://docs.python.org/3/library/wave.html (wave.open, setnchannels, setsampwidth, setframerate, writeframes); https://docs.python.org/3/library/struct.html (<h = little-endian signed 16-bit); https://docs.python.org/3/library/math.html (math.sin, math.pi)_

```python
# sonari/earcons/generate.py
"""Pure-stdlib earcon generator.  No third-party dependencies.

WAV format produced: RIFF/WAVE, PCM (AudioFormat=1), 16-bit signed
Little-Endian, mono, 44100 Hz.  This is the exact format winsound
requires; it is also accepted by macOS AudioToolbox and Linux ALSA.
"""
from __future__ import annotations
import math
import pathlib
import struct
import wave


_SAMPLE_RATE = 44100  # Hz — universally accepted by winsound / Windows
_AMPLITUDE   = 28000  # out of 32767 (16-bit max); leaves headroom
_ATTACK_S    = 0.010  # 10 ms linear attack
_RELEASE_S   = 0.010  # 10 ms linear release


def generate_earcon(
    path: str | pathlib.Path,
    freq: float,
    duration: float,
    *,
    sample_rate: int = _SAMPLE_RATE,
    wave_type: str = "sine",   # "sine" | "dual" | "chirp"
    freq2: float | None = None,
) -> None:
    """Write a short earcon .wav file.

    Parameters
    ----------
    path:        destination file path (parents must exist)
    freq:        fundamental frequency in Hz
    duration:    length in seconds
    sample_rate: default 44100 (required by winsound on Windows)
    wave_type:   "sine"  — pure sine at *freq*
                 "dual"  — 60% *freq* + 40% *freq2* (two-tone blend)
                 "chirp" — linear freq sweep from *freq* to *freq2*
    freq2:       second frequency; required for "dual" and "chirp"
    """
    n           = int(sample_rate * duration)
    attack_n    = int(_ATTACK_S  * sample_rate)
    release_n   = int(_RELEASE_S * sample_rate)
    frames: list[bytes] = []

    for i in range(n):
        t = i / sample_rate

        # Trapezoid amplitude envelope — removes click at start/end
        if i < attack_n:
            env = i / attack_n
        elif i >= n - release_n:
            env = (n - i) / release_n
        else:
            env = 1.0

        if wave_type == "sine":
            v = math.sin(2 * math.pi * freq * t)

        elif wave_type == "dual":
            if freq2 is None:
                raise ValueError("freq2 required for wave_type='dual'")
            v = 0.6 * math.sin(2 * math.pi * freq  * t) \
              + 0.4 * math.sin(2 * math.pi * freq2 * t)

        elif wave_type == "chirp":
            if freq2 is None:
                raise ValueError("freq2 required for wave_type='chirp'")
            # instantaneous frequency increases linearly over the clip
            f_inst = freq + (freq2 - freq) * (i / n)
            v = math.sin(2 * math.pi * f_inst * t)

        else:
            raise ValueError(f"Unknown wave_type: {wave_type!r}")

        sample = max(-32767, min(32767, int(v * env * _AMPLITUDE)))
        frames.append(struct.pack("<h", sample))  # little-endian signed 16-bit

    with wave.open(str(path), "w") as w:
        w.setnchannels(1)          # mono
        w.setsampwidth(2)          # 16-bit = 2 bytes
        w.setframerate(sample_rate)
        w.writeframes(b"".join(frames))


# ---------------------------------------------------------------------------
# The 6 canonical Sonari earcons
# ---------------------------------------------------------------------------
_EARCON_SPECS: dict[str, tuple] = {
    # name         freq   dur   wave_type  freq2
    "permission": (440.0, 0.12, "sine",    None ),  # A4 — clean, neutral ask
    "choice":     (660.0, 0.15, "dual",    880.0),  # E5+A5 — bright two-tone
    "plan":       (528.0, 0.20, "chirp",   660.0),  # C5→E5 rising sweep
    "error":      (220.0, 0.25, "dual",    185.0),  # low dissonant pair
    "turn_done":  (880.0, 0.10, "sine",    None ),  # A5 — short, high
    "ready":      (523.0, 0.18, "chirp",   784.0),  # C5→G5 ascending
}


def generate_all_earcons(output_dir: str | pathlib.Path) -> None:
    """Write all 6 earcon .wav files into *output_dir*.

    Idempotent — safe to call multiple times; overwrites existing files.
    Typical use: run once from the repo root to regenerate assets::

        python -m sonari.earcons.generate
    """
    output_dir = pathlib.Path(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    for name, (freq, dur, wtype, freq2) in _EARCON_SPECS.items():
        generate_earcon(
            output_dir / f"{name}.wav",
            freq, dur,
            wave_type=wtype,
            freq2=freq2,
        )


if __name__ == "__main__":
    import sys
    dest = sys.argv[1] if len(sys.argv) > 1 else str(pathlib.Path(__file__).parent)
    generate_all_earcons(dest)
    print(f"Generated {len(_EARCON_SPECS)} earcons in {dest}")
```

**Gotchas:** wave.open() second arg must be 'w' (string), not a mode flag — it does not accept 'wb'. setsampwidth(2) = 16-bit; setsampwidth(1) = 8-bit unsigned (different range, avoid). The wave module writes standard RIFF/WAVE with AudioFormat=1 (PCM) — verified by inspecting header bytes at offsets 20-22. 44100 Hz is the only sample rate guaranteed to work with winsound on all Windows versions; 22050 Hz works on most but some old hardware drivers reject it. The chirp formula uses instantaneous phase (f_inst * t) rather than accumulated phase, which introduces a small phase discontinuity — use accumulated phase for perfectly smooth chirps if audible artifacts appear: phase += 2*pi*f_inst/sample_rate per sample.

### default_earcons() — return bundled .wav paths via importlib.resources (zip-safe) or __file__ fallback
_Source: https://docs.python.org/3/library/importlib.resources.html#importlib.resources.files (Python 3.9+); https://docs.python.org/3/library/importlib.resources.html#importlib.resources.as_file_

```python
# sonari/earcons/__init__.py
"""Resolve the bundled earcon .wav paths.

Package layout::

    sonari/
        earcons/
            __init__.py        <- this file
            permission.wav
            choice.wav
            plan.wav
            error.wav
            turn_done.wav
            ready.wav

pyproject.toml / setup.cfg must declare these as package data::

    [tool.setuptools.package-data]
    sonari = ["earcons/*.wav"]
"""
from __future__ import annotations
import importlib.resources as _ilr
import pathlib
import sys

_EARCON_NAMES: tuple[str, ...] = (
    "permission",
    "choice",
    "plan",
    "error",
    "turn_done",
    "ready",
)

# Cache so we only resolve paths once per process
_cache: dict[str, str] = {}


def default_earcons() -> dict[str, str]:
    """Return {earcon_name: absolute_wav_path} for all bundled earcons.

    Resolution strategy:
    * Python 3.9+ : importlib.resources.files() — zip-safe Traversable API.
      Works when sonari is installed from a wheel (.whl) without unpacking.
    * Python 3.7-3.8 : pathlib relative to __file__ (dev installs,
      editable installs, and unpacked wheels).

    Raises FileNotFoundError if an expected .wav is absent from the package
    (e.g. package data was not included in the distribution).
    """
    if _cache:
        return dict(_cache)

    for name in _EARCON_NAMES:
        fname = f"{name}.wav"

        if sys.version_info >= (3, 9):
            # Traversable path — works inside zip archives (wheels, zipapp)
            ref = _ilr.files(__package__).joinpath(fname)
            with _ilr.as_file(ref) as p:
                resolved = str(p.resolve())
        else:
            # __file__-relative — reliable for editable / unpacked installs
            resolved = str(
                (pathlib.Path(__file__).parent / fname).resolve()
            )

        if not pathlib.Path(resolved).exists():
            raise FileNotFoundError(
                f"Bundled earcon not found: {resolved!r}\n"
                f"Run: python -m sonari.earcons.generate  "
                f"(then commit the .wav files)"
            )
        _cache[name] = resolved

    return dict(_cache)
```

**Gotchas:** importlib.resources.as_file() returns a context manager that may yield a TEMPORARY path for zip-based installs — the temp file is cleaned up when the context exits. Do NOT store the path from inside the with-block and use it after the block closes. The implementation above stores the resolved path while still inside as_file(), which is safe because as_file() for regular (non-zip) file-based packages returns the real on-disk path directly (no temp copy). For zip-based installs a temp file is extracted; callers must not delete the returned path. The __package__ passed to files() must match the installed package name exactly; if this module is run as __main__ (python earcons/__init__.py), __package__ is None — add a guard or use the generate.py script instead.

### pytest mock strategy — inject fake winsound into sys.modules before importing the backend
_Source: pytest monkeypatch.setitem docs: https://docs.pytest.org/en/stable/reference/fixtures.html#monkeypatch; sys.modules injection: https://docs.python.org/3/reference/import.html#the-module-cache_

```python
# tests/conftest.py
import sys
import types
import pytest


@pytest.fixture(autouse=True)
def fake_winsound(monkeypatch):
    """Inject a fake winsound module so the Windows backend is importable
    and testable on macOS / Linux CI without a sound device.

    The fake replicates only the symbols the backend uses:
      winsound.PlaySound(sound: str, flags: int) -> None
      winsound.SND_FILENAME, winsound.SND_ASYNC
    """
    mod = types.ModuleType("winsound")
    mod.SND_FILENAME  = 0x20000  # 131072
    mod.SND_ASYNC     = 0x0004   #      4
    mod.SND_NODEFAULT = 0x0002
    mod.SND_SYNC      = 0x0000

    calls: list[tuple[str, int]] = []

    def _play(sound: str, flags: int) -> None:
        calls.append((sound, flags))

    mod.PlaySound = _play
    mod._calls = calls  # test helper

    monkeypatch.setitem(sys.modules, "winsound", mod)
    # If the backend was already imported, reload it so it picks up the mock
    if "sonari.backends.windows" in sys.modules:
        import importlib
        importlib.reload(sys.modules["sonari.backends.windows"])
    yield mod


# -----------------------------------------------------------------
# tests/test_windows_backend.py
import os
import pathlib
import struct
import tempfile
import wave
import math
import pytest


@pytest.fixture
def tmp_wav(tmp_path) -> pathlib.Path:
    """Write a minimal valid 16-bit PCM WAV to a temp file."""
    p = tmp_path / "earcon.wav"
    with wave.open(str(p), "w") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(44100)
        frames = b"".join(
            struct.pack("<h", int(math.sin(2 * math.pi * 440 * i / 44100) * 10000))
            for i in range(4410)  # 0.1 s
        )
        w.writeframes(frames)
    return p


def test_play_returns_done_handle_for_existing_file(tmp_wav, fake_winsound):
    from sonari.backends.windows import play
    handle = play(str(tmp_wav))
    assert handle.poll() == 0, "Expected 0 (done) for a real file"


def test_play_calls_playsound_with_correct_flags(tmp_wav, fake_winsound):
    from sonari.backends.windows import play
    play(str(tmp_wav))
    assert len(fake_winsound._calls) == 1
    sound, flags = fake_winsound._calls[0]
    assert sound == str(tmp_wav)
    # SND_FILENAME | SND_ASYNC
    assert flags == (fake_winsound.SND_FILENAME | fake_winsound.SND_ASYNC)


def test_play_returns_missing_handle_for_absent_file(fake_winsound):
    from sonari.backends.windows import play
    handle = play("/nonexistent/path/sound.wav")
    assert handle.poll() is None, "Expected None for missing file"


def test_missing_handle_does_not_call_playsound(fake_winsound):
    from sonari.backends.windows import play
    play("/nonexistent/path/sound.wav")
    assert fake_winsound._calls == [], "PlaySound must not be called for missing files"


# -----------------------------------------------------------------
# tests/test_earcon_generator.py  — pure stdlib, no mock needed
import pathlib
import struct
import wave
import math

from sonari.earcons.generate import generate_earcon, _EARCON_SPECS


def _wav_header(path: pathlib.Path) -> dict:
    with open(path, "rb") as f:
        raw = f.read(44)
    return {
        "riff":        raw[0:4],
        "wave":        raw[8:12],
        "fmt":         raw[12:16],
        "audio_fmt":   struct.unpack("<H", raw[20:22])[0],
        "channels":    struct.unpack("<H", raw[22:24])[0],
        "sample_rate": struct.unpack("<I", raw[24:28])[0],
        "bits":        struct.unpack("<H", raw[34:36])[0],
    }


def test_generate_earcon_writes_valid_pcm_wav(tmp_path):
    p = tmp_path / "test.wav"
    generate_earcon(p, freq=440.0, duration=0.12)
    h = _wav_header(p)
    assert h["riff"]        == b"RIFF"
    assert h["wave"]        == b"WAVE"
    assert h["audio_fmt"]   == 1,     "Must be PCM (AudioFormat=1)"
    assert h["channels"]    == 1,     "Must be mono"
    assert h["sample_rate"] == 44100, "Must be 44100 Hz"
    assert h["bits"]        == 16,    "Must be 16-bit"


def test_generate_earcon_duration_is_accurate(tmp_path):
    p = tmp_path / "test.wav"
    target_dur = 0.20
    generate_earcon(p, freq=528.0, duration=target_dur, wave_type="chirp", freq2=660.0)
    with wave.open(str(p), "r") as w:
        actual = w.getnframes() / w.getframerate()
    assert abs(actual - target_dur) < 1e-3
    assert w.getnframes() == int(44100 * target_dur)  # exact frame count


@pytest.mark.parametrize("name,spec", list(_EARCON_SPECS.items()))
def test_all_earcon_specs_produce_valid_wav(tmp_path, name, spec):
    freq, dur, wtype, freq2 = spec
    p = tmp_path / f"{name}.wav"
    generate_earcon(p, freq, dur, wave_type=wtype, freq2=freq2)
    h = _wav_header(p)
    assert h["audio_fmt"]   == 1
    assert h["channels"]    == 1
    assert h["sample_rate"] == 44100
    assert h["bits"]        == 16
    assert p.stat().st_size > 0
```

**Gotchas:** sys.modules injection must happen BEFORE the module under test is imported, or the module will have already bound the real (absent) winsound at import time. The conftest fixture uses monkeypatch.setitem (not setattr) on sys.modules so pytest automatically restores the original state after each test — avoids leaking the fake between test files. If the backend module caches winsound at the top level (import winsound as _winsound), you must importlib.reload() it after injection, or restructure the backend to import lazily (inside the function). The test for the WAV generator (test_earcon_generator.py) is pure stdlib and runs unmodified on any OS — no mock needed.

**Mock strategy:**

Inject a fake winsound module via monkeypatch.setitem(sys.modules, 'winsound', fake_mod) in a pytest conftest.py fixture BEFORE any import of the backend. The fake module (types.ModuleType) defines winsound.PlaySound(sound, flags) as a list-appending spy, plus the integer constants SND_FILENAME=0x20000, SND_ASYNC=0x0004, SND_NODEFAULT=0x0002, SND_SYNC=0x0000. Use autouse=True on the fixture so every test gets the fake automatically on non-Windows CI. If the backend module imports winsound at module-level (not lazily), also call importlib.reload(sys.modules['sonari.backends.windows']) after injection to force re-binding. The WAV generator tests (test_earcon_generator.py) need NO mock at all — generate_earcon() is pure stdlib wave+struct+math and runs identically on macOS, Linux, and Windows. Assert the WAV header directly by reading raw bytes: offset 20-22 must be b'\\x01\\x00' (AudioFormat=1, PCM), offset 22-24 channels=1, offset 24-28 sample_rate=44100, offset 34-36 bits=16. Assert frame count equals int(44100 * duration) exactly.

**Open risks (mock-blind — for the acceptance checklist):**
- winsound.PlaySound() with SND_ASYNC posts the sound to the Win32 multimedia scheduler and returns immediately — there is NO OS-level completion signal exposed to Python. The _DoneHandle.poll()=0 contract means 'dispatched successfully', not 'finished playing'. Callers that need to know when playback ends must track duration manually (time.sleep(duration)) or use a higher-level Windows API (pywaveout / win32api) outside stdlib.
- Concurrent SND_ASYNC calls: each new PlaySound call silently cancels the previous async sound. If two earcons are triggered in rapid succession (e.g. 'turn_done' immediately followed by 'ready'), the first is cut off. Mitigation: add a minimum gap guard (e.g. 150ms) in the scheduler layer, or use SND_NOSTOP which causes PlaySound to return False (not play) if a sound is already active.
- The .wav file must remain on disk for the full playback duration (~0.10–0.25s) after play() returns. If the file lives in a tempdir that is deleted immediately, behavior is undefined (Windows may play silence or crash the audio thread). Bundled-in-package files are safe since they persist for the process lifetime.
- importlib.resources.as_file() extracts a temp copy for zip/wheel installs and removes it when the context exits. The default_earcons() implementation stores the path inside the with-block (safe for file-based installs where as_file returns the real path), but on zip-based installs the temp file is immediately deleted when the context exits, leaving a stale path in _cache. Fix: for zip installs, copy the extracted file to a stable per-process tempdir (e.g. via atexit-registered tempfile.mkdtemp()) rather than relying on the as_file temp path surviving outside the context.
- Python's wave module writes a standard 44-byte RIFF/WAVE PCM header with no LIST chunk or metadata. This is fully compatible with winsound. However, some third-party audio editors add non-standard chunks (e.g. 'id3 ', 'smpl') when re-saving — if users replace the bundled .wav files with edited versions, winsound may reject them. Validate AudioFormat==1 at load time in default_earcons().
- The chirp wave_type uses instantaneous frequency (f_inst * t) rather than phase accumulation. This introduces a small phase discontinuity that may be audible as a click, especially at high frequencies. For clean chirps use accumulated phase: phase += 2*pi*f_inst/sample_rate per sample, then sin(phase).
- Ship .wav files in the repository (not generate-at-build) — this is the correct recommendation. Rationale: the 6 files total ~90KB, are deterministic, and require zero build-time tooling. generate_earcons.py serves as a reproducibility audit script. pyproject.toml must declare [tool.setuptools.package-data] sonari = ['earcons/*.wav'] or the .wav files will be absent from the wheel.

---

## Windows SupervisorBackend (no admin) — Task Scheduler + thin Python supervisor
**pip/import packages:** winreg (stdlib, Windows-only — inject fake module in tests on macOS/Linux), subprocess (stdlib), shutil (stdlib), tempfile (stdlib), xml.etree.ElementTree (stdlib — use in tests to parse generated XML, not string-contains checks)

### Zero-admin Task Scheduler autostart via hand-authored XML — task_install(), task_uninstall(), task_is_installed()
_Source: https://learn.microsoft.com/en-us/windows/win32/taskschd/task-scheduler-schema — Task element, version 1.2, namespace http://schemas.microsoft.com/windows/2004/02/mit/task; schtasks /? on Windows 10+_

```python
import os
import subprocess
import tempfile
from pathlib import Path

TASK_NAME = "Sonari.Speechd"

# UTF-16 LE with BOM is required by schtasks /xml on older Windows builds.
# Python's encoding='utf-16' produces exactly that.
TASK_XML_TEMPLATE = '''<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2"
  xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>{user_id}</Author>
    <Description>Sonari speech daemon supervisor (autostart on logon)</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user_id}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <Hidden>true</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>PT5M</Interval>
      <Count>5</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{pythonw}</Command>
      <Arguments>"{supervisor_py}"</Arguments>
      <WorkingDirectory>{work_dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
'''


def _current_user_id() -> str:
    """Return DOMAIN\\user or COMPUTERNAME\\user for LogonTrigger/UserId."""
    import ctypes
    buf = ctypes.create_unicode_buffer(256)
    size = ctypes.c_ulong(256)
    ctypes.windll.secur32.GetUserNameExW(2, buf, ctypes.byref(size))  # 2 = NameSamCompatible
    return buf.value


def task_install(pythonw: str, supervisor_py: str) -> int:
    """Register the Task Scheduler task. Returns schtasks exit code (0 = success)."""
    user_id = _current_user_id()
    xml_content = TASK_XML_TEMPLATE.format(
        user_id=user_id,
        pythonw=pythonw,
        supervisor_py=supervisor_py,
        work_dir=str(Path(supervisor_py).parent),
    )
    # Write UTF-16 LE with BOM — required by schtasks /xml
    with tempfile.NamedTemporaryFile(
            mode='w', suffix='.xml', encoding='utf-16',
            delete=False) as fh:
        fh.write(xml_content)
        tmp = fh.name
    try:
        return subprocess.call(
            ["schtasks", "/create", "/tn", TASK_NAME, "/xml", tmp, "/f"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    finally:
        os.unlink(tmp)


def task_uninstall() -> int:
    """Delete the task. /f suppresses confirmation prompt."""
    return subprocess.call(
        ["schtasks", "/delete", "/tn", TASK_NAME, "/f"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )


def task_is_installed() -> bool:
    """Return True if the task exists (schtasks /query exit 0 = found)."""
    return subprocess.call(
        ["schtasks", "/query", "/tn", TASK_NAME],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    ) == 0

# KEY GOTCHA: RestartOnFailure is NOT expressible via schtasks CLI flags — XML only.
# The Task Scheduler's RestartOnFailure only restarts the *supervisor* process if
# it crashes (unlikely). The supervisor_loop below is the real daemon restarter.
```

**Gotchas:** 1) schtasks /xml requires UTF-16 LE with BOM — Python encoding='utf-16' produces exactly that; UTF-8 causes 'The task XML is malformed' on older builds. 2) Non-admin registration is permitted ONLY when LogonTrigger/UserId matches the registering user's own SamCompatible name AND RunLevel=LeastPrivilege. Elevated RunLevel requires admin. 3) RestartOnFailure has no schtasks CLI equivalent — XML is the only way. 4) /f flag on /create overwrites a pre-existing task silently; always use it. 5) LogonType=InteractiveToken is mandatory for SAPI TTS to reach the audio device in the GUI session; using S4U or Service breaks TTS. 6) schtasks exit codes on non-English Windows locales can be unreliable — always probe with /query separately.

### Thin Python supervisor loop — launch_spec() returning (argv, kwargs) with Windows process-creation flags; exponential backoff daemon restarter
_Source: https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags — CREATE_NO_WINDOW (0x08000000), DETACHED_PROCESS (0x00000008); https://docs.python.org/3/library/subprocess.html#subprocess.Popen — creationflags, start_new_session conflict documented in CPython source Modules/_posixsubprocess.c and Lib/subprocess.py_

```python
# src/sonari/platform/windows/supervisor_loop.py
import subprocess
import time

# These constants are defined in subprocess only on win32.
# Use hex literals so this file imports cleanly on macOS/Linux.
_CREATE_NO_WINDOW = 0x08000000
_DETACHED_PROCESS = 0x00000008
_SPAWN_FLAGS      = _CREATE_NO_WINDOW | _DETACHED_PROCESS  # 0x08000008

# Never combine start_new_session=True with DETACHED_PROCESS:
# Python 3.9+ raises ValueError on Windows if both are set.


def launch_spec(pythonw: str) -> tuple:
    """Return (argv, spawn_kwargs) compatible with subprocess.Popen(**kwargs).

    argv drives both the supervisor loop and is returned from
    WinSupervisorBackend.launch_spec() for the lazy-start path.
    """
    argv = [pythonw, "-m", "sonari.daemon"]
    kwargs = dict(
        creationflags=_SPAWN_FLAGS,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        # start_new_session intentionally absent — incompatible with DETACHED_PROCESS
    )
    return argv, kwargs


def run_supervisor_loop(pythonw: str) -> None:
    """Restart sonari.daemon indefinitely with exponential back-off.

    Back-off resets to base when the daemon ran for >= 300 s (healthy restart).
    Sequence (seconds): 2, 4, 8, 16, 32, 64, 120, 120, 120 ...
    """
    BASE, CAP, HEALTHY_UPTIME = 2, 120, 300
    attempt = 0
    while True:
        argv, kwargs = launch_spec(pythonw)
        t_start = time.monotonic()
        proc = subprocess.Popen(argv, **kwargs)
        proc.wait()  # blocks until daemon exits
        elapsed = time.monotonic() - t_start
        if elapsed >= HEALTHY_UPTIME:
            attempt = 0          # reset debt after a healthy run
        else:
            attempt += 1
        delay = min(BASE * (2 ** (attempt - 1)), CAP)
        time.sleep(delay)


# Entry point when Task Scheduler launches this file directly:
# schtasks Action: pythonw.exe "<path>/supervisor_loop.py"
if __name__ == "__main__":
    from sonari.platform.windows.supervisor import WinSupervisorBackend
    pw = WinSupervisorBackend().resolve_python()
    if pw:
        run_supervisor_loop(pw)
```

**Gotchas:** 1) subprocess.CREATE_NO_WINDOW and subprocess.DETACHED_PROCESS are not defined on non-win32 platforms — always use hex literals 0x08000000 / 0x00000008 in cross-platform code. 2) start_new_session=True + DETACHED_PROCESS raises ValueError on Python 3.9+ Windows — do NOT copy the macOS pattern. 3) pythonw.exe suppresses the console window; python.exe would flash a console on each daemon start. 4) proc.wait() in the supervisor is blocking — the supervisor itself is daemonised by Task Scheduler, so this is correct. 5) Back-off sequence: attempt=0→delay=min(2*2^(-1),120) which is invalid; start attempt at 1 after first crash or use max(attempt-1,0). Shown above correctly increments before computing delay.

### Windows Python resolution — py -3 launcher, PATH probe, Microsoft Store stub detection, pythonw.exe sibling finder
_Source: https://docs.python.org/3/using/windows.html#python-launcher-for-windows — py.exe launcher at C:\Windows\py.exe; HKCU/HKLM registry enumeration; exit code 9009 is the Windows store stub sentinel documented at https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/desktop-to-uwp-extensions#prevent-python-stub-launching_

```python
import os
import shutil
import subprocess


def _is_store_stub(path: str) -> bool:
    """Return True if *path* is the Windows Store Python stub.

    Fast path: WindowsApps in the normalised path.
    Slow path: run it and check for exit code 9009 (store stub sentinel) or
    empty stdout (the stub prints nothing and exits non-zero).
    """
    if "WindowsApps" in os.path.normcase(path):
        return True
    try:
        result = subprocess.run(
            [path, "-c", "import sys; print(sys.executable)"],
            capture_output=True, text=True, timeout=5,
        )
        return result.returncode == 9009 or not result.stdout.strip()
    except Exception:
        return True   # treat anything broken as a stub


def _find_pythonw(python_real: str) -> "str | None":
    """Return the pythonw.exe sibling of *python_real*, or None."""
    d = os.path.dirname(python_real)
    for candidate in (
        os.path.join(d, "pythonw.exe"),
        os.path.join(d, "Scripts", "pythonw.exe"),   # venv layout
    ):
        if os.path.isfile(candidate):
            return candidate
    return None


def _probe_python_version(candidate: str):
    """Return (major, minor) or None."""
    try:
        out = subprocess.check_output(
            [candidate, "-c",
             "import sys; print('%d.%d' % sys.version_info[:2])"],
            stderr=subprocess.DEVNULL, text=True, timeout=5,
        ).strip()
        major, minor = out.split(".")
        return (int(major), int(minor))
    except Exception:
        return None


def _probe_version_via_launcher(py_exe: str) -> "str | None":
    """Use `py -3 -c 'print(sys.executable)'` to resolve the real interpreter."""
    try:
        real = subprocess.check_output(
            [py_exe, "-3", "-c", "import sys; print(sys.executable)"],
            stderr=subprocess.DEVNULL, text=True, timeout=5,
        ).strip()
        return real if real else None
    except Exception:
        return None


def resolve_python_windows() -> "str | None":
    """Return pythonw.exe path for the best Python 3 >= 3.9, or None.

    Resolution order:
      1. py -3 launcher (works even when python.exe is not on PATH)
      2. 'python' on PATH (skip Microsoft Store stubs)
      3. 'python3' on PATH (skip Microsoft Store stubs)
    Deduped by realpath; prefers the py-launcher result.
    """
    seen_real = set()
    candidates = []   # list of (real_python_path, source_label)

    # 1. Windows Python Launcher
    py = shutil.which("py")
    if py:
        real = _probe_version_via_launcher(py)
        if real and not _is_store_stub(real):
            candidates.append((real, "py-launcher"))

    # 2 & 3. PATH-based names
    for name in ("python", "python3"):
        found = shutil.which(name)
        if found and not _is_store_stub(found):
            try:
                real = subprocess.check_output(
                    [found, "-c", "import sys; print(sys.executable)"],
                    stderr=subprocess.DEVNULL, text=True, timeout=5,
                ).strip()
            except Exception:
                continue
            if real:
                candidates.append((real, name))

    for real, _src in candidates:
        norm = os.path.normcase(os.path.realpath(real))
        if norm in seen_real:
            continue
        seen_real.add(norm)
        ver = _probe_python_version(real)
        if ver and ver >= (3, 9):
            pw = _find_pythonw(real)
            if pw:
                return pw

    return None
```

**Gotchas:** 1) The Microsoft Store stub at %LOCALAPPDATA%\Microsoft\WindowsApps\python.exe returns exit code 9009 in non-interactive invocations — never use it. 2) py.exe lives at C:\Windows\py.exe (system-wide) and is NOT on PATH by default on all setups — shutil.which('py') is the right probe. 3) A venv's python.exe is at Scripts\python.exe; its pythonw.exe sibling is at Scripts\pythonw.exe — both locations checked by _find_pythonw. 4) sys.executable from a py-launcher-resolved interpreter gives the full real path including version-specific directory; use that, not the py.exe path itself. 5) _probe_python_version runs the interpreter twice (once via launcher, once directly) — acceptable because this runs only at install time.

### exec-form hooks.json for Windows (no bash shim) + .gitattributes LF enforcement for hook .py files
_Source: Claude Code hooks documentation: https://docs.anthropic.com/en/docs/claude-code/hooks — exec-form with command + args array; JSON backslash escaping per RFC 8259 §7_

```python
# hooks/hooks.json — Windows exec-form (no shell interpreter needed)
# The resolved pythonw.exe path is baked in at install time by WinSupervisorBackend.install().
# Claude Code supports separate 'command' + 'args' (exec-form) — no bash shim required.

HOOKS_JSON_TEMPLATE = '''{{
  "hooks": {{
    "MessageDisplay": [
      {{
        "matcher": "",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "MessageDisplay"
            ]
          }}
        ]
      }}
    ],
    "Stop": [
      {{
        "matcher": "",
        "hooks": [
          {{
            "type": "command",
            "command": "{pythonw}",
            "args": [
              "{hook_py}",
              "Stop"
            ]
          }}
        ]
      }}
    ]
  }}
}}'''


def build_hooks_json(pythonw: str, hook_py: str) -> str:
    """Return hooks.json content with backslashes doubled for JSON."""
    return HOOKS_JSON_TEMPLATE.format(
        pythonw=pythonw.replace("\\", "\\\\"),
        hook_py=hook_py.replace("\\", "\\\\"),
    )


# .gitattributes entry — prevents CRLF injection on Windows checkout.
# Add this line to the repo root .gitattributes:
GITATTRIBUTES_LINE = "hooks/*.py  text eol=lf\n"

# Example rendered hooks.json (Windows paths):
# {
#   "hooks": {
#     "MessageDisplay": [{
#       "matcher": "",
#       "hooks": [{
#         "type": "command",
#         "command": "C:\\Users\\nima\\.sonari\\pythonw.exe",
#         "args": ["C:\\...\\sonari\\hooks\\hook.py", "MessageDisplay"]
#       }]
#     }]
#   }
# }
```

**Gotchas:** 1) JSON requires backslashes doubled: C:\\Users\\... not C:\Users\... — build_hooks_json() handles this. 2) The macOS hooks.json uses '${CLAUDE_PLUGIN_ROOT}/bin/sonari-hook MessageDisplay' (shell-form, single string); Windows must use exec-form (command + args) because there is no bash. 3) hooks/*.py text eol=lf in .gitattributes must be committed BEFORE the .py files are checked in on Windows; git will not retroactively fix line endings unless the files are re-staged. 4) At install time, write the rendered hooks.json to the user's Claude Code config directory, not the repo root — the resolved pythonw.exe path is user-specific.

### WinSupervisorBackend class — is_installed(), is_running(), doctor_rows() with neural voice probe via winreg
_Source: winreg stdlib: https://docs.python.org/3/library/winreg.html; HKLM\SOFTWARE\Microsoft\Speech_OneCore\Voices\Tokens documented at https://learn.microsoft.com/en-us/windows/win32/api/sapi/nn-sapi-ispvoice (OneCore voices); SupervisorBackend ABC: /Users/Nima.Hakimi/Projects/private/claude-tts/src/sonari/platform/base.py_

```python
# src/sonari/platform/windows/supervisor.py
from __future__ import annotations

import os
import shutil
import subprocess
from typing import Optional

from sonari.platform.base import SupervisorBackend

TASK_NAME = "Sonari.Speechd"
_CREATE_NO_WINDOW = 0x08000000
_DETACHED_PROCESS = 0x00000008
_SPAWN_FLAGS = _CREATE_NO_WINDOW | _DETACHED_PROCESS


class WinSupervisorBackend(SupervisorBackend):

    # --- monkeypatchable thin wrappers ---

    def _schtasks(self, args: list) -> int:
        """Run 'schtasks <args>'. Monkeypatched in tests."""
        return subprocess.call(
            ["schtasks"] + args,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )

    def _probe_python_version(self, candidate: str):
        """Return (major, minor) or None. Monkeypatched in tests."""
        try:
            out = subprocess.check_output(
                [candidate, "-c",
                 "import sys; print('%d.%d' % sys.version_info[:2])"],
                stderr=subprocess.DEVNULL, text=True, timeout=5,
            ).strip()
            major, minor = out.split(".")
            return (int(major), int(minor))
        except Exception:
            return None

    def _list_neural_voices(self) -> list:
        """Return list of neural voice token names. Monkeypatched in tests.

        Registry path: HKLM\\SOFTWARE\\Microsoft\\Speech_OneCore\\Voices\\Tokens
        NOT the legacy Speech\\Voices\\Tokens key (Narrator/OneCore voices only).
        """
        import winreg
        key_path = r"SOFTWARE\Microsoft\Speech_OneCore\Voices\Tokens"
        voices = []
        try:
            key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, key_path)
            i = 0
            while True:
                try:
                    voices.append(winreg.EnumKey(key, i))
                    i += 1
                except OSError:
                    break
        except OSError:
            pass
        return voices

    # --- SupervisorBackend ABC ---

    def is_installed(self) -> bool:
        """Return True if the Task Scheduler task exists."""
        return self._schtasks(["/query", "/tn", TASK_NAME]) == 0

    def is_running(self) -> bool:
        """Return True if the daemon socket is accepting connections."""
        from sonari import paths
        return paths.socket_connectable()

    def resolve_python(self) -> Optional[str]:
        """Return pythonw.exe for the best Python >= 3.9, or None."""
        from sonari.platform.windows.supervisor import resolve_python_windows
        return resolve_python_windows()

    def launch_spec(self) -> tuple:
        """Return (argv, spawn_kwargs) for lazy daemon start."""
        pw = self.resolve_python() or "pythonw.exe"
        argv = [pw, "-m", "sonari.daemon"]
        kwargs = dict(
            creationflags=_SPAWN_FLAGS,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return argv, kwargs

    def doctor_rows(self) -> list:
        """Return Windows-specific [(name, ok, detail), ...] rows."""
        rows = []

        # schtasks availability
        schtasks = shutil.which("schtasks")
        rows.append(("schtasks", schtasks is not None,
                     schtasks or "not found (unexpected on Windows)"))

        # Task registered
        task_ok = self.is_installed()
        rows.append(("Task Scheduler task", task_ok,
                     TASK_NAME if task_ok
                     else "not registered (run 'sonari install')"))

        # pythonw.exe
        pw = self.resolve_python()
        rows.append(("pythonw.exe", pw is not None,
                     pw or "no Python >= 3.9 found; install from python.org"))

        # Neural voices (Speech_OneCore)
        try:
            voices = self._list_neural_voices()
            ok = bool(voices)
            detail = voices[0] if ok else (
                "none; install from Settings > Time & language > Speech")
            rows.append(("neural voice", ok, detail))
        except Exception as exc:
            rows.append(("neural voice", False, "error: {0}".format(exc)))

        # Daemon running
        running = self.is_running()
        rows.append(("daemon running", running,
                     "accepting connections" if running
                     else "not running (run 'sonari start')"))

        return rows

    def install(self, python: str, app_dir: str) -> None:
        from sonari.platform.windows.supervisor import task_install
        supervisor_py = os.path.join(app_dir, "sonari", "platform",
                                     "windows", "supervisor_loop.py")
        task_install(python, supervisor_py)

    def uninstall(self) -> None:
        from sonari.platform.windows.supervisor import task_uninstall
        task_uninstall()
```

**Gotchas:** 1) winreg is Windows-only stdlib — never import at module level; always inside a method so the module imports cleanly on macOS/Linux. 2) Neural voices are under Speech_OneCore\Voices\Tokens, NOT the legacy Speech\Voices\Tokens (which only has Narrator/SAPI 5 voices and is empty on fresh Windows 11). 3) _schtasks() wraps subprocess.call with DEVNULL — monkeypatch at the instance level in tests so no real schtasks is invoked. 4) launch_spec() must NOT include start_new_session=True — see pattern 2 gotchas. 5) doctor_rows() must never raise — wrap every external call in try/except exactly as MacSupervisorBackend does.

**Mock strategy:**

Inject a fake winreg module before importing the Windows backend, then monkeypatch instance methods for all external calls. Full test file target: tests/test_win_supervisor.py

```python
# tests/test_win_supervisor.py
import sys
import types
import xml.etree.ElementTree as ET

# --- winreg injection (must happen before any import of the windows backend) ---
if sys.platform != \"win32\":
    _fake_winreg = types.ModuleType(\"winreg\")
    _fake_winreg.HKEY_LOCAL_MACHINE = 0x80000002
    _fake_winreg.OpenKey = lambda *a, **kw: None
    _fake_winreg.EnumKey = lambda *a, **kw: (_ for _ in ()).throw(OSError())
    _fake_winreg.QueryValueEx = lambda *a, **kw: (_ for _ in ()).throw(OSError())
    sys.modules.setdefault(\"winreg\", _fake_winreg)

from sonari.platform.windows.supervisor import (
    WinSupervisorBackend, TASK_NAME, TASK_XML_TEMPLATE, _SPAWN_FLAGS,
)

_NS = \"http://schemas.microsoft.com/windows/2004/02/mit/task\"


def test_task_xml_logon_trigger_user_id():
    xml_str = TASK_XML_TEMPLATE.format(
        user_id=\"DESKTOP-ABC\\\\nima\",
        pythonw=r\"C:\\Python311\\pythonw.exe\",
        supervisor_py=r\"C:\\sonari\\supervisor_loop.py\",
        work_dir=r\"C:\\sonari\",
    )
    root = ET.fromstring(xml_str)
    uid_el = root.find(f\".//{{{_NS}}}LogonTrigger/{{{_NS}}}UserId\")
    assert uid_el is not None and uid_el.text == \"DESKTOP-ABC\\\\nima\"


def test_task_xml_restart_on_failure_present():
    xml_str = TASK_XML_TEMPLATE.format(
        user_id=\"DESKTOP\\\\u\", pythonw=\"pw.exe\",
        supervisor_py=\"s.py\", work_dir=\".\",
    )
    root = ET.fromstring(xml_str)
    rof = root.find(f\".//{{{_NS}}}RestartOnFailure\")
    assert rof is not None
    interval = rof.find(f\"{{{_NS}}}Interval\")
    assert interval.text == \"PT5M\"


def test_task_xml_run_level_least_privilege():
    xml_str = TASK_XML_TEMPLATE.format(
        user_id=\"U\", pythonw=\"pw.exe\", supervisor_py=\"s.py\", work_dir=\".\",
    )
    root = ET.fromstring(xml_str)
    rl = root.find(f\".//{{{_NS}}}Principal/{{{_NS}}}RunLevel\")
    assert rl.text == \"LeastPrivilege\"


def test_launch_spec_creationflags(monkeypatch):
    sup = WinSupervisorBackend()
    monkeypatch.setattr(sup, \"resolve_python\", lambda: r\"C:\\Python311\\pythonw.exe\")
    argv, kwargs = sup.launch_spec()
    assert argv[0].endswith(\"pythonw.exe\")
    assert argv[-1] == \"sonari.daemon\"
    flags = kwargs[\"creationflags\"]
    assert flags & 0x08000000, \"CREATE_NO_WINDOW must be set\"
    assert flags & 0x00000008, \"DETACHED_PROCESS must be set\"
    assert not kwargs.get(\"start_new_session\", False), \"must NOT combine with DETACHED_PROCESS\"


def test_is_installed_calls_schtasks_query(monkeypatch):
    sup = WinSupervisorBackend()
    calls = []
    monkeypatch.setattr(sup, \"_schtasks\", lambda args: calls.append(args) or 0)
    assert sup.is_installed() is True
    assert calls[0] == [\"/query\", \"/tn\", TASK_NAME]


def test_doctor_rows_include_task_and_neural_voice(monkeypatch):
    sup = WinSupervisorBackend()
    monkeypatch.setattr(sup, \"_schtasks\", lambda args: 0)
    monkeypatch.setattr(sup, \"resolve_python\", lambda: r\"C:\\Python311\\pythonw.exe\")
    monkeypatch.setattr(sup, \"_list_neural_voices\", lambda: [\"Microsoft Aria Online\"])
    monkeypatch.setattr(\"sonari.paths.socket_connectable\", lambda: True)
    names = [r[0] for r in sup.doctor_rows()]
    assert \"Task Scheduler task\" in names
    assert \"neural voice\" in names


def test_resolve_python_skips_store_stub(monkeypatch, tmp_path):
    # Verify _is_store_stub fast-path (WindowsApps in path)
    from sonari.platform.windows.supervisor import _is_store_stub
    stub = str(tmp_path / \"WindowsApps\" / \"python.exe\")
    assert _is_store_stub(stub) is True


def test_spawn_flags_value():
    # Hex literal correctness — no subprocess import needed
    assert _SPAWN_FLAGS == 0x08000008
```

The sys.modules.setdefault call is idempotent — running on real Windows leaves the genuine winreg intact. All subprocess-touching methods (_schtasks, _probe_python_version, _list_neural_voices) are patched at the instance level so no OS calls escape the test. XML structure is validated via ElementTree.fromstring() with the full namespace string, which is more robust than string-contains checks.

**Open risks (mock-blind — for the acceptance checklist):**
- UTF-16 LE encoding: schtasks /xml silently rejects UTF-8 on Windows builds before 22H2 — always write the temp XML file with encoding='utf-16'; Python's codecs emit the correct BOM automatically.
- RestartOnFailure Interval minimum: Task Scheduler enforces a minimum of PT1M (1 minute); values below that are silently clamped. PT5M is safe.
- Non-admin registration scope: a standard user can only register a LogonTrigger task for their own UserId + LeastPrivilege. Attempting to set RunLevel=HighestAvailable without elevation raises E_ACCESSDENIED from the COM Task Scheduler API (schtasks surfaces this as exit code 1).
- venv / sys.executable mismatch: if Sonari is installed inside a venv, resolve_python_windows() must return the venv's pythonw.exe (Scripts\pythonw.exe), not the base interpreter — _find_pythonw() checks both locations.
- DEVNULL on stdin: subprocess.DEVNULL must be passed explicitly for stdin when using DETACHED_PROCESS; without it the child inherits the parent's stdin handle, which can cause hangs if the parent's stdin is a pipe.
- schtasks exit codes on non-English Windows: schtasks /query returns 0 for 'found' and 1 for 'not found' in en-US; on some locale builds the exit code is reliable but the stderr message is localised — always use exit code, never parse stderr text.
- SAPI TTS + DETACHED_PROCESS: the speech daemon must call CoInitializeEx(COINIT_APARTMENTTHREADED) before any SAPI calls; DETACHED_PROCESS does not affect COM apartment threading, but if the daemon is started without a message loop, some SAPI voices (especially neural) may hang on Speak() — ensure the daemon runs a STA message pump or uses SpVoice with SVSFlagsAsync=0.
