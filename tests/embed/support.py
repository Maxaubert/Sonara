"""Shared parts of the embedder suite (#274): where the runtime is, the WAV
files ``sonarad --output wav:<dir>`` writes, a fake speech provider, the
Kokoro model for a temp home, running a host app, and the checks every
host's scenario report must pass.

Stdlib only (Python 3.9), like the hosts' own code.
"""
from __future__ import annotations

import json
import math
import os
import re
import shutil
import struct
import subprocess
import threading
import time
import wave
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Dict, List, Optional

REPO = Path(__file__).resolve().parents[2]
HOSTS = Path(__file__).resolve().parent / "hosts"

# The fake engine (crates/sonara-engine/src/fake.rs): a 400 Hz square wave
# of +-8000 at 16 kHz, 10 ms per character at rate 200.
FAKE_RATE = 16_000
FAKE_AMPLITUDE = 8_000
# What the fake provider answers: 24 kHz, a constant the fake engine never makes.
PROVIDER_RATE = 24_000
PROVIDER_VALUE = 3_000
PROVIDER_SAMPLES = 9_600
KOKORO_FILES = ("kokoro-v1.0.onnx", "voices-v1.0.bin")
# Said by the real voice: about 3 to 4 s of speech.
REAL_SENTENCE = "Sonara reads this sentence aloud for an app that bundles it."


def which_node() -> str:
    node = shutil.which("node")
    assert node, "node is needed"
    return node


def find_runtime() -> Optional[Path]:
    """The release sonarad.exe (``SONARAD`` wins): the build that ships."""
    env = os.environ.get("SONARAD")
    if env:
        return Path(env) if Path(env).is_file() else None
    exe = REPO / "target" / "release" / "sonarad.exe"
    return exe if exe.is_file() else None


def has_kokoro_dlls(runtime: Path) -> bool:
    return (runtime.parent / "onnxruntime.dll").is_file()


def kokoro_models() -> Optional[Path]:
    """A folder with the two Kokoro model files: ``SONARA_KOKORO_MODELS``,
    else the one the user's runtime already downloaded (only read)."""
    candidates = [os.environ.get("SONARA_KOKORO_MODELS")]
    local = os.environ.get("LOCALAPPDATA")
    if local:
        candidates.append(os.path.join(local, "Sonara", "models", "kokoro", "v1.0"))
    for c in candidates:
        if c and all((Path(c) / f).is_file() for f in KOKORO_FILES):
            return Path(c)
    return None


def seed_kokoro(home: Path, models: Path) -> None:
    """Pre-seed a home with the model, as docs/bundling.md tells an
    installer to: hard links when the volume allows (no 354 MB copy, the
    source is never written), else copies. ``verified.json`` is copied so
    the runtime need not hash the files again."""
    dest = home / "models" / "kokoro" / "v1.0"
    dest.mkdir(parents=True, exist_ok=True)
    for f in KOKORO_FILES:
        try:
            os.link(models / f, dest / f)
        except OSError:
            shutil.copy2(models / f, dest / f)
    if (models / "verified.json").is_file():
        shutil.copy2(models / "verified.json", dest / "verified.json")


# ---------------------------------------------------------------- WAV files


