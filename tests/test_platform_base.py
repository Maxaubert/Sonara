import abc
import pytest
from sonari.platform import base


def test_backends_are_abstract():
    for cls in (base.TtsBackend, base.EarconBackend,
                base.HotkeyBackend, base.SupervisorBackend):
        assert issubclass(cls, abc.ABC)
        with pytest.raises(TypeError):
            cls()  # cannot instantiate an ABC with abstract methods


def test_platform_backend_bundles_the_four():
    class _Tts(base.TtsBackend):
        def run(self, text, voice, rate): return None
        def best_voice(self): return "x"
        def list_voices(self): return []
    class _Ear(base.EarconBackend):
        def play(self, path): return None
        def default_earcons(self): return {}
    class _Hk(base.HotkeyBackend):
        def install(self): return (True, "")
        def uninstall(self): return None
        def display_combo(self, modifiers, key_code): return ""
    class _Sup(base.SupervisorBackend):
        def install(self, python, app_dir): return None
        def uninstall(self): return None
        def is_running(self): return False
        def is_installed(self): return False
        def resolve_python(self): return None
        def launch_spec(self): return ([], {})
        def doctor_rows(self): return []
    pb = base.PlatformBackend(tts=_Tts(), earcon=_Ear(),
                              hotkey=_Hk(), supervisor=_Sup())
    assert isinstance(pb.tts, base.TtsBackend)
    assert isinstance(pb.supervisor, base.SupervisorBackend)
