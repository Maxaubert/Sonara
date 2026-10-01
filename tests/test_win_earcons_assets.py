"""The bundled earcon set equals the set of kinds Sonara plays (#136, audit H2).

The daemon used to fire nav, nav_edge, session_change and summary_failed with
no bundled wav (Speaker.earcon silently skips a missing kind), while plan and
ready wavs shipped with no caller. Both directions are checked here by scanning
the source for every earcon kind literal.
"""
from __future__ import annotations

import ast
import pathlib

from sonara.platform.windows.earcons import default_earcons, _cache
from sonara.platform.windows.earcons.generate import _EARCON_SPECS

_SRC = pathlib.Path(__file__).resolve().parent.parent / "src" / "sonara"
_EARCON_DIR = _SRC / "platform" / "windows" / "earcons"


def _strings(node) -> "set[str]":
    return {n.value for n in ast.walk(node)
            if isinstance(n, ast.Constant) and isinstance(n.value, str)}


def _is_earcon_msg_type(node) -> bool:
    # MsgType.EARCON (hooks_entry builds EARCON messages with kind="...")
    return isinstance(node, ast.Attribute) and node.attr == "EARCON"


def played_earcon_kinds() -> "set[str]":
    """Every literal passed to _earcon(...), plus every kind= literal on an
    EARCON protocol message (the daemon plays those through _earcon(kind))."""
    kinds = set()
    for path in _SRC.rglob("*.py"):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            func = node.func
            name = getattr(func, "attr", None) or getattr(func, "id", None)
            if name == "_earcon" and node.args:
                kinds |= _strings(node.args[0])
            kw = {k.arg: k.value for k in node.keywords if k.arg}
            if "kind" in kw and _is_earcon_msg_type(kw.get("type")):
                kinds |= _strings(kw["kind"])
    return kinds


def test_scan_finds_the_known_callers():
    # Guard the scanner itself: an AST shape change must not make it vacuous.
    kinds = played_earcon_kinds()
    assert {"choice", "permission", "turn_done", "nav", "error"} <= kinds


def test_bundled_wavs_equal_played_kinds():
    wavs = {p.stem for p in _EARCON_DIR.glob("*.wav")}
    assert wavs == played_earcon_kinds()


def test_generator_specs_equal_bundled_wavs():
    assert set(_EARCON_SPECS) == {p.stem for p in _EARCON_DIR.glob("*.wav")}


def test_default_earcons_resolve_every_played_kind():
    _cache.clear()
    earcons = default_earcons()
    assert set(earcons) == played_earcon_kinds()
    for name, path in earcons.items():
        assert pathlib.Path(path).exists(), f"Earcon {name!r} path does not exist: {path!r}"
