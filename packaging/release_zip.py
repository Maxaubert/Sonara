"""Build a runtime release archive, one per platform (``--platform``):

- ``win-x64`` (default): ``sonara-runtime-win-x64-<version>.zip`` with
  ``sonarad.exe``, ``sonara-hook.exe`` and ``sonara.exe``, the runtime files
  staged next to them by ``runtime_dlls.py`` (``onnxruntime.dll`` and its
  licence files for the Kokoro engine, the Visual C++ runtime DLLs it
  imports);
- ``macos-arm64``: ``sonara-runtime-macos-arm64-<version>.tar.gz`` with
  ``sonarad``, ``sonara-hook`` and ``sonara`` (mode 0755, which a zip would
  lose), ``libonnxruntime.dylib`` and its licence files. The executables
  and the library must be signed first (``packaging/sign_macos.py``); the
  plugin's bootstrap refuses an archive whose signatures do not verify.

Both also hold ``LICENSE``, ``LICENSING.md`` and ``THIRD_PARTY_NOTICES.md``
under one ``sonara-runtime-<platform>-<version>/`` folder. The Claude Code
plugin installs the archive (#202); hosts that use neither npm nor PyPI
bundle it. The earcons, prompts and settings page are built into the
executables. Next to the archive it writes ``SHA256SUMS`` (``sha256sum``
format), which the plugin's bootstrap checks the download against.
``--merge`` joins the ``SHA256SUMS`` files of several platform builds into
the one a release publishes, checking each listed archive that is present.

    cargo build -p sonarad -p sonara-hook -p sonara-cli --release
    python packaging/runtime_dlls.py stage target/release
    python packaging/release_zip.py [--platform win-x64|macos-arm64] [--exe target/release/sonarad.exe] [--out dist]
    python packaging/release_zip.py --merge win/SHA256SUMS mac/SHA256SUMS --out dist

Prints the path of the archive (with ``--merge``, of ``SHA256SUMS``).
Stdlib only.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import re
import sys
import tarfile
import zipfile
from pathlib import Path
from typing import NamedTuple

REPO = Path(__file__).resolve().parent.parent
NOTICE_FILES = ("LICENSE", "LICENSING.md", "THIRD_PARTY_NOTICES.md")
# The executables, all next to sonarad.exe: the runtime, the Claude Code
# hook adapter and the command line of the plugin.
EXES = ("sonarad.exe", "sonara-hook.exe", "sonara.exe")
# Files that ship next to sonarad.exe when present (runtime_dlls.py stages
# them); onnxruntime.dll is required: without it there is no Kokoro.
RUNTIME_FILES = (
    "onnxruntime.dll",
    "onnxruntime-LICENSE.txt",
    "onnxruntime-ThirdPartyNotices.txt",
    "msvcp140.dll",
    "msvcp140_1.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
)
REQUIRED = ("onnxruntime.dll", "onnxruntime-LICENSE.txt")

MACOS_EXES = ("sonarad", "sonara-hook", "sonara")
MACOS_RUNTIME_FILES = ("libonnxruntime.dylib", "onnxruntime-LICENSE.txt", "onnxruntime-ThirdPartyNotices.txt")
MACOS_REQUIRED = ("libonnxruntime.dylib", "onnxruntime-LICENSE.txt")


class Platform(NamedTuple):
    """What one platform's release archive holds."""

    name: str
    exes: tuple
    runtime_files: tuple
    required: tuple
    # ".zip" or ".tar.gz"
    suffix: str
    # Files that keep mode 0755 in a tar archive (the executables and the
    # library; everything else is 0644).
    executable: tuple


PLATFORMS = {
    "win-x64": Platform("win-x64", EXES, RUNTIME_FILES, REQUIRED, ".zip", ()),
    "macos-arm64": Platform("macos-arm64", MACOS_EXES, MACOS_RUNTIME_FILES, MACOS_REQUIRED, ".tar.gz",
                            MACOS_EXES + ("libonnxruntime.dylib",)),
}


def host_platform() -> str:
    """The platform this script runs on."""
    return "macos-arm64" if sys.platform == "darwin" else "win-x64"


def archive_name(platform: str, version: str) -> str:
    """The file name of ``platform``'s release archive for ``version``."""
    return f"sonara-runtime-{platform}-{version}{PLATFORMS[platform].suffix}"


