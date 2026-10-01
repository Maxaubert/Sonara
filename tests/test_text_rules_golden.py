"""Golden text-rule cases (#143): every tests/fixtures/text_rules/*.json case
is run through the Python cleaner or assembler and must match its recorded
output. The same files are the contract a later TypeScript port is checked
against, so a change here is a change to that contract: update the fixture
deliberately, never to make a refactor pass. Format: README.md beside them."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from sonara.assembler import PARAGRAPH_BREAK, ProseAssembler
from sonara.cleaner import clean_markdown, normalize_for_speech

RULES_DIR = Path(__file__).parent / "fixtures" / "text_rules"

_STRING_FNS = {
    "clean_markdown": clean_markdown,
    "normalize_for_speech": normalize_for_speech,
}


def _assemble(deltas):
    """Feed *deltas* as one message block (index i, final on the last) and
    return every emitted chunk, with a paragraph break as None (JSON null)."""
    a = ProseAssembler()
    out = []
    for i, delta in enumerate(deltas):
        for chunk in a.feed(delta, i, i == len(deltas) - 1):
            out.append(None if chunk is PARAGRAPH_BREAK else chunk)
    return out


def _cases():
    out = []
    for path in sorted(RULES_DIR.glob("*.json")):
        data = json.loads(path.read_text(encoding="utf-8"))
        for case in data["cases"]:
            out.append(pytest.param(case, id="{0}::{1}".format(path.stem, case["name"])))
    return out


def test_golden_files_cover_every_category():
    stems = {p.stem for p in RULES_DIR.glob("*.json")}
    assert {"markdown", "code_blocks", "lists", "links", "emoji",
            "numbers"} <= stems


@pytest.mark.parametrize("case", _cases())
def test_text_rule_case(case):
    fn = case["fn"]
    if fn == "assemble":
        assert _assemble(case["deltas"]) == case["output"]
    else:
        assert fn in _STRING_FNS, "unknown fn {0!r}".format(fn)
        assert _STRING_FNS[fn](case["input"]) == case["output"]
