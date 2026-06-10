# tests/test_macos_supervisor.py
from sonari.platform.macos.supervisor import MacSupervisorBackend


def test_resolve_python_prefers_usr_bin(monkeypatch):
    sup = MacSupervisorBackend()
    monkeypatch.setattr(sup, "_probe_python_version", lambda c: (3, 11))
    monkeypatch.setattr("sonari.platform.macos.supervisor.shutil.which",
                        lambda n: "/opt/homebrew/bin/python3")
    monkeypatch.setattr("sonari.platform.macos.supervisor.os.path.realpath",
                        lambda p: p)
    assert sup.resolve_python() == "/usr/bin/python3"


def test_launch_spec_uses_start_new_session():
    argv, kwargs = MacSupervisorBackend().launch_spec()
    assert argv[-1].endswith("sonari-daemon")
    assert kwargs.get("start_new_session") is True


def test_doctor_rows_include_say_and_swiftc(monkeypatch):
    monkeypatch.setattr("sonari.platform.macos.supervisor.shutil.which",
                        lambda n: "/usr/bin/" + n)
    names = [r[0] for r in MacSupervisorBackend().doctor_rows()]
    assert "say" in names and "swiftc" in names
