"""The Rust agent layer (crates/sonara-agent) carries copies of two Python
contracts until the Python plugin is removed (M11): the summarizer's
built-in instructions and the bundled earcon names. Keep them equal."""
from __future__ import annotations

import re
from pathlib import Path

from sonara import summarizer
from sonara.platform.windows.earcons.generate import _EARCON_SPECS

AGENT = Path(__file__).resolve().parent.parent / "crates" / "sonara-agent"


def test_summary_instructions_match_the_rust_copies():
    for style, text in summarizer.INSTRUCTIONS.items():
        copy = (AGENT / "prompts" / f"{style}.txt").read_bytes().decode("utf-8")
        assert copy.replace("\r\n", "\n") == text, style


def test_earcon_names_match_the_rust_list():
    src = (AGENT / "src" / "earcon.rs").read_text(encoding="utf-8")
    names = re.findall(r'^\s*[A-Z][A-Za-z]* => "([a-z_]+)",\s*$', src, flags=re.M)
    assert sorted(names) == sorted(_EARCON_SPECS)
