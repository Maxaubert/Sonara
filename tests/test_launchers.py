"""Interpreter choice in the launchers (#139: E3, E4, E5, M14).

On Windows the WindowsApps `python` / `python3` aliases are on PATH by
default and run nothing (the Store stub). The plugin hooks relied on a
`python3` shebang and the CLI shims preferred PATH `python`, so a box whose
only real Python is the one the bootstrap recorded got no hooks and no CLI.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from sonara import paths

REPO = Path(__file__).resolve().parent.parent
BIN = REPO / "bin"
_WINDOWS = os.name == "nt"


def _find_bash():
    if _WINDOWS:
        for root in (os.environ.get("ProgramFiles", r"C:\Program Files"),
                     os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")):
            cand = os.path.join(root, "Git", "bin", "bash.exe")
            if os.path.isfile(cand):
                return cand
        return None
    import shutil
    return shutil.which("bash")


_BASH = _find_bash()
_needs_bash = pytest.mark.skipif(_BASH is None, reason="no Git Bash available")


def _stub_dir(tmp_path):
    """A PATH dir named like the Store alias folder whose python/python3 run
    nothing and exit 9009, as the Store stub does."""
    d = tmp_path / "WindowsApps"
    d.mkdir()
    for name in ("python", "python3"):
        p = d / name
        p.write_text("#!/bin/sh\necho STUB\nexit 9009\n", encoding="utf-8", newline="\n")
        p.chmod(0o755)
        (d / (name + ".bat")).write_text("@echo STUB\r\n@exit /b 9009\r\n", encoding="utf-8")
    return d


def _home(tmp_path, record=None, bom=False):
    home = tmp_path / "home"
    (home / ".sonara").mkdir(parents=True)
    if record is not None:
        data = record.encode("utf-8")
        if bom:
            data = b"\xef\xbb\xbf" + data + b"\r\n"
        (home / ".sonara" / "python.path").write_bytes(data)
    return home


def _bash_env(tmp_path, home):
    env = dict(os.environ)
    tools = []
    if _WINDOWS:   # dirname, uname, cat...: Git's coreutils, never a python
        tools.append(os.path.join(os.path.dirname(os.path.dirname(_BASH)), "usr", "bin"))
    env["PATH"] = os.pathsep.join([str(_stub_dir(tmp_path))] + tools)
    env["HOME"] = str(home)
    env["USERPROFILE"] = str(home)
    return env


def _posix(p):
    return str(p).replace("\\", "/")


# --- E3: plugin hooks -------------------------------------------------------------

def test_plugin_hooks_run_through_the_interpreter_launcher():
    hooks = json.loads((REPO / "hooks" / "hooks.json").read_text(encoding="utf-8"))["hooks"]
    for event, entries in hooks.items():
        for entry in entries:
            for h in entry["hooks"]:
                assert h["command"] == '"${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook-run" ' + event


def _run_hook_launcher(tmp_path, home, payload=b'{"session_id": "s1", "delta": "Hi."}'):
    sent = tmp_path / "sent.jsonl"
    env = _bash_env(tmp_path, home)
    env["PYTHONPATH"] = os.pathsep.join(
        [str(REPO / "tests" / "_fakeclient"), str(REPO / "src")])
    env["SONARA_FAKE_SENT_LOG"] = str(sent)
    proc = subprocess.run([_BASH, _posix(BIN / "sonara-hook-run"), "MessageDisplay"],
                          input=payload, capture_output=True, env=env, timeout=60)
    lines = sent.read_text().splitlines() if sent.exists() else []
    return proc, lines


@_needs_bash
def test_hook_launcher_runs_the_recorded_interpreter_not_the_store_stub(tmp_path):
    proc, lines = _run_hook_launcher(tmp_path, _home(tmp_path, sys.executable))
    assert proc.returncode == 0, proc.stderr
    assert len(lines) == 1 and json.loads(lines[0])["delta"] == "Hi."


@_needs_bash
def test_hook_launcher_reads_a_bom_record(tmp_path):
    proc, lines = _run_hook_launcher(tmp_path, _home(tmp_path, sys.executable, bom=True))
    assert proc.returncode == 0, proc.stderr
    assert len(lines) == 1


@_needs_bash
def test_hook_launcher_exits_zero_when_no_python_exists(tmp_path):
    proc, lines = _run_hook_launcher(tmp_path, _home(tmp_path))
    assert proc.returncode == 0, proc.stderr
    assert lines == []
    assert b"STUB" not in proc.stdout


# --- E4: CLI shims -------------------------------------------------------------------

@_needs_bash
def test_bash_cli_shim_prefers_the_recorded_python_over_the_store_stub(tmp_path):
    env = _bash_env(tmp_path, _home(tmp_path, sys.executable))
    proc = subprocess.run([_BASH, _posix(BIN / "sonara")], capture_output=True,
                          text=True, env=env, timeout=60)
    assert "STUB" not in proc.stdout
    assert proc.returncode == 2, (proc.returncode, proc.stdout, proc.stderr)  # argparse


@pytest.mark.skipif(not _WINDOWS, reason="sonara.cmd is the Windows launcher")
def test_cmd_cli_shim_prefers_the_recorded_python_over_the_store_stub(tmp_path):
    home = _home(tmp_path, sys.executable)
    env = dict(os.environ)
    system32 = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32")
    env["PATH"] = os.pathsep.join([str(_stub_dir(tmp_path)), system32])
    env["USERPROFILE"] = str(home)
    proc = subprocess.run([str(BIN / "sonara.cmd")], capture_output=True, text=True,
                          env=env, timeout=60)
    assert "STUB" not in proc.stdout
    assert proc.returncode == 2, (proc.returncode, proc.stdout, proc.stderr)


@pytest.mark.skipif(not _WINDOWS, reason="sonara.cmd is the Windows launcher")
def test_cmd_cli_shim_skips_the_store_stub_on_path(tmp_path):
    # No record: a real python later on PATH must win over the stub before it.
    home = _home(tmp_path)
    env = dict(os.environ)
    system32 = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32")
    env["PATH"] = os.pathsep.join([str(_stub_dir(tmp_path)),
                                   os.path.dirname(sys.executable), system32])
    env["USERPROFILE"] = str(home)
    proc = subprocess.run([str(BIN / "sonara.cmd")], capture_output=True, text=True,
                          env=env, timeout=60)
    assert "STUB" not in proc.stdout
    assert proc.returncode == 2, (proc.returncode, proc.stdout, proc.stderr)


# --- E5 / M14: bootstrap -------------------------------------------------------------

def _bootstrap():
    return (BIN / "sonara-bootstrap.ps1").read_text(encoding="utf-8")


def test_bootstrap_py_probe_cannot_abort_under_powershell_51():
    # With $ErrorActionPreference = "Stop", PowerShell 5.1 turns a native
    # command's redirected stderr into a terminating error; a leftover py.exe
    # with no Python 3 then killed the script before uv could provision one.
    txt = _bootstrap()
    probe = next(line for line in txt.splitlines() if "& py -3" in line)
    assert probe.strip().startswith("try {") and "catch" in probe, probe


def test_bootstrap_records_interpreter_paths_as_utf8_without_bom():
    txt = _bootstrap()
    assert "-Encoding ASCII" not in txt
    assert "UTF8Encoding($false)" in txt
    assert txt.count("WriteAllText") >= 2


def test_recorded_interpreter_path_survives_bom_and_non_ascii(tmp_path, monkeypatch):
    exe = tmp_path / "Pyth\u00f6n" / "pythonw.exe"
    exe.parent.mkdir()
    exe.write_bytes(b"")
    rec = tmp_path / "pythonw.path"
    rec.write_bytes(b"\xef\xbb\xbf" + str(exe).encode("utf-8") + b"\r\n")
    monkeypatch.setattr(paths, "PYTHONW_RECORD_PATH", rec)
    assert paths.recorded_pythonw() == str(exe)
