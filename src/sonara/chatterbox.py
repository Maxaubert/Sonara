"""Chatterbox TTS engine (optional, opt-in, GPU-only): registry + client.

Chatterbox voices are cloned from a short reference clip dropped into
CHATTERBOX_VOICES_DIR (`<stem>.wav`, plus an optional `<stem>.json` sidecar for
`variant`/`exaggeration` overrides). `cb_default` needs no clip at all. Selection
mirrors kokoro.py: bare stem (`calm-lady`) or engine-prefixed (`chatterbox:calm-lady`).

Synthesis itself happens out of process in chatterbox_worker.py, run inside the
opt-in `chatterbox-venv` (torch + chatterbox-tts do not belong in the daemon's own
stdlib-only environment). ChatterboxClient spawns that worker on demand and talks
newline-JSON over its stdin/stdout; CLIENT is the single, module-level instance so
the worker process (and its loaded model) persists across utterances.
"""
from __future__ import annotations

import base64
import collections
import json
import os
import subprocess
import threading
import time
from pathlib import Path

from sonara.config import DEFAULTS as _CONFIG_DEFAULTS
from sonara.paths import CHATTERBOX_HF_CACHE, CHATTERBOX_VOICES_DIR, chatterbox_venv_python

DEFAULT_VOICE = "cb_default"


class ChatterboxError(Exception):
    """Raised when the Chatterbox worker fails to synthesize or is unreachable."""


def worker_script_path() -> str:
    """Absolute path to chatterbox_worker.py, installed beside this module."""
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), "chatterbox_worker.py")


def is_provisioned() -> bool:
    """True if the Chatterbox venv's python.exe exists (opt-in extra installed)."""
    return os.path.exists(chatterbox_venv_python())


# --- voice registry ----------------------------------------------------------

def _clip_stems() -> "list[str]":
    """Sorted stems of the .wav voice clips in the registry dir ([] if missing)."""
    try:
        return sorted(p.stem for p in Path(CHATTERBOX_VOICES_DIR).glob("*.wav"))
    except OSError:
        return []


def list_voices() -> "list[str]":
    """Every registered voice-clip stem, sorted. The no-clip built-in
    (`cb_default`) is deliberately NOT advertised (#42): it clutters the
    picker next to real cloned voices, but it still resolves everywhere
    (is_chatterbox_voice / voice_spec) so an old config keeps speaking."""
    return list(_clip_stems())


def normalize_voice(name) -> "str | None":
    """Strip an optional `chatterbox:` engine prefix. None for a falsy *name*.

    Stems keep their original case (the registry is case-preserving); callers
    that need to match against it should lowercase both sides themselves.
    """
    if not name:
        return None
    s = str(name).strip()
    if ":" in s:
        engine, _, rest = s.partition(":")
        if engine.strip().lower() == "chatterbox":
            s = rest.strip()
    return s or None


def is_chatterbox_voice(name) -> bool:
    """True if *name* (bare stem, `cb_default`, or `chatterbox:`-prefixed) is ours."""
    norm = normalize_voice(name)
    if norm is None:
        return False
    if norm.lower() == DEFAULT_VOICE:
        return True
    return norm.lower() in {stem.lower() for stem in _clip_stems()}


