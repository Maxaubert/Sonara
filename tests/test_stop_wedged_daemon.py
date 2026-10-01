"""A daemon wedged under its lock still accepts connections but never shuts
down (#166). stop_sonara() must kill it after the SHUTDOWN grace, and every
command that deletes files under it must respect a failed stop instead of
deleting under a live daemon (a half-deleted venv plus a traceback)."""
import os

import pytest

from sonara import cli, paths
from sonara import kokoro_provision as kp
from sonara.install import app_copy, installer, service
from tests._fakeplatform import fake_platform, FakeSupervisor, FakeHotkey


class _WedgedSup:
    """A supervisor whose stray sweep is the only thing that ends the daemon."""

    def __init__(self, state, sweep_works=True):
        self._state = state
        self._works = sweep_works
        self.swept = 0

    def end_task(self):
        pass

    def kill_stray_daemons(self):
        self.swept += 1
        if self._works:
            self._state["alive"] = False
        return 1 if self._works else 0


def _wire_wedged(monkeypatch, tmp_path):
    state = {"alive": True, "t": 0.0}
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", tmp_path / "stopped")
    monkeypatch.setattr(paths, "ensure_sonara_dir", lambda: None)
    monkeypatch.setattr(paths, "socket_connectable", lambda: state["alive"])
    monkeypatch.setattr(service.time, "sleep", lambda s: None)

    def fake_time():
        state["t"] += 1.0
        return state["t"]
    monkeypatch.setattr(service.time, "time", fake_time)

    def wedged_send(msg, expect_reply=False):
        raise OSError("DaemonUnresponsive: no reply under the lock")
    monkeypatch.setattr(service, "_send", wedged_send)
    return state


def test_stop_sonara_kills_a_wedged_daemon_that_still_accepts(monkeypatch, tmp_path):
    state = _wire_wedged(monkeypatch, tmp_path)
    sup = _WedgedSup(state)
    assert service.stop_sonara(sup) is True
    assert sup.swept >= 1
    assert state["alive"] is False


def test_stop_sonara_fails_when_even_the_sweep_cannot_end_it(monkeypatch, tmp_path):
    state = _wire_wedged(monkeypatch, tmp_path)
    sup = _WedgedSup(state, sweep_works=False)
    assert service.stop_sonara(sup) is False
    assert sup.swept >= 1


def test_stray_sweep_also_matches_the_supervisor_loop():
    # E14: the supervisor loop restarts the daemon; a sweep that misses it
    # leaves a restarter behind.
    from sonara.platform.windows import supervisor
    seen = []

    class FakeProc:
        stdout = b""

    supervisor.kill_stray_daemons(runner=lambda argv: seen.append(argv) or FakeProc())
    script = seen[0][-1]
    assert "sonara[.]daemon" in script and "supervisor_loop" in script


def test_stray_sweep_matches_only_the_launched_supervisor_script():
    # The sweep force-kills by command line, so it must match the script the
    # scheduled task launches, not any python that mentions "supervisor_loop"
    # (a pytest -k run, a REPL, another tool).
    import re
    from sonara.platform.windows import supervisor
    seen = []

    class FakeProc:
        stdout = b""

    supervisor.kill_stray_daemons(runner=lambda argv: seen.append(argv) or FakeProc())
    pattern = re.search(r"-match '([^']+)'", seen[0][-1]).group(1)
    hits = [
        r'"C:\Py\pythonw.exe" "C:\Users\u\.sonara\app\sonara\platform\windows\supervisor_loop.py"',
        r"C:\Py\pythonw.exe -m sonara.daemon",
    ]
    misses = [
        "python -m pytest -k supervisor_loop",
        r"python -m pytest tests\test_supervisor_loop.py",
    ]
    for cmd in hits:
        assert re.search(pattern, cmd), cmd
    for cmd in misses:
        assert not re.search(pattern, cmd), cmd


# --- callers respect a failed stop -------------------------------------------

def _failed_stop(monkeypatch):
    def stop(sup=None):
        paths.ensure_sonara_dir()
        paths.STOPPED_SENTINEL_PATH.write_text("sonara shutdown")
        return False
    monkeypatch.setattr(service, "stop_sonara", stop)
    monkeypatch.setattr(paths, "socket_connectable", lambda: False)


def test_voices_uninstall_keeps_the_venv_when_the_daemon_will_not_stop(monkeypatch, capsys):
    _failed_stop(monkeypatch)
    monkeypatch.setattr(kp, "uninstall_kokoro",
                        lambda: pytest.fail("must not delete under a live daemon"))
    monkeypatch.setattr(installer, "install", lambda: pytest.fail("must not install"))
    assert cli._cmd_voices_uninstall(None) == 1
    assert "did not stop" in capsys.readouterr().err
    assert not paths.STOPPED_SENTINEL_PATH.exists()     # Sonara not left off


def test_voices_install_does_not_provision_under_a_live_daemon(monkeypatch, tmp_path, capsys):
    _failed_stop(monkeypatch)
    monkeypatch.setattr(paths, "package_root", lambda: os.path.join(str(tmp_path), "src"))
    monkeypatch.setattr(kp, "install_kokoro",
                        lambda pp: pytest.fail("must not provision under a live daemon"))
    monkeypatch.setattr(installer, "install", lambda: pytest.fail("must not install"))
    assert cli._cmd_voices_install(None) == 1
    assert "did not stop" in capsys.readouterr().err
    assert not paths.STOPPED_SENTINEL_PATH.exists()


def _plugin_env(monkeypatch, tmp_path):
    from sonara.install import deps
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    monkeypatch.setattr(deps, "winrt_importable", lambda python: True)
    sup = FakeSupervisor(python="/PY/pythonw.exe")
    monkeypatch.setattr("sonara.platform.get_platform",
                        lambda: fake_platform(supervisor=sup, hotkey=FakeHotkey()))
    return sup


def test_install_does_not_replace_the_app_under_a_live_daemon(monkeypatch, tmp_path, capsys):
    _plugin_env(monkeypatch, tmp_path)
    _failed_stop(monkeypatch)
    monkeypatch.setattr(app_copy, "copy_app",
                        lambda root: pytest.fail("must not copy under a live daemon"))
    assert installer.install() == 1
    assert "did not stop" in capsys.readouterr().err
    assert not paths.STOPPED_SENTINEL_PATH.exists()


def test_uninstall_does_not_delete_under_a_live_daemon(monkeypatch, tmp_path, capsys):
    sup = _plugin_env(monkeypatch, tmp_path)
    sup.uninstall = lambda: pytest.fail("must not uninstall under a live daemon")
    _failed_stop(monkeypatch)
    paths.APP_DIR.mkdir(parents=True, exist_ok=True)
    assert installer.uninstall() == 1
    assert paths.APP_DIR.is_dir()
    assert "did not stop" in capsys.readouterr().err
