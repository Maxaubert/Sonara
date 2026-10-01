import sonara.platform as platform
from sonara.platform import base


def test_get_platform_returns_windows_backend_on_win32(monkeypatch):
    monkeypatch.setattr(platform.sys, "platform", "win32")
    platform._CACHE = None
    pb = platform.get_platform()
    assert isinstance(pb, base.PlatformBackend)


def test_get_platform_rejects_non_win32(monkeypatch):
    monkeypatch.setattr(platform.sys, "platform", "darwin")
    platform._CACHE = None
    import pytest
    with pytest.raises(RuntimeError):
        platform.get_platform()


def test_daemon_process_is_the_windows_process_module(monkeypatch):
    # The daemon's process setup and single-instance guard come through the
    # seam, and the guard is the Windows singleton module's (audit section 2).
    from sonara.platform.windows import process, singleton
    monkeypatch.setattr(platform.sys, "platform", "win32")
    mod = platform.daemon_process()
    assert mod is process
    assert mod.acquire_singleton_mutex is singleton.acquire_singleton_mutex
    assert mod.acquire_singleton is singleton.acquire_singleton


def test_daemon_process_rejects_non_win32(monkeypatch):
    import pytest
    monkeypatch.setattr(platform.sys, "platform", "darwin")
    with pytest.raises(RuntimeError):
        platform.daemon_process()


def test_transport_is_os_free():
    # The Win32 / msvcrt single-instance code moved under platform/windows.
    import inspect
    from sonara.platform import transport
    src = inspect.getsource(transport)
    for word in ("msvcrt", "ctypes", "kernel32", "os.name"):
        assert word not in src, word


def test_child_processes_is_the_windows_child_process_module(monkeypatch):
    # The summarizer spawns, finds and kills its engine through the seam (#161).
    from sonara.platform.windows import child_process
    monkeypatch.setattr(platform.sys, "platform", "win32")
    assert platform.child_processes() is child_process


def test_child_processes_rejects_non_win32(monkeypatch):
    import pytest
    monkeypatch.setattr(platform.sys, "platform", "darwin")
    with pytest.raises(RuntimeError):
        platform.child_processes()


def test_child_process_names_apply_pathext(monkeypatch):
    from sonara.platform.windows import child_process
    monkeypatch.setenv("PATHEXT", ".EXE;.CMD")
    assert child_process.command_names("claude") == ["claude.EXE", "claude.CMD"]
    assert child_process.command_names("codex.cmd") == ["codex.cmd"]


def test_child_process_kill_tree_runs_taskkill_windowless(monkeypatch):
    from sonara.platform.windows import child_process
    calls = []
    monkeypatch.setattr(child_process.subprocess, "run",
                        lambda argv, **kw: calls.append((argv, kw)))

    class _Proc:
        pid = 4242
    child_process.kill_tree(_Proc())
    (argv, kw), = calls
    assert argv == ["taskkill", "/T", "/F", "/PID", "4242"]
    assert kw["creationflags"] == 0x08000000


def test_child_process_kill_tree_never_raises(monkeypatch):
    from sonara.platform.windows import child_process

    def boom(*a, **k):
        raise OSError("no taskkill")
    monkeypatch.setattr(child_process.subprocess, "run", boom)

    class _Proc:
        pid = 1
    child_process.kill_tree(_Proc())
