from __future__ import annotations

from sonara.platform.base import PlatformBackend
from sonara.platform.windows.tts import WinTtsBackend
from sonara.platform.windows.earcon import WinEarconBackend
from sonara.platform.windows.hotkeys import WinHotkeyBackend
from sonara.platform.windows.supervisor import WinSupervisorBackend
from sonara.platform.windows.ducking import AudioDucker


def make_backend() -> PlatformBackend:
    return PlatformBackend(
        tts=WinTtsBackend(),
        earcon=WinEarconBackend(),
        hotkey=WinHotkeyBackend(),
        supervisor=WinSupervisorBackend(),
        ducker=AudioDucker(),
    )
