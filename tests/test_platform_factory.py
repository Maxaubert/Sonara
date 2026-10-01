import sonara.platform as platform
from sonara.platform import base


def test_get_platform_returns_windows_backend_on_win32(monkeypatch):
    monkeypatch.setattr(platform.sys, "platform", "win32")
    platform._CACHE = None
    pb = platform.get_platform()
    assert isinstance(pb, base.PlatformBackend)


def test_get_platform_rejects_non_win32(monkeypatch):
    monkeypatch.setattr(platform.sys, "platform", "darwin")
    platform._CACHE = None
    import pytest
    with pytest.raises(RuntimeError):
        platform.get_platform()


def test_daemon_process_is_the_windows_process_module(monkeypatch):
    # The daemon's process setup and single-instance guard come through the
    # seam, and the guard is the Windows singleton module's (audit section 2).
    from sonara.platform.windows import process, singleton
    monkeypatch.setattr(platform.sys, "platform", "win32")
    mod = platform.daemon_process()
    assert mod is process
    assert mod.acquire_singleton_mutex is singleton.acquire_singleton_mutex
    assert mod.acquire_singleton is singleton.acquire_singleton


def test_daemon_process_rejects_non_win32(monkeypatch):
    import pytest
    monkeypatch.setattr(platform.sys, "platform", "darwin")
    with pytest.raises(RuntimeError):
        platform.daemon_process()


def test_transport_is_os_free():
    # The Win32 / msvcrt single-instance code moved under platform/windows.
    import inspect
    from sonara.platform import transport
    src = inspect.getsource(transport)
    for word in ("msvcrt", "ctypes", "kernel32", "os.name"):
        assert word not in src, word
