"""Windows earcon backend — winsound.PlaySound with poll()-able handles.

winsound is Windows-only; imported lazily (guarded try/except ModuleNotFoundError)
so this module is importable on macOS/Linux for tests via the _winfakes harness.
"""
from __future__ import annotations

import pathlib

from sonara.platform.base import EarconBackend

# winsound is Windows-only; imported lazily so the module is importable
# on macOS/Linux (for tests / dev).
try:
    import winsound as _winsound
except ModuleNotFoundError:  # non-Windows; reached at import-time when winsound is absent
    _winsound = None  # type: ignore[assignment]


class WinEarconBackend(EarconBackend):
    """Earcon backend for Windows using winsound.PlaySound."""

    # CREATE_NO_WINDOW | DETACHED_PROCESS — windowless, no console flash.
    _SPAWN_FLAGS = 0x08000000 | 0x00000008

    def play(self, path: str):
        """Play *path* in a SEPARATE, windowless helper process.

        The daemon plays SPEECH on winsound, which is a single channel — playing
        an earcon on the SAME process's winsound would purge the speech. A
        separate process has its own audio session, so the earcon MIXES with the
        speech (shared-mode audio) and plays simultaneously without cutting it.

        Returns the Popen handle (has .poll()), or None if the file is missing.
        """
        if not pathlib.Path(path).exists():
            return None
        import subprocess
        import sys
        try:
            return subprocess.Popen(
                [sys.executable, "-c",
                 "import winsound,sys;winsound.PlaySound(sys.argv[1],winsound.SND_FILENAME)",
                 path],
                creationflags=self._SPAWN_FLAGS,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except Exception:  # noqa: BLE001 - a failed earcon spawn must never crash
            # the caller, but for an eyes-free user a SILENT missing earcon is
            # also untraceable. Log the traceback (daemon routes stderr -> the
            # log) so the failure is diagnosable, then preserve the None contract.
            import traceback
            traceback.print_exc(file=sys.stderr)
            return None

    def default_earcons(self) -> dict:
        """Return the platform's default {kind: sound_path} mapping."""
        from sonara.platform.windows.earcons import default_earcons
        return default_earcons()
