"""Autostart wiring: Task Scheduler launches supervisor_loop.py by BARE SCRIPT
PATH, so sys.path[0] is the file's own dir, not the package root. The loop must
self-bootstrap so `import sonara` resolves, and the daemon it spawns must inherit
PYTHONPATH. Regression for the dead-autostart bug (#6).

The subprocess run is confined to an ISOLATED home with the stop sentinel
pre-created: on Windows _main() enters the real respawn loop, and without the
sentinel this test used to burn its full 30s timeout AND spawn real detached
`python -m sonara.daemon` processes against the live ~/.sonara (conftest's
in-process path isolation cannot reach a subprocess) -- deep-audit #25. The
sentinel makes run_supervisor_loop return immediately, still exercising the
bare-script-path import resolution this test pins.
"""
import os
import subprocess
import sys

import sonara.platform.windows.supervisor_loop as sl


def test_supervisor_loop_imports_sonara_when_launched_by_script_path(tmp_path):
    loop_py = os.path.abspath(sl.__file__)
    env = {k: v for k, v in os.environ.items() if k != "PYTHONPATH"}
    # Isolated home + pre-created stop sentinel: the loop exits before spawning
    # anything, and nothing in the subprocess can touch the real ~/.sonara.
    home = tmp_path / "home"
    (home / ".sonara").mkdir(parents=True)
    (home / ".sonara" / "stopped").write_text("test sentinel")
    env["USERPROFILE"] = str(home)   # Path.home() on Windows
    env["HOME"] = str(home)          # Path.home() elsewhere
    proc = subprocess.run(
        [sys.executable, loop_py],
        cwd=str(tmp_path), env=env,
        capture_output=True, text=True, timeout=30,
    )
    assert "ModuleNotFoundError" not in proc.stderr, proc.stderr
    assert proc.returncode == 0, proc.stderr


def test_launch_spec_sets_pythonpath_so_the_spawned_daemon_can_import():
    argv, kwargs = sl.launch_spec("pythonw.exe")
    assert argv == ["pythonw.exe", "-m", "sonara.daemon"]
    pp = kwargs["env"]["PYTHONPATH"]
    root = pp.split(os.pathsep)[0]
    # the first PYTHONPATH entry must be the dir that contains the 'sonara' package
    assert os.path.isdir(os.path.join(root, "sonara")), pp


def test_launch_spec_routes_stderr_to_log_file_not_devnull(tmp_path, monkeypatch):
    """The spawned daemon's stderr must land in the daemon log under SONARA_DIR so
    the speak-loop catch-all traceback survives on Windows (it was DEVNULL'd -> the
    resilience traceback was unrecoverable). Mirrors the macOS plist StandardErrorPath.
    Regression for #20."""
    from sonara import paths

    log = tmp_path / "speechd.log"
    monkeypatch.setattr(paths, "SONARA_DIR", tmp_path)
    monkeypatch.setattr(paths, "LOG_PATH", log)

    argv, kwargs = sl.launch_spec("pythonw.exe")
    assert kwargs["stderr"] is not subprocess.DEVNULL
    assert str(kwargs["stderr"].name) == str(log)
    # stdin/stdout stay DEVNULL
    assert kwargs["stdin"] is subprocess.DEVNULL
    assert kwargs["stdout"] is subprocess.DEVNULL
    kwargs["stderr"].close()


# ---------------------------------------------------------------------------
# FIX C: _main() uses sys.executable and is guarded by sys.platform == 'win32'
# ---------------------------------------------------------------------------

def test_main_runs_loop_with_sys_executable_on_win32(monkeypatch):
    monkeypatch.setattr(sl, "_ensure_importable", lambda: None)
    monkeypatch.setattr(sl.sys, "platform", "win32")
    calls = []
    monkeypatch.setattr(sl, "run_supervisor_loop", lambda pw: calls.append(pw))
    sl._main()
    assert calls == [sl.sys.executable]


def test_main_skips_loop_off_win32(monkeypatch):
    monkeypatch.setattr(sl, "_ensure_importable", lambda: None)
    monkeypatch.setattr(sl.sys, "platform", "darwin")
    calls = []
    monkeypatch.setattr(sl, "run_supervisor_loop", lambda pw: calls.append(pw))
    sl._main()
    assert calls == []
