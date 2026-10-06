"""The npm side of the embedder suite (#274): build ``@sonara/client`` and
``@sonara/runtime-win32-x64``, pack them as ``npm publish`` would and install
the tarballs into an empty project with a copy of ``hosts/node_host.mjs``."""
from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

from . import support

NODE = shutil.which("node")
NPM = shutil.which("npm")
CLIENT = support.REPO / "clients" / "ts"
NPM_RUNTIME = support.REPO / "packaging" / "npm-runtime"


def npm(args, cwd: Path) -> str:
    r = subprocess.run([NPM, *args], cwd=str(cwd), capture_output=True, text=True, encoding="utf-8")
    if r.returncode != 0:
        raise AssertionError("npm {0} failed:\n{1}\n{2}".format(" ".join(args), r.stdout[-3000:], r.stderr[-3000:]))
    return r.stdout.strip()


def build_packages(runtime: Path) -> None:
    """Build both packages from this checkout (never a stale dist/)."""
    if not (CLIENT / "node_modules").is_dir():
        npm(["ci"], CLIENT)
    npm(["run", "build"], CLIENT)
    # The runtime package from this runtime and the files staged next to it.
    r = subprocess.run([NODE, "scripts/build.mjs", "--exe", str(runtime)], cwd=str(NPM_RUNTIME),
                       capture_output=True, text=True)
    assert r.returncode == 0, r.stderr


def pack_and_install(dest: Path) -> Path:
    """Pack both packages and install the tarballs into a new project."""
    tarballs = []
    for pkg in (CLIENT, NPM_RUNTIME):
        out = npm(["pack", "--ignore-scripts", "--json", "--pack-destination", str(dest)], pkg)
        tarballs.append(dest / json.loads(out)[0]["filename"])
    app = dest / "app"
    app.mkdir()
    (app / "package.json").write_text(json.dumps({"name": "embed-node-app", "private": True, "type": "module"}))
    npm(["install", "--no-audit", "--no-fund", *[str(t) for t in tarballs]], app)
    shutil.copy(support.HOSTS / "node_host.mjs", app / "host.mjs")
    return app
