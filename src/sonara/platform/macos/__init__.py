from sonara.platform.base import PlatformBackend
from sonara.platform.macos.tts import MacTtsBackend
from sonara.platform.macos.earcon import MacEarconBackend
from sonara.platform.macos.hotkeys import MacHotkeyBackend
from sonara.platform.macos.supervisor import MacSupervisorBackend


def make_backend() -> PlatformBackend:
    return PlatformBackend(
        tts=MacTtsBackend(),
        earcon=MacEarconBackend(),
        hotkey=MacHotkeyBackend(),
        supervisor=MacSupervisorBackend(),
    )
