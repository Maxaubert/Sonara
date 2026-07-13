"""Named-mutex single-instance guard (Windows). The old byte-lock was tied to the
lock FILE's inode, so a deleted/recreated file or racing starts stopped excluding
and daemons piled up. A named kernel mutex is keyed by name, immune to that."""
import os

import pytest

from sonara.platform import transport


@pytest.mark.skipif(os.name != "nt", reason="named mutex is Windows-only")
def test_named_mutex_excludes_second_acquire():
    name = "Local\\Sonara-test-" + str(os.getpid())
    h1 = transport.acquire_singleton_mutex(name)
    assert h1                                    # first owner gets a handle
    try:
        assert transport.acquire_singleton_mutex(name) is None   # second excluded
    finally:
        transport.release_singleton_mutex(h1)


@pytest.mark.skipif(os.name != "nt", reason="named mutex is Windows-only")
def test_named_mutex_reacquired_after_release():
    name = "Local\\Sonara-test-reacq-" + str(os.getpid())
    h1 = transport.acquire_singleton_mutex(name)
    assert h1
    transport.release_singleton_mutex(h1)        # last handle closed -> mutex gone
    h2 = transport.acquire_singleton_mutex(name)
    assert h2                                     # ownable again
    transport.release_singleton_mutex(h2)


def test_non_windows_returns_sentinel(monkeypatch):
    # Off Windows the API is absent; the guard must not gate on it (byte-lock
    # remains the guard there), so a truthy sentinel is returned.
    monkeypatch.setattr(os, "name", "posix")
    assert transport.acquire_singleton_mutex("whatever") is True
