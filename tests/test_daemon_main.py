import sys
from unittest import mock

import sonara.daemon as daemon_mod


def test_ensure_running_noop_when_socket_connectable():
    with mock.patch("sonara.daemon.socket_connectable", return_value=True) as conn, \
         mock.patch("sonara.daemon.subprocess.Popen") as popen:
        daemon_mod.ensure_running()
    conn.assert_called_once()
    popen.assert_not_called()


def test_ensure_running_spawns_via_platform_launch_spec_when_socket_absent():
    # ensure_running asks the platform supervisor for its (argv, kwargs) launch
    # spec and Popens it verbatim. On Windows that's [pythonw, -m, sonara.daemon]
    # with detach creationflags -- no POSIX start_new_session, no bin shim.
    fake_argv = ["pythonw.exe", "-m", "sonara.daemon"]
    fake_kwargs = {"creationflags": 0x208}
    sup = mock.Mock()
    sup.launch_spec.return_value = (fake_argv, dict(fake_kwargs))
    plat = mock.Mock(supervisor=sup)
    with mock.patch("sonara.daemon.socket_connectable", return_value=False), \
         mock.patch("sonara.platform.get_platform", return_value=plat), \
         mock.patch("sonara.daemon.subprocess.Popen") as popen:
        daemon_mod.ensure_running()
    popen.assert_called_once_with(fake_argv, **fake_kwargs)


def test_main_builds_components_and_runs():
    fake_cfg = {"voice": None, "rate": 200, "verbosity": "everything",
                "background_policy": "earcon_only", "earcons": {}}
    with mock.patch("sonara.daemon.load_config", return_value=fake_cfg), \
         mock.patch("sonara.daemon.socket_connectable", return_value=False), \
         mock.patch("sonara.daemon.transport.acquire_singleton_mutex", return_value=object()), \
         mock.patch("sonara.daemon.transport.acquire_singleton", return_value=object()), \
         mock.patch("sonara.daemon.SpeechDaemon.run", autospec=True) as run:
        daemon_mod.main()
    assert run.call_count == 1
    built = run.call_args[0][0]
    assert isinstance(built, daemon_mod.SpeechDaemon)
    assert built.config is fake_cfg


class _FakeK32:
    """Records the Win32 process calls _harden_process makes."""
    def __init__(self):
        self.calls = {}
    def GetCurrentProcess(self):
        return 4321
    def SetProcessInformation(self, h, info_class, info, size):
        self.calls["info"] = (h, info_class, size)
        return 1
    def SetPriorityClass(self, h, prio):
        self.calls["prio"] = (h, prio)
        return 1


def test_harden_process_noop_off_windows(monkeypatch):
    # On non-Windows it must do nothing (and never touch kernel32).
    monkeypatch.setattr(sys, "platform", "linux")
    k32 = _FakeK32()
    daemon_mod._harden_process(k32=k32)
    assert k32.calls == {}


def test_harden_process_disables_throttling_and_raises_priority(monkeypatch):
    # On win32 it opts out of power throttling (ProcessPowerThrottling == 4) and
    # raises the priority class to NORMAL (0x20).
    monkeypatch.setattr(sys, "platform", "win32")
    k32 = _FakeK32()
    daemon_mod._harden_process(k32=k32)
    h, info_class, size = k32.calls["info"]
    assert h == 4321 and info_class == 4 and size > 0   # ProcessPowerThrottling
    assert k32.calls["prio"] == (4321, 0x20)            # NORMAL_PRIORITY_CLASS


def test_harden_process_actually_raises_priority_on_real_win32():
    # Integration: run the REAL ctypes path (no fake) so a handle-marshaling
    # regression -- the GetCurrentProcess pseudo-handle (-1) truncated to 32 bits,
    # which fails with ERROR_INVALID_HANDLE and silently no-ops -- is caught. A
    # pure-mock test cannot see that. Saves/restores this process's priority.
    if sys.platform != "win32":
        import pytest
        pytest.skip("win32-only integration test")
    import ctypes
    from ctypes import wintypes
    k = ctypes.WinDLL("kernel32", use_last_error=True)
    k.GetCurrentProcess.restype = wintypes.HANDLE
    k.GetPriorityClass.argtypes = [wintypes.HANDLE]
    k.GetPriorityClass.restype = wintypes.DWORD
    k.SetPriorityClass.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    k.SetPriorityClass.restype = wintypes.BOOL
    h = k.GetCurrentProcess()
    orig = k.GetPriorityClass(h)
    try:
        k.SetPriorityClass(h, 0x4000)            # BELOW_NORMAL_PRIORITY_CLASS
        daemon_mod._harden_process()             # real path, builds its own WinDLL
        assert k.GetPriorityClass(h) == 0x20     # NORMAL -> proves the handle typing works
    finally:
        k.SetPriorityClass(h, orig or 0x20)