def workspace_version(repo: Path = REPO) -> str:
    text = (repo / "Cargo.toml").read_text(encoding="utf-8")
    section = re.search(r"^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    m = section and re.search(r'^version = "([^"]+)"', section.group(1), re.M)
    if not m:
        raise SystemExit("Cargo.toml declares no [workspace.package] version")
    return m.group(1)


def _inputs(exe: Path, plat: Platform, repo: Path) -> list:
    """(source, name in the archive) of every file, after checking that
    the required ones are there."""
    if not exe.is_file():
        raise SystemExit(f"{exe} not found: run 'cargo build -p sonarad --release' first")
    lacking = [f for f in plat.exes if not (exe.parent / f).is_file()]
    if lacking:
        raise SystemExit(
            f"{', '.join(lacking)} not next to {exe}: run "
            "'cargo build -p sonarad -p sonara-hook -p sonara-cli --release' first"
        )
    missing = [f for f in NOTICE_FILES if not (repo / f).is_file()]
    if missing:
        raise SystemExit(f"missing at the repository root: {', '.join(missing)}")
    absent = [f for f in plat.required if not (exe.parent / f).is_file()]
    if absent:
        raise SystemExit(
            f"{', '.join(absent)} not next to {exe}: run 'python packaging/runtime_dlls.py stage {exe.parent}'"
        )
    files = [(exe.parent / f, f) for f in plat.exes]
    files += [(exe.parent / f, f) for f in plat.runtime_files if (exe.parent / f).is_file()]
    files += [(repo / f, f) for f in NOTICE_FILES]
    return files


def _tar(path: Path, folder: str, files: list, executable: tuple) -> None:
    """A gzipped tar of ``files`` under ``folder/``: regular files only
    (links are read through), mode 0755 for ``executable`` and 0644 for the
    rest, no owner names and no macOS metadata (AppleDouble ``._`` files or
    extended attributes)."""
    def info(name: str, size: int, mode: int, mtime: float) -> tarfile.TarInfo:
        ti = tarfile.TarInfo(name)
        ti.size = size
        ti.mode = mode
        ti.mtime = int(mtime)
        ti.uid = ti.gid = 0
        ti.uname = ti.gname = ""
        return ti

    with tarfile.open(path, "w:gz", format=tarfile.PAX_FORMAT) as t:
        top = info(folder, 0, 0o755, max(src.stat().st_mtime for src, _ in files))
        top.type = tarfile.DIRTYPE
        t.addfile(top)
        for src, name in files:
            data = src.read_bytes()
            mode = 0o755 if name in executable else 0o644
            t.addfile(info(f"{folder}/{name}", len(data), mode, src.stat().st_mtime), io.BytesIO(data))


def build_archive(exe: Path, out_dir: Path, version: str, platform: str = "win-x64", repo: Path = REPO) -> Path:
    """The release archive of ``platform`` from the build next to ``exe``."""
    plat = PLATFORMS[platform]
    files = _inputs(exe, plat, repo)
    out_dir.mkdir(parents=True, exist_ok=True)
    folder = f"sonara-runtime-{platform}-{version}"
    path = out_dir / archive_name(platform, version)
    if plat.suffix == ".zip":
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as z:
            for src, name in files:
                z.write(src, f"{folder}/{name}")
    else:
        _tar(path, folder, files, plat.executable)
    return path


def build_zip(exe: Path, out_dir: Path, version: str, repo: Path = REPO) -> Path:
    """The Windows release zip (``build_archive`` for ``win-x64``)."""
    return build_archive(exe, out_dir, version, "win-x64", repo)


def write_sums(files, out_dir: Path) -> Path:
    """``SHA256SUMS`` in ``out_dir``: ``<sha256>  <name>`` per file, LF."""
    lines = []
    for f in files:
        digest = hashlib.sha256(Path(f).read_bytes()).hexdigest()
        lines.append(f"{digest}  {Path(f).name}\n")
    path = Path(out_dir) / "SHA256SUMS"
    path.write_bytes("".join(lines).encode("ascii"))
    return path


SUM_LINE = re.compile(r"^([0-9a-fA-F]{64}) [ *](.+)$")


def read_sums(path: Path) -> dict:
    """The ``name -> sha256`` entries of a ``SHA256SUMS`` file."""
    sums = {}
    for line in Path(path).read_text(encoding="ascii").splitlines():
        if not line.strip():
            continue
        m = SUM_LINE.match(line.strip())
        if not m:
            raise SystemExit(f"{path}: not a SHA256SUMS line: {line!r}")
        sums[m.group(2)] = m.group(1).lower()
    return sums


def merge_sums(sums_files, out_dir: Path) -> Path:
    """One ``SHA256SUMS`` in ``out_dir`` with the entries of every file in
    ``sums_files`` (one per platform build), sorted by name. Two different
    hashes for one name, or a listed file in ``out_dir`` whose hash differs,
    is an error: a release never lists a hash its asset does not have."""
    merged: dict = {}
    for f in sums_files:
        for name, digest in read_sums(f).items():
            if merged.get(name, digest) != digest:
                raise SystemExit(f"{name} has two different SHA-256 values in {', '.join(map(str, sums_files))}")
            merged[name] = digest
    if not merged:
        raise SystemExit("no SHA256SUMS entries to merge")
    out_dir = Path(out_dir)
    for name, digest in merged.items():
        asset = out_dir / name
        if asset.is_file() and hashlib.sha256(asset.read_bytes()).hexdigest() != digest:
            raise SystemExit(f"{asset}: its SHA-256 is not the one SHA256SUMS lists")
    path = out_dir / "SHA256SUMS"
    path.write_bytes("".join(f"{merged[n]}  {n}\n" for n in sorted(merged)).encode("ascii"))
    return path


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Build sonara-runtime-<platform>-<version>.zip / .tar.gz")
    ap.add_argument("--platform", choices=sorted(PLATFORMS), default=None,
                    help="the archive's platform (default: this machine's)")
    ap.add_argument("--exe", type=Path, default=None,
                    help="the built sonarad (default: target/release/sonarad[.exe])")
    ap.add_argument("--out", type=Path, default=REPO / "dist")
    ap.add_argument("--merge", nargs="+", type=Path, metavar="SHA256SUMS",
                    help="merge these SHA256SUMS files into OUT/SHA256SUMS instead of building")
    args = ap.parse_args(argv)
    if args.merge:
        print(merge_sums(args.merge, args.out))
        return 0
    platform = args.platform or host_platform()
    exe = args.exe or REPO / "target" / "release" / PLATFORMS[platform].exes[0]
    path = build_archive(exe, args.out, workspace_version(), platform)
    write_sums([path], args.out)
    print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
