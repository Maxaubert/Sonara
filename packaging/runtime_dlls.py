"""Stage the DLLs the Kokoro engine needs next to ``sonarad.exe``.

- ``onnxruntime.dll`` from Microsoft's official ONNX Runtime CPU build for
  Windows x64 (MIT), fetched by URL and checked against a pinned SHA-256,
  with its ``LICENSE`` and ``ThirdPartyNotices.txt`` (shipped as
  ``onnxruntime-LICENSE.txt`` and ``onnxruntime-ThirdPartyNotices.txt``).
- The Visual C++ runtime DLLs ``onnxruntime.dll`` imports (``msvcp140.dll``,
  ``msvcp140_1.dll``, ``vcruntime140.dll``, ``vcruntime140_1.dll``), copied
  from the Visual Studio redistributable folder of this machine (app-local
  deployment, allowed by the Visual Studio licence terms). ``sonarad.exe``
  itself links the C runtime statically and needs none of them.

    python packaging/runtime_dlls.py fetch            # download + verify (cached)
    python packaging/runtime_dlls.py stage target/release
    python packaging/runtime_dlls.py stage target/release --no-vc   # ORT only

Stdlib only. The cache is ``target/onnxruntime/``.
"""
from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ORT_VERSION = "1.28.2"
ORT_URL = (
    f"https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/"
    f"onnxruntime-win-x64-{ORT_VERSION}.zip"
)
ORT_SHA256 = "c4eedd29489d5feca21866d054638416f3655bf6b18851b3b6b85c8313e95c35"
CACHE = REPO / "target" / "onnxruntime"
# (member in the zip, name next to sonarad.exe)
ORT_FILES = (
    ("lib/onnxruntime.dll", "onnxruntime.dll"),
    ("LICENSE", "onnxruntime-LICENSE.txt"),
    ("ThirdPartyNotices.txt", "onnxruntime-ThirdPartyNotices.txt"),
)
VC_DLLS = ("msvcp140.dll", "msvcp140_1.dll", "vcruntime140.dll", "vcruntime140_1.dll")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def fetch(cache: Path = CACHE, url: str = ORT_URL, expected: str = ORT_SHA256) -> Path:
    """The folder with the extracted ORT files, downloading the zip once."""
    out = cache / ORT_VERSION
    if all((out / name).is_file() for _, name in ORT_FILES):
        return out
    cache.mkdir(parents=True, exist_ok=True)
    zpath = cache / f"onnxruntime-win-x64-{ORT_VERSION}.zip"
    if not zpath.is_file() or sha256(zpath) != expected:
        tmp = zpath.with_suffix(".part")
        with urllib.request.urlopen(url, timeout=60) as r, tmp.open("wb") as f:
            shutil.copyfileobj(r, f, 1 << 20)
        got = sha256(tmp)
        if got != expected:
            tmp.unlink()
            raise SystemExit(f"{url}: SHA-256 {got} is not the pinned {expected}")
        tmp.replace(zpath)
    out.mkdir(parents=True, exist_ok=True)
    root = f"onnxruntime-win-x64-{ORT_VERSION}/"
    with zipfile.ZipFile(zpath) as z:
        for member, name in ORT_FILES:
            (out / name).write_bytes(z.read(root + member))
    return out


def vc_redist_dir() -> Path:
    """The x64 Microsoft.VC14x.CRT folder of the newest Visual Studio here."""
    env = os.environ.get("VCToolsRedistDir")
    candidates = []
    if env:
        candidates.append(Path(env))
    vswhere = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / (
        r"Microsoft Visual Studio\Installer\vswhere.exe"
    )
    if vswhere.is_file():
        found = subprocess.run(
            [str(vswhere), "-latest", "-products", "*", "-property", "installationPath"],
            capture_output=True, text=True, check=False,
        ).stdout.strip()
        for line in found.splitlines():
            candidates.extend(sorted((Path(line) / "VC" / "Redist" / "MSVC").glob("*"), reverse=True))
    for c in candidates:
        crts = sorted((c / "x64").glob("Microsoft.VC14*.CRT"), reverse=True)
        if crts and all((crts[0] / d).is_file() for d in VC_DLLS):
            return crts[0]
    raise SystemExit("no Visual C++ redistributable folder found (install Visual Studio Build Tools)")


def stage(dest: Path, vc: bool = True, cache: Path = CACHE) -> list:
    """Copy the runtime files into ``dest``; returns their names."""
    dest.mkdir(parents=True, exist_ok=True)
    src = fetch(cache)
    names = []
    for _, name in ORT_FILES:
        shutil.copy2(src / name, dest / name)
        names.append(name)
    if vc:
        crt = vc_redist_dir()
        for d in VC_DLLS:
            shutil.copy2(crt / d, dest / d)
            names.append(d)
    return names


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Stage onnxruntime.dll and the VC++ runtime next to sonarad.exe")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("fetch", help="download and verify the ONNX Runtime zip (cached)")
    st = sub.add_parser("stage", help="copy the runtime files into a folder")
    st.add_argument("dest", type=Path)
    st.add_argument("--no-vc", action="store_true", help="skip the Visual C++ runtime DLLs")
    args = ap.parse_args(argv)
    if args.cmd == "fetch":
        print(fetch())
    else:
        for name in stage(args.dest, vc=not args.no_vc):
            print(args.dest / name)
    return 0


if __name__ == "__main__":
    sys.exit(main())
