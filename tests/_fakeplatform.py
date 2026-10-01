"""A fake PlatformBackend for install dispatch tests.

sonara.install (install/uninstall/doctor) delegates every OS-specific step to
get_platform(). These fakes let the tests assert the *dispatch contract* (what
the installer calls, in what order) independently of any real OS backend.
The OS mechanics themselves are tested against the real backends in
test_win_supervisor / test_win_hotkeys.
"""
import types


class FakeSupervisor:
    def __init__(self, python="PYEXE", rows=None, hooks_row=None):
        self.calls = []
        self.plugin_roots = []
        self._py = python
        self._rows = rows if rows is not None else [("os-row", True, "ok")]
        self._hooks_row = hooks_row or ("hooks installed", True, "ok")

    def resolve_python(self):
        return self._py

    def _probe_python_version(self, p):
        return (3, 12)

    def install(self, py, app, plugin_root=None):
        self.calls.append(("install", py, app))
        self.plugin_roots.append(plugin_root)

    def uninstall(self):
        self.calls.append(("uninstall",))

    def end_task(self):
        pass

    def kill_stray_daemons(self):
        return 0

    def post_install_notes(self):
        self.calls.append(("notes",))
        print("Run 'sonara doctor' to confirm everything is green.")

    def doctor_rows(self):
        return list(self._rows)

    def hooks_doctor_row(self):
        return self._hooks_row


class FakeHotkey:
    def __init__(self, ok=True, detail="ok"):
        self.calls = []
        self._ok = ok
        self._detail = detail

    def install(self):
        self.calls.append(("install",))
        return (self._ok, self._detail)

    def uninstall(self):
        self.calls.append(("uninstall",))

    def display_combo(self, modifiers, key_code):
        return "Ctrl+Shift+Alt+O"

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
