"""Chatterbox TTS engine (optional, opt-in, GPU-only): registry, VRAM gate, client.

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
import json
import os
import subprocess
import threading
from pathlib import Path

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
    """["cb_default"] plus every registered voice-clip stem, sorted.

    `cb_default` is reserved for the built-in voice; a user clip that happens
    to share that stem is de-duplicated out rather than listed twice.
    """
    return [DEFAULT_VOICE] + [stem for stem in _clip_stems() if stem != DEFAULT_VOICE]


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
    norm = normalize_voice(name)
    if norm is None or norm.lower() == DEFAULT_VOICE:
        return {"voice_path": None, "variant": default_variant, "exaggeration": None}

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
    return {"voice_path": str(voice_path), "variant": variant, "exaggeration": exaggeration}


# --- VRAM gate -----------------------------------------------------------------

def _default_smi_run(argv, **kwargs):
    """subprocess.check_output, but windowless on Windows (no console flash)."""
    if os.name == "nt":
        kwargs["creationflags"] = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    return subprocess.check_output(argv, **kwargs)


def free_vram_gb(run=_default_smi_run) -> "float | None":
    """Free GPU VRAM in GiB via nvidia-smi. None if it cannot be determined."""
    try:
        out = run(
            ["nvidia-smi", "--query-gpu=memory.free", "--format=csv,noheader,nounits"],
            text=True,
        )
    except Exception:  # noqa: BLE001 - no nvidia-smi / no GPU / anything else -> unknown
        return None
    try:
        first_line = out.strip().splitlines()[0]
        return float(first_line.strip()) / 1024.0
    except (IndexError, ValueError):
        return None


def gate_ok(config, run=_default_smi_run) -> bool:
    """Safe to try Chatterbox: threshold<=0, VRAM unknown, or free >= threshold."""
    threshold = config.get("chatterbox_min_free_vram_gb", 5)
    if threshold is None or threshold <= 0:
        return True
    free = free_vram_gb(run=run)
    if free is None:
        return True
    return free >= threshold


# --- fallback notice -----------------------------------------------------------

# Once-per-read event: Task 4's routing sets this when a Chatterbox request
# fails or is gated off and it silently falls back to the native/Kokoro engine.
_FALLBACK: "list[str]" = []


def _set_fallback_notice(reason) -> None:
    _FALLBACK[:] = [reason]


def pop_fallback_notice() -> "str | None":
    """The pending fallback reason, or None. Clears it (once-per-read)."""
    return _FALLBACK.pop() if _FALLBACK else None


# --- client --------------------------------------------------------------------

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
            self._ensure_proc(config)
            resp = self._try_request(payload, timeout)
            if resp is not None:
                return resp
            # Dead pipe: kill, respawn once, retry.
            self._kill()
            self._spawn(config)
            resp = self._try_request(payload, timeout)
            if resp is None:
                self._kill()
                raise ChatterboxError("chatterbox worker is unavailable")
            return resp

    def synth_wav(self, text, name, config) -> bytes:
        """Synthesize *text* with voice *name*. Raises ChatterboxError on failure.

        Gating is the caller's job (`gate_ok`) - this always tries the worker.
        """
        spec = voice_spec(name, config)
        payload = {
            "type": "synth",
            "text": text,
            "voice_path": spec["voice_path"],
            "variant": spec["variant"],
            "exaggeration": spec["exaggeration"],
        }
        timeout = config.get("chatterbox_timeout", 120)
        resp = self._request(payload, timeout, config)
        if not resp.get("ok"):
            raise ChatterboxError(resp.get("error", "unknown chatterbox error"))
        return base64.b64decode(resp["wav_b64"])


CLIENT = ChatterboxClient()
