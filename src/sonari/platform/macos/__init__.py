from sonari.platform.base import PlatformBackend
from sonari.platform.macos.tts import MacTtsBackend
from sonari.platform.macos.earcon import MacEarconBackend
from sonari.platform.macos.hotkeys import MacHotkeyBackend
from sonari.platform.macos.supervisor import MacSupervisorBackend


def make_backend() -> PlatformBackend:
    return PlatformBackend(
        tts=MacTtsBackend(),
        earcon=MacEarconBackend(),
        hotkey=MacHotkeyBackend(),
        supervisor=MacSupervisorBackend(),
    )
