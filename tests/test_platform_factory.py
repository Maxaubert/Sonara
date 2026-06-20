import sonara.platform as platform
from sonara.platform import base


def test_get_platform_returns_macos_backend_on_darwin(monkeypatch):
    monkeypatch.setattr(platform.sys, "platform", "darwin")
    platform._CACHE = None
    pb = platform.get_platform()
    assert isinstance(pb, base.PlatformBackend)


def test_get_platform_rejects_unknown_os(monkeypatch):
    monkeypatch.setattr(platform.sys, "platform", "sunos5")
    platform._CACHE = None
    import pytest
    with pytest.raises(RuntimeError):
        platform.get_platform()
