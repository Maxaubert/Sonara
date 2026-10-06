"""Build and check the packages release.yml publishes (#279).

Usage: python packaging/build_packages.py [--out DIR] [--exe PATH] [--publish-dry-run]

Builds, into --out (default target/packages):
  npm/      @sonara/client and @sonara/runtime-win32-x64 tarballs, packed the
            way tests/embed packs them (npm run build, scripts/build.mjs,
            npm pack --ignore-scripts)
  python/   the sonara-client wheel and sdist (python -m build)
and checks each one holds the files an app needs and carries the release
version (Cargo.toml [workspace.package]). The Python dists also pass
`twine check --strict`. --publish-dry-run adds `npm publish --dry-run` for
both tarballs. @sonara/player is never built here: it is private (not
published).

Needs first: cargo build -p sonarad --release; python packaging/runtime_dlls.py
stage target/release; npm ci in clients/ts; python -m pip install build twine
(both are in the dev dependency group). ci.yml (clients job) and
packaging/gate.py (sdk gate) run it with --publish-dry-run; release.yml runs it
to build what it publishes. Prints one line per built file.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import shutil
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path
from typing import Iterable, List, Optional

REPO_ROOT = Path(__file__).resolve().parent.parent
CLIENT_TS = REPO_ROOT / "clients" / "ts"
NPM_RUNTIME = REPO_ROOT / "packaging" / "npm-runtime"
CLIENT_PY = REPO_ROOT / "clients" / "python"

# What each published package must contain (paths inside the package).
TS_REQUIRED = ("package.json", "README.md", "LICENSE", "dist/cjs/index.js", "dist/cjs/index.d.ts",
               "dist/esm/index.js", "dist/esm/index.d.ts")
RUNTIME_REQUIRED = ("package.json", "README.md", "LICENSE", "THIRD_PARTY_NOTICES.md", "index.js", "index.d.ts",
                    "bin/sonarad.exe", "bin/onnxruntime.dll", "bin/onnxruntime-LICENSE.txt",
                    "bin/onnxruntime-ThirdPartyNotices.txt", "bin/msvcp140.dll", "bin/msvcp140_1.dll",
                    "bin/vcruntime140.dll", "bin/vcruntime140_1.dll")
WHEEL_REQUIRED = ("sonara_client/__init__.py", "sonara_client/client.py", "sonara_client/version.py")
SDIST_REQUIRED = ("pyproject.toml", "README.md", "LICENSE", "src/sonara_client/__init__.py")


def _load_bump_version():
    spec = importlib.util.spec_from_file_location("bump_version", REPO_ROOT / "packaging" / "bump_version.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def missing(names: Iterable[str], required: Iterable[str]) -> List[str]:
    """The ``required`` paths absent from ``names`` (archive members, forward slashes)."""
    have = set(names)
    return [r for r in required if r not in have]


def tarball_files(path: Path) -> List[str]:
    """Paths inside an npm tarball, without npm's ``package/`` prefix."""
    with tarfile.open(path, "r:gz") as t:
        return [m.name.split("/", 1)[1] for m in t.getmembers() if m.isfile() and "/" in m.name]


def wheel_files(path: Path) -> List[str]:
    with zipfile.ZipFile(path) as z:
        return z.namelist()


def sdist_files(path: Path) -> List[str]:
    """Paths inside an sdist, without its ``<name>-<version>/`` prefix."""
    with tarfile.open(path, "r:gz") as t:
        return [m.name.split("/", 1)[1] for m in t.getmembers() if m.isfile() and "/" in m.name]


def _run(argv: List[str], cwd: Path) -> str:
    r = subprocess.run([str(a) for a in argv], cwd=str(cwd), capture_output=True, text=True, encoding="utf-8")
    if r.returncode != 0:
        raise SystemExit("build_packages: {0} failed (in {1}):\n{2}\n{3}".format(
            " ".join(str(a) for a in argv), cwd, r.stdout[-3000:], r.stderr[-3000:]))
    return r.stdout


def _check(label: str, names: List[str], required: Iterable[str]) -> None:
    gone = missing(names, required)
    if gone:
        raise SystemExit("build_packages: {0} lacks {1}".format(label, ", ".join(gone)))


def build_npm(out: Path, exe: Path, version: str, dry_run: bool) -> List[Path]:
    npm = shutil.which("npm") or "npm"
    node = shutil.which("node") or "node"
    out.mkdir(parents=True, exist_ok=True)
    _run([npm, "run", "build"], CLIENT_TS)
    _run([node, "scripts/build.mjs", "--exe", exe], NPM_RUNTIME)
    built = []
    for pkg, required in ((CLIENT_TS, TS_REQUIRED), (NPM_RUNTIME, RUNTIME_REQUIRED)):
        meta = json.loads(_run([npm, "pack", "--ignore-scripts", "--json", "--pack-destination", out], pkg))[0]
        tarball = out / meta["filename"]
        if meta["version"] != version:
            raise SystemExit("build_packages: {0} is {1}, the release is {2}".format(meta["name"], meta["version"],
                                                                                     version))
        _check(tarball.name, tarball_files(tarball), required)
        if dry_run:
            _run([npm, "publish", "--dry-run", "--access", "public", tarball], REPO_ROOT)
        built.append(tarball)
    return built


def build_python(out: Path, version: str) -> List[Path]:
    out.mkdir(parents=True, exist_ok=True)
    py = sys.executable
    # Builds the sdist, then the wheel from it (isolated environments).
    _run([py, "-m", "build", "--outdir", out, CLIENT_PY], REPO_ROOT)
    wheels = sorted(out.glob("sonara_client-{0}-*.whl".format(version)))
    sdists = sorted(out.glob("sonara_client-{0}.tar.gz".format(version)))
    if len(wheels) != 1 or len(sdists) != 1:
        raise SystemExit("build_packages: expected one {0} wheel and sdist in {1}, found {2}".format(
            version, out, sorted(p.name for p in out.iterdir())))
    _check(wheels[0].name, wheel_files(wheels[0]), WHEEL_REQUIRED)
    _check(wheels[0].name, [n.rsplit("/", 1)[-1] for n in wheel_files(wheels[0]) if ".dist-info/" in n],
           ("LICENSE", "METADATA"))
    _check(sdists[0].name, sdist_files(sdists[0]), SDIST_REQUIRED)
    _run([py, "-m", "twine", "check", "--strict", wheels[0], sdists[0]], REPO_ROOT)
    return [wheels[0], sdists[0]]


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="Build and check the packages release.yml publishes (#279).")
    ap.add_argument("--out", default=str(REPO_ROOT / "target" / "packages"), help="output folder (emptied first)")
    ap.add_argument("--exe", default=str(REPO_ROOT / "target" / "release" / "sonarad.exe"),
                    help="the release sonarad.exe, with runtime_dlls.py's files next to it")
    ap.add_argument("--publish-dry-run", action="store_true", help="also run npm publish --dry-run on the tarballs")
    args = ap.parse_args(argv)

    out = Path(args.out).resolve()
    exe = Path(args.exe).resolve()
    if not exe.is_file():
        raise SystemExit("build_packages: {0} not found: cargo build -p sonarad --release".format(exe))
    version = _load_bump_version().release_version(REPO_ROOT)
    shutil.rmtree(out, ignore_errors=True)
    built = build_npm(out / "npm", exe, version, args.publish_dry_run)
    built += build_python(out / "python", version)
    for path in built:
        print(path.relative_to(out).as_posix())
    return 0


if __name__ == "__main__":
    sys.exit(main())
