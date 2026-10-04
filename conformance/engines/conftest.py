"""Fixtures of the external engine tests: a fake OpenAI-compatible server
(``fake_openai``) and its profile. The runtime runs with ``--keys fake``
(see ``harness``), so keys go to ``<home>/fake-keys.json``."""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from fake_openai import FakeOpenAI  # noqa: E402


@pytest.fixture
def provider():
    p = FakeOpenAI()
    yield p
    p.stop()


@pytest.fixture
def profile(provider):
    """A local Kokoro-FastAPI-like profile that still takes a key, so the
    tests see the Authorization header."""
    return {
        "id": "local",
        "kind": "openai-compatible",
        "label": "Test server",
        "url": provider.url,
        "key_ref": "credman",
        "options": {"preset": "kokoro-fastapi", "timeout_ms": 5000},
    }
