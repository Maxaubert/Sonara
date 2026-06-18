"""macOS TTS backend — wraps the `say` command, with optional Kokoro neural
voices routed through the portable engine and played via ``afplay``.

A Kokoro voice (af_heart, af_nicole, ...) is synthesized by the shared
``sonari.kokoro`` engine to WAV bytes and played by spawning ``afplay`` on a
temp file; everything else goes to the native ``say`` command. Both paths return
a proc-like handle (.wait/.terminate/.poll/.returncode) so the Speaker's
cancel/interrupt, earcon mixing, and temp-file cleanup behave identically. This
mirrors the Windows backend post-#42 (issue #41) — additive, no Windows change.

NOTE: ``_TMP_PREFIX`` / ``_sweep_stale_wavs`` deliberately duplicate the Windows
backend's (windows/tts.py) rather than cross-importing a sibling platform module;
the prefix is byte-identical so either backend's startup sweep reclaims the
other's crash-leaked WAVs. A shared helper would touch the Windows file (out of
scope for this macOS-only change).
"""
from __future__ import annotations

import os
import subprocess
import tempfile
from typing import List, Optional, Tuple

from sonari.platform.base import TtsBackend

_TMP_PREFIX = "sonari-tts-"


def _sweep_stale_wavs(max_age_s: float = 300.0) -> None:
    """Best-effort cleanup of temp WAVs leaked by a prior crashed/killed daemon.
    Only removes files older than *max_age_s* (so a clip another instance may
    still be playing is never deleted) and only our own ``sonari-tts-*`` prefix
    is touched. Never raises. (#26 parity)"""
    import glob
    import time
    try:
        now = time.time()
        pattern = os.path.join(tempfile.gettempdir(), _TMP_PREFIX + "*.wav")
        for p in glob.glob(pattern):
            try:
                if now - os.path.getmtime(p) > max_age_s:
                    os.unlink(p)
            except OSError:
                pass
    except Exception:
        pass


class _AfplayHandle:
    """Subprocess-like handle for an in-flight ``afplay`` utterance.

    Delegates .wait()/.terminate()/.poll()/.returncode to the afplay child and
    unlinks the temp WAV exactly once, when playback ends (normal completion,
    an observed exit via poll(), or terminate()). Unlike the Windows handle there
    is no completion timer — afplay is a real process, so wait() blocks on it
    directly (winsound's async playback is what forced the timer there).

    returncode delegates to the child: 0 == completed; a terminate() leaves the
    real signal code (≈ -15), which the Speaker reads as "not completed" (replay).
    """

    def __init__(self, proc, wav_path: str) -> None:
        self._proc = proc
        self._path = wav_path
        self._cleaned = False

    def _cleanup(self) -> None:
        if self._cleaned:
            return
        self._cleaned = True
        try:
            os.unlink(self._path)
        except OSError:
            pass

    @property
    def returncode(self) -> Optional[int]:
        return self._proc.returncode

    def wait(self, timeout: Optional[float] = None) -> int:
        # Propagates subprocess.TimeoutExpired (the Speaker catches exactly that);
        # cleanup must NOT run on a timeout — afplay is still reading the file.
        rc = self._proc.wait(timeout=timeout)
        self._cleanup()
        return rc

    def poll(self) -> Optional[int]:
        rc = self._proc.poll()
        if rc is not None:
            self._cleanup()
        return rc

    def terminate(self) -> None:
        # The Speaker's synth-gap cancel calls terminate() then returns WITHOUT a
        # later wait(), so terminate() must reap the child and clean up itself.
        try:
            self._proc.terminate()
        except Exception:
            pass
        try:
            # Bounded reap so a wedged afplay can't hang cancel; unlinking an open
            # file is safe on Unix, so we clean up even if the reap times out.
            self._proc.wait(timeout=2.0)
        except Exception:
            pass
        self._cleanup()


def _play_wav_bytes(data: bytes):
    """Write WAV *data* to a temp file, spawn ``afplay`` on it, and return an
    _AfplayHandle. If afplay can't be spawned, unlink the temp WAV before
    re-raising so a failed utterance never leaks one (parity with the Windows
    _play_wav_bytes / the #26 sweep). Never returns None — the Speaker
    dereferences the handle."""
    fd, path = tempfile.mkstemp(suffix=".wav", prefix=_TMP_PREFIX)
    try:
        try:
            os.write(fd, data)
        finally:
            os.close(fd)
        proc = subprocess.Popen(["afplay", path])
    except Exception:
        # Any failure (write or spawn) — unlink before re-raising so a failed
        # utterance never leaks a temp WAV (the #26 sweep is only the crash backstop).
        try:
            os.unlink(path)
        except OSError:
            pass
        raise
    return _AfplayHandle(proc, path)


