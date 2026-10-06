"""Embedder end-to-end suite (#274): apps that use Sonara for speech, as a
developer would build them, against the release runtime.

- ``test_node_host.py``: the npm packages, packed and installed into an
  empty project.
- ``test_python_host.py``: the ``sonara-client`` wheel in a fresh venv,
  with ``sonarad.exe`` from the release zip.
- ``test_raw_host.py``: a client written only from docs/protocol-v1.md and
  docs/bundling.md (TCP JSON lines, HTTP and SSE).
- ``test_release_zip.py``: the latest GitHub release, downloaded and
  verified (opt-in: ``SONARA_EMBED_RELEASE=1``, needs ``gh`` and network).

Every run uses a temp ``SONARA_HOME`` and ``sonarad --output wav:<dir>``, so
the tests check the audio itself. The real-voice tests need Kokoro's model
(``SONARA_KOKORO_MODELS``, else the one in ``%LOCALAPPDATA%\\Sonara``, only
read) and ``onnxruntime.dll`` next to the runtime; without them the Kokoro
test skips and OneCore still runs. Skipped without a release build
(``cargo build -p sonarad -p sonara-hook -p sonara-cli --release``).
"""
from __future__ import annotations

import shutil
import tempfile
from pathlib import Path

import pytest

from . import support


@pytest.fixture(scope="session")
def runtime() -> Path:
    exe = support.find_runtime()
    if exe is None:
        pytest.skip("no release sonarad.exe (cargo build -p sonarad --release) and SONARAD not set")
    return exe


@pytest.fixture(scope="session")
def node_app(runtime):
    """An empty Node project with the packed packages installed and the
    host app; the runtime package is built from ``runtime``."""
    from . import npm_app

    if not (npm_app.NODE and npm_app.NPM):
        pytest.skip("node and npm are needed")
    npm_app.build_packages(runtime)
    d = Path(tempfile.mkdtemp(prefix="sonara-embed-npm-"))
    yield npm_app.pack_and_install(d)
    shutil.rmtree(d, ignore_errors=True)


@pytest.fixture
def work():
    """A temp folder for one test (its home, WAV files and app), removed after."""
    d = Path(tempfile.mkdtemp(prefix="sonara-embed-"))
    yield d
    support.wait_runtime_gone(d / "home")
    shutil.rmtree(d, ignore_errors=True)


@pytest.fixture
def provider():
    p = support.FakeProvider()
    yield p
    p.close()


def real_engine_or_skip(engine: str, runtime_dir: Path) -> None:
    if engine == "kokoro":
        if not (runtime_dir / "onnxruntime.dll").is_file():
            pytest.skip("no onnxruntime.dll next to the runtime (python packaging/runtime_dlls.py stage ...)")
        if support.kokoro_models() is None:
            pytest.skip("Kokoro's model is not on this PC (set SONARA_KOKORO_MODELS)")


def prepare_voice_home(engine: str, home: Path) -> None:
    home.mkdir(parents=True, exist_ok=True)
    if engine == "kokoro":
        support.seed_kokoro(home, support.kokoro_models())


def unavailable_skips_only_onecore(report: dict, engine: str) -> None:
    """A host found ``engine`` unavailable. OneCore may lack voices on a PC
    (a skip). Kokoro passed ``real_engine_or_skip`` (its DLL and model are
    there), so unavailable means the runtime could not load them: a
    packaging regression, a failure."""
    if not report.get("unavailable"):
        return
    if engine == "onecore":
        pytest.skip(f"onecore cannot speak on this PC: {report['engine_status']}")
    pytest.fail(f"{engine} is unavailable although its DLL and model are there: {report['engine_status']}")
