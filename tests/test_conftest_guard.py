"""The conftest guard that keeps tests off this machine's real autostart task.

A mutating schtasks call must be refused whatever the call style: call or run,
bare name, .exe suffix or full path. Read-only /query still runs. These tests
check the predicate and the wrapping without ever spawning schtasks, so a
broken guard cannot reach the real task.
"""
import subprocess

import pytest

from tests.conftest import _is_mutating_schtasks


@pytest.mark.parametrize("argv", [
    ["schtasks", "/end", "/tn", "Sonara.Speechd"],
    ["schtasks.exe", "/run", "/tn", "Sonara.Speechd"],
    [r"C:\Windows\System32\SCHTASKS.EXE", "/delete", "/tn", "Sonara.Speechd", "/f"],
    ["C:/Windows/System32/schtasks.exe", "/create", "/tn", "x", "/xml", "t.xml"],
    "schtasks /end /tn Sonara.Speechd",
])
def test_guard_flags_mutating_schtasks(argv):
    assert _is_mutating_schtasks(argv)


@pytest.mark.parametrize("argv", [
    ["schtasks", "/query", "/tn", "Sonara.Speechd"],
    ["schtasks.exe", "/QUERY", "/tn", "Sonara.Speechd"],
    ["python", "-c", "print('schtasks /end')"],
    ["notschtasks", "/end"],
])
def test_guard_lets_query_and_other_commands_through(argv):
    assert not _is_mutating_schtasks(argv)


def test_guard_wraps_call_and_run():
    assert subprocess.call.__name__ == "guarded_call"
    assert subprocess.run.__name__ == "guarded_run"
