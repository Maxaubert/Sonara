"""Thin Python supervisor loop -- Task Scheduler launches this; it restarts the
sonara.daemon process indefinitely with exponential back-off.

WINDOWS-only behaviour, but the module imports cleanly on macOS/Linux (the
process-creation flags are hex literals, not subprocess.CREATE_NO_WINDOW which
is win32-only). "Imports + mock-green" does NOT mean Windows-verified -- the
DETACHED_PROCESS/CREATE_NO_WINDOW spawn behaviour is a deferred acceptance item
(docs/superpowers/M2-WINDOWS-ACCEPTANCE.md).

Body copied verbatim from docs/superpowers/m2-windows-api-reference.md
(§Thin Python supervisor loop), adapting only the import location.
"""
from __future__ import annotations

import os
import subprocess
import sys
import time

# These constants are defined in subprocess only on win32.
# Use hex literals so this file imports cleanly on macOS/Linux.
_CREATE_NO_WINDOW = 0x08000000
_DETACHED_PROCESS = 0x00000008
_SPAWN_FLAGS      = _CREATE_NO_WINDOW | _DETACHED_PROCESS  # 0x08000008


def _package_root() -> str:
    """Directory that contains the 'sonara' package, derived from THIS file's
    location: <root>/sonara/platform/windows/supervisor_loop.py -> <root>.
    Works for both the dev (src/) and installed (app_dir/) layouts because it is
    relative to __file__, not to a cwd or a configured path."""
    return os.path.dirname(os.path.dirname(os.path.dirname(
        os.path.dirname(os.path.abspath(__file__)))))


def _ensure_importable() -> str:
    """Put the package root on sys.path so `import sonara` resolves even when this
    file is launched by BARE SCRIPT PATH. Task Scheduler does exactly that
    (Action: pythonw.exe "<path>/supervisor_loop.py"), which makes sys.path[0]
    this file's own dir, not the package root -> the daemon never autostarts.
    Returns the root. Idempotent and safe on every OS."""
    root = _package_root()
    if root not in sys.path:
        sys.path.insert(0, root)
    return root

# Never combine start_new_session=True with DETACHED_PROCESS:
# Python 3.9+ raises ValueError on Windows if both are set.


def launch_spec(pythonw: str) -> tuple:
    """Return (argv, spawn_kwargs) compatible with subprocess.Popen(**kwargs).

    argv drives both the supervisor loop and is returned from
    WinSupervisorBackend.launch_spec() for the lazy-start path.
    """
    argv = [pythonw, "-m", "sonara.daemon"]
    # The spawned daemon is a fresh process; without PYTHONPATH it cannot import
    # 'sonara' -> it exits instantly -> a relaunch storm. Put the package root
    # (derived from this file's location) first on PYTHONPATH so the daemon
    # resolves self-containedly. Parity with WinSupervisorBackend.launch_spec.
    env = dict(os.environ)
    root = _package_root()
    existing = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = root + (os.pathsep + existing if existing else "")
    # Route the daemon's stderr to the daemon log (parity with the macOS plist
    # StandardErrorPath) so the speak-loop catch-all traceback survives (#20);
    # DEVNULL made it unrecoverable. Open lazily inside launch_spec.
    from sonara import paths
    paths.ensure_sonara_dir()
    err = open(paths.LOG_PATH, "a")
    kwargs = dict(
        creationflags=_SPAWN_FLAGS,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=err,
        env=env,
        # start_new_session intentionally absent -- incompatible with DETACHED_PROCESS
    )
    return argv, kwargs


def _stop_requested() -> bool:
    """True when the stop sentinel exists: `sonara shutdown` was issued and no
    start/install has cleared it, so the loop must EXIT instead of respawning
    (previously NOTHING could stop the respawn loop, #23)."""
    _ensure_importable()
    from sonara import paths
    return os.path.exists(str(paths.STOPPED_SENTINEL_PATH))


def run_supervisor_loop(pythonw: str) -> None:
    """Restart sonara.daemon indefinitely with exponential back-off.

    Back-off resets to base when the daemon ran for >= 300 s (healthy restart).
    Sequence (seconds): 2, 4, 8, 16, 32, 64, 120, 120, 120 ...
    Exits when the stop sentinel appears (`sonara shutdown`, #23).
    """
    BASE, CAP, HEALTHY_UPTIME = 2, 120, 300
    attempt = 0
    while True:
        if _stop_requested():
            return
        argv, kwargs = launch_spec(pythonw)
        t_start = time.monotonic()
        proc = subprocess.Popen(argv, **kwargs)
        proc.wait()  # blocks until daemon exits
        elapsed = time.monotonic() - t_start
        if elapsed >= HEALTHY_UPTIME:
            attempt = 0          # reset debt after a healthy run
        else:
            attempt += 1
        if _stop_requested():
            return          # do not sleep out the back-off when told to stop
        delay = min(BASE * (2 ** (attempt - 1)), CAP)
        time.sleep(delay)


def _main() -> None:
    _ensure_importable()  # MUST run before importing sonara (script-path launch)
    if sys.platform == "win32":
        # Run the daemon on the interpreter Task Scheduler launched us with (the venv
        # pythonw when neural is enabled -- baked in at install). Re-resolving here
        # discarded it and forced system python (no kokoro). macOS bakes the python
        # into the plist; this is the Windows parity.
        run_supervisor_loop(sys.executable)


# Entry point when Task Scheduler launches this file directly:
# schtasks Action: pythonw.exe "<path>/supervisor_loop.py"
if __name__ == "__main__":
    _main()
