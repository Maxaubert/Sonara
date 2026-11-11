"""Stage the libraries the Kokoro engine needs next to ``sonarad``.

- Windows (``win-x64``): ``onnxruntime.dll`` from Microsoft's official ONNX Runtime CPU build for
  Windows x64 (MIT), fetched by URL and checked against a pinned SHA-256,
  with its ``LICENSE`` and ``ThirdPartyNotices.txt`` (shipped as
  ``onnxruntime-LICENSE.txt`` and ``onnxruntime-ThirdPartyNotices.txt``).
- The Visual C++ runtime DLLs ``onnxruntime.dll`` imports (``msvcp140.dll``,
  ``msvcp140_1.dll``, ``vcruntime140.dll``, ``vcruntime140_1.dll``), copied
  from the Visual Studio redistributable folder of this machine (app-local
  deployment, allowed by the Visual Studio licence terms). ``sonarad.exe``
  itself links the C runtime statically and needs none of them. A redist
  older than ``MIN_VC_VERSION`` is refused: a runtime older than the toolset
  ORT was built with can crash (the ``std::mutex`` constexpr change of
  VS 2022 17.10, runtime 14.40).
- macOS (``macos-arm64``): ``libonnxruntime.dylib`` from Microsoft's
  official ``osx-arm64`` build (a tgz, also pinned by SHA-256), with the same
  two licence files. It links only system frameworks and libc++, so there is
  no VC++ step.

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
import subprocess
import sys
import tarfile
import urllib.request
import zipfile
from typing import NamedTuple
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ORT_VERSION = "1.28.2"
ORT_URL = (
    f"https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/"
    f"onnxruntime-win-x64-{ORT_VERSION}.zip"
)
ORT_SHA256 = "c4eedd29489d5feca21866d054638416f3655bf6b18851b3b6b85c8313e95c35"
CACHE = REPO / "target" / "onnxruntime"
# In the extracted folder: the SHA-256 of each file, from the checked zip.
CACHE_SUMS = "SHA256SUMS"
# (member in the zip, name next to sonarad.exe)
ORT_FILES = (
    ("lib/onnxruntime.dll", "onnxruntime.dll"),
    ("LICENSE", "onnxruntime-LICENSE.txt"),
    ("ThirdPartyNotices.txt", "onnxruntime-ThirdPartyNotices.txt"),
)


class Ort(NamedTuple):
    """One platform's ONNX Runtime asset."""

    platform: str
    # The archive's top folder, also its name in the cache (plus ``suffix``).
    stem: str
    suffix: str
    url: str
    sha256: str
    # (member under ``stem/``, name next to sonarad)
    files: tuple
    # The cache folder under ``CACHE``.
    folder: str
    # Whether the VC++ runtime ships next to it.
    vc: bool


ORT_MACOS_STEM = f"onnxruntime-osx-arm64-{ORT_VERSION}"
ORT_MACOS_URL = (
    f"https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/{ORT_MACOS_STEM}.tgz"
)
# Checked against the downloaded release asset on 2026-11-11.
ORT_MACOS_SHA256 = "c4fceacfc53765d0869dc9180c31ec91054d149017a99d1e80ffe28dc79596de"
# ``lib/libonnxruntime.dylib`` is a regular file in this tgz (a copy of
# ``libonnxruntime.1.28.2.dylib``); a symlink there would be followed.
ORT_MACOS_FILES = (
    ("lib/libonnxruntime.dylib", "libonnxruntime.dylib"),
    ("LICENSE", "onnxruntime-LICENSE.txt"),
    ("ThirdPartyNotices.txt", "onnxruntime-ThirdPartyNotices.txt"),
)

PLATFORMS = {
    "win-x64": Ort("win-x64", f"onnxruntime-win-x64-{ORT_VERSION}", ".zip", ORT_URL, ORT_SHA256,
                   ORT_FILES, ORT_VERSION, True),
    "macos-arm64": Ort("macos-arm64", ORT_MACOS_STEM, ".tgz", ORT_MACOS_URL, ORT_MACOS_SHA256,
                       ORT_MACOS_FILES, f"{ORT_VERSION}-osx-arm64", False),
}


def host_platform() -> str:
    """The platform this script runs on."""
    return "macos-arm64" if sys.platform == "darwin" else "win-x64"


VC_DLLS = ("msvcp140.dll", "msvcp140_1.dll", "vcruntime140.dll", "vcruntime140_1.dll")
# The oldest VC++ runtime ORT 1.28.2 may load with: VS 2022 17.10 (14.40),
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
    extraction (``SHA256SUMS``, from the hash-checked archive)."""
    try:
        lines = (out / CACHE_SUMS).read_text(encoding="ascii").splitlines()
    except (OSError, ValueError):
        return False
    want = {}
    for line in lines:
        digest, _, name = line.partition("  ")
        want[name] = digest
    return all((out / name).is_file() and want.get(name) == sha256(out / name) for _, name in files)


def _tgz_reader(t: tarfile.TarFile, path: Path, root: str):
    """Reads a member under ``root`` of the open tgz ``t``, following
    symlinks inside the archive (never out of it)."""

    def read(member: str) -> bytes:
        info = t.getmember(root + member)
        for _ in range(8):
            if not info.issym():
                break
            target = os.path.normpath(os.path.join(os.path.dirname(info.name), info.linkname))
            info = t.getmember(target.replace(os.sep, "/"))
        f = t.extractfile(info) if info.isfile() else None
        if f is None:
            raise SystemExit(f"{path}: {root + member} is not a regular file")
        return f.read()

    return read


def fetch(cache: Path = CACHE, url: str | None = None, expected: str | None = None,
          platform: str = "win-x64") -> Path:
    """The folder with the extracted ORT files of ``platform``, downloading
    its archive once. A cached file is used only while its SHA-256 matches
    the one recorded when it was extracted (#55); else the folder is
    extracted again."""
    ort = PLATFORMS[platform]
    url = url or ort.url
    expected = expected or ort.sha256
    out = cache / ort.folder
    if _cached(out, ort.files):
        return out
    cache.mkdir(parents=True, exist_ok=True)
    zpath = cache / f"{ort.stem}{ort.suffix}"
    if not zpath.is_file() or sha256(zpath) != expected:
        tmp = zpath.with_suffix(".part")
        with urllib.request.urlopen(url, timeout=60) as r, tmp.open("wb") as f:
            shutil.copyfileobj(r, f, 1 << 20)
        got = sha256(tmp)
        if got != expected:
            tmp.unlink()
            raise SystemExit(f"{url}: SHA-256 {got} is not the pinned {expected}")
        tmp.replace(zpath)
    # Extract into a folder of its own and rename it into place, so a run
    # that stops part-way never leaves files a later run accepts.
    part = cache / f"{ort.folder}.part"
    shutil.rmtree(part, ignore_errors=True)
    part.mkdir()
    root = f"{ort.stem}/"
    sums = []
    archive = zipfile.ZipFile(zpath) if ort.suffix == ".zip" else tarfile.open(zpath, "r:gz")
    with archive:
        if isinstance(archive, zipfile.ZipFile):
            def read(member: str) -> bytes:
                return archive.read(root + member)
        else:
            read = _tgz_reader(archive, zpath, root)
        for member, name in ort.files:
            data = read(member)
            (part / name).write_bytes(data)
            sums.append(f"{hashlib.sha256(data).hexdigest()}  {name}\n")
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
