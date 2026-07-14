"""Assemble streamed text deltas into complete, speakable chunks.

PURE: no I/O. Splits prose into sentences and replaces triple-backtick
fenced code blocks with a spoken one-line summary.
"""
from __future__ import annotations

import re

from sonara.cleaner import clean_markdown

_FENCE = "```"
# a complete sentence ends at . ! or ? followed by whitespace or end-of-string
_SENTENCE = re.compile(r"(.+?[.!?])(?:\s+|$)", flags=re.DOTALL)

# A paragraph boundary = a blank line. We split the RAW buffer on this (before
# clean_markdown collapses whitespace) so the boundary survives even when the
# blank line straddles two streamed deltas. feed() yields PARAGRAPH_BREAK between
# paragraphs so the daemon can group history by paragraph (the nav 'item' unit).
_PARA = re.compile(r"\n[ \t]*\n")

# Sentinel object emitted in the feed() output stream between paragraphs.
PARAGRAPH_BREAK = object()


class ProseAssembler:
    def __init__(self) -> None:
        self._seen: set[int] = set()
        self._buf = ""                 # pending prose text (RAW, outside fences)
        self._emitted = 0              # chars of the CURRENT paragraph's CLEANED text already emitted
        self._pending = ""             # raw tail not yet split into a line/fence token
        self._in_fence = False
        self._fence_lang = ""
        self._fence_lines: list[str] = []
        self._fence_opened_line = False  # have we consumed the opening info-string line?

    def feed(self, delta: str, index: int, final: bool) -> list[str]:
        out: list[str] = []
        if index in self._seen:
            if index == 0:
                # A NEW message block restarts at index 0. A lost final from the
                # previous block would otherwise leave _seen poisoned and every
                # colliding delta of the new block silently DROPPED (deep audit
                # #25). Flush the stale block and start fresh; a true duplicate
                # first delta re-speaks at worst, never drops.
                out.extend(self._consume(force=True))
                out.extend(self._flush_prose())
                self._reset()
            else:
                # still honor a final flush even on a duplicate index
                if final:
                    out.extend(self._flush_prose())
                    self._reset()
                return out
        self._seen.add(index)

        self._pending += delta
        out.extend(self._consume())

        if final:
            out.extend(self._consume(force=True))
            out.extend(self._flush_prose())
            self._reset()
        return out

    def _consume(self, force: bool = False) -> list[str]:
        """Scan _pending for fence boundaries, routing text to prose or fence.

        Only acts on text we can resolve: a fence marker, or (inside a fence)
        a complete line. Leftover ambiguous tail stays in _pending unless force.
        """
        out: list[str] = []
        while True:
            if self._in_fence:
                nl = self._pending.find("\n")
                if nl != -1:
                    line = self._pending[:nl]
                    self._pending = self._pending[nl + 1:]
                    stripped = line.strip()
                    # A closing fence STARTS its line with >= 3 backticks and is
                    # followed by nothing (or whitespace + trailing prose, which
                    # resumes as prose). Matching ``` ANYWHERE closed early on
                    # code that CONTAINS a ``` literal, leaking code lines as
                    # spoken prose (deep audit #25). Backticks followed directly
                    # by a word ("```python") are content, not a close.
                    close_rest = self._closing_fence_rest(stripped)
                    if close_rest is not None:
                        out.append(self._close_fence())
                        if close_rest:
                            self._pending = close_rest + "\n" + self._pending
                        continue
                    if not self._fence_opened_line:
                        # first line after opening ``` is the info string
                        self._fence_lang = stripped
                        self._fence_opened_line = True
                    else:
                        self._fence_lines.append(line)
                    continue
                # no complete line yet
                if force:
                    # unterminated fence at EOF: the tail is either a
                    # (newline-less) closing fence or a final content line
                    tail = self._pending
                    self._pending = ""
                    stripped = tail.strip()
                    close_rest = self._closing_fence_rest(stripped)
                    if close_rest is not None:
                        out.append(self._close_fence())
                        if close_rest:
                            self._pending = close_rest
                            continue        # resume prose scan (still forced)
                    else:
                        if stripped:
                            if self._fence_opened_line:
                                self._fence_lines.append(tail)
                            else:
                                self._fence_lang = stripped
                        out.append(self._close_fence())
                break
            else:
                open_at = self._pending.find(_FENCE)
                if open_at != -1:
                    prose = self._pending[:open_at]
                    self._buf += prose
                    self._pending = self._pending[open_at + len(_FENCE):]
                    out.extend(self._split_sentences())
                    # An unterminated lead-in ("Here is the code:") must be
                    # spoken BEFORE the fence summary -- it used to sit as the
                    # buffered remainder until the final flush and play AFTER
                    # the block it introduced (deep audit #25). The fence is a
                    # hard boundary, so the lead-in is complete: flush it now.
                    out.extend(self._flush_prose())
                    self._in_fence = True
                    self._fence_opened_line = False
                    self._fence_lang = ""
                    self._fence_lines = []
                    continue
                # no fence opening visible
                if force:
                    self._buf += self._pending
                    self._pending = ""
                    out.extend(self._split_sentences())
                else:
                    # hold back only enough tail to detect a future "```";
                    # commit everything except a possible partial fence marker
                    keep = self._partial_fence_tail_len()
                    if keep:
                        commit = self._pending[:-keep]
                        self._pending = self._pending[-keep:]
                    else:
                        commit = self._pending
                        self._pending = ""
                    if commit:
                        self._buf += commit
                        out.extend(self._split_sentences())
                break
        return out

    def _closing_fence_rest(self, stripped: str):
        """If *stripped* (a fence-interior line) is a closing fence, return the
        trailing prose after it ('' if none); else None. A close = the line
        STARTS with >= 3 backticks followed by end-of-line or whitespace; a
        word glued to the backticks ('```python') is content (deep audit #25).
        Only valid once the info-string line was consumed."""
        if not self._fence_opened_line or not stripped.startswith(_FENCE):
            return None
        ticks = len(stripped) - len(stripped.lstrip("`"))
        if ticks < 3:
            return None
        rest = stripped[ticks:]
        if rest and rest[0] not in (" ", "\t"):
            return None
        return rest.strip()

    def _partial_fence_tail_len(self) -> int:
        """How many trailing chars of _pending could be the start of a fence."""
        for n in (2, 1):
            if self._pending.endswith("`" * n):
                return n
        return 0

    def _close_fence(self) -> str:
        n = len(self._fence_lines)
        lang = self._fence_lang
        self._in_fence = False
        self._fence_opened_line = False
        self._fence_lang = ""
        self._fence_lines = []
        if lang:
            return f"{n}-line {lang} code block"
        return f"{n}-line code block"

    def _sentences_of(self, text: str, keep_remainder: bool):
        """Split *text* into sentences. Return (sentences, remainder). When
        keep_remainder is False the trailing fragment is emitted too (a complete
        paragraph) and remainder is ''."""
        out: list = []
        last_end = 0
        for m in _SENTENCE.finditer(text):
            sentence = m.group(1).strip()
            if len(sentence) > 1:
                out.append(sentence)
            last_end = m.end()
        remainder = text[last_end:]
        if not keep_remainder:
            tail = remainder.strip()
            if len(tail) > 1:
                out.append(tail)
            remainder = ""
        return out, remainder

    def _split_sentences(self) -> list:
        """Emit complete sentences from _buf, with PARAGRAPH_BREAK markers between
        paragraphs (blank-line boundaries). Keeps the trailing partial sentence.

        _buf is kept RAW (uncleaned). Cleaning collapses whitespace, so storing the
        cleaned remainder used to erase the trailing newline of a blank line that
        straddles two streamed deltas -- the break was lost and the paragraphs
        merged. Instead we split the RAW buffer on blank lines (preserving the
        straddling newline for the next delta) and track how much of the current
        paragraph's CLEANED text has already been emitted (_emitted), so re-cleaning
        the growing raw buffer never re-emits or drops a sentence."""
        out: list = []
        raw_paragraphs = _PARA.split(self._buf)
        # All but the last are COMPLETE paragraphs (each was followed by a blank line).
        for raw_para in raw_paragraphs[:-1]:
            cleaned = clean_markdown(raw_para)
            start = min(self._emitted, len(cleaned))
            sents, _ = self._sentences_of(cleaned[start:], keep_remainder=False)
            out.extend(sents)
            out.append(PARAGRAPH_BREAK)
            self._emitted = 0                # paragraph done; the next one starts fresh
        # The last raw paragraph is the current, possibly-incomplete one.
        last_raw = raw_paragraphs[-1]
        cleaned_last = clean_markdown(last_raw)
        start = min(self._emitted, len(cleaned_last))
        sents, remainder = self._sentences_of(cleaned_last[start:], keep_remainder=True)
        out.extend(sents)
        self._emitted = len(cleaned_last) - len(remainder)
        self._buf = last_raw                 # keep RAW so a straddling blank line survives
        return out

    def _flush_prose(self) -> list[str]:
        if not self._buf:
            self._emitted = 0
            return []
        cleaned = clean_markdown(self._buf)
        start = min(self._emitted, len(cleaned))
        tail = cleaned[start:]               # only the not-yet-emitted remainder
        self._buf = ""
        self._emitted = 0
        if len(tail) > 1:
            return [tail]
        return []

    def _reset(self) -> None:
        self._seen = set()
        self._buf = ""
        self._emitted = 0
        self._pending = ""
        self._in_fence = False
        self._fence_lang = ""
        self._fence_lines = []
        self._fence_opened_line = False
