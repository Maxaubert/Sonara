"""Strip markdown noise from text so it reads naturally aloud.

PURE: no I/O. Does NOT handle triple-backtick fenced code blocks; the
ProseAssembler handles those before text reaches here.
"""
from __future__ import annotations

import re

# [label](url) -> label   (run BEFORE the bare-url rule)
_LINK = re.compile(r"\[([^\]\n]+)\]\((?:[^)\n]+)\)")
# inline code: `code` -> code  (drop the backticks, keep the text)
_INLINE_CODE = re.compile(r"`([^`\n]*)`")
# leading heading hashes at start of any line: "# ", "## " ... -> ""
_HEADING = re.compile(r"^#{1,6}\s+", flags=re.MULTILINE)
# bold/italic markers around a run of text -> the text (1-3 of * or _)
_EMPHASIS = re.compile(r"(\*{1,3}|_{1,3})([^*_\n]+)\1")
# a bare http/https url -> the word "link"
_BARE_URL = re.compile(r"https?://\S+")
# a markdown table separator row, e.g. |---|---| or | :--- | ---: |
_TABLE_SEP = re.compile(r"^\s*\|?[\s:|-]*-{3,}[\s:|-]*\|?\s*$", flags=re.MULTILINE)
# numbered-list ordinal: "2. Run" -> "2: Run". The sentence splitter treats the
# ordinal's dot as a sentence END, chunking "Install the package 2." then "Run
# the tests" -- the ordinal attached to the WRONG spoken item (deep audit #25).
# Applied as soon as "N. " appears at line start, so it is prefix-stable while
# the line is still streaming in.
_LIST_ORDINAL = re.compile(r"^(\s*)(\d{1,3})\.(\s+)", flags=re.MULTILINE)
# a COMPLETE (newline-terminated) list-item line with no terminal punctuation
# gets a period, so each item reads as its own chunk. Only fires once the line
# has its newline -> monotonic, never destabilizes an already-emitted prefix.
_LIST_ITEM_END = re.compile(r"^(\s*\d{1,3}: .*[^\s.!?:;])[ \t]*\n",
                            flags=re.MULTILINE)
# any run of whitespace (spaces, tabs, newlines) -> a single space
_WHITESPACE = re.compile(r"\s+")


def clean_markdown(text: str) -> str:
    # links first so the embedded url is gone before _BARE_URL runs
    text = _LINK.sub(r"\1", text)
    text = _INLINE_CODE.sub(r"\1", text)
    text = _HEADING.sub("", text)
    # apply emphasis twice to peel nested markers like ***x***
    text = _EMPHASIS.sub(r"\2", text)
    text = _EMPHASIS.sub(r"\2", text)
    text = _BARE_URL.sub("link", text)
    text = _TABLE_SEP.sub(" ", text)
    text = _LIST_ORDINAL.sub(r"\1\2:\3", text)
    text = _LIST_ITEM_END.sub(r"\1.\n", text)
    text = _WHITESPACE.sub(" ", text)
    return text.strip()
