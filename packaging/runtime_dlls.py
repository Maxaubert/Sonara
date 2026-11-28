"""Stage the libraries the Kokoro engine needs next to ``sonarad``.

Both platforms take ONNX Runtime from one archive: Microsoft's official NuGet
package ``Microsoft.ML.OnnxRuntime`` (MIT, the CPU build), fetched by URL
from nuget.org and checked against a pinned SHA-256. It is downloaded once
and cached; each platform extracts its own library from it, with the
package's ``LICENSE`` and ``ThirdPartyNotices.txt`` (shipped as
``onnxruntime-LICENSE.txt`` and ``onnxruntime-ThirdPartyNotices.txt``).
(The GitHub release assets used before are gone: github.com/microsoft/onnxruntime
answers 404 since 2026-11.)

- Windows (``win-x64``): ``runtimes/win-x64/native/onnxruntime.dll``, plus
  the Visual C++ runtime DLLs it imports (``msvcp140.dll``,
  ``msvcp140_1.dll``, ``vcruntime140.dll``, ``vcruntime140_1.dll``), copied
  from the Visual Studio redistributable folder of this machine (app-local
  deployment, allowed by the Visual Studio licence terms). ``sonarad.exe``
  itself links the C runtime statically and needs none of them. A redist
  older than ``MIN_VC_VERSION`` is refused: a runtime older than the toolset
  ORT was built with can crash (the ``std::mutex`` constexpr change of
  VS 2022 17.10, runtime 14.40).
- macOS (``macos-arm64``): ``runtimes/osx-arm64/native/libonnxruntime.dylib``.
  It links only system frameworks and libc++, so there is no VC++ step.

    python packaging/runtime_dlls.py fetch            # download + verify (cached)
    python packaging/runtime_dlls.py stage target/release
    python packaging/runtime_dlls.py stage target/release --no-vc   # ORT only
    python packaging/runtime_dlls.py --platform macos-arm64 fetch

``--platform`` defaults to the platform this runs on (``macos-arm64`` on
macOS, ``win-x64`` elsewhere).

Stdlib only. The cache is ``target/onnxruntime/``.
"""
from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import stat
import subprocess
import sys
import urllib.request
import zipfile
from typing import NamedTuple
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ORT_VERSION = "1.28.0"
# The one archive both platforms take ONNX Runtime from (a nupkg is a zip).
ORT_PACKAGE = f"microsoft.ml.onnxruntime.{ORT_VERSION}.nupkg"
ORT_URL = f"https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/{ORT_VERSION}/{ORT_PACKAGE}"
# Checked against the package downloaded from nuget.org on 2026-11-24.
ORT_SHA256 = "769d1d3ea8ab6cd69f737c9dd4d4462aa4ad0ccfa106eaf506efc40d7bead5db"
CACHE = REPO / "target" / "onnxruntime"
# In each extracted folder: the SHA-256 of each file, from the checked package.
CACHE_SUMS = "SHA256SUMS"
# (member in the package, name next to sonarad.exe)
ORT_FILES = (
    ("runtimes/win-x64/native/onnxruntime.dll", "onnxruntime.dll"),
    ("LICENSE", "onnxruntime-LICENSE.txt"),
    ("ThirdPartyNotices.txt", "onnxruntime-ThirdPartyNotices.txt"),
)
ORT_MACOS_FILES = (
    ("runtimes/osx-arm64/native/libonnxruntime.dylib", "libonnxruntime.dylib"),
    ("LICENSE", "onnxruntime-LICENSE.txt"),
    ("ThirdPartyNotices.txt", "onnxruntime-ThirdPartyNotices.txt"),
)


class Ort(NamedTuple):
    """One platform's ONNX Runtime files in the package."""

    platform: str
    # (member in the package, name next to sonarad)
    files: tuple
    # The cache folder under ``CACHE``.
    folder: str
    # Whether the VC++ runtime ships next to it.
    vc: bool


PLATFORMS = {
    "win-x64": Ort("win-x64", ORT_FILES, ORT_VERSION, True),
    "macos-arm64": Ort("macos-arm64", ORT_MACOS_FILES, f"{ORT_VERSION}-osx-arm64", False),
}


def host_platform() -> str:
    """The platform this script runs on."""
    return "macos-arm64" if sys.platform == "darwin" else "win-x64"


VC_DLLS = ("msvcp140.dll", "msvcp140_1.dll", "vcruntime140.dll", "vcruntime140_1.dll")
# The oldest VC++ runtime ORT 1.28 may load with: VS 2022 17.10 (14.40),
# the first with the constexpr std::mutex its builds rely on.
MIN_VC_VERSION = (14, 40, 33810, 0)
VERSIONED = ("msvcp140.dll", "vcruntime140.dll")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def _cached(out: Path, files=ORT_FILES) -> bool:
    """True when ``out`` holds every ORT file with the SHA-256 recorded at
    extraction (``SHA256SUMS``, from the hash-checked package)."""
    try:
        lines = (out / CACHE_SUMS).read_text(encoding="ascii").splitlines()
    except (OSError, ValueError):
        return False
    want = {}
    for line in lines:
        digest, _, name = line.partition("  ")
        want[name] = digest
    return all((out / name).is_file() and want.get(name) == sha256(out / name) for _, name in files)


def download(cache: Path, url: str, expected: str) -> Path:
    """The package in ``cache``, downloaded only when it is missing or does
    not match ``expected``. Both platforms share this one file."""
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / ORT_PACKAGE
    if path.is_file() and sha256(path) == expected:
        return path
    tmp = path.with_name(path.name + ".part")
    with urllib.request.urlopen(url, timeout=60) as r, tmp.open("wb") as f:
        shutil.copyfileobj(r, f, 1 << 20)
    got = sha256(tmp)
    if got != expected:
        tmp.unlink()
        raise SystemExit(f"{url}: SHA-256 {got} is not the pinned {expected}")
    tmp.replace(path)
    return path


