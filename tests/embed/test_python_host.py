"""A Python app on the ``sonara-client`` wheel (#274): the wheel is built
from clients/python as it would be published, installed into a fresh venv
(``uv venv`` when uv is there, else ``python -m venv``), and
``hosts/python_host.py`` runs with that venv's Python. As docs/bundling.md
says, the app ships ``sonarad.exe`` from the release zip: the zip is built
with packaging/release_zip.py from the runtime under test, checked against
its SHA256SUMS and unpacked, and ``SONARA_RUNTIME`` names the
``sonarad.exe`` in it."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

import pytest

from . import support
from .conftest import prepare_voice_home, real_engine_or_skip

CLIENT = support.REPO / "clients" / "python"
# SONARA_EMBED_VENV=venv uses pip and python -m venv even where uv is (as on CI).
UV = None if os.environ.get("SONARA_EMBED_VENV") == "venv" else shutil.which("uv")


def run(argv, **kw) -> str:
    r = subprocess.run([str(a) for a in argv], capture_output=True, text=True, **kw)
    if r.returncode != 0:
        raise AssertionError("{0} failed:\n{1}\n{2}".format(argv, r.stdout[-3000:], r.stderr[-3000:]))
    return r.stdout


def build_wheel(out: Path) -> Path:
    # A copy, so the build leaves nothing (build/, *.egg-info) in the repo.
    src = out / "src-copy"
    shutil.copytree(CLIENT, src, ignore=shutil.ignore_patterns("tests", "build", "dist", "*.egg-info",
                                                               "__pycache__", ".pytest_cache"))
    if UV:
        run([UV, "build", "--wheel", "--out-dir", out, src])
    else:
        run([sys.executable, "-m", "pip", "wheel", "--no-deps", "--wheel-dir", out, src])
    wheels = sorted(out.glob("sonara_client-*.whl"))
    assert len(wheels) == 1, wheels
    return wheels[0]


def fresh_venv(where: Path, wheel: Path) -> Path:
    if UV:
        run([UV, "venv", "--quiet", "--python", sys.executable, where])
        python = where / "Scripts" / "python.exe"
        run([UV, "pip", "install", "--quiet", "--python", python, wheel])
    else:
        run([sys.executable, "-m", "venv", where])
        python = where / "Scripts" / "python.exe"
        run([python, "-m", "pip", "install", "--quiet", "--no-index", wheel])
    return python


@pytest.fixture(scope="module")
def python_app(runtime):
    d = Path(tempfile.mkdtemp(prefix="sonara-embed-py-"))
    wheel = build_wheel(d / "dist")
    python = fresh_venv(d / "venv", wheel)
    out = run([sys.executable, str(support.REPO / "packaging" / "release_zip.py"), "--exe", runtime,
               "--out", d / "zip"])
    zip_path = Path(out.strip().splitlines()[-1])
    runtime_dir = support.unpack_release(zip_path, d / "zip" / "SHA256SUMS", d / "app")
    shutil.copy(support.HOSTS / "python_host.py", d / "app" / "host.py")
    yield {"python": python, "wheel": wheel, "runtime": runtime_dir / "sonarad.exe", "app": d / "app"}
    shutil.rmtree(d, ignore_errors=True)


def test_the_wheel_ships_the_client_and_nothing_else(python_app):
    wheel = python_app["wheel"]
    with zipfile.ZipFile(wheel) as z:
        names = z.namelist()
    mods = {n for n in names if n.startswith("sonara_client/")}
    for m in ("__init__.py", "client.py", "connect.py", "connection.py", "discovery.py", "engines.py",
              "errors.py", "extensions.py", "version.py"):
        assert "sonara_client/" + m in mods, m
    assert not any(n.startswith(("tests/", "test_")) for n in names), names
    assert any(n.endswith("LICENSE") for n in names), names
    # Installed from the wheel, not this checkout: the venv imports its copy.
    where = run([python_app["python"], "-c", "import sonara_client, json; print(json.dumps("
                 "[sonara_client.__file__, sonara_client.__version__]))"], cwd=str(python_app["app"]))
    path, installed = json.loads(where)
    assert "site-packages" in path and str(support.REPO) not in path, path
    assert wheel.name.startswith("sonara_client-" + installed + "-"), (wheel.name, installed)


def test_a_python_app_uses_the_whole_api_and_its_audio_is_real(python_app, work, provider):
    wav_dir = work / "wav"
    env = support.host_env(work / "home", support.scenario_args(wav_dir), "scenario", provider,
                           SONARA_RUNTIME=str(python_app["runtime"]))
    report = support.run_host([str(python_app["python"]), "host.py"], env, python_app["app"])
    support.check_scenario(report, wav_dir, provider)


@pytest.mark.parametrize("engine", ["kokoro", "onecore"])
def test_a_python_app_speaks_with_a_real_voice_from_the_release_zip(python_app, work, engine):
    real_engine_or_skip(engine, python_app["runtime"].parent)
    home = work / "home"
    prepare_voice_home(engine, home)
    wav_dir = work / "wav"
    env = support.host_env(home, support.voice_args(engine, wav_dir), "voice",
                           SONARA_RUNTIME=str(python_app["runtime"]))
    report = support.run_host([str(python_app["python"]), "host.py"], env, python_app["app"])
    if report.get("unavailable"):
        pytest.skip(f"{engine} cannot speak on this PC: {report['engine_status']}")
    support.check_real_voice(report, wav_dir, engine)
