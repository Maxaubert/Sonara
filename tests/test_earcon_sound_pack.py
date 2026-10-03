"""The runtime's bundled earcons (#211): crates/sonara-agent/sounds.

The eight WAVs are compiled into sonarad (crates/sonara-agent/src/earcon.rs)
and rendered by packaging/sounds/build_earcons.py. SHA256SUMS pins them; the
regeneration check needs numpy and scipy and is skipped without them.
"""
from __future__ import annotations

import hashlib
import importlib.util
import re
import sys
import wave
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[1]
SOUNDS = REPO / "crates" / "sonara-agent" / "sounds"
KINDS = [
    "choice",
    "permission",
    "turn_done",
    "session_change",
    "nav",
    "nav_edge",
    "error",
    "summary_failed",
]


def _sums() -> dict:
    out = {}
    for line in (SOUNDS / "SHA256SUMS").read_text(encoding="ascii").splitlines():
        digest, name = line.split("  ", 1)
        out[name] = digest
    return out


def test_the_runtime_knows_exactly_these_earcon_kinds():
    src = (REPO / "crates" / "sonara-agent" / "src" / "earcon.rs").read_text(encoding="utf-8")
    block = src[src.index("earcons! {"):]
    block = block[: block.index("}")]
    assert sorted(re.findall(r'=> "([a-z_]+)"', block)) == sorted(KINDS)
    assert '"../sounds/"' in src


@pytest.mark.parametrize("kind", KINDS)
def test_every_bundled_earcon_is_a_short_48k_mono_16bit_wav(kind):
    with wave.open(str(SOUNDS / f"{kind}.wav"), "rb") as w:
        assert (w.getnchannels(), w.getsampwidth(), w.getframerate()) == (1, 2, 48000)
        frames = w.getnframes()
        data = w.readframes(frames)
    assert 0.01 < frames / 48000 < 2.0
    assert any(data), f"{kind} is silent"


def test_the_bundled_earcons_match_their_checked_in_hashes():
    sums = _sums()
    assert sorted(sums) == sorted(f"{k}.wav" for k in KINDS)
    for name, digest in sums.items():
        assert hashlib.sha256((SOUNDS / name).read_bytes()).hexdigest() == digest, name
    # Only the eight picks and their manifest live there.
    assert sorted(p.name for p in SOUNDS.iterdir()) == sorted(list(sums) + ["SHA256SUMS"])


def test_no_rust_code_reads_the_legacy_python_earcons():
    # (sonarad's settings migration still recognises the Python package's
    # earcon paths in an old config.json; it never reads those files.)
    for path in (REPO / "crates").rglob("*.rs"):
        text = path.read_text(encoding="utf-8").replace("\\", "/")
        assert "src/sonara/platform/windows/earcons" not in text, path


@pytest.mark.skipif(
    importlib.util.find_spec("numpy") is None or importlib.util.find_spec("scipy") is None,
    reason="the sound generators need numpy and scipy",
)
def test_the_generators_rebuild_the_bundled_earcons_byte_for_byte(tmp_path, monkeypatch):
    monkeypatch.syspath_prepend(str(REPO / "packaging" / "sounds"))
    for mod in ("build_earcons", "make_pack", "make_round2", "synth"):
        monkeypatch.delitem(sys.modules, mod, raising=False)
    import build_earcons

    build_earcons.render(tmp_path)
    for kind in KINDS:
        got = (tmp_path / f"{kind}.wav").read_bytes()
        assert got == (SOUNDS / f"{kind}.wav").read_bytes(), kind
