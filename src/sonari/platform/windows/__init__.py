from __future__ import annotations

from sonari.platform.base import PlatformBackend
from sonari.platform.windows.tts import WinTtsBackend
from sonari.platform.windows.earcon import WinEarconBackend
from sonari.platform.windows.hotkeys import WinHotkeyBackend
from sonari.platform.windows.supervisor import WinSupervisorBackend


def make_backend() -> PlatformBackend:
    return PlatformBackend(
        tts=WinTtsBackend(),
        earcon=WinEarconBackend(),
        hotkey=WinHotkeyBackend(),
        supervisor=WinSupervisorBackend(),
    )
