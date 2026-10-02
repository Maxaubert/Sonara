"""Fixtures for the protocol v1 conformance suite.

Run after building the runtime:

    cargo build -p sonarad
    python -m pytest conformance -q

Set ``SONARAD`` to test another build of ``sonarad.exe``.
"""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import harness  # noqa: E402


@pytest.fixture(scope="session")
def sonarad_exe() -> Path:
    exe = harness.find_sonarad()
    if exe is None or not exe.is_file():
        pytest.exit(
            "sonarad.exe not found: run 'cargo build -p sonarad' or set SONARAD",
            returncode=4,
        )
    return exe


@pytest.fixture
def start(sonarad_exe, tmp_path):
    """Start sonarad processes; all are killed at teardown."""
    started = []

    def _start(*args: str, home: Path | None = None, wait: bool = True) -> harness.Runtime:
        rt = harness.Runtime(sonarad_exe, home or (tmp_path / "home"), *args, wait=wait)
        started.append(rt)
        return rt

    yield _start
    for rt in started:
        rt.close()


@pytest.fixture
def rt(start) -> harness.Runtime:
    """A running sonarad on a fresh home."""
    return start()


@pytest.fixture
def client(rt):
    """A TCP client that completed hello."""
    c = rt.tcp()
    yield c
    c.close()