@dataclass
class Wav:
    name: str
    rate: int
    channels: int
    samples: List[int]

    @property
    def seconds(self) -> float:
        return len(self.samples) / float(self.rate * self.channels)

    @property
    def peak(self) -> int:
        return max((abs(s) for s in self.samples), default=0)

    @property
    def rms(self) -> float:
        if not self.samples:
            return 0.0
        return math.sqrt(sum(s * s for s in self.samples) / len(self.samples))

    def voiced_fraction(self, window_ms: int = 20, threshold: float = 300.0) -> float:
        """The share of ``window_ms`` windows whose RMS passes ``threshold``."""
        n = max(1, self.rate * self.channels * window_ms // 1000)
        windows = [self.samples[i:i + n] for i in range(0, len(self.samples), n)]
        if not windows:
            return 0.0
        loud = sum(1 for w in windows if math.sqrt(sum(s * s for s in w) / len(w)) > threshold)
        return loud / len(windows)


def read_wav(path: Path) -> Wav:
    with wave.open(str(path), "rb") as w:
        assert w.getsampwidth() == 2, f"{path.name}: not 16-bit"
        frames = w.readframes(w.getnframes())
        rate, channels = w.getframerate(), w.getnchannels()
    samples = list(struct.unpack("<%dh" % (len(frames) // 2), frames))
    return Wav(path.name, rate, channels, samples)


_NAME = re.compile(r"^(\d+)-item(\d+)-chunk(\d+)(?:\.(\d+))?\.wav$")


def item_wavs(wav_dir: Path) -> Dict[int, List[Wav]]:
    """item id -> its chunk files, in the order they were played."""
    out: Dict[int, List[Wav]] = {}
    for p in sorted(wav_dir.glob("*.wav")):
        m = _NAME.match(p.name)
        if m:
            out.setdefault(int(m.group(2)), []).append(read_wav(p))
    return out


def joined(wavs: List[Wav]) -> Wav:
    assert wavs, "no audio"
    rates = {(w.rate, w.channels) for w in wavs}
    assert len(rates) == 1, f"mixed formats: {rates}"
    rate, ch = rates.pop()
    return Wav("+".join(w.name for w in wavs), rate, ch, [s for w in wavs for s in w.samples])


# ------------------------------------------------------------ fake provider


def provider_wav() -> bytes:
    data = struct.pack("<h", PROVIDER_VALUE) * PROVIDER_SAMPLES
    return (b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVE"
            + b"fmt " + struct.pack("<IHHIIHH", 16, 1, 1, PROVIDER_RATE, PROVIDER_RATE * 2, 2, 16)
            + b"data" + struct.pack("<I", len(data)) + data)


class FakeProvider:
    """An OpenAI-compatible speech server on 127.0.0.1: ``POST
    /v1/audio/speech`` answers ``provider_wav()``, ``GET /v1/audio/voices``
    a Kokoro-FastAPI voice list. Requests are kept."""

    def __init__(self) -> None:
        self.requests: List[dict] = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):  # quiet
                pass

            def _send(self, ctype: str, body: bytes) -> None:
                self.send_response(200)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):  # noqa: N802 - http.server API
                outer.requests.append({"method": "GET", "path": self.path, "headers": dict(self.headers)})
                self._send("application/json", json.dumps({"voices": ["af_heart", "am_echo"]}).encode())

            def do_POST(self):  # noqa: N802 - http.server API
                n = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(n)
                outer.requests.append({"method": "POST", "path": self.path,
                                       "headers": {k.lower(): v for k, v in self.headers.items()},
                                       "body": json.loads(body or b"{}")})
                self._send("audio/wav", provider_wav())

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = "http://127.0.0.1:%d/v1" % self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def speech_requests(self) -> List[dict]:
        return [r for r in self.requests if r["method"] == "POST" and r["path"].endswith("/audio/speech")]

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


# ---------------------------------------------------------------- the hosts


def run_host(argv: List[str], env: Dict[str, str], cwd: Path, timeout: float = 240) -> dict:
    """Run a host app; its last stdout line is its JSON report."""
    full = dict(os.environ)
    full.update(env)
    r = subprocess.run(argv, cwd=str(cwd), env=full, capture_output=True, text=True,
                       encoding="utf-8", timeout=timeout)
    if r.returncode != 0:
        raise AssertionError("host {0} failed ({1}):\nstdout:\n{2}\nstderr:\n{3}".format(
            argv, r.returncode, r.stdout[-4000:], r.stderr[-4000:]))
    lines = [ln for ln in r.stdout.splitlines() if ln.strip()]
    assert lines, f"host {argv} printed nothing"
    return json.loads(lines[-1])


def host_env(home: Path, runtime_args: List[str], mode: str, provider: Optional[FakeProvider] = None,
             **extra: str) -> Dict[str, str]:
    env = {
        "SONARA_HOME": str(home),
        "SONARA_EMBED_ARGS": json.dumps(runtime_args),
        "SONARA_EMBED_MODE": mode,
        # The real voice's sentence, so every host says the same.
        "SONARA_EMBED_SENTENCE": REAL_SENTENCE,
        "PYTHONIOENCODING": "utf-8",
    }
    if provider:
        env["SONARA_EMBED_PROVIDER"] = provider.url
    env.update(extra)
    return env


def wait_runtime_gone(home: Path, timeout: float = 15) -> None:
    """The runtime exits on its own after the host left (--idle-exit);
    wait for it so the temp home can go."""
    rt = home / "runtime.json"
    deadline = time.monotonic() + timeout
    while rt.exists() and time.monotonic() < deadline:
        time.sleep(0.1)
    if rt.exists():
        try:
            pid = json.loads(rt.read_text(encoding="utf-8"))["pid"]
            subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)
        except (OSError, ValueError, KeyError):
            pass


def scenario_args(wav_dir: Path) -> List[str]:
    return ["--engine", "fake", "--keys", "fake", "--system", "fake", "--output", "wav:" + str(wav_dir),
            "--idle-exit", "2"]


def voice_args(engine: str, wav_dir: Path) -> List[str]:
    return ["--engine", engine, "--keys", "fake", "--system", "fake", "--output", "wav:" + str(wav_dir),
            "--idle-exit", "2"]


# ------------------------------------------------------------- the checks


def fake_samples(text: str, rate: int) -> int:
    """Samples the fake engine makes for one chunk (fake.rs ``render``)."""
    ms = len(text) * 10 * 200 // max(rate, 1)
    return max(1, ms * FAKE_RATE // 1000)


def check_scenario(report: dict, wav_dir: Path, provider: FakeProvider) -> None:
    """What every host's ``scenario`` run must show in its WAV files and
    its report (the host checked the protocol side itself)."""
    ids = report["items"]
    texts = report["texts"]
    wavs = item_wavs(wav_dir)

    # Each item that finished was played, chunk by chunk, as the fake
    # engine's tone: 16 kHz mono, +-8000, the exact length for its text.
    for name in ("first", "second", "long", "replacement", "cut_in", "paused", "after_skip"):
        audio = joined(wavs.get(ids[name], []))
        assert (audio.rate, audio.channels) == (FAKE_RATE, 1), (name, audio.rate)
        assert set(audio.samples) == {FAKE_AMPLITUDE, -FAKE_AMPLITUDE}, name
    assert len(joined(wavs[ids["first"]]).samples) == fake_samples(texts["first"], 200)
    # Items dropped before they were heard never reached the output.
    for name in ("queued", "never_read"):
        assert ids[name] not in wavs, f"{name} was played"
    # The voice reached the engine: 'silence' is all zeros.
    silent = joined(wavs[ids["silent"]])
    assert silent.peak == 0 and len(silent.samples) == fake_samples(texts["silent"], 200)
    # The rate reached the engine: rate 400 makes half the samples of 200.
    fast = joined(wavs[ids["fast"]])
    assert len(fast.samples) == fake_samples(texts["fast"], 400), len(fast.samples)
    # The external engine spoke, not its fallback: the provider's exact audio.
    ext = joined(wavs[ids["provider"]])
    assert (ext.rate, set(ext.samples)) == (PROVIDER_RATE, {PROVIDER_VALUE}), (ext.rate, ext.peak)
    assert len(ext.samples) % PROVIDER_SAMPLES == 0
    said = [r["body"].get("input") for r in provider.speech_requests()]
    assert texts["provider"] in said, said
    assert all(r["headers"].get("authorization") == "Bearer " + report["secret"]
               for r in provider.speech_requests())
    # Channels: both channels' messages were read.
    for name in ("channel_build", "channel_tests"):
        assert name in report["channel_items"], report["channel_items"]
        audio = joined(wavs[report["channel_items"][name]])
        assert audio.rate == FAKE_RATE and audio.peak == FAKE_AMPLITUDE


def check_real_voice(report: dict, wav_dir: Path, engine: str) -> Wav:
    """A real engine's sentence: speech, not silence or a tone, of a
    plausible length, at the engine's rate."""
    assert report["engine_status"]["engine"] == engine, report["engine_status"]
    assert report["engine_status"]["ready"] is True, report["engine_status"]
    assert report["phase"] == "finished", report
    audio = joined(item_wavs(wav_dir)[report["item"]])
    if engine == "kokoro":
        assert audio.rate == 24_000, audio.rate
    assert audio.rate >= 8_000, audio.rate
    words = len(REAL_SENTENCE.split())
    # 11 words at rate 250: about 2.6 s; any real voice lands in 1.2..10 s.
    assert 1.2 <= audio.seconds <= 10.0, (audio.seconds, words)
    assert audio.peak > 2_000, audio.peak
    assert audio.rms > 300, audio.rms
    # Speech has pauses and many levels; a tone or noise floor does not.
    voiced = audio.voiced_fraction()
    assert 0.3 <= voiced <= 0.99, voiced
    assert len(set(audio.samples)) > 1000, "too few distinct levels for speech"
    return audio


# ------------------------------------------------------------ release zip

ZIP_FILES = ("sonarad.exe", "sonara-hook.exe", "sonara.exe", "onnxruntime.dll", "onnxruntime-LICENSE.txt",
             "LICENSE", "LICENSING.md", "THIRD_PARTY_NOTICES.md")


def unpack_release(zip_path: Path, sums: Path, dest: Path) -> Path:
    """Check ``zip_path`` against ``SHA256SUMS`` the way the plugin's
    bootstrap does, unpack it into ``dest`` and return the folder that
    holds ``sonarad.exe`` (the zip's one top folder)."""
    import hashlib
    import zipfile

    want = {}
    for line in sums.read_text(encoding="ascii").splitlines():
        digest, _, name = line.partition("  ")
        want[name.strip()] = digest.strip()
    got = hashlib.sha256(zip_path.read_bytes()).hexdigest()
    assert want.get(zip_path.name) == got, f"{zip_path.name}: SHA256SUMS says {want.get(zip_path.name)}, file is {got}"
    with zipfile.ZipFile(zip_path) as z:
        tops = {n.split("/")[0] for n in z.namelist()}
        assert tops == {zip_path.stem}, f"the zip's top folders: {sorted(tops)}"
        z.extractall(dest)
    folder = dest / zip_path.stem
    missing = [f for f in ZIP_FILES if not (folder / f).is_file()]
    assert not missing, f"the zip lacks {missing}"
    return folder
