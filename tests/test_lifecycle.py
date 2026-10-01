"""ensure_running lives in sonara.lifecycle (#141): every hook process imports
the client, and the client used to import the whole daemon just to reach it
(about a third of the hook's import time, audit section 2)."""
import os
import subprocess
import sys

SRC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src")


def test_hook_client_imports_do_not_load_the_daemon():
    code = ("import sys, sonara.client, sonara.hooks_entry; "
            "print('sonara.daemon' in sys.modules)")
    env = dict(os.environ, PYTHONPATH=SRC)
    out = subprocess.run([sys.executable, "-c", code], env=env,
                         capture_output=True, text=True, timeout=60)
    assert out.returncode == 0, out.stderr
    assert out.stdout.strip() == "False"


def test_daemon_still_exports_ensure_running_for_old_callers():
    from sonara import daemon, lifecycle
    assert daemon.ensure_running is lifecycle.ensure_running
