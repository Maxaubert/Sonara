"""Fixtures for the sonara-client tests.

Unit tests run against ``fake_runtime.py`` (a child process). The tests in
``test_sonarad.py`` run against a real ``sonarad.exe`` built in this
workspace (``cargo build -p sonarad``, or ``$SONARAD``) with the fake engine,
and skip when there is none.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent.parent
sys.path.insert(0, str(HERE.parent / "src"))

from sonara_client.discovery import pid_alive  # noqa: E402


def wait_until(pred, timeout: float = 10.0, step: float = 0.02) -> bool:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if pred():
            return True
        time.sleep(step)
    return bool(pred())


def read_json(path: Path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def kill(pid: int) -> None:
    if not pid_alive(pid):
        return
    if os.name == "nt":
        subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)
    else:
        os.kill(pid, 9)


class Fake:
    def __init__(self, home: Path, **opts):
        self.home = home
        self.proc = subprocess.Popen(
            [sys.executable, str(HERE / "fake_runtime.py"), json.dumps({"home": str(home), **opts})],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        ok = wait_until(lambda: (read_json(home / "runtime.json") or {}).get("pid") == self.proc.pid, 10)
        assert ok, "the fake runtime wrote no runtime.json"

    @property
    def pid(self) -> int:
        return self.proc.pid

    def requests(self) -> list:
        path = self.home / "requests.jsonl"
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]

    def stop(self) -> None:
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait(10)


@pytest.fixture
def home(tmp_path) -> Path:
    h = tmp_path / "home"
    h.mkdir()
    return h


@pytest.fixture
def fake(home):
    started = []

    def _start(**opts) -> Fake:
        f = Fake(home, **opts)
        started.append(f)
        return f

    yield _start
    for f in started:
        f.stop()


def find_sonarad():
    env = os.environ.get("SONARAD")
    if env:
        return Path(env)
    found = [REPO / "target" / p / "sonarad.exe" for p in ("release", "debug")]
    found = [p for p in found if p.is_file()]
    return max(found, key=lambda p: p.stat().st_mtime) if found else None


@pytest.fixture
def sonarad():
    exe = find_sonarad()
    if exe is None or not exe.is_file():
        pytest.skip("sonarad.exe not built (cargo build -p sonarad) and SONARAD not set")
    return exe
