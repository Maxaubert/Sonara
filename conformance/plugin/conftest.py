"""Fixtures of the plugin suite (#202). Needs
``cargo build -p sonarad -p sonara-hook -p sonara-cli``, Git Bash and
Windows PowerShell."""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import plugin_harness as ph  # noqa: E402


@pytest.fixture(scope="session")
def exes() -> Path:
    d = ph.build_dir()
    if d is None:
        pytest.fail("sonarad.exe, sonara-hook.exe and sonara.exe not built side by side: "
                    "run 'cargo build -p sonarad -p sonara-hook -p sonara-cli'")
    return d


@pytest.fixture(scope="session")
def bash() -> Path:
    b = ph.find_bash()
    if b is None:
        pytest.fail("Git Bash not found (Claude Code on Windows runs hooks through it)")
    return b


@pytest.fixture
def box(tmp_path, exes, bash):
    b = ph.Box(tmp_path)
    yield b
    b.stop_runtime(exes)


@pytest.fixture
def releases():
    started = []

    def _serve(files, delay=0.0, hold=False):
        r = ph.Releases(files, delay, hold)
        started.append(r)
        return r

    yield _serve
    for r in started:
        r.close()
