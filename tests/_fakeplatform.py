"""A fake PlatformBackend for cli dispatch tests.

After the seam refactor, cli.install/uninstall/doctor delegate every OS-specific
step to get_platform(). These fakes let the cli tests assert the *dispatch
contract* (what cli calls, in what order) independently of any real OS backend.
The OS mechanics themselves are tested against the real backends in
test_macos_supervisor / test_win_supervisor / test_macos_hotkeys.
"""
import types


class FakeSupervisor:
    def __init__(self, python="PYEXE", rows=None, hooks_row=None):
        self.calls = []
        self._py = python
        self._rows = rows if rows is not None else [("os-row", True, "ok")]
        self._hooks_row = hooks_row or ("hooks installed", True, "ok")

    def resolve_python(self):
        return self._py

    def _probe_python_version(self, p):
        return (3, 12)

    def install(self, py, app):
        self.calls.append(("install", py, app))

    def uninstall(self):
        self.calls.append(("uninstall",))

    def post_install_notes(self):
        self.calls.append(("notes",))
        print("Run 'sonari doctor' to confirm everything is green.")

    def doctor_rows(self):
        return list(self._rows)

    def hooks_doctor_row(self):
        return self._hooks_row


class FakeHotkey:
    def __init__(self, ok=True, detail="ok"):
        self.calls = []
        self._ok = ok
        self._detail = detail

    def install(self, **kwargs):
        self.calls.append(("install", kwargs))
        return (self._ok, self._detail)

    def uninstall(self):
        self.calls.append(("uninstall",))

    def display_combo(self, modifiers, key_code):
        return "Ctrl+Cmd+O"

    def doctor_rows(self):
        return []


class FakeTts:
    def __init__(self, voice="Aria"):
        self._voice = voice

    def best_voice(self):
        return self._voice


def fake_platform(supervisor=None, hotkey=None, tts=None):
    return types.SimpleNamespace(
        supervisor=supervisor or FakeSupervisor(),
        hotkey=hotkey or FakeHotkey(),
        tts=tts or FakeTts(),
        earcon=None,
    )
