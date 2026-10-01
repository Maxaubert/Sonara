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


class _BareHotkey(base.HotkeyBackend):
    """Minimal concrete backend exercising only the portable base defaults."""
    def install(self): return (True, "")
    def uninstall(self): return None
    def display_combo(self, modifiers, key_code): return ""


def test_base_hotkey_keytable_defaults_are_empty():
    hk = _BareHotkey()
    assert hk.key_codes() == {}
    assert hk.mod_masks() == {}
    assert hk.default_mods() == []


def test_base_hotkey_lifecycle_defaults_are_noops():
    hk = _BareHotkey()
    hk.start(lambda msg: None)   # base default -> no-op
    hk.stop()
    assert hk.doctor_rows() == []


def test_base_hotkey_reports_no_altgr_conflicts():
    assert _BareHotkey().altgr_conflicts([{"action": "mute"}]) == []


def test_recover_audio_sweeps_the_ducker_and_the_pauser():
    calls = []

    class _Part:
        def __init__(self, name):
            self.name = name

        def recover(self):
            calls.append(self.name)

    pb = base.PlatformBackend(tts=None, earcon=None, hotkey=None, supervisor=None,
                              ducker=_Part("duck"), pauser=_Part("pause"))
    pb.recover_audio()
    assert calls == ["duck", "pause"]


def test_recover_audio_tolerates_missing_parts():
    base.PlatformBackend(tts=None, earcon=None, hotkey=None,
                         supervisor=None).recover_audio()     # no ducker/pauser
    base.PlatformBackend(tts=None, earcon=None, hotkey=None, supervisor=None,
                         ducker=base.NullDucker(),
                         pauser=base.NullPauser()).recover_audio()


def test_tts_prewarm_defaults_to_a_noop():
    class _Tts(base.TtsBackend):
        def run(self, text, voice, rate, on_play=None): return None
        def best_voice(self): return "x"
        def list_voices(self): return []
    assert _Tts().prewarm(200) is None