def _read_regular(z: zipfile.ZipFile, path: Path, member: str) -> bytes:
    """A member of the package; refuses a folder or a symlink entry."""
    info = z.getinfo(member)
    mode = info.external_attr >> 16
    if info.is_dir() or info.external_attr & 0x10 or stat.S_ISDIR(mode) or stat.S_ISLNK(mode):
        raise SystemExit(f"{path}: {member} is not a regular file")
    return z.read(info)


def fetch(cache: Path = CACHE, url: str | None = None, expected: str | None = None,
          platform: str = "win-x64") -> Path:
    """The folder with the extracted ORT files of ``platform``, downloading
    the package once. A cached file is used only while its SHA-256 matches
    the one recorded when it was extracted (#55); else the folder is
    extracted again."""
    ort = PLATFORMS[platform]
    out = cache / ort.folder
    if _cached(out, ort.files):
        return out
    pkg = download(cache, url or ORT_URL, expected or ORT_SHA256)
    # Extract into a folder of its own and rename it into place, so a run
    # that stops part-way never leaves files a later run accepts.
    part = cache / f"{ort.folder}.part"
    shutil.rmtree(part, ignore_errors=True)
    part.mkdir()
    sums = []
    try:
        with zipfile.ZipFile(pkg) as z:
            for member, name in ort.files:
                data = _read_regular(z, pkg, member)
                (part / name).write_bytes(data)
                sums.append(f"{hashlib.sha256(data).hexdigest()}  {name}\n")
    except BaseException:
        shutil.rmtree(part, ignore_errors=True)
        raise
    (part / CACHE_SUMS).write_bytes("".join(sums).encode("ascii"))
    shutil.rmtree(out, ignore_errors=True)
    part.rename(out)
    return out


def file_version(path: Path):
    """The file version of a Windows PE as (major, minor, build, private), or None."""
    if sys.platform != "win32":
        return None
    import ctypes
    from ctypes import wintypes

    ver = ctypes.WinDLL("version")
    ver.GetFileVersionInfoSizeW.argtypes = [wintypes.LPCWSTR, wintypes.LPDWORD]
    ver.GetFileVersionInfoW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
    ver.VerQueryValueW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR, ctypes.POINTER(ctypes.c_void_p),
                                   ctypes.POINTER(wintypes.UINT)]
    size = ver.GetFileVersionInfoSizeW(str(path), None)
    if not size:
        return None
    buf = ctypes.create_string_buffer(size)
    if not ver.GetFileVersionInfoW(str(path), 0, size, buf):
        return None
    ptr, n = ctypes.c_void_p(), wintypes.UINT()
    if not ver.VerQueryValueW(buf, "\\", ctypes.byref(ptr), ctypes.byref(n)) or n.value < 52:
        return None
    # VS_FIXEDFILEINFO: signature, struc version, then FileVersionMS and LS.
    ms, ls = ctypes.cast(ptr, ctypes.POINTER(wintypes.DWORD * 4)).contents[2:4]
    return (ms >> 16, ms & 0xFFFF, ls >> 16, ls & 0xFFFF)


def check_vc(crt: Path, read=file_version) -> dict:
    """Refuse a redist older than ``MIN_VC_VERSION``; the versions found, by DLL."""
    found = {}
    least = ".".join(map(str, MIN_VC_VERSION))
    for d in VERSIONED:
        v = read(crt / d)
        if v is None:
            raise SystemExit(f"{crt / d}: no file version; cannot check it is at least {least}")
        shown = ".".join(map(str, v))
        if tuple(v) < MIN_VC_VERSION:
            raise SystemExit(
                f"{crt / d} {shown} is older than {least}, the oldest VC++ runtime "
                f"ONNX Runtime {ORT_VERSION} runs with; update Visual Studio Build Tools"
            )
        found[d] = shown
    return found


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


def stage(dest: Path, vc: bool = True, cache: Path = CACHE, platform: str = "win-x64") -> list:
    """Copy the runtime files of ``platform`` into ``dest``; returns their
    names. The VC++ runtime ships only on Windows."""
    ort = PLATFORMS[platform]
    dest.mkdir(parents=True, exist_ok=True)
    src = fetch(cache, platform=platform)
    names = []
    for _, name in ort.files:
        shutil.copy2(src / name, dest / name)
        names.append(name)
    if vc and ort.vc:
        crt = vc_redist_dir()
        for d, v in check_vc(crt).items():
            print(f"{d} {v}", file=sys.stderr)
        for d in VC_DLLS:
            shutil.copy2(crt / d, dest / d)
            names.append(d)
    return names


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Stage ONNX Runtime (on Windows with the VC++ runtime) next to sonarad")
    ap.add_argument("--platform", choices=sorted(PLATFORMS), default=host_platform(),
                    help="the runtime's platform (default: this machine's)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("fetch", help="download and verify the ONNX Runtime archive (cached)")
    st = sub.add_parser("stage", help="copy the runtime files into a folder")
    st.add_argument("dest", type=Path)
    st.add_argument("--no-vc", action="store_true", help="skip the Visual C++ runtime DLLs (Windows)")
    args = ap.parse_args(argv)
    if args.cmd == "fetch":
        print(fetch(platform=args.platform))
    else:
        for name in stage(args.dest, vc=not args.no_vc, platform=args.platform):
            print(args.dest / name)
    return 0


if __name__ == "__main__":
    sys.exit(main())
