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
    resilience traceback was unrecoverable). Regression for #20."""
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


# ---------------------------------------------------------------------------
# #123: which COPY of the code is running must be observable from the log
# ---------------------------------------------------------------------------

def test_package_root_is_the_dir_containing_the_sonara_package():
    """paths.package_root() answers "where does the code I am executing live",
    which is correct in BOTH layouts (<repo>/src and ~/.sonara/app). Contrast
    repo_root(), which assumes the repo shape and returns ~/.sonara from the
    deployed copy -- the defect behind the nonexistent ~/.sonara/src that the
    lazy start was putting on PYTHONPATH."""
    from sonara import paths
    root = paths.package_root()
    assert os.path.isdir(os.path.join(root, "sonara")), root
    assert os.path.isfile(os.path.join(root, "sonara", "paths.py")), root


def test_daemon_start_log_records_which_copy_is_running(monkeypatch, capsys):
    """speechd.log said only `[daemon] started pid=N`, so two daemons running
    DIFFERENT copies of Sonara were indistinguishable in the log (observed live:
    one on ~/.sonara/app, the next on the repo checkout). The start marker must
    name the root the daemon imported from."""
    from tests.daemon_helpers import make_daemon
    from sonara import paths

    daemon, *_ = make_daemon(foreground="fg")
    # run() blocks; drive only the startup marker the same way run() emits it.
    daemon._log_start_marker()
    err = capsys.readouterr().err
    assert "[daemon] started pid=" in err
    assert paths.package_root() in err, err


# ---------------------------------------------------------------------------
# L-log: speechd.log is size-capped and the parent never keeps its handle
# ---------------------------------------------------------------------------

def test_launch_spec_rotates_an_oversized_log(tmp_path, monkeypatch):
    from sonara import paths
    log = tmp_path / "speechd.log"
    log.write_text("x" * 64)
    monkeypatch.setattr(paths, "SONARA_DIR", tmp_path)
    monkeypatch.setattr(paths, "LOG_PATH", log)
    monkeypatch.setattr(sl, "_LOG_MAX_BYTES", 32)
    argv, kwargs = sl.launch_spec("pythonw.exe")
    kwargs["stderr"].close()
    assert log.stat().st_size == 0                       # a fresh log
    assert (tmp_path / "speechd.old.log").read_text() == "x" * 64


def test_launch_spec_keeps_a_small_log(tmp_path, monkeypatch):
    from sonara import paths
    log = tmp_path / "speechd.log"
    log.write_text("keep")
    monkeypatch.setattr(paths, "SONARA_DIR", tmp_path)
    monkeypatch.setattr(paths, "LOG_PATH", log)
    argv, kwargs = sl.launch_spec("pythonw.exe")
    kwargs["stderr"].close()
    assert log.read_text() == "keep"
    assert not (tmp_path / "speechd.old.log").exists()


def test_supervisor_loop_closes_its_log_handle_after_each_spawn(tmp_path, monkeypatch):
    from sonara import paths
    monkeypatch.setattr(paths, "SONARA_DIR", tmp_path)
    monkeypatch.setattr(paths, "LOG_PATH", tmp_path / "speechd.log")
    handles = []

    class _Proc:
        def wait(self):
            return 0

    def fake_popen(argv, **kwargs):
        handles.append(kwargs["stderr"])
        return _Proc()

    stops = iter([False, True])
    monkeypatch.setattr(sl, "_stop_requested", lambda: next(stops, True))
    monkeypatch.setattr(sl.subprocess, "Popen", fake_popen)
    sl.run_supervisor_loop("pythonw.exe")
    assert handles and all(h.closed for h in handles)


def test_lazy_start_closes_its_log_handle(tmp_path, monkeypatch):
    import types
    from sonara import lifecycle
    fh = open(str(tmp_path / "speechd.log"), "a")
    plat = types.SimpleNamespace(supervisor=types.SimpleNamespace(
        launch_spec=lambda: (["pythonw.exe"], {"stderr": fh})))
    monkeypatch.setattr("sonara.platform.get_platform", lambda: plat)
    monkeypatch.setattr(lifecycle, "socket_connectable", lambda: False)
    monkeypatch.setattr(lifecycle.subprocess, "Popen", lambda argv, **kw: None)
    lifecycle.ensure_running()
    assert fh.closed
