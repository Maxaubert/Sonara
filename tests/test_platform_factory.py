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
