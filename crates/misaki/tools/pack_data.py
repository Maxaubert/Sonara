"""Rebuild the compressed data files of the vendored misaki crate.

The inputs are the data files of the misaki-rs 0.6.0 crate (MIT), checked
against the SHA-256 values below so a rebuild always starts from the same
upstream bytes. Only the US English lexicons are kept. Each file is
compressed with xz (preset 9 extreme, CRC64 check), which the crate decodes
with lzma-rust2 (Apache-2.0) when the G2P is built.

    cargo fetch   # any crate that depends on misaki-rs 0.6.0, or download it
    python crates/misaki/tools/pack_data.py <path to misaki-rs-0.6.0>

Stdlib only. Prints each file with its raw and compressed size.
"""
from __future__ import annotations

import hashlib
import lzma
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "data"

# (upstream path, SHA-256 of the upstream file, output name)
INPUTS = [
    ("data/us_gold.json", "bb83c899d8dbfa160fa05661bea052bacfeece9b639851662334e85002ee8ad9", "us_gold.json.xz"),
    ("data/us_silver.json", "57cae2a1a9d73ce219ad9142b0d904914a0228cb1babce20e5bfd4e1b1307ee4", "us_silver.json.xz"),
    (
        "src/resources/tagger/weights.json",
        "789e8e35f6fac656d9b7eccea29ba4e8a137cbb5b525dd31f39e4291a9e80832",
        "tagger-weights.json.xz",
    ),
    (
        "src/resources/tagger/tags.json",
        "4c713731cb06727736962bc8cd94ad919217a040e76691059af99bc0c2c9c246",
        "tagger-tags.json.xz",
    ),
]


def pack(upstream: Path) -> None:
    for rel, sha, name in INPUTS:
        raw = (upstream / rel).read_bytes()
        got = hashlib.sha256(raw).hexdigest()
        if got != sha:
            raise SystemExit(f"{rel}: SHA-256 {got} is not the pinned {sha}")
        packed = lzma.compress(raw, format=lzma.FORMAT_XZ, check=lzma.CHECK_CRC64, preset=9 | lzma.PRESET_EXTREME)
        (OUT / name).write_bytes(packed)
        print(f"{name}: {len(raw)} -> {len(packed)} bytes")


def main(argv: list) -> int:
    if len(argv) != 1:
        print(__doc__)
        return 2
    pack(Path(argv[0]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
