from __future__ import annotations

from sonara.platform.base import PlatformBackend


def make_backend() -> PlatformBackend:
    # Backend imports stay inside make_backend so importing a light helper such
    # as sonara.platform.windows.process (daemon main(), before the #29 VC
    # runtime preload) does not load every backend module with this package.
    from sonara.platform.windows.tts import WinTtsBackend
    from sonara.platform.windows.earcon import WinEarconBackend
    from sonara.platform.windows.hotkeys import WinHotkeyBackend
    from sonara.platform.windows.supervisor import WinSupervisorBackend
    from sonara.platform.windows.ducking import AudioDucker
    from sonara.platform.windows.pausing import MediaPauser
    return PlatformBackend(
        tts=WinTtsBackend(),
        earcon=WinEarconBackend(),
        hotkey=WinHotkeyBackend(),
        supervisor=WinSupervisorBackend(),
        ducker=AudioDucker(),
        pauser=MediaPauser(),
    )
