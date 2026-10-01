"""Stopping and starting Sonara around file changes (#23): the stop sentinel,
the scheduled task, the daemon, and putting things back when a command that
stopped Sonara fails."""
from __future__ import annotations

import os
import sys
import time

from sonara import paths
from sonara import platform as sonara_platform
from sonara.protocol import MsgType, PROTOCOL_VERSION


def _send(msg: dict, expect_reply: bool = False):
    from sonara import client  # local import so tests can patch sonara.client.send
    return client.send(msg, expect_reply=expect_reply)


def stop_sonara(sup=None) -> bool:
    """Stop Sonara everywhere (#23): write the stop sentinel (gates the
    supervisor loop AND the per-hook-event lazy start), end the scheduled task,
    SHUTDOWN the daemon, and wait for it to be gone; one that still runs
    after the grace is killed. Returns True when the daemon is confirmed gone
    (a daemon that was not running counts as stopped). install()/uninstall()
    call this BEFORE mutating files under APP_DIR and change nothing when it
    returns False."""
    paths.ensure_sonara_dir()
    try:
        with open(str(paths.STOPPED_SENTINEL_PATH), "w", encoding="utf-8") as fh:
            fh.write("sonara shutdown")
    except OSError:
        pass
    if sup is None:
        sup = sonara_platform.get_platform().supervisor
    try:
        sup.end_task()
    except Exception:  # noqa: BLE001 - task may not exist; never fail a stop
        pass
    try:
        _send({"v": PROTOCOL_VERSION, "type": MsgType.SHUTDOWN}, expect_reply=True)
    except Exception:  # noqa: BLE001 - not running IS stopped
        pass
    if _wait_gone(5.0):
        time.sleep(0.3)     # grace: process exit releases the mutex
        sup.kill_stray_daemons()
        return True
    # Still accepting after the grace: a daemon wedged under its lock (E18)
    # answers connects but never runs SHUTDOWN, and a lazily started one is
    # outside the task tree end_task ended. Kill it by command line.
    sup.kill_stray_daemons()
    return _wait_gone(2.0)


def _wait_gone(seconds: float) -> bool:
    """True once the daemon socket stops accepting, within *seconds*."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        if not paths.socket_connectable():
            return True
        time.sleep(0.1)
    return False


def print_not_stopped() -> None:
    """Explain a refused command after stop_sonara() returned False."""
    print("Sonara did not stop, so nothing was changed (a running daemon "
          "locks its files). Run 'sonara shutdown', then try again.",
          file=sys.stderr)


def start_sonara() -> int:
    """Clear a previous shutdown and start the daemon (#23). This is the
    'sonara start' that doctor has always told users to run."""
    try:
        os.remove(str(paths.STOPPED_SENTINEL_PATH))
    except OSError:
        pass
    from sonara import lifecycle
    lifecycle.ensure_running()
    deadline = time.time() + 5.0
    while time.time() < deadline:
        if paths.socket_connectable():
            print("Sonara daemon is running.")
            return 0
        time.sleep(0.1)
    print("Start requested; the daemon is not accepting yet "
          "(check ~/.sonara/speechd.log).")
    return 1


def clear_stop_sentinel() -> None:
    """Remove the stop sentinel (best-effort)."""
    try:
        os.remove(str(paths.STOPPED_SENTINEL_PATH))
    except OSError:
        pass


def stopped_state_restorer():
    """Snapshot whether Sonara runs now, for a command that stops it and may
    then fail: the returned callable starts it again if it was running, or
    removes the stop sentinel the command's own stop_sonara wrote, so a
    failure never leaves Sonara off. An earlier explicit shutdown stays."""
    was_running = paths.socket_connectable()
    was_shut_down = os.path.exists(str(paths.STOPPED_SENTINEL_PATH))

    def restore():
        if was_running:
            start_sonara()
        elif not was_shut_down:
            clear_stop_sentinel()
    return restore
