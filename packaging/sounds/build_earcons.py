"""Render Sonara's bundled earcons into crates/sonara-agent/sounds/.

The eight WAVs compiled into the runtime (crates/sonara-agent/src/earcon.rs)
are picks from two procedural candidate sets, chosen by ear in #211:
make_pack.py (round 1, all kinds) and make_round2.py (round 2: question,
reply_finished, session_changed). Original work, Sonara, MIT licence; no
samples or third-party audio. Needs numpy and scipy.

    python packaging/sounds/build_earcons.py [out_dir]   (default: crates/sonara-agent/sounds)
    python packaging/sounds/build_earcons.py --check     (render to a temp dir, compare with SHA256SUMS)

Writes the eight `<kind>.wav` files and SHA256SUMS. `nav_edge` uses the same
sound as `nav` (the user's pick), rendered to its own file so every kind
keeps one file per kind and a custom `<kind>.wav` replaces exactly one.
Byte-identical output is checked with the numpy/scipy versions this was made
with (numpy 2.4, scipy 1.17); other versions may round differently.
"""
from __future__ import annotations

import hashlib
import os
import shutil
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
SOUNDS = REPO / "crates" / "sonara-agent" / "sounds"

# kind -> (generator, candidate file it renders)
PICKS = {
    "choice": ("round2", "question-13.wav"),
    "permission": ("pack", "permission-6.wav"),
    "turn_done": ("round2", "reply_finished-08.wav"),
    "session_change": ("round2", "session_changed-01.wav"),
    "nav": ("pack", "nav-3.wav"),
    "nav_edge": ("pack", "nav-3.wav"),
    "error": ("pack", "error-3.wav"),
    "summary_failed": ("pack", "error-6.wav"),
}


def render(out_dir: Path) -> None:
    """Render the eight picks into out_dir as <kind>.wav, plus SHA256SUMS."""
    sys.path.insert(0, str(HERE))
    import make_pack as mp
    import make_round2 as r2

    out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        # Round 2 draws its noise from one shared generator (synth.RNG), so a
        # pick is only reproduced when every candidate before it is rendered
        # too, in the original order.
        r2.render_all(str(tmp / "round2"))
        pack = tmp / "pack"
        pack.mkdir()
        for name in {f for g, f in PICKS.values() if g == "pack"}:
            ev, i = name[: -len(".wav")].rsplit("-", 1)
            mp.write_wav(pack / name, mp.finish(mp.EVENTS[ev](int(i)), mp.TARGET[ev]))
        for kind, (gen, name) in PICKS.items():
            shutil.copyfile(tmp / gen / name, out_dir / f"{kind}.wav")
    (out_dir / "SHA256SUMS").write_bytes(sums(out_dir).encode("ascii"))


def sums(d: Path) -> str:
    """sha256sum-style lines for the eight WAVs, in PICKS order, LF endings."""
    lines = []
    for kind in PICKS:
        digest = hashlib.sha256((d / f"{kind}.wav").read_bytes()).hexdigest()
        lines.append(f"{digest}  {kind}.wav\n")
    return "".join(lines)


def main(argv: list[str]) -> int:
    if "--check" in argv:
        with tempfile.TemporaryDirectory() as tmp:
            render(Path(tmp))
            got = (Path(tmp) / "SHA256SUMS").read_text(encoding="ascii")
        want = (SOUNDS / "SHA256SUMS").read_text(encoding="ascii").replace("\r\n", "\n")
        if got != want:
            print("the rendered earcons differ from crates/sonara-agent/sounds/SHA256SUMS:")
            print(got)
            return 1
        print("the rendered earcons match SHA256SUMS")
        return 0
    out = Path(argv[0]) if argv else SOUNDS
    render(out)
    print(f"wrote {len(PICKS)} earcons to {os.path.relpath(out)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
