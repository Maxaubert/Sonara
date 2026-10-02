"""Build the runtime release zip for hosts that use neither npm nor PyPI:
``sonara-runtime-win-x64-<version>.zip`` with ``sonarad.exe``, any runtime
DLLs built next to it (``onnxruntime*.dll`` once the Kokoro engine ships),
``LICENSE``, ``LICENSING.md`` and ``THIRD_PARTY_NOTICES.md``.

    cargo build -p sonarad --release
    python packaging/release_zip.py [--exe target/release/sonarad.exe] [--out dist]

Prints the path of the zip. Stdlib only.
"""
from __future__ import annotations

import argparse
import re
import sys
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
NOTICE_FILES = ("LICENSE", "LICENSING.md", "THIRD_PARTY_NOTICES.md")


def workspace_version(repo: Path = REPO) -> str:
    text = (repo / "Cargo.toml").read_text(encoding="utf-8")
    section = re.search(r"^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    m = section and re.search(r'^version = "([^"]+)"', section.group(1), re.M)
    if not m:
        raise SystemExit("Cargo.toml declares no [workspace.package] version")
    return m.group(1)


def build_zip(exe: Path, out_dir: Path, version: str, repo: Path = REPO) -> Path:
    if not exe.is_file():
        raise SystemExit(f"{exe} not found: run 'cargo build -p sonarad --release' first")
    missing = [f for f in NOTICE_FILES if not (repo / f).is_file()]
    if missing:
        raise SystemExit(f"missing at the repository root: {', '.join(missing)}")
    out_dir.mkdir(parents=True, exist_ok=True)
    name = f"sonara-runtime-win-x64-{version}"
    path = out_dir / f"{name}.zip"
    dlls = sorted(p for p in exe.parent.glob("*.dll") if p.name.lower().startswith("onnxruntime"))
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as z:
        z.write(exe, f"{name}/sonarad.exe")
        for dll in dlls:
            z.write(dll, f"{name}/{dll.name}")
        for f in NOTICE_FILES:
            z.write(repo / f, f"{name}/{f}")
    return path


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Build sonara-runtime-win-x64-<version>.zip")
    ap.add_argument("--exe", type=Path, default=REPO / "target" / "release" / "sonarad.exe")
    ap.add_argument("--out", type=Path, default=REPO / "dist")
    args = ap.parse_args(argv)
    print(build_zip(args.exe, args.out, workspace_version()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
