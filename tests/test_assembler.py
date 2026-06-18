from sonari.assembler import ProseAssembler


def test_two_sentences_across_two_feeds_emit_both_and_hold_nothing():
    a = ProseAssembler()
    out1 = a.feed("Hello world. Second one. ", 0, False)
    assert out1 == ["Hello world.", "Second one."]
    # nothing partial held back; a final flush yields nothing
    assert a.feed("", 1, True) == []


def test_partial_is_buffered_until_completed_by_later_delta():
    a = ProseAssembler()
    assert a.feed("No terminator yet", 0, False) == []
    assert a.feed(" but now it ends.", 1, False) == ["No terminator yet but now it ends."]


def test_partial_is_flushed_on_final():
    a = ProseAssembler()
    assert a.feed("No terminator yet", 0, False) == []
    assert a.feed("", 1, True) == ["No terminator yet"]


def test_repeated_index_is_ignored():
    a = ProseAssembler()
    assert a.feed("Hello world. ", 0, False) == ["Hello world."]
    # same index again: must be ignored, no duplicate emission
    assert a.feed("Hello world. ", 0, False) == []


def test_two_sentences_in_single_delta_emit_both():
    a = ProseAssembler()
    assert a.feed("First sentence! Second sentence?", 0, True) == [
        "First sentence!",
        "Second sentence?",
    ]


def test_final_resets_state_for_reuse():
    a = ProseAssembler()
    assert a.feed("Leftover text", 0, True) == ["Leftover text"]
    # after reset, index 0 is fresh again and buffer is empty
    assert a.feed("Brand new. ", 0, False) == ["Brand new."]


def test_fence_with_info_string_emits_lang_code_block_summary():
    a = ProseAssembler()
    delta = "```python\nline one\nline two\nline three\n```"
    assert a.feed(delta, 0, True) == ["3-line python code block"]


def test_fence_without_info_string_emits_plain_code_block_summary():
    a = ProseAssembler()
    delta = "```\nalpha\nbeta\n```"
    assert a.feed(delta, 0, True) == ["2-line code block"]


def test_fence_suppresses_code_and_keeps_surrounding_prose():
    a = ProseAssembler()
    delta = "Here it is. ```python\nx = 1\ny = 2\n``` Done now."
    out = a.feed(delta, 0, True)
    assert out == ["Here it is.", "2-line python code block", "Done now."]


def test_fence_spanning_multiple_feed_calls_emits_n_line_summary():
    """A code fence whose open/body/close arrive in separate feed() calls must
    produce the correct N-line summary (not prematurely closed or dropped)."""
    a = ProseAssembler()

    # Delta 0: opening fence marker + language tag
    out0 = a.feed("```python\n", 0, False)
    # The fence is open; no summary yet.
    assert out0 == []

    # Delta 1: first body line
    out1 = a.feed("x = 1\n", 1, False)
    assert out1 == []

    # Delta 2: second body line
    out2 = a.feed("y = 2\n", 2, False)
    assert out2 == []

    # Delta 3: third body line
    out3 = a.feed("z = 3\n", 3, False)
    assert out3 == []

    # Delta 4: closing fence — summary must fire here
    out4 = a.feed("```", 4, True)
    assert out4 == ["3-line python code block"]


def test_paragraph_break_emitted_between_paragraphs():
    from sonari.assembler import ProseAssembler, PARAGRAPH_BREAK
    a = ProseAssembler()
    out = a.feed("First paragraph here.\n\nSecond paragraph here.", 0, True)
    assert "First paragraph here." in out and "Second paragraph here." in out
    assert PARAGRAPH_BREAK in out
    assert out.index("First paragraph here.") < out.index(PARAGRAPH_BREAK) < out.index("Second paragraph here.")


def test_no_paragraph_break_within_one_paragraph():
    from sonari.assembler import ProseAssembler, PARAGRAPH_BREAK
    a = ProseAssembler()
    out = a.feed("One sentence. Two sentences. Still one paragraph.", 0, True)
    assert PARAGRAPH_BREAK not in out


def test_three_paragraphs_two_breaks():
    from sonari.assembler import ProseAssembler, PARAGRAPH_BREAK
    a = ProseAssembler()
    out = a.feed("Para one.\n\nPara two.\n\nPara three.", 0, True)
    assert out.count(PARAGRAPH_BREAK) == 2


def test_blank_line_split_across_deltas_still_breaks_paragraphs():
    """A blank line (\\n\\n) straddling two streamed deltas must still produce a
    PARAGRAPH_BREAK and must not merge the two paragraphs. Regression: the buffer
    was overwritten with whitespace-collapsed text between deltas, erasing the
    trailing newline of a straddling blank line, so the heading merged into the
    next paragraph and reading stalled/garbled at every blank line."""
    from sonari.assembler import ProseAssembler, PARAGRAPH_BREAK
    a = ProseAssembler()
    out = []
    out += a.feed("The headline issue\n", 0, False)        # heading, no period, ends with \n
    out += a.feed("\nThe next part is here.\n", 1, False)   # starts with \n -> blank line straddles
    out += a.feed("", 2, True)
    texts = [c for c in out if c is not PARAGRAPH_BREAK]
    assert "The headline issue" in texts, f"heading not emitted as its own chunk: {texts}"
    assert any("next part is here" in t for t in texts), texts
    assert PARAGRAPH_BREAK in out, "paragraph break lost across deltas"
    assert out.index("The headline issue") < out.index(PARAGRAPH_BREAK)
    # no word-joining across the lost newline/space
    assert not any("issueThe" in t or "issue The next" in t for t in texts), texts
