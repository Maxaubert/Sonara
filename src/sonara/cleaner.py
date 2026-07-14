"""Strip markdown noise from text so it reads naturally aloud.

PURE: no I/O. Does NOT handle triple-backtick fenced code blocks; the
ProseAssembler handles those before text reaches here.

Since the live-prose audit (#56) the assembler splits RAW text into chunks
first and cleans each chunk at emission, so these rules no longer need to be
prefix-stable against streaming re-cleans. The one rule that still runs on
raw pre-split text is stabilize_ordinals(), which therefore must stay
length-preserving.
"""
from __future__ import annotations

import re

# [label](url) -> label   (run BEFORE the bare-url rule)
_LINK = re.compile(r"\[([^\]\n]+)\]\((?:[^)\n]+)\)")
# inline code: `code` -> code  (drop the backticks, keep the text)
_INLINE_CODE = re.compile(r"`([^`\n]*)`")
# leading heading hashes at start of any line: "# ", "## " ... -> ""
_HEADING = re.compile(r"^#{1,6}\s+", flags=re.MULTILINE)
# a leading list-bullet marker: "- item" / "* item" / "+ item" / "• item" -> "item".
# Closing bullet lists are the most common shape of a turn's last paragraph;
# spoken markers ("dash fixed the race dash added tests") read as noise (#56).
_BULLET = re.compile(r"^(\s*)[-*+•][ \t]+", flags=re.MULTILINE)
# bold/italic markers around a run of text -> the text (1-3 of * or _)
_EMPHASIS = re.compile(r"(\*{1,3}|_{1,3})([^*_\n]+)\1")
# a bare http/https url -> the word "link". The lazy body + lookahead leaves
# trailing sentence punctuation in place; \S+ used to eat the terminator and
# merge two sentences into one run-on (#56).
_BARE_URL = re.compile(r"https?://\S+?(?=[.,;:!?)\]]*(?:\s|$))")
# a markdown table separator row, e.g. |---|---| or | :--- | ---: |
_TABLE_SEP = re.compile(r"^\s*\|?[\s:|-]*-{3,}[\s:|-]*\|?\s*$", flags=re.MULTILINE)
# numbered-list ordinal: "2. Run" -> "2: Run". The sentence splitter treats the
# ordinal's dot as a sentence END, chunking "Install the package 2." then "Run
# the tests" -- the ordinal attached to the WRONG spoken item (deep audit #25).
_LIST_ORDINAL = re.compile(r"^(\s*)(\d{1,3})\.(\s+)", flags=re.MULTILINE)
# a COMPLETE (newline-terminated) list-item line with no terminal punctuation
# gets a period, so each item reads as its own chunk.
_LIST_ITEM_END = re.compile(r"^(\s*\d{1,3}: .*[^\s.!?:;])[ \t]*\n",
                            flags=re.MULTILINE)
# any run of whitespace (spaces, tabs, newlines) -> a single space
_WHITESPACE = re.compile(r"\s+")


def stabilize_ordinals(text: str) -> str:
    """Length-preserving 'N. ' -> 'N: ' rewrite for numbered list items.

    Applied by the assembler to RAW text BEFORE sentence splitting, so an
    ordinal's dot never reads as a sentence end. Length preservation keeps the
    assembler's raw-offset bookkeeping valid (#56); do not add rules here that
    change the text's length."""
    return _LIST_ORDINAL.sub(r"\1\2:\3", text)


def clean_markdown(text: str) -> str:
    # links first so the embedded url is gone before _BARE_URL runs
    text = _LINK.sub(r"\1", text)
    text = _INLINE_CODE.sub(r"\1", text)
    text = _HEADING.sub("", text)
    text = _BULLET.sub(r"\1", text)
    # apply emphasis twice to peel nested markers like ***x***
    text = _EMPHASIS.sub(r"\2", text)
    text = _EMPHASIS.sub(r"\2", text)
    text = _BARE_URL.sub("link", text)
    text = _TABLE_SEP.sub(" ", text)
    text = stabilize_ordinals(text)
    text = _LIST_ITEM_END.sub(r"\1.\n", text)
    text = _WHITESPACE.sub(" ", text)
    return text.strip()


# --- speech normalization (#27, extended by #56) ------------------------------
# snake_case -> spaced words; arrows -> "to"; " & " -> " and "; stray markdown
# characters stripped. Originally digest-only (#27); since the live-prose audit
# (#56) the assembler routes every live chunk through normalize_for_speech too,
# so code identifiers and symbols never reach the voice raw on either path.
# Leading underscores allowed so "_voiced_upto" unsnakes instead of gluing.
_SNAKE = re.compile(r"(?<![A-Za-z0-9_])_{0,2}[A-Za-z][A-Za-z0-9]*(?:_[A-Za-z0-9]+)+(?![A-Za-z0-9_])")
_ARROW = re.compile(r"\s*(?:->|=>|-->|→)\s*")
_STRAY_MD = re.compile(r"[*_`~#|•✓✔✗✘❌✅]+")


def normalize_for_speech(text: str) -> str:
    """TTS-normalize text so symbols never reach the voice (#27, #56)."""
    # unsnake FIRST: clean_markdown's emphasis rule eats intraword underscores
    # ("get_user_id" -> "getuserid"), which would glue the words together.
    text = _SNAKE.sub(lambda m: m.group(0).replace("_", " "), text or "")
    text = clean_markdown(text)
    text = _ARROW.sub(" to ", text)
    text = text.replace(" & ", " and ")
    text = _STRAY_MD.sub(" ", text)
    return _WHITESPACE.sub(" ", text).strip()
