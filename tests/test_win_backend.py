from __future__ import annotations
from sonara.platform import base


def test_make_windows_backend_full_bundle():
    from sonara.platform.windows import make_backend
    pb = make_backend()
    assert isinstance(pb, base.PlatformBackend)
    for part, cls in [(pb.tts, base.TtsBackend), (pb.earcon, base.EarconBackend),
                      (pb.hotkey, base.HotkeyBackend), (pb.supervisor, base.SupervisorBackend)]:
        assert isinstance(part, cls)


def test_hotkey_backend_is_real_not_stub():
    from sonara.platform.windows.hotkeys import WinHotkeyBackend
    hk = WinHotkeyBackend()
    # M3: real in-process backend -- keytables + display labels, no "M3 deferred".
    assert hk.default_mods() == ["ctrl", "alt"]
    assert hk.display_combo(0x0002, 0x53) == "Ctrl+S"


def test_display_combo_labels_ctrl_alt_o():
    from sonara.platform.windows.hotkeys import WinHotkeyBackend
    # MSDN RegisterHotKey modifier bits: 0x0001=Alt, 0x0002=Ctrl, 0x0004=Shift
    # 0x0003 == Ctrl | Alt; VK 0x4F == 'O'
    assert WinHotkeyBackend().display_combo(0x0003, 0x4F) == "Ctrl+Alt+O"


def test_get_platform_win32(monkeypatch):
    import sonara.platform as platform
    monkeypatch.setattr(platform.sys, "platform", "win32")
    platform._CACHE = None
    pb = platform.get_platform()
    assert isinstance(pb, base.PlatformBackend)
    platform._CACHE = None
