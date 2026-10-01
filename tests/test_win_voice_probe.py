"""Doctor's Windows voice row (D7): it synthesizes for real instead of reading
a registry key, and says exactly what is wrong when the listed OneCore voices
cannot speak (their voice data is missing on some PCs: FileNotFoundError)."""
from __future__ import annotations

from sonara.platform.windows import tts as wtts


class _V:
    def __init__(self, name):
        self.display_name = name


class _Backend:
    def __init__(self, names, err=None):
        self._names = names
        self._err = err

    def _all_voice_infos(self):
        return [_V(n) for n in self._names]

    def _synthesize_wav(self, text, voice, rate):
        if self._err is not None:
            raise self._err
        return b"RIFF...."


def test_probe_ok_names_the_voice():
    ok, detail = wtts.probe_windows_voice(_Backend(["Microsoft David"]))
    assert ok is True
    assert "Microsoft David" in detail


def test_probe_reports_missing_voice_data_and_the_fix():
    err = FileNotFoundError("[WinError -2147024894] The system cannot find the file specified.")
    ok, detail = wtts.probe_windows_voice(
        _Backend(["Microsoft David", "Microsoft Zira"], err=err))
    assert ok is False
    low = detail.lower()
    assert "david" in low and "voice data" in low
    assert "language.texttospeech" in low          # the exact repair command
    assert "speech" in low and "voices" in low     # the Settings path


def test_probe_with_no_voices_says_how_to_add_one():
    ok, detail = wtts.probe_windows_voice(_Backend([]))
    assert ok is False and "add voices" in detail.lower()


def test_probe_other_errors_are_reported_not_raised():
    ok, detail = wtts.probe_windows_voice(_Backend(["X"], err=RuntimeError("boom")))
    assert ok is False and "boom" in detail


def test_supervisor_row_uses_the_synthesis_probe(monkeypatch):
    from sonara.platform.windows.supervisor import WinSupervisorBackend
    sup = WinSupervisorBackend()
    monkeypatch.setattr(sup, "_schtasks", lambda args: 0)
    monkeypatch.setattr(sup, "resolve_python", lambda: r"C:\Py\pythonw.exe")
    monkeypatch.setattr("sonara.paths.socket_connectable", lambda: True)
    monkeypatch.setattr(wtts, "probe_windows_voice",
                        lambda backend=None: (False, "voice data missing"))
    rows = {r[0]: r for r in sup.doctor_rows()}
    assert rows["Windows voice"][1:] == (False, "voice data missing")
    assert "neural voice" not in rows        # the registry-read row is gone
