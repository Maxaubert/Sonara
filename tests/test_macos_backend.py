# tests/test_macos_backend.py
from sonara.platform.macos import make_backend
from sonara.platform import base


def test_make_backend_returns_full_bundle():
    pb = make_backend()
    assert isinstance(pb, base.PlatformBackend)
    assert isinstance(pb.tts, base.TtsBackend)
    assert isinstance(pb.earcon, base.EarconBackend)
    assert isinstance(pb.hotkey, base.HotkeyBackend)
    assert isinstance(pb.supervisor, base.SupervisorBackend)