def voice_spec(name, config) -> dict:
    """{"voice_path": str|None, "variant": str, "exaggeration": float|None} for *name*.

    `cb_default` (or an unrecognized name) has no clip: variant comes from
    config, exaggeration is None. A registered voice reads an optional JSON
    sidecar (same stem, `.json`) for variant/exaggeration overrides.
    """
    default_variant = config.get("chatterbox_variant", "turbo")
    default_exag = config.get("chatterbox_exaggeration")   # settings slider (#38)
    norm = normalize_voice(name)
    if norm is None or norm.lower() == DEFAULT_VOICE:
        return {"voice_path": None, "variant": default_variant,
                "exaggeration": default_exag}

    voices_dir = Path(CHATTERBOX_VOICES_DIR)
    voice_path = voices_dir / (norm + ".wav")
    sidecar = voices_dir / (norm + ".json")
    variant = default_variant
    exaggeration = None
    if sidecar.exists():
        try:
            data = json.loads(sidecar.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            data = {}
        if not isinstance(data, dict):
            data = {}
        variant = data.get("variant", default_variant)
        exaggeration = data.get("exaggeration")
    if exaggeration is None:
        exaggeration = default_exag        # settings slider fills the gap (#38)
    return {"voice_path": str(voice_path), "variant": variant, "exaggeration": exaggeration}


def split_text(text, max_chars=280):
    """Split *text* into speakable chunks no longer than *max_chars*, on sentence
    boundaries (a too-long sentence is hard-split on spaces). Chatterbox degrades
    on long input, so the daemon drives chunking for pipelined, interruptible
    playback. NOTE: chatterbox_worker.py keeps its own _split_text as a defensive
    net; the worker cannot import sonara, so the small pure logic is duplicated on
    purpose. Keep the two in sync."""
    import re
    text = (text or "").strip()
    if not text:
        return []
    sentences = re.findall(r"[^.!?]*[.!?]+|\S[^.!?]*$", text)
    chunks = []
    cur = ""
    for s in sentences:
        s = s.strip()
        if not s:
            continue
        if len(s) > max_chars:
            if cur:
                chunks.append(cur)
                cur = ""
            buf = ""
            for word in s.split(" "):
                if buf and len(buf) + 1 + len(word) > max_chars:
                    chunks.append(buf)
                    buf = word
                else:
                    buf = (buf + " " + word).strip()
            if buf:
                cur = buf
        elif cur and len(cur) + 1 + len(s) > max_chars:
            chunks.append(cur)
            cur = s
        else:
            cur = (cur + " " + s).strip()
    if cur:
        chunks.append(cur)
    return chunks


def chunk_chars(config) -> int:
    """Clamped synth-chunk size (#27). Capped at 280: the worker's defensive
    re-split is fixed at 280 and would silently re-split larger chunks; floor
    80 so a typo cannot degrade playback into word-sized fragments."""
    try:
        n = int(config.get("chatterbox_max_chunk_chars",
                           _CONFIG_DEFAULTS["chatterbox_max_chunk_chars"]))
    except (TypeError, ValueError):
        return _CONFIG_DEFAULTS["chatterbox_max_chunk_chars"]
    return max(80, min(280, n))




# --- fallback notice -----------------------------------------------------------

# Once-per-read event: Task 4's routing sets this when a Chatterbox request
# fails or is gated off and it silently falls back to the native/Kokoro engine.
_FALLBACK: "list[str]" = []


def _set_fallback_notice(reason) -> None:
    _FALLBACK[:] = [reason]


def pop_fallback_notice() -> "str | None":
    """The pending fallback reason, or None. Clears it (once-per-read)."""
    return _FALLBACK.pop() if _FALLBACK else None


# --- synth cache ---------------------------------------------------------------

# Bounded LRU of rendered WAVs keyed by (variant, voice_path, exaggeration, text).
# Small: a digest is a handful of chunks, and only the most recent utterances are
# ever re-read. Guarded by a lock because synth_wav runs on the pipelined producer
# thread and a re-read may start a fresh producer that overlaps the old one.
_SYNTH_CACHE: "collections.OrderedDict" = collections.OrderedDict()
_SYNTH_CACHE_LOCK = threading.Lock()
_SYNTH_CACHE_MAX = 64


def _synth_cache_get(key):
    with _SYNTH_CACHE_LOCK:
        wav = _SYNTH_CACHE.get(key)
        if wav is not None:
            _SYNTH_CACHE.move_to_end(key)
        return wav


def _synth_cache_put(key, wav) -> None:
    with _SYNTH_CACHE_LOCK:
        _SYNTH_CACHE[key] = wav
        _SYNTH_CACHE.move_to_end(key)
        while len(_SYNTH_CACHE) > _SYNTH_CACHE_MAX:
            _SYNTH_CACHE.popitem(last=False)


# --- client --------------------------------------------------------------------

# After a worker failure, refuse further requests for this long so each chunk's
# Kokoro fallback fires FAST instead of re-paying spawn+timeout per chunk. A
# success clears it immediately (audit #21).
_COOLDOWN_S = 60.0


class ChatterboxClient:
    """Owns (at most) one chatterbox_worker.py subprocess, spawned on demand.

    Windows anonymous pipes have no read timeout, so `_request` reads the
    response line on a helper thread and joins it with the request timeout.
    A dead pipe (write failure, or an empty read because the worker exited)
    triggers exactly one respawn-and-retry; a second failure raises.
    """

    def __init__(self) -> None:
        self._proc = None
        self._lock = threading.Lock()
        self._cooldown_until = 0.0   # monotonic deadline; 0 = healthy

    def _spawn(self, config) -> None:
        idle_s = config.get("chatterbox_idle_unload_s", 600)
        # -P (isolated: do not prepend the script's own dir to sys.path) plus a
        # stripped PYTHONPATH so the worker's directory (the sonara package, which
        # holds same-named modules: platform/, queue.py, chatterbox.py) can never
        # shadow the venv's stdlib/pip packages. Without this the worker imported
        # sonara.platform for `import platform`, and torch's platform.machine()
        # crashed every synth into a silent Kokoro fallback (verified live).
        argv = [chatterbox_venv_python(), "-P", worker_script_path(), str(idle_s)]
        env = dict(os.environ)
        env.pop("PYTHONPATH", None)
        env["HF_HOME"] = str(CHATTERBOX_HF_CACHE)
        kwargs = {}
        if os.name == "nt":
            kwargs["creationflags"] = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        try:
            self._proc = subprocess.Popen(
                argv,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                env=env,
                text=True,
                encoding="utf-8",
                **kwargs
            )
        except Exception as exc:  # noqa: BLE001 - venv python gone, spawn denied, ...
            # Funnel EVERY spawn failure into ChatterboxError: the per-chunk
            # Kokoro fallback catches only ChatterboxError, so a raw OSError
            # would bypass it and silently drop the utterance while reporting
            # success (audit #19).
            self._proc = None
            raise ChatterboxError(
                "failed to spawn chatterbox worker: {0}".format(exc))

    def _kill(self) -> None:
        proc, self._proc = self._proc, None
        if proc is None:
            return
        try:
            proc.kill()
        except Exception:  # noqa: BLE001 - already dead, nothing to clean up
            pass
        for stream in (proc.stdin, proc.stdout):
            try:
                stream.close()
            except Exception:  # noqa: BLE001 - already closed
                pass

    def _ensure_proc(self, config) -> None:
        if self._proc is None or self._proc.poll() is not None:
            self._spawn(config)

    def _try_request(self, payload, timeout):
        """One request/response round-trip. None on a dead pipe (write failure
        or empty read); raises ChatterboxError("timeout") on a stuck read."""
        proc = self._proc
        try:
            proc.stdin.write(json.dumps(payload) + "\n")
            proc.stdin.flush()
        except Exception:  # noqa: BLE001 - broken pipe means the worker died
            return None

        result = {}

        def _read():
            try:
                result["line"] = proc.stdout.readline()
            except Exception:  # noqa: BLE001 - closed/broken pipe
                result["line"] = ""

        reader = threading.Thread(target=_read, daemon=True)
        reader.start()
        reader.join(timeout)
        if reader.is_alive():
            self._kill()
            raise ChatterboxError("timeout")

        raw = result.get("line", "")
        if not raw:
            return None
        try:
            return json.loads(raw)
        except ValueError:
            return None

    def _request(self, payload, timeout, config) -> dict:
        with self._lock:
            # Failure memo (audit #21): while cooling down after a failure, fail
            # IMMEDIATELY so the caller's per-chunk Kokoro fallback fires fast --
            # a persistently broken worker was otherwise respawned and re-paid
            # (spawn + timeout, GPU thrash) for every chunk of every utterance.
            if time.monotonic() < self._cooldown_until:
                raise ChatterboxError("chatterbox worker cooling down after failure")
            try:
                self._ensure_proc(config)
                resp = self._try_request(payload, timeout)
                if resp is None:
                    # Dead pipe: kill, respawn once, retry.
                    self._kill()
                    self._spawn(config)
                    resp = self._try_request(payload, timeout)
                if resp is None:
                    self._kill()
                    raise ChatterboxError("chatterbox worker is unavailable")
            except ChatterboxError:
                self._cooldown_until = time.monotonic() + _COOLDOWN_S
                raise
            self._cooldown_until = 0.0   # healthy again: clear the memo
            return resp

    def warm(self, config) -> bool:
        """Load the model in the worker ahead of the first synth (best-effort), so
        the first digest does not pay the ~40s cold load. Returns True on success.
        Uses a generous timeout since a warm IS the cold load."""
        variant = config.get("chatterbox_variant", "turbo")
        timeout = config.get("chatterbox_warm_timeout", 90)
        try:
            resp = self._request({"type": "warm", "variant": variant}, timeout, config)
        except ChatterboxError:
            return False
        return bool(resp.get("ok"))

    def synth_wav(self, text, name, config) -> bytes:
        """Synthesize *text* with voice *name*. Raises ChatterboxError on failure.

        This always tries the worker; a genuine failure falls back per chunk.

        A rendered result is cached per (variant, voice, exaggeration, text): the
        summary-mode Up ('re-read the digest') speaks the exact same text, and
        Chatterbox generation is both slow (~2s/chunk) and non-deterministic (the
        intonation drifts each render), so re-reads replay byte-identical cached
        audio instead of regenerating.
        """
        spec = voice_spec(name, config)
        key = (spec["variant"], spec["voice_path"], spec["exaggeration"], text)
        cached = _synth_cache_get(key)
        if cached is not None:
            return cached
        payload = {
            "type": "synth",
            "text": text,
            "voice_path": spec["voice_path"],
            "variant": spec["variant"],
            "exaggeration": spec["exaggeration"],
        }
        # Fallback comes FROM config DEFAULTS: a second literal here drifted from
        # DEFAULTS once already (120 vs 30, audit #19) and the 30 broke post-idle
        # synthesis (cold reload ~40s > 30s timeout -> cascade to Kokoro).
        timeout = config.get("chatterbox_timeout",
                             _CONFIG_DEFAULTS["chatterbox_timeout"])
        resp = self._request(payload, timeout, config)
        if not resp.get("ok"):
            raise ChatterboxError(resp.get("error", "unknown chatterbox error"))
        wav = base64.b64decode(resp["wav_b64"])
        _synth_cache_put(key, wav)
        return wav


CLIENT = ChatterboxClient()
