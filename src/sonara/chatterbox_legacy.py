"""Leftovers of the removed Chatterbox engine (#134).

Chatterbox (the optional GPU voice-cloning engine) is gone; Kokoro and the
native Windows voices are the only engines. Two jobs remain:

- Migration: a saved Chatterbox voice would otherwise fall through to the
  native David/Zira voice with no notice, so it maps to Kokoro's Heart when
  config and session prefs load.
- Disk: the old venv, model cache and smoke-test files take several GB.
  `sonara doctor` reports them and `sonara cleanup` removes them on request.

The user's own recorded clips in CHATTERBOX_VOICES_DIR are only read here (to
recognise old voice names), never deleted.
"""
from __future__ import annotations

import os
import shutil
from pathlib import Path

from sonara import paths

REPLACEMENT_VOICE = "af_heart"
SETTING_PREFIX = "chatterbox_"
_BUILTIN_VOICE = "cb_default"


def _clip_stems() -> "set[str]":
    try:
        return {p.stem.lower() for p in Path(paths.CHATTERBOX_VOICES_DIR).glob("*.wav")}
    except OSError:
        return set()


def is_legacy_voice(name) -> bool:
    """True for a name the removed engine owned: `cb_default`, any
    `chatterbox:`-prefixed name, or the stem of a clip in the old voices folder.
    Kokoro names always won the old routing, so they are never legacy."""
    if not name:
        return False
    s = str(name).strip()
    engine, sep, rest = s.partition(":")
    if sep and engine.strip().lower() == "chatterbox":
        return True
    from sonara import kokoro
    if kokoro.is_kokoro_voice(s):
        return False
    return s.lower() == _BUILTIN_VOICE or s.lower() in _clip_stems()


def migrate_voice(name):
    """*name*, or Heart when it was a Chatterbox voice."""
    return REPLACEMENT_VOICE if is_legacy_voice(name) else name


def migrate_config(cfg: dict) -> dict:
    """In place: drop chatterbox_* settings and map Chatterbox voices to Heart."""
    for key in [k for k in cfg if str(k).startswith(SETTING_PREFIX)]:
        del cfg[key]
    for key in ("voice", "cue_voice"):
        if cfg.get(key):
            cfg[key] = migrate_voice(cfg[key])
    return cfg


# --- leftovers on disk -------------------------------------------------------

def _size(path: Path) -> int:
    if path.is_file():
        try:
            return path.stat().st_size
        except OSError:
            return 0
    total = 0
    for root, _dirs, files in os.walk(str(path)):
        for f in files:
            try:
                total += os.path.getsize(os.path.join(root, f))
            except OSError:
                pass
    return total


def leftovers() -> "list[tuple[Path, int]]":
    """(path, size in bytes) for every leftover that exists: the venv, the
    model cache and the smoke-test files. Never the voices folder."""
    found = []
    for p in (Path(paths.CHATTERBOX_VENV), Path(paths.CHATTERBOX_MODEL_CACHE)):
        if p.is_dir():
            found.append(p)
    root = Path(paths.SONARA_DIR)
    smoke = set()
    for pattern in paths.CHATTERBOX_SMOKE_GLOBS:
        try:
            smoke.update(p for p in root.glob(pattern) if p.is_file())
        except OSError:
            pass
    found.extend(sorted(smoke))
    return [(p, _size(p)) for p in found]


def remove_leftovers(rmtree=shutil.rmtree, unlink=os.unlink):
    """Delete every leftover. Returns (removed paths, [(path, error)]); one
    locked path does not stop the rest."""
    removed, failed = [], []
    for p, _size_bytes in leftovers():
        try:
            if p.is_dir():
                rmtree(str(p))
            else:
                unlink(str(p))
            removed.append(p)
        except OSError as exc:
            failed.append((p, exc))
    return removed, failed


def format_size(n: int) -> str:
    if n < 1024:
        return "{0} B".format(n)
    size = n / 1024.0
    for unit in ("KB", "MB"):
        if size < 1024:
            return "{0:.1f} {1}".format(size, unit)
        size /= 1024
    return "{0:.1f} GB".format(size)
