"""`sonara cleanup`: remove the removed Chatterbox engine's leftovers (#134)."""
from __future__ import annotations

import sys

from sonara import paths
from sonara.install import service


def cleanup() -> int:
    """Remove the removed Chatterbox engine's leftovers (#134): its venv, model
    cache and smoke-test files. voices/chatterbox, the user's own recorded
    clips, is never touched.

    The daemon is stopped first: a still-running Chatterbox worker locks files
    in the venv, and deleting it live failed partway. It is started again only
    if it was running, and an earlier explicit shutdown stays in place."""
    from sonara import chatterbox_legacy as cl
    found = cl.leftovers()
    if not found:
        print("Nothing to clean up: no Chatterbox leftovers in {0}.".format(
            paths.SONARA_DIR))
        return 0
    total = sum(size for _p, size in found)
    restore = service.stopped_state_restorer()
    if not service.stop_sonara():
        # stop_sonara already wrote the sentinel and ended the task: undo
        # that, or a daemon that later exits would never come back.
        restore()
        print("Sonara did not stop, so nothing was removed (a running worker "
              "would lock the files). Run 'sonara shutdown', then try again.",
              file=sys.stderr)
        return 1
    removed, failed = cl.remove_leftovers()
    for p in removed:
        print("Removed {0}".format(p))
    for p, exc in failed:
        print("Could not remove {0}: {1}".format(p, exc), file=sys.stderr)
    restore()
    if failed:
        return 1
    print("Freed {0}. Your voice clips in {1} were kept.".format(
        cl.format_size(total), paths.CHATTERBOX_VOICES_DIR))
    return 0
