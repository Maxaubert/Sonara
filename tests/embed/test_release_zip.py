"""The released runtime as users and hosts get it (#274): the latest GitHub
release's ``sonara-runtime-win-x64-<version>.zip`` and ``SHA256SUMS``,
downloaded with ``gh``, checked, unpacked, and the Node host app (the
packed npm packages of this checkout) run against the ``sonarad.exe`` in it.

Opt-in, it needs the network and ``gh``: ``SONARA_EMBED_RELEASE=1``
(``SONARA_EMBED_RELEASE_TAG`` picks a release other than the latest). A
release older than 0.21.5 has no ``--output wav:``: it runs on the silent
null output, and the host's own checks (phases, state, the provider's
engine status) are what is checked."""
from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest

from . import support
from .conftest import prepare_voice_home, real_engine_or_skip

REPO_SLUG = "Maxaubert/Sonara"
GH = shutil.which("gh")
pytestmark = [
    pytest.mark.skipif(os.environ.get("SONARA_EMBED_RELEASE") != "1",
                       reason="opt-in: SONARA_EMBED_RELEASE=1 downloads the latest release with gh"),
    pytest.mark.skipif(not GH, reason="gh is needed"),
]


def version_tuple(v: str):
    return tuple(int(x) for x in v.split("-")[0].split("."))


@pytest.fixture(scope="module")
def released():
    d = Path(tempfile.mkdtemp(prefix="sonara-embed-release-"))
    tag = os.environ.get("SONARA_EMBED_RELEASE_TAG")
    argv = [GH, "release", "download", *([tag] if tag else []), "--repo", REPO_SLUG,
            "--pattern", "sonara-runtime-win-x64-*.zip", "--pattern", "SHA256SUMS", "--dir", str(d / "dl")]
    r = subprocess.run(argv, capture_output=True, text=True, timeout=600)
    assert r.returncode == 0, r.stderr
    zips = sorted((d / "dl").glob("sonara-runtime-win-x64-*.zip"))
    assert len(zips) == 1, zips
    folder = support.unpack_release(zips[0], d / "dl" / "SHA256SUMS", d / "app")
    version = zips[0].stem.rsplit("-", 1)[-1]
    yield {"dir": folder, "exe": folder / "sonarad.exe", "version": version}
    shutil.rmtree(d, ignore_errors=True)


def output_args(released, wav_dir: Path):
    if version_tuple(released["version"]) >= (0, 21, 5):
        return ["--output", "wav:" + str(wav_dir)]
    return ["--output", "null"]


def test_the_released_runtime_is_the_version_its_zip_names(released):
    r = subprocess.run([str(released["exe"]), "--version"], capture_output=True, text=True, timeout=30)
    assert r.returncode == 0 and released["version"] in r.stdout, (r.stdout, r.stderr)


def test_the_node_host_runs_the_whole_api_on_the_released_runtime(released, node_app, work, provider):
    wav_dir = work / "wav"
    args = ["--engine", "fake", "--keys", "fake", "--system", "fake", *output_args(released, wav_dir),
            "--idle-exit", "2"]
    env = support.host_env(work / "home", args, "scenario", provider, SONARA_EMBED_RUNTIME=str(released["exe"]))
    report = support.run_host([support.which_node(), "host.mjs"], env, node_app)
    assert report["version"] == released["version"]
    if "wav:" in args[-3]:
        support.check_scenario(report, wav_dir, provider)
    else:
        # The provider was asked for the item's text, with the key.
        said = [q["body"].get("input") for q in provider.speech_requests()]
        assert report["texts"]["provider"] in said, said


@pytest.mark.parametrize("engine", ["kokoro", "onecore"])
def test_the_released_runtime_speaks_with_a_real_voice(released, node_app, work, engine):
    real_engine_or_skip(engine, released["dir"])
    home = work / "home"
    prepare_voice_home(engine, home)
    wav_dir = work / "wav"
    args = ["--engine", engine, "--keys", "fake", "--system", "fake", *output_args(released, wav_dir),
            "--idle-exit", "2"]
    env = support.host_env(home, args, "voice", SONARA_EMBED_RUNTIME=str(released["exe"]))
    report = support.run_host([support.which_node(), "host.mjs"], env, node_app)
    if report.get("unavailable"):
        pytest.skip(f"{engine} cannot speak on this PC: {report['engine_status']}")
    if "wav:" in args[-3]:
        support.check_real_voice(report, wav_dir, engine)
    else:
        if engine == "onecore" and report["phase"] == "failed":
            # Before 0.21.5 (#274) OneCore said ready with no usable voices.
            pytest.xfail(f"{released['version']} reports OneCore ready while it cannot speak here")
        assert report["phase"] == "finished" and report["engine_status"]["ready"] is True, report
        assert report["engine_status"]["engine"] == engine, report
