"""Child processes on Windows: the summarizer engine (claude -p, codex exec)
is spawned without a console window, found with PATHEXT applied and killed
with its whole process tree. Reached through
sonara.platform.child_processes(), so summarizer.py stays OS-free (#161).
Light: imports nothing beyond the standard library."""
from __future__ import annotations

import os
import subprocess

_CREATE_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000)


def popen_kwargs() -> dict:
    """Extra Popen arguments for a background child: no console window."""
    return {"creationflags": _CREATE_NO_WINDOW}


def command_names(name: str) -> list:
    """The file names a bare command *name* may have on PATH. CreateProcess
    does not apply PATHEXT to a bare name like 'claude' (an npm .cmd shim),
    so each extension is tried; a name that already has one is kept."""
    exts = [e for e in os.environ.get(
        "PATHEXT", ".COM;.EXE;.BAT;.CMD").split(os.pathsep) if e]
    if any(name.lower().endswith(e.lower()) for e in exts):
        return [name]
    return [name + e for e in exts]


def kill_tree(proc) -> None:
    """Kill every process *proc* started, then the caller kills *proc*. An
    npm .cmd shim (codex) runs as cmd.exe -> node: killing cmd.exe alone
    leaves node holding the pipes. Never raises."""
    try:
        subprocess.run(
            ["taskkill", "/T", "/F", "/PID", str(proc.pid)],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, timeout=10,
            creationflags=_CREATE_NO_WINDOW)
    except (OSError, subprocess.SubprocessError):
        pass
