"""Resolve the bundled earcon .wav paths for Windows.

Package layout::

    sonari/platform/windows/earcons/
        __init__.py        <- this file
        generate.py        <- stdlib WAV generator
        permission.wav
        choice.wav
        plan.wav
        error.wav
        turn_done.wav
        ready.wav

pyproject.toml declares these as package data::

    [tool.setuptools.package-data]
    sonari = ["platform/windows/earcons/*.wav"]
"""
from __future__ import annotations

import pathlib

# Derive the canonical name list directly from the generator so that adding
# or renaming an earcon in generate.py is automatically reflected here and
# caught by tests — rather than silently diverging until runtime.
from sonari.platform.windows.earcons.generate import _EARCON_SPECS

_EARCON_NAMES: tuple[str, ...] = tuple(_EARCON_SPECS.keys())

# Cache so we only resolve paths once per process
_cache: dict[str, str] = {}


def default_earcons() -> dict[str, str]:
    """Return {earcon_name: absolute_wav_path} for all bundled earcons.

    Resolution uses a plain ``pathlib.Path(__file__).parent / fname``
    lookup, which is correct and reliable for all standard install modes:
    editable installs, unpacked wheels, and sdist builds.

    Note: the ``importlib.resources.as_file()`` context-manager approach
    is NOT used here because for ZipPath-backed packages it deletes the
    extracted temporary file when the ``with`` block exits, leaving the
    cached path pointing at a nonexistent file.  Since earcons are static
    assets that ship alongside this module, a sibling-file lookup is both
    simpler and correct on every supported Python version (3.9+).

    Raises FileNotFoundError if an expected .wav is absent from the package
    (e.g. package data was not included in the distribution).
    """
    if _cache:
        return dict(_cache)

    _pkg_dir = pathlib.Path(__file__).parent

    for name in _EARCON_NAMES:
        fname = f"{name}.wav"
        resolved = str((_pkg_dir / fname).resolve())

        if not pathlib.Path(resolved).exists():
            raise FileNotFoundError(
                f"Bundled earcon not found: {resolved!r}\n"
                f"Run: python -m sonari.platform.windows.earcons.generate  "
                f"(then commit the .wav files)"
            )
        _cache[name] = resolved

    return dict(_cache)
