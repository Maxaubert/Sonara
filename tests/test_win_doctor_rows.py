"""Task 9: Doctor wiring — Windows rows reachable via PlatformBackend.supervisor.

Tests that the Windows seam (sonara.platform.windows.make_backend() →
PlatformBackend.supervisor) returns a WinSupervisorBackend whose doctor_rows()
yields the expected row names. This exercises the make_backend() factory and the
PlatformBackend.supervisor attribute — the actual dispatch seam — rather than
constructing WinSupervisorBackend directly.
"""
from __future__ import annotations


def test_windows_supervisor_doctor_rows_via_seam(monkeypatch):
    """doctor_rows() is reachable through PlatformBackend.supervisor (the seam)."""
    from sonara.platform.windows import make_backend
    platform = make_backend()
    sup = platform.supervisor
    monkeypatch.setattr(sup, "_schtasks", lambda args: 0)
    monkeypatch.setattr(sup, "resolve_python", lambda: r"C:\Py\pythonw.exe")
    monkeypatch.setattr(sup, "_list_neural_voices", lambda: ["Microsoft Aria"])
    monkeypatch.setattr("sonara.paths.socket_connectable", lambda: True)
    names = [r[0] for r in sup.doctor_rows()]
    assert {"Task Scheduler task", "pythonw.exe", "neural voice", "daemon running"} <= set(names)
