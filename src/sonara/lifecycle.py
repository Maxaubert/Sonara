"""Starting the daemon on demand. Hook clients, `sonara start` and the
settings page's restart respawner all call ensure_running(); it lives outside
sonara.daemon so none of them has to import the daemon itself."""
from __future__ import annotations

import os
import subprocess

from sonara.paths import socket_connectable


def ensure_running() -> None:
    from sonara import paths as _paths
    if os.path.exists(str(_paths.STOPPED_SENTINEL_PATH)):
        return   # explicitly shut down: hook events must not resurrect it (#23)
    if socket_connectable():
        return
    from sonara.platform import get_platform
    argv, kwargs = get_platform().supervisor.launch_spec()
    try:
        subprocess.Popen(argv, **kwargs)
    finally:
        # The child has its own copy of the log handle; the parent's is closed
        # here instead of leaking until this process exits (L-log).
        err = kwargs.get("stderr")
        if hasattr(err, "close"):
            try:
                err.close()
            except OSError:
                pass
