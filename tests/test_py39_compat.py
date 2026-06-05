"""Guard the public 3.9 target: every shipped module future-imports annotations,
and pyproject declares requires-python >= 3.9.

PEP 563 (the future import) defers annotation evaluation so `X | Y`-style hints
never run on 3.9, where they would raise at import time. cli.py already has it;
this test makes the rest of the package keep it.
"""
import os

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(REPO, "src", "sonari")

FUTURE = "from __future__ import annotations"


def _first_code_line(path):
    """Return the first non-blank, non-docstring source line of a module."""
    with open(path, encoding="utf-8") as f:
        text = f.read()
    lines = text.splitlines()
    i = 0
    # Skip leading blanks.
    while i < len(lines) and lines[i].strip() == "":
        i += 1
    # Skip a module docstring if present (single- or triple-quoted).
    if i < len(lines):
        s = lines[i].lstrip()
        for q in ('"""', "'''", '"', "'"):
            if s.startswith(q):
                # Find the closing quote (may be on the same line).
                rest = s[len(q):]
                if q in (('"""', "'''")) and rest.endswith(q) and len(rest) >= len(q):
                    i += 1
                elif q in ('"', "'") and rest.endswith(q):
                    i += 1
                else:
                    j = i + 1
                    while j < len(lines) and q not in lines[j]:
                        j += 1
                    i = j + 1
                break
    while i < len(lines) and lines[i].strip() == "":
        i += 1
    return lines[i].strip() if i < len(lines) else ""


def test_every_module_has_future_annotations():
    for name in os.listdir(SRC):
        if not name.endswith(".py"):
            continue
        path = os.path.join(SRC, name)
        assert _first_code_line(path) == FUTURE, (
            f"{name}: first code line must be {FUTURE!r}")


def test_pyproject_requires_python_39():
    pyproject = os.path.join(REPO, "pyproject.toml")
    with open(pyproject, encoding="utf-8") as f:
        text = f.read()
    assert 'requires-python = ">=3.9"' in text, (
        "pyproject must declare requires-python >= 3.9")
