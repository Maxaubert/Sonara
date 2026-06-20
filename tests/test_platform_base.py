import abc
import pytest
from sonara.platform import base


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


def test_macos_hotkey_exposes_keytables_and_default_mods():
    from sonara.platform.macos.hotkeys import MacHotkeyBackend
    hk = MacHotkeyBackend()
    assert hk.key_codes()["s"] == 1 and hk.mod_masks()["cmd"] == 256
    assert hk.default_mods() == ["ctrl", "cmd"]


def test_base_hotkey_lifecycle_defaults_are_noops():
    from sonara.platform.macos.hotkeys import MacHotkeyBackend
    hk = MacHotkeyBackend()
    hk.start(lambda msg: None)   # macOS: hotkeyd is a separate process -> no-op
    hk.stop()
    assert hk.doctor_rows() == []