def _parse_listing(listing: str) -> "List[Tuple[str, str, bool]]":
    """Parse ``say -v ?`` output into (bare_name, locale, is_premium) triples.

    Each output line has the form::

        Voice Name (Enhanced)   en_US  # some sample text

    The hash and everything after it is a sample phrase that we discard.
    We split the portion before the hash into tokens; the last token is the
    locale code, everything preceding it is the voice name (possibly
    including a ``(Premium)`` / ``(Enhanced)`` qualifier).
    """
    results: "List[Tuple[str, str, bool]]" = []
    for line in listing.splitlines():
        line = line.rstrip()
        if not line:
            continue
        before_hash = line.split("#", 1)[0].rstrip()
        parts = before_hash.split()
        if len(parts) < 2:
            continue
        locale = parts[-1]
        name = " ".join(parts[:-1])
        is_premium = "(Premium)" in name or "(Enhanced)" in name
        bare = name.replace("(Premium)", "").replace("(Enhanced)", "").strip()
        results.append((bare, locale, is_premium))
    return results


class MacTtsBackend(TtsBackend):
    def __init__(self) -> None:
        self._kokoro = None        # lazy KokoroEngine (only built if a Kokoro voice is used)
        _sweep_stale_wavs()        # clear temp WAVs leaked by a prior crash (#26)

    def _get_kokoro(self):
        """Lazy KokoroEngine — downloads the ~316 MB model on first Kokoro voice."""
        if self._kokoro is None:
            from sonari import kokoro, paths
            self._kokoro = kokoro.KokoroEngine(paths.SONARI_DIR / "kokoro")
        return self._kokoro

    def run(self, text: str, voice: Optional[str], rate: int):
        """Speak *text*, returning a proc-like handle the Speaker orchestrates.

        A Kokoro voice (af_heart, ...) is synthesized by the portable engine and
        played via afplay; anything else goes to the native `say` command. A
        Kokoro voice without the [kokoro] extra raises an actionable RuntimeError
        instead of `say` silently falling back to the default voice — the silent
        no-op an eyes-free user would never notice (issue #41)."""
        from sonari import kokoro
        if kokoro.is_kokoro_voice(voice):
            kokoro.require_installed()   # actionable error instead of a silent say fallback
            data = self._get_kokoro().wav_bytes(
                text, voice, kokoro.rate_to_speed(rate))
            return _play_wav_bytes(data)
        cmd = ["say"]
        if voice:
            cmd += ["-v", voice]
        cmd += ["-r", str(rate), text]
        return subprocess.Popen(cmd)

    def list_voices(self) -> "List[str]":
        """Return all selectable voice names: the installed `say` voices (bare,
        without qualifier tags) PLUS the 28 Kokoro neural voices, but only when
        the optional [kokoro] extra is installed — else we'd advertise voices
        whose first speak silently fails (parity with the Windows gate, #42)."""
        from sonari import kokoro
        try:
            listing = subprocess.check_output(["say", "-v", "?"], text=True)
            native = [bare for bare, _locale, _premium in _parse_listing(listing)]
        except (FileNotFoundError, OSError, subprocess.SubprocessError):
            native = []
        kokoro_voices = list(kokoro.VOICES) if kokoro.is_installed() else []
        return native + kokoro_voices

    def best_voice(self) -> str:
        """Return the best English voice: Premium/Enhanced first, then
        Allison > Samantha as plain-English fallbacks, then ``"Samantha"``
        hard-coded as the last resort."""
        fallback = "Samantha"
        try:
            listing = subprocess.check_output(["say", "-v", "?"], text=True)
        except (FileNotFoundError, OSError, subprocess.SubprocessError):
            return fallback
        premium_en: "List[str]" = []
        plain_en: "List[str]" = []
        for bare, locale, is_premium in _parse_listing(listing):
            if not locale.startswith("en"):
                continue
            (premium_en if is_premium else plain_en).append(bare)
        if premium_en:
            return premium_en[0]
        for preferred in ("Allison", "Samantha"):
            if preferred in plain_en:
                return preferred
        return fallback
