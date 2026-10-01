"""Live OneCore checks: real WinRT synthesis and voice listing on this machine.

Opt-in only (E22): skipped unless selected with `python -m pytest -m live_windows`.
The regular suite exercises WinTtsBackend against tests/_winfakes.py instead.
"""
from __future__ import annotations

import sys

import pytest

pytestmark = [
    pytest.mark.live_windows,
    pytest.mark.skipif(sys.platform != "win32", reason="needs real Windows"),
]


def test_live_onecore_lists_voices():
    from sonara.platform.windows.tts import WinTtsBackend
    assert WinTtsBackend().list_voices()


def test_live_onecore_synthesizes_a_wav():
    from sonara.platform.windows.tts import WinTtsBackend
    b = WinTtsBackend()
    data = b._synthesize_wav("Sonara live check.", None, 200)
    assert data[:4] == b"RIFF"
