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


# ---------------------------------------------------------------------------
# M11: scoped to the user, and a failure is told apart from "already owned"
# ---------------------------------------------------------------------------

def test_mutex_name_is_scoped_to_this_users_sonara_dir():
    a = transport.singleton_mutex_name(r"C:\Users\alice\.sonara")
    b = transport.singleton_mutex_name(r"C:\Users\bob\.sonara")
    assert a != b
    assert a == transport.singleton_mutex_name(r"c:\users\ALICE\.sonara")
    assert "\\" not in a.split("\\", 1)[1]        # one namespace prefix only


def test_default_mutex_name_follows_paths(monkeypatch, tmp_path):
    from sonara import paths
    monkeypatch.setattr(paths, "SONARA_DIR", tmp_path / "x")
    assert transport.default_mutex_name() == transport.singleton_mutex_name(tmp_path / "x")


class _K32:
    def __init__(self, handle):
        self._handle = handle
        self.closed = []

    def CreateMutexW(self, sec, owner, name):
        return self._handle

    def CloseHandle(self, h):
        self.closed.append(h)


def test_create_failure_raises_instead_of_reading_as_owned(monkeypatch):
    monkeypatch.setattr(os, "name", "nt")
    with pytest.raises(OSError) as ei:
        transport.acquire_singleton_mutex(
            "n", kernel32=_K32(0), last_error=lambda: 5)
    assert "5" in str(ei.value)


def test_already_exists_returns_none_and_closes(monkeypatch):
    monkeypatch.setattr(os, "name", "nt")
    k = _K32(1234)
    assert transport.acquire_singleton_mutex(
        "n", kernel32=k, last_error=lambda: 183) is None
    assert k.closed


def test_owner_gets_the_handle(monkeypatch):
    monkeypatch.setattr(os, "name", "nt")
    assert transport.acquire_singleton_mutex(
        "n", kernel32=_K32(99), last_error=lambda: 0) == 99
